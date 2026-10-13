/* Generated from Rust via schema.json. Run pnpm contracts:generate. */

/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "ApiVersion".
 */
export type ApiVersion = "1";
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "ErrorCode".
 */
export type ErrorCode =
  | "not_implemented"
  | "provider_unavailable"
  | "invalid_request"
  | "recording_not_found"
  | "audio_not_saved"
  | "idempotency_conflict"
  | "authentication_failed"
  | "rate_limited"
  | "provider_timeout"
  | "provider_failed"
  | "invalid_provider_response"
  | "storage_failed"
  | "unsupported_capability"
  | "interrupted";
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "CleanupPolicy".
 */
export type CleanupPolicy =
  | {
      mode: "off";
    }
  | {
      mode: "enabled";
      selection: ModelSelection;
    };
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "ProviderId".
 */
export type ProviderId = ("openai" | "gemini" | "local") | "grok" | "openrouter";
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "InputLanguage".
 */
export type InputLanguage = "en" | "he" | "mixed";
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "OutputLanguage".
 */
export type OutputLanguage =
  | {
      mode: "preserve";
    }
  | {
      language: TextLanguage;
      mode: "translate";
    };
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "TextLanguage".
 */
export type TextLanguage = "en" | "he";
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "JobStatus".
 */
export type JobStatus =
  | {
      state: "queued";
    }
  | {
      state: "transcribing";
    }
  | {
      result: TranscriptionResult;
      state: "succeeded";
    }
  | {
      error: ApiError;
      state: "failed";
    };
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "CleanupResult".
 */
export type CleanupResult =
  | {
      state: "off";
    }
  | {
      state: "pending";
    }
  | {
      cleaned_text: string;
      selection: ModelSelection;
      state: "succeeded";
    }
  | {
      error: ApiError;
      state: "failed";
    };
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "ExecutionLocation".
 */
export type ExecutionLocation = "cloud" | "self_hosted_server";
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "AudioState".
 */
export type AudioState =
  | {
      received_bytes: number;
      state: "uploading";
    }
  | {
      state: "saved";
    }
  | {
      state: "deleted";
    };
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "CaptureState".
 */
export type CaptureState = "capturing" | "complete" | "interrupted";
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "StorageLocation".
 */
export type StorageLocation = "browser" | "self_hosted_server";

/**
 * Schema catalog only; HTTP endpoints return individual envelopes.
 */
