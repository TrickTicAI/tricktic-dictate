//! Native capture journal. PCM is committed before metadata and never deleted by
//! a model operation. A stale journal is repaired from the retained PCM length.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, String>;
fn disk(_: impl std::fmt::Display) -> String {
    "Recording storage failed; saved audio is retained.".into()
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: uuid::Uuid,
    pub created_at_ms: u64,
    pub input_language: String,
    pub duration_ms: u64,
    pub byte_length: u64,
    pub mime_type: String,
    pub capture_state: String,
    pub chunk_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
}
#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).map_err(disk)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(disk)?;
        }
        Ok(Self { root })
    }
    pub fn path(&self, id: uuid::Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }
    pub fn create(
        &self,
        id: uuid::Uuid,
        language: String,
        rate: u32,
        channels: u16,
    ) -> Result<Journal> {
        if !["en", "he", "mixed"].contains(&language.as_str()) || rate == 0 || channels == 0 {
            return Err("Invalid recording configuration.".into());
        }
        let dir = self.path(id);
        fs::create_dir(&dir).map_err(disk)?; // Never overwrite an existing recording.
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(dir.join("audio.wav"))
            .map_err(disk)?;
        let metadata = Recording {
            id,
            created_at_ms: now(),
            input_language: language,
            duration_ms: 0,
            byte_length: 0,
            mime_type: "audio/wav".into(),
            capture_state: "capturing".into(),
            chunk_count: 0,
            sample_rate: Some(rate),
            channels: Some(channels),
        };
        let mut journal = Journal {
            dir: dir.clone(),
            file,
            metadata,
            samples: 0,
            prepared: PreparedJournal::open(&dir, rate, channels).ok(),
        };
        journal.commit()?;
        Ok(journal)
    }
    pub fn get(&self, id: uuid::Uuid, active: bool) -> Result<Option<Recording>> {
        let dir = self.path(id);
        let bytes = match fs::read(dir.join("recording.json")) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(disk(e)),
        };
        let mut m: Recording = serde_json::from_slice(&bytes).map_err(disk)?;
        if !active && m.capture_state == "capturing" {
            // Sync/header can be interrupted. Recover every complete PCM frame.
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(dir.join("audio.wav"))
                .map_err(disk)?;
            let channels = m.channels.ok_or_else(|| disk("channels"))?;
            let rate = m.sample_rate.ok_or_else(|| disk("rate"))?;
            let raw = f.metadata().map_err(disk)?.len().saturating_sub(80);
            let raw = raw - raw % (u64::from(channels) * 2);
            f.set_len(80 + raw).map_err(disk)?;
            write_header(&mut f, raw, rate, channels).map_err(disk)?;
            f.sync_all().map_err(disk)?;
            m.byte_length = if raw > 0 { 80 + raw } else { 0 };
            m.duration_ms = raw / (u64::from(channels) * 2) * 1000 / u64::from(rate);
            m.capture_state = "interrupted".into();
            atomic_json(&dir.join("recording.json"), &m)?;
        }
        Ok(Some(m))
    }
    #[cfg(test)]
    fn list(&self, active: Option<uuid::Uuid>) -> Result<Vec<Recording>> {
        self.list_with_recovery(|id| self.get(id, Some(id) == active))
    }
    /// Enumerate on the caller's history worker. Only unfinished journals need
    /// the recorder to decide whether recovery is safe; completed history never
    /// enters its command queue. The initial read must not repair a live journal.
    pub fn list_with_recovery(
        &self,
        mut recover: impl FnMut(uuid::Uuid) -> Result<Option<Recording>>,
    ) -> Result<Vec<Recording>> {
        let mut result = Vec::new();
        for e in fs::read_dir(&self.root).map_err(disk)? {
            let e = e.map_err(disk)?;
            if let Ok(id) = e.file_name().to_string_lossy().parse() {
                // One unreadable or unrepairable recording must not hide the
                // others or stop startup. Its files stay for the next attempt.
                let metadata = self.get(id, true).and_then(|metadata| match metadata {
                    Some(m) if m.capture_state == "capturing" => recover(id),
                    other => Ok(other),
                });
                if let Ok(Some(m)) = metadata {
                    result.push(m);
                }
            }
        }
        result.sort_by_key(|m| std::cmp::Reverse(m.created_at_ms));
        Ok(result)
    }
    /// Every recording folder, readable or not, so retention keeps its context.
    pub fn ids(&self) -> Result<Vec<uuid::Uuid>> {
        let mut ids = Vec::new();
        for e in fs::read_dir(&self.root).map_err(disk)? {
            if let Ok(id) = e.map_err(disk)?.file_name().to_string_lossy().parse() {
                ids.push(id);
            }
        }
        Ok(ids)
    }
    pub fn audio(&self, id: uuid::Uuid) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        File::open(self.path(id).join("audio.wav"))
            .map_err(disk)?
            .read_to_end(&mut data)
            .map_err(disk)?;
        Ok(data)
    }
    /// Read only a committed prefix and construct a private header. Capture can
    /// append or rewrite its own header concurrently without corrupting this copy.
    pub fn snapshot_audio(&self, id: uuid::Uuid) -> Result<Vec<u8>> {
        let metadata = self.get(id, true)?.ok_or("Recording not found.")?;
        let length = metadata.byte_length;
        if !(81..=64 * 1024 * 1024).contains(&length) {
            return Err("No committed preview audio, or preview exceeds 64 MiB.".into());
        }
        let rate = metadata.sample_rate.ok_or("Missing sample rate.")?;
        let channels = metadata
            .channels
            .filter(|c| *c > 0)
            .ok_or("Missing channels.")?;
        let raw = length - 80;
        if rate == 0 || raw % (u64::from(channels) * 2) != 0 {
            return Err("Incomplete PCM frame.".into());
        }
        let mut source = File::open(self.path(id).join("audio.wav")).map_err(disk)?;
        source.seek(SeekFrom::Start(80)).map_err(disk)?;
        let mut bytes = vec![0; length as usize];
        source.read_exact(&mut bytes[80..]).map_err(disk)?;
        // write_header takes a generic seekable writer, never the live file here.
        write_header(&mut std::io::Cursor::new(&mut bytes), raw, rate, channels).map_err(disk)?;
        Ok(bytes)
    }
    pub fn delete(&self, id: uuid::Uuid) -> Result<()> {
        fs::remove_dir_all(self.path(id)).map_err(disk)
    }
}
pub struct Journal {
    dir: PathBuf,
    file: File,
    pub metadata: Recording,
    samples: u64,
    prepared: Option<PreparedJournal>,
}
impl Journal {
    /// Immutable extent of already appended PCM, independent of disk checkpoints.
    pub fn preview(&self) -> Option<(Recording, PathBuf, u64)> {
        let prepared = self.prepared.as_ref()?;
        if prepared.bytes == 0 {
            return None;
        }
        let mut metadata = self.metadata.clone();
        metadata.byte_length = self.samples * 2 + 80;
        metadata.duration_ms =
            self.samples / u64::from(metadata.channels?) * 1000 / u64::from(metadata.sample_rate?);
        Some((metadata, self.dir.join("prepared.pcm"), prepared.bytes))
    }
    pub fn append(&mut self, samples: &[i16]) -> Result<()> {
        self.file.seek(SeekFrom::End(0)).map_err(disk)?;
        let bytes: Vec<_> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.file.write_all(&bytes).map_err(disk)?;
        self.samples += samples.len() as u64;
        if self
            .prepared
            .as_mut()
            .is_some_and(|p| p.append(samples).is_err())
        {
            self.prepared = None;
            let _ = fs::remove_file(self.dir.join("prepared.json"));
        }
        Ok(())
    }
    pub fn commit(&mut self) -> Result<()> {
        let raw = self.samples * 2;
        let rate = self.metadata.sample_rate.unwrap();
        let channels = self.metadata.channels.unwrap();
        self.file.set_len(80 + raw).map_err(disk)?;
        write_header(&mut self.file, raw, rate, channels).map_err(disk)?;
        self.file.sync_all().map_err(disk)?;
        self.metadata.byte_length = if raw > 0 { 80 + raw } else { 0 };
        self.metadata.duration_ms = self.samples / u64::from(channels) * 1000 / u64::from(rate);
        self.metadata.chunk_count += 1;
        atomic_json(&self.dir.join("recording.json"), &self.metadata)?;
        // A derivative failure never invalidates successfully saved source PCM.
        if self.prepared.as_mut().is_some_and(|p| {
            p.commit(
                &self.dir,
                raw + 80,
                self.metadata.capture_state != "capturing",
            )
            .is_err()
        }) {
            self.prepared = None;
            let _ = fs::remove_file(self.dir.join("prepared.json"));
        }
        Ok(())
    }
    pub fn finish(&mut self, interrupted: bool) -> Result<()> {
        if self.prepared.as_mut().is_some_and(|p| p.finish().is_err()) {
            self.prepared = None;
            let _ = fs::remove_file(self.dir.join("prepared.json"));
        }
        self.metadata.capture_state = if interrupted {
            "interrupted"
        } else {
            "complete"
        }
        .into();
        self.commit()
    }
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let written = write_and_replace(&temporary, path, value);
    if written.is_err() {
        // Never leave a temporary behind; each failed launch used to add one.
        let _ = fs::remove_file(&temporary);
    }
    written
}
fn write_and_replace(temporary: &Path, path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temporary)
        .map_err(disk)?;
    file.write_all(&serde_json::to_vec(value).map_err(disk)?)
        .map_err(disk)?;
    file.sync_all().map_err(disk)?;
    drop(file);
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let (from, to) = (verbatim(temporary)?, verbatim(path)?);
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
            return Err(disk("replace"));
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(temporary, path).map_err(disk)?;
        File::open(path.parent().unwrap())
            .map_err(disk)?
            .sync_all()
            .map_err(disk)?;
    }
    Ok(())
}
/// `MoveFileExW` stops at MAX_PATH unless it is given a `\\?\` path, which
/// a data root of about 150 characters reaches (MW-SEC-21 W-05).
#[cfg(windows)]
fn verbatim(path: &Path) -> Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let absolute: Vec<u16> = std::path::absolute(path)
        .map_err(disk)?
        .as_os_str()
        .encode_wide()
        .collect();
    let prefix = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let mut wide =
        if absolute.starts_with(&prefix(r"\\?\")) || absolute.starts_with(&prefix(r"\\.\")) {
            absolute
        } else if let Some(unc) = absolute.strip_prefix(&prefix(r"\\")[..]) {
            [prefix(r"\\?\UNC\"), unc.to_vec()].concat()
        } else {
            [prefix(r"\\?\"), absolute].concat()
        };
    wide.push(0);
    Ok(wide)
}
fn write_header(
    file: &mut (impl Write + Seek),
    bytes: u64,
    rate: u32,
    channels: u16,
) -> std::io::Result<()> {
    // Reserve ds64 space from the first sample; promote RIFF to RF64 beyond the
    // actual WAV 32-bit size ceiling without imposing a recording time cutoff.
    let large = bytes + 72 > u64::from(u32::MAX);
    let mut h = Vec::with_capacity(80);
    h.extend_from_slice(if large { b"RF64" } else { b"RIFF" });
    h.extend_from_slice(&(if large { u32::MAX } else { (bytes + 72) as u32 }).to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(if large { b"ds64" } else { b"JUNK" });
    h.extend_from_slice(&28u32.to_le_bytes());
    h.extend_from_slice(&(bytes + 72).to_le_bytes());
    h.extend_from_slice(&bytes.to_le_bytes());
    h.extend_from_slice(&(bytes / (u64::from(channels) * 2)).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    h.extend_from_slice(&(channels * 2).to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(if large { u32::MAX } else { bytes as u32 }).to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&h)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_history_only_consults_recorder_for_unfinished_journals() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().join("recordings")).unwrap();
        let active_id = uuid::Uuid::new_v4();
        let mut active = store.create(active_id, "en".into(), 16000, 1).unwrap();
        active.append(&[321; 1600]).unwrap();
        active.commit().unwrap();
        // Include an uncommitted tail: a history read must not repair/truncate it.
        active.append(&[123; 401]).unwrap();
        let before = store.audio(active_id).unwrap();
        let stale_id = uuid::Uuid::new_v4();
        let mut stale = store.create(stale_id, "he".into(), 16000, 1).unwrap();
        stale.append(&[123; 800]).unwrap();
        stale.commit().unwrap();
        drop(stale);
        let mut completed = active.metadata.clone();
        completed.capture_state = "complete".into();
        for i in 0..1200 {
            completed.id = uuid::Uuid::new_v4();
            completed.created_at_ms = i;
            fs::create_dir(store.path(completed.id)).unwrap();
            fs::write(
                store.path(completed.id).join("recording.json"),
                serde_json::to_vec(&completed).unwrap(),
            )
            .unwrap();
        }
        let mut consulted = Vec::new();
        let history = store
            .list_with_recovery(|id| {
                consulted.push(id);
                // The background scan only reads; the recorder owns the active check.
                assert_eq!(store.audio(active_id).unwrap(), before);
                store.get(id, id == active_id)
            })
            .unwrap();
        assert_eq!(history.len(), 1202);
        consulted.sort();
        let mut expected = vec![active_id, stale_id];
        expected.sort();
        assert_eq!(consulted, expected);
        assert_eq!(
            history
                .iter()
                .find(|m| m.id == active_id)
                .unwrap()
                .capture_state,
            "capturing"
        );
        assert_eq!(
            history
                .iter()
                .find(|m| m.id == stale_id)
                .unwrap()
                .capture_state,
            "interrupted"
        );
        assert_eq!(store.audio(active_id).unwrap(), before);
        assert!(history
            .windows(2)
            .all(|pair| pair[0].created_at_ms >= pair[1].created_at_ms));
        active.finish(false).unwrap();
    }
    fn temporaries(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count()
    }
    #[test]
    fn failed_replace_removes_its_temporary() {
        let root = std::env::temp_dir().join(format!("writer-replace-{}", uuid::Uuid::new_v4()));
        let target = root.join("recording.json");
        // A non-empty directory cannot be replaced by a file on any platform.
        fs::create_dir_all(target.join("occupied")).unwrap();
        assert!(atomic_json(&target, &serde_json::json!({"ok": true})).is_err());
        assert_eq!(temporaries(&root), 0);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn one_unreadable_recording_does_not_hide_the_others() {
        let root = std::env::temp_dir().join(format!("writer-skip-{}", uuid::Uuid::new_v4()));
        let store = Store::open(root.clone()).unwrap();
        let (good, bad) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        store
            .create(good, "en".into(), 16000, 1)
            .unwrap()
            .finish(false)
            .unwrap();
        drop(store.create(bad, "en".into(), 16000, 1).unwrap());
        fs::write(store.path(bad).join("recording.json"), b"{").unwrap();
        let listed = store.list(None).unwrap();
        assert_eq!(listed.iter().map(|m| m.id).collect::<Vec<_>>(), [good]);
        let mut ids = store.ids().unwrap();
        ids.sort();
        let mut expected = vec![good, bad];
        expected.sort();
        assert_eq!(
            ids, expected,
            "retention still sees the unreadable recording"
        );
        assert!(store.path(bad).join("audio.wav").exists());
        fs::remove_dir_all(root).unwrap();
    }
    /// W-05: a data root near MAX_PATH used to stop startup at the metadata
    /// replace of an interrupted recording and leave a `.tmp` on every launch.
    #[cfg(windows)]
    #[test]
    fn interrupted_recording_is_repaired_under_a_long_data_root() {
        let base = std::env::temp_dir().join(format!("writer-long-{}", uuid::Uuid::new_v4()));
        let mut root = base.clone();
        while root.as_os_str().len() < 200 {
            root.push("a-long-data-root-segment");
        }
        let store = Store::open(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4();
        let mut journal = store.create(id, "en".into(), 16000, 1).unwrap();
        journal.append(&[321; 1600]).unwrap();
        journal.commit().unwrap();
        drop(journal);
        let temporary_path = store
            .path(id)
            .join(format!("recording.{}.tmp", uuid::Uuid::nil()));
        assert!(
            temporary_path.as_os_str().len() > 260,
            "{}",
            temporary_path.display()
        );
        let recovered = store.get(id, false).unwrap().unwrap();
        assert_eq!(recovered.capture_state, "interrupted");
        assert_eq!(store.list(None).unwrap().len(), 1);
        assert_eq!(temporaries(&store.path(id)), 0);
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn live_preview_advances_without_committing_and_recovery_keeps_source() {
        let root = std::env::temp_dir().join(format!("writer-live-{}", uuid::Uuid::new_v4()));
        let store = Store::open(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4();
        let mut journal = store.create(id, "he".into(), 16000, 1).unwrap();
        let checkpoint = fs::read(store.path(id).join("recording.json")).unwrap();
        journal.append(&vec![1200; 1600]).unwrap();
        let (metadata, path, bytes) = journal.preview().unwrap();
        assert_eq!(metadata.duration_ms, 100);
        assert_eq!(bytes, 3200);
        assert_eq!(
            fs::read(store.path(id).join("recording.json")).unwrap(),
            checkpoint
        );
        let prefix = fs::read(&path).unwrap();
        journal.append(&vec![2200; 1600]).unwrap();
        assert_eq!(&fs::read(path).unwrap()[..3200], prefix);
        drop(journal);
        let recovered = store.get(id, false).unwrap().unwrap();
        assert_eq!(recovered.capture_state, "interrupted");
        assert_eq!(recovered.byte_length, 6480);
        assert!(store.prepared(id, &recovered).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn preview_reads_only_committed_frames_without_touching_growing_audio() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = Store::open(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4();
        let mut capture = store.create(id, "en".into(), 16000, 1).unwrap();
        capture.append(&[100, 200, 300]).unwrap();
        capture.commit().unwrap();
        capture.append(&[400, 500]).unwrap();
        let before = store.audio(id).unwrap();
        let preview = store.snapshot_audio(id).unwrap();
        assert_eq!(preview.len(), 86);
        assert_eq!(&preview[80..], &[100, 0, 200, 0, 44, 1]);
        assert_eq!(store.audio(id).unwrap(), before);
        capture.finish(false).unwrap();
        assert_eq!(store.snapshot_audio(id).unwrap().len(), 90);
        drop(capture);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn interrupted_tail_is_repaired_and_duplicate_ids_cannot_overwrite_audio() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = Store::open(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4();
        let mut j = store.create(id, "mixed".into(), 16000, 1).unwrap();
        j.append(&[123; 16000]).unwrap();
        j.commit().unwrap();
        assert!(store.create(id, "he".into(), 16000, 1).is_err());
        j.append(&[456; 8000]).unwrap();
        drop(j);
        let recovered = store.get(id, false).unwrap().unwrap();
        assert_eq!(recovered.capture_state, "interrupted");
        assert_eq!(recovered.duration_ms, 1500);
        let bytes = store.audio(id).unwrap();
        assert_eq!(bytes.len(), 48080);
        assert_eq!(u32::from_le_bytes(bytes[76..80].try_into().unwrap()), 48000);
        assert_eq!(&bytes[80..82], &123i16.to_le_bytes());
        assert_eq!(store.list(None).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn normalization_preserves_original_pcm_and_stale_derivatives_are_ignored() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = Store::open(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4();
        let mut journal = store.create(id, "he".into(), 48000, 2).unwrap();
        let samples: Vec<i16> = (0..9600).map(|i| ((i % 100) * 100) as i16).collect();
        for block in samples.chunks(74) {
            journal.append(block).unwrap();
        }
        journal.commit().unwrap();
        let live = store.get(id, true).unwrap().unwrap();
        let (_, committed) = store.prepared(id, &live).unwrap();
        assert!(committed < 3200); // filter lookahead has not yet been flushed
        journal.finish(false).unwrap();
        drop(journal);
        let final_metadata = store.get(id, false).unwrap().unwrap();
        let (prepared, length) = store.prepared(id, &final_metadata).unwrap();
        assert_eq!(length, 3200);
        assert_eq!(fs::metadata(prepared).unwrap().len(), length);
        let source = store.audio(id).unwrap();
        assert_eq!(
            &source[80..],
            samples
                .iter()
                .flat_map(|s| s.to_le_bytes())
                .collect::<Vec<_>>()
        );
        fs::write(store.path(id).join("prepared.json"), b"interrupted stamp").unwrap();
        assert!(store.prepared(id, &final_metadata).is_none());
        assert_eq!(store.audio(id).unwrap(), source);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn riff_promotes_to_rf64_without_a_time_limit() {
        let path = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let mut file = File::create(&path).unwrap();
        write_header(&mut file, u64::from(u32::MAX) + 1, 48000, 2).unwrap();
        drop(file);
        let b = fs::read(&path).unwrap();
        assert_eq!(&b[..4], b"RF64");
        assert_eq!(&b[12..16], b"ds64");
        assert_eq!(b.len(), 80);
        fs::remove_file(path).unwrap();
    }
}

/// Disposable normalized PCM. Original WAV is always written first.
struct PreparedJournal {
    resampler: writer_dictation_audio::Resampler,
    file: File,
    bytes: u64,
}
#[derive(Serialize, Deserialize)]
pub struct PreparedStamp {
    pub source_bytes: u64,
    pub pcm_bytes: u64,
    pub complete: bool,
}
impl PreparedJournal {
    fn open(dir: &Path, rate: u32, channels: u16) -> Result<Self> {
        let resampler = writer_dictation_audio::Resampler::new(rate, channels).map_err(disk)?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(dir.join("prepared.pcm"))
            .map_err(disk)?;
        Ok(Self {
            resampler,
            file,
            bytes: 0,
        })
    }
    fn write(&mut self, output: &[i16]) -> Result<()> {
        let bytes: Vec<_> = output.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.file.write_all(&bytes).map_err(disk)?;
        self.bytes += bytes.len() as u64;
        Ok(())
    }
    fn append(&mut self, samples: &[i16]) -> Result<()> {
        let mut output = Vec::new();
        self.resampler.push(samples, &mut output).map_err(disk)?;
        self.write(&output)
    }
    fn finish(&mut self) -> Result<()> {
        let mut output = Vec::new();
        self.resampler.finish(&mut output).map_err(disk)?;
        self.write(&output)
    }
    fn commit(&mut self, dir: &Path, source_bytes: u64, complete: bool) -> Result<()> {
        if complete {
            self.file.sync_all().map_err(disk)?;
        }
        atomic_json(
            &dir.join("prepared.json"),
            &PreparedStamp {
                source_bytes,
                pcm_bytes: self.bytes,
                complete,
            },
        )
    }
}
impl Store {
    pub fn prepared(&self, id: uuid::Uuid, metadata: &Recording) -> Option<(PathBuf, u64)> {
        let stamp: PreparedStamp =
            serde_json::from_slice(&fs::read(self.path(id).join("prepared.json")).ok()?).ok()?;
        if stamp.source_bytes != metadata.byte_length
            || stamp.pcm_bytes == 0
            || !stamp.pcm_bytes.is_multiple_of(2)
            || (metadata.capture_state != "capturing" && !stamp.complete)
        {
            return None;
        }
        let path = self.path(id).join("prepared.pcm");
        if fs::metadata(&path).ok()?.len() < stamp.pcm_bytes {
            return None;
        }
        Some((path, stamp.pcm_bytes))
    }
}
