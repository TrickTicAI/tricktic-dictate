//! Disposable local-only precomputation. Source capture and durable jobs are independent.
use crate::{
    jobs::{invalid, response_error, runtime},
    providers::SavedAudio,
    transcription::*,
};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, sync::Semaphore};
const LIMIT: usize = 64 * 1024 * 1024;
static PREVIEW_SLOT: Semaphore = Semaphore::const_new(1);
struct PreviewTask {
    id: uuid::Uuid,
    started: Instant,
    result: Option<Result<String, ApiError>>,
}
static TASK: Mutex<Option<PreviewTask>> = Mutex::new(None);
pub(crate) fn pause() -> Result<tokio::sync::SemaphorePermit<'static>, ApiError> {
    let guard = PREVIEW_SLOT.try_acquire().map_err(|_| {
        crate::recordings::disk::error(
            ErrorCode::IdempotencyConflict,
            "Wait for local preview inference to finish before deleting history.",
            true,
        )
    })?;
    *TASK.lock().unwrap() = None;
    Ok(guard)
}
async fn status(Path(id): Path<uuid::Uuid>) -> Response {
    let mut state = TASK.lock().unwrap();
    if state
        .as_ref()
        .is_some_and(|t| t.started.elapsed() > Duration::from_secs(600))
    {
        *state = None;
    }
    let Some(task) = state.as_ref().filter(|t| t.id == id) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    match &task.result {
        None => (
            axum::http::StatusCode::ACCEPTED,
            Json(serde_json::json!({"id":id})),
        )
            .into_response(),
        Some(Ok(text)) => Json(serde_json::json!({"text":text,"provisional":true})).into_response(),
        Some(Err(error)) => response_error(error.clone()),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    recording_id: uuid::Uuid,
    options: TranscriptionOptions,
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn preview(multipart: Multipart) -> Response {
    let pipeline = match runtime().await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    preview_on(pipeline, multipart).await
}
async fn preview_on(
    pipeline: std::sync::Arc<crate::jobs::Pipeline>,
    mut multipart: Multipart,
) -> Response {
    let Ok(_permit) = PREVIEW_SLOT.try_acquire() else {
        return axum::http::StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let request: Request = match multipart.next_field().await {
        Ok(Some(field)) if field.name() == Some("request") => match field.bytes().await {
            Ok(bytes) if bytes.len() <= 16 * 1024 => match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                Err(_) => return invalid("Invalid preview options."),
            },
            _ => return invalid("Preview options exceed 16 KiB."),
        },
        _ => return invalid("Expected preview request then audio."),
    };
    if !matches!(request.options.selection.provider, ProviderId::Local) {
        return invalid("Live precomputation supports local models only.");
    }
    // Do not queue speculative work ahead of any durable job.
    use crate::jobs::JobStore;
    let pending = match pipeline.jobs.recoverable().await {
        Ok(jobs) => jobs,
        Err(e) => return response_error(e),
    };
    if !pending.is_empty() {
        return axum::http::StatusCode::CONFLICT.into_response();
    }
    let Some(provider) = pipeline
        .providers
        .iter()
        .find(|p| matches!(p.info().id, ProviderId::Local))
    else {
        return invalid("Local provider unavailable.");
    };
    let temporary = Temporary(pipeline.preview_root.join(uuid::Uuid::new_v4().to_string()));
    if let Err(e) = crate::recordings::disk::private_dir(&temporary.0).await {
        return response_error(e);
    }
    let path = temporary.0.join("audio.wav");
    let mut file = match tokio::fs::File::create(&path).await {
        Ok(f) => f,
        Err(_) => return invalid("Preview storage unavailable."),
    };
    let mut length = 0usize;
    match multipart.next_field().await {
        Ok(Some(mut field)) if field.name() == Some("audio") => {
            loop {
                match field.chunk().await {
                Ok(Some(bytes)) if bytes.len() <= LIMIT.saturating_sub(length) => { length += bytes.len(); if file.write_all(&bytes).await.is_err() { return invalid("Preview storage unavailable."); } },
                Ok(None) => break,
                _ => return invalid("Preview audio is incomplete or exceeds 64 MiB. Final capture is unaffected."),
            }
            }
        }
        _ => return invalid("Expected preview audio."),
    }
    if file.flush().await.is_err() {
        return invalid("Preview storage unavailable.");
    }
    drop(file);
    let audio = SavedAudio {
        prepared: None,
        path,
        metadata: RecordingMetadata {
            id: request.recording_id,
            capture_state: CaptureState::Complete,
            created_at_ms: 0,
            duration_ms: 0,
            byte_length: length as u64,
            mime_type: "audio/wav".into(),
            input_language: request.options.input_language.clone(),
        },
    };
    let id = uuid::Uuid::new_v4();
    *TASK.lock().unwrap() = Some(PreviewTask {
        id,
        started: Instant::now(),
        result: None,
    });
    let provider = provider.clone();
    tokio::spawn(async move {
        let result = provider.preview(&audio, &request.options).await;
        if let Some(task) = TASK.lock().unwrap().as_mut().filter(|t| t.id == id) {
            task.result = Some(result);
        }
        drop(temporary);
        drop(_permit);
    });
    (
        axum::http::StatusCode::ACCEPTED,
        Json(serde_json::json!({"id":id})),
    )
        .into_response()
}
/// Native-only entry: a trusted committed prefix, never a caller-provided path.
pub async fn native(audio: SavedAudio, options: TranscriptionOptions) -> Result<String, ApiError> {
    let _permit = PREVIEW_SLOT
        .try_acquire()
        .map_err(|_| super::unavailable())?;
    let pipeline = runtime().await?;
    use crate::jobs::JobStore;
    if !pipeline.jobs.recoverable().await?.is_empty() {
        return Err(super::unavailable());
    }
    let provider = pipeline
        .providers
        .iter()
        .find(|p| matches!(p.info().id, ProviderId::Local))
        .ok_or_else(super::unavailable)?;
    provider.preview(&audio, &options).await
}

/// Native-only: run the final-quality pass on the complete prepared audio the
/// moment capture is durable, before import and job persistence. The durable job
/// that follows waits on the provider slot and then finds every window cached.
/// The result is not delivered from here; the job remains the authority.
pub async fn native_final(
    audio: SavedAudio,
    options: TranscriptionOptions,
) -> Result<(), ApiError> {
    let pipeline = runtime().await?;
    let provider = pipeline
        .providers
        .iter()
        .find(|p| matches!(p.info().id, ProviderId::Local))
        .ok_or_else(super::unavailable)?;
    crate::latency::mark(audio.metadata.id, "final_ahead_start");
    let result = provider.transcribe(&audio, &options).await;
    crate::latency::mark(
        audio.metadata.id,
        if result.is_ok() {
            "final_ahead_done"
        } else {
            "final_ahead_failed"
        },
    );
    result.map(drop)
}

pub fn routes() -> Router {
    Router::new()
        .route("/api/v1/transcriptions/preview", post(preview))
        .route("/api/v1/transcriptions/preview/{id}", get(status))
        .layer(DefaultBodyLimit::max(LIMIT + 32 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderInfo, TranscriptionProvider};
    use std::sync::Arc;
    use tower::ServiceExt;
    struct Slow {
        release: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl TranscriptionProvider for Slow {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                id: ProviderId::Local,
                execution_location: ExecutionLocation::SelfHostedServer,
                configured: true,
                available: true,
            }
        }
        async fn transcribe(
            &self,
            _: &SavedAudio,
            _: &TranscriptionOptions,
        ) -> Result<Transcript, ApiError> {
            unreachable!()
        }
        async fn preview(
            &self,
            audio: &SavedAudio,
            _: &TranscriptionOptions,
        ) -> Result<String, ApiError> {
            assert!(audio.path.exists());
            self.release.notified().await;
            Ok("provisional fixture".into())
        }
    }
    #[tokio::test]
    async fn submission_returns_before_inference_and_deletion_waits_for_disposable_snapshot() {
        let root =
            std::env::temp_dir().join(format!("writer-preview-test-{}", uuid::Uuid::new_v4()));
        let provider = Arc::new(Slow {
            release: tokio::sync::Notify::new(),
        });
        let pipeline = crate::jobs::Pipeline::open(root.clone(), 1024, vec![provider.clone()])
            .await
            .unwrap();
        let p = pipeline.clone();
        let router = Router::new()
            .route(
                "/preview",
                post(move |m: Multipart| preview_on(p.clone(), m)),
            )
            .route("/preview/{id}", get(status));
        let fixture: ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        let mut options = fixture.transcription_request.data.options;
        options.selection.provider = ProviderId::Local;
        let request = serde_json::json!({"recording_id":uuid::Uuid::new_v4(),"options":options});
        let body=format!("--test\r\nContent-Disposition: form-data; name=\"request\"\r\n\r\n{request}\r\n--test\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\nfixture\r\n--test--\r\n");
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            router.clone().oneshot(
                axum::http::Request::post("/preview")
                    .header("Content-Type", "multipart/form-data; boundary=test")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
        let data: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        let path = format!("/preview/{}", data["id"].as_str().unwrap());
        let pending = router
            .clone()
            .oneshot(
                axum::http::Request::get(&path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pending.status(), axum::http::StatusCode::ACCEPTED);
        assert!(matches!(
            pipeline.delete_history().await.unwrap_err().code,
            ErrorCode::IdempotencyConflict
        ));
        provider.release.notify_one();
        for _ in 0..100 {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::get(&path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if response.status() == axum::http::StatusCode::OK {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // The slot is held through disposable-file removal, so successful delete
        // cannot return with an active preview about to rewrite its cache.
        for _ in 0..100 {
            if pipeline.delete_history().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(tokio::fs::read_dir(&pipeline.preview_root)
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none());
        assert!(TASK.lock().unwrap().is_none());
        drop(router);
        drop(pipeline);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
