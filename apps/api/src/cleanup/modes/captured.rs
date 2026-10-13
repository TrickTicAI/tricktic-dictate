use super::*;
use crate::recordings::disk::{private_dir, read_json, write_json};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub struct CapturedMode {
    pub recording_id: Uuid,
    pub profile: ModeProfile,
    pub context: ContextSnapshot,
    #[serde(default)]
    pub context_erased: bool,
    /// Native capture can listen while the permitted OS context is collected.
    /// Only this unresolved snapshot can be completed; attempts stay immutable.
    #[serde(default)]
    pub context_pending: bool,
}
#[derive(Serialize)]
pub struct LiveProcessing {
    pub bound: bool,
    pub attempt: Option<ProcessingAttempt>,
    pub selected_attempt: Option<ProcessingAttempt>,
    pub delivery: Option<DeliveryReceipt>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub attempt_id: Uuid,
    pub original_used: bool,
    pub fallback_used: bool,
    pub selected_model: Option<crate::transcription::ModelSelection>,
    pub copied: bool,
    pub pasted: bool,
    pub at_ms: u64,
    #[serde(default)]
    pub stop_to_delivery_ms: Option<u64>,
    /// The application the text went to, by the name it gives itself
    /// ("Slack"). Kept in local history only; never a window title or path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}
#[derive(Serialize)]
pub struct PreparedDelivery {
    pub text: String,
    pub original_used: bool,
    pub fallback_used: bool,
    pub selected_model: Option<crate::transcription::ModelSelection>,
    pub notice: Option<String>,
}
impl ModeService {
    pub async fn prepare_delivery(
        self: &Arc<Self>,
        job_id: Uuid,
    ) -> Result<PreparedDelivery, ApiError> {
        let text = self.delivery_text(job_id).await?;
        let attempt = self.live_processing(job_id).await?.attempt;
        let diagnostics = attempt.map(|a| a.diagnostics).unwrap_or_default();
        Ok(PreparedDelivery {
            text,
            original_used: diagnostics.original_used,
            fallback_used: diagnostics.fallback_used,
            selected_model: diagnostics.selected_model,
            notice: diagnostics
                .original_used
                .then(|| "Polishing unavailable; using the original.".into()),
        })
    }
    /// Called only after native delivery returns. Contains no text, context or
    /// window title: only where the words went, by the application's own name.
    pub async fn record_delivery(
        &self,
        job_id: Uuid,
        copied: bool,
        pasted: bool,
        stop_to_delivery_ms: Option<u64>,
        app: Option<String>,
    ) -> Result<(), ApiError> {
        let _lock = self.lock.lock().await;
        let path = self.root.join(job_id.to_string());
        let mut attempt: ProcessingAttempt =
            read_json(&path.join(format!("{job_id}.json"))).await?;
        let receipt = DeliveryReceipt {
            attempt_id: attempt.id,
            original_used: attempt.diagnostics.original_used,
            fallback_used: attempt.diagnostics.fallback_used,
            selected_model: attempt.diagnostics.selected_model.clone(),
            copied,
            pasted,
            at_ms: crate::local_stt::managed::now_ms(),
            stop_to_delivery_ms,
            // Bounded, so a receipt stays a receipt whatever a process calls itself.
            app: app
                .map(|name| name.chars().take(80).collect::<String>())
                .filter(|name| !name.trim().is_empty()),
        };
        attempt.diagnostics.delivery = Some(receipt.clone());
        write_json(&path.join(format!("{job_id}.json")), &attempt).await?;
        write_json(&path.join("delivery"), &receipt).await
    }
    /// Event-driven final text for native delivery, including the captured mode.
    /// Raw recognition is already durable. Failure never promotes unfinished text.
    pub async fn delivery_text(self: &Arc<Self>, job_id: Uuid) -> Result<String, ApiError> {
        let job = self.jobs.get(job_id).await?;
        let crate::transcription::JobStatus::Succeeded { result } = &job.status else {
            return Err(processing::invalid("Transcription has not completed."));
        };
        if self.start_recorded(job_id).await?.is_none() {
            return Ok(crate::vocabulary::process(
                &result.transcript.raw_text,
                &job.request.options.vocabulary,
            )?
            .text());
        }
        loop {
            let notified = self.finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(attempt) = self.live_processing(job_id).await?.attempt {
                match attempt.state {
                    AttemptState::Succeeded { text } => return Ok(text),
                    AttemptState::Failed { error } => return Err(error),
                    AttemptState::Pending => {}
                }
            }
            tokio::select! {
                _ = notified => {},
                // Recovery fallback for failed writes; no WebView timer dependency.
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
            }
        }
    }

