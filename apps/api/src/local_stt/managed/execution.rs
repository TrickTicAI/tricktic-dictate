use super::*;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    #[default]
    Automatic,
    Cpu,
    Gpu,
}

#[derive(Serialize)]
pub struct ExecutionView {
    pub requested: ExecutionBackend,
    pub cpu_available: bool,
    pub gpu_available: bool,
    pub fallback_reason: Option<String>,
}

pub(super) fn read_backend(root: &Path) -> Result<ExecutionBackend, ApiError> {
    match std::fs::read(root.join("execution.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(storage),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ExecutionBackend::default()),
        Err(e) => Err(storage(e)),
    }
}

pub(super) fn busy() -> ApiError {
    ApiError {
        code: ErrorCode::IdempotencyConflict,
        message: "Wait for active inference, downloads or verification to finish before changing models. Cancel a download first if you want to delete it.".into(),
        retryable: true,
        retry_after_ms: None,
    }
}

/// Serializes persistent settings changes with inference and file operations.
pub struct Maintenance<'a> {
    runtime: &'a ManagedRuntime,
    _guard: tokio::sync::RwLockWriteGuard<'a, ()>,
}
/// Recording never waits on model execution/download locks. Already-started
/// maintenance may finish; subsequent destructive changes see the active pin.
pub struct CapturePin {
    runtime: Arc<ManagedRuntime>,
}
impl Drop for CapturePin {
    fn drop(&mut self) {
        self.runtime
            .captures
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
impl Maintenance<'_> {
    pub fn select(&self, model: Option<&str>) -> Result<(), ApiError> {
        self.runtime.select_unlocked(model)
    }
}

impl ManagedRuntime {
    pub fn capture_pin(self: &Arc<Self>) -> CapturePin {
        self.captures
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        CapturePin {
            runtime: self.clone(),
        }
    }
    pub fn recording_active(&self) -> bool {
        self.captures.load(std::sync::atomic::Ordering::Acquire) != 0
    }
    /// Copy an explicit native import under the same gate as capture/inference,
    /// downloads and deletion. Publish only fully hashed bytes; never alter source.
    pub async fn import_file(self: &Arc<Self>, id: &str, source: PathBuf) -> Result<(), ApiError> {
        let spec = self
            .spec(id)
            .ok_or_else(|| invalid("Unknown local model."))?;
        let _maintenance = self.maintenance()?;
        let target = self.weights_path(spec);
        if target.exists() {
            return Err(invalid(
                "Model weights already exist. Verify or delete the managed copy first.",
            ));
        }
        self.stop_worker().await;
        self.wait_stopped().await?;
        private_dir(&self.layout.models_dir).await?;
        let temporary = target.with_extension(format!("{}.import", uuid::Uuid::new_v4()));
        struct Partial(PathBuf);
        impl Drop for Partial {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _partial = Partial(temporary.clone());
        tokio::fs::copy(source, &temporary).await.map_err(storage)?;
        let copy = temporary.clone();
        let (bytes, hash) = tokio::task::spawn_blocking(move || hash_file(&copy))
            .await
            .map_err(storage)?
            .map_err(storage)?;
        if bytes != spec.bytes || hash != spec.sha256 {
            return Err(invalid(
                "Imported file size or SHA-256 does not match this model.",
            ));
        }
        tokio::fs::hard_link(&temporary, &target)
            .await
            .map_err(storage)?;
        self.clone().verify_file(spec, "import").await;
        if self.installed(id).is_none() {
            return Err(invalid(
                "Import verification could not be persisted. Retry Verify.",
            ));
        }
        Ok(())
    }
    /// The one gate every change passes, whether or not it needs the lock below.
    pub fn guard_recording(&self) -> Result<(), ApiError> {
        if self.recording_active() {
            return Err(invalid(
                "Finish recording before changing, importing or deleting models.",
            ));
        }
        Ok(())
    }
    pub fn maintenance(&self) -> Result<Maintenance<'_>, ApiError> {
        self.guard_recording()?;
        Ok(Maintenance {
            runtime: self,
            _guard: self.operations.try_write().map_err(|_| busy())?,
        })
    }

    pub fn inference_lease(&self) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, ApiError> {
        self.operations.clone().try_read_owned().map_err(|_| busy())
    }

