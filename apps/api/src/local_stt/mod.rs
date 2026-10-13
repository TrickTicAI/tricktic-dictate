//! whisper.cpp inference on the self-hosted GPU host, never browser/device inference.
//! The worker is either Writer-managed (`managed`) or an explicitly configured external server.
pub mod catalog;
pub mod hardware;
#[cfg(all(test, windows))]
mod latency_replay;
#[cfg(all(test, target_os = "macos"))]
mod macos_acceptance;
pub mod managed;
mod mixed;
#[cfg(all(test, windows))]
mod pause_replay;
pub(crate) mod pcm;
mod prepared;
mod preview;
#[cfg(all(test, any(windows, target_os = "macos")))]
mod security_acceptance;
mod segment_cache;
pub(crate) mod silence;
mod speech;
pub(crate) use preview::pause as pause_previews;
pub use preview::{native as native_preview, native_final, routes as preview_routes};
pub(crate) mod process;
#[cfg(all(test, unix))]
mod tests;

use crate::{
    providers::{ProviderInfo, SavedAudio, TranscriptionProvider},
    transcription::{
        ApiError, ErrorCode, ExecutionLocation, InputLanguage, OutputLanguage, ProviderId,
        Transcript, TranscriptionOptions,
    },
};
use async_trait::async_trait;
use reqwest::{multipart, Client, Url};
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, sync::Semaphore};

const RESPONSE_LIMIT: usize = 1024 * 1024;
const HEALTH_TTL: Duration = Duration::from_secs(30);

/// whisper-server serializes one line per decoder segment (`output_str`), so one
/// spoken sentence comes back as several lines and always ends in a newline.
/// Those breaks are segment boundaries, not dictated ones, and must not reach
/// the transcript, the clipboard or the target application.
fn join_segments(text: &str) -> String {
    let mut joined = String::new();
    for segment in text.split('\n') {
        append_phrase(&mut joined, segment);
    }
    joined
}

/// Append recognized text with exactly one word space at the seam and no
/// trailing break. Whisper already starts a segment or window with its own
/// space; a worker that does not gets one, so words never run together. The
/// leading space of the first part is kept: it is what separates one dictation
/// from the text already in the field.
fn append_phrase(text: &mut String, part: &str) {
    let part = part.trim_end();
    if part.trim_start().is_empty() {
        return;
    }
    if !text.is_empty() && !part.starts_with(char::is_whitespace) {
        text.push(' ');
    }
    text.push_str(part);
}

/// Trusted server configuration. Request model strings never become paths or URLs.
#[derive(Clone)]
pub struct LocalConfig {
    pub inference_url: String,
    /// Private managed launch identity; external servers use None. Never serialized.
    pub worker_auth: Option<managed::WorkerAuth>,
    pub model: String,
    /// API-readable copy/mount of the exact file loaded by the managed server.
    pub model_path: PathBuf,
    pub ffmpeg: PathBuf,
    pub timeout: Duration,
    /// Whole-file and converted PCM byte ceiling, not a recording duration cutoff.
    pub audio_limit: usize,
}