    /// Commit the selected profile and permitted context for this recording.
    pub async fn bind_recording(
        &self,
        id: Uuid,
        profile: ModeProfile,
        context: ContextSnapshot,
    ) -> Result<CapturedMode, ApiError> {
        self.bind_capture(id, profile, context, false).await
    }

    /// Persist the profile before opening the microphone, without waiting for
    /// optional context. A failed/interrupted read retains raw audio and text;
    /// Command still requires a verified selection before any processing.
    pub async fn begin_context_capture(
        &self,
        id: Uuid,
        profile: ModeProfile,
    ) -> Result<CapturedMode, ApiError> {
        self.bind_capture(id, profile, ContextSnapshot::default(), true)
            .await
    }

    async fn bind_capture(
        &self,
        id: Uuid,
        profile: ModeProfile,
        context: ContextSnapshot,
        pending: bool,
    ) -> Result<CapturedMode, ApiError> {
        let _lock = self.lock.lock().await;
        let directory = self.root.join("captures");
        let path = directory.join(format!("{id}.json"));
        let context = if pending {
            ContextSnapshot::default()
        } else {
            processing::permitted_context(&profile, &context)?
        };
        let captured = CapturedMode {
            recording_id: id,
            context_erased: false,
            context_pending: pending,
            profile,
            context,
        };
        if tokio::fs::try_exists(&path).await.map_err(storage)? {
            let previous: CapturedMode = read_json(&path).await?;
            if serde_json::to_value(&previous).map_err(storage)?
                != serde_json::to_value(&captured).map_err(storage)?
            {
                return Err(processing::invalid(
                    "This recording already has a different mode snapshot.",
                ));
            }
            crate::latency::detail(
                id,
                "mode",
                serde_json::to_value(previous.profile.kind).unwrap_or_default(),
            );
            return Ok(previous);
        }
        let bytes = serde_json::to_vec(&captured).map_err(storage)?.len() as u64;
        let _reservation = self
            .budget
            .reserve(&self.root, path.clone(), bytes + 16384)
            .await?;
        processing::validate_profile(&captured.profile)?;
        private_dir(&directory).await?;
        write_json(&path, &captured).await?;
        crate::latency::detail(
            id,
            "mode",
            serde_json::to_value(captured.profile.kind).unwrap_or_default(),
        );
        // The microphone opens next; the text provider connection is prepared
        // while the user speaks instead of after release.
        if captured.profile.kind != ModeKind::Regular {
            for model in captured
                .profile
                .text_model
                .iter()
                .chain(captured.profile.fallback_model.iter())
            {
                self.processor.warm(model);
            }
        }
        if captured.profile.kind == ModeKind::Super && captured.profile.process_while_speaking {
            self.drafts.begin(id).await;
        }
        Ok(captured)
    }

    /// Complete only the original pending binding, serialized with deletion,
    /// erasure and timeout recovery. Never recreate a discarded capture or
    /// overwrite context already consumed by a processing attempt.
    pub async fn complete_context_capture(
        &self,
        recording: Uuid,
        context: Option<ContextSnapshot>,
    ) -> Result<bool, ApiError> {
        let _lock = self.lock.lock().await;
        let path = self.root.join("captures").join(format!("{recording}.json"));
        if !tokio::fs::try_exists(&path).await.map_err(storage)? {
            return Ok(false);
        }
        let mut captured: CapturedMode = read_json(&path).await?;
        if !captured.context_pending || captured.context_erased {
            return Ok(false);
        }
        let permitted =
            context.and_then(|c| processing::permitted_context(&captured.profile, &c).ok());
        captured.context_pending = false;
        captured.context_erased = permitted.is_none();
        captured.context = permitted.unwrap_or_default();
        let bytes = serde_json::to_vec(&captured).map_err(storage)?.len() as u64;
        let _reservation = self
            .budget
            .reserve(&self.root, path.clone(), bytes + 16384)
            .await?;
        write_json(&path, &captured).await?;
        self.finished.notify_waiters();
        Ok(true)
    }

