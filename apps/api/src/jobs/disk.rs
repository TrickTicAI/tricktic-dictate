use super::JobStore;
use crate::{
    recordings::{
        disk::{error, private_dir, read_json, storage, write_json},
        RecordingStore,
    },
    transcription::*,
};
use async_trait::async_trait;
use std::collections::{BTreeSet, HashMap};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs,
    sync::{broadcast, Mutex},
};
use uuid::Uuid;

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Default)]
struct Index {
    keys: HashMap<Uuid, Uuid>,
    recordings: HashMap<Uuid, Uuid>,
    pending: BTreeSet<(u64, Uuid)>,
}
impl Index {
    fn record(&mut self, job: &Job) {
        self.keys.insert(job.request.idempotency_key, job.id);
        self.recordings.insert(job.id, job.request.recording_id);
        let key = (job.created_at_ms, job.id);
        if matches!(
            job.status,
            JobStatus::Queued {} | JobStatus::Transcribing {}
        ) {
            self.pending.insert(key);
        } else {
            self.pending.remove(&key);
        }
    }
}
pub struct FileJobStore {
    root: PathBuf,
    recordings: Arc<dyn RecordingStore>,
    lock: Mutex<Option<Index>>,
    changed: broadcast::Sender<Job>,
    tickets: std::sync::Mutex<HashMap<Uuid, crate::limits::work::QueueTicket>>,
    pub(crate) limits: crate::limits::Limits,
    work: Option<crate::limits::work::WorkAccount>,
    budget: Option<Arc<crate::limits::storage::StorageBudget>>,
    revision: std::sync::atomic::AtomicU64,
    changes: std::sync::Mutex<std::collections::VecDeque<(u64, Job)>>,
    deleted_revision: std::sync::atomic::AtomicU64,
}
impl FileJobStore {
    pub async fn open(
        root: PathBuf,
        recordings: Arc<dyn RecordingStore>,
    ) -> Result<Self, ApiError> {
        Self::open_budgeted(root, recordings, None, None).await
    }
    pub(crate) async fn open_budgeted(
        root: PathBuf,
        recordings: Arc<dyn RecordingStore>,
        work: Option<crate::limits::work::WorkAccount>,
        budget: Option<Arc<crate::limits::storage::StorageBudget>>,
    ) -> Result<Self, ApiError> {
        private_dir(&root).await?;
        Ok(Self {
            root,
            recordings,
            lock: Mutex::new(None),
            changed: broadcast::channel(32).0,
            tickets: std::sync::Mutex::new(HashMap::new()),
            limits: crate::limits::Limits::from_env()?,
            work,
            budget,
            revision: std::sync::atomic::AtomicU64::new(0),
            changes: std::sync::Mutex::new(std::collections::VecDeque::new()),
            deleted_revision: std::sync::atomic::AtomicU64::new(0),
        })
    }
    pub fn latency_path(&self) -> PathBuf {
        self.root.parent().unwrap().join("latency.json")
    }
    pub fn measure(&self, job: &Job) {
        crate::latency::measurements::save_job(job, &self.latency_path());
    }
    pub fn revision(&self) -> u64 {
        self.revision.load(std::sync::atomic::Ordering::Acquire)
    }
    /// Bounded, coalescible source notifications. Falling behind or deleting
    /// recordings requests an authoritative snapshot instead of dropping jobs.
    pub fn changes_since(&self, after: u64) -> Option<(u64, Vec<Job>)> {
        let changes = self.changes.lock().unwrap();
        let revision = self.revision();
        if self
            .deleted_revision
            .load(std::sync::atomic::Ordering::Acquire)
            > after
            || changes.front().is_some_and(|(first, _)| *first > after + 1)
        {
            return None;
        }
        let jobs = changes
            .iter()
            .filter(|(sequence, _)| *sequence > after)
            .map(|(_, job)| job.clone())
            .collect();
        Some((revision, jobs))
    }
    fn record_change(&self, job: &Job) {
        let mut changes = self.changes.lock().unwrap();
        let revision = self
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::Release)
            + 1;
        changes.push_back((revision, job.clone()));
        // This is an event buffer, not a history/data retention limit.
        while changes.len() > 256 {
            changes.pop_front();
        }
    }
    fn deleted(&self) {
        let _changes = self.changes.lock().unwrap();
        let revision = self
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::Release)
            + 1;
        self.deleted_revision
            .store(revision, std::sync::atomic::Ordering::Release);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Job> {
        self.changed.subscribe()
    }
    async fn ensure_index(&self, index: &mut Option<Index>) -> Result<(), ApiError> {
        if index.is_none() {
            let mut rebuilt = Index::default();
            for job in self.all().await? {
                rebuilt.record(&job);
            }
            // Keep reservations across transient index/persistence failures.
            let mut tickets = self.tickets.lock().unwrap();
            tickets.retain(|id, _| rebuilt.pending.iter().any(|(_, pending)| pending == id));
            if let Some(work) = &self.work {
                for (_, id) in &rebuilt.pending {
                    if !tickets.contains_key(id) {
                        tickets.insert(*id, work.queue(true)?);
                    }
                }
            }
            *index = Some(rebuilt);
        }
        Ok(())
    }
    fn path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }
    pub async fn all(&self) -> Result<Vec<Job>, ApiError> {
        let mut entries = fs::read_dir(&self.root).await.map_err(storage)?;
        let mut jobs = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(storage)? {
            let path = entry.path();
            if path.extension().is_some_and(|x| x == "json")
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| Uuid::parse_str(s).is_ok())
            {
                jobs.push(read_json(&path).await?);
            }
        }
        jobs.sort_by_key(|job: &Job| (job.created_at_ms, job.id));
        Ok(jobs)
    }
    /// Explicit user deletion serialized with job submissions and transitions.
    /// Only recording/job stores are touched; models, credentials and settings remain.
    pub async fn delete_history(
        &self,
        recordings: &crate::recordings::FileRecordingStore,
    ) -> Result<(usize, usize), ApiError> {
        let mut index = self.lock.lock().await;
        let jobs = self.all().await?;
        if jobs.iter().any(|job| {
            matches!(
                job.status,
                JobStatus::Queued {} | JobStatus::Transcribing {}
            )
        }) {
            return Err(error(
                ErrorCode::IdempotencyConflict,
                "Wait for transcription to finish before deleting history.",
                true,
            ));
        }
        // Invalidate before deletion so partial filesystem failures rebuild safely.
        *index = None;
        crate::latency::measurements::erase(&self.latency_path(), &[]);
        let _invalidation = SourceInvalidation(self);
        let deleted = recordings.delete_all().await?;
        for job in &jobs {
            fs::remove_file(self.path(job.id)).await.map_err(storage)?;
        }
        Ok((deleted, jobs.len()))
    }
    /// Serialize deletion with submission. A failed attempt is retained unless explicitly deleted.
    pub async fn delete_recording(&self, id: Uuid) -> Result<(), ApiError> {
        let _guard = self.lock.lock().await;
        if self.all().await?.iter().any(|job| {
            job.request.recording_id == id
                && matches!(
                    job.status,
                    JobStatus::Queued {} | JobStatus::Transcribing {}
                )
        }) {
            return Err(error(ErrorCode::IdempotencyConflict, "Recording has queued or active transcription; wait for it to finish before deleting.", true));
        }
        let _invalidation = SourceInvalidation(self);
        self.recordings.delete(id).await?;
        Ok(())
    }
    /// A single preflight and submission lock for a confirmed batch. Jobs remain
    /// for audio-only deletion; full history removes only the selected families.
    pub(crate) async fn delete_selected(
        &self,
        recordings: &crate::recordings::FileRecordingStore,
        ids: &[Uuid],
        history: bool,
    ) -> Result<Vec<Uuid>, ApiError> {
        let mut index = self.lock.lock().await;
        let jobs = self.all().await?;
        if jobs.iter().any(|job| {
            matches!(
                job.status,
                JobStatus::Queued {} | JobStatus::Transcribing {}
            )
        }) {
            return Err(error(
                ErrorCode::IdempotencyConflict,
                "Wait for transcription to finish before clearing storage.",
                true,
            ));
        }
        let selected: Vec<_> = jobs
            .iter()
            .filter(|j| ids.contains(&j.request.recording_id))
            .map(|j| j.id)
            .collect();
        let _invalidation = SourceInvalidation(self);
        if history {
            *index = None;
            crate::latency::measurements::erase(&self.latency_path(), ids);
            recordings.purge_selected(ids).await?;
            for id in &selected {
                fs::remove_file(self.path(*id)).await.map_err(storage)?;
            }
        } else {
            for id in ids {
                match recordings.get(*id).await {
                    Ok(_) => recordings.delete(*id).await?,
                    Err(e) if matches!(e.code, ErrorCode::RecordingNotFound) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(selected)
    }
    pub(crate) async fn set_execution(
        &self,
        id: Uuid,
        status: JobStatus,
        recovery: RecognitionRecovery,
    ) -> Result<Job, ApiError> {
        self.change_status(id, status, Some(recovery), false).await
    }
    pub async fn cancel_retries(&self, id: Uuid) -> Result<Job, ApiError> {
        // Read/modify/write under the transition lock, so a result cannot revive retries.
        self.change_status(id, JobStatus::Queued {}, None, true)
            .await
    }
    async fn change_status(
        &self,
        id: Uuid,
        mut status: JobStatus,
        recovery: Option<RecognitionRecovery>,
        cancel: bool,
    ) -> Result<Job, ApiError> {
        let mut index = self.lock.lock().await;
        self.ensure_index(&mut index).await?;
        let mut job = self.get(id).await?;
        if cancel
            && !matches!(
                job.status,
                JobStatus::Queued {} | JobStatus::Transcribing {}
            )
        {
            return Ok(job);
        }
        if cancel && matches!(job.status, JobStatus::Transcribing {}) {
            status = JobStatus::Transcribing {};
        }
        let mut next = recovery
            .or_else(|| job.recovery.clone())
            .unwrap_or_default();
        next.canceled |= cancel || job.recovery.as_ref().is_some_and(|r| r.canceled);
        if next.canceled && matches!(status, JobStatus::Queued {}) {
            status = JobStatus::Failed {
                error: canceled_error(),
            };
        }
        // Cancellation may win between admission and the durable attempt start.
        if next.canceled
            && matches!(job.status, JobStatus::Failed { .. })
            && matches!(status, JobStatus::Transcribing {})
        {
            return Ok(job);
        }
        if next.canceled {
            next.next_retry_at_ms = None;
            next.waiting_for_capacity = false;
        }
        job.recovery = Some(next);
        let same_status = serde_json::to_value(&job.status).map_err(storage)?
            == serde_json::to_value(&status).map_err(storage)?;
        let allowed = same_status
            || match (&job.status, &status) {
                (JobStatus::Queued {}, JobStatus::Transcribing {} | JobStatus::Failed { .. }) => {
                    true
                }
                (
                    JobStatus::Transcribing {},
                    JobStatus::Queued {} | JobStatus::Failed { .. } | JobStatus::Succeeded { .. },
                ) => true,
                (JobStatus::Succeeded { result: old }, JobStatus::Succeeded { result: new }) => {
                    serde_json::to_value(&old.transcript).map_err(storage)?
                        == serde_json::to_value(&new.transcript).map_err(storage)?
                }
                _ => false,
            };
        if !allowed {
            return Err(error(
                ErrorCode::IdempotencyConflict,
                "Invalid job transition; original request and raw transcript are immutable.",
                false,
            ));
        }
        if matches!(status, JobStatus::Transcribing {}) {
            crate::local_stt::silence::clear(job.request.recording_id);
        } else if matches!(
            status,
            JobStatus::Succeeded { .. } | JobStatus::Failed { .. }
        ) {
            if let Some(measured) = crate::local_stt::silence::take(job.request.recording_id) {
                job.audio_preparation = Some(measured);
            }
        }
        job.status = status;
        job.updated_at_ms = now().max(job.updated_at_ms.saturating_add(1));
        let size = serde_json::to_vec(&job).map_err(storage)?.len() as u64;
        if size > 4 * 1024 * 1024 {
            return Err(error(
                ErrorCode::StorageFailed,
                "Job result exceeds the 4 MiB storage limit; retained audio is available.",
                true,
            ));
        }
        let _reservation = match &self.budget {
            Some(budget) => Some(
                budget
                    .reserve(self.root.parent().unwrap(), self.path(id), size)
                    .await?,
            ),
            None => None,
        };
        if let Err(error) = write_json(&self.path(id), &job).await {
            *index = None;
            return Err(error);
        }
        index.as_mut().unwrap().record(&job);
        if !matches!(
            job.status,
            JobStatus::Queued {} | JobStatus::Transcribing {}
        ) {
            self.tickets.lock().unwrap().remove(&id);
        }
        crate::latency::mark(
            job.request.recording_id,
            if matches!(job.status, JobStatus::Succeeded { .. }) {
                "raw_result_durable"
            } else {
                "job_status_durable"
            },
        );
        self.record_change(&job);
        let _ = self.changed.send(job.clone());
        crate::account_sync::note_local_change();
        Ok(job)
    }
}
#[async_trait]
impl JobStore for FileJobStore {
    fn latency_path(&self) -> Option<PathBuf> {
        Some(FileJobStore::latency_path(self))
    }
    async fn submit(&self, request: TranscriptionRequest) -> Result<Job, ApiError> {
        let mut index = self.lock.lock().await;
        self.ensure_index(&mut index).await?;
        if let Some(id) = index.as_ref().unwrap().keys.get(&request.idempotency_key) {
            let existing = self.get(*id).await?;
            if serde_json::to_value(&existing.request).map_err(storage)?
                != serde_json::to_value(&request).map_err(storage)?
            {
                return Err(error(ErrorCode::IdempotencyConflict, "Idempotency key belongs to different options; use a new key for an intentional retry.", false));
            }
            return Ok(existing.clone());
        }
        if let Some(previous) = request.retry_of_job_id {
            if index.as_ref().unwrap().recordings.get(&previous) != Some(&request.recording_id) {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "Retry must reference an existing attempt for the same recording.",
                    false,
                ));
            }
            let existing = self.get(previous).await?;
            if existing.request.recording_id == request.recording_id
                && matches!(
                    existing.status,
                    JobStatus::Queued {} | JobStatus::Transcribing {}
                )
            {
                return Err(error(ErrorCode::IdempotencyConflict, "Transcription is already queued or retrying automatically; wait for this saved attempt to finish.", false));
            }
        }
        let current = index.as_ref().unwrap();
        if current.pending.len() >= self.limits.device_pending
            || current.keys.len() >= self.limits.jobs
        {
            return Err(crate::limits::busy("Job budget reached; wait for pending work or export/delete history before retrying. Saved audio is retained."));
        }
        if serde_json::to_vec(&request).map_err(storage)?.len() > 128 * 1024 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Transcription options exceed 128 KiB.",
                false,
            ));
        }
        self.recordings.audio(request.recording_id).await?;
        if request.options.selection.model.trim().is_empty()
            || request.options.selection.model.len() > 256
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Choose a nonempty provider model name (at most 256 bytes).",
                false,
            ));
        }
        if let Some(previous) = request.retry_of_job_id {
            if index.as_ref().unwrap().recordings.get(&previous) != Some(&request.recording_id) {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "Retry must reference an existing attempt for the same recording.",
                    false,
                ));
            }
        }
        let reservation = match &self.budget {
            Some(budget) => Some(
                budget
                    .reserve(
                        self.root.parent().unwrap(),
                        self.path(request.idempotency_key),
                        4 * 1024 * 1024,
                    )
                    .await?,
            ),
            None => None,
        };
        let ticket = self.work.as_ref().map(|w| w.queue(false)).transpose()?;
        let timestamp = now();
        let job = Job {
            audio_preparation: None,
            id: Uuid::new_v4(),
            request,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
            status: JobStatus::Queued {},
            recovery: Some(RecognitionRecovery::default()),
        };
        if let Err(error) = write_json(&self.path(job.id), &job).await {
            *index = None;
            return Err(error);
        }
        index.as_mut().unwrap().record(&job);
        drop(reservation);
        if let Some(ticket) = ticket {
            self.tickets.lock().unwrap().insert(job.id, ticket);
        }
        crate::latency::measurements::register(&job, self.latency_path());
        crate::latency::mark(job.request.recording_id, "job_durable");
        self.record_change(&job);
        let _ = self.changed.send(job.clone());
        crate::account_sync::note_local_change();
        Ok(job)
    }
    async fn get(&self, id: Uuid) -> Result<Job, ApiError> {
        if !fs::try_exists(self.path(id)).await.map_err(storage)? {
            return Err(error(
                ErrorCode::RecordingNotFound,
                "Transcription job was not found.",
                false,
            ));
        }
        read_json(&self.path(id)).await
    }
    async fn set_status(&self, id: Uuid, status: JobStatus) -> Result<Job, ApiError> {
        self.change_status(id, status, None, false).await
    }
    async fn recoverable(&self) -> Result<Vec<Job>, ApiError> {
        let mut index = self.lock.lock().await;
        self.ensure_index(&mut index).await?;
        let mut jobs = Vec::new();
        for (_, id) in &index.as_ref().unwrap().pending {
            jobs.push(self.get(*id).await?);
        }
        Ok(jobs)
    }
}

