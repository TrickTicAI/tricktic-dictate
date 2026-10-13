use super::{empty_document, validate_document};
use crate::transcription::{
    ApiError, ApiVersion, Envelope, ErrorCode, ErrorEnvelope, Vocabulary, VocabularyDocument,
};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};
use tokio::{fs, sync::Mutex};

pub(super) const MAX_BYTES: usize = 256 * 1024;

/// One shared store per API process. PUT replaces the document. A PUT that
/// names the version it was edited from (`If-Match`) is refused when the stored
/// document has changed since, e.g. because sync applied another device's edit
/// while this one was being typed; without `If-Match` the last writer wins.
pub struct VocabularyStore {
    path: PathBuf,
    write_lock: Mutex<()>,
}

fn storage_error() -> ApiError {
    ApiError { code: ErrorCode::StorageFailed, message: "Vocabulary storage is unavailable; previously saved settings were not intentionally removed.".into(), retryable: true, retry_after_ms: None }
}

impl VocabularyStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Mutex::new(()),
        }
    }

    pub async fn load(&self) -> Result<VocabularyDocument, ApiError> {
        let _guard = self.write_lock.lock().await;
        let value = self.read_document().await?;
        // Migrate before an editor or sync sees this copy. Identity assignment
        // must not silently invalidate an ETag returned a moment earlier.
        let mut identified = value.clone();
        if identified.assign_ids() {
            // An old file close to the byte limit must remain readable so the
            // user can remove entries. Never write an oversized migration.
            if validate_document(&identified).is_err() {
                return Ok(value);
            }
            self.persist(&identified).await?;
        }
        Ok(identified)
    }

    async fn read_document(&self) -> Result<VocabularyDocument, ApiError> {
        let bytes = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(empty_document()),
            Err(_) => return Err(storage_error()),
        };
        if bytes.len() > MAX_BYTES {
            return Err(storage_error());
        }
        let value = parse(&bytes).ok_or_else(storage_error)?;
        validate_document(&value).map_err(|_| storage_error())?;
        Ok(value)
    }

    /// The stored document and its version tag.
    pub async fn load_tagged(&self) -> Result<(VocabularyDocument, String), ApiError> {
        let value = self.load().await?;
        let tag = tag(&value);
        Ok((value, tag))
    }

    pub async fn save(&self, value: &VocabularyDocument) -> Result<(), ApiError> {
        // Without an expected version the save is unconditional.
        let _ = self.save_if(value, None).await?;
        Ok(())
    }

    /// Saves only if the stored document is still `expected` (when given), under
    /// the same lock every writer, including sync, takes. `Ok(Err(()))` means it
    /// changed and nothing was written. Returns the persisted document and tag.
    pub async fn save_if(
        &self,
        value: &VocabularyDocument,
        expected: Option<&str>,
    ) -> Result<Result<(VocabularyDocument, String), ()>, ApiError> {
        validate_document(value)?;
        let _guard = self.write_lock.lock().await;
        if let Some(expected) = expected {
            if tag(&self.read_document().await?) != expected {
                return Ok(Err(()));
            }
        }
        let mut saved = value.clone();
        saved.assign_ids();
        validate_document(&saved)?;
        self.persist(&saved).await?;
        let tag = tag(&saved);
        Ok(Ok((saved, tag)))
    }

    async fn persist(&self, value: &VocabularyDocument) -> Result<(), ApiError> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        crate::recordings::disk::private_dir(parent).await?;
        crate::recordings::disk::write_json(&self.path, value).await?;
        crate::account_sync::note_local_change();
        Ok(())
    }
}

/// A strong entity tag over the document as parsed, so a legacy file and its
/// rewritten form are the same version when they hold the same entries.
fn tag(value: &VocabularyDocument) -> String {
    let digest = Sha256::digest(serde_json::to_vec(value).unwrap_or_default());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"v1-{hex}\"")
}

