//! Context is retry input, not permanent processing history.
use super::*;
use crate::recordings::disk::{read_json, storage, write_json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::PathBuf};

/// When the last complete context-minimizing pass started, in Unix milliseconds.
const SWEPT: &str = "context-swept.json";

pub(super) fn digest(request: &ProcessRequest) -> Result<String, ApiError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(request).map_err(storage)?)
    ))
}

pub(super) fn erase_success_context(attempt: &mut ProcessingAttempt) -> Result<(), ApiError> {
    if matches!(attempt.state, AttemptState::Succeeded { .. }) {
        if attempt.request_digest.is_none() {
            attempt.request_digest = Some(digest(&attempt.request)?);
        }
        attempt.request.context = ContextSnapshot::default();
    }
    Ok(())
}

impl ModeService {
    /// Called under the processing lock, after the result is durable. Keep the
    /// profile binding so a reload cannot silently fall back to Regular mode.
    pub(super) async fn erase_completed_capture(
        &self,
        attempt: &ProcessingAttempt,
    ) -> Result<(), ApiError> {
        if !matches!(attempt.state, AttemptState::Succeeded { .. }) || attempt.id != attempt.job_id
        {
            return Ok(());
        }
        let job = match self.jobs.get(attempt.job_id).await {
            Ok(job) => job,
            // Bulk deletion may have removed jobs before its context commit.
            Err(error)
                if matches!(
                    error.code,
                    crate::transcription::ErrorCode::RecordingNotFound
                ) =>
            {
                return Ok(())
            }
            Err(error) => return Err(error),
        };
        let path = self
            .root
            .join("captures")
            .join(format!("{}.json", job.request.recording_id));
        if tokio::fs::try_exists(&path).await.map_err(storage)? {
            let mut captured: CapturedMode = read_json(&path).await?;
            if captured.context != ContextSnapshot::default() || captured.context_pending {
                captured.context = ContextSnapshot::default();
                captured.context_erased = true;
                captured.context_pending = false;
                write_json(&path, &captured).await?;
            }
        }
        Ok(())
    }

