use super::*;
use crate::transcription::Envelope;
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;

pub async fn service() -> Result<Arc<ModeService>, ApiError> {
    crate::jobs::runtime().await?.mode_service().await
}
async fn resolve(injected: Option<Arc<ModeService>>) -> Result<Arc<ModeService>, ApiError> {
    match injected {
        Some(s) => Ok(s),
        None => service().await,
    }
}
fn response<T: Serialize>(result: Result<T, ApiError>) -> Response {
    match result {
        Ok(data) => Json(Envelope::new(data)).into_response(),
        Err(error) => crate::jobs::response_error(error),
    }
}
async fn settings(State(injected): State<Option<Arc<ModeService>>>) -> Response {
    response(match resolve(injected).await {
        Ok(s) => s.settings().await,
        Err(e) => Err(e),
    })
}
async fn save(
    State(injected): State<Option<Arc<ModeService>>>,
    payload: Result<Json<Envelope<ModeSettings>>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected version-one mode settings.");
    };
    response(match resolve(injected).await {
        Ok(s) => s.save(payload.data).await,
        Err(e) => Err(e),
    })
}
async fn attempts(
    State(injected): State<Option<Arc<ModeService>>>,
    Path(id): Path<Uuid>,
) -> Response {
    response(match resolve(injected).await {
        Ok(s) => s.attempts(id).await,
        Err(e) => Err(e),
    })
}
async fn live(State(injected): State<Option<Arc<ModeService>>>, Path(id): Path<Uuid>) -> Response {
    response(match resolve(injected).await {
        Ok(s) => s.live_processing(id).await,
        Err(e) => Err(e),
    })
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserCapture {
    profile: ModeProfile,
}
async fn bind(
    State(injected): State<Option<Arc<ModeService>>>,
    Path(id): Path<Uuid>,
    payload: Result<Json<Envelope<BrowserCapture>>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected a version-one recording mode snapshot.");
    };
    response(match resolve(injected).await {
        Ok(s) => {
            s.bind_recording(id, payload.data.profile, ContextSnapshot::default())
                .await
        }
        Err(e) => Err(e),
    })
}
async fn start(
    State(injected): State<Option<Arc<ModeService>>>,
    Path(id): Path<Uuid>,
    payload: Result<Json<Envelope<ProcessRequest>>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected a version-one mode processing request.");
    };
    match resolve(injected).await {
        Ok(s) => match s.start(id, payload.data).await {
            Ok(attempt) => (
                axum::http::StatusCode::ACCEPTED,
                Json(Envelope::new(attempt)),
            )
                .into_response(),
            Err(e) => crate::jobs::response_error(e),
        },
        Err(e) => crate::jobs::response_error(e),
    }
}
async fn preview(
    State(injected): State<Option<Arc<ModeService>>>,
    payload: Result<
        Json<Envelope<super::preview::PreviewRequest>>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected a version-one mode preview.");
    };
    response(match resolve(injected).await {
        Ok(s) => s.preview(payload.data).await,
        Err(e) => Err(e),
    })
}
async fn defaults() -> impl IntoResponse {
    Json(Envelope::new(ModeSettings::default()))
}
async fn providers() -> impl IntoResponse {
    Json(Envelope::new(crate::cleanup::provider::mode_discovery()))
}
pub(super) fn router(service: Option<Arc<ModeService>>) -> Router {
    Router::new()
        .route("/api/v1/modes", get(settings).put(save))
        .route("/api/v1/modes/providers", get(providers))
        .route("/api/v1/modes/defaults", get(defaults))
        .route("/api/v1/modes/preview", post(preview))
        .route("/api/v1/recordings/{id}/mode", post(bind))
        .route("/api/v1/jobs/{id}/processing", get(attempts).post(start))
        .route("/api/v1/jobs/{id}/live-processing", get(live))
        .route("/api/v1/modes/export", get(settings))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(service)
}
pub fn routes() -> Router {
    router(None)
}