export interface ContractCatalog {
  error: ErrorEnvelope;
  job: JobEnvelope;
  recording: RecordingEnvelope;
  recording_request: RecordingMetadataEnvelope;
  settings: SettingsEnvelope;
  transcription_request: TranscriptionRequestEnvelope;
  vocabulary: VocabularyDocumentEnvelope;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "ErrorEnvelope".
 */
export interface ErrorEnvelope {
  api_version: ApiVersion;
  error: ApiError;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "ApiError".
 */
export interface ApiError {
  code: ErrorCode;
  /**
   * Safe user-facing summary; never a raw upstream body or credential.
   */
  message: string;
  /**
   * Provider's minimum retry delay, when supplied. Never a raw response header.
   */
  retry_after_ms?: number | null;
  retryable: boolean;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "JobEnvelope".
 */
export interface JobEnvelope {
  api_version: ApiVersion;
  data: Job;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "Job".
 */
export interface Job {
  audio_preparation?: AudioPreparation | null;
  created_at_ms: number;
  id: string;
  recovery?: RecognitionRecovery | null;
  request: TranscriptionRequest;
  status: JobStatus;
  updated_at_ms: number;
}
/**
 * Measurements of disposable 16 kHz PCM, independent of original recording duration.
 *
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "AudioPreparation".
 */
export interface AudioPreparation {
  input_bytes: number;
  input_ms: number;
  output_bytes: number;
  output_ms: number;
  preparation_ms: number;
  removed_ms: number;
}
/**
 * Durable recognition recovery. Capacity waits do not start or charge attempts.
 *
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "RecognitionRecovery".
 */
export interface RecognitionRecovery {
  attempts_started: number;
  canceled: boolean;
  failures: RecognitionFailure[];
  next_retry_at_ms?: number | null;
  waiting_for_capacity: boolean;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "RecognitionFailure".
 */
export interface RecognitionFailure {
  at_ms: number;
  attempt: number;
  error: ApiError;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "TranscriptionRequest".
 */
export interface TranscriptionRequest {
  /**
   * Client UUID reused for identical submissions only.
   */
  idempotency_key: string;
  options: TranscriptionOptions;
  recording_id: string;
  /**
   * Omit for the first attempt; retries retain the same recording ID.
   */
  retry_of_job_id?: string | null;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "TranscriptionOptions".
 */
export interface TranscriptionOptions {
  cleanup: CleanupPolicy;
  input_language: InputLanguage;
  output_language: OutputLanguage;
  selection: ModelSelection;
  vocabulary: Vocabulary;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "ModelSelection".
 */
export interface ModelSelection {
  model: string;
  provider: ProviderId;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "Vocabulary".
 */
export interface Vocabulary {
  corrections: Correction[];
  hints: string[];
  snippets: Snippet[];
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "Correction".
 */
export interface Correction {
  from: string;
  to: string;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "Snippet".
 */
export interface Snippet {
  phrase: string;
  text: string;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "TranscriptionResult".
 */
export interface TranscriptionResult {
  cleanup: CleanupResult;
  transcript: Transcript;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "Transcript".
 */
export interface Transcript {
  execution_location: ExecutionLocation;
  input_language: InputLanguage;
  output_language: OutputLanguage;
  /**
   * Exact adapter result, retained even if postprocessing fails.
   */
  raw_text: string;
  selection: ModelSelection;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "RecordingEnvelope".
 */
export interface RecordingEnvelope {
  api_version: ApiVersion;
  data: Recording;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "Recording".
 */
export interface Recording {
  audio: AudioState;
  metadata: RecordingMetadata;
  storage: StorageLocation;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "RecordingMetadata".
 */
export interface RecordingMetadata {
  byte_length: number;
  capture_state: CaptureState;
  /**
   * Unix epoch milliseconds; safe JavaScript integer.
   */
  created_at_ms: number;
  duration_ms: number;
  id: string;
  input_language: InputLanguage;
  /**
   * Actual MediaRecorder/container MIME, including codecs when known.
   */
  mime_type: string;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "RecordingMetadataEnvelope".
 */
export interface RecordingMetadataEnvelope {
  api_version: ApiVersion;
  data: RecordingMetadata;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "SettingsEnvelope".
 */
export interface SettingsEnvelope {
  api_version: ApiVersion;
  data: Settings;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "Settings".
 */
export interface Settings {
  transcription: TranscriptionOptions;
  ui_language: TextLanguage;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "TranscriptionRequestEnvelope".
 */
export interface TranscriptionRequestEnvelope {
  api_version: ApiVersion;
  data: TranscriptionRequest;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "VocabularyDocumentEnvelope".
 */
export interface VocabularyDocumentEnvelope {
  api_version: ApiVersion;
  data: VocabularyDocument;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "VocabularyDocument".
 */
export interface VocabularyDocument {
  corrections: CorrectionEntry[];
  hints: HintEntry[];
  snippets: SnippetEntry[];
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "CorrectionEntry".
 */
export interface CorrectionEntry {
  enabled: boolean;
  from: string;
  id?: string | null;
  to: string;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "HintEntry".
 */
export interface HintEntry {
  enabled: boolean;
  id?: string | null;
  text: string;
}
/**
 * This interface was referenced by `ContractCatalog`'s JSON-Schema
 * via the `definition` "SnippetEntry".
 */
export interface SnippetEntry {
  enabled: boolean;
  id?: string | null;
  phrase: string;
  text: string;
}
