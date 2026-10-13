//! Projection of the existing monotonic trace, never a transcript/history export.
//! Diagnostics are best effort and cannot fail recording or model execution.
use super::{traces, Event};
use crate::{
    cleanup::modes::ModeKind,
    transcription::{Job, JobStatus, ProviderId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use uuid::Uuid;

pub const RETAINED_ATTEMPTS: usize = 256;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MS: u64 = 7 * 24 * 60 * 60 * 1000;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Warm {
    Cold,
    Warm,
    Unknown,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measurement {
    Unclassified,
    Synthetic,
    PhysicalDevice,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
    Android,
    Browser,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pending,
    Succeeded,
    Failed,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    CaptureReadiness,
    CaptureHandoff,
    NativeImport,
    ImportMaintenanceWait,
    ImportBudgetWait,
    Preparation,
    Upload,
    Queue,
    Asr,
    Cleanup,
    RawAvailability,
    Insertion,
    StopToInsertion,
    CloudRequest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub recording_id: Uuid,
    pub attempt_id: Uuid,
    pub retry_of: Option<Uuid>,
    pub provider_attempt: u64,
    pub platform: Platform,
    pub provider: ProviderId,
    /// SHA256 of the configured identifier; custom model names cannot leak content.
    pub model_sha256: String,
    pub mode: ModeKind,
    pub warm: Warm,
    pub measurement: Measurement,
    pub outcome: Outcome,
    pub stages_ms: BTreeMap<Stage, u64>,
    pub stage_outcomes: BTreeMap<Stage, Outcome>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Export {
    pub records: Vec<Record>,
}
#[derive(Clone)]
struct Active {
    path: PathBuf,
    record: Record,
}
static ACTIVE: OnceLock<Mutex<HashMap<Uuid, Active>>> = OnceLock::new();
static DISK: Mutex<()> = Mutex::new(());
fn active() -> &'static Mutex<HashMap<Uuid, Active>> {
    ACTIVE.get_or_init(Default::default)
}
fn platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}
fn retain_capture(id: Uuid) {
    if let Ok(mut traces) = traces().lock() {
        if let Some(t) = traces.get_mut(&id) {
            t.events.retain(|e| {
                matches!(
                    e.stage,
                    "shortcut_pressed"
                        | "start_requested"
                        | "capture_requested"
                        | "capture_started"
                        | "first_audio"
                        | "first_audio_saved"
                        | "stop_accepted"
                        | "audio_durable"
                        | "audio_import_requested"
                        | "audio_import_lock_acquired"
                        | "import_budget_requested"
                        | "import_budget_acquired"
                        | "audio_imported"
                )
            });
            t.details.retain(|key, _| *key == "mode");
        }
    }
}
pub fn register(job: &Job, path: PathBuf) {
    retain_capture(job.request.recording_id);
    let mode = traces()
        .lock()
        .ok()
        .and_then(|t| {
            t.get(&job.request.recording_id)
                .and_then(|t| t.details.get("mode"))
                .cloned()
        })
        .and_then(|mode| serde_json::from_value(mode).ok())
        .or_else(|| {
            read(&path)
                .records
                .iter()
                .rev()
                .find(|r| r.attempt_id == job.id)
                .map(|r| r.mode)
        })
        .unwrap_or(ModeKind::Regular);
    let record = Record {
        recording_id: job.request.recording_id,
        attempt_id: job.id,
        retry_of: job.request.retry_of_job_id,
        provider_attempt: 0,
        platform: platform(),
        provider: job.request.options.selection.provider.clone(),
        model_sha256: format!(
            "{:x}",
            Sha256::digest(job.request.options.selection.model.as_bytes())
        ),
        mode,
        warm: Warm::Unknown,
        measurement: Measurement::Unclassified,
        outcome: Outcome::Pending,
        stages_ms: BTreeMap::new(),
        stage_outcomes: BTreeMap::new(),
    };
    if let Ok(mut a) = active().lock() {
        if a.len() >= 64 && !a.contains_key(&record.recording_id) {
            // Trace residency is bounded as well as disk residency. Eviction means
            // later stages remain absent; it never reconstructs a duration.
            if let Some(id) = a.keys().next().copied() {
                a.remove(&id);
            }
        }
        a.insert(record.recording_id, Active { path, record });
    }
}
pub fn ensure_registered(job: &Job, path: PathBuf) {
    let exists = active().lock().ok().is_some_and(|a| {
        a.get(&job.request.recording_id)
            .is_some_and(|r| r.record.attempt_id == job.id)
    });
    if !exists {
        register(job, path);
    }
}
pub fn start_attempt(job: &Job, ordinal: u64) {
    // Automatic retries share a job ID but never share model stage endpoints.
    // The previous attempt was flushed before this reset.
    if let Ok(mut a) = active().lock() {
        if let Some(a) = a.get_mut(&job.request.recording_id) {
            a.record.provider_attempt = ordinal;
            a.record.warm = Warm::Unknown;
            a.record.outcome = Outcome::Pending;
        }
    }
}
pub fn set_warm(id: Uuid, warm: Warm) {
    if let Ok(mut a) = active().lock() {
        if let Some(a) = a.get_mut(&id) {
            a.record.warm = warm;
        }
    }
}
pub fn set_measurement(id: Uuid, measurement: Measurement) {
    if let Ok(mut a) = active().lock() {
        if let Some(a) = a.get_mut(&id) {
            a.record.measurement = measurement;
        }
    }
}
pub fn set_mode(id: Uuid, mode: ModeKind) {
    if let Ok(mut a) = active().lock() {
        if let Some(a) = a.get_mut(&id) {
            a.record.mode = mode;
        }
    }
}
pub fn save_job(job: &Job, path: &Path) {
    ensure_registered(job, path.to_owned());
    if let Ok(mut a) = active().lock() {
        if let Some(a) = a.get_mut(&job.request.recording_id) {
            a.record.outcome = match job.status {
                JobStatus::Succeeded { .. } => Outcome::Succeeded,
                _ => Outcome::Failed,
            };
        }
    }
    flush(job.request.recording_id);
    if matches!(job.status, JobStatus::Queued {}) {
        // Begin retry queue at the failed attempt's durable transition, so the
        // next queue duration includes backoff and capacity waits.
        retain_capture(job.request.recording_id);
        super::mark(job.request.recording_id, "job_durable");
    }
}
fn duration(events: &[Event], start: &str, end: &str) -> Option<u64> {
    let from = events.iter().find(|e| e.stage == start)?.elapsed_ms;
    events
        .iter()
        .find(|e| e.stage == end && e.elapsed_ms >= from)?
        .elapsed_ms
        .checked_sub(from)
        .filter(|v| *v <= MAX_MS)
}
fn paired_sum(events: &[Event], start: &str, end: &str) -> Option<u64> {
    let mut from = None;
    let mut total = 0u64;
    let mut measured = false;
    for e in events {
        if e.stage == start {
            from = Some(e.elapsed_ms);
        }
        if e.stage == end {
            if let Some(at) = from.take() {
                total = total.checked_add(e.elapsed_ms.checked_sub(at)?)?;
                measured = true;
            }
        }
    }
    (measured && from.is_none() && total <= MAX_MS).then_some(total)
}
fn project(record: &mut Record, events: &[Event], details: &BTreeMap<&str, serde_json::Value>) {
    record.stages_ms.clear();
    let mut put = |stage, value| {
        if let Some(v) = value {
            record.stages_ms.insert(stage, v);
        }
    };
    let ready = duration(events, "capture_requested", "first_audio")
        .or_else(|| duration(events, "start_requested", "first_audio"))
        .or_else(|| duration(events, "shortcut_pressed", "first_audio"));
    put(Stage::CaptureReadiness, ready);
    for (stage, start, end) in [
        (
            Stage::CaptureHandoff,
            "audio_durable",
            "audio_import_requested",
        ),
        (
            Stage::NativeImport,
            "audio_import_requested",
            "audio_imported",
        ),
        (
            Stage::ImportMaintenanceWait,
            "audio_import_requested",
            "audio_import_lock_acquired",
        ),
        (
            Stage::ImportBudgetWait,
            "import_budget_requested",
            "import_budget_acquired",
        ),
    ] {
        put(stage, duration(events, start, end));
    }
    put(
        Stage::Queue,
        duration(events, "job_durable", "recognition_started"),
    );
    put(
        Stage::Preparation,
        duration(events, "preparation_started", "preparation_finished").or_else(|| {
            details
                .get("silence_prepare_ms")
                .and_then(|v| v.as_u64())
                .filter(|v| *v <= MAX_MS)
        }),
    );
    // Inline cloud calls combine upload/network and provider execution; do not
    // relabel their historical final_inference events as isolated ASR.
    if matches!(record.provider, ProviderId::Local) {
        put(
            Stage::Asr,
            paired_sum(events, "final_inference_start", "final_inference_done"),
        );
    }
    put(
        Stage::CloudRequest,
        duration(events, "cloud_request_started", "cloud_request_finished"),
    );
    put(
        Stage::Upload,
        duration(events, "upload_started", "upload_finished"),
    );

    if matches!(record.outcome, Outcome::Succeeded) {
        put(
            Stage::RawAvailability,
            duration(events, "stop_accepted", "raw_result_durable"),
        );
    }
    put(
        Stage::Insertion,
        duration(events, "delivery_started", "paste_sent"),
    );
    put(
        Stage::StopToInsertion,
        duration(events, "stop_accepted", "paste_sent"),
    );
}
/// Update a durable job by its identity, including manual processing of an older
/// recording. Never attribute overlapping cleanup to the latest recording trace.
pub fn record_stage(
    path: &Path,
    job_id: Uuid,
    mode: ModeKind,
    stage: Stage,
    elapsed_ms: u64,
    outcome: Outcome,
) {
    if elapsed_ms > MAX_MS {
        return;
    }
    let Ok(_guard) = DISK.lock() else {
        return;
    };
    let mut export = read(path);
    if let Some(r) = export
        .records
        .iter_mut()
        .rev()
        .find(|r| r.attempt_id == job_id)
    {
        r.mode = mode;
        r.stages_ms.insert(stage, elapsed_ms);
        r.stage_outcomes.insert(stage, outcome);
        let _ = write(path, &export);
    }
}
pub fn flush(id: Uuid) {
    let Some(mut a) = active().lock().ok().and_then(|a| a.get(&id).cloned()) else {
        return;
    };
    if let Ok(t) = traces().lock() {
        if let Some(t) = t.get(&id) {
            project(&mut a.record, &t.events, &t.details);
        }
    }
    let Ok(_guard) = DISK.lock() else {
        return;
    };
    let still_registered = active().lock().ok().is_some_and(|active| {
        active.get(&id).is_some_and(|current| {
            current.record.attempt_id == a.record.attempt_id && current.path == a.path
        })
    });
    if !still_registered {
        return;
    } // History deletion/replacement wins a pending flush.
    let mut export = read(&a.path);
    if let Some(previous) = export.records.iter().find(|r| {
        r.attempt_id == a.record.attempt_id && r.provider_attempt == a.record.provider_attempt
    }) {
        a.record.mode = previous.mode;
        for (stage, ms) in &previous.stages_ms {
            a.record.stages_ms.entry(*stage).or_insert(*ms);
        }
        for (stage, outcome) in &previous.stage_outcomes {
            a.record.stage_outcomes.insert(*stage, *outcome);
            if let Some(ms) = previous.stages_ms.get(stage) {
                a.record.stages_ms.insert(*stage, *ms);
            }
        }
    }
    upsert(&mut export, a.record);
    let _ = write(&a.path, &export); // Never turn measurement failure into dictation failure.
}
fn upsert(export: &mut Export, record: Record) {
    export.records.retain(|r| {
        r.attempt_id != record.attempt_id || r.provider_attempt != record.provider_attempt
    });
    export.records.push(record);
    if export.records.len() > RETAINED_ATTEMPTS {
        export
            .records
            .drain(..export.records.len() - RETAINED_ATTEMPTS);
    }
}
pub fn read(path: &Path) -> Export {
    if fs::metadata(path).map_or(true, |m| m.len() > MAX_BYTES) {
        return Export::default();
    }
    let export: Export = fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // Validate our file on export too. Never echo arbitrary strings/unknown keys.
    if export.records.len() > RETAINED_ATTEMPTS
        || export.records.iter().any(|r| {
            r.model_sha256.len() != 64
                || !r.model_sha256.bytes().all(|b| b.is_ascii_hexdigit())
                || r.stages_ms.values().any(|v| *v > MAX_MS)
                || r.provider_attempt > 3
        })
    {
        return Export::default();
    }
    export
}
fn write(path: &Path, export: &Export) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(export)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(());
    }
    let temp = path.with_extension("latency-tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(windows)]
    {
        let _ = fs::remove_file(path);
    }
    fs::rename(temp, path)
}
pub fn interrupt_job(path: &Path, job_id: Uuid) {
    let Ok(_guard) = DISK.lock() else {
        return;
    };
    let mut export = read(path);
    for r in &mut export.records {
        if r.attempt_id == job_id && matches!(r.outcome, Outcome::Pending) {
            r.outcome = Outcome::Failed;
        }
    }
    let _ = write(path, &export);
}
pub fn erase(path: &Path, recordings: &[Uuid]) {
    let Ok(_guard) = DISK.lock() else {
        return;
    };
    let mut export = read(path);
    export
        .records
        .retain(|r| !recordings.is_empty() && !recordings.contains(&r.recording_id));
    if let Ok(mut a) = active().lock() {
        a.retain(|id, a| a.path != path || (!recordings.is_empty() && !recordings.contains(id)));
    }
    let _ = write(path, &export);
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> Record {
        Record {
            recording_id: Uuid::new_v4(),
            attempt_id: Uuid::new_v4(),
            retry_of: None,
            provider_attempt: 1,
            platform: Platform::Windows,
            provider: ProviderId::Local,
            model_sha256: "a".repeat(64),
            mode: ModeKind::Regular,
            warm: Warm::Cold,
            measurement: Measurement::Synthetic,
            outcome: Outcome::Succeeded,
            stages_ms: BTreeMap::new(),
            stage_outcomes: BTreeMap::new(),
        }
    }
    #[test]
    fn missing_incomplete_and_repeated_stages() {
        let events = [
            ("job_durable", 10),
            ("recognition_started", 15),
            ("final_inference_start", 20),
            ("final_inference_done", 30),
            ("final_inference_start", 35),
            ("final_inference_done", 55),
        ];
        let mut r = record();
        let events: Vec<_> = events
            .into_iter()
            .map(|(stage, elapsed_ms)| Event { stage, elapsed_ms })
            .collect();
        project(&mut r, &events, &BTreeMap::new());
        assert_eq!(r.stages_ms[&Stage::Asr], 30);
        assert_eq!(r.stages_ms[&Stage::Queue], 5);
        assert!(!r.stages_ms.contains_key(&Stage::Insertion));
        let mut events = events;
        events.push(Event {
            stage: "final_inference_start",
            elapsed_ms: 60,
        });
        project(&mut r, &events, &BTreeMap::new());
        assert!(!r.stages_ms.contains_key(&Stage::Asr));
    }
    #[test]
    fn handoff_projection_distinguishes_history_maintenance_and_budget_waits() {
        let mut r = record();
        let events = [
            ("audio_durable", 20),
            ("audio_import_requested", 7020),
            ("audio_import_lock_acquired", 7025),
            ("import_budget_requested", 7030),
            ("import_budget_acquired", 7040),
            ("audio_imported", 7060),
        ]
        .into_iter()
        .map(|(stage, elapsed_ms)| Event { stage, elapsed_ms })
        .collect::<Vec<_>>();
        // Job registration used to erase every import event.
        super::super::with_trace(r.recording_id, |trace| trace.events = events);
        retain_capture(r.recording_id);
        let events = traces().lock().unwrap()[&r.recording_id].events.clone();
        project(&mut r, &events, &BTreeMap::new());
        assert_eq!(r.stages_ms[&Stage::CaptureHandoff], 7000);
        assert_eq!(r.stages_ms[&Stage::NativeImport], 40);
        assert_eq!(r.stages_ms[&Stage::ImportMaintenanceWait], 5);
        assert_eq!(r.stages_ms[&Stage::ImportBudgetWait], 10);
        assert!(!r.stages_ms.contains_key(&Stage::Asr));
    }
    #[test]
    fn bounded_retry_linkage_and_content_exclusion() {
        let path = std::env::temp_dir().join(format!("writer-latency-{}.json", Uuid::new_v4()));
        let mut export = Export::default();
        let original = record();
        let mut retry = original.clone();
        retry.provider_attempt = 2;
        retry.outcome = Outcome::Failed;
        upsert(&mut export, original.clone());
        upsert(&mut export, retry);
        assert_eq!(export.records.len(), 2);
        for _ in 0..300 {
            upsert(&mut export, record());
        }
        assert_eq!(export.records.len(), RETAINED_ATTEMPTS);
        write(&path, &export).unwrap();
        assert_eq!(read(&path).records.len(), RETAINED_ATTEMPTS);
        let mut value = serde_json::to_value(export).unwrap();
        value["records"][0]["raw_text"] = "short secret".into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(read(&path).records.is_empty());
        fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn durable_projection_survives_reload_late_cleanup_and_deletion() {
        use crate::{
            jobs::{JobStore, Pipeline},
            recordings::RecordingStore,
            transcription::ContractCatalog,
        };
        let path =
            std::env::temp_dir().join(format!("writer-latency-lifecycle-{}", Uuid::new_v4()));
        let pipeline = Pipeline::open(path.clone(), 1_000_000, vec![])
            .await
            .unwrap();
        let f: ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        let mut metadata = f.recording.data.metadata;
        metadata.id = Uuid::new_v4();
        let bytes = vec![0; metadata.byte_length as usize];
        pipeline
            .recordings
            .save(metadata.clone(), Box::new(std::io::Cursor::new(bytes)))
            .await
            .unwrap();
        let mut request = f.transcription_request.data;
        request.recording_id = metadata.id;
        request.idempotency_key = Uuid::new_v4();
        super::super::detail(metadata.id, "mode", "command");
        super::super::mark(metadata.id, "stop_accepted");
        for stage in [
            "audio_durable",
            "audio_import_requested",
            "audio_import_lock_acquired",
            "import_budget_requested",
            "import_budget_acquired",
            "audio_imported",
        ] {
            super::super::mark(metadata.id, stage);
        }
        let job = pipeline.jobs.submit(request.clone()).await.unwrap();
        start_attempt(&job, 1);
        super::super::mark(metadata.id, "recognition_started");
        pipeline
            .jobs
            .set_status(job.id, JobStatus::Transcribing {})
            .await
            .unwrap();
        let job = pipeline
            .jobs
            .set_status(job.id, f.job.data.status)
            .await
            .unwrap();
        pipeline.jobs.measure(&job);
        let export = read(&pipeline.jobs.latency_path());
        assert_eq!(export.records.len(), 1);
        assert_eq!(export.records[0].mode, ModeKind::Command);
        assert!(export.records[0]
            .stages_ms
            .contains_key(&Stage::CaptureHandoff));
        assert!(export.records[0]
            .stages_ms
            .contains_key(&Stage::NativeImport));
        assert!(export.records[0]
            .stages_ms
            .contains_key(&Stage::ImportBudgetWait));
        assert!(export.records[0]
            .stages_ms
            .contains_key(&Stage::RawAvailability));
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let response = crate::jobs::routes_with_pipeline(Some(pipeline.clone()))
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/statistics/latency")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["api_version"], "1");
        assert_eq!(value["data"]["records"].as_array().unwrap().len(), 1);
        let serialized = serde_json::to_string(&export).unwrap();
        assert!(!serialized.contains("raw_text"));
        assert!(!serialized.contains("vocabulary"));
        request.idempotency_key = Uuid::new_v4();
        request.retry_of_job_id = Some(job.id);
        let newer = pipeline.jobs.submit(request).await.unwrap();
        record_stage(
            &pipeline.jobs.latency_path(),
            job.id,
            ModeKind::Super,
            Stage::Cleanup,
            57,
            Outcome::Failed,
        );
        flush(metadata.id);
        let export = read(&pipeline.jobs.latency_path());
        let older = export
            .records
            .iter()
            .find(|r| r.attempt_id == job.id)
            .unwrap();
        assert_eq!(older.stages_ms[&Stage::Cleanup], 57);
        assert_eq!(older.mode, ModeKind::Super);
        assert!(export
            .records
            .iter()
            .any(|r| r.attempt_id == newer.id && r.retry_of == Some(job.id)));
        erase(&pipeline.jobs.latency_path(), &[metadata.id]);
        flush(metadata.id);
        assert!(read(&pipeline.jobs.latency_path()).records.is_empty());
        drop(pipeline);
        std::fs::remove_dir_all(path).unwrap();
    }
}