    /// Startup migration also completes erasure interrupted between the result
    /// write and capture write. No providers are invoked during this pass.
    ///
    /// Every attempt write replaces its file inside the job's folder, which
    /// updates the folder's modification time. A folder unchanged since the
    /// last complete pass (recorded in `SWEPT`, less an hour of slack) has
    /// nothing new to erase and is skipped, so startup no longer reads every
    /// saved attempt: that took 43 s at 3,800 dictations. Folders are processed
    /// concurrently; each touches only its own files and its own capture.
    pub(super) async fn minimize_completed_context(&self) -> Result<(), ApiError> {
        use futures_util::{StreamExt, TryStreamExt};
        let marker = self.root.join(SWEPT);
        let since = read_json::<u64>(&marker).await.ok().map(|ms| {
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)
                - std::time::Duration::from_secs(3600)
        });
        let started = std::time::SystemTime::now();
        let mut folders = Vec::new();
        let mut dirs = tokio::fs::read_dir(&self.root).await.map_err(storage)?;
        while let Some(dir) = dirs.next_entry().await.map_err(storage)? {
            let name = dir.file_name();
            let Some(name) = name.to_str() else { continue };
            let captures = name == "captures";
            if (!captures && Uuid::parse_str(name).is_err())
                || !dir.file_type().await.map_err(storage)?.is_dir()
            {
                continue;
            }
            if let (false, Some(since)) = (captures, since) {
                let modified = dir.metadata().await.ok().and_then(|m| m.modified().ok());
                if modified.is_some_and(|modified| modified < since) {
                    continue;
                }
            }
            folders.push((captures, dir.path()));
        }
        futures_util::stream::iter(folders)
            .map(|(captures, folder)| self.minimize_folder(captures, folder))
            .buffer_unordered(16)
            .try_collect::<Vec<()>>()
            .await?;
        let started = started
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        write_json(&marker, &started).await
    }

    async fn minimize_folder(&self, captures: bool, folder: PathBuf) -> Result<(), ApiError> {
        let mut files = tokio::fs::read_dir(folder).await.map_err(storage)?;
        while let Some(file) = files.next_entry().await.map_err(storage)? {
            let path = file.path();
            if path.extension().is_some_and(|e| e == "tmp") {
                // Atomic writes abandoned by a crash may contain context.
                tokio::fs::remove_file(path).await.map_err(storage)?;
            } else if !captures && path.extension().is_some_and(|e| e == "json") {
                let mut attempt: ProcessingAttempt = read_json(&path).await?;
                if matches!(attempt.state, AttemptState::Succeeded { .. }) {
                    if attempt.request_digest.is_none()
                        || attempt.request.context != ContextSnapshot::default()
                    {
                        erase_success_context(&mut attempt)?;
                        write_json(&path, &attempt).await?;
                    }
                    self.erase_completed_capture(&attempt).await?;
                }
            }
        }
        Ok(())
    }

    /// Startup only, before admitting capture: preserve recoverable audio/jobs,
    /// remove pre-microphone snapshots that never acquired either parent.
    pub async fn reconcile_captures(&self, retained: &BTreeSet<Uuid>) -> Result<(), ApiError> {
        self.reconcile_captures_before(retained, std::time::SystemTime::now())
            .await
    }
    /// As `reconcile_captures`, but never removes a capture written at or after
    /// `cutoff`. Startup reconciles in the background from a snapshot taken at
    /// launch, so a dictation begun since then keeps its context.
    pub async fn reconcile_captures_before(
        &self,
        retained: &BTreeSet<Uuid>,
        cutoff: std::time::SystemTime,
    ) -> Result<(), ApiError> {
        let _lock = self.lock.lock().await;
        let directory = self.root.join("captures");
        if !tokio::fs::try_exists(&directory).await.map_err(storage)? {
            return Ok(());
        }
        let mut files = tokio::fs::read_dir(directory).await.map_err(storage)?;
        while let Some(file) = files.next_entry().await.map_err(storage)? {
            let path = file.path();
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            if path.extension().is_some_and(|s| s == "json")
                && id.is_some_and(|id| !retained.contains(&id))
                && file
                    .metadata()
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .is_some_and(|modified| modified < cutoff)
            {
                tokio::fs::remove_file(path).await.map_err(storage)?;
            }
        }
        Ok(())
    }

    /// Discard after transcription settles. Refuse while any affected mode
    /// attempt is executing, so a late worker cannot restore erased context.
    pub async fn discard_recording_context(
        &self,
        recording: Uuid,
        jobs: &[Uuid],
    ) -> Result<(), ApiError> {
        let active = self.lock.lock().await;
        let mut attempts = Vec::new();
        let mut temporary = Vec::new();
        for job in jobs {
            let directory = self.root.join(job.to_string());
            if !tokio::fs::try_exists(&directory).await.map_err(storage)? {
                continue;
            }
            let mut files = tokio::fs::read_dir(directory).await.map_err(storage)?;
            while let Some(file) = files.next_entry().await.map_err(storage)? {
                let path = file.path();
                if path.extension().is_some_and(|s| s == "json") {
                    let attempt: ProcessingAttempt = read_json(&path).await?;
                    if active.contains(&attempt.id) {
                        return Err(crate::cleanup::failure(
                            crate::transcription::ErrorCode::ProviderUnavailable,
                            "Wait for text processing to finish before discarding context.",
                            true,
                        ));
                    }
                    attempts.push((path, attempt));
                } else if path.extension().is_some_and(|s| s == "tmp") {
                    temporary.push(path);
                }
            }
        }
        for (path, mut attempt) in attempts {
            if attempt.request_digest.is_none() {
                attempt.request_digest = Some(digest(&attempt.request)?);
            }
            attempt.request.context = ContextSnapshot::default();
            write_json(&path, &attempt).await?;
        }
        for path in temporary {
            tokio::fs::remove_file(path).await.map_err(storage)?;
        }
        self.remove_capture(recording).await
    }

    /// Failed microphone start or a recording with no submitted job.
    pub async fn discard_capture(&self, recording: Uuid) -> Result<(), ApiError> {
        let _lock = self.lock.lock().await;
        self.remove_capture(recording).await
    }
    async fn remove_capture(&self, recording: Uuid) -> Result<(), ApiError> {
        self.drafts.discard(recording).await;
        let path = self.root.join("captures").join(format!("{recording}.json"));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(storage(error)),
        }
        self.finished.notify_waiters();
        let directory = path.parent().unwrap();
        if tokio::fs::try_exists(directory).await.map_err(storage)? {
            let mut files = tokio::fs::read_dir(directory).await.map_err(storage)?;
            while let Some(file) = files.next_entry().await.map_err(storage)? {
                let path = file.path();
                if path.extension().is_some_and(|s| s == "tmp")
                    && file
                        .file_name()
                        .to_str()
                        .is_some_and(|s| s.starts_with(&format!("{recording}.")))
                {
                    tokio::fs::remove_file(path).await.map_err(storage)?;
                }
            }
        }
        Ok(())
    }
}
