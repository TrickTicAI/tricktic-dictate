//! Single-process file store. IDs are UUIDs, never client filesystem paths.
use super::{AudioReader, RecordingStore};
use crate::{providers::SavedAudio, transcription::*};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{Mutex, RwLock, Semaphore},
};
use uuid::Uuid;

pub(crate) fn error(code: ErrorCode, message: &str, retryable: bool) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
        retry_after_ms: None,
    }
}
pub(crate) fn storage(_: impl std::fmt::Display) -> ApiError {
    error(ErrorCode::StorageFailed, "Storage operation failed; check free space and data-directory permissions. Retained audio is not removed.", true)
}
pub(crate) async fn private_dir(path: &Path) -> Result<(), ApiError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).await.map_err(storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(storage)?;
    }
    Ok(())
}
pub(crate) async fn private_file(path: &Path) -> Result<fs::File, ApiError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).await.map_err(storage)
}
pub(crate) async fn sync_dir(path: &Path) -> Result<(), ApiError> {
    // Windows does not support FlushFileBuffers on ordinary directory handles.
    // File contents are flushed before same-volume replacement; do not pretend
    // that this supplies Unix directory-fsync/power-loss guarantees on Windows.
    #[cfg(unix)]
    fs::File::open(path)
        .await
        .map_err(storage)?
        .sync_all()
        .await
        .map_err(storage)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub(crate) async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ApiError> {
    let temp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut file = private_file(&temp).await?;
    file.write_all(&serde_json::to_vec(value).map_err(storage)?)
        .await
        .map_err(storage)?;
    file.sync_all().await.map_err(storage)?;
    drop(file); // Windows replacement must not retain our writer handle.
    replace_file(&temp, path).await?;
    sync_dir(path.parent().unwrap()).await
}
pub(crate) async fn replace_file(source: &Path, destination: &Path) -> Result<(), ApiError> {
    // std's Windows rename uses MoveFileExW(REPLACE_EXISTING). Both files are
    // siblings, so replacement never crosses a volume or deletes the old file first.
    fs::rename(source, destination).await.map_err(storage)
}
pub(crate) async fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    serde_json::from_slice(&fs::read(path).await.map_err(storage)?).map_err(storage)
}

/// Ordered byte extents, NOT independently decodable ASR segments.
#[derive(Debug, Serialize, Deserialize)]
pub struct AudioExtent {
    pub sequence: u64,
    pub byte_offset: u64,
    pub byte_length: u64,
}

