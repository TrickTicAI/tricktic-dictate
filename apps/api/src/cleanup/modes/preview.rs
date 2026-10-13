use super::*;
use crate::{transcription::Vocabulary, vocabulary::empty_vocabulary};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreviewRequest {
    pub profile: ModeProfile,
    pub text: String,
    #[serde(default)]
    pub context: ContextSnapshot,
    pub job_id: Option<Uuid>,
}
#[derive(Serialize)]
pub(super) struct PreviewResult {
    text: String,
    input_text: String,
    diagnostics: Diagnostics,
}
impl ModeService {
    pub(super) async fn preview(&self, request: PreviewRequest) -> Result<PreviewResult, ApiError> {
        let _permit = self.preview_slots.try_acquire().map_err(|_| {
            processing::invalid("A mode preview is already running. Try again shortly.")
        })?;
        processing::validate_profile(&request.profile)?;
        let context = processing::permitted_context(&request.profile, &request.context)?;
        let (raw_text, vocabulary): (String, Vocabulary) = if let Some(id) = request.job_id {
            let job = self.jobs.get(id).await?;
            let crate::transcription::JobStatus::Succeeded { result } = job.status else {
                return Err(processing::invalid("Choose a completed transcript."));
            };
            (result.transcript.raw_text, job.request.options.vocabulary)
        } else {
            (request.text, empty_vocabulary())
        };
        if raw_text.trim().is_empty() || raw_text.len() > 20_000 {
            return Err(processing::invalid(
                "Try a passage between 1 and 20,000 UTF-8 bytes.",
            ));
        }
        let attempt = ProcessingAttempt {
            request_digest: None,
            id: Uuid::nil(),
            job_id: Uuid::nil(),
            created_at_ms: 0,
            request: ProcessRequest {
                operation_id: Uuid::nil(),
                profile_id: request.profile.id.clone(),
                profile_revision: request.profile.revision,
                context,
            },
            profile: request.profile,
            input_text: String::new(),
            raw_text: raw_text.clone(),
            prompt_version: PROMPT_VERSION,
            state: AttemptState::Pending,
            diagnostics: Diagnostics::default(),
        };
        let (result, diagnostics) = diagnostics::measure(async {
            tokio::time::timeout(
                if attempt.profile.kind == ModeKind::Super {
                    processing::polish_timeout(&attempt.profile, self.processor.as_ref())
                } else {
                    std::time::Duration::from_secs(120)
                },
                processing::run(&attempt, &vocabulary, self.processor.as_ref()),
            )
            .await
            .unwrap_or_else(|_| {
                Err(processing::invalid(
                    "Mode preview timed out. Your settings were not changed.",
                ))
            })
        })
        .await;
        Ok(PreviewResult {
            text: result?,
            input_text: raw_text,
            diagnostics,
        })
    }
}
