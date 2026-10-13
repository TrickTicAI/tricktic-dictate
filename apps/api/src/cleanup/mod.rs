//! Explicit, independent cleanup of an already durable raw transcript.
mod custom;
pub mod local_text;
pub mod modes;
mod provider;
mod text;
mod validation;

use crate::{
    jobs::{runtime, JobStore},
    transcription::*,
};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(45);

#[async_trait::async_trait]
trait TextProvider: Send + Sync {
    async fn clean(
        &self,
        selection: &ModelSelection,
        spans: &[String],
    ) -> Result<Vec<String>, ApiError>;
}

struct CleanupService {
    mobile_only: bool,
    jobs: Arc<dyn JobStore>,
    provider: Arc<dyn TextProvider>,
    active: Mutex<BTreeSet<Uuid>>,
    timeout: Duration,
}
impl CleanupService {
    async fn start(self: &Arc<Self>, id: Uuid, selection: ModelSelection) -> Result<Job, ApiError> {
        if self.mobile_only && matches!(selection.provider, ProviderId::Local) {
            return Err(failure(
                ErrorCode::UnsupportedCapability,
                "Mobile cleanup supports the advertised cloud text models only.",
                false,
            ));
        }
        provider::validate(&selection)?;
        let mut active = self.active.lock().await;
        let job = self.jobs.get(id).await?;
        if active.contains(&id) {
            return Ok(job);
        }
        let JobStatus::Succeeded { mut result } = job.status else {
            return Err(failure(
                ErrorCode::InvalidRequest,
                "Cleanup requires a successfully saved original transcript.",
                false,
            ));
        };
        if active.len() >= crate::limits::Limits::from_env()?.device_work_concurrency {
            return Err(crate::limits::busy(
                "Cleanup is busy; retry from the saved transcript.",
            ));
        }
        result.cleanup = CleanupResult::Pending {};
        let pending = self
            .jobs
            .set_status(
                id,
                JobStatus::Succeeded {
                    result: result.clone(),
                },
            )
            .await?;
        active.insert(id);
        let service = self.clone();
        // The task survives a closed tab/request. Raw text is already committed and readable.
        tokio::spawn(async move {
            let original = result.transcript.raw_text.clone();
            let vocabulary = job.request.options.vocabulary;
            let provider = service.provider.clone();
            let selected = selection.clone();
            let timeout = service.timeout;
            let outcome = tokio::spawn(async move {
                tokio::time::timeout(timeout, text::clean(&original, &vocabulary, &selected, provider.as_ref())).await
                    .unwrap_or_else(|_| Err(failure(ErrorCode::ProviderTimeout, "Cleanup timed out. The original transcript is available; retry cleanup when ready.", true)))
            }).await.unwrap_or_else(|_| Err(failure(ErrorCode::ProviderFailed, "Cleanup worker stopped. The original transcript is available; retry cleanup.", true)));
            result.cleanup = match outcome {
                Ok(cleaned_text) => CleanupResult::Succeeded {
                    cleaned_text,
                    selection,
                },
                Err(error) => CleanupResult::Failed { error },
            };
            // Keep the finished result in memory while transient persistence errors recover.
            // A process restart may leave pending state, which an explicit retry can replace.
            for attempt in 0..3 {
                if service
                    .jobs
                    .set_status(
                        id,
                        JobStatus::Succeeded {
                            result: result.clone(),
                        },
                    )
                    .await
                    .is_ok()
                {
                    break;
                }
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                } else {
                    tracing::error!("Cleanup result persistence failed; original transcript retained and explicit retry available");
                }
            }
            service.active.lock().await.remove(&id);
        });
        Ok(pending)
    }
}

static SERVICE: tokio::sync::OnceCell<Arc<CleanupService>> = tokio::sync::OnceCell::const_new();
async fn service() -> Result<Arc<CleanupService>, ApiError> {
    SERVICE
        .get_or_try_init(|| async {
            Ok(Arc::new(CleanupService {
                mobile_only: false,
                jobs: runtime().await?.jobs.clone(),
                provider: Arc::new(
                    provider::CloudText::from_env().with_budget(runtime().await?.work.clone()),
                ),
                active: Mutex::new(BTreeSet::new()),
                timeout: TIMEOUT,
            }))
        })
        .await
        .cloned()
}
async fn start(
    State(injected): State<Option<Arc<CleanupService>>>,
    path: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
    payload: Result<Json<Envelope<ModelSelection>>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return crate::jobs::invalid("Job ID must be a UUID.");
    };
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected a version-one cleanup model selection.");
    };
    let service = match injected {
        Some(service) => service,
        None => match service().await {
            Ok(service) => service,
            Err(error) => return crate::jobs::response_error(error),
        },
    };
    match service.start(id, payload.data).await {
        Ok(job) => (axum::http::StatusCode::ACCEPTED, Json(Envelope::new(job))).into_response(),
        Err(error) => crate::jobs::response_error(error),
    }
}
async fn processed(
    State(injected): State<Option<Arc<CleanupService>>>,
    path: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return crate::jobs::invalid("Job ID must be a UUID.");
    };
    let service = match injected {
        Some(service) => service,
        None => match service().await {
            Ok(service) => service,
            Err(error) => return crate::jobs::response_error(error),
        },
    };
    let job = match service.jobs.get(id).await {
        Ok(job) => job,
        Err(error) => return crate::jobs::response_error(error),
    };
    let JobStatus::Succeeded { result } = job.status else {
        return crate::jobs::invalid("Vocabulary processing requires a saved original transcript.");
    };
    match crate::vocabulary::process(&result.transcript.raw_text, &job.request.options.vocabulary) {
        Ok(processed) => {
            Json(Envelope::new(serde_json::json!({"text": processed.text()}))).into_response()
        }
        Err(error) => crate::jobs::response_error(error),
    }
}
async fn providers() -> impl IntoResponse {
    Json(Envelope::new(provider::discovery()))
}
fn router(service: Option<Arc<CleanupService>>) -> Router {
    Router::new()
        .route("/api/v1/jobs/{id}/cleanup", post(start))
        .route("/api/v1/cleanup/providers", get(providers))
        .route("/api/v1/jobs/{id}/processed", get(processed))
        .with_state(service)
}
pub(crate) fn mobile_routes(
    jobs: Arc<dyn JobStore>,
    work: crate::limits::work::WorkAccount,
) -> Router {
    router(Some(Arc::new(CleanupService {
        mobile_only: true,
        jobs,
        provider: Arc::new(provider::CloudText::from_env().with_budget(work)),
        active: Mutex::new(BTreeSet::new()),
        timeout: TIMEOUT,
    })))
}
pub fn routes() -> Router {
    router(None)
        .merge(modes::routes())
        .merge(local_text::routes())
        .merge(custom::router(custom::global()))
}
fn failure(code: ErrorCode, message: &str, retryable: bool) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
        retry_after_ms: None,
    }
}
#[cfg(test)]
mod tests;
