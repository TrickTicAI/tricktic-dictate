use super::RecordingStore;
use crate::{
    jobs::{invalid, response_error},
    transcription::{Envelope, RecordingMetadata},
};
use axum::{
    body::Body,
    extract::{multipart::MultipartRejection, rejection::PathRejection, Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub async fn upload(
    State(injected): State<Option<std::sync::Arc<crate::jobs::Pipeline>>>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Response {
    let Ok(mut multipart) = multipart else {
        return invalid("Expected multipart metadata followed by an audio file.");
    };
    let pipeline = match crate::jobs::runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    let mut metadata = Vec::new();
    match multipart.next_field().await {
        Ok(Some(mut field)) if field.name() == Some("metadata") => loop {
            match field.chunk().await {
                Ok(Some(chunk)) if metadata.len() + chunk.len() <= 16 * 1024 => {
                    metadata.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                Err(_) => return response_error(crate::limits::idle()),
                _ => return invalid("Metadata is malformed or exceeds 16 KiB."),
            }
        },
        Err(_) => return response_error(crate::limits::idle()),
        _ => return invalid("The first multipart field must be metadata."),
    }
    let metadata: Envelope<RecordingMetadata> = match serde_json::from_slice(&metadata) {
        Ok(m) => m,
        Err(_) => return invalid("Expected version-one recording metadata."),
    };
    if metadata.data.byte_length > pipeline.recordings.max_upload_bytes {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(crate::transcription::ErrorEnvelope { api_version: crate::transcription::ApiVersion::V1, error: super::disk::error(crate::transcription::ErrorCode::InvalidRequest, "Recording exceeds WRITER_MAX_UPLOAD_BYTES; raise the server storage limit or upload a smaller file.", false) })).into_response();
    }
    if let Err(e) = pipeline.recordings.validate_metadata(&metadata.data) {
        return response_error(e);
    }
    let (reader, mut writer) = tokio::io::duplex(64 * 1024);
    let save = pipeline.recordings.save(metadata.data, Box::new(reader));
    let forward = async {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) if field.name() == Some("audio") => field,
            _ => return Err(crate::limits::idle()),
        };
        loop {
            let chunk = tokio::time::timeout(pipeline.recordings.budget.limits.idle, field.chunk())
                .await
                .map_err(|_| crate::limits::idle())?
                .map_err(|_| crate::limits::idle())?;
            let Some(chunk) = chunk else {
                break;
            };
            writer
                .write_all(&chunk)
                .await
                .map_err(super::disk::storage)?;
        }
        drop(field);
        // Check the terminator before EOF allows the store to commit.
        match tokio::time::timeout(
            pipeline.recordings.budget.limits.idle,
            multipart.next_field(),
        )
        .await
        {
            Ok(Ok(None)) => {}
            _ => return Err(crate::limits::idle()),
        }
        writer.shutdown().await.map_err(super::disk::storage)
    };
    // Cancel the forwarding future promptly when the store rejects admission or
    // declared bytes; join! would keep waiting for a malicious client to finish.
    match tokio::try_join!(save, forward) {
        Ok((recording, ())) => {
            (StatusCode::CREATED, Json(Envelope::new(recording))).into_response()
        }
        Err(e) => response_error(e),
    }
}

pub async fn get(
    State(injected): State<Option<std::sync::Arc<crate::jobs::Pipeline>>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return invalid("Recording ID must be a UUID.");
    };
    let pipeline = match crate::jobs::runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.recordings.get(id).await {
        Ok(r) => Json(Envelope::new(r)).into_response(),
        Err(e) => response_error(e),
    }
}
pub async fn audio(
    State(injected): State<Option<std::sync::Arc<crate::jobs::Pipeline>>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return invalid("Recording ID must be a UUID.");
    };
    let pipeline = match crate::jobs::runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    let audio = match pipeline.recordings.audio(id).await {
        Ok(a) => a,
        Err(e) => return response_error(e),
    };
    match tokio::fs::File::open(&audio.path).await {
        Ok(file) => (
            [
                (header::CONTENT_TYPE, audio.metadata.mime_type),
                (
                    header::CONTENT_LENGTH,
                    audio.metadata.byte_length.to_string(),
                ),
            ],
            Body::from_stream(tokio_util::io::ReaderStream::new(file)),
        )
            .into_response(),
        Err(e) => response_error(super::disk::storage(e)),
    }
}
pub async fn delete(
    State(injected): State<Option<std::sync::Arc<crate::jobs::Pipeline>>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return invalid("Recording ID must be a UUID.");
    };
    let pipeline = match crate::jobs::runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.jobs.delete_recording(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => response_error(e),
    }
}