pub struct FileRecordingStore {
    pub(super) root: PathBuf,
    pub(super) quota_root: PathBuf,
    pub(super) lock: Mutex<()>,
    pub(super) activity: RwLock<()>,
    pub(super) uploads: std::sync::Arc<Semaphore>,
    pub(crate) budget: std::sync::Arc<crate::limits::storage::StorageBudget>,
    pub max_upload_bytes: u64,
}
impl FileRecordingStore {
    pub async fn open(root: PathBuf, max_upload_bytes: u64) -> Result<Self, ApiError> {
        let limits = crate::limits::Limits::from_env()?;
        let budget = crate::limits::storage::StorageBudget::new(root.clone(), limits);
        Self::open_with_budget(root, max_upload_bytes, budget).await
    }
    pub(crate) async fn open_with_budget(
        root: PathBuf,
        max_upload_bytes: u64,
        budget: std::sync::Arc<crate::limits::storage::StorageBudget>,
    ) -> Result<Self, ApiError> {
        Self::open_scoped(root.clone(), max_upload_bytes, budget, root).await
    }
    pub(crate) async fn open_scoped(
        root: PathBuf,
        max_upload_bytes: u64,
        budget: std::sync::Arc<crate::limits::storage::StorageBudget>,
        quota_root: PathBuf,
    ) -> Result<Self, ApiError> {
        private_dir(&root).await?;
        // Only uncommitted crash leftovers, after the pipeline acquired its lease.
        let mut entries = fs::read_dir(&root).await.map_err(storage)?;
        while let Some(entry) = entries.next_entry().await.map_err(storage)? {
            if entry
                .file_name()
                .to_str()
                .and_then(|s| s.strip_suffix(".uploading"))
                .is_some_and(|s| s.parse::<Uuid>().is_ok())
                && entry.file_type().await.map_err(storage)?.is_dir()
                && !fs::try_exists(entry.path().join("upload.json"))
                    .await
                    .map_err(storage)?
            {
                fs::remove_dir_all(entry.path()).await.map_err(storage)?;
            }
        }
        Ok(Self {
            root,
            quota_root,
            lock: Mutex::new(()),
            activity: RwLock::new(()),
            uploads: std::sync::Arc::new(Semaphore::new(budget.limits.device_uploads)),
            budget,
            max_upload_bytes,
        })
    }
    pub(super) fn dir(&self, id: Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }
    pub(super) fn partial(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.uploading"))
    }
    pub fn validate_metadata(&self, m: &RecordingMetadata) -> Result<(), ApiError> {
        const SAFE: u64 = 9_007_199_254_740_991;
        if m.byte_length == 0
            || m.byte_length > self.max_upload_bytes
            || m.byte_length > SAFE
            || m.created_at_ms > SAFE
            || m.duration_ms > SAFE
            || m.mime_type.len() > 128
            || !m.mime_type.starts_with("audio/")
            || m.mime_type.chars().any(char::is_control)
            || matches!(m.capture_state, CaptureState::Capturing)
        {
            return Err(error(ErrorCode::InvalidRequest, "Upload requires nonempty completed/interrupted audio, a valid audio MIME and safe metadata within the configured byte limit.", false));
        }
        Ok(())
    }
    pub(super) async fn get_inner(&self, id: Uuid) -> Result<Recording, ApiError> {
        let dir = self.dir(id);
        if fs::try_exists(dir.join("recording.json"))
            .await
            .map_err(storage)?
        {
            return read_json(&dir.join("recording.json")).await;
        }
        let partial = self.partial(id);
        if fs::try_exists(partial.join("recording.json"))
            .await
            .map_err(storage)?
        {
            let mut recording: Recording = read_json(&partial.join("recording.json")).await?;
            let received_bytes = fs::metadata(partial.join("audio"))
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            let received_bytes =
                match read_json::<serde_json::Value>(&partial.join("upload.json")).await {
                    Ok(journal) => journal["offset"].as_u64().unwrap_or(0).min(received_bytes),
                    Err(_) => received_bytes,
                };
            recording.audio = AudioState::Uploading { received_bytes };
            return Ok(recording);
        }
        Err(error(
            ErrorCode::RecordingNotFound,
            "Recording was not found.",
            false,
        ))
    }
    /// Enumerate retained audio for the native desktop history. No HTTP listing
    /// is added to the private self-hosted prototype.
    pub async fn list(&self) -> Result<Vec<Recording>, ApiError> {
        let mut entries = fs::read_dir(&self.root).await.map_err(storage)?;
        let mut result = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(storage)? {
            if let Some(id) = entry.file_name().to_str().and_then(|s| s.parse().ok()) {
                let recording = self.get_inner(id).await?;
                if matches!(recording.audio, AudioState::Saved { .. }) {
                    result.push(recording);
                }
            }
        }
        result.sort_by_key(|r| std::cmp::Reverse(r.metadata.created_at_ms));
        Ok(result)
    }
    /// Remove only explicitly selected history, including audio tombstones.
    pub(crate) async fn purge_selected(&self, ids: &[Uuid]) -> Result<(), ApiError> {
        let _activity = self.activity.try_write().map_err(|_| {
            crate::limits::busy("An upload is active; retry history deletion later.")
        })?;
        let _guard = self.lock.lock().await;
        for id in ids {
            for path in [self.dir(*id), self.partial(*id)] {
                match fs::symlink_metadata(&path).await {
                    Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {
                        fs::remove_dir_all(path).await.map_err(storage)?
                    }
                    Ok(_) => return Err(error(
                        ErrorCode::StorageFailed,
                        "Unexpected recording storage entry; retained history was not followed.",
                        false,
                    )),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(storage(e)),
                }
            }
        }
        sync_dir(&self.root).await
    }
    /// Bulk deletion shares the ingestion lock, including incomplete uploads.
    /// Fail promptly if an upload is active; never erase its source mid-write.
    pub async fn delete_all(&self) -> Result<usize, ApiError> {
        let _activity = self.activity.try_write().map_err(|_| {
            crate::limits::busy("An upload is active; retry history deletion later.")
        })?;
        let _guard = self.lock.try_lock().map_err(|_| {
            error(
                ErrorCode::IdempotencyConflict,
                "An audio upload is active; wait before deleting history.",
                true,
            )
        })?;
        let mut entries = fs::read_dir(&self.root).await.map_err(storage)?;
        let mut paths = Vec::new();
        let mut ids = std::collections::HashSet::new();
        while let Some(entry) = entries.next_entry().await.map_err(storage)? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Ok(id) = name
                .strip_suffix(".uploading")
                .unwrap_or(&name)
                .parse::<Uuid>()
            {
                if entry.file_type().await.map_err(storage)?.is_dir() {
                    ids.insert(id);
                    paths.push(entry.path());
                }
            }
        }
        for path in paths {
            fs::remove_dir_all(path).await.map_err(storage)?;
        }
        Ok(ids.len())
    }
    /// Called only by the native host after its UUID-owned capture is finalized.
    /// HTTP uploads can never supply a source or prepared filesystem path.
    pub async fn import_native(&self, source: SavedAudio) -> Result<Recording, ApiError> {
        self.validate_metadata(&source.metadata)?;
        let _activity = self.activity.try_write().map_err(|_| {
            crate::limits::busy("An upload is active; retry importing retained native audio.")
        })?;
        let _guard = self.lock.lock().await;
        let id = source.metadata.id;
        if fs::try_exists(self.dir(id)).await.map_err(storage)? {
            let previous = self.get_inner(id).await?;
            if matches!(previous.audio, AudioState::Saved {})
                && serde_json::to_value(&previous.metadata).map_err(storage)?
                    == serde_json::to_value(&source.metadata).map_err(storage)?
            {
                return Ok(previous);
            }
            return Err(error(
                ErrorCode::IdempotencyConflict,
                "Saved recording ID already exists with different metadata.",
                false,
            ));
        }
        let extra = source.prepared.as_ref().map_or(0, |p| p.byte_length);
        crate::latency::mark(id, "import_budget_requested");
        let _reservation = self
            .budget
            .reserve(
                &self.quota_root,
                self.partial(id),
                source
                    .metadata
                    .byte_length
                    .saturating_add(extra)
                    .saturating_add(source.metadata.byte_length / 128)
                    .saturating_add(16384),
            )
            .await?;
        crate::latency::mark(id, "import_budget_acquired");
        let _partial_cleanup = PartialCleanup(self.partial(id));
        let file = fs::metadata(&source.path).await.map_err(storage)?;
        if !file.is_file() || file.len() != source.metadata.byte_length {
            return Err(error(
                ErrorCode::AudioNotSaved,
                "Native audio is incomplete; the source is retained.",
                true,
            ));
        }
        let partial = self.partial(id);
        if fs::try_exists(&partial).await.map_err(storage)? {
            fs::remove_dir_all(&partial).await.map_err(storage)?;
        }
        private_dir(&partial).await?;
        link_or_copy(&source.path, &partial.join("audio")).await?;
        if let Some(prepared) = source.prepared {
            // Derived audio is optional. A failed copy does not invalidate capture.
            if prepared.byte_length > 0
                && prepared.byte_length.is_multiple_of(2)
                && fs::metadata(&prepared.path)
                    .await
                    .is_ok_and(|m| m.len() == prepared.byte_length)
                && link_or_copy(&prepared.path, &partial.join("prepared.pcm"))
                    .await
                    .is_ok()
            {
                write_json(&partial.join("prepared.json"), &prepared.byte_length).await?;
            }
        }
        let recording = Recording {
            metadata: source.metadata,
            storage: StorageLocation::SelfHostedServer,
            audio: AudioState::Saved {},
        };
        let extents: Vec<_> = (0..recording.metadata.byte_length)
            .step_by(64 * 1024)
            .enumerate()
            .map(|(i, offset)| AudioExtent {
                sequence: i as u64,
                byte_offset: offset,
                byte_length: (recording.metadata.byte_length - offset).min(64 * 1024),
            })
            .collect();
        write_json(&partial.join("extents.json"), &extents).await?;
        write_json(&partial.join("recording.json"), &recording).await?;
        fs::rename(&partial, self.dir(id)).await.map_err(storage)?;
        sync_dir(&self.root).await?;
        Ok(recording)
    }
    pub async fn extents(&self, id: Uuid) -> Result<Vec<AudioExtent>, ApiError> {
        self.audio(id).await?;
        read_json(&self.dir(id).join("extents.json")).await
    }
}
#[async_trait]
impl RecordingStore for FileRecordingStore {
    async fn save(
        &self,
        metadata: RecordingMetadata,
        reader: AudioReader,
    ) -> Result<Recording, ApiError> {
        self.validate_metadata(&metadata)?;
        let _activity = self
            .activity
            .try_read()
            .map_err(|_| crate::limits::busy("History maintenance is active; retry upload."))?;
        let _global = self
            .budget
            .uploads
            .clone()
            .try_acquire_owned()
            .map_err(|_| crate::limits::busy("Server upload slots are full; retry upload."))?;
        let _device = self
            .uploads
            .clone()
            .try_acquire_owned()
            .map_err(|_| crate::limits::busy("Device upload slots are full; retry upload."))?;
        let _guard = self.lock.lock().await;
        let id = metadata.id;
        if fs::try_exists(self.dir(id)).await.map_err(storage)? {
            return Err(error(
                ErrorCode::IdempotencyConflict,
                "This recording ID already exists; retained audio was not overwritten.",
                false,
            ));
        }
        let _reservation = self
            .budget
            .reserve(
                &self.quota_root,
                self.partial(id),
                metadata
                    .byte_length
                    .saturating_add(metadata.byte_length / 128)
                    .saturating_add(16384),
            )
            .await?;
        let _partial_cleanup = PartialCleanup(self.partial(id));
        // A repeated incomplete upload restarts at byte zero. Saved audio is never touched.
        let partial = self.partial(id);
        if fs::try_exists(&partial).await.map_err(storage)? {
            fs::remove_dir_all(&partial).await.map_err(storage)?;
        }
        private_dir(&partial).await?;
        let mut recording = Recording {
            metadata,
            storage: StorageLocation::SelfHostedServer,
            audio: AudioState::Uploading { received_bytes: 0 },
        };
        write_json(&partial.join("recording.json"), &recording).await?;
        let file = private_file(&partial.join("audio")).await?;
        drop(_guard); // No store-wide lock is held while reading the client.
        let total = super::ingest::receive(
            reader,
            file,
            recording.metadata.byte_length,
            self.budget.limits.idle,
        )
        .await?;
        let mut extents = Vec::new();
        for offset in (0..total).step_by(64 * 1024) {
            extents.push(AudioExtent {
                sequence: extents.len() as u64,
                byte_offset: offset,
                byte_length: (total - offset).min(64 * 1024),
            });
        }
        recording.audio = AudioState::Saved {};
        write_json(&partial.join("extents.json"), &extents).await?;
        write_json(&partial.join("recording.json"), &recording).await?;
        fs::rename(&partial, self.dir(id)).await.map_err(storage)?;
        sync_dir(&self.root).await?;
        Ok(recording)
    }
    async fn get(&self, id: Uuid) -> Result<Recording, ApiError> {
        self.get_inner(id).await
    }
    async fn audio(&self, id: Uuid) -> Result<SavedAudio, ApiError> {
        let recording = self.get_inner(id).await?;
        if !matches!(recording.audio, AudioState::Saved {})
            || !fs::try_exists(self.dir(id).join("audio"))
                .await
                .map_err(storage)?
        {
            return Err(error(ErrorCode::AudioNotSaved, "Audio is incomplete or explicitly deleted; upload retained audio before transcription.", false));
        }
        let prepared = match read_json::<u64>(&self.dir(id).join("prepared.json")).await {
            Ok(length) if length > 0 && length.is_multiple_of(2) => {
                let path = self.dir(id).join("prepared.pcm");
                fs::metadata(&path)
                    .await
                    .ok()
                    .filter(|m| m.len() == length)
                    .map(|_| crate::providers::PreparedAudio {
                        path,
                        byte_length: length,
                    })
            }
            _ => None,
        };
        Ok(SavedAudio {
            prepared,
            metadata: recording.metadata,
            path: self.dir(id).join("audio"),
        })
    }
    async fn delete(&self, id: Uuid) -> Result<(), ApiError> {
        let _activity = self
            .activity
            .try_write()
            .map_err(|_| crate::limits::busy("An upload is active; retry deletion later."))?;
        let _guard = self.lock.lock().await;
        let mut recording = self.get_inner(id).await?;
        let dir = self.dir(id);
        private_dir(&dir).await?;
        recording.audio = AudioState::Deleted {};
        // Tombstone first: a crash may retain inaccessible bytes, never expose deleted audio.
        write_json(&dir.join("recording.json"), &recording).await?;
        if fs::try_exists(dir.join("audio")).await.map_err(storage)? {
            fs::remove_file(dir.join("audio")).await.map_err(storage)?;
        }
        for name in ["prepared.pcm", "prepared.json", "audio.gemini-upload.json"] {
            let path = dir.join(name);
            if fs::try_exists(&path).await.map_err(storage)? {
                fs::remove_file(path).await.map_err(storage)?;
            }
        }
        if fs::try_exists(self.partial(id)).await.map_err(storage)? {
            fs::remove_dir_all(self.partial(id))
                .await
                .map_err(storage)?;
        }
        sync_dir(&dir).await
    }
}

