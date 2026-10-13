//! Persistent in-app transcription configuration: selected model/languages, the managed
//! local runtime, credential presence and the setup routes used by the web UI and the
//! native Windows setup module. Configuration presence never claims working inference.
pub mod catalog;
pub mod cloud_check;
pub mod credentials;

/// The desktop app's data folder under `%LOCALAPPDATA%` or
/// `~/Library/Application Support`, named after its bundle identifier. The
/// desktop shell always sets `WRITER_DATA_DIR`; this is only the fallback.
pub const DESKTOP_DATA_FOLDER: &str = credentials::KEYCHAIN_SERVICE;

use crate::{
    language,
    local_stt::{
        catalog as legacy_catalog,
        managed::{
            ExecutionBackend, ExecutionView, ManagedRuntime, RuntimeLayout, WeightsState,
            WorkerState,
        },
    },
    recordings::disk::{private_dir, write_json},
    transcription::{
        ApiError, CleanupPolicy, Envelope, ErrorCode, InputLanguage, ModelSelection,
        OutputLanguage, ProviderId, Settings, TextLanguage, TranscriptionOptions, Vocabulary,
    },
};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path as AxumPath, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cloud_check::{CheckOutcome, CloudCheck};
use credentials::{CredentialStatus, Credentials};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Desktop,
    SelfHosted,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub mode: Mode,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub models_dir: PathBuf,
    pub runtime_dir: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub startup_timeout: Duration,
}

fn sibling(data_dir: &Path, sibling: &str, nested: &str) -> PathBuf {
    match (
        data_dir.file_name().and_then(|n| n.to_str()),
        data_dir.parent(),
    ) {
        (Some("Data"), Some(parent)) => parent.join(sibling),
        _ => data_dir.join(nested),
    }
}

