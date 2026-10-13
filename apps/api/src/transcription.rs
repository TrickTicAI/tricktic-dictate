//! Version-one wire types. Generate schema and TypeScript with `pnpm contracts:generate`.
use axum::{http::StatusCode, response::IntoResponse, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub enum ApiVersion {
    #[serde(rename = "1")]
    V1,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "{T}Envelope")]
pub struct Envelope<T> {
    pub api_version: ApiVersion,
    pub data: T,
}

impl<T> Envelope<T> {
    pub fn new(data: T) -> Self {
        Self {
            api_version: ApiVersion::V1,
            data,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputLanguage {
    En,
    He,
    Mixed,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextLanguage {
    En,
    He,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputLanguage {
    Preserve {},
    Translate { language: TextLanguage },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Openai,
    Gemini,
    /// xAI. Its speech-to-text does not publish Hebrew, so the language catalog
    /// registers it for English only; its chat models are used for text processing.
    Grok,
    Local,
    /// OpenRouter. Text processing only: it relays chat models from several
    /// publishers, and Writer registers no speech model for it.
    Openrouter,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionLocation {
    Cloud,
    SelfHostedServer,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelSelection {
    pub provider: ProviderId,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Snippet {
    pub phrase: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vocabulary {
    pub hints: Vec<String>,
    pub corrections: Vec<Correction>,
    pub snippets: Vec<Snippet>,
}

/*
 * The saved dictionary, which is not the same object as the vocabulary a
 * transcription runs with. An entry can be switched off and kept: it stays in
 * place, in its list, and never reaches a provider, because everything that
 * transcribes is given `VocabularyDocument::active` instead of this document.
 * Keeping the two types apart is what makes that unmissable — a disabled rule
 * cannot leak into a request by someone forgetting a filter.
 *
 * `id` is the entry's stable portable identity (MW-OUATH-6). It is assigned
 * once, before an export or sync needs it, and never derived from position or
 * text. Editors keep it by carrying the entry object through their edits.
 */
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HintEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    pub text: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorrectionEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    pub from: String,
    pub to: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnippetEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    pub phrase: String,
    pub text: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VocabularyDocument {
    pub hints: Vec<HintEntry>,
    pub corrections: Vec<CorrectionEntry>,
    pub snippets: Vec<SnippetEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum CleanupPolicy {
    Off {},
    Enabled { selection: ModelSelection },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionOptions {
    pub selection: ModelSelection,
    pub input_language: InputLanguage,
    pub output_language: OutputLanguage,
    pub vocabulary: Vocabulary,
    pub cleanup: CleanupPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub ui_language: TextLanguage,
    pub transcription: TranscriptionOptions,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Capturing,
    Complete,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingMetadata {
    pub id: Uuid,
    /// Unix epoch milliseconds; safe JavaScript integer.
    #[schemars(range(max = 9007199254740991_u64))]
    pub created_at_ms: u64,
    /// Actual MediaRecorder/container MIME, including codecs when known.
    pub mime_type: String,
    #[schemars(range(max = 9007199254740991_u64))]
    pub byte_length: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub duration_ms: u64,
    pub input_language: InputLanguage,
    pub capture_state: CaptureState,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageLocation {
    Browser,
    SelfHostedServer,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioState {
    Uploading {
        #[schemars(range(max = 9007199254740991_u64))]
        received_bytes: u64,
    },
    Saved {},
    Deleted {},
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recording {
    pub metadata: RecordingMetadata,
    pub storage: StorageLocation,
    pub audio: AudioState,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionRequest {
    pub recording_id: Uuid,
    /// Client UUID reused for identical submissions only.
    pub idempotency_key: Uuid,
    pub options: TranscriptionOptions,
    /// Omit for the first attempt; retries retain the same recording ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of_job_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotImplemented,
    ProviderUnavailable,
    InvalidRequest,
    RecordingNotFound,
    AudioNotSaved,
    IdempotencyConflict,
    AuthenticationFailed,
    RateLimited,
    ProviderTimeout,
    ProviderFailed,
    InvalidProviderResponse,
    StorageFailed,
    UnsupportedCapability,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: ErrorCode,
    /// Safe user-facing summary; never a raw upstream body or credential.
    pub message: String,
    pub retryable: bool,
    /// Provider's minimum retry delay, when supplied. Never a raw response header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9007199254740991_u64))]
    pub retry_after_ms: Option<u64>,
}

impl ApiError {
    pub fn unavailable(feature: &str) -> Self {
        Self {
            code: ErrorCode::NotImplemented,
            message: format!("{feature} has not been implemented."),
            retryable: false,
            retry_after_ms: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    pub api_version: ApiVersion,
    pub error: ApiError,
}

pub async fn unavailable() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorEnvelope {
            api_version: ApiVersion::V1,
            error: ApiError::unavailable("This action"),
        }),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    /// Exact adapter result, retained even if postprocessing fails.
    pub raw_text: String,
    pub selection: ModelSelection,
    pub execution_location: ExecutionLocation,
    pub input_language: InputLanguage,
    pub output_language: OutputLanguage,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CleanupResult {
    Off {},
    Pending {},
    Succeeded {
        cleaned_text: String,
        selection: ModelSelection,
    },
    Failed {
        error: ApiError,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionResult {
    pub transcript: Transcript,
    pub cleanup: CleanupResult,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobStatus {
    Queued {},
    Transcribing {},
    Succeeded { result: TranscriptionResult },
    Failed { error: ApiError },
}

/// Durable recognition recovery. Capacity waits do not start or charge attempts.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecognitionRecovery {
    #[schemars(range(max = 3_u64))]
    pub attempts_started: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub next_retry_at_ms: Option<u64>,
    pub waiting_for_capacity: bool,
    pub canceled: bool,
    pub failures: Vec<RecognitionFailure>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecognitionFailure {
    #[schemars(range(max = 3_u64))]
    pub attempt: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub at_ms: u64,
    pub error: ApiError,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub id: Uuid,
    pub request: TranscriptionRequest,
    #[schemars(range(max = 9007199254740991_u64))]
    pub created_at_ms: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub updated_at_ms: u64,
    pub status: JobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecognitionRecovery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_preparation: Option<AudioPreparation>,
}

/// Measurements of disposable 16 kHz PCM, independent of original recording duration.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioPreparation {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub input_ms: u64,
    pub output_ms: u64,
    pub removed_ms: u64,
    pub preparation_ms: u64,
}

/// Schema catalog only; HTTP endpoints return individual envelopes.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContractCatalog {
    pub recording_request: Envelope<RecordingMetadata>,
    pub recording: Envelope<Recording>,
    pub transcription_request: Envelope<TranscriptionRequest>,
    pub job: Envelope<Job>,
    pub settings: Envelope<Settings>,
    pub vocabulary: Envelope<VocabularyDocument>,
    pub error: ErrorEnvelope,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_current() {
        let schema = schemars::generate::SchemaSettings::draft07()
            .into_generator()
            .into_root_schema_for::<ContractCatalog>();
        let rendered = format!("{}\n", serde_json::to_string_pretty(&schema).unwrap());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/contracts/schema.json");
        if std::env::var_os("WRITER_UPDATE_CONTRACTS").is_some() {
            std::fs::write(&path, &rendered).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            rendered,
            "Run pnpm contracts:generate"
        );
    }

    #[test]
    fn wire_examples_round_trip_without_losing_fields() {
        let examples: ContractCatalog =
            serde_json::from_str(include_str!("../../../packages/contracts/examples.json"))
                .unwrap();
        let original: serde_json::Value =
            serde_json::from_str(include_str!("../../../packages/contracts/examples.json"))
                .unwrap();
        assert_eq!(serde_json::to_value(examples).unwrap(), original);
        let retry: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/contracts/retry-examples.json"
        ))
        .unwrap();
        let job: Envelope<Job> = serde_json::from_value(retry["failed_job"].clone()).unwrap();
        let request: Envelope<TranscriptionRequest> =
            serde_json::from_value(retry["retry_request"].clone()).unwrap();
        assert_eq!(job.data.request.recording_id, request.data.recording_id);
        assert_eq!(request.data.retry_of_job_id, Some(job.data.id));
        assert_ne!(
            job.data.request.idempotency_key,
            request.data.idempotency_key
        );
    }

    #[test]
    fn failed_job_cannot_masquerade_as_success_or_accept_credentials() {
        assert!(serde_json::from_value::<JobStatus>(serde_json::json!({
            "state": "succeeded", "error": {"code": "provider_timeout"}
        }))
        .is_err());
        assert!(serde_json::from_value::<ModelSelection>(serde_json::json!({
            "provider": "openai", "model": "test", "api_key": "not-a-secret"
        }))
        .is_err());
        assert!(serde_json::from_value::<OutputLanguage>(serde_json::json!({
            "mode": "preserve", "language": "en"
        }))
        .is_err());
    }
}
