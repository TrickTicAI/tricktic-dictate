use super::{
    processing::{self, Processor},
    *,
};
use crate::{
    cleanup::{failure, provider::CloudText},
    jobs::JobStore,
    recordings::disk::{private_dir, read_json, write_json},
    transcription::{ErrorCode, JobStatus},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{Mutex, Notify};

pub struct ModeService {
    pub(super) root: PathBuf,
    pub(super) budget: Arc<crate::limits::storage::StorageBudget>,
    pub(super) jobs: Arc<dyn JobStore>,
    pub(super) processor: Arc<dyn Processor>,
    pub(super) finished: Notify,
    pub(super) drafts: speculative::Drafts,
    pub(super) preview_slots: tokio::sync::Semaphore,
    pub(super) lock: Mutex<BTreeSet<Uuid>>,
    pub(super) progress: std::sync::Mutex<BTreeMap<Uuid, Arc<std::sync::Mutex<Diagnostics>>>>,
}
impl ModeService {
    pub async fn open(root: PathBuf, jobs: Arc<dyn JobStore>) -> Result<Arc<Self>, ApiError> {
        Self::open_with(root, jobs, Arc::new(CloudText::from_env())).await
    }
    pub(crate) async fn open_budgeted(
        root: PathBuf,
        jobs: Arc<dyn JobStore>,
        work: crate::limits::work::WorkAccount,
        budget: Arc<crate::limits::storage::StorageBudget>,
    ) -> Result<Arc<Self>, ApiError> {
        let mut service = Self::open_with(
            root.clone(),
            jobs,
            Arc::new(CloudText::from_env().with_budget(work)),
        )
        .await?;
        budget.include(root);
        Arc::get_mut(&mut service).unwrap().budget = budget;
        Ok(service)
    }
    pub(super) async fn open_with(
        root: PathBuf,
        jobs: Arc<dyn JobStore>,
        processor: Arc<dyn Processor>,
    ) -> Result<Arc<Self>, ApiError> {
        private_dir(&root).await?;
        let budget = crate::limits::storage::StorageBudget::new(
            root.clone(),
            crate::limits::Limits::from_env()?,
        );
        let service = Arc::new(Self {
            budget,
            root,
            jobs,
            processor,
            drafts: speculative::Drafts::default(),
            preview_slots: tokio::sync::Semaphore::new(2),
            lock: Mutex::new(BTreeSet::new()),
            finished: Notify::new(),
            progress: Default::default(),
        });
        service.minimize_completed_context().await?;
        Ok(service)
    }
    pub(super) async fn read_settings(&self) -> Result<ModeSettings, ApiError> {
        let path = self.root.join("settings.json");
        if !tokio::fs::try_exists(&path).await.map_err(storage)? {
            return Ok(ModeSettings::default());
        }
        let document: serde_json::Value = read_json(&path).await?;
        let early_setting_absent = document["profiles"].as_array().is_some_and(|profiles| {
            profiles
                .iter()
                .any(|p| p["id"] == "super" && p.get("process_while_speaking").is_none())
        });
        let fast_setting_absent = document["profiles"].as_array().is_some_and(|profiles| {
            profiles
                .iter()
                .any(|p| p["id"] == "super" && p.get("fast_processing").is_none())
        });
        let mut settings: ModeSettings = serde_json::from_value(document).map_err(storage)?;
        // Only the exact factory prompt is migrated. Captured attempts are read
        // separately and remain immutable, as do edited/duplicated profiles.
        let mut migrated = false;
        for profile in &mut settings.profiles {
            if profile.id == "super"
                && profile.kind == ModeKind::Super
                && profile.instructions == style::LEGACY_SUPER
            {
                profile.instructions = style::POLISHED_SUPER.into();
                // The maintainer opted into bounded early cloud processing. Only migrate
                // the factory profile before the setting existed; respect false.
                if fast_setting_absent {
                    profile.fast_processing = true;
                }
                if early_setting_absent {
                    profile.process_while_speaking = true;
                }
                profile.revision = profile
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| processing::invalid("Mode revision exhausted."))?;
                migrated = true;
            }
        }
        if migrated {
            settings.revision = settings
                .revision
                .checked_add(1)
                .ok_or_else(|| processing::invalid("Settings revision exhausted."))?;
            write_json(&path, &settings).await?;
        }
        Ok(settings)
    }
    pub async fn settings(&self) -> Result<ModeSettings, ApiError> {
        let _lock = self.lock.lock().await;
        self.read_settings().await
    }
    pub async fn save(&self, mut next: ModeSettings) -> Result<ModeSettings, ApiError> {
        let _lock = self.lock.lock().await;
        let current = self.read_settings().await?;
        if current.revision != next.revision {
            return Err(conflict(
                "Modes changed in another window. Reload before saving.",
            ));
        }
        if next.profiles.is_empty()
            || next.profiles.len() > 100
            || !next.profiles.iter().any(|p| p.id == next.active)
        {
            return Err(processing::invalid(
                "Choose an existing mode; at most 100 profiles are supported.",
            ));
        }
        let mut ids = BTreeSet::new();
        for profile in &mut next.profiles {
            processing::validate_profile(profile)?;
            if !ids.insert(profile.id.clone()) {
                return Err(processing::invalid("Mode IDs must be unique."));
            }
            if let Some(old) = current.profiles.iter().find(|p| p.id == profile.id) {
                if old.revision != profile.revision {
                    return Err(conflict("This mode changed. Reload before saving."));
                }
                if serde_json::to_value(old).map_err(storage)?
                    != serde_json::to_value(&profile).map_err(storage)?
                {
                    profile.revision = old
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| processing::invalid("Mode revision exhausted."))?;
                }
            } else {
                profile.revision = 1;
            }
        }
        next.revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| processing::invalid("Settings revision exhausted."))?;
        write_json(&self.root.join("settings.json"), &next).await?;
        crate::account_sync::note_local_change();
        Ok(next)
    }
    fn attempt_path(&self, job: Uuid, id: Uuid) -> PathBuf {
        self.root.join(job.to_string()).join(format!("{id}.json"))
    }
    pub async fn attempts(&self, job: Uuid) -> Result<Vec<ProcessingAttempt>, ApiError> {
        self.jobs.get(job).await?;
        let active = self.lock.lock().await;
        let mut output = Vec::new();
        let dir = self.root.join(job.to_string());
        if !tokio::fs::try_exists(&dir).await.map_err(storage)? {
            return Ok(output);
        }
        let mut files = tokio::fs::read_dir(&dir).await.map_err(storage)?;
        while let Some(entry) = files.next_entry().await.map_err(storage)? {
            if entry.path().extension().is_none_or(|s| s != "json") {
                continue;
            }
            let mut attempt: ProcessingAttempt = read_json(&entry.path()).await?;
            if matches!(attempt.state, AttemptState::Pending) && !active.contains(&attempt.id) {
                if attempt.profile.kind == ModeKind::Super {
                    attempt.diagnostics.original_used = true;
                    attempt.diagnostics.original_reason = Some(ErrorCode::Interrupted);
                    attempt.diagnostics.selected_model = None;
                    attempt.state = AttemptState::Succeeded {
                        text: attempt.input_text.clone(),
                    };
                } else {
                    attempt.state = AttemptState::Failed { error: failure(ErrorCode::Interrupted, "Processing was interrupted. Retry explicitly; the original transcript is available.", true) };
                }
                super::retention::erase_success_context(&mut attempt)?;
                write_json(&entry.path(), &attempt).await?;
                self.erase_completed_capture(&attempt).await?;
            }
            if matches!(attempt.state, AttemptState::Pending) {
                if let Some(metrics) = self.progress.lock().unwrap().get(&attempt.id) {
                    attempt.diagnostics = metrics.lock().unwrap().clone();
                }
            }
            output.push(attempt);
        }
        output.sort_by_key(|a| (a.created_at_ms, a.id));
        Ok(output)
    }
    pub async fn start(
        self: &Arc<Self>,
        job_id: Uuid,
        request: ProcessRequest,
    ) -> Result<ProcessingAttempt, ApiError> {
        self.start_with_profile(job_id, request, None).await
    }
    pub(super) async fn start_with_profile(
        self: &Arc<Self>,
        job_id: Uuid,
        mut request: ProcessRequest,
        captured: Option<ModeProfile>,
    ) -> Result<ProcessingAttempt, ApiError> {
        let mut active = self.lock.lock().await;
        // A cached operation must not bypass the parent job existence check.
        let job = self.jobs.get(job_id).await?;
        let path = self.attempt_path(job_id, request.operation_id);
        if tokio::fs::try_exists(&path).await.map_err(storage)? {
            let existing: ProcessingAttempt = read_json(&path).await?;
            request.context = processing::permitted_context(&existing.profile, &request.context)?;
            let matches = match &existing.request_digest {
                Some(digest) => super::retention::digest(&request)? == *digest,
                None => request == existing.request,
            };
            if !matches {
                return Err(conflict(
                    "This operation ID was already used with different input.",
                ));
            }
            return Ok(existing);
        }
        if captured.is_some()
            && !tokio::fs::try_exists(
                self.root
                    .join("captures")
                    .join(format!("{}.json", job.request.recording_id)),
            )
            .await
            .map_err(storage)?
        {
            return Err(processing::invalid(
                "This recording's context was discarded.",
            ));
        }
        let limits = crate::limits::Limits::from_env()?;
        if active.len() >= limits.device_work_concurrency {
            return Err(crate::limits::busy(
                "Text processing is busy; retry from the saved transcript.",
            ));
        }
        let reservation = self
            .budget
            .reserve(&self.root, path.clone(), 4 * 1024 * 1024)
            .await?;
        let profile = match captured {
            Some(profile) => profile,
            None => self
                .read_settings()
                .await?
                .profiles
                .into_iter()
                .find(|p| p.id == request.profile_id && p.revision == request.profile_revision)
                .ok_or_else(|| {
                    conflict("The selected mode changed. Reload it before processing.")
                })?,
        };
        let manual = request.operation_id != job_id;
        let cleanup_started = std::time::Instant::now();
        crate::latency::mark(job.request.recording_id, "cleanup_requested");
        processing::validate_profile(&profile)?;
        request.context = processing::permitted_context(&profile, &request.context)?;
        let JobStatus::Succeeded { result } = job.status else {
            return Err(processing::invalid(
                "Processing requires a successful saved transcript.",
            ));
        };
        let vocabulary = job.request.options.vocabulary;
        let input_text =
            crate::vocabulary::process(&result.transcript.raw_text, &vocabulary)?.text();
        let mut attempt = ProcessingAttempt {
            request_digest: None,
            id: request.operation_id,
            job_id,
            created_at_ms: crate::local_stt::managed::now_ms(),
            profile,
            request,
            raw_text: result.transcript.raw_text,
            input_text,
            prompt_version: PROMPT_VERSION,
            diagnostics: Diagnostics::default(),
            state: AttemptState::Pending,
        };
        private_dir(path.parent().unwrap()).await?;
        if attempt.profile.kind == ModeKind::Regular {
            // Deterministic corrections need one durable result, no pending write
            // and asynchronous model task followed by a second flush.
            attempt.state = AttemptState::Succeeded {
                text: attempt.input_text.clone(),
            };
            super::retention::erase_success_context(&mut attempt)?;
            write_json(&path, &attempt).await?;
            if manual {
                write_json(&path.parent().unwrap().join("selected"), &attempt.id).await?;
            }
            self.erase_completed_capture(&attempt).await?;
            crate::latency::mark(job.request.recording_id, "mode_result_durable");
            if let Some(path) = self.jobs.latency_path() {
                crate::latency::measurements::record_stage(
                    &path,
                    job_id,
                    attempt.profile.kind,
                    crate::latency::measurements::Stage::Cleanup,
                    cleanup_started.elapsed().as_millis() as u64,
                    crate::latency::measurements::Outcome::Succeeded,
                );
            }
            self.finished.notify_waiters();
            return Ok(attempt);
        }
        crate::latency::mark(job.request.recording_id, "mode_processing_start");
        write_json(&path, &attempt).await?;
        if manual {
            write_json(&path.parent().unwrap().join("selected"), &attempt.id).await?;
        }
        active.insert(attempt.id);
        let metrics = Arc::new(std::sync::Mutex::new(Diagnostics::default()));
        self.progress
            .lock()
            .unwrap()
            .insert(attempt.id, metrics.clone());
        let service = self.clone();
        let pending = attempt.clone();
        tokio::spawn(async move {
            let _reservation = reservation;
            let mut attempt = attempt;
            let processor = service.processor.clone();
            let input = attempt.clone();
            let timeout = if input.profile.kind == ModeKind::Super {
                processing::polish_timeout(&input.profile, processor.as_ref())
            } else if input
                .profile
                .text_model
                .as_ref()
                .is_some_and(|m| matches!(m.provider, crate::transcription::ProviderId::Local))
            {
                std::time::Duration::from_secs(900)
            } else {
                std::time::Duration::from_secs(120)
            };
            // Catch worker panic/timeout and persist a failure independently of the STT job.
            let recording_id = job.request.recording_id;
            let draft_service = service.clone();
            let output = tokio::spawn(async move {
                diagnostics::measure_into(metrics, async {
                    tokio::time::timeout(timeout, async {
                        let prepared = draft_service
                            .drafts
                            .take(recording_id, &input, &vocabulary)
                            .await;
                        processing::run_prepared(&input, &vocabulary, processor.as_ref(), prepared)
                            .await
                    })
                    .await
                    .unwrap_or_else(|_| {
                        diagnostics::update(processing::timed_out);
                        Err(failure(
                            ErrorCode::ProviderTimeout,
                            "Text processing timed out. Retry from the saved transcript.",
                            true,
                        ))
                    })
                })
                .await
            })
            .await
            .unwrap_or_else(|_| {
                (
                    Err(failure(
                        ErrorCode::ProviderFailed,
                        "Text processing stopped unexpectedly. Original text is retained.",
                        true,
                    )),
                    Diagnostics::default(),
                )
            });
            attempt.diagnostics = output.1;
            if output
                .0
                .as_ref()
                .err()
                .is_some_and(|e| matches!(e.code, ErrorCode::ProviderTimeout))
            {
                processing::timed_out(&mut attempt.diagnostics);
            }
            crate::latency::mark(recording_id, "mode_processing_done");
            attempt.state = match output.0 {
                Ok(text) => AttemptState::Succeeded { text },
                Err(error) if attempt.profile.kind == ModeKind::Super => {
                    attempt.diagnostics.original_used = true;
                    attempt.diagnostics.original_reason = Some(error.code);
                    attempt.diagnostics.selected_model = None;
                    AttemptState::Succeeded {
                        text: attempt.input_text.clone(),
                    }
                }
                Err(error) => AttemptState::Failed { error },
            };
            let mut active = service.lock.lock().await;
            let persisted = async {
                super::retention::erase_success_context(&mut attempt)?;
                write_json(&path, &attempt).await?;
                service.erase_completed_capture(&attempt).await
            }
            .await;
            if persisted.is_err() {
                // Persisted pending remains recoverable and will be surfaced as interrupted.
                tracing::error!("Mode result persistence failed; original transcript retained");
            } else {
                crate::latency::mark(recording_id, "mode_result_durable");
                if let Some(path) = service.jobs.latency_path() {
                    crate::latency::measurements::record_stage(
                        &path,
                        job_id,
                        attempt.profile.kind,
                        crate::latency::measurements::Stage::Cleanup,
                        cleanup_started.elapsed().as_millis() as u64,
                        if matches!(attempt.state, AttemptState::Succeeded { .. })
                            && !attempt.diagnostics.original_used
                        {
                            crate::latency::measurements::Outcome::Succeeded
                        } else {
                            crate::latency::measurements::Outcome::Failed
                        },
                    );
                }
            }
            active.remove(&attempt.id);
            service.progress.lock().unwrap().remove(&attempt.id);
            service.finished.notify_waiters();
        });
        Ok(pending)
    }
}
fn storage(_: impl std::fmt::Display) -> ApiError {
    failure(
        ErrorCode::StorageFailed,
        "Mode storage is unavailable. Existing recordings and transcripts are retained.",
        true,
    )
}
fn conflict(message: &str) -> ApiError {
    failure(ErrorCode::IdempotencyConflict, message, false)
}