impl Layout {
    /// Desktop packages are recognized by the resource manifest beside the backend
    /// directory; otherwise explicit environment settings select a managed runtime.
    pub fn from_env() -> Result<Self, ApiError> {
        let data_dir = std::env::var_os("WRITER_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                #[cfg(windows)]
                return std::env::var_os("LOCALAPPDATA")
                    .map(|p| PathBuf::from(p).join(DESKTOP_DATA_FOLDER).join("Data"));
                #[cfg(target_os = "macos")]
                return std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home)
                        .join("Library/Application Support")
                        .join(DESKTOP_DATA_FOLDER)
                        .join("Data")
                });
                #[cfg(not(any(windows, target_os = "macos")))]
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share/metrix-writer"))
            })
            .ok_or_else(|| ApiError {
                code: ErrorCode::StorageFailed,
                message: "Set WRITER_DATA_DIR to a private writable data directory.".into(),
                retryable: false,
                retry_after_ms: None,
            })?;
        let package_root = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent()?.parent().map(Path::to_path_buf))
            .filter(|root| root.join("resources.json").is_file());
        let runtime_dir = std::env::var_os("WRITER_RUNTIME_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                package_root
                    .as_ref()
                    .map(|root| root.join("runtime").join("whisper"))
                    .filter(|dir| dir.is_dir())
            });
        let ffmpeg = std::env::var_os("WRITER_LOCAL_STT_FFMPEG")
            .map(PathBuf::from)
            .or_else(|| {
                package_root
                    .as_ref()
                    .map(|root| root.join("runtime").join("ffmpeg").join("ffmpeg.exe"))
                    .filter(|path| path.is_file())
            })
            .or_else(|| {
                let default = PathBuf::from("/usr/bin/ffmpeg");
                (!cfg!(windows) && default.is_file()).then_some(default)
            });
        let startup_timeout = std::env::var("WRITER_LOCAL_STT_STARTUP_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|s: &u64| *s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(180));
        Ok(Self {
            mode: if package_root.is_some()
                || std::env::var("WRITER_DESKTOP_IN_PROCESS").as_deref() == Ok("1")
            {
                Mode::Desktop
            } else {
                Mode::SelfHosted
            },
            state_dir: std::env::var_os("WRITER_STATE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| sibling(&data_dir, "State", "state")),
            models_dir: std::env::var_os("WRITER_MODELS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| sibling(&data_dir, "Models", "models")),
            data_dir,
            runtime_dir,
            ffmpeg,
            startup_timeout,
        })
    }
    fn runtime_layout(&self) -> RuntimeLayout {
        RuntimeLayout {
            runtime_dir: self.runtime_dir.clone(),
            ffmpeg: self.ffmpeg.clone(),
            models_dir: self.models_dir.clone(),
            state_dir: self.state_dir.clone(),
            startup_timeout: self.startup_timeout,
        }
    }
}

pub fn default_settings() -> Settings {
    Settings {
        ui_language: TextLanguage::En,
        transcription: TranscriptionOptions {
            selection: ModelSelection {
                provider: ProviderId::Local,
                model: "whisper-large-v3-turbo".into(),
            },
            input_language: InputLanguage::He,
            output_language: OutputLanguage::Preserve {},
            vocabulary: Vocabulary {
                hints: vec![],
                corrections: vec![],
                snippets: vec![],
            },
            cleanup: CleanupPolicy::Off {},
        },
    }
}

pub struct Setup {
    pub layout: Layout,
    settings_path: PathBuf,
    settings: Mutex<Option<Settings>>,
    write_lock: tokio::sync::Mutex<()>,
    pub credentials: Arc<Credentials>,
    pub runtime: Arc<ManagedRuntime>,
    /// Last reachability check per provider, kept only for this process and only
    /// while the stored key is still the one that was checked.
    cloud_checks: Mutex<HashMap<&'static str, CheckRecord>>,
    cloud_endpoints: Mutex<cloud_check::Endpoints>,
}

/// A check plus the fingerprint of the key it was made with. Replacing the key
/// retires the result rather than letting an old success vouch for a new key,
/// which is the same rule `verified` already follows.
#[derive(Clone, Copy)]
struct CheckRecord {
    check: CloudCheck,
    fingerprint: [u8; 32],
}

fn fingerprint(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).into()
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

static GLOBAL: OnceLock<Result<Arc<Setup>, ApiError>> = OnceLock::new();

/// Process-wide setup shared by adapters, discovery and routes.
pub fn global() -> Result<Arc<Setup>, ApiError> {
    GLOBAL
        .get_or_init(|| Layout::from_env().and_then(Setup::open))
        .clone()
}

fn storage(_: impl std::fmt::Display) -> ApiError {
    ApiError {
        code: ErrorCode::StorageFailed,
        message: "Transcription settings storage is unavailable; previously saved choices were not intentionally removed.".into(),
        retryable: true,
        retry_after_ms: None,
    }
}

impl Setup {
    pub fn open(layout: Layout) -> Result<Arc<Self>, ApiError> {
        let credentials = Arc::new(Credentials::new(&layout.state_dir));
        Self::open_with(layout, credentials, catalog::specs())
    }
    pub fn open_with(
        layout: Layout,
        credentials: Arc<Credentials>,
        models: &'static [legacy_catalog::ModelSpec],
    ) -> Result<Arc<Self>, ApiError> {
        std::fs::create_dir_all(&layout.state_dir).map_err(storage)?;
        let settings_path = layout.state_dir.join("settings.json");
        let settings: Option<Settings> = match std::fs::read(&settings_path) {
            Ok(bytes) => serde_json::from_slice::<Envelope<Settings>>(&bytes)
                .ok()
                .map(|e| e.data)
                .filter(|s| language::validate_capabilities(&s.transcription).is_ok()),
            Err(_) => None,
        };
        let runtime = ManagedRuntime::open_with_catalog(layout.runtime_layout(), models)?;
        let setup = Arc::new(Self {
            settings_path,
            settings: Mutex::new(settings),
            write_lock: tokio::sync::Mutex::new(()),
            credentials,
            runtime,
            layout,
            cloud_checks: Mutex::new(HashMap::new()),
            cloud_endpoints: Mutex::new(cloud_check::Endpoints::default()),
        });
        setup.apply();
        Ok(setup)
    }
    pub fn first_run(&self) -> bool {
        self.settings.lock().unwrap().is_none()
    }
    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(default_settings)
    }
    /// Only an explicitly saved local selection loads a worker; first run stays idle.
    fn apply(&self) {
        let saved = self.settings.lock().unwrap().clone();
        let local = saved
            .filter(|s| matches!(s.transcription.selection.provider, ProviderId::Local))
            .map(|s| s.transcription.selection.model);
        let _ = self.runtime.select(local.as_deref());
    }
    /// Validates capabilities (never Hebrew-only weights for English/mixed), persists
    /// atomically, then reconciles the managed worker. Vocabulary has its own store.
    pub async fn save(&self, mut settings: Settings) -> Result<Settings, ApiError> {
        language::validate_capabilities(&settings.transcription)?;
        settings.transcription.vocabulary = Vocabulary {
            hints: vec![],
            corrections: vec![],
            snippets: vec![],
        };
        let _guard = self.write_lock.lock().await;
        let local = matches!(settings.transcription.selection.provider, ProviderId::Local)
            .then(|| settings.transcription.selection.model.clone());
        // The maintenance lock is exclusive with downloads, verification and
        // inference, so only a save that has a worker to reconcile takes it. Taken
        // unconditionally it refused saves that changed no model at all: choosing a
        // cloud provider while local weights were still downloading was told to wait
        // for the download to finish, for a `select` that was already a no-op. A save
        // that does move the worker still holds it across the whole write, so the
        // persisted selection and the loaded model cannot disagree.
        let maintenance = if local == self.runtime.desired() {
            self.runtime.guard_recording()?;
            None
        } else {
            Some(self.runtime.maintenance()?)
        };
        private_dir(&self.layout.state_dir).await?;
        write_json(&self.settings_path, &Envelope::new(settings.clone())).await?;
        *self.settings.lock().unwrap() = Some(settings.clone());
        if let Some(maintenance) = maintenance {
            maintenance.select(local.as_deref())?;
        }
        crate::account_sync::note_local_change();
        Ok(settings)
    }
    /// Asks the provider whether the stored key is accepted, without dictating.
    ///
    /// The key never leaves the server: it is read here from the same protected
    /// store the transcription adapters read, and only the outcome is recorded.
    /// A rejected or unreachable answer is a result, not an error — the caller
    /// still receives the view so the card can show what happened. Having no key
    /// at all is the one genuine error, because there is nothing to check.
    pub async fn check_cloud(&self, provider: &ProviderId) -> Result<CheckOutcome, ApiError> {
        let Some(label) = cloud_check::label(provider) else {
            return Err(ApiError {
                code: ErrorCode::InvalidRequest,
                message: "Only cloud providers have a key to check.".into(),
                retryable: false,
                retry_after_ms: None,
            });
        };
        let Some(credential) = self.credentials.read(provider) else {
            return Err(ApiError {
                code: ErrorCode::ProviderUnavailable,
                message: "No provider key is stored, so there is nothing to check.".into(),
                retryable: false,
                retry_after_ms: None,
            });
        };
        let base = self
            .cloud_endpoints
            .lock()
            .unwrap()
            .base(provider)
            .unwrap_or_default()
            .to_owned();
        let outcome = cloud_check::run(provider, &credential.value, &base).await;
        self.cloud_checks.lock().unwrap().insert(
            label,
            CheckRecord {
                check: CloudCheck {
                    outcome,
                    checked_at_ms: now_ms(),
                },
                fingerprint: fingerprint(&credential.value),
            },
        );
        Ok(outcome)
    }

    /// A recorded check, but only while it still describes the key in the store.
    fn recorded_check(&self, provider: &ProviderId, key: &str) -> Option<CloudCheck> {
        let label = cloud_check::label(provider)?;
        let record = *self.cloud_checks.lock().unwrap().get(label)?;
        (record.fingerprint == fingerprint(key)).then_some(record.check)
    }

    #[cfg(test)]
    pub fn set_cloud_endpoints(&self, endpoints: cloud_check::Endpoints) {
        *self.cloud_endpoints.lock().unwrap() = endpoints;
    }

    pub fn view(&self, verified: &dyn Fn(&ProviderId) -> bool) -> SetupView {
        let settings = self.settings();
        let selection = &settings.transcription.selection;
        let loaded = self.runtime.loaded().map(|l| l.model);
        let models = self
            .runtime
            .catalog()
            .iter()
            .map(|spec| ModelView {
                metadata: catalog::model(spec.id),
                smoke_tests: catalog::evidence(spec.id, spec.sha256),
                id: spec.id,
                display_name: spec.display_name,
                file_name: spec.file_name,
                bytes: spec.bytes,
                sha256: spec.sha256,
                source_url: spec.source_url,
                source_name: spec.source_name,
                license: spec.license,
                notes: spec.notes,
                input_languages: catalog::model(spec.id)
                    .map(|m| m.input_languages.clone())
                    .unwrap_or_else(|| legacy_catalog::input_languages(spec.id)),
                compatibility_issue: crate::local_stt::managed::engine::compatibility_issue(
                    spec.id,
                ),
                weights: self.runtime.weights(spec.id),
                selected: matches!(selection.provider, ProviderId::Local)
                    && selection.model == spec.id,
                loaded: loaded.as_deref() == Some(spec.id),
            })
            .collect();
        let cloud = [
            (
                ProviderId::Openai,
                vec![
                    "gpt-transcribe",
                    "gpt-4o-transcribe",
                    "gpt-4o-mini-transcribe",
                    "whisper-1",
                ],
            ),
            (ProviderId::Gemini, vec!["gemini-3.8-flash"]),
            (ProviderId::Grok, vec!["grok-voice-transcribe-2.0"]),
            // A key row with no speech model: the key is set and checked here,
            // but OpenRouter only serves text processing.
            (ProviderId::Openrouter, vec![]),
        ]
        .into_iter()
        .map(|(provider, models)| {
            // Read once: the value is needed to tell whether a recorded check
            // still belongs to the key that is stored now.
            let credential = self.credentials.read(&provider);
            CloudView {
                credential: credential
                    .as_ref()
                    .map(|c| c.status)
                    .unwrap_or_else(|| self.credentials.status(&provider)),
                check: credential
                    .as_ref()
                    .and_then(|c| self.recorded_check(&provider, &c.value)),
                verified: verified(&provider),
                selected: std::mem::discriminant(&selection.provider)
                    == std::mem::discriminant(&provider),
                provider,
                models,
            }
        })
        .collect();
        let runtime_dir = self.layout.runtime_dir.as_ref();
        SetupView {
            mode: self.layout.mode,
            first_run: self.first_run(),
            settings,
            runtime: RuntimeView {
                hardware: crate::local_stt::hardware::snapshot(),
                worker: self.runtime.worker(),
                execution: self.runtime.execution(),
                problem: self.runtime.runtime_problem(),
                packaged: self.layout.mode == Mode::Desktop,
                worker_executable_present: runtime_dir.is_some_and(|d| {
                    d.join(crate::local_stt::managed::WORKER_EXECUTABLE)
                        .is_file()
                }),
                vulkan_library_present: runtime_dir.is_some_and(|d| {
                    d.join(crate::local_stt::managed::VULKAN_BACKEND_LIBRARY)
                        .is_file()
                }),
                ffmpeg_present: self.layout.ffmpeg.as_ref().is_some_and(|f| f.is_file()),
                external_server: self.layout.mode == Mode::SelfHosted
                    && std::env::var_os("WRITER_LOCAL_STT_URL").is_some(),
                models_dir: (self.layout.mode == Mode::SelfHosted)
                    .then(|| self.layout.models_dir.to_string_lossy().into_owned()),
            },
            models,
            cloud,
        }
    }
}

