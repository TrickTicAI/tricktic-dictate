//! MW-121: a corrected reference text per recording, saved beside (never over)
//! the raw transcript. References feed the private personal scorecard; a
//! drafted reference may wait for the user's check before it counts as gold.
use super::{disk::now, response_error, runtime, JobStore, Pipeline};
use crate::recordings::disk::{error, private_dir, read_json, storage, write_json};
use crate::transcription::{ApiError, Envelope, ErrorCode, JobStatus};
use axum::{
    extract::{rejection::JsonRejection, rejection::PathRejection, Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::fs;
use uuid::Uuid;

/// Bounded so one reference stays a dictation, not a document dump.
const MAX_TEXT_BYTES: usize = 200_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSource {
    /// Typed or confirmed by the user in History.
    #[default]
    User,
    /// Drafted by agents from several recognitions of the same audio.
    Draft,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub recording_id: Uuid,
    /// The job whose raw transcript this corrects; that transcript is unchanged.
    pub job_id: Uuid,
    pub text: String,
    /// Counted in the personal gold set.
    pub gold: bool,
    #[serde(default)]
    pub source: ReferenceSource,
    /// Drafts disagreed: queued for the user to correct.
    #[serde(default)]
    pub needs_check: bool,
    /// The other draft, shown while the reference waits for a check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternative: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct ReferenceInput {
    pub job_id: Uuid,
    pub text: String,
    pub gold: bool,
}

/// One file per recording under the data directory, written atomically, so a
/// reference survives restart and an external drafting tool can add files.
pub struct ReferenceStore {
    root: PathBuf,
}

impl ReferenceStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    fn path(&self, recording: Uuid) -> PathBuf {
        self.root.join(format!("{recording}.json"))
    }
    pub async fn get(&self, recording: Uuid) -> Result<Option<Reference>, ApiError> {
        let path = self.path(recording);
        if !fs::try_exists(&path).await.map_err(storage)? {
            return Ok(None);
        }
        read_json(&path).await.map(Some)
    }
    pub async fn all(&self) -> Result<Vec<Reference>, ApiError> {
        if !fs::try_exists(&self.root).await.map_err(storage)? {
            return Ok(vec![]);
        }
        let mut entries = fs::read_dir(&self.root).await.map_err(storage)?;
        let mut references = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(storage)? {
            let path = entry.path();
            let named = path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| Uuid::parse_str(s).is_ok());
            if named && path.extension().is_some_and(|x| x == "json") {
                // A malformed file is skipped rather than hiding every other reference.
                if let Ok(reference) = read_json::<Reference>(&path).await {
                    references.push(reference);
                }
            }
        }
        references.sort_by_key(|r| (r.updated_at_ms, r.recording_id));
        Ok(references)
    }
    pub async fn save(&self, reference: &Reference) -> Result<(), ApiError> {
        private_dir(&self.root).await?;
        write_json(&self.path(reference.recording_id), reference).await
    }
    pub async fn delete(&self, recording: Uuid) -> Result<bool, ApiError> {
        match fs::remove_file(self.path(recording)).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(storage(e)),
        }
    }
    pub async fn delete_selected(&self, recordings: &[Uuid]) -> Result<(), ApiError> {
        for id in recordings {
            self.delete(*id).await?;
        }
        Ok(())
    }
    pub async fn delete_all(&self) -> Result<(), ApiError> {
        match fs::remove_dir_all(&self.root).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(storage(e)),
        }
    }
}

fn invalid(message: &str) -> ApiError {
    error(ErrorCode::InvalidRequest, message, false)
}

impl Pipeline {
    /// Saves the user's correction. The job must be a succeeded transcription of
    /// this recording; its raw text is only read, never rewritten.
    pub async fn correct(
        &self,
        recording: Uuid,
        input: ReferenceInput,
    ) -> Result<Reference, ApiError> {
        let text = input.text.trim();
        if text.len() > MAX_TEXT_BYTES {
            return Err(invalid("The corrected text is too long."));
        }
        let job = self.jobs.get(input.job_id).await?;
        if job.request.recording_id != recording {
            return Err(invalid("That transcript belongs to another recording."));
        }
        if !matches!(job.status, JobStatus::Succeeded { .. }) {
            return Err(invalid("Only a finished transcript can be corrected."));
        }
        let reference = Reference {
            recording_id: recording,
            job_id: job.id,
            text: text.to_owned(),
            gold: input.gold,
            source: ReferenceSource::User,
            needs_check: false,
            alternative: None,
            updated_at_ms: now(),
        };
        self.references.save(&reference).await?;
        Ok(reference)
    }
}

async fn list(State(injected): State<Option<Arc<Pipeline>>>) -> Response {
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.references.all().await {
        Ok(all) => Json(Envelope::new(all)).into_response(),
        Err(e) => response_error(e),
    }
}
async fn read(
    State(injected): State<Option<Arc<Pipeline>>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return super::invalid("Recording ID must be a UUID.");
    };
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.references.get(id).await {
        Ok(reference) => Json(Envelope::new(reference)).into_response(),
        Err(e) => response_error(e),
    }
}
async fn save(
    State(injected): State<Option<Arc<Pipeline>>>,
    path: Result<Path<Uuid>, PathRejection>,
    payload: Result<Json<Envelope<ReferenceInput>>, JsonRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return super::invalid("Recording ID must be a UUID.");
    };
    let Ok(Json(payload)) = payload else {
        return super::invalid("Expected a version-one reference.");
    };
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.correct(id, payload.data).await {
        Ok(reference) => Json(Envelope::new(reference)).into_response(),
        Err(e) => response_error(e),
    }
}
async fn remove(
    State(injected): State<Option<Arc<Pipeline>>>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return super::invalid("Recording ID must be a UUID.");
    };
    let pipeline = match runtime::selected(injected).await {
        Ok(p) => p,
        Err(e) => return response_error(e),
    };
    match pipeline.references.delete(id).await {
        Ok(_) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => response_error(e),
    }
}

pub(crate) fn routes(pipeline: Option<Arc<Pipeline>>) -> Router {
    Router::new()
        .route("/api/v1/references", get(list))
        .route(
            "/api/v1/recordings/{id}/reference",
            get(read).put(save).delete(remove),
        )
        .with_state(pipeline)
}

#[cfg(test)]
mod tests;