/// Hold while the caller deletes parent jobs/audio, then commit processing-history
/// removal before acknowledging success. New attempts cannot race the deletion.
pub struct HistoryDeletion<'a> {
    service: &'a ModeService,
    _guard: tokio::sync::MutexGuard<'a, BTreeSet<Uuid>>,
}
impl ModeService {
    pub async fn begin_history_deletion(&self) -> Result<HistoryDeletion<'_>, ApiError> {
        let guard = self.lock.lock().await;
        if !guard.is_empty() {
            return Err(failure(
                ErrorCode::ProviderUnavailable,
                "Wait for text processing to finish before deleting history.",
                true,
            ));
        }
        self.drafts.clear().await;
        Ok(HistoryDeletion {
            service: self,
            _guard: guard,
        })
    }
}
impl HistoryDeletion<'_> {
    pub async fn commit_selected(self, recordings: &[Uuid], jobs: &[Uuid]) -> Result<(), ApiError> {
        for id in recordings {
            let path = self
                .service
                .root
                .join("captures")
                .join(format!("{id}.json"));
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(storage(e)),
            }
        }
        for id in jobs {
            let path = self.service.root.join(id.to_string());
            match tokio::fs::symlink_metadata(&path).await {
                Ok(m) if m.is_dir() && !m.is_symlink() => {
                    tokio::fs::remove_dir_all(path).await.map_err(storage)?
                }
                Ok(_) => return Err(storage("unexpected mode directory")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(storage(e)),
            }
        }
        Ok(())
    }
    pub async fn commit(self) -> Result<usize, ApiError> {
        let mut directories = tokio::fs::read_dir(&self.service.root)
            .await
            .map_err(storage)?;
        let mut count = 0;
        while let Some(entry) = directories.next_entry().await.map_err(storage)? {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name == "captures" || Uuid::parse_str(name).is_ok())
            {
                let kind = entry.file_type().await.map_err(storage)?;
                if kind.is_dir() && !kind.is_symlink() {
                    tokio::fs::remove_dir_all(entry.path())
                        .await
                        .map_err(storage)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}