impl LocalConfig {
    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.worker_auth {
            Some(auth) => auth.authorize(request),
            None => request,
        }
    }
    fn authenticated(&self, response: &reqwest::Response) -> bool {
        self.worker_auth
            .as_ref()
            .is_none_or(|auth| auth.matches(response))
    }

    fn from_env() -> Option<Self> {
        Some(Self {
            worker_auth: None,
            inference_url: std::env::var("WRITER_LOCAL_STT_URL").ok()?,
            model: std::env::var("WRITER_LOCAL_STT_MODEL").ok()?,
            model_path: std::env::var_os("WRITER_LOCAL_STT_MODEL_PATH")?.into(),
            ffmpeg: std::env::var_os("WRITER_LOCAL_STT_FFMPEG")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/usr/bin/ffmpeg".into()),
            timeout: Duration::from_secs(
                std::env::var("WRITER_LOCAL_STT_TIMEOUT_SECONDS")
                    .map(|v| v.parse().ok())
                    .unwrap_or(Some(300))?,
            ),
            audio_limit: std::env::var("WRITER_LOCAL_STT_MAX_AUDIO_BYTES")
                .map(|v| v.parse().ok())
                .unwrap_or(Some(64 * 1024 * 1024))?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Unconfigured,
    MissingModel,
    MissingRuntime,
    Unreachable,
    LoadingModel,
    InvalidResponse,
    Ready,
}

enum Source {
    None,
    External(LocalConfig),
    Managed(Arc<managed::ManagedRuntime>),
}

pub struct LocalProvider {
    source: Source,
    /// Attempt deadline and converted-audio byte ceiling for the managed worker.
    limits: (Duration, usize),
    client: Client,
    ready_at: Mutex<Option<Instant>>,
    writer_worker: Mutex<Option<String>>,
    // Bound conversion buffers and inference concurrency per provider instance.
    slot: Semaphore,
    segments: Mutex<segment_cache::SegmentCache>,
    prepared_segments: Mutex<prepared::Cache>,
    active_preview: Mutex<Option<ActivePreview>>,
    /// Same-build benchmark control, absent from distributed builds.
    #[cfg(test)]
    preview_pause_reuse: bool,
}

/// The speculative request currently on the worker, with the prepared-PCM window
/// it covers, so a Stop can keep it when its result will serve the final pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActivePreview {
    pub recording: uuid::Uuid,
    /// Exact normalized audio plus model/runtime/options identity. Range alone
    /// cannot justify retaining a preview when settings or source bytes changed.
    pub key: [u8; 32],
    /// Absolute byte range of the normalized prepared PCM; None for batch input.
    pub window: Option<(u64, u64)>,
}

/// Whisper's encoder always runs over its padded 30 s context unless the request
/// limits it, and on this class of GPU the request cost is set by that context,
/// not by the audio length. Bounded live windows never exceed 15 s, so every
/// whisper model gets a qualified short-window profile with timestamps off;
/// longer batch input keeps the full context.
///
/// Qualification (2026-09-11, RX 7800 XT Vulkan, worker HTTP):
/// - 768 (15.36 s): large-v3-turbo q5_0 English, 30 public clips 3-14 s, 11/651
///   word edits against 15/651 at full context, no per-clip regression, ~500 ms
///   against ~1.15 s. ivrit-turbo Hebrew held-out: 9/170 both ways (earlier).
/// - 512 (10.24 s): same English clips up to 9.4 s, 9/363 edits equal to 768,
///   ~385 ms; it collapses once the window exceeds its context, and it broke a
///   0.76 s Hebrew clip for ivrit-turbo, so it applies only to non-ivrit windows
///   between 1.5 s and 9.5 s. Anything below 512 hallucinated and is never sent.
///
/// Whisper retries a low-confidence window at rising temperatures; with the
/// default step of 0.2 a hard clip costs up to five extra decodes (one of
/// the 8.9 s test clips: 1.5 s at 512, 4-5 s at full context). A step of 0.5
/// keeps that safety net but bounds it to two retries: identical word edits on
/// the English gate at both contexts, identical Hebrew held-out edits for
/// ivrit-turbo (9/170), and about 1.0 s on that clip. Removing the fallback
/// entirely produced a 220-word runaway on the same clip and is never used.
pub const SHORT_WINDOW_BYTES: usize = 15 * 32_000 + 80;
pub const SHORT_CONTEXT_BYTES: usize = 9_500 * 32 + 80;
const SHORT_CONTEXT_MIN_BYTES: usize = 1_500 * 32;
pub const TEMPERATURE_INC: &str = "0.5";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WhisperProfile {
    pub audio_ctx: u32,
    pub no_timestamps: bool,
}
pub fn whisper_profile(model: &str, wav_len: usize) -> WhisperProfile {
    let short = wav_len <= SHORT_WINDOW_BYTES;
    let ivrit = model == "ivrit-turbo";
    let audio_ctx = if !short {
        0
    } else if !ivrit && (SHORT_CONTEXT_MIN_BYTES..=SHORT_CONTEXT_BYTES).contains(&wav_len) {
        512
    } else {
        768
    };
    WhisperProfile {
        audio_ctx,
        no_timestamps: short || ivrit,
    }
}

impl LocalProvider {
    pub fn new(config: Option<LocalConfig>) -> Result<Self, ApiError> {
        if let Some(c) = &config {
            let url = Url::parse(&c.inference_url).map_err(|_| invalid_config())?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/inference"
                || !matches!(c.model.as_str(), "ivrit-turbo" | "whisper-tiny")
                || !c.model_path.is_absolute()
                || !c.ffmpeg.is_absolute()
                || c.timeout.is_zero()
                || c.audio_limit == 0
                || c.audio_limit > 1024 * 1024 * 1024
            {
                return Err(invalid_config());
            }
        }
        Ok(Self {
            source: config.map(Source::External).unwrap_or(Source::None),
            limits: limits_from_env(),
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| invalid_config())?,
            ready_at: Mutex::new(None),
            writer_worker: Mutex::new(None),
            slot: Semaphore::new(1),
            segments: Mutex::new(segment_cache::SegmentCache::default()),
            prepared_segments: Mutex::new(prepared::Cache::default()),
            active_preview: Mutex::new(None),
            #[cfg(test)]
            preview_pause_reuse: true,
        })
    }

    /// Inference against the Writer-owned worker; identity/GPU proof lives in the runtime.
    pub fn managed(runtime: Arc<managed::ManagedRuntime>) -> Self {
        let mut provider = Self::new(None).expect("unconfigured local client");
        provider.source = Source::Managed(runtime);
        provider
    }

    /// Current trusted configuration: the loaded managed worker or the external server.
    fn current(&self) -> Option<LocalConfig> {
        match &self.source {
            Source::None => None,
            Source::External(config) => Some(config.clone()),
            Source::Managed(runtime) => {
                let loaded = runtime.loaded()?;
                Some(LocalConfig {
                    worker_auth: Some(loaded.auth),
                    inference_url: format!(
                        "http://127.0.0.1:{}/{}",
                        loaded.port,
                        if managed::engine::nemo(&loaded.model) {
                            "v1/audio/transcriptions"
                        } else {
                            "inference"
                        }
                    ),
                    model: loaded.model,
                    model_path: loaded.model_path,
                    ffmpeg: runtime.layout.ffmpeg.clone()?,
                    timeout: self.limits.0,
                    audio_limit: self.limits.1,
                })
            }
        }
    }

    fn unavailable(&self) -> ApiError {
        match &self.source {
            Source::Managed(runtime) => worker_unavailable(&runtime.worker()),
            Source::None => error(ErrorCode::ProviderUnavailable,
                "No local transcription model is configured. Select a model in Settings → Transcription, then retry the saved audio.", true),
            Source::External(_) => unavailable(),
        }
    }

    /// Fresh bounded probe. For an external server, health proves a model is loaded, not
    /// its identity or GPU; the managed runtime verified both from its own child's log.
    pub async fn readiness(&self) -> Readiness {
        let Some(c) = self.current() else {
            let state = match &self.source {
                Source::Managed(runtime) => match runtime.worker() {
                    managed::WorkerState::MissingRuntime { .. } => Readiness::MissingRuntime,
                    managed::WorkerState::MissingWeights { .. } => Readiness::MissingModel,
                    managed::WorkerState::Starting { .. }
                    | managed::WorkerState::Stopping { .. } => Readiness::LoadingModel,
                    managed::WorkerState::Failed { .. } => Readiness::Unreachable,
                    managed::WorkerState::Idle | managed::WorkerState::Ready { .. } => {
                        Readiness::Unconfigured
                    }
                },
                _ => Readiness::Unconfigured,
            };
            *self.ready_at.lock().unwrap() = None;
            return state;
        };
        let state = match tokio::time::timeout(Duration::from_secs(5), self.probe(&c)).await {
            Ok(state) => state,
            Err(_) => Readiness::Unreachable,
        };
        *self.ready_at.lock().unwrap() = (state == Readiness::Ready).then(Instant::now);
        state
    }

    async fn probe(&self, c: &LocalConfig) -> Readiness {
        if !readable_nonempty_file(&c.model_path).await {
            return Readiness::MissingModel;
        }
        // Do not launch a codec subprocess on readiness/first dictation. PCM is
        // decoded in-process; compressed input verifies ffmpeg when it is used.
        if !readable_nonempty_file(&c.ffmpeg).await {
            return Readiness::MissingRuntime;
        }
        let mut url = Url::parse(&c.inference_url).expect("validated URL");
        url.set_path("/health");
        let response = match c.authorize(self.client.get(url)).send().await {
            Ok(response) => response,
            Err(_) => return Readiness::Unreachable,
        };
        if !c.authenticated(&response) {
            return Readiness::InvalidResponse;
        }
        let status = response.status();
        let bytes = match bounded_response(response, 4096).await {
            Ok(bytes) => bytes,
            Err(_) => return Readiness::InvalidResponse,
        };
        #[derive(Deserialize)]
        struct Health {
            status: String,
            writer_protocol: Option<u32>,
            warm: Option<bool>,
        }
        match serde_json::from_slice::<Health>(&bytes) {
            Ok(h) if status.is_success() && h.status == "ok" => {
                *self.writer_worker.lock().unwrap() = (h.writer_protocol == Some(1)
                    && h.warm == Some(true))
                .then(|| c.inference_url.clone());
                Readiness::Ready
            }
            Ok(h)
                if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                    && h.status == "loading model" =>
            {
                Readiness::LoadingModel
            }
            _ => Readiness::InvalidResponse,
        }
    }

    async fn finalize(&self, c: &LocalConfig) {
        if self.writer_worker.lock().unwrap().as_deref() != Some(&c.inference_url) {
            return;
        }
        let mut url = Url::parse(&c.inference_url).expect("validated URL");
        url.set_path("/writer/finalize");
        let _ = c
            .authorize(self.client.post(url))
            // reqwest does not automatically delimit an empty POST body. The
            // worker's HTTP parser otherwise waits until our 500ms timeout.
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .body(Vec::new())
            .timeout(Duration::from_millis(500))
            .send()
            .await;
    }

    async fn execute(
        &self,
        c: &LocalConfig,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
        preview: bool,
    ) -> Result<Transcript, ApiError> {
        let incremental = preview || self.segments.lock().unwrap().contains(audio.metadata.id);
        if !self
            .ready_at
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < HEALTH_TTL)
        {
            let readiness = self.readiness().await;
            if readiness != Readiness::Ready {
                return Err(match readiness {
                    Readiness::MissingModel => error(ErrorCode::ProviderUnavailable, "The selected local model file is missing or unreadable. Verify it in Settings → Transcription, then retry the saved audio.", false),
                    Readiness::MissingRuntime => error(ErrorCode::ProviderUnavailable, "The bundled audio converter is missing or unreadable. Reinstall or update TrickTic Dictate, then retry the saved audio.", false),
                    Readiness::InvalidResponse => error(ErrorCode::ProviderUnavailable, "The local speech worker returned an invalid or unauthenticated health response. Update TrickTic Dictate to repair its bundled runtime, then retry the saved audio.", false),
                    _ => self.unavailable(),
                });
            }
        }
        if audio.prepared.is_some() {
            let source = tokio::fs::metadata(&audio.path)
                .await
                .map_err(|_| audio_missing())?;
            if !source.is_file()
                || source.len() < audio.metadata.byte_length
                || (!preview && source.len() != audio.metadata.byte_length)
            {
                return Err(audio_missing());
            }
            // Final recognition consumes a disposable compacted copy. Preview
            // offsets and its journal remain in original coordinates; the
            // existing digest check rejects any prefix changed by compaction.
            let derived = if preview {
                None
            } else {
                Some(silence::prepared_copy(audio).await?)
            };
            let input_audio = derived.as_ref().map_or(audio, |copy| &copy.audio);
            let raw_text = self.prepared_text(c, input_audio, options, preview).await?;
            return Ok(Transcript {
                raw_text,
                selection: options.selection.clone(),
                execution_location: ExecutionLocation::SelfHostedServer,
                input_language: options.input_language.clone(),
                output_language: options.output_language.clone(),
            });
        }
        let format = match audio
            .metadata
            .mime_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
        {
            "audio/webm" | "video/webm" => "matroska",
            "audio/ogg" | "application/ogg" => "ogg",
            "audio/mp4" | "video/mp4" | "audio/x-m4a" => "mov",
            "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
            "audio/mpeg" => "mp3",
            "audio/flac" | "audio/x-flac" => "flac",
            _ => {
                return Err(error(
                    ErrorCode::UnsupportedCapability,
                    "Local transcription does not support this audio container.",
                    false,
                ))
            }
        };
        let input = tokio::fs::canonicalize(&audio.path)
            .await
            .map_err(|_| audio_missing())?;
        let metadata = tokio::fs::metadata(&input)
            .await
            .map_err(|_| audio_missing())?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(audio_missing());
        }
        if metadata.len() != audio.metadata.byte_length {
            return Err(error(
                ErrorCode::AudioNotSaved,
                "Saved audio length does not match its metadata.",
                false,
            ));
        }
        if metadata.len() > c.audio_limit as u64 {
            return Err(audio_too_large());
        }
        // Fixed demuxer and protocols prevent playlists from fetching URLs/other files.
        // stdout is bounded PCM; the source is opened only as input and never removed.
        let args = [
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            format,
            "-i",
        ]
        .into_iter()
        .map(Into::into)
        .chain(std::iter::once(input.into_os_string()))
        .chain(
            [
                "-vn",
                "-ar",
                "16000",
                "-ac",
                "1",
                "-c:a",
                "pcm_s16le",
                "-f",
                "s16le",
                "pipe:1",
            ]
            .into_iter()
            .map(Into::into),
        )
        .collect::<Vec<_>>();
        let pcm = match pcm::convert(&audio.path, c.audio_limit, true).await? {
            Some(pcm) => pcm,
            None => process::run(&c.ffmpeg, &args, c.audio_limit, c.timeout).await?,
        };
        // NeMo rejects the unknown RIFF/data lengths emitted by ffmpeg to a pipe.
        // Give it a finite WAV header over the already bounded normalized PCM.
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return Err(invalid_response());
        }
        let pcm = if preview {
            pcm
        } else {
            let id = audio.metadata.id;
            tokio::task::spawn_blocking(move || silence::prepare(id, &pcm))
                .await
                .map_err(|_| invalid_response())?
        };
        let wav = if incremental || matches!(options.input_language, InputLanguage::Mixed) {
            pcm
        } else {
            mixed::wav(&pcm)
        };
        let raw_text = if incremental || matches!(options.input_language, InputLanguage::Mixed) {
            if wav.is_empty() || wav.len() % 2 != 0 {
                return Err(invalid_response());
            }
            let (pcm, ranges) = tokio::task::spawn_blocking(move || {
                let ranges = if incremental {
                    mixed::live_ranges(&wav)
                } else {
                    mixed::ranges(&wav)
                };
                (wav, ranges)
            })
            .await
            .map_err(|_| invalid_response())?;
            let count = ranges.len();
            let mut text = String::new();
            let mut inferred = false;
            for (index, range) in ranges.into_iter().enumerate() {
                // A growing tail is provisional. Only closed, stable PCM ranges are
                // precomputed; the final job always includes every remaining sample.
                if preview && (index + 1 == count || range.end.saturating_add(3_200) > pcm.len()) {
                    break;
                }
                let wav = mixed::wav(&pcm[range]);
                let key = segment_cache::key(audio.metadata.id, c, options, &wav);
                let cached = if incremental {
                    self.segments.lock().unwrap().get(&key)
                } else {
                    None
                };
                let part = if let Some(text) = cached {
                    text
                } else {
                    if preview && inferred {
                        break;
                    }
                    let text = self
                        .infer(c, wav, options, preview, audio.metadata.id, None)
                        .await?;
                    if incremental {
                        self.segments
                            .lock()
                            .unwrap()
                            .put(audio.metadata.id, key, text.clone());
                    }
                    inferred = true;
                    text
                };
                if text.len().saturating_add(part.len()).saturating_add(1) > RESPONSE_LIMIT {
                    return Err(invalid_response());
                }
                append_phrase(&mut text, &part);
            }
            text
        } else {
            if wav.len() < 12 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
                return Err(error(
                    ErrorCode::ProviderFailed,
                    "Audio conversion did not produce WAV data; saved audio is retained.",
                    true,
                ));
            }
            self.infer(c, wav, options, preview, audio.metadata.id, None)
                .await?
        };
        Ok(Transcript {
            raw_text,
            selection: options.selection.clone(),
            execution_location: ExecutionLocation::SelfHostedServer,
            input_language: options.input_language.clone(),
            output_language: options.output_language.clone(),
        })
    }

    fn validate_options(&self, options: &TranscriptionOptions) -> Result<LocalConfig, ApiError> {
        if !matches!(options.selection.provider, ProviderId::Local) {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Select the configured local model alias.",
                false,
            ));
        }
        if matches!(self.source, Source::Managed(_)) {
            crate::language::validate_capabilities(options)?;
        }
        let c = self.current().ok_or_else(|| self.unavailable())?;
        if options.selection.model != c.model {
            return Err(match &self.source {
                Source::Managed(_) => error(
                    ErrorCode::ProviderUnavailable,
                    "The selected local model is not loaded. Open transcription setup to load it, then retry the saved audio.",
                    true,
                ),
                _ => error(
                    ErrorCode::InvalidRequest,
                    "Select the configured local model alias.",
                    false,
                ),
            });
        }
        if !matches!(options.output_language, OutputLanguage::Preserve { .. })
            || (c.model == "ivrit-turbo" && !matches!(options.input_language, InputLanguage::He))
        {
            return Err(error(
                ErrorCode::UnsupportedCapability,
                "This local model supports preservation only; ivrit-turbo requires Hebrew input.",
                false,
            ));
        }
        if options.vocabulary.hints.len() > 128
            || options
                .vocabulary
                .hints
                .iter()
                .map(String::len)
                .sum::<usize>()
                > 4096
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Local vocabulary hints exceed 128 entries or 4096 bytes.",
                false,
            ));
        }
        Ok(c)
    }

    async fn infer(
        &self,
        c: &LocalConfig,
        wav: Vec<u8>,
        options: &TranscriptionOptions,
        preview: bool,
        recording_id: uuid::Uuid,
        window: Option<(u64, u64)>,
    ) -> Result<String, ApiError> {
        // Saved native WAV retries also take this path without prepared PCM.
        // Never send a clearly speechless window to the generative decoder.
        // Already recognized phrases retain their conservative tail handling.
        if window.is_none_or(|(start, _)| start == 0)
            && writer_dictation_audio::wav_pcm16(&wav).is_ok_and(|pcm| {
                pcm.rate == 16_000 && pcm.channels == 1 && speech::speechless(pcm.data)
            })
        {
            crate::latency::mark(recording_id, "no_speech_skipped");
            return Ok(String::new());
        }
        crate::latency::mark(
            recording_id,
            if preview {
                "preview_inference_start"
            } else {
                "final_inference_start"
            },
        );
        crate::latency::count(
            recording_id,
            if preview {
                "preview_inference_calls"
            } else {
                "final_inference_calls"
            },
        );
        struct Active<'a>(&'a Mutex<Option<ActivePreview>>);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                *self.0.lock().unwrap() = None;
            }
        }
        let _active = if preview {
            *self.active_preview.lock().unwrap() = Some(ActivePreview {
                recording: recording_id,
                key: segment_cache::key(recording_id, c, options, &wav),
                window,
            });
            Some(Active(&self.active_preview))
        } else {
            crate::latency::detail(recording_id, "final_window_ms", (wav.len() / 32) as u64);
            None
        };
        let language = match options.input_language {
            InputLanguage::En => "en",
            InputLanguage::He => "he",
            InputLanguage::Mixed => "auto",
        };
        let profile = whisper_profile(&c.model, wav.len());
        let mut form = multipart::Form::new()
            .part(
                "file",
                multipart::Part::bytes(wav)
                    .file_name("recording.wav")
                    .mime_str("audio/wav")
                    .expect("constant MIME"),
            )
            .text("language", language)
            .text("response_format", "json");
        if managed::engine::nemo(&c.model) {
            form = form.text("verbatim", "true");
            if !options.vocabulary.hints.is_empty() {
                form = form.text(
                    "speech_contexts",
                    serde_json::json!([{"phrases": options.vocabulary.hints, "boost": 3}])
                        .to_string(),
                );
            }
        } else {
            form = form
                .text("translate", "false")
                .text("temperature", "0.0")
                .text("best_of", "2")
                .text("beam_size", "-1")
                .text("temperature_inc", TEMPERATURE_INC)
                .text("prompt", options.vocabulary.hints.join(", "));
            // Pin every inference-affecting option: whisper-server retains request params.
            // No weights or quantization are changed; only the encoder window and
            // the timestamp decoding differ between short live windows and batch input.
            crate::latency::detail(recording_id, "audio_ctx", profile.audio_ctx);
            crate::latency::detail(recording_id, "no_timestamps", profile.no_timestamps);
            form = form.text("audio_ctx", profile.audio_ctx.to_string()).text(
                "no_timestamps",
                if profile.no_timestamps {
                    "true"
                } else {
                    "false"
                },
            );
            if self.writer_worker.lock().unwrap().as_deref() == Some(&c.inference_url) {
                form = form.text("writer_preview", if preview { "true" } else { "false" });
            }
        }
        let response = c
            .authorize(self.client.post(&c.inference_url))
            .multipart(form)
            .send()
            .await
            .map_err(network_error)?;
        if !c.authenticated(&response) {
            return Err(error(ErrorCode::ProviderUnavailable, "The local speech worker's identity could not be verified. Restart or update TrickTic Dictate, then retry the saved audio.", true));
        }
        let status = response.status();
        if !status.is_success() {
            return Err(error(
                if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                    ErrorCode::ProviderUnavailable
                } else {
                    ErrorCode::ProviderFailed
                },
                "Local inference failed; saved audio is retained for retry.",
                status.is_server_error(),
            ));
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Output {
            text: String,
        }
        let bytes = bounded_response(response, RESPONSE_LIMIT).await?;
        let output: Output = serde_json::from_slice(&bytes).map_err(|_| invalid_response())?;
        crate::latency::mark(
            recording_id,
            if preview {
                "preview_inference_done"
            } else {
                "final_inference_done"
            },
        );
        Ok(join_segments(&output.text))
    }
}

