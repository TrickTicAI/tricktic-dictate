use super::{disk::FileJobStore, JobStore};
use crate::{
    providers::{self, TranscriptionProvider},
    recordings::{disk::error, FileRecordingStore, RecordingStore},
    transcription::*,
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, Notify, OnceCell};
mod storage_controls;

pub struct Pipeline {
    _directory_lock: std::fs::File,
    pub recordings: Arc<FileRecordingStore>,
    pub preview_root: PathBuf,
    pub jobs: Arc<FileJobStore>,
    pub(crate) work: crate::limits::work::WorkAccount,
    pub providers: Vec<Arc<dyn TranscriptionProvider>>,
    mode_root: PathBuf,
    modes: OnceCell<Arc<crate::cleanup::modes::ModeService>>,
    batches: OnceCell<Arc<super::batches::BatchService>>,
    wake: Notify,
    execution: Mutex<std::collections::BTreeMap<uuid::Uuid, (JobStatus, RecognitionRecovery)>>,
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        // Explicitly release the lease: a concurrently spawned process may
        // briefly inherit a duplicate descriptor before exec closes it.
        let _ = self._directory_lock.unlock();
    }
}
impl Pipeline {
    pub async fn open(
        root: PathBuf,
        max_upload_bytes: u64,
        providers: Vec<Arc<dyn TranscriptionProvider>>,
    ) -> Result<Arc<Self>, ApiError> {
        let mode_root = root.join("state/modes");
        Self::open_with_modes(root, max_upload_bytes, providers, mode_root).await
    }
    async fn open_with_modes(
        root: PathBuf,
        max_upload_bytes: u64,
        providers: Vec<Arc<dyn TranscriptionProvider>>,
        mode_root: PathBuf,
    ) -> Result<Arc<Self>, ApiError> {
        let limits = crate::limits::Limits::from_env()?;
        let storage = crate::limits::storage::StorageBudget::new(root.clone(), limits.clone());
        Self::open_shared(root, max_upload_bytes, providers, mode_root, storage, None).await
    }
    pub(crate) async fn open_shared(
        root: PathBuf,
        max_upload_bytes: u64,
        providers: Vec<Arc<dyn TranscriptionProvider>>,
        mode_root: PathBuf,
        storage_budget: Arc<crate::limits::storage::StorageBudget>,
        work: Option<crate::limits::work::WorkAccount>,
    ) -> Result<Arc<Self>, ApiError> {
        crate::recordings::disk::private_dir(&root).await?;
        let mut lock_options = tokio::fs::OpenOptions::new();
        lock_options.read(true).write(true).create(true);
        #[cfg(unix)]
        lock_options.mode(0o600);
        let directory_lock = lock_options
            .open(root.join("writer.lock"))
            .await
            .map_err(crate::recordings::disk::storage)?
            .into_std()
            .await;
        directory_lock.try_lock().map_err(|_| error(ErrorCode::StorageFailed, "Data directory is already in use by another TrickTic Dictate process; use a separate WRITER_DATA_DIR.", true))?;
        let work = match work {
            Some(work) => work,
            None => crate::limits::work::WorkCapacity::new(storage_budget.limits.clone()).account(),
        };
        let preview_root = root.join("previews");
        // Disposable snapshots from a crash are removed only after acquiring this
        // data directory's exclusive lease; another Writer instance is untouched.
        if tokio::fs::try_exists(&preview_root)
            .await
            .map_err(crate::recordings::disk::storage)?
        {
            tokio::fs::remove_dir_all(&preview_root)
                .await
                .map_err(crate::recordings::disk::storage)?;
        }
        crate::recordings::disk::private_dir(&preview_root).await?;
        storage_budget.include(mode_root.clone());
        let recordings = Arc::new(
            FileRecordingStore::open_scoped(
                root.join("recordings"),
                max_upload_bytes,
                storage_budget,
                root.clone(),
            )
            .await?,
        );
        let jobs = Arc::new(
            FileJobStore::open_budgeted(
                root.join("jobs"),
                recordings.clone(),
                Some(work.clone()),
                Some(recordings.budget.clone()),
            )
            .await?,
        );
        for job in jobs.recoverable().await? {
            if matches!(job.status, JobStatus::Transcribing {}) {
                crate::latency::measurements::interrupt_job(&jobs.latency_path(), job.id);
                let (status, recovery) = interrupted(job);
                if jobs
                    .set_execution(status.0, status.1, recovery)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "Job recovery write deferred; retained history remains readable"
                    );
                }
            }
        }
        Ok(Arc::new(Self {
            _directory_lock: directory_lock,
            preview_root,
            recordings,
            jobs,
            work,
            providers,
            mode_root,
            modes: OnceCell::const_new(),
            batches: OnceCell::const_new(),
            wake: Notify::new(),
            execution: Mutex::new(std::collections::BTreeMap::new()),
        }))
    }
    /// Runs queued jobs sequentially; ingestion and job status polling remain independent.
    pub async fn run_pending(&self) -> Result<(), ApiError> {
        let mut pending_results = self.execution.lock().await;
        for (id, (status, recovery)) in pending_results.clone() {
            let committed = self.jobs.set_execution(id, status, recovery).await?;
            pending_results.remove(&id);
            self.jobs.measure(&committed);
            if !committed.recovery.as_ref().is_some_and(|r| r.canceled) {
                self.process_recorded_mode(id).await;
            }
        }
        for mut job in self.jobs.recoverable().await? {
            if matches!(job.status, JobStatus::Transcribing {}) {
                crate::latency::measurements::interrupt_job(&self.jobs.latency_path(), job.id);
                let (status, recovery) = interrupted(job);
                job = self
                    .jobs
                    .set_execution(status.0, status.1, recovery)
                    .await?;
            }
            if !matches!(job.status, JobStatus::Queued {}) {
                continue;
            }
            crate::latency::measurements::ensure_registered(&job, self.jobs.latency_path());
            let mut recovery = job.recovery.clone().unwrap_or_default();
            if recovery.attempts_started >= 3 {
                let failure = recovery.failures.last().map(|f| f.error.clone()).unwrap_or_else(|| error(ErrorCode::Interrupted, "Automatic transcription retries exhausted; saved audio can be retried manually.", true));
                self.jobs
                    .set_execution(job.id, JobStatus::Failed { error: failure }, recovery)
                    .await?;
                continue;
            }
            if recovery
                .next_retry_at_ms
                .is_some_and(|at| at > super::disk::now())
            {
                continue;
            }
            let provider = self
                .providers
                .iter()
                .find(|p| {
                    std::mem::discriminant(&p.info().id)
                        == std::mem::discriminant(&job.request.options.selection.provider)
                })
                .cloned();
            if recovery.attempts_started > 0
                && provider
                    .as_ref()
                    .is_some_and(|p| !p.automatic_retry_ready())
            {
                if !recovery.waiting_for_capacity {
                    recovery.waiting_for_capacity = true;
                    self.jobs
                        .set_execution(job.id, JobStatus::Queued {}, recovery)
                        .await?;
                }
                continue;
            }
            let source = self.recordings.audio(job.request.recording_id).await;
            let mut may_retry = false;
            let result = match (source, provider) {
                (Err(error), _) => Err(error),
                (_, None) => Err(error(ErrorCode::ProviderUnavailable, "Selected provider is not registered; select another provider to retry saved audio.", false)),
                (Ok(audio), Some(provider)) => {
                    let permit = match self.work.try_admit_transcription() {
                        Some(permit) => permit,
                        None => {
                            if !recovery.waiting_for_capacity {
                                recovery.waiting_for_capacity = true;
                                self.jobs.set_execution(job.id, JobStatus::Queued {}, recovery).await?;
                            }
                            continue;
                        }
                    };
                    crate::latency::measurements::start_attempt(&job, recovery.attempts_started + 1);
                    crate::latency::mark(job.request.recording_id, "recognition_started");
                    crate::latency::measurements::flush(job.request.recording_id);
                    recovery.attempts_started += 1;
                    recovery.waiting_for_capacity = false;
                    recovery.next_retry_at_ms = None;
                    let started = self.jobs.set_execution(job.id, JobStatus::Transcribing {}, recovery.clone()).await?;
                    if started.recovery.as_ref().is_some_and(|r| r.canceled) {
                        self.jobs.set_execution(job.id, JobStatus::Failed { error: canceled_error() }, started.recovery.unwrap()).await?;
                        continue;
                    }
                    let options = job.request.options.clone();
                    let adapter = provider.clone();
                    let timeout = self.work.timeout();
                    // Preserve a completed response in memory if its durable write fails.
                    // Adapter panics remain explicit terminal errors; worker/network exits
                    // returned by an adapter use its transient-error classification.
                    match tokio::spawn(async move {
                        let _permit = permit;
                        tokio::time::timeout(timeout, adapter.transcribe(&audio, &options)).await
                            .unwrap_or_else(|_| Err(error(ErrorCode::ProviderTimeout, "Processing timed out; saved audio is available for retry.", true)))
                    }).await {
                        Ok(result) => {
                            may_retry = result.as_ref().err().is_some_and(|e| provider.automatic_retry(e));
                            result
                        }
                        Err(_) => Err(error(ErrorCode::ProviderFailed, "Transcription worker stopped unexpectedly; saved audio is available for retry.", true)),
                    }
                }
            };
            crate::latency::mark(job.request.recording_id, "recognition_finished");
            let status = match result {
                Ok(transcript) => JobStatus::Succeeded {
                    result: TranscriptionResult {
                        transcript,
                        cleanup: match job.request.options.cleanup {
                            CleanupPolicy::Off {} => CleanupResult::Off {},
                            CleanupPolicy::Enabled { .. } => CleanupResult::Pending {},
                        },
                    },
                },
                Err(error) => {
                    recovery.failures.push(RecognitionFailure {
                        attempt: recovery.attempts_started,
                        at_ms: super::disk::now(),
                        error: error.clone(),
                    });
                    if may_retry && recovery.attempts_started < 3 {
                        recovery.next_retry_at_ms = Some(
                            super::disk::now()
                                .saturating_add(retry_delay(recovery.attempts_started, &error)),
                        );
                        JobStatus::Queued {}
                    } else {
                        JobStatus::Failed { error }
                    }
                }
            };
            pending_results.insert(job.id, (status.clone(), recovery.clone()));
            let committed = self.jobs.set_execution(job.id, status, recovery).await?;
            pending_results.remove(&job.id);
            self.jobs.measure(&committed);
            if !committed.recovery.as_ref().is_some_and(|r| r.canceled) {
                self.process_recorded_mode(job.id).await;
            }
        }
        Ok(())
    }
    async fn process_recorded_mode(&self, id: uuid::Uuid) {
        let Ok(job) = self.jobs.get(id).await else {
            return;
        };
        if !matches!(job.status, JobStatus::Succeeded { .. })
            || job.recovery.as_ref().is_some_and(|r| r.canceled)
        {
            return;
        }
        if let Ok(service) = self.mode_service().await {
            let pending_context = service
                .captured_mode(job.request.recording_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|captured| captured.context_pending);
            let process = async move {
                if service.start_recorded(id).await.is_err() {
                    tracing::warn!(
                        "Live mode could not start; raw transcript retained for explicit processing."
                    );
                }
            };
            if pending_context {
                // Raw is durable and its recognition permit is released. An OS
                // context timeout must not stall later jobs in this raw queue.
                tokio::spawn(process);
            } else {
                process.await;
            }
        }
    }
    pub async fn mode_service(&self) -> Result<Arc<crate::cleanup::modes::ModeService>, ApiError> {
        self.modes
            .get_or_try_init(|| {
                crate::cleanup::modes::ModeService::open_budgeted(
                    self.mode_root.clone(),
                    self.jobs.clone(),
                    self.work.clone(),
                    self.recordings.budget.clone(),
                )
            })
            .await
            .cloned()
    }
    pub async fn delete_history(&self) -> Result<(usize, usize), ApiError> {
        let batches = self.batch_service().await?;
        let _batch_guard = batches.begin_deletion().await?;
        let setup = crate::desktop_settings::global()
            .ok()
            .filter(|s| self.mode_root == s.layout.state_dir.join("modes"));
        let _model_maintenance = setup
            .as_ref()
            .map(|s| s.runtime.maintenance())
            .transpose()?;
        let modes = self.mode_service().await?;
        let processing = modes.begin_history_deletion().await?;
        let _preview = crate::local_stt::pause_previews()?;
        let counts = self.jobs.delete_history(&self.recordings).await?;
        batches.clear_all().await?;
        processing.commit().await?;
        tokio::fs::remove_dir_all(&self.preview_root)
            .await
            .map_err(crate::recordings::disk::storage)?;
        crate::recordings::disk::private_dir(&self.preview_root).await?;
        for provider in &self.providers {
            provider.clear_preview();
        }
        Ok(counts)
    }
    pub fn wake_worker(&self) {
        self.wake.notify_one();
    }
    pub async fn batch_service(&self) -> Result<Arc<super::batches::BatchService>, ApiError> {
        self.batches
            .get_or_try_init(|| {
                super::batches::BatchService::open(
                    self.preview_root.parent().unwrap().join("batches"),
                    self.recordings.budget.clone(),
                )
            })
            .await
            .cloned()
    }
    pub fn start_worker(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(pipeline) = weak.upgrade() else {
                    break;
                };
                if pipeline.run_pending().await.is_err() {
                    tracing::error!("Transcription job persistence failed; worker will retry after storage recovers");
                }
                tokio::select! {
                    _ = pipeline.wake.notified() => {},
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                }
                drop(pipeline);
            }
        });
    }
}

