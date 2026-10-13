//! MW-06 owns job scheduling, durable status and restart recovery.
use crate::transcription::{ApiError, Job, JobStatus, TranscriptionRequest};
use async_trait::async_trait;
use axum::{
    routing::{delete, get, post},
    Router,
};
use uuid::Uuid;

#[async_trait]
pub trait JobStore: Send + Sync {
    /// Optional content-free timing projection; fixtures/in-memory stores need no disk metrics.
    fn latency_path(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// Atomic create-or-return by idempotency key. Reject different payloads
    /// under the same key, and verify saved source audio before enqueueing.
    async fn submit(&self, request: TranscriptionRequest) -> Result<Job, ApiError>;
    async fn get(&self, id: Uuid) -> Result<Job, ApiError>;
    /// Persist state without mutating the immutable request or source audio.
    async fn set_status(&self, id: Uuid, status: JobStatus) -> Result<Job, ApiError>;
    /// Queued/transcribing work for restart recovery; completed raw results stay saved.
    async fn recoverable(&self) -> Result<Vec<Job>, ApiError>;
}

pub struct UnavailableJobStore;

#[async_trait]
impl JobStore for UnavailableJobStore {
    async fn submit(&self, _: TranscriptionRequest) -> Result<Job, ApiError> {
        Err(ApiError::unavailable("Job storage"))
    }
    async fn get(&self, _: Uuid) -> Result<Job, ApiError> {
        Err(ApiError::unavailable("Job storage"))
    }
    async fn set_status(&self, _: Uuid, _: JobStatus) -> Result<Job, ApiError> {
        Err(ApiError::unavailable("Job storage"))
    }
    async fn recoverable(&self) -> Result<Vec<Job>, ApiError> {
        Err(ApiError::unavailable("Job storage"))
    }
}

pub mod batches;
pub mod disk;
pub mod runtime;
pub use disk::FileJobStore;
pub use runtime::{runtime, Pipeline};

pub(crate) fn response_error(error: ApiError) -> axum::response::Response {
    use crate::transcription::{ApiVersion, ErrorCode, ErrorEnvelope};
    use axum::{http::StatusCode, response::IntoResponse, Json};
    let status = match error.code {
        ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::RecordingNotFound => StatusCode::NOT_FOUND,
        ErrorCode::AudioNotSaved | ErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        ErrorCode::InvalidRequest | ErrorCode::Interrupted => StatusCode::BAD_REQUEST,
        ErrorCode::UnsupportedCapability => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let retryable = error.retryable;
    let retry_after_seconds = error.retry_after_ms.unwrap_or(30_000).saturating_add(999) / 1000;
    let mut response = (
        status,
        Json(ErrorEnvelope {
            api_version: ApiVersion::V1,
            error,
        }),
    )
        .into_response();
    if retryable {
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            retry_after_seconds.to_string().parse().unwrap(),
        );
    }
    response
}
pub(crate) fn invalid(message: &str) -> axum::response::Response {
    response_error(crate::recordings::disk::error(
        crate::transcription::ErrorCode::InvalidRequest,
        message,
        false,
    ))
}
async fn submit(
    axum::extract::State(injected): axum::extract::State<Option<std::sync::Arc<Pipeline>>>,
    payload: Result<
        axum::Json<crate::transcription::Envelope<TranscriptionRequest>>,
        axum::extract::rejection::JsonRejection,
    >,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Ok(axum::Json(payload)) = payload else {
        return invalid("Expected a version-one transcription request.");
    };
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.jobs.submit(payload.data).await {
        Ok(job) => {
            pipeline.wake_worker();
            (
                axum::http::StatusCode::ACCEPTED,
                axum::Json(crate::transcription::Envelope::new(job)),
            )
                .into_response()
        }
        Err(e) => response_error(e),
    }
}
async fn get_job(
    axum::extract::State(injected): axum::extract::State<Option<std::sync::Arc<Pipeline>>>,
    path: Result<axum::extract::Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Ok(axum::extract::Path(id)) = path else {
        return invalid("Job ID must be a UUID.");
    };
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.jobs.get(id).await {
        Ok(job) => axum::Json(crate::transcription::Envelope::new(job)).into_response(),
        Err(e) => response_error(e),
    }
}
async fn delete_history(
    axum::extract::State(injected): axum::extract::State<Option<std::sync::Arc<Pipeline>>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.delete_history().await {
        Ok((deleted_recordings, deleted_jobs)) => axum::Json(serde_json::json!({"deleted_recordings": deleted_recordings, "deleted_jobs": deleted_jobs})).into_response(),
        Err(e) => response_error(e),
    }
}
async fn cancel_retries(
    axum::extract::State(injected): axum::extract::State<Option<std::sync::Arc<Pipeline>>>,
    path: Result<axum::extract::Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Ok(axum::extract::Path(id)) = path else {
        return invalid("Job ID must be a UUID.");
    };
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.jobs.cancel_retries(id).await {
        Ok(job) => {
            pipeline.wake_worker();
            axum::Json(crate::transcription::Envelope::new(job)).into_response()
        }
        Err(e) => response_error(e),
    }
}
async fn latency_export(
    axum::extract::State(injected): axum::extract::State<Option<std::sync::Arc<Pipeline>>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match runtime::selected(injected).await {
        Ok(p) => axum::Json(crate::transcription::Envelope::new(
            crate::latency::measurements::read(&p.jobs.latency_path()),
        ))
        .into_response(),
        Err(e) => response_error(e),
    }
}
pub fn routes() -> Router {
    routes_with_pipeline(None).route("/api/v1/history", delete(delete_history).with_state(None))
}
pub(crate) fn routes_with_pipeline(pipeline: Option<std::sync::Arc<Pipeline>>) -> Router {
    Router::new()
        .route("/api/v1/statistics/latency", get(latency_export))
        .route("/api/v1/transcriptions", post(submit))
        .route("/api/v1/jobs/{id}", get(get_job))
        .route("/api/v1/jobs/{id}/cancel-retries", post(cancel_retries))
        .with_state(pipeline.clone())
        .merge(batches::routes(pipeline))
}

#[cfg(test)]
mod tests;

#[cfg(all(test, windows))]
mod windows_acceptance;
