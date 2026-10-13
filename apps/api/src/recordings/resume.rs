//! Writer-owned resumable ingestion. Acknowledged offsets follow file fsync + atomic journal save.
use super::{disk::*, FileRecordingStore};
use crate::transcription::*;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use uuid::Uuid;
const CHUNK: usize = 1024 * 1024;
const LIFETIME: u64 = 7 * 24 * 3600 * 1000;
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn conflict() -> ApiError {
    error(
        ErrorCode::IdempotencyConflict,
        "Upload checkpoint changed or expired; reopen the saved-audio upload from byte zero.",
        true,
    )
}
#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub metadata: RecordingMetadata,
    pub sha256: String,
}
#[derive(Deserialize, Serialize)]
struct Journal {
    identity: Identity,
    session_id: Uuid,
    offset: u64,
    expires_at_ms: u64,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Checkpoint {
    pub session_id: Uuid,
    pub offset: u64,
    pub expires_at_ms: u64,
    pub saved: bool,
    pub sha256: String,
}
impl Journal {
    fn checkpoint(&self, saved: bool) -> Checkpoint {
        Checkpoint {
            session_id: self.session_id,
            offset: self.offset,
            expires_at_ms: self.expires_at_ms,
            saved,
            sha256: self.identity.sha256.clone(),
        }
    }
}
fn same(a: &Identity, b: &Identity) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}
impl FileRecordingStore {
    pub(super) async fn begin_upload(&self, identity: Identity) -> Result<Checkpoint, ApiError> {
        self.validate_metadata(&identity.metadata)?;
        if identity.sha256.len() != 64
            || !identity
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Upload requires a lowercase SHA-256 source digest.",
                false,
            ));
        }
        let _activity = self
            .activity
            .try_write()
            .map_err(|_| crate::limits::busy("An upload is active; retry shortly."))?;
        let _guard = self.lock.lock().await;
        let id = identity.metadata.id;
        let saved = self.dir(id);
        if fs::try_exists(&saved).await.map_err(storage)? {
            let recording = self.get_inner(id).await?;
            if !matches!(recording.audio, AudioState::Saved {}) {
                return Err(conflict());
            }
            let journal: Journal = match read_json(&saved.join("upload.json")).await {
                Ok(journal) => journal,
                Err(_) => {
                    // Legacy multipart/native audio has no receipt yet. Verify bytes before adopting it.
                    if serde_json::to_value(&recording.metadata).map_err(storage)?
                        != serde_json::to_value(&identity.metadata).map_err(storage)?
                        || digest_file(&saved.join("audio")).await? != identity.sha256
                    {
                        return Err(conflict());
                    }
                    let journal = Journal {
                        identity: identity.clone(),
                        session_id: Uuid::new_v4(),
                        offset: identity.metadata.byte_length,
                        expires_at_ms: now() + LIFETIME,
                    };
                    write_json(&saved.join("upload.json"), &journal).await?;
                    journal
                }
            };
            if !same(&journal.identity, &identity) {
                return Err(conflict());
            }
            return Ok(journal.checkpoint(true));
        }
        let partial = self.partial(id);
        if let Ok(journal) = read_json::<Journal>(&partial.join("upload.json")).await {
            if same(&journal.identity, &identity)
                && journal.expires_at_ms > now()
                && fs::metadata(partial.join("audio"))
                    .await
                    .is_ok_and(|m| m.len() >= journal.offset)
            {
                return Ok(journal.checkpoint(false));
            }
        }
        let _reservation = self
            .budget
            .reserve(
                &self.quota_root,
                partial.clone(),
                identity.metadata.byte_length.saturating_add(32768),
            )
            .await?;
        if fs::try_exists(&partial).await.map_err(storage)? {
            fs::remove_dir_all(&partial).await.map_err(storage)?;
        }
        private_dir(&partial).await?;
        private_file(&partial.join("audio"))
            .await?
            .sync_all()
            .await
            .map_err(storage)?;
        let recording = Recording {
            metadata: identity.metadata.clone(),
            storage: StorageLocation::SelfHostedServer,
            audio: AudioState::Uploading { received_bytes: 0 },
        };
        write_json(&partial.join("recording.json"), &recording).await?;
        let journal = Journal {
            identity,
            session_id: Uuid::new_v4(),
            offset: 0,
            expires_at_ms: now() + LIFETIME,
        };
        write_json(&partial.join("upload.json"), &journal).await?;
        Ok(journal.checkpoint(false))
    }
    async fn upload_journal(
        &self,
        id: Uuid,
        session: Uuid,
    ) -> Result<(std::path::PathBuf, Journal, bool), ApiError> {
        let saved = fs::try_exists(self.dir(id)).await.map_err(storage)?;
        let path = if saved {
            self.dir(id)
        } else {
            self.partial(id)
        };
        let journal: Journal = read_json(&path.join("upload.json"))
            .await
            .map_err(|_| conflict())?;
        if journal.session_id != session || (!saved && journal.expires_at_ms <= now()) {
            return Err(conflict());
        }
        Ok((path, journal, saved))
    }
    pub(super) async fn upload_chunk(
        &self,
        id: Uuid,
        session: Uuid,
        offset: u64,
        bytes: &[u8],
    ) -> Result<Checkpoint, ApiError> {
        if bytes.is_empty() || bytes.len() > CHUNK {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Upload chunks must contain 1 byte to 1 MiB.",
                false,
            ));
        }
        let _activity = self
            .activity
            .try_read()
            .map_err(|_| crate::limits::busy("History maintenance is active."))?;
        let _global = self
            .budget
            .uploads
            .try_acquire()
            .map_err(|_| crate::limits::busy("Upload slots are full."))?;
        let _device = self
            .uploads
            .try_acquire()
            .map_err(|_| crate::limits::busy("Device upload slots are full."))?;
        let _guard = self.lock.lock().await;
        let (path, mut journal, saved) = self.upload_journal(id, session).await?;
        if saved
            || offset != journal.offset
            || offset.saturating_add(bytes.len() as u64) > journal.identity.metadata.byte_length
        {
            return Err(conflict());
        }
        let _reservation = self
            .budget
            .reserve(
                &self.quota_root,
                path.clone(),
                journal
                    .identity
                    .metadata
                    .byte_length
                    .saturating_sub(offset)
                    .saturating_add(32768),
            )
            .await?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(path.join("audio"))
            .await
            .map_err(storage)?;
        // A crash after writing bytes but before journal publication rolls back to the confirmed offset.
        file.set_len(offset).await.map_err(storage)?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(storage)?;
        file.write_all(bytes).await.map_err(storage)?;
        file.sync_all().await.map_err(storage)?;
        journal.offset += bytes.len() as u64;
        write_json(&path.join("upload.json"), &journal).await?;
        Ok(journal.checkpoint(false))
    }
    pub(super) async fn finish_upload(
        &self,
        id: Uuid,
        session: Uuid,
    ) -> Result<Checkpoint, ApiError> {
        let _activity = self
            .activity
            .try_write()
            .map_err(|_| crate::limits::busy("An upload is active."))?;
        let _guard = self.lock.lock().await;
        let (path, journal, saved) = self.upload_journal(id, session).await?;
        if saved {
            if !matches!(self.get_inner(id).await?.audio, AudioState::Saved {}) {
                return Err(conflict());
            }
            return Ok(journal.checkpoint(true));
        }
        if journal.offset != journal.identity.metadata.byte_length {
            return Err(conflict());
        }
        if digest_file(&path.join("audio")).await? != journal.identity.sha256 {
            return Err(error(ErrorCode::InvalidRequest,"Uploaded audio digest differs from saved source; keep the original and restart with its current identity.",false));
        }
        let extents: Vec<_> = (0..journal.offset)
            .step_by(65536)
            .enumerate()
            .map(|(i, offset)| AudioExtent {
                sequence: i as u64,
                byte_offset: offset,
                byte_length: (journal.offset - offset).min(65536),
            })
            .collect();
        write_json(&path.join("extents.json"), &extents).await?;
        write_json(
            &path.join("recording.json"),
            &Recording {
                metadata: journal.identity.metadata.clone(),
                storage: StorageLocation::SelfHostedServer,
                audio: AudioState::Saved {},
            },
        )
        .await?;
        fs::rename(&path, self.dir(id)).await.map_err(storage)?;
        sync_dir(&self.root).await?;
        Ok(journal.checkpoint(true))
    }
}
async fn digest_file(path: &std::path::Path) -> Result<String, ApiError> {
    let mut file = fs::File::open(path).await.map_err(storage)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let size = file.read(&mut buffer).await.map_err(storage)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
type Pipeline = Option<std::sync::Arc<crate::jobs::Pipeline>>;
fn respond(result: Result<Checkpoint, ApiError>) -> Response {
    match result {
        Ok(value) => Json(Envelope::new(value)).into_response(),
        Err(e) => crate::jobs::response_error(e),
    }
}
fn session(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    headers
        .get("upload-session")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(conflict)
}
pub(super) async fn start(
    State(p): State<Pipeline>,
    Path(id): Path<Uuid>,
    Json(body): Json<Envelope<Identity>>,
) -> Response {
    if id != body.data.metadata.id {
        return crate::jobs::response_error(conflict());
    }
    match crate::jobs::runtime::selected(p).await {
        Ok(p) => respond(p.recordings.begin_upload(body.data).await),
        Err(e) => crate::jobs::response_error(e),
    }
}
pub(super) async fn chunk(
    State(p): State<Pipeline>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let result = async {
        let session = session(&headers)?;
        let offset = headers
            .get("upload-offset")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .ok_or_else(conflict)?;
        let bytes = to_bytes(body, CHUNK)
            .await
            .map_err(|_| crate::limits::idle())?;
        crate::jobs::runtime::selected(p)
            .await?
            .recordings
            .upload_chunk(id, session, offset, &bytes)
            .await
    }
    .await;
    respond(result)
}
pub(super) async fn finish(
    State(p): State<Pipeline>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let session = session(&headers)?;
        crate::jobs::runtime::selected(p)
            .await?
            .recordings
            .finish_upload(id, session)
            .await
    }
    .await;
    respond(result)
}
#[cfg(test)]
mod tests;