fn canceled_error() -> ApiError {
    error(
        ErrorCode::Interrupted,
        "Transcription retry canceled; saved audio is retained.",
        false,
    )
}

struct SourceInvalidation<'a>(&'a FileJobStore);
impl Drop for SourceInvalidation<'_> {
    fn drop(&mut self) {
        self.0.deleted();
    }
}
#[cfg(test)]
mod projection_tests {
    use super::*;
    #[tokio::test]
    async fn event_buffer_is_bounded_and_lag_or_deletion_requests_full_recovery() {
        let root = std::env::temp_dir().join(format!("writer-job-projection-{}", Uuid::new_v4()));
        let recordings = Arc::new(
            crate::recordings::FileRecordingStore::open(root.join("recordings"), 1_000_000)
                .await
                .unwrap(),
        );
        let store = FileJobStore::open(root.join("jobs"), recordings)
            .await
            .unwrap();
        let fixture: ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        for _ in 0..300 {
            store.record_change(&fixture.job.data);
        }
        assert_eq!(store.changes.lock().unwrap().len(), 256);
        assert!(store.changes_since(0).is_none());
        let (revision, changes) = store.changes_since(299).unwrap();
        assert_eq!(revision, 300);
        assert_eq!(changes.len(), 1);
        {
            let _guard = SourceInvalidation(&store);
        }
        assert!(store.changes_since(revision).is_none());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