async fn link_or_copy(source: &Path, target: &Path) -> Result<(), ApiError> {
    if fs::hard_link(source, target).await.is_err() {
        fs::copy(source, target).await.map_err(storage)?;
        fs::OpenOptions::new()
            .write(true)
            .open(target)
            .await
            .map_err(storage)?
            .sync_all()
            .await
            .map_err(storage)?;
    }
    Ok(())
}

// Dropping a cancelled request also releases partial disk space. This directory
// has at most audio and three small sidecars, never committed audio.
struct PartialCleanup(PathBuf);
impl Drop for PartialCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    #[tokio::test]
    async fn native_registration_is_idempotent_and_deletion_retains_the_capture_source() {
        let root = std::env::temp_dir().join(format!("writer-native-import-{}", Uuid::new_v4()));
        private_dir(&root).await.unwrap();
        let original = root.join("capture.wav");
        fs::write(&original, b"original bytes").await.unwrap();
        let pcm = root.join("prepared.pcm");
        fs::write(&pcm, [0, 0, 1, 0]).await.unwrap();
        let catalog: ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        let mut metadata = catalog.recording.data.metadata;
        metadata.id = Uuid::new_v4();
        metadata.byte_length = 14;
        metadata.capture_state = CaptureState::Complete;
        let source = SavedAudio {
            metadata: metadata.clone(),
            path: original.clone(),
            prepared: Some(crate::providers::PreparedAudio {
                path: pcm.clone(),
                byte_length: 4,
            }),
        };
        let store = FileRecordingStore::open(root.join("store"), 1024)
            .await
            .unwrap();
        store.import_native(source.clone()).await.unwrap();
        store.import_native(source.clone()).await.unwrap();
        let saved = store.audio(metadata.id).await.unwrap();
        assert_eq!(fs::read(&saved.path).await.unwrap(), b"original bytes");
        assert_eq!(saved.prepared.unwrap().byte_length, 4);
        let mut changed = source;
        changed.metadata.byte_length = 13;
        assert!(store.import_native(changed).await.is_err());
        store.delete(metadata.id).await.unwrap();
        assert!(store.audio(metadata.id).await.is_err());
        assert!(!store.dir(metadata.id).join("prepared.pcm").exists());
        assert_eq!(fs::read(original).await.unwrap(), b"original bytes");
        assert_eq!(fs::read(pcm).await.unwrap(), [0, 0, 1, 0]);
        fs::remove_dir_all(root).await.unwrap();
    }
}