#[derive(Serialize)]
pub struct ModelView {
    pub smoke_tests: Option<&'static catalog::ModelEvidence>,
    pub metadata: Option<&'static catalog::CatalogModel>,
    pub compatibility_issue: Option<&'static str>,
    pub id: &'static str,
    pub display_name: &'static str,
    pub file_name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub source_url: &'static str,
    pub source_name: &'static str,
    pub license: &'static str,
    pub notes: &'static str,
    pub input_languages: Vec<InputLanguage>,
    pub weights: WeightsState,
    pub selected: bool,
    pub loaded: bool,
}
#[derive(Serialize)]
pub struct CloudView {
    pub provider: ProviderId,
    pub models: Vec<&'static str>,
    pub credential: CredentialStatus,
    /// The provider's answer to the last explicit key check in this process.
    /// Absent until one is run, and retired when the stored key changes.
    pub check: Option<CloudCheck>,
    /// A successful transcript in this process; a configured key alone is unverified.
    pub verified: bool,
    pub selected: bool,
}
#[derive(Serialize)]
pub struct RuntimeView {
    pub hardware: crate::local_stt::hardware::Hardware,
    pub execution: ExecutionView,
    pub worker: WorkerState,
    pub problem: Option<String>,
    pub packaged: bool,
    pub worker_executable_present: bool,
    pub vulkan_library_present: bool,
    pub ffmpeg_present: bool,
    pub external_server: bool,
    /// Self-hosted only: where imported weights must be placed before verification.
    pub models_dir: Option<String>,
}
#[derive(Serialize)]
pub struct SetupView {
    pub mode: Mode,
    pub first_run: bool,
    pub settings: Settings,
    pub runtime: RuntimeView,
    pub models: Vec<ModelView>,
    pub cloud: Vec<CloudView>,
}

