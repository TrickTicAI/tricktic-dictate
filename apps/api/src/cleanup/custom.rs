//! Text models the user adds by ID, for any cloud text provider.
//!
//! Writer cannot know what an arbitrary model accepts, so the user also says
//! what each mode speed sends as reasoning. The provider stays the judge: a
//! model or setting it rejects fails that request, and the original transcript
//! is kept, exactly as for the built-in models.
use crate::{
    recordings::disk::{private_dir, write_json},
    transcription::*,
};
use axum::{
    extract::{rejection::JsonRejection, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::Duration,
};

const LIMIT: usize = 50;
const MODEL_BYTES: usize = 128;

/// What one speed sends. `ProviderDefault` sends no reasoning parameter at all,
/// which every model accepts; the others are sent as the provider spells them.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reasoning {
    ProviderDefault,
    Off,
    Minimal,
    Low,
    Medium,
    High,
}
impl Reasoning {
    /// The effort string OpenAI, xAI and OpenRouter share. Gemini is mapped
    /// separately because it has no "none" effort.
    pub(super) fn effort(self) -> Option<&'static str> {
        match self {
            Self::ProviderDefault => None,
            Self::Off => Some("none"),
            Self::Minimal => Some("minimal"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomModel {
    pub provider: ProviderId,
    pub model: String,
    /// Sent when the mode's speed is Fast.
    pub fast: Reasoning,
    /// Sent when the mode's speed is Balanced, and by transcript cleanup.
    pub balanced: Reasoning,
    /// OpenRouter serves one model from many hosts of differing speed and
    /// quantization. Empty leaves the choice to OpenRouter's latency routing;
    /// otherwise only these hosts may serve it.
    #[serde(default)]
    pub only_hosts: Vec<String>,
    /// Hosts that may never serve it. Both lists cannot be used at once.
    #[serde(default)]
    pub ignore_hosts: Vec<String>,
}
const HOST_LIMIT: usize = 20;
/// OpenRouter's provider slugs, as its endpoints list spells them.
fn well_formed_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 64
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
        && !host.contains("..")
}

/// Speeds reported beside a custom model in discovery.
#[derive(Clone, Copy, Serialize)]
pub(super) struct Speeds {
    pub(super) fast: Reasoning,
    pub(super) balanced: Reasoning,
}

pub struct CustomModels {
    path: Option<PathBuf>,
    models: RwLock<Vec<CustomModel>>,
    write: tokio::sync::Mutex<()>,
    /// OpenRouter's public API, where the host list for a model is read. No key
    /// is sent: this route reads what anyone can read.
    endpoint: String,
    hosts: Mutex<HashMap<String, (std::time::Instant, Vec<Host>)>>,
}
/// One host OpenRouter can route a model to.
#[derive(Clone, Serialize)]
pub struct Host {
    /// What `only`/`ignore` name, for example `deepinfra/fp8`.
    pub tag: String,
    pub name: String,
    pub uptime: Option<f64>,
    pub quantization: Option<String>,
}
/// Long enough to serve a dialog being used, short enough to notice a new host.
const HOSTS_CACHED: Duration = Duration::from_secs(600);
impl CustomModels {
    /// Reads the saved list. A missing or unreadable file is an empty list: modes
    /// that name one of its models show it as unavailable instead of failing to load.
    pub fn open(path: Option<PathBuf>) -> Self {
        let models = path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Envelope<Vec<CustomModel>>>(&bytes).ok())
            .map(|envelope| envelope.data)
            .filter(|models| check(models).is_ok())
            .unwrap_or_default();
        Self {
            path,
            models: RwLock::new(models),
            write: tokio::sync::Mutex::new(()),
            endpoint: "https://openrouter.ai/api/v1".into(),
            hosts: Mutex::new(HashMap::new()),
        }
    }
    #[cfg(test)]
    pub(super) fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }
    /// The hosts OpenRouter can route this model to, cached briefly. A model
    /// nobody serves any more answers with an empty list rather than an error.
    pub async fn hosts(&self, model: &str) -> Result<Vec<Host>, ApiError> {
        if !well_formed(&ProviderId::Openrouter, model) {
            return Err(invalid("Hosts are listed for OpenRouter model IDs."));
        }
        if let Some((read, hosts)) = self.hosts.lock().unwrap().get(model) {
            if read.elapsed() < HOSTS_CACHED {
                return Ok(hosts.clone());
            }
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| unavailable())?;
        let response = client
            .get(format!("{}/models/{model}/endpoints", self.endpoint))
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let body: serde_json::Value = response.json().await.map_err(|_| unavailable())?;
        let hosts: Vec<Host> = body["data"]["endpoints"]
            .as_array()
            .ok_or_else(unavailable)?
            .iter()
            .filter_map(|endpoint| {
                let tag = endpoint["tag"].as_str()?;
                well_formed_host(tag).then(|| Host {
                    tag: tag.to_owned(),
                    name: endpoint["provider_name"].as_str().unwrap_or(tag).to_owned(),
                    uptime: endpoint["uptime_last_30m"].as_f64(),
                    quantization: endpoint["quantization"]
                        .as_str()
                        .filter(|q| *q != "unknown")
                        .map(str::to_owned),
                })
            })
            .take(64)
            .collect();
        self.hosts
            .lock()
            .unwrap()
            .insert(model.to_owned(), (std::time::Instant::now(), hosts.clone()));
        Ok(hosts)
    }
    pub fn list(&self) -> Vec<CustomModel> {
        self.models.read().unwrap().clone()
    }
    pub(super) fn find(&self, selection: &ModelSelection) -> Option<CustomModel> {
        self.models
            .read()
            .unwrap()
            .iter()
            .find(|m| same(&m.provider, &selection.provider) && m.model == selection.model)
            .cloned()
    }
    /// Replaces the whole list, so adding, editing and removing are one operation.
    pub async fn save(&self, models: Vec<CustomModel>) -> Result<Vec<CustomModel>, ApiError> {
        check(&models)?;
        let path = self.path.as_ref().ok_or_else(|| {
            invalid("Custom text models need a writable TrickTic Dictate data directory.")
        })?;
        let _write = self.write.lock().await;
        if let Some(parent) = path.parent() {
            private_dir(parent).await?;
        }
        write_json(path, &Envelope::new(models.clone())).await?;
        *self.models.write().unwrap() = models.clone();
        Ok(models)
    }
}

