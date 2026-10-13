use super::*;
use crate::{
    providers::{ProviderInfo, SavedAudio, TranscriptionProvider},
    recordings::RecordingStore,
    transcription::*,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::io::AsyncWriteExt;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer בדיקה space-test-{}", Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> ContractCatalog {
    serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap()
}
async fn saved(pipeline: &Pipeline) -> TranscriptionRequest {
    let f = fixture();
    let mut metadata = f.recording.data.metadata;
    metadata.byte_length = 150_000;
    metadata.duration_ms = 601_000;
    pipeline
        .recordings
        .save(metadata, Box::new(std::io::Cursor::new(vec![42; 150_000])))
        .await
        .unwrap();
    f.transcription_request.data
}
struct Fake {
    calls: Arc<AtomicUsize>,
    panic: bool,
}
#[tokio::test]
async fn saved_raw_job_automatically_runs_its_capture_time_mode() {
    use crate::cleanup::modes::{AttemptState, ContextSnapshot};
    let temp = Temp::new();
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(Fake {
            calls: Arc::new(AtomicUsize::new(0)),
            panic: false,
        })],
    )
    .await
    .unwrap();
    let request = saved(&pipeline).await;
    let modes = pipeline.mode_service().await.unwrap();
    let mut settings = modes.settings().await.unwrap();
    let original = settings.profiles[0].clone();
    modes
        .bind_recording(
            request.recording_id,
            original.clone(),
            ContextSnapshot::default(),
        )
        .await
        .unwrap();
    settings.profiles[0].name = "Changed after capture".into();
    modes.save(settings).await.unwrap();
    let job = pipeline.jobs.submit(request).await.unwrap();
    pipeline.run_pending().await.unwrap();
    let raw = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(raw.status, JobStatus::Succeeded { .. }));
    let completed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let live = modes.live_processing(job.id).await.unwrap();
            if let Some(attempt) = live.attempt {
                if !matches!(attempt.state, AttemptState::Pending) {
                    break attempt;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(completed.state, AttemptState::Succeeded { .. }));
    assert_eq!(completed.profile.name, original.name);
    assert_eq!(
        serde_json::to_value(pipeline.jobs.get(job.id).await.unwrap()).unwrap(),
        serde_json::to_value(raw).unwrap()
    );
    assert!(pipeline
        .recordings
        .audio(job.request.recording_id)
        .await
        .is_ok());
}
#[async_trait]
impl TranscriptionProvider for Fake {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::Openai,
            execution_location: ExecutionLocation::Cloud,
            configured: true,
            available: true,
        }
    }
    async fn transcribe(
        &self,
        audio: &SavedAudio,
        _: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            tokio::fs::read(&audio.path).await.unwrap(),
            vec![42; 150_000]
        );
        assert!(!self.panic, "synthetic worker panic");
        let JobStatus::Succeeded { result } = fixture().job.data.status else {
            panic!()
        };
        Ok(result.transcript)
    }
}
#[tokio::test]
async fn interrupted_upload_is_not_transcribable_and_ordered_extents_reconstruct_exact_audio() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let mut metadata = fixture().recording.data.metadata;
    metadata.byte_length = 150_000;
    metadata.duration_ms = 601_000;
    let id = metadata.id;
    let (reader, mut writer) = tokio::io::duplex(100);
    tokio::spawn(async move {
        writer.write_all(b"partial").await.unwrap();
    });
    assert!(pipeline
        .recordings
        .save(metadata, Box::new(reader))
        .await
        .is_err());
    assert!(matches!(
        pipeline.recordings.get(id).await.unwrap_err().code,
        ErrorCode::RecordingNotFound
    ));
    assert!(matches!(
        pipeline.recordings.audio(id).await.unwrap_err().code,
        ErrorCode::RecordingNotFound
    ));
    drop(pipeline);
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    assert!(matches!(
        pipeline.recordings.get(id).await.unwrap_err().code,
        ErrorCode::RecordingNotFound
    ));
    saved(&pipeline).await;
    let extents = pipeline.recordings.extents(id).await.unwrap();
    let mut end = 0;
    for (sequence, extent) in extents.iter().enumerate() {
        assert_eq!(extent.sequence, sequence as u64);
        assert_eq!(extent.byte_offset, end);
        assert!(extent.byte_length <= 64 * 1024);
        end += extent.byte_length;
    }
    assert_eq!(end, 150_000);
    let duplicate = pipeline
        .recordings
        .save(
            fixture().recording.data.metadata,
            Box::new(std::io::Cursor::new(vec![1; 4096])),
        )
        .await
        .unwrap_err();
    assert!(matches!(duplicate.code, ErrorCode::IdempotencyConflict));
    assert_eq!(
        tokio::fs::read(pipeline.recordings.audio(id).await.unwrap().path)
            .await
            .unwrap(),
        vec![42; 150_000]
    );
}
#[tokio::test]
async fn restart_duplicate_retry_provider_crash_and_raw_preservation() {
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(Fake {
            calls: calls.clone(),
            panic: true,
        })],
    )
    .await
    .unwrap();
    let request = saved(&pipeline).await;
    let (first, duplicate) = tokio::join!(
        pipeline.jobs.submit(request.clone()),
        pipeline.jobs.submit(request.clone())
    );
    let first = first.unwrap();
    assert_eq!(first.id, duplicate.unwrap().id);
    let mut conflict = request.clone();
    conflict.options.selection.model = "another".into();
    assert!(matches!(
        pipeline.jobs.submit(conflict).await.unwrap_err().code,
        ErrorCode::IdempotencyConflict
    ));
    assert!(pipeline
        .jobs
        .delete_recording(request.recording_id)
        .await
        .is_err());
    pipeline
        .jobs
        .set_status(first.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    drop(pipeline);
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(Fake {
            calls: calls.clone(),
            panic: true,
        })],
    )
    .await
    .unwrap();
    assert!(matches!(
        pipeline.jobs.get(first.id).await.unwrap().status,
        JobStatus::Queued {}
    ));
    pipeline.run_pending().await.unwrap();
    let failed = pipeline.jobs.get(first.id).await.unwrap().status;
    pipeline.jobs.set_status(first.id, failed).await.unwrap();
    assert!(matches!(
        pipeline.jobs.get(first.id).await.unwrap().status,
        JobStatus::Failed {
            error: ApiError {
                code: ErrorCode::ProviderFailed,
                ..
            }
        }
    ));
    assert!(pipeline
        .recordings
        .audio(request.recording_id)
        .await
        .is_ok());
    let mut retry = request.clone();
    retry.idempotency_key = Uuid::new_v4();
    retry.retry_of_job_id = Some(first.id);
    retry.options.selection.model = "different-model".into();
    let second = pipeline.jobs.submit(retry.clone()).await.unwrap();
    assert_eq!(pipeline.jobs.submit(retry).await.unwrap().id, second.id);
    drop(pipeline);
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(Fake {
            calls: calls.clone(),
            panic: false,
        })],
    )
    .await
    .unwrap();
    pipeline.run_pending().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let JobStatus::Succeeded { mut result } = pipeline.jobs.get(second.id).await.unwrap().status
    else {
        panic!()
    };
    let original = result.transcript.raw_text.clone();
    result.cleanup = CleanupResult::Failed {
        error: ApiError::unavailable("Cleanup"),
    };
    pipeline
        .jobs
        .set_status(
            second.id,
            JobStatus::Succeeded {
                result: result.clone(),
            },
        )
        .await
        .unwrap();
    result.transcript.raw_text = "changed".into();
    assert!(pipeline
        .jobs
        .set_status(second.id, JobStatus::Succeeded { result })
        .await
        .is_err());
    drop(pipeline);
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let JobStatus::Succeeded { result } = pipeline.jobs.get(second.id).await.unwrap().status else {
        panic!()
    };
    assert_eq!(result.transcript.raw_text, original);
    assert!(matches!(result.cleanup, CleanupResult::Failed { .. }));
    assert!(pipeline.jobs.recoverable().await.unwrap().is_empty());
    pipeline
        .jobs
        .delete_recording(request.recording_id)
        .await
        .unwrap();
    assert!(matches!(
        pipeline
            .recordings
            .get(request.recording_id)
            .await
            .unwrap()
            .audio,
        AudioState::Deleted {}
    ));
    assert!(pipeline
        .recordings
        .audio(request.recording_id)
        .await
        .is_err());
    assert!(matches!(
        pipeline.jobs.get(first.id).await.unwrap().status,
        JobStatus::Failed { .. }
    ));
}
#[tokio::test]
async fn invalid_ids_and_malformed_requests_are_versioned_errors_without_filesystem_access() {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;
    let app = Router::new()
        .merge(crate::recordings::routes())
        .merge(routes());
    for (method, uri) in [
        ("GET", "/api/v1/jobs/not-a-uuid"),
        ("GET", "/api/v1/recordings/%2E%2E%2Fsecret/audio"),
        ("DELETE", "/api/v1/recordings/not-a-uuid"),
        ("POST", "/api/v1/transcriptions"),
        ("POST", "/api/v1/recordings"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "{uri}"
        );
        let error: ErrorEnvelope =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert!(matches!(error.error.code, ErrorCode::InvalidRequest));
    }
}

