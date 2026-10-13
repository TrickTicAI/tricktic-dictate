//! Dedicated single-owner mobile service. No desktop router or global data runtime.
use crate::{jobs::Pipeline, providers::TranscriptionProvider, transcription::*};
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Device {
    id: Uuid,
    digest: String,
    revoked: bool,
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn device_path(root: &Path, id: Uuid) -> PathBuf {
    root.join("devices").join(format!("{id}.json"))
}

/// Provision out of band. The secret is written once to a new private file, never stdout.
pub async fn issue(root: &Path, output: &Path) -> Result<Uuid, Box<dyn std::error::Error>> {
    issue_for(root, output, Uuid::new_v4()).await
}

/// Rotate a device in place so existing pending jobs keep their storage identity.
pub async fn rotate(
    root: &Path,
    id: Uuid,
    output: &Path,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let _: Device = serde_json::from_slice(&tokio::fs::read(device_path(root, id)).await?)?;
    issue_for(root, output, id).await
}
async fn issue_for(
    root: &Path,
    output: &Path,
    id: Uuid,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    use tokio::io::AsyncWriteExt;
    crate::recordings::disk::private_dir(root)
        .await
        .map_err(|_| "Cannot open private mobile root")?;
    crate::recordings::disk::private_dir(&root.join("devices"))
        .await
        .map_err(|_| "Cannot open device directory")?;
    let token = format!(
        "mw1.{id}.{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(output).await?;
    file.write_all(token.as_bytes()).await?;
    file.sync_all().await?;
    crate::recordings::disk::write_json(
        &device_path(root, id),
        &Device {
            id,
            digest: digest(&token),
            revoked: false,
        },
    )
    .await
    .map_err(|_| "Cannot save device")?;
    Ok(id)
}
pub async fn revoke(root: &Path, id: Uuid) -> Result<(), Box<dyn std::error::Error>> {
    let path = device_path(root, id);
    let mut device: Device = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
    device.revoked = true;
    crate::recordings::disk::write_json(&path, &device)
        .await
        .map_err(|_| "Cannot revoke device")?;
    Ok(())
}

pub struct Mobile {
    _lease: std::fs::File,
    root: PathBuf,
    max_upload: u64,
    providers: Vec<Arc<dyn TranscriptionProvider>>,
    routers: Mutex<HashMap<Uuid, Router>>,
    storage: Arc<crate::limits::storage::StorageBudget>,
    work: Arc<crate::limits::work::WorkCapacity>,
}
impl Mobile {
    pub async fn open(
        root: PathBuf,
        max_upload: u64,
        providers: Vec<Arc<dyn TranscriptionProvider>>,
    ) -> Result<Arc<Self>, ApiError> {
        crate::recordings::disk::private_dir(&root).await?;
        crate::recordings::disk::private_dir(&root.join("devices")).await?;
        let mut options = tokio::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let lease = options
            .open(root.join("mobile.lock"))
            .await
            .map_err(crate::recordings::disk::storage)?
            .into_std()
            .await;
        lease.try_lock().map_err(|_| {
            crate::recordings::disk::error(
                ErrorCode::StorageFailed,
                "Mobile data directory is already in use.",
                false,
            )
        })?;
        let limits = crate::limits::Limits::from_env()?;
        let storage = crate::limits::storage::StorageBudget::new(root.join("data"), limits.clone());
        let work = crate::limits::work::WorkCapacity::new(limits);
        let service = Arc::new(Self {
            storage,
            work,
            _lease: lease,
            root,
            max_upload,
            providers,
            routers: Mutex::new(HashMap::new()),
        });
        // Resume already accepted work after restart, even if a device was revoked.
        let mut entries = tokio::fs::read_dir(service.root.join("devices"))
            .await
            .map_err(crate::recordings::disk::storage)?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(crate::recordings::disk::storage)?
        {
            if let Some(id) = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                let _ = service.device_router(id).await?;
            }
        }
        Ok(service)
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .fallback(dispatch)
            .with_state(self.clone())
            .layer(axum::middleware::from_fn_with_state(
                crate::limits::http::Admission::new(&self.storage.limits, self.max_upload, false),
                crate::limits::http::admit,
            ))
    }
    async fn device_router(&self, id: Uuid) -> Result<Router, ApiError> {
        let mut routers = self.routers.lock().await;
        if let Some(router) = routers.get(&id) {
            return Ok(router.clone());
        }
        let root = self.root.join("data").join(id.to_string());
        let pipeline = Pipeline::open_shared(
            root.clone(),
            self.max_upload,
            self.providers.clone(),
            root.join("state/modes"),
            self.storage.clone(),
            Some(self.work.account()),
        )
        .await?;
        pipeline.start_worker();
        let providers = self.providers.clone();
        let router = Router::new()
            .route("/api/v1/mobile/connection", get(move || { let providers = providers.clone(); async move {
                Json(Envelope::new(serde_json::json!({"service": "metrix-writer-mobile", "mobile_version": 1, "device_id": id, "models": capabilities(&providers)})))
            }}))
            .merge(crate::recordings::routes_with_pipeline(Some(pipeline.clone())))
            .merge(crate::jobs::routes_with_pipeline(Some(pipeline.clone())))
            .merge(crate::cleanup::mobile_routes(pipeline.jobs.clone(), pipeline.work.clone()))
            .merge(crate::vocabulary::routes_with_store(Arc::new(crate::vocabulary::VocabularyStore::new(root.join("vocabulary.json")))))
            .merge(crate::language::routes())
            .layer(axum::middleware::from_fn_with_state(crate::limits::http::Admission::new(&self.storage.limits, self.max_upload, true), crate::limits::http::admit));
        routers.insert(id, router.clone());
        Ok(router)
    }
    async fn authenticate(
        &self,
        headers: &axum::http::HeaderMap,
        uri: &axum::http::Uri,
    ) -> Option<Uuid> {
        // No browser credential transport, query credentials, redirects or CORS surface.
        if headers.contains_key(header::ORIGIN) || uri.query().is_some() {
            return None;
        }
        if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
            return None;
        }
        let token = headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3
            || parts[0] != "mw1"
            || parts[2].len() != 64
            || !parts[2].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        let id = Uuid::parse_str(parts[1]).ok()?;
        let device: Device =
            serde_json::from_slice(&tokio::fs::read(device_path(&self.root, id)).await.ok()?)
                .ok()?;
        let candidate = digest(token);
        let equal = device.digest.len() == candidate.len()
            && device
                .digest
                .bytes()
                .zip(candidate.bytes())
                .fold(0u8, |v, (a, b)| v | (a ^ b))
                == 0;
        (device.id == id && !device.revoked && equal).then_some(id)
    }
}
fn capabilities(providers: &[Arc<dyn TranscriptionProvider>]) -> Vec<serde_json::Value> {
    crate::language::catalog().models.into_iter().filter_map(|model| {
        let info = providers.iter().map(|p| p.info()).find(|info| serde_json::to_value(&info.id).ok() == serde_json::to_value(&model.selection.provider).ok())?;
        Some(serde_json::json!({ "selection": model.selection, "input_languages": model.input_languages, "translation_targets": model.translation_targets, "execution_location": info.execution_location, "configured": info.configured, "available": info.available, "status": if !info.configured { "not_configured" } else if info.available { "available" } else { "unverified" } }))
    }).collect()
}
async fn dispatch(State(service): State<Arc<Mobile>>, mut request: Request) -> Response {
    let Some(id) = service.authenticate(request.headers(), request.uri()).await else {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer"), (header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({"api_version":"1", "error":{"code":"device_unauthorized", "message":"Pair this device or ask the owner for a replacement credential.", "retryable":false}}))).into_response();
    };
    request.headers_mut().remove(header::AUTHORIZATION);
    let mut response = match service.device_router(id).await {
        Ok(router) => router.oneshot(request).await.unwrap(),
        Err(error) => crate::jobs::response_error(error),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
#[cfg(test)]
mod tests;
