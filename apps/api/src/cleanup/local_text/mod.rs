//! Managed local text weights reuse the speech library's verified transfer and
//! deletion gates, while execution has its own packaged runtime and model list.
mod runtime;
#[cfg(test)]
pub(crate) use runtime::output_schema;
#[cfg(all(test, any(target_os = "macos", windows)))]
mod native_tests;
#[cfg(all(test, unix))]
mod tests;
use crate::{
    local_stt::{
        catalog::ModelSpec,
        managed::{ExecutionBackend, ManagedRuntime, RuntimeLayout},
    },
    transcription::*,
};
use axum::{
    extract::Path,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
/// Text models for Super and Command, run on this computer.
///
/// Each entry names who published the weights, because that is a real part of
/// the choice and cannot be read off a file name. Both run under the same
/// 8192-token runtime context; neither is a quality claim.
pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "qwen3-4b-instruct-2507-q4", display_name: "Qwen3 4B Instruct 2507 (Q4_K_M)",
        file_name: "qwen3-4b-instruct-q4.gguf", bytes: 2_497_280_448,
        sha256: "8cdb57cbb880d313736a9bc4e3d3d2485f145b5e19cf33783746e753e82641fc",
        source_url: "https://huggingface.co/lmstudio-community/Qwen3-4B-Instruct-2507-GGUF/resolve/4edb920b6f14e3b9284d4502a6485103d72cde05/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        source_name: "lmstudio-community/Qwen3-4B-Instruct-2507-GGUF", license: "Apache-2.0",
        notes: "Published by Alibaba (China). Smaller and faster of the two. 8192-token shared input/output context; results remain separate from raw dictation.", startup_language: "auto",
    },
    ModelSpec {
        id: "mistral-7b-instruct-v0-3-q4", display_name: "Mistral 7B Instruct v0.3 (Q4_K_M)",
        file_name: "mistral-7b-instruct-v0.3-q4.gguf", bytes: 4_372_812_000,
        sha256: "1270d22c0fbb3d092fb725d4d96c457b7b687a5f5a715abe1e818da303e562b6",
        source_url: "https://huggingface.co/bartowski/Mistral-7B-Instruct-v0.3-GGUF/resolve/61fd4167fff3ab01ee1cfe0da183fa27a944db48/Mistral-7B-Instruct-v0.3-Q4_K_M.gguf",
        source_name: "bartowski/Mistral-7B-Instruct-v0.3-GGUF", license: "Apache-2.0",
        notes: "Published by Mistral AI (France). Larger download and slower on CPU. 8192-token shared input/output context; results remain separate from raw dictation.", startup_language: "auto",
    },
];
pub fn model(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.id == id)
}
#[derive(Clone, Serialize)]
pub struct Execution {
    pub model: String,
    pub backend: &'static str,
    pub fallback: bool,
}
pub struct TextStore {
    pub assets: Arc<ManagedRuntime>,
    runtimes: Option<PathBuf>,
    execution: tokio::sync::Mutex<()>,
    last: Mutex<Option<Execution>>,
}
pub fn store() -> Result<Arc<TextStore>, ApiError> {
    static STORE: OnceLock<Result<Arc<TextStore>, ApiError>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let layout = crate::desktop_settings::Layout::from_env()?;
            let runtimes = layout
                .runtime_dir
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.join("text"));
            let assets = ManagedRuntime::open_with_catalog(
                RuntimeLayout {
                    runtime_dir: None,
                    ffmpeg: None,
                    models_dir: layout.models_dir.join("text"),
                    state_dir: layout.state_dir.join("text-models"),
                    startup_timeout: Duration::from_secs(180),
                },
                MODELS,
            )?;
            Ok(Arc::new(TextStore {
                assets,
                runtimes,
                execution: tokio::sync::Mutex::new(()),
                last: Mutex::new(None),
            }))
        })
        .clone()
}
impl TextStore {
    fn runtime(&self, gpu: bool) -> Option<PathBuf> {
        if !cfg!(any(
            windows,
            all(target_os = "macos", target_arch = "aarch64")
        )) {
            return None;
        }
        let root = self.runtimes.as_ref()?.join(if gpu {
            crate::local_stt::managed::GPU_BACKEND
        } else {
            "cpu"
        });
        (root.join(runtime::EXECUTABLE).is_file()
            && root
                .join(crate::local_stt::managed::VULKAN_BACKEND_LIBRARY)
                .is_file()
                == gpu)
            .then_some(root)
    }
    pub async fn generate(
        &self,
        selection: &ModelSelection,
        instructions: &str,
        input: &str,
    ) -> Result<String, ApiError> {
        let spec =
            model(&selection.model).ok_or_else(|| unavailable("Unknown local text model."))?;
        let _execution = self.execution.lock().await;
        let _lease = self.assets.inference_lease()?;
        if self.assets.installed(spec.id).is_none() {
            return Err(unavailable(
                "Download and verify the local text model in Settings → Models.",
            ));
        }
        let requested = self.assets.execution().requested;
        let mut fallback = false;
        if requested != ExecutionBackend::Cpu {
            if let Some(root) = self.runtime(true) {
                match runtime::generate(
                    &root,
                    &self.assets.layout.models_dir,
                    spec,
                    true,
                    instructions,
                    input,
                )
                .await
                {
                    Ok(output) => {
                        *self.last.lock().unwrap() = Some(Execution {
                            model: spec.id.into(),
                            backend: crate::local_stt::managed::GPU_BACKEND,
                            fallback: false,
                        });
                        return Ok(output);
                    }
                    Err(error)
                        if requested == ExecutionBackend::Automatic
                            && matches!(
                                error.code,
                                ErrorCode::ProviderUnavailable | ErrorCode::ProviderFailed
                            ) =>
                    {
                        fallback = true
                    }
                    Err(error) => return Err(error),
                }
            } else if requested == ExecutionBackend::Gpu {
                return Err(unavailable("The local text GPU runtime is not installed."));
            } else {
                fallback = true;
            }
        }
        let root = self.runtime(false).ok_or_else(|| {
            unavailable("The local text CPU runtime is not installed. Repair the Windows package.")
        })?;
        let output = runtime::generate(
            &root,
            &self.assets.layout.models_dir,
            spec,
            false,
            instructions,
            input,
        )
        .await?;
        *self.last.lock().unwrap() = Some(Execution {
            model: spec.id.into(),
            backend: "cpu",
            fallback,
        });
        Ok(output)
    }
    pub fn configured(&self, id: &str) -> bool {
        self.assets.installed(id).is_some()
            && (self.runtime(false).is_some() || self.runtime(true).is_some())
    }
    fn view(&self) -> serde_json::Value {
        serde_json::json!({"supported": cfg!(windows), "cpu_available": self.runtime(false).is_some(), "gpu_available": self.runtime(true).is_some(),
            "requested": self.assets.execution().requested, "last_execution": self.last.lock().unwrap().clone(),
            "models": MODELS.iter().map(|m| serde_json::json!({"id": m.id, "display_name": m.display_name, "bytes": m.bytes,
                "sha256": m.sha256, "source_url": m.source_url, "license": m.license, "notes": m.notes,
                "weights": self.assets.weights(m.id)})).collect::<Vec<_>>()})
    }
}
fn unavailable(message: &str) -> ApiError {
    super::failure(ErrorCode::ProviderUnavailable, message, true)
}
fn reply(result: Result<serde_json::Value, ApiError>) -> Response {
    match result {
        Ok(value) => Json(Envelope::new(value)).into_response(),
        Err(error) => crate::jobs::response_error(error),
    }
}
async fn view() -> Response {
    reply(store().map(|s| s.view()))
}
async fn action(Path((id, action)): Path<(String, String)>) -> Response {
    let result = async {
        let store = store()?;
        if model(&id).is_none() {
            return Err(unavailable("Unknown local text model."));
        }
        if action != "download"
            && action != "cancel"
            && crate::desktop_settings::global().is_ok_and(|s| s.runtime.recording_active())
        {
            return Err(unavailable(
                "Finish recording before changing text model files.",
            ));
        }
        match action.as_str() {
            "download" => store.assets.download(&id)?,
            "cancel" => store.assets.cancel_download(&id)?,
            "verify" => store.assets.verify(&id)?,
            "delete" => store.assets.delete_model(&id).await?,
            _ => {
                return Err(super::failure(
                    ErrorCode::InvalidRequest,
                    "Unknown text-model action.",
                    false,
                ))
            }
        }
        Ok(store.view())
    }
    .await;
    reply(result)
}
async fn execution(
    payload: Result<Json<Envelope<ExecutionBackend>>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected a version-one text execution preference.");
    };
    reply(
        async {
            let store = store()?;
            store.assets.save_backend(payload.data).await?;
            Ok(store.view())
        }
        .await,
    )
}
pub fn routes() -> Router {
    Router::new()
        .route("/api/v1/modes/local-models", get(view))
        .route("/api/v1/modes/local-models/execution", post(execution))
        .route("/api/v1/modes/local-models/{id}/{action}", post(action))
}