#[async_trait]
impl TranscriptionProvider for LocalProvider {
    fn automatic_retry_ready(&self) -> bool {
        match &self.source {
            Source::Managed(runtime) => match runtime.worker() {
                managed::WorkerState::Starting { .. } | managed::WorkerState::Stopping { .. } => {
                    false
                }
                managed::WorkerState::Idle => runtime.desired().is_none(),
                _ => true,
            },
            _ => true,
        }
    }
    fn automatic_retry(&self, error: &ApiError) -> bool {
        if !error.retryable {
            return false;
        }
        if matches!(
            error.code,
            ErrorCode::ProviderTimeout | ErrorCode::ProviderFailed
        ) {
            return true;
        }
        if !matches!(
            error.code,
            ErrorCode::ProviderUnavailable | ErrorCode::IdempotencyConflict
        ) {
            return false;
        }
        match &self.source {
            Source::None => false,
            Source::External(_) => self
                .current()
                .is_some_and(|c| c.model_path.is_file() && c.ffmpeg.is_file()),
            Source::Managed(runtime) => match runtime.worker() {
                managed::WorkerState::Ready { .. }
                | managed::WorkerState::Starting { .. }
                | managed::WorkerState::Stopping { .. } => true,
                managed::WorkerState::Failed { error, .. } => matches!(
                    error.as_str(),
                    "worker_start_failed"
                        | "worker_exited_during_startup"
                        | "worker_start_timeout"
                        | "worker_exited"
                ),
                _ => false,
            },
        }
    }
    fn clear_preview(&self) {
        *self.prepared_segments.lock().unwrap() = prepared::Cache::default();
        *self.segments.lock().unwrap() = segment_cache::SegmentCache::default();
    }
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::Local,
            execution_location: ExecutionLocation::SelfHostedServer,
            configured: match &self.source {
                Source::None => false,
                Source::External(_) => true,
                Source::Managed(runtime) => runtime.desired().is_some(),
            },
            available: self
                .ready_at
                .lock()
                .unwrap()
                .is_some_and(|at| at.elapsed() < HEALTH_TTL),
        }
    }

    async fn preview(
        &self,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> Result<String, ApiError> {
        let _lease = match &self.source {
            Source::Managed(runtime) => Some(runtime.inference_lease()?),
            _ => None,
        };
        let c = self.validate_options(options)?;
        let _permit = self.slot.try_acquire().map_err(|_| error(ErrorCode::ProviderUnavailable, "The local speech worker is busy with another transcription. Saved audio can be retried when it finishes.", true))?;
        self.segments.lock().unwrap().activate(audio.metadata.id);
        tokio::time::timeout(c.timeout, self.execute(&c, audio, options, true))
            .await
            .unwrap_or_else(|_| Err(timeout_error()))
            .map(|t| t.raw_text)
    }

    async fn transcribe(
        &self,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        let _lease = match &self.source {
            Source::Managed(runtime) => Some(runtime.inference_lease()?),
            _ => None,
        };
        let c = self.validate_options(options)?;
        let c = &c;
        let warm = matches!(&self.source, Source::Managed(runtime) if runtime.loaded().is_some_and(|loaded| loaded.model == c.model))
            && self
                .ready_at
                .lock()
                .unwrap()
                .is_some_and(|at| at.elapsed() < HEALTH_TTL);
        crate::latency::measurements::set_warm(
            audio.metadata.id,
            if warm {
                crate::latency::measurements::Warm::Warm
            } else {
                crate::latency::measurements::Warm::Unknown
            },
        );
        // Cancel speculative compute before taking the local serialization slot;
        // waiting first would defeat worker preemption. Legacy workers keep FIFO.
        crate::latency::mark(audio.metadata.id, "final_requested");
        crate::latency::detail(audio.metadata.id, "model", c.model.clone());
        if let Source::Managed(runtime) = &self.source {
            if let managed::WorkerState::Ready { backend, .. } = runtime.worker() {
                crate::latency::detail(audio.metadata.id, "backend", backend);
            }
        }
        // A speculative request already covering every voiced frame is kept and
        // reused; anything else is canceled so the final pass starts at once.
        let retained = self.active_preview.lock().unwrap().is_some()
            && self.retainable(c, audio, options).await;
        if retained {
            crate::latency::mark(audio.metadata.id, "matching_preview_retained");
        } else {
            self.finalize(c).await;
        }
        // New previews never queue for this slot.
        let acquire = self.slot.acquire();
        tokio::pin!(acquire);
        let _permit = loop {
            tokio::select! {
                permit = &mut acquire => break permit.map_err(|_| unavailable())?,
                _ = tokio::time::sleep(Duration::from_millis(50)) => { if !retained { self.finalize(c).await; } },
            }
        };
        crate::latency::mark(audio.metadata.id, "provider_slot_acquired");
        let result = tokio::time::timeout(c.timeout, self.execute(c, audio, options, false))
            .await
            .unwrap_or_else(|_| Err(timeout_error()));
        if result.is_err() {
            *self.ready_at.lock().unwrap() = None;
            if result.as_ref().is_err_and(|e| {
                matches!(
                    e.code,
                    ErrorCode::ProviderFailed | ErrorCode::ProviderTimeout
                )
            }) {
                if let Source::Managed(runtime) = &self.source {
                    runtime.inference_failed();
                }
            }
        }
        result
    }
}