    // A CPU-only package lives in runtime/cpu, or is the explicitly configured
    // runtime itself. Never load the Vulkan DLL as a CPU fallback dependency.
    pub(super) fn cpu_dir(&self, model: Option<&str>) -> Option<PathBuf> {
        let model = model.unwrap_or("");
        if !engine::supported(model) {
            return None;
        }
        let root = self.layout.runtime_dir.as_ref()?;
        engine::directories(root, model, true)
            .into_iter()
            .find(|dir| {
                dir.join(engine::executable(model)).is_file()
                    && !engine::gpu_library(dir, model).exists()
                    && (engine::nemo(model)
                        || !cfg!(windows)
                        || dir.join("ggml-cpu-x64.dll").is_file())
            })
    }
    pub(super) fn gpu_dir(&self, model: Option<&str>) -> Option<PathBuf> {
        let model = model.unwrap_or("");
        if !engine::supported(model) {
            return None;
        }
        let root = self.layout.runtime_dir.as_ref()?;
        engine::directories(root, model, false)
            .into_iter()
            .find(|dir| {
                dir.join(engine::executable(model)).is_file()
                    && engine::gpu_library(dir, model).is_file()
            })
    }

    pub fn execution(&self) -> ExecutionView {
        let inner = self.inner.lock().unwrap();
        ExecutionView {
            requested: inner.backend,
            cpu_available: self.cpu_dir(inner.desired.as_deref()).is_some(),
            gpu_available: self.gpu_dir(inner.desired.as_deref()).is_some(),
            fallback_reason: inner.fallback.clone().or_else(|| {
                (inner.backend == ExecutionBackend::Automatic
                    && self.gpu_dir(inner.desired.as_deref()).is_none()
                    && self.cpu_dir(inner.desired.as_deref()).is_some())
                .then(|| {
                    if cfg!(target_os = "macos") {
                        "metal_runtime_missing".into()
                    } else {
                        "vulkan_runtime_missing".into()
                    }
                })
            }),
        }
    }

    pub(super) fn worker_choice(&self) -> Option<(PathBuf, ExecutionBackend)> {
        let inner = self.inner.lock().unwrap();
        match inner.backend {
            ExecutionBackend::Cpu => self
                .cpu_dir(inner.desired.as_deref())
                .map(|d| (d, ExecutionBackend::Cpu)),
            ExecutionBackend::Gpu => self
                .gpu_dir(inner.desired.as_deref())
                .map(|d| (d, ExecutionBackend::Gpu)),
            ExecutionBackend::Automatic => {
                if inner.fallback.is_none() {
                    if let Some(dir) = self.gpu_dir(inner.desired.as_deref()) {
                        return Some((dir, ExecutionBackend::Gpu));
                    }
                }
                self.cpu_dir(inner.desired.as_deref())
                    .map(|d| (d, ExecutionBackend::Cpu))
            }
        }
    }

    /// A failed GPU request remains a failed saved-audio attempt. After its lease
    /// drains, Automatic prepares the same weights on CPU for an explicit retry.
    pub fn inference_failed(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.backend == ExecutionBackend::Automatic
            && inner.fallback.is_none()
            && self.cpu_dir(inner.desired.as_deref()).is_some()
            && matches!(&inner.worker, WorkerState::Ready { backend, .. } if backend == GPU_BACKEND)
        {
            inner.fallback = Some("gpu_inference_failed".into());
            inner.generation += 1;
            inner.restarts = 0;
            drop(inner);
            self.changed.notify_one();
        }
    }

    pub async fn save_backend(&self, backend: ExecutionBackend) -> Result<(), ApiError> {
        let _maintenance = self.maintenance()?;
        write_json(&self.layout.state_dir.join("execution.json"), &backend).await?;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.backend = backend;
            inner.fallback = None;
            inner.restarts = 0;
            inner.generation += 1;
        }
        self.changed.notify_one();
        Ok(())
    }

    pub(super) async fn wait_stopped(&self) -> Result<(), ApiError> {
        let _stopped = tokio::time::timeout(Duration::from_secs(10), self.worker_run.lock())
            .await
            .map_err(|_| busy())?;
        Ok(())
    }

    /// Deletes only managed copies and resumable bytes. Import sources, model
    /// selection, audio and transcripts are retained. Busy work must finish first.
    pub async fn delete_model(&self, id: &str) -> Result<(), ApiError> {
        let spec = self
            .spec(id)
            .ok_or_else(|| invalid("Unknown local model."))?;
        let _maintenance = self.maintenance()?;
        self.stop_worker().await;
        self.wait_stopped().await?;
        let _registry = self.registry_write.lock().await;
        let snapshot = {
            let inner = self.inner.lock().unwrap();
            let mut models = inner.registry.models.clone();
            models.remove(id);
            Registry { version: 1, models }
        };
        // Revoke readiness durably first. A failed removal remains retryable and
        // cannot resurrect an installed entry after a crash.
        write_json(&self.layout.state_dir.join("models.json"), &snapshot).await?;
        self.inner.lock().unwrap().registry = snapshot;
        for path in [self.weights_path(spec), self.partial_path(spec)] {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(storage(e)),
            }
        }
        self.inner.lock().unwrap().transient.remove(id);
        self.changed.notify_one();
        Ok(())
    }
}