struct DamageJobFile {
    root: PathBuf,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl TranscriptionProvider for DamageJobFile {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::Openai,
            execution_location: ExecutionLocation::Cloud,
            configured: true,
            available: true,
        }
    }
    async fn transcribe(
        &self,
        _: &SavedAudio,
        _: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut entries = tokio::fs::read_dir(self.root.join("jobs")).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry.path().extension().is_some_and(|e| e == "json") {
                tokio::fs::rename(entry.path(), entry.path().with_extension("backup"))
                    .await
                    .unwrap();
                tokio::fs::create_dir(entry.path()).await.unwrap();
            }
        }
        let JobStatus::Succeeded { result } = fixture().job.data.status else {
            panic!()
        };
        Ok(result.transcript)
    }
}
#[tokio::test]
async fn final_result_write_failure_retries_persistence_without_repeating_provider() {
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(DamageJobFile {
            root: temp.0.clone(),
            calls: calls.clone(),
        })],
    )
    .await
    .unwrap();
    let request = saved(&pipeline).await;
    let job = pipeline.jobs.submit(request).await.unwrap();
    assert!(pipeline.run_pending().await.is_err());
    let path = temp.0.join("jobs").join(format!("{}.json", job.id));
    tokio::fs::remove_dir(&path).await.unwrap();
    tokio::fs::rename(path.with_extension("backup"), &path)
        .await
        .unwrap();
    pipeline.run_pending().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        pipeline.jobs.get(job.id).await.unwrap().status,
        JobStatus::Succeeded { .. }
    ));
}

