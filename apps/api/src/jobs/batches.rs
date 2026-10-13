//! Explicit historical batches. One active item, atomic snapshots, no live job/delivery events.
use super::{JobStore, Pipeline};
use crate::{
    cleanup::modes::{ModeKind, ModeProfile},
    recordings::{
        disk::{error, private_dir, read_json, storage, write_json},
        RecordingStore,
    },
    transcription::*,
};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, MutexGuard, Notify};
use uuid::Uuid;

const MAX_ITEMS: usize = 32;
const MAX_BATCHES: usize = 128;
const MAX_DOCUMENT: usize = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
// Bump whenever recognition/vocabulary/prompt semantics change. Old unverifiable attempts are not cached.
const STAGE_VERSION: &str = "mw113-stages-1";
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Transcribe,
    Cleanup,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub recording_id: Uuid,
    pub job_id: Option<Uuid>,
    #[serde(default)]
    pub version_id: Option<Uuid>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    pub id: Uuid,
    pub sources: Vec<Source>,
    pub operation: Operation,
    pub options: TranscriptionOptions,
    pub profile: Option<ModeProfile>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Version {
    pub id: Uuid,
    pub source: Source,
    pub input_hash: String,
    pub fingerprint: String,
    pub raw_text: String,
    pub text: String,
    pub transcript: Option<Transcript>,
    pub reused_from: Option<Uuid>,
    pub created_at_ms: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    pub source: Source,
    pub state: String,
    pub unavailable: Option<String>,
    pub input_hash: String,
    pub fingerprint: String,
    pub input_raw: Option<String>,
    pub previous_text: Option<String>,
    pub reuse_available: bool,
    pub version: Option<Version>,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Batch {
    pub id: Uuid,
    pub created_at_ms: u64,
    pub state: String,
    pub request: PlanRequest,
    pub execution_identity: serde_json::Value,
    pub items: Vec<Item>,
    pub likely_paid_items: usize,
    pub paid_work: bool,
    pub provider_model: String,
}
pub struct BatchService {
    root: PathBuf,
    budget: Arc<crate::limits::storage::StorageBudget>,
    lock: Mutex<()>,
    active: Mutex<Option<Uuid>>,
    canceled: Notify,
}
fn invalid(message: &str) -> ApiError {
    error(ErrorCode::InvalidRequest, message, false)
}
fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
fn fingerprint(
    hash: &str,
    request: &PlanRequest,
    identity: &serde_json::Value,
) -> Result<String, ApiError> {
    Ok(digest(&serde_json::to_vec(&serde_json::json!({"stage":STAGE_VERSION,"input":hash,"operation":request.operation,"options":request.options,"profile":request.profile,"identity":identity})).map_err(storage)?))
}
fn cleanup_identity(identity: &serde_json::Value, transcript: &Transcript) -> serde_json::Value {
    serde_json::json!({"execution":identity,"source_selection":transcript.selection,
        "source_input_language":transcript.input_language,"source_output_language":transcript.output_language})
}
fn cloud(request: &PlanRequest) -> bool {
    match request.operation {
        Operation::Transcribe => !matches!(request.options.selection.provider, ProviderId::Local),
        Operation::Cleanup => request.profile.as_ref().is_some_and(|p| {
            p.kind != ModeKind::Regular
                && [p.text_model.as_ref(), p.fallback_model.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|m| !matches!(m.provider, ProviderId::Local))
        }),
    }
}
impl BatchService {
    pub(crate) async fn open(
        root: PathBuf,
        budget: Arc<crate::limits::storage::StorageBudget>,
    ) -> Result<Arc<Self>, ApiError> {
        private_dir(&root).await?;
        let service = Arc::new(Self {
            root,
            budget,
            lock: Mutex::new(()),
            active: Mutex::new(None),
            canceled: Notify::new(),
        });
        // Process loss never silently restarts paid work. Outcomes already committed stay intact.
        for mut batch in service.list().await? {
            if batch.state == "running" {
                batch.state = "paused".into();
                for item in &mut batch.items {
                    if item.state == "running" {
                        item.state = "interrupted".into();
                        item.error = Some("Interrupted; provider outcome may be unknown. Explicit resume may repeat paid work.".into());
                    }
                }
                service.save(&batch).await?;
            }
        }
        Ok(service)
    }
    fn path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }
    async fn save(&self, batch: &Batch) -> Result<(), ApiError> {
        let bytes = serde_json::to_vec(batch).map_err(storage)?.len();
        if bytes > MAX_DOCUMENT {
            return Err(invalid(
                "Batch results exceed the retained document budget; originals remain saved.",
            ));
        }
        let mut files = tokio::fs::read_dir(&self.root).await.map_err(storage)?;
        let mut total = bytes as u64;
        while let Some(file) = files.next_entry().await.map_err(storage)? {
            if file.path() != self.path(batch.id)
                && file.path().extension().is_some_and(|x| x == "json")
            {
                total = total.saturating_add(file.metadata().await.map_err(storage)?.len());
            }
        }
        if total > MAX_TOTAL_BYTES {
            return Err(crate::limits::busy("Historical batch storage is full (32 MiB); export and delete completed batch versions. Originals remain saved."));
        }
        let _reservation = self
            .budget
            .reserve(
                self.root.parent().unwrap(),
                self.path(batch.id),
                bytes as u64,
            )
            .await?;
        write_json(&self.path(batch.id), batch).await
    }
    pub async fn list(&self) -> Result<Vec<Batch>, ApiError> {
        let mut files = tokio::fs::read_dir(&self.root).await.map_err(storage)?;
        let mut batches = Vec::new();
        while let Some(file) = files.next_entry().await.map_err(storage)? {
            if file.path().extension().is_some_and(|x| x == "json") {
                batches.push(read_json::<Batch>(&file.path()).await?);
            }
        }
        batches.sort_by_key(|b| std::cmp::Reverse(b.created_at_ms));
        Ok(batches)
    }
    pub async fn get(&self, id: Uuid) -> Result<Batch, ApiError> {
        if !tokio::fs::try_exists(self.path(id))
            .await
            .map_err(storage)?
        {
            return Err(invalid("Batch not found."));
        }
        read_json(&self.path(id)).await
    }
    async fn cached(&self, fingerprint: &str) -> Result<Option<Version>, ApiError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .flat_map(|b| b.items)
            .filter(|i| i.state == "succeeded")
            .filter_map(|i| i.version)
            .find(|v| v.fingerprint == fingerprint))
    }
    async fn source_text(
        &self,
        pipeline: &Pipeline,
        source: &Source,
    ) -> Result<(Transcript, String), ApiError> {
        if source.job_id.is_some() && source.version_id.is_some() {
            return Err(invalid("Choose one saved raw version."));
        }
        if let Some(id) = source.version_id {
            let version = self
                .list()
                .await?
                .into_iter()
                .flat_map(|b| b.items)
                .filter(|i| i.state == "succeeded")
                .filter_map(|i| i.version)
                .find(|v| v.id == id && v.source.recording_id == source.recording_id)
                .ok_or_else(|| invalid("Selected historical raw version is unavailable."))?;
            return Ok((
                version
                    .transcript
                    .ok_or_else(|| invalid("Raw version metadata unavailable."))?,
                version.text,
            ));
        }
        let job = pipeline
            .jobs
            .get(
                source
                    .job_id
                    .ok_or_else(|| invalid("Select a saved raw transcript."))?,
            )
            .await?;
        if job.request.recording_id != source.recording_id {
            return Err(invalid("Source transcript belongs to another recording."));
        }
        let JobStatus::Succeeded { result } = job.status else {
            return Err(invalid("Select a successful saved raw transcript."));
        };
        let text = match result.cleanup {
            CleanupResult::Succeeded { cleaned_text, .. } => cleaned_text,
            _ => result.transcript.raw_text.clone(),
        };
        Ok((result.transcript, text))
    }
    pub async fn plan(&self, pipeline: &Pipeline, request: PlanRequest) -> Result<Batch, ApiError> {
        let _lock = self.lock.lock().await;
        if request.sources.is_empty() || request.sources.len() > MAX_ITEMS {
            return Err(invalid("Select 1–32 saved items."));
        }
        if request
            .sources
            .iter()
            .map(|s| s.recording_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != request.sources.len()
        {
            return Err(invalid("Select each recording only once."));
        }
        if serde_json::to_vec(&request).map_err(storage)?.len() > 128 * 1024 {
            return Err(invalid("Batch settings exceed 128 KiB."));
        }
        if request.operation == Operation::Cleanup
            && request
                .profile
                .as_ref()
                .is_none_or(|p| p.kind == ModeKind::Command)
        {
            return Err(invalid("Choose a Regular or Super cleanup mode."));
        }
        if tokio::fs::try_exists(self.path(request.id))
            .await
            .map_err(storage)?
        {
            let existing = self.get(request.id).await?;
            if serde_json::to_value(&existing.request).map_err(storage)?
                != serde_json::to_value(&request).map_err(storage)?
            {
                return Err(invalid("Batch ID already belongs to different settings."));
            }
            return Ok(existing);
        }
        if self.list().await?.len() >= MAX_BATCHES {
            return Err(crate::limits::busy(
                "Batch history is full; export results and delete completed batches first.",
            ));
        }
        let identity = crate::cleanup::modes::historical_identity();
        let mut items = Vec::new();
        for source in &request.sources {
            let mut item = Item {
                source: source.clone(),
                state: "queued".into(),
                unavailable: None,
                input_hash: String::new(),
                fingerprint: String::new(),
                input_raw: None,
                previous_text: None,
                reuse_available: false,
                version: None,
                error: None,
            };
            let resolved = async {
                let mut stage_identity = identity.clone();
                if source.job_id.is_some() || source.version_id.is_some() {
                    let (transcript, text) = self.source_text(pipeline, source).await?;
                    if request.operation == Operation::Cleanup { stage_identity = cleanup_identity(&identity, &transcript); }
                    item.previous_text = Some(text);
                    item.input_raw = Some(transcript.raw_text);
                }
                if request.operation == Operation::Transcribe {
                    let audio = pipeline.recordings.audio(source.recording_id).await?;
                    // Stream hashing; never load arbitrarily long recordings into RAM.
                    item.input_hash = hash_audio(&audio.path).await?;
                } else {
                    item.input_hash = digest(item.input_raw.as_ref().ok_or_else(|| invalid("No saved successful raw transcript; transcribe saved audio first."))?.as_bytes());
                }
                item.fingerprint = fingerprint(&item.input_hash, &request, &stage_identity)?;
                // Local model names do not identify immutable weights/runtime; conservative miss.
                item.reuse_available = cacheable(&request) && self.cached(&item.fingerprint).await?.is_some();
                Ok::<_, ApiError>(())
            }.await;
            if let Err(e) = resolved {
                item.state = "unavailable".into();
                item.unavailable = Some(e.message);
            }
            items.push(item);
        }
        let paid_work = cloud(&request);
        let likely_paid_items = if paid_work {
            items
                .iter()
                .filter(|i| i.state == "queued" && !i.reuse_available)
                .count()
        } else {
            0
        };
        let selection = if request.operation == Operation::Transcribe {
            Some(&request.options.selection)
        } else {
            request.profile.as_ref().and_then(|p| p.text_model.as_ref())
        };
        let provider_model = selection
            .map(|s| {
                format!(
                    "{} / {}",
                    serde_json::to_value(&s.provider).unwrap().as_str().unwrap(),
                    s.model
                )
            })
            .unwrap_or_else(|| "Dictionary and snippets on your server".into());
        let batch = Batch {
            id: request.id,
            created_at_ms: super::disk::now(),
            state: "planned".into(),
            request,
            execution_identity: identity,
            items,
            likely_paid_items,
            paid_work,
            provider_model,
        };
        self.save(&batch).await?;
        Ok(batch)
    }
    pub async fn start(
        self: &Arc<Self>,
        pipeline: Arc<Pipeline>,
        id: Uuid,
        acknowledge_paid: bool,
    ) -> Result<Batch, ApiError> {
        let _lock = self.lock.lock().await;
        let mut active = self.active.lock().await;
        let mut batch = self.get(id).await?;
        if batch.state == "running" && *active == Some(id) {
            return Ok(batch);
        }
        if !matches!(batch.state.as_str(), "planned" | "paused") {
            return Err(invalid("Only a planned or paused batch can start."));
        }
        if batch.paid_work && !acknowledge_paid {
            return Err(invalid(
                "Confirm potential paid cloud work before starting or resuming.",
            ));
        }
        if active.is_some() {
            return Err(crate::limits::busy(
                "Another historical batch is active; wait or cancel it.",
            ));
        }
        if batch.execution_identity != crate::cleanup::modes::historical_identity() {
            return Err(invalid(
                "Model routing or prompt settings changed; make a new plan.",
            ));
        }
        batch.state = "running".into();
        for item in &mut batch.items {
            if matches!(item.state.as_str(), "interrupted" | "running") {
                item.state = "queued".into();
            }
        }
        self.save(&batch).await?;
        *active = Some(id);
        let service = self.clone();
        tokio::spawn(async move {
            if service.run(&pipeline, id).await.is_err() {
                // On storage failure stop spending. Snapshot is repaired as paused on restart.
                tracing::warn!("Historical batch stopped; explicit recovery is required.");
                let _lock = service.lock.lock().await;
                if let Ok(mut batch) = service.get(id).await {
                    batch.state = "paused".into();
                    let _ = service.save(&batch).await;
                }
            }
            *service.active.lock().await = None;
        });
        Ok(batch)
    }
    pub async fn cancel(&self, id: Uuid) -> Result<Batch, ApiError> {
        let _lock = self.lock.lock().await;
        let mut batch = self.get(id).await?;
        if batch.state == "complete" || batch.state == "canceled" {
            return Ok(batch);
        }
        batch.state = "canceled".into();
        for item in &mut batch.items {
            if matches!(item.state.as_str(), "queued" | "running" | "interrupted") {
                item.state = "canceled".into();
            }
        }
        self.save(&batch).await?;
        self.canceled.notify_one();
        Ok(batch)
    }
    async fn run(&self, pipeline: &Pipeline, id: Uuid) -> Result<(), ApiError> {
        loop {
            let (mut batch, index) = {
                let _lock = self.lock.lock().await;
                let mut batch = self.get(id).await?;
                if batch.state != "running" {
                    return Ok(());
                }
                let Some(index) = batch.items.iter().position(|i| i.state == "queued") else {
                    batch.state = "complete".into();
                    self.save(&batch).await?;
                    return Ok(());
                };
                batch.items[index].state = "running".into();
                self.save(&batch).await?;
                (batch, index)
            };
            let result = {
                let execution = self.execute(pipeline, &batch, index);
                tokio::pin!(execution);
                loop {
                    tokio::select! {
                        value = &mut execution => break value,
                        _ = self.canceled.notified() => {
                            if self.get(id).await?.state != "running" { return Ok(()); }
                        },
                    }
                }
            };
            let _lock = self.lock.lock().await;
            if self.get(id).await?.state != "running" {
                return Ok(());
            }
            let item = &mut batch.items[index];
            match result {
                Ok(version) => {
                    item.state = "succeeded".into();
                    item.version = Some(version);
                    item.error = None;
                }
                Err(e) => {
                    item.state = "failed".into();
                    item.error = Some(e.message);
                }
            }
            self.save(&batch).await?;
        }
    }
    async fn execute(
        &self,
        pipeline: &Pipeline,
        batch: &Batch,
        index: usize,
    ) -> Result<Version, ApiError> {
        let item = &batch.items[index];
        if batch.execution_identity != crate::cleanup::modes::historical_identity() {
            return Err(invalid(
                "Model routing/prompt settings changed; create a new batch.",
            ));
        }
        let mut reused_from = None;
        let (raw_text, text, transcript) = if batch.request.operation == Operation::Transcribe {
            let audio = pipeline.recordings.audio(item.source.recording_id).await?;
            if hash_audio(&audio.path).await? != item.input_hash {
                return Err(invalid(
                    "Saved audio changed since planning; make a new plan.",
                ));
            }
            if let Some(cached) = if cacheable(&batch.request) {
                self.cached(&item.fingerprint).await?
            } else {
                None
            } {
                reused_from = Some(cached.id);
                (cached.raw_text, cached.text, cached.transcript)
            } else {
                let provider = pipeline
                    .providers
                    .iter()
                    .find(|p| {
                        std::mem::discriminant(&p.info().id)
                            == std::mem::discriminant(&batch.request.options.selection.provider)
                    })
                    .ok_or_else(|| invalid("Selected speech provider is unavailable."))?;
                let _permit = pipeline.work.try_admit_transcription().ok_or_else(|| {
                    crate::limits::busy(
                        "Recognition busy; audio retained. Start a new batch when ready.",
                    )
                })?;
                let result = tokio::time::timeout(
                    pipeline.work.timeout(),
                    provider.transcribe(&audio, &batch.request.options),
                )
                .await
                .map_err(|_| {
                    error(
                        ErrorCode::ProviderTimeout,
                        "Historical transcription timed out; audio retained.",
                        true,
                    )
                })??;
                (
                    result.raw_text.clone(),
                    result.raw_text.clone(),
                    Some(result),
                )
            }
        } else {
            // Cleanup can still work after audio retention expires; verify the selected raw version.
            let (source_transcript, _) = self.source_text(pipeline, &item.source).await?;
            if digest(source_transcript.raw_text.as_bytes()) != item.input_hash {
                return Err(invalid("Source raw text changed; make a new plan."));
            }
            if fingerprint(
                &item.input_hash,
                &batch.request,
                &cleanup_identity(&batch.execution_identity, &source_transcript),
            )? != item.fingerprint
            {
                return Err(invalid(
                    "Source language or model metadata changed; make a new plan.",
                ));
            }
            if let Some(cached) = if cacheable(&batch.request) {
                self.cached(&item.fingerprint).await?
            } else {
                None
            } {
                reused_from = Some(cached.id);
                (
                    source_transcript.raw_text.clone(),
                    cached.text,
                    Some(source_transcript),
                )
            } else {
                let text = pipeline
                    .mode_service()
                    .await?
                    .historical(
                        &source_transcript.raw_text,
                        &batch.request.options.vocabulary,
                        batch.request.profile.clone().unwrap(),
                    )
                    .await?;
                (
                    source_transcript.raw_text.clone(),
                    text,
                    Some(source_transcript),
                )
            }
        };
        if batch.execution_identity != crate::cleanup::modes::historical_identity() {
            return Err(invalid(
                "Model settings changed during processing; result cannot be reused.",
            ));
        }
        Ok(Version {
            id: Uuid::new_v4(),
            source: item.source.clone(),
            input_hash: item.input_hash.clone(),
            fingerprint: item.fingerprint.clone(),
            raw_text,
            text,
            transcript,
            reused_from,
            created_at_ms: super::disk::now(),
        })
    }
    pub(crate) async fn begin_deletion(&self) -> Result<MutexGuard<'_, ()>, ApiError> {
        let guard = self.lock.lock().await;
        if self.active.lock().await.is_some() {
            return Err(crate::limits::busy(
                "Cancel or finish historical processing before deleting its sources.",
            ));
        }
        Ok(guard)
    }
    pub(crate) async fn clear_all(&self) -> Result<(), ApiError> {
        for batch in self.list().await? {
            tokio::fs::remove_file(self.path(batch.id))
                .await
                .map_err(storage)?;
        }
        Ok(())
    }
    pub(crate) async fn clear_selected(&self, ids: &[Uuid]) -> Result<(), ApiError> {
        for mut batch in self.list().await? {
            batch
                .items
                .retain(|i| !ids.contains(&i.source.recording_id));
            batch
                .request
                .sources
                .retain(|i| !ids.contains(&i.recording_id));
            if batch.items.is_empty() {
                tokio::fs::remove_file(self.path(batch.id))
                    .await
                    .map_err(storage)?;
            } else {
                self.save(&batch).await?;
            }
        }
        Ok(())
    }
    async fn delete(&self, id: Uuid) -> Result<(), ApiError> {
        let _lock = self.lock.lock().await;
        if self.active.lock().await.as_ref() == Some(&id) {
            return Err(crate::limits::busy(
                "Cancel and wait before deleting this batch.",
            ));
        }
        self.get(id).await?;
        tokio::fs::remove_file(self.path(id)).await.map_err(storage)
    }
}
fn cacheable(request: &PlanRequest) -> bool {
    match request.operation {
        Operation::Transcribe => !matches!(request.options.selection.provider, ProviderId::Local),
        Operation::Cleanup => request.profile.as_ref().is_some_and(|p| {
            [p.text_model.as_ref(), p.fallback_model.as_ref()]
                .into_iter()
                .flatten()
                .all(|m| !matches!(m.provider, ProviderId::Local))
        }),
    }
}
async fn hash_audio(path: &std::path::Path) -> Result<String, ApiError> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await.map_err(storage)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).await.map_err(storage)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
async fn selected(
    state: Option<Arc<Pipeline>>,
) -> Result<(Arc<Pipeline>, Arc<BatchService>), ApiError> {
    let pipeline = super::runtime::selected(state).await?;
    let service = pipeline.batch_service().await?;
    Ok((pipeline, service))
}
fn response<T: Serialize>(result: Result<T, ApiError>) -> Response {
    match result {
        Ok(value) => Json(Envelope::new(value)).into_response(),
        Err(e) => super::response_error(e),
    }
}
async fn list(State(state): State<Option<Arc<Pipeline>>>) -> Response {
    response(async { selected(state).await?.1.list().await }.await)
}
async fn plan(
    State(state): State<Option<Arc<Pipeline>>>,
    Json(body): Json<Envelope<PlanRequest>>,
) -> Response {
    response(
        async {
            let (p, s) = selected(state).await?;
            s.plan(&p, body.data).await
        }
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    acknowledge_paid: bool,
}
async fn start(
    State(state): State<Option<Arc<Pipeline>>>,
    Path(id): Path<Uuid>,
    Json(body): Json<Envelope<Start>>,
) -> Response {
    response(
        async {
            let (p, s) = selected(state).await?;
            s.start(p, id, body.data.acknowledge_paid).await
        }
        .await,
    )
}
async fn cancel(State(state): State<Option<Arc<Pipeline>>>, Path(id): Path<Uuid>) -> Response {
    response(async { selected(state).await?.1.cancel(id).await }.await)
}
async fn delete(State(state): State<Option<Arc<Pipeline>>>, Path(id): Path<Uuid>) -> Response {
    response(
        async {
            selected(state).await?.1.delete(id).await?;
            Ok(serde_json::json!({"deleted":true}))
        }
        .await,
    )
}
pub(super) fn routes(state: Option<Arc<Pipeline>>) -> Router {
    Router::new()
        .route("/api/v1/batches", get(list).post(plan))
        .route("/api/v1/batches/{id}", axum::routing::delete(delete))
        .route("/api/v1/batches/{id}/start", post(start))
        .route("/api/v1/batches/{id}/cancel", post(cancel))
        .with_state(state)
}
#[cfg(test)]
mod tests;