    async fn resolved_capture(&self, recording: Uuid) -> Result<Option<CapturedMode>, ApiError> {
        // The OS reader has a five-second deadline. Recovery after a crashed
        // reader is bounded too; no wait holds the mode lock or raw ASR capacity.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let notified = self.finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let captured = self.captured_mode(recording).await?;
            if captured.as_ref().is_none_or(|c| !c.context_pending) {
                return Ok(captured);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                self.complete_context_capture(recording, None).await?;
            }
        }
    }
    pub async fn captured_mode(&self, recording: Uuid) -> Result<Option<CapturedMode>, ApiError> {
        let _lock = self.lock.lock().await;
        let path = self.root.join("captures").join(format!("{recording}.json"));
        if !tokio::fs::try_exists(&path).await.map_err(storage)? {
            return Ok(None);
        }
        read_json(&path).await.map(Some)
    }
    pub async fn start_recorded(
        self: &Arc<Self>,
        job_id: Uuid,
    ) -> Result<Option<ProcessingAttempt>, ApiError> {
        let job = self.jobs.get(job_id).await?;
        if !matches!(
            job.status,
            crate::transcription::JobStatus::Succeeded { .. }
        ) {
            return Ok(None);
        }
        // Reuse durable results even after their context has been erased.
        if let Some(attempt) = self
            .attempts(job_id)
            .await?
            .into_iter()
            .find(|a| a.id == job_id)
        {
            return Ok(Some(attempt));
        }
        let Some(captured) = self.resolved_capture(job.request.recording_id).await? else {
            return Ok(None);
        };
        // A bounded context wait can outlive cancellation/deletion. Never start
        // automatic polishing from the stale parent read above.
        let job = self.jobs.get(job_id).await?;
        if job
            .recovery
            .as_ref()
            .is_some_and(|recovery| recovery.canceled)
        {
            return Ok(None);
        }
        if captured.context_erased {
            // Persist a visible failure so both native delivery and history stop
            // waiting. Never run a new Command against an empty/replaced target.
            return self.context_unavailable(job, captured).await.map(Some);
        }
        let request = ProcessRequest {
            operation_id: job_id,
            profile_id: captured.profile.id.clone(),
            profile_revision: captured.profile.revision,
            context: captured.context,
        };
        self.start_with_profile(job_id, request, Some(captured.profile))
            .await
            .map(Some)
    }
    async fn context_unavailable(
        &self,
        job: crate::transcription::Job,
        captured: CapturedMode,
    ) -> Result<ProcessingAttempt, ApiError> {
        let _lock = self.lock.lock().await;
        // Serialize with deletion, and recheck the parent and any racing caller.
        self.jobs.get(job.id).await?;
        let path = self
            .root
            .join(job.id.to_string())
            .join(format!("{}.json", job.id));
        if tokio::fs::try_exists(&path).await.map_err(storage)? {
            return read_json(&path).await;
        }
        let crate::transcription::JobStatus::Succeeded { result } = job.status else {
            return Err(processing::invalid("Transcription has not completed."));
        };
        let mut attempt = ProcessingAttempt {
            id: job.id,
            job_id: job.id,
            created_at_ms: crate::local_stt::managed::now_ms(),
            request: ProcessRequest {
                operation_id: job.id,
                profile_id: captured.profile.id.clone(),
                profile_revision: captured.profile.revision,
                context: ContextSnapshot::default(),
            },
            request_digest: None,
            profile: captured.profile,
            input_text: crate::vocabulary::process(&result.transcript.raw_text, &job.request.options.vocabulary)?.text(),
            raw_text: result.transcript.raw_text,
            prompt_version: PROMPT_VERSION,
            state: AttemptState::Failed { error: processing::invalid("Original context is unavailable. Process the saved transcript with fresh context, or start a new dictation.") },
            diagnostics: Diagnostics::default(),
        };
        if attempt.profile.kind == ModeKind::Super {
            attempt.diagnostics.original_used = true;
            attempt.diagnostics.original_reason =
                Some(crate::transcription::ErrorCode::Interrupted);
            attempt.state = AttemptState::Succeeded {
                text: attempt.input_text.clone(),
            };
            super::retention::erase_success_context(&mut attempt)?;
        }
        private_dir(path.parent().unwrap()).await?;
        write_json(&path, &attempt).await?;
        self.finished.notify_waiters();
        Ok(attempt)
    }
    pub async fn live_processing(&self, job_id: Uuid) -> Result<LiveProcessing, ApiError> {
        let job = self.jobs.get(job_id).await?;
        let bound = self
            .captured_mode(job.request.recording_id)
            .await?
            .is_some();
        let attempts = self.attempts(job_id).await?;
        let attempt = attempts.iter().find(|a| a.id == job_id).cloned();
        let dir = self.root.join(job_id.to_string());
        let selected = if tokio::fs::try_exists(dir.join("selected"))
            .await
            .map_err(storage)?
        {
            Some(read_json::<Uuid>(&dir.join("selected")).await?)
        } else {
            None
        };
        let selected_attempt = selected.and_then(|id| attempts.into_iter().find(|a| a.id == id));
        let delivery = if tokio::fs::try_exists(dir.join("delivery"))
            .await
            .map_err(storage)?
        {
            Some(read_json(&dir.join("delivery")).await?)
        } else {
            None
        };
        Ok(LiveProcessing {
            bound,
            attempt,
            selected_attempt,
            delivery,
        })
    }
}
fn storage(_: impl std::fmt::Display) -> ApiError {
    crate::cleanup::failure(
        crate::transcription::ErrorCode::StorageFailed,
        "Mode snapshot storage is unavailable. Saved audio and transcripts are retained.",
        true,
    )
}