fn limits_from_env() -> (Duration, usize) {
    let timeout = std::env::var("WRITER_LOCAL_STT_TIMEOUT_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|s: &u64| *s > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(300));
    let limit = std::env::var("WRITER_LOCAL_STT_MAX_AUDIO_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|b: &usize| *b > 0 && *b <= 1024 * 1024 * 1024)
        .unwrap_or(64 * 1024 * 1024);
    (timeout, limit)
}

/// An explicit external server (`WRITER_LOCAL_STT_URL`) keeps the operator-managed mode;
/// otherwise inference goes through the Writer-managed worker.
pub fn adapters_with(
    setup: Option<&crate::desktop_settings::Setup>,
) -> Vec<Arc<dyn TranscriptionProvider>> {
    // An installed desktop's local choice must stay on that PC even if an old
    // developer shell left an external-server URL in its environment.
    let external = if setup.is_some_and(|s| s.layout.mode == crate::desktop_settings::Mode::Desktop)
    {
        None
    } else {
        LocalConfig::from_env()
    };
    let provider = match (external, setup) {
        (Some(config), _) => LocalProvider::new(Some(config)).unwrap_or_else(|_| {
            // No configuration values, paths or transport errors enter logs or HTTP errors.
            LocalProvider::new(None).expect("unconfigured local client")
        }),
        (None, Some(setup)) => LocalProvider::managed(setup.runtime.clone()),
        (None, None) => LocalProvider::new(None).expect("unconfigured local client"),
    };
    let provider = Arc::new(provider);
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let weak = Arc::downgrade(&provider);
        runtime.spawn(async move {
            while let Some(probe) = weak.upgrade() {
                probe.readiness().await;
                drop(probe);
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });
    }
    vec![provider]
}

pub fn adapters() -> Vec<Arc<dyn TranscriptionProvider>> {
    adapters_with(crate::desktop_settings::global().ok().as_deref())
}

async fn readable_nonempty_file(path: &std::path::Path) -> bool {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
    let Ok(metadata) = file.metadata().await else {
        return false;
    };
    metadata.is_file() && metadata.len() > 0 && file.read_u8().await.is_ok()
}

async fn bounded_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ApiError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(invalid_response());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(invalid_response());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn error(code: ErrorCode, message: &str, retryable: bool) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
        retry_after_ms: None,
    }
}
fn invalid_config() -> ApiError {
    error(ErrorCode::ProviderUnavailable, "Local configuration requires an HTTP /inference URL, an allowlisted model, absolute model/runtime paths and positive limits (audio at most 1 GiB).", false)
}
fn unavailable() -> ApiError {
    error(ErrorCode::ProviderUnavailable, "The local speech worker could not be reached. Restart the local model in Settings → Transcription, then retry the saved audio.", true)
}
fn worker_unavailable(state: &managed::WorkerState) -> ApiError {
    use managed::WorkerState;
    let message = match state {
        WorkerState::MissingRuntime { .. } => "The bundled speech runtime is missing. Reinstall or update TrickTic Dictate, then retry the saved audio.",
        WorkerState::MissingWeights { .. } => "The selected model has not been downloaded or verified. Open Settings → Transcription to prepare it, then retry the saved audio.",
        WorkerState::Starting { .. } => "The local model is still starting. Wait for Ready in Settings → Transcription, then retry the saved audio.",
        WorkerState::Stopping { .. } => "The local model is stopping. Wait for it to restart, then retry the saved audio.",
        WorkerState::Idle => "No local model is loaded. Select one in Settings → Transcription, then retry the saved audio.",
        WorkerState::Failed { error, .. } => match error.as_str() {
            "worker_identity_mismatch" | "worker_handshake_failed" => "The bundled speech worker could not complete its authentication handshake. Update or reinstall TrickTic Dictate to repair the runtime; your saved audio can be retried.",
            "worker_start_timeout" => "The local speech worker did not become ready before the startup deadline. Restart the model in Settings → Transcription; if this began after an update, update or reinstall TrickTic Dictate to repair its bundled runtime. Saved audio can be retried.",
            "weights_missing" | "weights_changed_since_verification" => "The local model file is missing or changed since verification. Verify it in Settings → Transcription, then retry the saved audio.",
            "gpu_unavailable" | "gpu_backend_not_used" => "The local model could not start on the GPU. Choose Automatic or CPU in Settings → Transcription, then retry the saved audio.",
            "worker_exited_during_startup" => "The local speech worker exited while loading the model. Restart it in Settings → Transcription, then retry the saved audio.",
            _ => "The local speech worker failed to start. Open Settings → Transcription for its runtime status, then retry the saved audio.",
        },
        WorkerState::Ready { .. } => "The local speech worker stopped responding. Restart the model in Settings → Transcription, then retry the saved audio.",
    };
    error(ErrorCode::ProviderUnavailable, message, true)
}
fn audio_missing() -> ApiError {
    error(
        ErrorCode::AudioNotSaved,
        "Saved audio is missing or unreadable.",
        false,
    )
}
fn audio_too_large() -> ApiError {
    error(ErrorCode::UnsupportedCapability, "Audio exceeds the configured local byte limit; retained audio can be segmented or retried with a larger limit.", false)
}
fn invalid_response() -> ApiError {
    error(
        ErrorCode::InvalidProviderResponse,
        "Local runtime returned malformed or oversized output; saved audio is retained.",
        true,
    )
}
fn timeout_error() -> ApiError {
    error(
        ErrorCode::ProviderTimeout,
        "Local transcription timed out; saved audio is retained for retry.",
        true,
    )
}
fn network_error(e: reqwest::Error) -> ApiError {
    if e.is_timeout() {
        timeout_error()
    } else {
        unavailable()
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) mod worker_job;