/// The process-wide list, beside the provider keys in Writer's state directory.
pub fn global() -> Arc<CustomModels> {
    static STORE: OnceLock<Arc<CustomModels>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            // Tests never read a developer's real list.
            let path = if cfg!(test) {
                None
            } else {
                crate::desktop_settings::Layout::from_env()
                    .ok()
                    .map(|layout| layout.state_dir.join("text-models.json"))
            };
            Arc::new(CustomModels::open(path))
        })
        .clone()
}

fn same(a: &ProviderId, b: &ProviderId) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// The ID is placed in request paths (`/models/{id}`), so it is limited to the
/// characters provider model IDs use. Only OpenRouter IDs are publisher-qualified.
pub(super) fn well_formed(provider: &ProviderId, model: &str) -> bool {
    let routed = matches!(provider, ProviderId::Openrouter);
    !model.is_empty()
        && model.len() <= MODEL_BYTES
        && model
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || routed && b == b'~')
        && model.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b"._:-".contains(&b) || routed && b"/~".contains(&b)
        })
        && !model.contains("..")
        && (!routed || model.contains('/') && model.split('/').all(|part| !part.is_empty()))
}

pub(super) fn check(models: &[CustomModel]) -> Result<(), ApiError> {
    if models.len() > LIMIT {
        return Err(invalid("At most 50 custom text models are supported."));
    }
    for (index, model) in models.iter().enumerate() {
        if matches!(model.provider, ProviderId::Local) {
            return Err(invalid(
                "Local text models are downloaded in Text processing, not added by ID.",
            ));
        }
        if !well_formed(&model.provider, &model.model) {
            return Err(invalid(
                if matches!(model.provider, ProviderId::Openrouter) {
                    "Enter the OpenRouter model ID as publisher/model, for example mistralai/mistral-medium-3-5."
                } else {
                    "Enter the model ID exactly as the provider's API names it, using letters, digits, '.', '_', ':' and '-'."
                },
            ));
        }
        let selection = ModelSelection {
            provider: model.provider.clone(),
            model: model.model.clone(),
        };
        if super::provider::builtin(&selection) {
            return Err(invalid("That model is already in TrickTic Dictate's list."));
        }
        if models[..index]
            .iter()
            .any(|m| same(&m.provider, &model.provider) && m.model == model.model)
        {
            return Err(invalid("That custom model is already added."));
        }
        if !model.only_hosts.is_empty() && !model.ignore_hosts.is_empty() {
            return Err(invalid(
                "Choose the hosts to use or the hosts to avoid, not both.",
            ));
        }
        let pinned = !model.only_hosts.is_empty() || !model.ignore_hosts.is_empty();
        if pinned && !matches!(model.provider, ProviderId::Openrouter) {
            return Err(invalid(
                "Only OpenRouter models are served by several hosts.",
            ));
        }
        let hosts = model.only_hosts.iter().chain(model.ignore_hosts.iter());
        if hosts.clone().count() > HOST_LIMIT || !hosts.clone().all(|h| well_formed_host(h)) {
            return Err(invalid(
                "Choose at most 20 hosts, named as OpenRouter's endpoint list spells them.",
            ));
        }
    }
    Ok(())
}

fn invalid(message: &str) -> ApiError {
    super::failure(ErrorCode::InvalidRequest, message, false)
}
fn unavailable() -> ApiError {
    super::failure(
        ErrorCode::ProviderUnavailable,
        "OpenRouter's host list could not be read. The model still works with automatic routing.",
        true,
    )
}

async fn list(State(store): State<Arc<CustomModels>>) -> impl IntoResponse {
    Json(Envelope::new(store.list()))
}
async fn save(
    State(store): State<Arc<CustomModels>>,
    payload: Result<Json<Envelope<Vec<CustomModel>>>, JsonRejection>,
) -> Response {
    let Ok(Json(payload)) = payload else {
        return crate::jobs::invalid("Expected a version-one list of custom text models.");
    };
    match store.save(payload.data).await {
        Ok(models) => Json(Envelope::new(models)).into_response(),
        Err(error) => crate::jobs::response_error(error),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostQuery {
    model: String,
}
async fn hosts(
    State(store): State<Arc<CustomModels>>,
    query: Result<Json<Envelope<HostQuery>>, JsonRejection>,
) -> Response {
    let Ok(Json(query)) = query else {
        return crate::jobs::invalid("Name the model whose hosts to list.");
    };
    match store.hosts(&query.data.model).await {
        Ok(hosts) => Json(Envelope::new(hosts)).into_response(),
        Err(error) => crate::jobs::response_error(error),
    }
}
pub(super) fn router(store: Arc<CustomModels>) -> Router {
    Router::new()
        .route("/api/v1/modes/custom-models", get(list).put(save))
        .route("/api/v1/modes/custom-models/hosts", post(hosts))
        .with_state(store)
}