fn retry_delay(attempt: u64, error: &ApiError) -> u64 {
    (if attempt <= 1 { 2_000_u64 } else { 5_000_u64 }).max(error.retry_after_ms.unwrap_or(0))
}
fn canceled_error() -> ApiError {
    error(
        ErrorCode::Interrupted,
        "Transcription retry canceled; saved audio is retained.",
        false,
    )
}
fn interrupted(job: Job) -> ((uuid::Uuid, JobStatus), RecognitionRecovery) {
    let mut recovery = job.recovery.unwrap_or_default();
    // Old files (and pre-admission transitions) have no recorded provider attempt.
    if recovery.attempts_started == 0 {
        return ((job.id, JobStatus::Queued {}), recovery);
    }
    let failure = error(
        ErrorCode::Interrupted,
        "Writer stopped during transcription; saved audio is retained.",
        true,
    );
    recovery.failures.push(RecognitionFailure {
        attempt: recovery.attempts_started,
        at_ms: super::disk::now(),
        error: failure.clone(),
    });
    recovery.waiting_for_capacity = false;
    let status = if recovery.canceled || recovery.attempts_started >= 3 {
        recovery.next_retry_at_ms = None;
        JobStatus::Failed {
            error: if recovery.canceled {
                canceled_error()
            } else {
                failure
            },
        }
    } else {
        recovery.next_retry_at_ms = Some(
            super::disk::now().saturating_add(retry_delay(recovery.attempts_started, &failure)),
        );
        JobStatus::Queued {}
    };
    ((job.id, status), recovery)
}