type Verifier = Arc<dyn Fn(&ProviderId) -> bool + Send + Sync>;
#[derive(Clone)]
struct RouteState {
    setup: Option<Arc<Setup>>,
    verifier: Option<Verifier>,
}
impl RouteState {
    async fn resolve(&self) -> Result<(Arc<Setup>, Verifier), ApiError> {
        match (&self.setup, &self.verifier) {
            (Some(setup), Some(verifier)) => Ok((setup.clone(), verifier.clone())),
            _ => {
                let setup = global()?;
                let pipeline = crate::jobs::runtime().await?;
                let verifier: Verifier = Arc::new(move |provider: &ProviderId| {
                    pipeline.providers.iter().any(|p| {
                        let info = p.info();
                        std::mem::discriminant(&info.id) == std::mem::discriminant(provider)
                            && info.available
                    })
                });
                Ok((setup, verifier))
            }
        }
    }
}

fn fail(error: ApiError) -> Response {
    crate::jobs::response_error(error)
}

async fn get_settings(State(state): State<RouteState>) -> Response {
    match state.resolve().await {
        Ok((setup, _)) => Json(Envelope::new(setup.settings())).into_response(),
        Err(e) => fail(e),
    }
}
async fn put_settings(
    State(state): State<RouteState>,
    payload: Result<Json<Envelope<Settings>>, JsonRejection>,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected a version-one settings document.");
    };
    match state.resolve().await {
        Ok((setup, _)) => match setup.save(payload.data).await {
            Ok(saved) => Json(Envelope::new(saved)).into_response(),
            Err(e) => fail(e),
        },
        Err(e) => fail(e),
    }
}
async fn get_setup(State(state): State<RouteState>) -> Response {
    match state.resolve().await {
        Ok((setup, verifier)) => Json(Envelope::new(setup.view(&*verifier))).into_response(),
        Err(e) => fail(e),
    }
}
async fn model_action(
    State(state): State<RouteState>,
    AxumPath((model, action)): AxumPath<(String, String)>,
) -> Response {
    // Model ids are catalog aliases validated by the runtime; they never become paths here.
    if model.len() > 64
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return crate::jobs::invalid("Unknown local model.");
    }
    let (setup, verifier) = match state.resolve().await {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let result = match action.as_str() {
        "download" => setup.runtime.download(&model),
        "cancel" => setup.runtime.cancel_download(&model),
        "verify" => setup.runtime.verify(&model),
        "delete" => setup.runtime.delete_model(&model).await,
        _ => return crate::jobs::invalid("Unknown model action."),
    };
    match result {
        Ok(()) => Json(Envelope::new(setup.view(&*verifier))).into_response(),
        Err(e) => fail(e),
    }
}
async fn runtime_action(
    State(state): State<RouteState>,
    AxumPath(action): AxumPath<String>,
) -> Response {
    let (setup, verifier) = match state.resolve().await {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    match action.as_str() {
        "retry" => {
            // Reload the saved selection (a stop clears it) and reset the restart budget.
            let settings = setup.settings();
            let selected = &settings.transcription.selection;
            if let Err(error) = setup.runtime.retry(
                matches!(selected.provider, ProviderId::Local).then_some(selected.model.as_str()),
            ) {
                return fail(error);
            }
        }
        "stop" => {
            // Unload without changing the saved selection; the next retry/select reloads.
            if let Err(error) = setup.runtime.select(None) {
                return fail(error);
            }
        }
        _ => return crate::jobs::invalid("Unknown runtime action."),
    }
    Json(Envelope::new(setup.view(&*verifier))).into_response()
}

async fn execution_settings(
    State(state): State<RouteState>,
    payload: Result<Json<Envelope<ExecutionBackend>>, JsonRejection>,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Choose automatic, cpu or gpu execution.");
    };
    let (setup, verifier) = match state.resolve().await {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    match setup.runtime.save_backend(payload.data).await {
        Ok(()) => Json(Envelope::new(setup.view(&*verifier))).into_response(),
        Err(e) => fail(e),
    }
}

async fn cloud_test(
    State(state): State<RouteState>,
    AxumPath(provider): AxumPath<String>,
) -> Response {
    let Some(id) = cloud_check::provider_from(&provider) else {
        return crate::jobs::invalid("Unknown cloud provider.");
    };
    let (setup, verifier) = match state.resolve().await {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    match setup.check_cloud(&id).await {
        Ok(_) => Json(Envelope::new(setup.view(&*verifier))).into_response(),
        Err(e) => fail(e),
    }
}
fn router(state: RouteState) -> Router {
    Router::new()
        .route("/api/v1/settings", get(get_settings).put(put_settings))
        .route("/api/v1/setup", get(get_setup))
        .route(
            "/api/v1/setup/execution",
            axum::routing::put(execution_settings),
        )
        .route("/api/v1/setup/models/{model}/{action}", post(model_action))
        .route("/api/v1/setup/runtime/{action}", post(runtime_action))
        .route("/api/v1/setup/cloud/{provider}/test", post(cloud_test))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
}
pub fn routes() -> Router {
    router(RouteState {
        setup: None,
        verifier: None,
    })
}
pub fn routes_with(setup: Arc<Setup>, verifier: Verifier) -> Router {
    router(RouteState {
        setup: Some(setup),
        verifier: Some(verifier),
    })
}

#[cfg(all(test, unix))]
mod tests;
