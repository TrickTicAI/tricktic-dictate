//! Historical work computes a new value; it never updates live attempts or delivery.
use super::*;
use crate::transcription::Vocabulary;

pub(crate) fn historical_identity() -> serde_json::Value {
    serde_json::json!({"prompt_version": PROMPT_VERSION, "custom_models": super::super::custom::global().list()})
}

impl ModeService {
    pub(crate) async fn historical(
        &self,
        raw: &str,
        vocabulary: &Vocabulary,
        profile: ModeProfile,
    ) -> Result<String, ApiError> {
        processing::validate_profile(&profile)?;
        if profile.kind == ModeKind::Command {
            return Err(processing::invalid("Historical cleanup supports Regular or Super modes; choose an explicit cleanup mode."));
        }
        let attempt = ProcessingAttempt {
            id: Uuid::new_v4(),
            job_id: Uuid::nil(),
            created_at_ms: crate::jobs::disk::now(),
            request: ProcessRequest {
                operation_id: Uuid::new_v4(),
                profile_id: profile.id.clone(),
                profile_revision: profile.revision,
                context: ContextSnapshot::default(),
            },
            request_digest: None,
            profile,
            raw_text: raw.into(),
            input_text: raw.into(),
            prompt_version: PROMPT_VERSION,
            state: AttemptState::Pending,
            diagnostics: Diagnostics::default(),
        };
        tokio::time::timeout(
            processing::polish_timeout(&attempt.profile, self.processor.as_ref()),
            processing::run(&attempt, vocabulary, self.processor.as_ref()),
        )
        .await
        .map_err(|_| {
            super::super::failure(
                crate::transcription::ErrorCode::ProviderTimeout,
                "Historical cleanup timed out; the original raw transcript remains saved.",
                true,
            )
        })?
    }
}