static RUNTIME: OnceCell<Arc<Pipeline>> = OnceCell::const_new();
/// Shared process runtime; callers including cleanup/discovery reuse the same adapters and stores.
pub async fn runtime() -> Result<Arc<Pipeline>, ApiError> {
    RUNTIME
        .get_or_try_init(|| async {
            let root = std::env::var_os("WRITER_DATA_DIR")
                .map(PathBuf::from)
                .or_else(|| {
                    #[cfg(windows)]
                    return std::env::var_os("LOCALAPPDATA")
                        .map(|p| {
                            PathBuf::from(p)
                                .join(crate::desktop_settings::DESKTOP_DATA_FOLDER)
                                .join("Data")
                        });
                    #[cfg(target_os = "macos")]
                    return std::env::var_os("HOME").map(|home| {
                        PathBuf::from(home)
                            .join("Library/Application Support")
                            .join(crate::desktop_settings::DESKTOP_DATA_FOLDER)
                            .join("Data")
                    });
                    #[cfg(not(any(windows, target_os = "macos")))]
                    std::env::var_os("HOME")
                        .map(|home| PathBuf::from(home).join(".local/share/metrix-writer"))
                })
                .ok_or_else(|| {
                    error(
                        ErrorCode::StorageFailed,
                        "Set WRITER_DATA_DIR to a private writable data directory.",
                        false,
                    )
                })?;
            let max = match std::env::var("WRITER_MAX_UPLOAD_BYTES") {
                Ok(value) => value
                    .parse::<u64>()
                    .ok()
                    .filter(|v| *v > 0)
                    .ok_or_else(|| {
                        error(
                            ErrorCode::InvalidRequest,
                            "WRITER_MAX_UPLOAD_BYTES must be a positive integer.",
                            false,
                        )
                    })?,
                Err(_) => 1024 * 1024 * 1024,
            };
            // Persistence must remain available when model/settings state is corrupt.
            let mode_root = crate::desktop_settings::Layout::from_env()?
                .state_dir
                .join("modes");
            let pipeline =
                Pipeline::open_with_modes(root, max, providers::adapters(), mode_root).await?;
            // Recover the crash window after raw persistence but before creating
            // the mode attempt. Existing pending/failed attempts are never rebilled.
            for job in pipeline.jobs.all().await? {
                if matches!(job.status, JobStatus::Succeeded { .. }) {
                    pipeline.process_recorded_mode(job.id).await;
                }
            }
            pipeline.start_worker();
            Ok(pipeline)
        })
        .await
        .cloned()
}

/// HTTP callers may supply an isolated store; desktop callers retain the global runtime.
pub(crate) async fn selected(injected: Option<Arc<Pipeline>>) -> Result<Arc<Pipeline>, ApiError> {
    match injected {
        Some(pipeline) => Ok(pipeline),
        None => runtime().await,
    }
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    #[tokio::test]
    async fn dropping_owner_releases_lease_even_while_a_duplicate_descriptor_exists() {
        let root = std::env::temp_dir().join(format!("writer-lease-{}", uuid::Uuid::new_v4()));
        let owner = Pipeline::open(root.clone(), 1024, vec![]).await.unwrap();
        let inherited = owner._directory_lock.try_clone().unwrap();
        drop(owner);
        let next = Pipeline::open(root.clone(), 1024, vec![]).await.unwrap();
        drop(inherited);
        drop(next);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