fn with_tag(tag: String, body: Json<Envelope<VocabularyDocument>>) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&tag) {
        headers.insert(header::ETAG, value);
    }
    (headers, body)
}

/// A dictionary saved before entries could be switched off holds plain strings
/// and rules with no switch. Everything in such a document was in force, so it
/// is read rather than refused, and the next save rewrites it in the new shape.
fn parse(bytes: &[u8]) -> Option<VocabularyDocument> {
    serde_json::from_slice::<VocabularyDocument>(bytes)
        .ok()
        .or_else(|| {
            serde_json::from_slice::<Vocabulary>(bytes)
                .ok()
                .map(VocabularyDocument::from_vocabulary)
        })
}

type Failure = (StatusCode, Json<ErrorEnvelope>);
fn failure(status: StatusCode, error: ApiError) -> Failure {
    (
        status,
        Json(ErrorEnvelope {
            api_version: ApiVersion::V1,
            error,
        }),
    )
}
async fn read(State(store): State<Arc<VocabularyStore>>) -> Result<impl IntoResponse, Failure> {
    store
        .load_tagged()
        .await
        .map(|(v, tag)| with_tag(tag, Json(Envelope::new(v))))
        .map_err(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e))
}
async fn write(
    State(store): State<Arc<VocabularyStore>>,
    headers: HeaderMap,
    payload: Result<Json<Envelope<VocabularyDocument>>, JsonRejection>,
) -> Result<impl IntoResponse, Failure> {
    let expected = match headers.get(header::IF_MATCH) {
        None => None,
        Some(value) => Some(value.to_str().map_err(|_| {
            failure(
                StatusCode::BAD_REQUEST,
                ApiError {
                    code: ErrorCode::InvalidRequest,
                    message: "Invalid If-Match header.".into(),
                    retryable: false,
                    retry_after_ms: None,
                },
            )
        })?),
    };
    let Json(payload) = payload.map_err(|e| {
        failure(
            if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            },
            ApiError {
                code: ErrorCode::InvalidRequest,
                message: "Expected a v1 vocabulary JSON envelope within the 256 KiB request limit."
                    .into(),
                retryable: false,
                retry_after_ms: None,
            },
        )
    })?;
    validate_document(&payload.data).map_err(|e| failure(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let (saved, tag) = store
        .save_if(&payload.data, expected)
        .await
        .map_err(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(|()| {
            failure(
                StatusCode::PRECONDITION_FAILED,
                ApiError {
                    code: ErrorCode::IdempotencyConflict,
                    message: "The dictionary changed since it was loaded; nothing was saved."
                        .into(),
                    retryable: false,
                    retry_after_ms: None,
                },
            )
        })?;
    Ok(with_tag(tag, Json(Envelope::new(saved))))
}

pub fn routes_with_store(store: Arc<VocabularyStore>) -> Router {
    Router::new()
        .route("/api/v1/vocabulary", get(read).put(write))
        .layer(DefaultBodyLimit::max(MAX_BYTES))
        .with_state(store)
}
/// The process's store for the configured dictionary path, shared by the
/// routes and by portable import/sync so their writes are serialized.
pub fn global_store() -> Arc<VocabularyStore> {
    static STORE: std::sync::OnceLock<Arc<VocabularyStore>> = std::sync::OnceLock::new();
    STORE
        .get_or_init(|| Arc::new(VocabularyStore::new(configured_path())))
        .clone()
}

pub fn routes() -> Router {
    routes_with_store(global_store())
}

fn configured_path() -> PathBuf {
    std::env::var_os("WRITER_VOCABULARY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if let Some(root) = std::env::var_os("WRITER_DATA_DIR") {
                return PathBuf::from(root).join("vocabulary.json");
            }
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
                .unwrap_or_else(|| PathBuf::from(".data"));
            base.join("metrix-writer/vocabulary.json")
        })
}
