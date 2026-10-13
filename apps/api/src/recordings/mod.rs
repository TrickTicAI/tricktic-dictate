//! MW-06 implements atomic, bounded audio ingestion and persistence here.
use crate::{
    providers::SavedAudio,
    transcription::{ApiError, Recording, RecordingMetadata},
};
use async_trait::async_trait;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tokio::io::AsyncRead;
use uuid::Uuid;

pub type AudioReader = Box<dyn AsyncRead + Send + Unpin>;

#[async_trait]
pub trait RecordingStore: Send + Sync {
    /// Stream into temporary storage, then atomically publish metadata + audio.
    /// Reusing an ID with different content must fail, never overwrite audio.
    async fn save(
        &self,
        metadata: RecordingMetadata,
        audio: AudioReader,
    ) -> Result<Recording, ApiError>;
    async fn get(&self, id: Uuid) -> Result<Recording, ApiError>;
    /// Resolve an opaque ID; callers cannot supply a filesystem path.
    async fn audio(&self, id: Uuid) -> Result<SavedAudio, ApiError>;
    /// Explicit deletion only; jobs must never call this on inference failure.
    async fn delete(&self, id: Uuid) -> Result<(), ApiError>;
}

pub struct UnavailableRecordingStore;

#[async_trait]
impl RecordingStore for UnavailableRecordingStore {
    async fn save(&self, _: RecordingMetadata, _: AudioReader) -> Result<Recording, ApiError> {
        Err(ApiError::unavailable("Recording storage"))
    }
    async fn get(&self, _: Uuid) -> Result<Recording, ApiError> {
        Err(ApiError::unavailable("Recording storage"))
    }
    async fn audio(&self, _: Uuid) -> Result<SavedAudio, ApiError> {
        Err(ApiError::unavailable("Recording storage"))
    }
    async fn delete(&self, _: Uuid) -> Result<(), ApiError> {
        Err(ApiError::unavailable("Recording storage"))
    }
}

pub mod disk;
pub use disk::FileRecordingStore;
mod http;
pub(crate) mod ingest;
mod resume;

/// Resolve the same initialized store used by the durable jobs runtime.
pub async fn store() -> Result<Arc<dyn RecordingStore>, ApiError> {
    Ok(crate::jobs::runtime().await?.recordings.clone())
}

pub fn routes() -> Router {
    routes_with_pipeline(None)
}
pub(crate) fn routes_with_pipeline(pipeline: Option<Arc<crate::jobs::Pipeline>>) -> Router {
    let admission = match pipeline.as_ref() {
        Some(p) => crate::limits::http::Admission::new(
            &p.recordings.budget.limits,
            p.recordings.max_upload_bytes,
            true,
        ),
        None => match crate::limits::http::Admission::environment() {
            Ok(a) => a,
            Err(e) => {
                return Router::new().fallback(move || {
                    let e = e.clone();
                    async move { crate::jobs::response_error(e) }
                })
            }
        },
    };
    Router::new()
        .route("/api/v1/recordings", post(http::upload))
        .route(
            "/api/v1/recordings/{id}/upload",
            post(resume::start).patch(resume::chunk),
        )
        .route(
            "/api/v1/recordings/{id}/upload/finish",
            post(resume::finish),
        )
        .route(
            "/api/v1/recordings/{id}",
            get(http::get).delete(http::delete),
        )
        .route("/api/v1/recordings/{id}/audio", get(http::audio))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(axum::middleware::from_fn_with_state(
            admission,
            crate::limits::http::admit,
        ))
        .with_state(pipeline)
}
