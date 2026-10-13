//! Writer-owned hidden whisper.cpp worker, explicit weight downloads and verification.
//! Readiness verifies the selected CPU/Vulkan backend and exact weights identity.
//! Automatic fallback changes only the backend, never the model or provider.
mod auth;
pub mod engine;
pub use auth::WorkerAuth;
mod execution;
use super::catalog::{self, ModelSpec};
pub use execution::{CapturePin, ExecutionBackend, ExecutionView};

use crate::recordings::disk::{private_dir, read_json, write_json};
use crate::transcription::{ApiError, ErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

pub const WORKER_EXECUTABLE: &str = if cfg!(windows) {
    "whisper-server.exe"
} else {
    "whisper-server"
};
/// Presence of this library identifies a GPU build; readiness still requires the runtime log.
pub const VULKAN_BACKEND_LIBRARY: &str = if cfg!(target_os = "macos") {
    "libggml-metal.dylib"
} else {
    "ggml-vulkan.dll"
};
pub const GPU_BACKEND: &str = if cfg!(target_os = "macos") {
    "metal"
} else {
    "vulkan"
};
const MAX_RESTARTS: u32 = 3;
const STARTUP_LOG_LINES: usize = 64;
const LINE_LIMIT: usize = 4096;

#[derive(Clone, Debug)]
pub struct RuntimeLayout {
    /// Directory holding the worker executable and its DLLs (packaged or explicitly configured).
    pub runtime_dir: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub models_dir: PathBuf,
    pub state_dir: PathBuf,
    pub startup_timeout: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledWeights {
    pub file_name: String,
    pub bytes: u64,
    pub sha256: String,
    pub verified_at_ms: u64,
    pub origin: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    version: u32,
    models: BTreeMap<String, InstalledWeights>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WeightsState {
    NotInstalled {
        resumable_bytes: Option<u64>,
    },
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Verifying {
        bytes: u64,
    },
    Installed {
        bytes: u64,
        sha256: String,
        verified_at_ms: u64,
        origin: String,
    },
    Failed {
        error: String,
        resumable_bytes: Option<u64>,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkerState {
    Idle,
    MissingRuntime {
        detail: String,
    },
    MissingWeights {
        model: String,
    },
    Starting {
        model: String,
        since_ms: u64,
    },
    Ready {
        model: String,
        device: String,
        backend: String,
        started_at_ms: u64,
        restarts: u32,
        #[serde(skip)]
        port: u16,
        #[serde(skip)]
        process_id: u32,
    },
    Failed {
        model: String,
        error: String,
        restarts: u32,
    },
    Stopping {
        model: String,
    },
}

/// Loaded worker identity for the inference adapter; never serialized to clients.
#[derive(Clone, Debug)]
pub struct LoadedWorker {
    pub model: String,
    pub model_path: PathBuf,
    pub port: u16,
    pub auth: WorkerAuth,
}

struct Download {
    cancel: CancellationToken,
    progress: Arc<AtomicU64>,
}

struct Inner {
    registry: Registry,
    transient: BTreeMap<String, WeightsState>,
    downloads: BTreeMap<String, Download>,
    desired: Option<String>,
    worker: WorkerState,
    auth: Option<WorkerAuth>,
    restarts: u32,
    generation: u64,
    /// Generation that spawned the current `Starting` task; a stale one is respawned.
    start_generation: u64,
    stop: Option<Arc<Notify>>,
    log: VecDeque<String>,
    backend: ExecutionBackend,
    fallback: Option<String>,
}

pub struct ManagedRuntime {
    pub layout: RuntimeLayout,
    catalog: &'static [ModelSpec],
    inner: Mutex<Inner>,
    changed: Arc<Notify>,
    client: reqwest::Client,
    downloader: reqwest::Client,
    operations: Arc<tokio::sync::RwLock<()>>,
    worker_run: tokio::sync::Mutex<()>,
    registry_write: tokio::sync::Mutex<()>,
    captures: std::sync::atomic::AtomicUsize,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn storage(_: impl std::fmt::Display) -> ApiError {
    ApiError {
        code: ErrorCode::StorageFailed,
        message: "Model storage is unavailable; check free space and directory permissions.".into(),
        retryable: true,
        retry_after_ms: None,
    }
}
fn invalid(message: &str) -> ApiError {
    ApiError {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
        retryable: false,
        retry_after_ms: None,
    }
}

impl ManagedRuntime {
    /// Reads the installed-weights registry and resumable partial files. Starts nothing.
    pub fn open(layout: RuntimeLayout) -> Result<Arc<Self>, ApiError> {
        Self::open_with_catalog(layout, catalog::MODELS)
    }
    pub fn open_with_catalog(
        layout: RuntimeLayout,
        models: &'static [ModelSpec],
    ) -> Result<Arc<Self>, ApiError> {
        #[cfg(windows)]
        {
            // Helpers inherit this mode: a missing GPU DLL/driver must return an
            // error for CPU fallback rather than open a blocking Windows dialog.
            static ERROR_MODE: std::sync::Once = std::sync::Once::new();
            ERROR_MODE.call_once(|| unsafe {
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetErrorMode() -> u32;
                    fn SetErrorMode(mode: u32) -> u32;
                }
                SetErrorMode(GetErrorMode() | 0x0001 | 0x0002 | 0x8000);
            });
        }
        crate::helper_security::protect_private_dir(&layout.models_dir).map_err(storage)?;
        crate::helper_security::protect_private_dir(&layout.state_dir).map_err(storage)?;
        clear_legacy_diagnostics(&layout.state_dir.join("worker")).map_err(storage)?;
        let registry_path = layout.state_dir.join("models.json");
        let mut registry: Registry = match std::fs::read(&registry_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Registry::default(),
        };
        // A registry entry is only trusted while the exact file is still present and the same size.
        registry.models.retain(|id, installed| {
            models.iter().find(|m| m.id == id).is_some_and(|spec| {
                spec.file_name == installed.file_name
                    && spec.sha256 == installed.sha256
                    && std::fs::metadata(layout.models_dir.join(spec.file_name)).is_ok_and(|m| {
                        m.is_file() && m.len() == spec.bytes && m.len() == installed.bytes
                    })
            })
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(storage)?;
        let downloader = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.url().scheme() == "https" && attempt.previous().len() < 5 {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(storage)?;
        let backend = execution::read_backend(&layout.state_dir)?;
        let runtime = Arc::new(Self {
            layout,
            catalog: models,
            inner: Mutex::new(Inner {
                registry,
                transient: BTreeMap::new(),
                downloads: BTreeMap::new(),
                desired: None,
                worker: WorkerState::Idle,
                auth: None,
                restarts: 0,
                generation: 0,
                start_generation: 0,
                stop: None,
                log: VecDeque::new(),
                backend,
                fallback: None,
            }),
            changed: Arc::new(Notify::new()),
            client,
            downloader,
            operations: Arc::new(tokio::sync::RwLock::new(())),
            worker_run: tokio::sync::Mutex::new(()),
            registry_write: tokio::sync::Mutex::new(()),
            captures: std::sync::atomic::AtomicUsize::new(0),
        });
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&runtime);
            handle.spawn(async move {
                loop {
                    let Some(runtime) = weak.upgrade() else { break };
                    runtime.reconcile().await;
                    let changed = runtime.changed.clone();
                    drop(runtime);
                    let _ = tokio::time::timeout(Duration::from_secs(2), changed.notified()).await;
                }
            });
        }
        Ok(runtime)
    }

    pub fn catalog(&self) -> &'static [ModelSpec] {
        self.catalog
    }
    pub fn spec(&self, id: &str) -> Option<&'static ModelSpec> {
        self.catalog.iter().find(|model| model.id == id)
    }
    pub fn weights_path(&self, spec: &ModelSpec) -> PathBuf {
        self.layout.models_dir.join(spec.file_name)
    }
    fn partial_path(&self, spec: &ModelSpec) -> PathBuf {
        self.layout
            .models_dir
            .join(format!("{}.part", spec.file_name))
    }
    fn resumable_bytes(&self, spec: &ModelSpec) -> Option<u64> {
        std::fs::metadata(self.partial_path(spec))
            .ok()
            .filter(|m| m.is_file() && m.len() > 0)
            .map(|m| m.len())
    }

    pub fn weights(&self, id: &str) -> WeightsState {
        let Some(spec) = self.spec(id) else {
            return WeightsState::Failed {
                error: "unknown_model".into(),
                resumable_bytes: None,
            };
        };
        let inner = self.inner.lock().unwrap();
        if let Some(state) = inner.transient.get(id) {
            if let (WeightsState::Downloading { total_bytes, .. }, Some(download)) =
                (state, inner.downloads.get(id))
            {
                return WeightsState::Downloading {
                    downloaded_bytes: download.progress.load(Ordering::Relaxed),
                    total_bytes: *total_bytes,
                };
            }
            return state.clone();
        }
        match inner.registry.models.get(id) {
            Some(installed) => WeightsState::Installed {
                bytes: installed.bytes,
                sha256: installed.sha256.clone(),
                verified_at_ms: installed.verified_at_ms,
                origin: installed.origin.clone(),
            },
            None => WeightsState::NotInstalled {
                resumable_bytes: self.resumable_bytes(spec),
            },
        }
    }
    pub fn installed(&self, id: &str) -> Option<InstalledWeights> {
        let inner = self.inner.lock().unwrap();
        if inner.transient.contains_key(id) {
            return None;
        }
        inner.registry.models.get(id).cloned()
    }
    pub fn worker(&self) -> WorkerState {
        self.inner.lock().unwrap().worker.clone()
    }
    pub fn desired(&self) -> Option<String> {
        self.inner.lock().unwrap().desired.clone()
    }
    /// Fixed diagnostic codes only; helper text is untrusted and never retained.
    pub fn startup_log(&self) -> Vec<String> {
        self.inner.lock().unwrap().log.iter().cloned().collect()
    }
    pub fn loaded(&self) -> Option<LoadedWorker> {
        let inner = self.inner.lock().unwrap();
        match &inner.worker {
            WorkerState::Ready { model, port, .. }
                if inner.start_generation == inner.generation
                    && inner.desired.as_ref() == Some(model) =>
            {
                self.spec(model).map(|spec| LoadedWorker {
                    model: model.clone(),
                    model_path: self.weights_path(spec),
                    port: *port,
                    auth: inner
                        .auth
                        .clone()
                        .expect("ready worker has private identity"),
                })
            }
            _ => None,
        }
    }

    /// Runtime binaries check without starting anything.
    pub fn runtime_problem(&self) -> Option<String> {
        if self.layout.runtime_dir.is_none() {
            return Some("runtime_not_packaged".into());
        }
        if self.worker_choice().is_none() {
            return Some(
                match self.execution().requested {
                    ExecutionBackend::Cpu => "cpu_runtime_missing",
                    ExecutionBackend::Gpu => {
                        if cfg!(target_os = "macos") {
                            "metal_backend_missing"
                        } else {
                            "vulkan_backend_missing"
                        }
                    }
                    ExecutionBackend::Automatic => "compatible_runtime_missing",
                }
                .into(),
            );
        }
        match &self.layout.ffmpeg {
            Some(ffmpeg) if ffmpeg.is_file() => None,
            _ => Some("ffmpeg_missing".into()),
        }
    }

    /// Select which local model (if any) should be loaded. Persisting the choice is the caller's job.
    pub fn select(&self, model: Option<&str>) -> Result<(), ApiError> {
        self.maintenance()?.select(model)
    }
    fn select_unlocked(&self, model: Option<&str>) -> Result<(), ApiError> {
        if let Some(id) = model {
            self.spec(id)
                .ok_or_else(|| invalid("Unknown local model."))?;
        }
        let mut inner = self.inner.lock().unwrap();
        let next = model.map(str::to_owned);
        if inner.desired != next {
            inner.desired = next;
            inner.fallback = None;
            inner.restarts = 0;
            inner.generation += 1;
        }
        drop(inner);
        self.changed.notify_one();
        Ok(())
    }
    /// Explicit retry after failures: resets the automatic restart budget and starts again.
    pub fn retry(&self, model: Option<&str>) -> Result<(), ApiError> {
        let guard = self.maintenance()?;
        guard.select(model)?;
        let mut inner = self.inner.lock().unwrap();
        inner.restarts = 0;
        inner.fallback = None;
        inner.generation += 1;
        if matches!(inner.worker, WorkerState::Failed { .. }) {
            inner.worker = WorkerState::Idle;
        }
        drop(inner);
        self.changed.notify_one();
        Ok(())
    }

    fn set_worker(&self, state: WorkerState) {
        self.inner.lock().unwrap().worker = state;
        self.changed.notify_one();
    }
    fn set_transient(&self, id: &str, state: Option<WeightsState>) {
        let mut inner = self.inner.lock().unwrap();
        match state {
            Some(state) => {
                inner.transient.insert(id.to_owned(), state);
            }
            None => {
                inner.transient.remove(id);
            }
        }
        drop(inner);
        self.changed.notify_one();
    }

    async fn reconcile(self: &Arc<Self>) {
        let Ok(_operation) = self.operations.try_write() else {
            return;
        };
        let (desired, worker, restarts, generation) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.desired.clone(),
                inner.worker.clone(),
                inner.restarts,
                inner.generation,
            )
        };
        let Some(model) = desired else {
            if !matches!(worker, WorkerState::Idle) {
                self.stop_worker().await;
                self.set_worker(WorkerState::Idle);
            }
            return;
        };
        let loaded_other = match &worker {
            WorkerState::Ready { model: m, .. } | WorkerState::Starting { model: m, .. } => {
                m != &model
            }
            _ => false,
        };
        let stale_ready = matches!(worker, WorkerState::Ready { .. })
            && self.inner.lock().unwrap().start_generation != generation;
        if loaded_other || stale_ready {
            self.stop_worker().await;
        }
        if let Some(detail) = self.runtime_problem() {
            if worker
                != (WorkerState::MissingRuntime {
                    detail: detail.clone(),
                })
            {
                self.set_worker(WorkerState::MissingRuntime { detail });
            }
            return;
        }
        if self.installed(&model).is_none() {
            let next = WorkerState::MissingWeights {
                model: model.clone(),
            };
            if worker != next {
                self.set_worker(next);
            }
            return;
        }
        let start_generation = self.inner.lock().unwrap().start_generation;
        match &worker {
            WorkerState::Ready { model: m, .. }
                if m == &model && start_generation == generation =>
            {
                return
            }
            WorkerState::Starting { model: m, .. }
                if m == &model && start_generation == generation =>
            {
                return
            }
            WorkerState::Failed { model: m, .. } if m == &model && restarts >= MAX_RESTARTS => {
                return
            }
            WorkerState::Stopping { .. } => return,
            _ => {}
        }
        // Stopping another model bumped the generation; spawn against the current one.
        let generation = {
            let mut inner = self.inner.lock().unwrap();
            inner.start_generation = inner.generation;
            inner.worker = WorkerState::Starting {
                model: model.clone(),
                since_ms: now_ms(),
            };
            inner.generation
        };
        self.changed.notify_one();
        let runtime = self.clone();
        tokio::spawn(async move {
            runtime.start_worker(model, generation).await;
        });
    }

    async fn stop_worker(&self) {
        let (stop, model) = {
            let mut inner = self.inner.lock().unwrap();
            let model = match &inner.worker {
                WorkerState::Ready { model, .. } | WorkerState::Starting { model, .. } => {
                    Some(model.clone())
                }
                _ => None,
            };
            inner.generation += 1;
            inner.auth = None;
            (inner.stop.take(), model)
        };
        if let Some(model) = model {
            self.set_worker(WorkerState::Stopping { model });
        }
        if let Some(stop) = stop {
            stop.notify_one();
        }
        // Wait for the owned process task itself, including startup cancellation.
        // Do not publish Idle while a stop operation still owns the runtime gate.
        if self.wait_stopped().await.is_err() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if matches!(
            inner.worker,
            WorkerState::Stopping { .. } | WorkerState::Starting { .. }
        ) {
            inner.worker = WorkerState::Idle;
        }
    }

    fn generation_is(&self, generation: u64) -> bool {
        self.inner.lock().unwrap().generation == generation
    }

    fn fail_worker(&self, model: &str, error: &str, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return;
        }
        inner.auth = None;
        if inner.backend == ExecutionBackend::Automatic
            && self.gpu_dir(Some(model)).is_some()
            && inner.fallback.is_none()
            && self.cpu_dir(Some(model)).is_some()
            && matches!(
                error,
                "gpu_unavailable"
                    | "gpu_backend_not_used"
                    | "worker_start_failed"
                    | "worker_exited_during_startup"
                    | "worker_start_timeout"
                    | "worker_exited"
            )
        {
            inner.fallback = Some(error.into());
            inner.generation += 1;
            inner.restarts = 0;
            inner.worker = WorkerState::Idle;
            inner.stop = None;
            drop(inner);
            self.changed.notify_one();
            return;
        }
        inner.restarts += 1;
        inner.worker = WorkerState::Failed {
            model: model.to_owned(),
            error: error.to_owned(),
            restarts: inner.restarts,
        };
        inner.stop = None;
        drop(inner);
        self.changed.notify_one();
    }

    fn push_log(&self, line: String) {
        let mut inner = self.inner.lock().unwrap();
        if inner.log.contains(&line) {
            return;
        }
        if inner.log.len() >= STARTUP_LOG_LINES {
            inner.log.pop_front();
        }
        inner.log.push_back(line);
    }

    async fn start_worker(self: Arc<Self>, model: String, generation: u64) {
        let _running = self.worker_run.lock().await;
        if !self.generation_is(generation) {
            return;
        }
        let Some(spec) = self.spec(&model) else {
            return;
        };
        let Some((runtime_dir, backend)) = self.worker_choice() else {
            return;
        };
        let model_path = self.weights_path(spec);
        let Some(installed) = self.installed(&model) else {
            self.fail_worker(&model, "weights_missing", generation);
            return;
        };
        match std::fs::metadata(&model_path) {
            Ok(m) if m.is_file() && m.len() == installed.bytes => {}
            _ => {
                self.fail_worker(&model, "weights_changed_since_verification", generation);
                return;
            }
        }
        let auth = WorkerAuth::new();
        let work_dir = self.layout.state_dir.join("worker");
        if clear_legacy_diagnostics(&work_dir).is_err() {
            self.fail_worker(&model, "worker_diagnostic_storage_unavailable", generation);
            return;
        }
        // Keep CPU orchestration small for checkpointed, interactive Vulkan work.
        // On the RX 7800 XT, 1 thread measured 155ms
        // versus 563ms with 8, with identical transcripts. CPU inference still
        // needs its parallel compute pool; NeMo supplies its own flags below.
        let threads = if backend == ExecutionBackend::Cpu {
            std::thread::available_parallelism()
                .map(|n| n.get().min(16))
                .unwrap_or(4)
        } else {
            1
        };
        // whisper.cpp opens the model through the ANSI code page, so a Unicode profile path
        // (Hebrew user names, the test fixtures) fails. The catalog file name is ASCII and is
        // resolved against the models directory set as the Unicode working directory.
        let nemo = engine::nemo(spec.id);
        let mut command =
            super::process::owned_command(&runtime_dir.join(engine::executable(spec.id)));
        if nemo {
            command
                .args([
                    "serve",
                    "--asr-model",
                    spec.file_name,
                    "--host",
                    "127.0.0.1",
                    "--port",
                ])
                .arg("0")
                .args([
                    "--threads",
                    "2",
                    "--no-ui",
                    "--backend",
                    if backend == ExecutionBackend::Cpu {
                        "cpu"
                    } else {
                        GPU_BACKEND
                    },
                ]);
        } else {
            command
                .args([
                    "-m",
                    spec.file_name,
                    "-l",
                    spec.startup_language,
                    "--host",
                    "127.0.0.1",
                    "--port",
                ])
                .arg("0")
                .arg("-t")
                .arg(threads.to_string());
        }
        command
            .current_dir(&self.layout.models_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if backend == ExecutionBackend::Cpu && !nemo {
            command.arg("--no-gpu");
        }
        #[cfg(windows)]
        {
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: never a console flash.
        }
        {
            self.inner.lock().unwrap().log.clear();
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                self.fail_worker(&model, "worker_start_failed", generation);
                return;
            }
        };
        let process_id = child.id().unwrap_or(0);
        #[cfg(any(windows, target_os = "macos"))]
        let _job = match super::worker_job::WorkerJob::attach(process_id) {
            Some(job) => job,
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                self.fail_worker(&model, "worker_ownership_failed", generation);
                return;
            }
        };
        if auth
            .send(&mut child.stdin.take().expect("piped stdin"))
            .await
            .is_err()
        {
            let _ = child.kill().await;
            self.fail_worker(&model, "worker_handshake_failed", generation);
            return;
        }
        let markers = Arc::new(Mutex::new(Markers::default()));
        let stderr = child.stderr.take().expect("piped stderr");
        let reader_markers = markers.clone();
        let reader_runtime = self.clone();
        let expected_path = spec.file_name.to_owned();
        let log_auth = auth.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr);
            let mut buffer = Vec::new();
            loop {
                buffer.clear();
                match (&mut lines)
                    .take(LINE_LIMIT as u64 + 1)
                    .read_until(b'\n', &mut buffer)
                    .await
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if buffer.len() > LINE_LIMIT {
                    buffer.truncate(LINE_LIMIT);
                }
                let line = log_auth.redact(&String::from_utf8_lossy(&buffer));
                if let Some(port) = line
                    .strip_prefix("writer: private listener ")
                    .and_then(|value| value.parse::<u16>().ok())
                    .filter(|port| *port != 0)
                {
                    reader_markers.lock().unwrap().port = Some(port);
                    continue;
                }
                if nemo {
                    reader_markers.lock().unwrap().observe_nemo(&line);
                } else {
                    reader_markers
                        .lock()
                        .unwrap()
                        .observe(&line, &expected_path);
                }
                let markers = reader_markers.lock().unwrap();
                for (observed, code) in [
                    (markers.cpu, "cpu_backend_observed"),
                    (markers.gpu, "gpu_backend_observed"),
                    (markers.no_gpu, "gpu_unavailable"),
                    (markers.model_path_matched, "expected_model_observed"),
                ] {
                    if observed {
                        reader_runtime.push_log(code.to_owned());
                    }
                }
            }
        });
        let deadline = tokio::time::Instant::now() + self.layout.startup_timeout;
        let outcome = loop {
            if !self.generation_is(generation) {
                break Err("worker_start_canceled");
            }
            if let Ok(Some(_)) = child.try_wait() {
                break Err("worker_exited_during_startup");
            }
            {
                let m = markers.lock().unwrap();
                if backend == ExecutionBackend::Gpu && m.no_gpu {
                    break Err("gpu_unavailable");
                }
            }
            if tokio::time::Instant::now() > deadline {
                break Err("worker_start_timeout");
            }
            let port = markers.lock().unwrap().port;
            let Some(port) = port else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            let health_url = format!(
                "http://127.0.0.1:{port}/{}",
                if nemo { "ready" } else { "health" }
            );
            if let Ok(response) = auth.authorize(self.client.get(&health_url)).send().await {
                if !auth.matches(&response) {
                    break Err("worker_identity_mismatch");
                }
                let success = response.status().is_success();
                if let Ok(body) = super::bounded_response(response, 4096).await {
                    let ready = if nemo {
                        serde_json::from_slice::<serde_json::Value>(&body)
                            .is_ok_and(|v| v["ready"] == true)
                    } else {
                        serde_json::from_slice::<serde_json::Value>(&body).is_ok_and(|v| {
                            v["status"] == "ok" && v["writer_protocol"] == 1 && v["warm"] == true
                        })
                    };
                    if success && ready {
                        // The Vulkan line precedes model load; absence after health means CPU use.
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let m = markers.lock().unwrap();
                        if backend == ExecutionBackend::Gpu && m.no_gpu {
                            break Err("gpu_unavailable");
                        }
                        if backend == ExecutionBackend::Gpu && !m.gpu {
                            break Err("gpu_backend_not_used");
                        }
                        if backend == ExecutionBackend::Cpu && (m.gpu || !m.cpu) {
                            break Err("cpu_backend_not_used");
                        }
                        if !m.model_path_matched {
                            break Err("loaded_model_mismatch");
                        }
                        break Ok(if backend == ExecutionBackend::Cpu {
                            "CPU".into()
                        } else {
                            format!("{} GPU", GPU_BACKEND)
                        });
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        let device = match outcome {
            Ok(device) => device,
            Err(error) => {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                if error != "worker_start_canceled" {
                    self.fail_worker(&model, error, generation);
                }
                return;
            }
        };
        if !matches!(child.try_wait(), Ok(None)) {
            self.fail_worker(&model, "worker_exited_during_startup", generation);
            return;
        }
        let port = markers
            .lock()
            .unwrap()
            .port
            .expect("authenticated listener");
        let stop = Arc::new(Notify::new());
        let stale = {
            let mut inner = self.inner.lock().unwrap();
            let stale = inner.generation != generation;
            if !stale {
                inner.stop = Some(stop.clone());
                inner.auth = Some(auth);
                inner.worker = WorkerState::Ready {
                    model: model.clone(),
                    device,
                    backend: if backend == ExecutionBackend::Cpu {
                        "cpu"
                    } else {
                        GPU_BACKEND
                    }
                    .into(),
                    started_at_ms: now_ms(),
                    restarts: inner.restarts,
                    port,
                    process_id,
                };
            }
            stale
        };
        if stale {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            return;
        }
        self.changed.notify_one();
        let exited = tokio::select! {
            _ = stop.notified() => false,
            _ = child.wait() => true,
        };
        if !exited {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            let mut inner = self.inner.lock().unwrap();
            inner.stop = None;
            drop(inner);
            self.changed.notify_one();
        } else {
            self.fail_worker(&model, "worker_exited", generation);
        }
    }

    /// Verify weights already present at the fixed models-directory path (import or re-check).
    pub fn verify(self: &Arc<Self>, id: &str) -> Result<(), ApiError> {
        let spec = self
            .spec(id)
            .ok_or_else(|| invalid("Unknown local model."))?;
        {
            let inner = self.inner.lock().unwrap();
            if matches!(
                inner.transient.get(id),
                Some(WeightsState::Downloading { .. } | WeightsState::Verifying { .. })
            ) {
                return Ok(());
            }
        }
        let lease = self
            .operations
            .clone()
            .try_write_owned()
            .map_err(|_| execution::busy())?;
        let path = self.weights_path(spec);
        let size = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len());
        let Some(size) = size else {
            self.set_transient(
                id,
                Some(WeightsState::Failed {
                    error: "weights_file_missing".into(),
                    resumable_bytes: self.resumable_bytes(spec),
                }),
            );
            return Ok(());
        };
        self.set_transient(id, Some(WeightsState::Verifying { bytes: size }));
        let runtime = self.clone();
        tokio::spawn(async move {
            let _lease = lease;
            runtime.stop_worker().await;
            if runtime.wait_stopped().await.is_err() {
                runtime.set_transient(
                    spec.id,
                    Some(WeightsState::Failed {
                        error: "worker_stop_timeout".into(),
                        resumable_bytes: None,
                    }),
                );
                return;
            }
            runtime.verify_file(spec, "import").await;
        });
        Ok(())
    }

    async fn verify_file(self: Arc<Self>, spec: &'static ModelSpec, origin: &str) {
        let path = self.weights_path(spec);
        let result = tokio::task::spawn_blocking(move || hash_file(&path)).await;
        let outcome = match result {
            Ok(Ok((bytes, _))) if bytes != spec.bytes => Err("size_mismatch"),
            Ok(Ok((_, sha256))) if sha256 != spec.sha256 => Err("checksum_mismatch"),
            Ok(Ok((bytes, sha256))) => Ok((bytes, sha256)),
            _ => Err("weights_unreadable"),
        };
        let _registry = self.registry_write.lock().await;
        match outcome {
            Ok((bytes, sha256)) => {
                let installed = InstalledWeights {
                    file_name: spec.file_name.into(),
                    bytes,
                    sha256,
                    verified_at_ms: now_ms(),
                    origin: origin.into(),
                };
                // Persist before publishing: an "installed" state always survives a restart.
                let snapshot = {
                    let inner = self.inner.lock().unwrap();
                    let mut models = inner.registry.models.clone();
                    models.insert(spec.id.into(), installed.clone());
                    Registry { version: 1, models }
                };
                let path = self.layout.state_dir.join("models.json");
                let persisted = match private_dir(&self.layout.state_dir).await {
                    Ok(()) => write_json(&path, &snapshot).await.is_ok(),
                    Err(_) => false,
                };
                if !persisted {
                    self.set_transient(
                        spec.id,
                        Some(WeightsState::Failed {
                            error: "registry_unwritable".into(),
                            resumable_bytes: None,
                        }),
                    );
                    return;
                }
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.registry = snapshot;
                    inner.transient.remove(spec.id);
                }
                self.changed.notify_one();
            }
            Err(error) => {
                // Never leave corrupt bytes at the loadable path.
                let _ = tokio::fs::remove_file(self.weights_path(spec)).await;
                {
                    self.inner.lock().unwrap().registry.models.remove(spec.id);
                }
                self.set_transient(
                    spec.id,
                    Some(WeightsState::Failed {
                        error: error.into(),
                        resumable_bytes: None,
                    }),
                );
            }
        }
    }

    /// Explicit named download with resume; the caller has shown size, source and checksum.
    pub fn download(self: &Arc<Self>, id: &str) -> Result<(), ApiError> {
        let spec = self
            .spec(id)
            .ok_or_else(|| invalid("Unknown local model."))?;
        if self.installed(id).is_some() {
            return Ok(());
        }
        // A file already present at the fixed path (an import) is verified instead of fetched.
        if self.weights_path(spec).is_file() {
            return self.verify(id);
        }
        let lease = self
            .operations
            .clone()
            .try_read_owned()
            .map_err(|_| execution::busy())?;
        let (cancel, progress) = {
            let mut inner = self.inner.lock().unwrap();
            if matches!(
                inner.transient.get(id),
                Some(WeightsState::Downloading { .. } | WeightsState::Verifying { .. })
            ) {
                return Ok(());
            }
            let cancel = CancellationToken::new();
            let progress = Arc::new(AtomicU64::new(self.resumable_bytes(spec).unwrap_or(0)));
            inner.downloads.insert(
                id.into(),
                Download {
                    cancel: cancel.clone(),
                    progress: progress.clone(),
                },
            );
            inner.transient.insert(
                id.into(),
                WeightsState::Downloading {
                    downloaded_bytes: progress.load(Ordering::Relaxed),
                    total_bytes: spec.bytes,
                },
            );
            (cancel, progress)
        };
        self.changed.notify_one();
        let runtime = self.clone();
        tokio::spawn(async move {
            let _lease = lease;
            let result = runtime.fetch(spec, cancel.clone(), progress.clone()).await;
            {
                runtime.inner.lock().unwrap().downloads.remove(spec.id);
            }
            match result {
                Ok(()) => {
                    runtime.set_transient(
                        spec.id,
                        Some(WeightsState::Verifying { bytes: spec.bytes }),
                    );
                    runtime.verify_file(spec, "download").await;
                }
                Err(error) => {
                    let resumable = runtime.resumable_bytes(spec);
                    runtime.set_transient(
                        spec.id,
                        Some(WeightsState::Failed {
                            error: error.into(),
                            resumable_bytes: resumable,
                        }),
                    );
                }
            }
        });
        Ok(())
    }

    pub fn cancel_download(&self, id: &str) -> Result<(), ApiError> {
        self.spec(id)
            .ok_or_else(|| invalid("Unknown local model."))?;
        let inner = self.inner.lock().unwrap();
        if let Some(download) = inner.downloads.get(id) {
            download.cancel.cancel();
        }
        Ok(())
    }

    async fn fetch(
        &self,
        spec: &'static ModelSpec,
        cancel: CancellationToken,
        progress: Arc<AtomicU64>,
    ) -> Result<(), &'static str> {
        let partial = self.partial_path(spec);
        let existing = self.resumable_bytes(spec).unwrap_or(0);
        if existing > spec.bytes {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err("partial_file_invalid");
        }
        let url = reqwest::Url::parse(spec.source_url).map_err(|_| "source_invalid")?;
        // Public sources must be HTTPS; plain HTTP is accepted only from this machine's loopback.
        let loopback = url
            .host_str()
            .is_some_and(|h| h == "127.0.0.1" || h == "localhost" || h == "[::1]");
        if !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
            return Err("source_invalid");
        }
        let mut request = self.downloader.get(url);
        if existing > 0 {
            request = request.header("Range", format!("bytes={existing}-"));
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err("download_canceled"),
            response = request.send() => response.map_err(|_| "source_unreachable")?,
        };
        let status = response.status();
        let mut offset = existing;
        let mut file = tokio::fs::OpenOptions::new();
        file.create(true).write(true);
        let mut file = match status.as_u16() {
            206 if existing > 0 => file
                .append(true)
                .open(&partial)
                .await
                .map_err(|_| "storage_failed")?,
            200 => {
                offset = 0;
                progress.store(0, Ordering::Relaxed);
                file.truncate(true)
                    .open(&partial)
                    .await
                    .map_err(|_| "storage_failed")?
            }
            416 if existing == spec.bytes => {
                return self.promote_partial(spec).await;
            }
            _ => return Err("source_rejected"),
        };
        if let Some(length) = response.content_length() {
            if offset + length != spec.bytes {
                return Err("size_mismatch");
            }
        }
        let mut stream = response;
        let result = loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => break Err("download_canceled"),
                chunk = tokio::time::timeout(Duration::from_secs(60), stream.chunk()) => match chunk {
                    Ok(Ok(Some(chunk))) => chunk,
                    Ok(Ok(None)) => break Ok(()),
                    Ok(Err(_)) => break Err("download_interrupted"),
                    Err(_) => break Err("download_stalled"),
                },
            };
            offset += chunk.len() as u64;
            if offset > spec.bytes {
                break Err("size_mismatch");
            }
            if file.write_all(&chunk).await.is_err() {
                break Err("storage_failed");
            }
            progress.store(offset, Ordering::Relaxed);
        };
        // Wait for in-flight writes so reported resumable bytes match the file.
        let synced = file.sync_all().await.is_ok();
        drop(file);
        match result {
            Err("size_mismatch") => {
                let _ = tokio::fs::remove_file(&partial).await;
                progress.store(0, Ordering::Relaxed);
                return Err("size_mismatch");
            }
            Err(error) => return Err(error),
            Ok(()) if !synced => return Err("storage_failed"),
            Ok(()) => {}
        }
        if offset != spec.bytes {
            return Err("download_incomplete");
        }
        self.promote_partial(spec).await
    }

    async fn promote_partial(&self, spec: &ModelSpec) -> Result<(), &'static str> {
        tokio::fs::rename(self.partial_path(spec), self.weights_path(spec))
            .await
            .map_err(|_| "storage_failed")
    }
}