#[tokio::test]
async fn storage_lock_limits_writers_and_releases_after_shutdown() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    assert!(Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let request = saved(&pipeline).await;
        let job = pipeline.jobs.submit(request).await.unwrap();
        assert_eq!(
            std::fs::metadata(&temp.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(temp.0.join("jobs").join(format!("{}.json", job.id)))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(pipeline);
    assert!(Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .is_ok());
}

#[tokio::test]
async fn bulk_delete_rejects_active_jobs_then_removes_history_and_preserves_settings() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    tokio::fs::write(temp.0.join("settings.json"), b"keep")
        .await
        .unwrap();
    let request = saved(&pipeline).await;
    let job = pipeline.jobs.submit(request).await.unwrap();
    assert!(matches!(
        pipeline
            .jobs
            .delete_history(&pipeline.recordings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyConflict
    ));
    assert!(pipeline
        .recordings
        .audio(job.request.recording_id)
        .await
        .is_ok());
    pipeline.run_pending().await.unwrap();
    assert_eq!(
        pipeline
            .jobs
            .delete_history(&pipeline.recordings)
            .await
            .unwrap(),
        (1, 1)
    );
    assert!(pipeline.jobs.all().await.unwrap().is_empty());
    assert!(pipeline.recordings.list().await.unwrap().is_empty());
    assert_eq!(
        tokio::fs::read(temp.0.join("settings.json")).await.unwrap(),
        b"keep"
    );
    assert_eq!(
        pipeline
            .jobs
            .delete_history(&pipeline.recordings)
            .await
            .unwrap(),
        (0, 0)
    );
}

#[tokio::test]
async fn selected_storage_keeps_text_for_audio_only_and_preserves_unselected_history() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(Fake {
            calls: Arc::new(AtomicUsize::new(0)),
            panic: false,
        })],
    )
    .await
    .unwrap();
    let mut selected = Vec::new();
    for date in [99, 100, 101] {
        let f = fixture();
        let mut metadata = f.recording.data.metadata;
        metadata.id = Uuid::new_v4();
        metadata.created_at_ms = date;
        metadata.byte_length = 100;
        pipeline
            .recordings
            .save(
                metadata.clone(),
                Box::new(std::io::Cursor::new(vec![42; 100])),
            )
            .await
            .unwrap();
        let mut request = f.transcription_request.data;
        request.recording_id = metadata.id;
        request.idempotency_key = Uuid::new_v4();
        let job = pipeline.jobs.submit(request).await.unwrap();
        selected.push((metadata.id, job.id));
    }
    assert!(pipeline
        .clear_selected_storage(&[selected[0].0], false)
        .await
        .is_err());
    assert!(pipeline.recordings.audio(selected[0].0).await.is_ok());
    pipeline.run_pending().await.unwrap();
    let original = pipeline.jobs.get(selected[0].1).await.unwrap();
    pipeline
        .clear_selected_storage(&[selected[0].0], false)
        .await
        .unwrap();
    assert!(matches!(
        pipeline.recordings.get(selected[0].0).await.unwrap().audio,
        AudioState::Deleted {}
    ));
    assert_eq!(
        serde_json::to_value(pipeline.jobs.get(selected[0].1).await.unwrap()).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    // Text can subsequently be erased even though its audio is already gone.
    let modes = temp.0.join("state/modes");
    for (id, job) in &selected {
        tokio::fs::create_dir_all(modes.join(job.to_string()))
            .await
            .unwrap();
        tokio::fs::create_dir_all(modes.join("captures"))
            .await
            .unwrap();
        tokio::fs::write(
            modes.join(job.to_string()).join("synthetic.json"),
            b"private processing history",
        )
        .await
        .unwrap();
        tokio::fs::write(
            modes.join("captures").join(format!("{id}.json")),
            b"private context",
        )
        .await
        .unwrap();
    }
    tokio::fs::write(temp.0.join("dictionary.json"), b"keep")
        .await
        .unwrap();
    pipeline
        .clear_selected_storage(&[selected[0].0], true)
        .await
        .unwrap();
    // Retrying after a WebView cache failure cannot erase a newer recording.
    pipeline
        .clear_selected_storage(&[selected[0].0], true)
        .await
        .unwrap();
    assert!(pipeline.jobs.get(selected[0].1).await.is_err());
    assert!(pipeline.recordings.get(selected[0].0).await.is_err());
    assert!(!modes.join(selected[0].1.to_string()).exists());
    assert!(!modes
        .join("captures")
        .join(format!("{}.json", selected[0].0))
        .exists());
    for (id, job) in &selected[1..] {
        assert!(pipeline.recordings.audio(*id).await.is_ok());
        assert!(pipeline.jobs.get(*job).await.is_ok());
        assert!(modes.join(job.to_string()).exists());
    }
    assert_eq!(
        tokio::fs::read(temp.0.join("dictionary.json"))
            .await
            .unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn bulk_delete_rejects_ingestion_and_clears_abandoned_partial_audio() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let mut metadata = fixture().recording.data.metadata;
    metadata.byte_length = 10;
    let (reader, mut writer) = tokio::io::duplex(64);
    let store = pipeline.recordings.clone();
    let id = metadata.id;
    let task = tokio::spawn(async move { store.save(metadata, Box::new(reader)).await });
    writer.write_all(&[1, 2, 3]).await.unwrap();
    let partial = temp.0.join("recordings").join(format!("{id}.uploading"));
    for _ in 0..100 {
        if partial.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(partial.exists());
    assert!(matches!(
        pipeline
            .jobs
            .delete_history(&pipeline.recordings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::RateLimited
    ));
    drop(writer);
    assert!(task.await.unwrap().is_err());
    assert_eq!(
        pipeline
            .jobs
            .delete_history(&pipeline.recordings)
            .await
            .unwrap(),
        (0, 0)
    );
    assert!(!partial.exists());
}

#[tokio::test]
async fn crashed_preview_copies_are_purged_on_restart_without_touching_other_data() {
    let temp = Temp::new();
    crate::recordings::disk::private_dir(&temp.0.join("previews"))
        .await
        .unwrap();
    tokio::fs::write(
        temp.0.join("previews/crashed.wav"),
        b"private preview fixture",
    )
    .await
    .unwrap();
    tokio::fs::write(temp.0.join("model.bin"), b"keep")
        .await
        .unwrap();
    let reopened = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    assert!(!reopened.preview_root.join("crashed.wav").exists());
    assert_eq!(
        tokio::fs::read(temp.0.join("model.bin")).await.unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn pipeline_history_deletion_removes_mode_attempts_and_preserves_profiles_after_restart() {
    use crate::cleanup::modes::{AttemptState, ContextSnapshot, ProcessRequest};
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let job = pipeline.jobs.submit(saved(&pipeline).await).await.unwrap();
    pipeline
        .jobs
        .set_status(job.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    pipeline
        .jobs
        .set_status(job.id, fixture().job.data.status)
        .await
        .unwrap();
    let service = pipeline.mode_service().await.unwrap();
    let profiles = service.settings().await.unwrap();
    let profiles = service.save(profiles).await.unwrap();
    let regular = profiles
        .profiles
        .iter()
        .find(|p| p.id == "regular")
        .unwrap();
    let request = ProcessRequest {
        operation_id: Uuid::new_v4(),
        profile_id: regular.id.clone(),
        profile_revision: regular.revision,
        context: ContextSnapshot::default(),
    };
    service.start(job.id, request).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let attempts = service.attempts(job.id).await.unwrap();
        if attempts
            .iter()
            .any(|a| matches!(a.state, AttemptState::Succeeded { .. }))
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(temp.0.join("state/modes").join(job.id.to_string()).exists());
    pipeline.delete_history().await.unwrap();
    assert!(!temp.0.join("state/modes").join(job.id.to_string()).exists());
    assert!(pipeline.jobs.all().await.unwrap().is_empty());
    assert_eq!(
        service.settings().await.unwrap().revision,
        profiles.revision
    );
    drop(service);
    drop(pipeline);
    let reopened = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    assert_eq!(
        reopened
            .mode_service()
            .await
            .unwrap()
            .settings()
            .await
            .unwrap()
            .revision,
        profiles.revision
    );
    assert!(!temp.0.join("state/modes").join(job.id.to_string()).exists());
}

#[tokio::test]
async fn indexed_jobs_rebuild_on_restart_and_emit_only_durable_transitions() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let request = saved(&pipeline).await;
    let mut events = pipeline.jobs.subscribe();
    let first = pipeline.jobs.submit(request.clone()).await.unwrap();
    assert_eq!(events.recv().await.unwrap().id, first.id);
    assert_eq!(
        pipeline.jobs.submit(request.clone()).await.unwrap().id,
        first.id
    );
    assert!(
        events.try_recv().is_err(),
        "idempotent submission is not another transition"
    );
    pipeline
        .jobs
        .set_status(first.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert!(matches!(event.status, JobStatus::Transcribing {}));
    assert!(matches!(
        pipeline.jobs.get(first.id).await.unwrap().status,
        JobStatus::Transcribing {}
    ));
    drop(pipeline);
    let reopened = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    assert_eq!(reopened.jobs.submit(request).await.unwrap().id, first.id);
    let pending = reopened.jobs.recoverable().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(matches!(pending[0].status, JobStatus::Queued {}));
    assert!(reopened
        .jobs
        .set_status(
            first.id,
            JobStatus::Succeeded {
                result: match fixture().job.data.status {
                    JobStatus::Succeeded { result } => result,
                    _ => unreachable!(),
                }
            }
        )
        .await
        .is_err());
}

#[tokio::test]
async fn completed_history_bodies_are_not_rescanned_for_new_work() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let request = saved(&pipeline).await;
    let first = pipeline.jobs.submit(request.clone()).await.unwrap();
    pipeline
        .jobs
        .set_status(first.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    pipeline
        .jobs
        .set_status(first.id, fixture().job.data.status)
        .await
        .unwrap();
    // A sentinel proves the hot path does not deserialize unrelated completed
    // history. Explicit history reads still surface the disk corruption.
    tokio::fs::write(
        temp.0.join("jobs").join(format!("{}.json", first.id)),
        b"unreadable completed history sentinel",
    )
    .await
    .unwrap();
    assert!(pipeline.jobs.recoverable().await.unwrap().is_empty());
    let mut next = request;
    next.idempotency_key = Uuid::new_v4();
    let second = pipeline.jobs.submit(next).await.unwrap();
    assert_eq!(pipeline.jobs.recoverable().await.unwrap()[0].id, second.id);
    assert!(pipeline.jobs.all().await.is_err());
}

struct TransientFake {
    calls: Arc<AtomicUsize>,
    fail_count: usize,
    failure: ApiError,
}
#[async_trait]
impl TranscriptionProvider for TransientFake {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::Openai,
            execution_location: ExecutionLocation::Cloud,
            configured: true,
            available: true,
        }
    }
    async fn transcribe(
        &self,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.fail_count {
            return Err(self.failure.clone());
        }
        Fake {
            calls: Arc::new(AtomicUsize::new(0)),
            panic: false,
        }
        .transcribe(audio, options)
        .await
    }
}
fn transient_error() -> ApiError {
    crate::recordings::disk::error(
        ErrorCode::ProviderTimeout,
        "Synthetic transient timeout",
        true,
    )
}
async fn transient_pipeline(
    root: PathBuf,
    calls: Arc<AtomicUsize>,
    fail_count: usize,
    failure: ApiError,
) -> Arc<Pipeline> {
    Pipeline::open(
        root,
        1_000_000,
        vec![Arc::new(TransientFake {
            calls,
            fail_count,
            failure,
        })],
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn recognition_has_capacity_when_primary_and_backup_text_are_running() {
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = transient_pipeline(temp.0.clone(), calls.clone(), 0, transient_error()).await;
    let primary = pipeline.work.admit().unwrap();
    let backup = pipeline.work.admit().unwrap();
    let request = saved(&pipeline).await;
    let job = pipeline.jobs.submit(request).await.unwrap();
    pipeline.run_pending().await.unwrap();
    let completed = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(completed.status, JobStatus::Succeeded { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.recovery.unwrap().attempts_started, 1);
    drop((primary, backup));
}

#[tokio::test]
async fn full_global_capacity_leaves_saved_recognition_queued_without_a_provider_attempt() {
    use crate::limits::{storage::StorageBudget, work::WorkCapacity, Limits};
    let temp = Temp::new();
    let limits = Limits {
        work_concurrency: 2,
        ..Limits::default()
    };
    let work = WorkCapacity::new(limits.clone()).account();
    let primary = work.admit().unwrap();
    let backup = work.admit().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = Pipeline::open_shared(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(Fake {
            calls: calls.clone(),
            panic: false,
        })],
        temp.0.join("modes"),
        StorageBudget::new(temp.0.clone(), limits),
        Some(work),
    )
    .await
    .unwrap();
    let request = saved(&pipeline).await;
    let job = pipeline.jobs.submit(request.clone()).await.unwrap();
    pipeline.run_pending().await.unwrap();
    let waiting = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(waiting.status, JobStatus::Queued {}));
    let recovery = waiting.recovery.unwrap();
    assert!(recovery.waiting_for_capacity);
    assert_eq!(recovery.attempts_started, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut manual = request;
    manual.retry_of_job_id = Some(job.id);
    manual.idempotency_key = Uuid::new_v4();
    assert!(matches!(
        pipeline.jobs.submit(manual).await.unwrap_err().code,
        ErrorCode::IdempotencyConflict
    ));
    drop(primary);
    pipeline.run_pending().await.unwrap();
    assert!(matches!(
        pipeline.jobs.get(job.id).await.unwrap().status,
        JobStatus::Succeeded { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(backup);
}

#[tokio::test]
async fn automatic_retry_is_durable_and_resumes_the_same_job_after_restart() {
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = transient_pipeline(temp.0.clone(), calls.clone(), 1, transient_error()).await;
    let job = pipeline.jobs.submit(saved(&pipeline).await).await.unwrap();
    pipeline.run_pending().await.unwrap();
    let queued = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(queued.status, JobStatus::Queued {}));
    let recovery = queued.recovery.as_ref().unwrap();
    assert_eq!(recovery.attempts_started, 1);
    assert_eq!(recovery.failures.len(), 1);
    assert!(recovery.next_retry_at_ms.unwrap() >= recovery.failures[0].at_ms + 2000);
    drop(pipeline);
    let pipeline = transient_pipeline(temp.0.clone(), calls.clone(), 1, transient_error()).await;
    assert_eq!(
        serde_json::to_value(pipeline.jobs.get(job.id).await.unwrap()).unwrap(),
        serde_json::to_value(&queued).unwrap()
    );
    pipeline.run_pending().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    pipeline.run_pending().await.unwrap();
    let completed = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(completed.status, JobStatus::Succeeded { .. }));
    assert_eq!(completed.recovery.unwrap().attempts_started, 2);
    pipeline.run_pending().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        tokio::fs::read(
            pipeline
                .recordings
                .audio(job.request.recording_id)
                .await
                .unwrap()
                .path
        )
        .await
        .unwrap(),
        vec![42; 150_000]
    );
}

#[tokio::test]
async fn only_two_automatic_retries_are_allowed_and_exhaustion_survives_restart() {
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline =
        transient_pipeline(temp.0.clone(), calls.clone(), usize::MAX, transient_error()).await;
    let job = pipeline.jobs.submit(saved(&pipeline).await).await.unwrap();
    pipeline.run_pending().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    pipeline.run_pending().await.unwrap();
    let queued = pipeline.jobs.get(job.id).await.unwrap();
    let recovery = queued.recovery.unwrap();
    assert_eq!(recovery.attempts_started, 2);
    assert!(recovery.next_retry_at_ms.unwrap() >= recovery.failures[1].at_ms + 5000);
    tokio::time::sleep(std::time::Duration::from_millis(5100)).await;
    pipeline.run_pending().await.unwrap();
    let failed = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(failed.status, JobStatus::Failed { .. }));
    assert_eq!(failed.recovery.unwrap().failures.len(), 3);
    drop(pipeline);
    let pipeline =
        transient_pipeline(temp.0.clone(), calls.clone(), usize::MAX, transient_error()).await;
    pipeline.run_pending().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_after_is_respected_and_cancel_stops_a_scheduled_retry() {
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut failure =
        crate::recordings::disk::error(ErrorCode::RateLimited, "Synthetic provider throttle", true);
    failure.retry_after_ms = Some(30_000);
    let pipeline = transient_pipeline(temp.0.clone(), calls.clone(), 1, failure).await;
    let job = pipeline.jobs.submit(saved(&pipeline).await).await.unwrap();
    pipeline.run_pending().await.unwrap();
    let queued = pipeline.jobs.get(job.id).await.unwrap();
    let recovery = queued.recovery.unwrap();
    assert!(recovery.next_retry_at_ms.unwrap() >= recovery.failures[0].at_ms + 30_000);
    let canceled = pipeline.jobs.cancel_retries(job.id).await.unwrap();
    assert!(matches!(canceled.status, JobStatus::Failed { .. }));
    assert!(canceled.recovery.unwrap().canceled);
    drop(pipeline);
    let pipeline = transient_pipeline(temp.0.clone(), calls.clone(), 1, transient_error()).await;
    pipeline.run_pending().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(pipeline
        .recordings
        .audio(job.request.recording_id)
        .await
        .is_ok());
}

#[tokio::test]
async fn configuration_credentials_audio_and_storage_errors_do_not_retry_automatically() {
    for code in [
        ErrorCode::ProviderUnavailable,
        ErrorCode::AuthenticationFailed,
        ErrorCode::InvalidRequest,
        ErrorCode::UnsupportedCapability,
        ErrorCode::StorageFailed,
        ErrorCode::InvalidProviderResponse,
    ] {
        let temp = Temp::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let pipeline = transient_pipeline(
            temp.0.clone(),
            calls.clone(),
            usize::MAX,
            crate::recordings::disk::error(code, "Synthetic nontransient failure", true),
        )
        .await;
        let job = pipeline.jobs.submit(saved(&pipeline).await).await.unwrap();
        pipeline.run_pending().await.unwrap();
        assert!(matches!(
            pipeline.jobs.get(job.id).await.unwrap().status,
            JobStatus::Failed { .. }
        ));
        pipeline.run_pending().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn legacy_exhausted_or_corrupt_budget_does_not_block_saved_audio_after_restart() {
    let exhausted = serde_json::json!({
        "since": crate::jobs::disk::now(),
        "total": { "calls": u64::MAX, "bytes": u64::MAX },
        "owners": { "owner": { "calls": u64::MAX, "bytes": u64::MAX } }
    })
    .to_string();
    for legacy in [exhausted, "invalid legacy ledger".into()] {
        let temp = Temp::new();
        tokio::fs::create_dir_all(&temp.0).await.unwrap();
        let ledger = temp.0.join("work-budget.json");
        tokio::fs::write(&ledger, &legacy).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let pipeline = Pipeline::open(
            temp.0.clone(),
            1_000_000,
            vec![Arc::new(Fake {
                calls: calls.clone(),
                panic: false,
            })],
        )
        .await
        .unwrap();
        // Simulate a recording saved by the old app, then reopen with the new app.
        let request = saved(&pipeline).await;
        drop(pipeline);
        let pipeline = Pipeline::open(
            temp.0.clone(),
            1_000_000,
            vec![Arc::new(Fake {
                calls: calls.clone(),
                panic: false,
            })],
        )
        .await
        .unwrap();
        for _ in 0..501 {
            drop(pipeline.work.admit().unwrap());
        }
        let job = pipeline.jobs.submit(request.clone()).await.unwrap();
        pipeline.run_pending().await.unwrap();
        let completed = pipeline.jobs.get(job.id).await.unwrap();
        assert!(matches!(completed.status, JobStatus::Succeeded { .. }));
        assert_eq!(completed.recovery.unwrap().attempts_started, 1);
        pipeline.run_pending().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(pipeline
            .recordings
            .audio(request.recording_id)
            .await
            .is_ok());
        assert_eq!(tokio::fs::read_to_string(ledger).await.unwrap(), legacy);
    }
}

struct InFlightFake {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl TranscriptionProvider for InFlightFake {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::Openai,
            execution_location: ExecutionLocation::Cloud,
            configured: true,
            available: true,
        }
    }
    async fn transcribe(
        &self,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        self.entered.notify_one();
        self.release.notified().await;
        Fake {
            calls: Arc::new(AtomicUsize::new(0)),
            panic: false,
        }
        .transcribe(audio, options)
        .await
    }
}
#[tokio::test]
async fn cancellation_during_inference_keeps_the_result_and_never_revives_retries() {
    let temp = Temp::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(InFlightFake {
            entered: entered.clone(),
            release: release.clone(),
        })],
    )
    .await
    .unwrap();
    let job = pipeline.jobs.submit(saved(&pipeline).await).await.unwrap();
    let runner = pipeline.clone();
    let task = tokio::spawn(async move { runner.run_pending().await });
    entered.notified().await;
    let canceled = pipeline.jobs.cancel_retries(job.id).await.unwrap();
    assert!(matches!(canceled.status, JobStatus::Transcribing { .. }));
    assert!(canceled.recovery.unwrap().canceled);
    assert!(pipeline
        .jobs
        .delete_recording(job.request.recording_id)
        .await
        .is_err());
    release.notify_one();
    task.await.unwrap().unwrap();
    let completed = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(completed.status, JobStatus::Succeeded { .. }));
    assert!(completed.recovery.unwrap().canceled);
    assert!(pipeline.jobs.recoverable().await.unwrap().is_empty());
    assert!(pipeline
        .recordings
        .audio(job.request.recording_id)
        .await
        .is_ok());
}

struct RestartingFake {
    provider: TransientFake,
    ready: Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait]
impl TranscriptionProvider for RestartingFake {
    fn info(&self) -> ProviderInfo {
        self.provider.info()
    }
    fn automatic_retry_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
    async fn transcribe(
        &self,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        self.provider.transcribe(audio, options).await
    }
}
#[tokio::test]
async fn a_worker_restart_wait_does_not_spend_automatic_attempts() {
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(RestartingFake {
            provider: TransientFake {
                calls: calls.clone(),
                fail_count: 1,
                failure: transient_error(),
            },
            ready: ready.clone(),
        })],
    )
    .await
    .unwrap();
    let job = pipeline.jobs.submit(saved(&pipeline).await).await.unwrap();
    pipeline.run_pending().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    pipeline.run_pending().await.unwrap();
    let waiting = pipeline.jobs.get(job.id).await.unwrap();
    assert!(matches!(waiting.status, JobStatus::Queued { .. }));
    assert_eq!(waiting.recovery.unwrap().attempts_started, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    ready.store(true, Ordering::SeqCst);
    pipeline.run_pending().await.unwrap();
    assert!(matches!(
        pipeline.jobs.get(job.id).await.unwrap().status,
        JobStatus::Succeeded { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn audio_preparation_survives_provider_failure_and_restart() {
    let temp = Temp::new();
    let pipeline = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let request = saved(&pipeline).await;
    let job = pipeline.jobs.submit(request.clone()).await.unwrap();
    assert!(job.audio_preparation.is_none());
    pipeline
        .jobs
        .set_status(job.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    let input = vec![0; 6 * 32_000];
    let copy = crate::local_stt::silence::prepare(request.recording_id, &input);
    assert_eq!(copy.len(), 32_000);
    pipeline
        .jobs
        .set_status(
            job.id,
            JobStatus::Failed {
                error: ApiError::unavailable("Provider"),
            },
        )
        .await
        .unwrap();
    drop(pipeline);
    let reopened = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let measured = reopened
        .jobs
        .get(job.id)
        .await
        .unwrap()
        .audio_preparation
        .unwrap();
    assert_eq!(
        (
            measured.input_bytes,
            measured.output_bytes,
            measured.removed_ms
        ),
        (192_000, 32_000, 5000)
    );
    assert_eq!(
        std::fs::read(
            reopened
                .recordings
                .audio(request.recording_id)
                .await
                .unwrap()
                .path
        )
        .unwrap(),
        vec![42; 150_000]
    );
}

#[tokio::test]
async fn pending_native_context_does_not_stall_the_following_raw_job() {
    use crate::cleanup::modes::ModeSettings;
    let temp = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = Pipeline::open(
        temp.0.clone(),
        1_000_000,
        vec![Arc::new(Fake {
            calls: calls.clone(),
            panic: false,
        })],
    )
    .await
    .unwrap();
    let first = saved(&pipeline).await;
    let modes = pipeline.mode_service().await.unwrap();
    let profile = ModeSettings::default().profiles.remove(2); // unverified Command selection
    modes
        .begin_context_capture(first.recording_id, profile)
        .await
        .unwrap();
    let first_job = pipeline.jobs.submit(first).await.unwrap();
    let mut catalog = fixture();
    let next_id = Uuid::new_v4();
    catalog.recording.data.metadata.id = next_id;
    catalog.recording.data.metadata.byte_length = 150_000;
    pipeline
        .recordings
        .save(
            catalog.recording.data.metadata,
            Box::new(std::io::Cursor::new(vec![42; 150_000])),
        )
        .await
        .unwrap();
    catalog.transcription_request.data.recording_id = next_id;
    catalog.transcription_request.data.idempotency_key = Uuid::new_v4();
    let second_job = pipeline
        .jobs
        .submit(catalog.transcription_request.data)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), pipeline.run_pending())
        .await
        .expect("pending context must not hold the raw queue")
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    for id in [first_job.id, second_job.id] {
        assert!(matches!(
            pipeline.jobs.get(id).await.unwrap().status,
            JobStatus::Succeeded { .. }
        ));
    }
    assert!(
        modes
            .captured_mode(first_job.request.recording_id)
            .await
            .unwrap()
            .unwrap()
            .context_pending
    );
    // Resolve and settle the owned background worker before deleting test data.
    modes
        .complete_context_capture(first_job.request.recording_id, None)
        .await
        .unwrap();
    assert!(modes.delivery_text(first_job.id).await.is_err());
}