fn clear_legacy_diagnostics(work_dir: &Path) -> std::io::Result<()> {
    crate::helper_security::protect_private_dir(work_dir)?;
    // Unlink, never read/follow. Run on open even if no worker can start (missing
    // runtime/weights), and fail closed if a legacy diagnostic cannot be erased.
    match std::fs::remove_file(work_dir.join("startup.log")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Default)]
struct Markers {
    port: Option<u16>,
    gpu: bool,
    cpu: bool,
    no_gpu: bool,
    model_path_matched: bool,
}
impl Markers {
    fn observe(&mut self, line: &str, expected_path: &str) {
        if line.contains("whisper_init_with_params_no_state: use gpu = 0")
            || line.contains("whisper_init_with_params_no_state: use gpu    = 0")
        {
            self.cpu = true;
        }
        if line.contains("whisper_backend_init_gpu: using Vulkan")
            || (cfg!(target_os = "macos") && line.contains("whisper_backend_init_gpu: using MTL"))
        {
            self.gpu = true;
        }
        if line.contains("Found 0 Vulkan devices")
            || line.contains("whisper_backend_init_gpu: no GPU found")
        {
            self.no_gpu = true;
        }
        if let Some(index) = line.find("loading model from '") {
            let rest = &line[index + "loading model from '".len()..];
            let path = rest.trim_end_matches('\'');
            if path == expected_path {
                self.model_path_matched = true;
            }
        }
    }
}

pub fn hash_file(path: &Path) -> std::io::Result<(u64, String)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

/// Registry file read used by tests and diagnostics.
pub async fn read_registry(state_dir: &Path) -> Option<BTreeMap<String, InstalledWeights>> {
    read_json::<Registry>(&state_dir.join("models.json"))
        .await
        .ok()
        .map(|r| r.models)
}
