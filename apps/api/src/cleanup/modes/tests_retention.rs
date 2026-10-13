use super::*;

const APP: &str = "SYNTHETIC-APP-SEC14-931f";
const SELECTED: &str = "SYNTHETIC-SELECTION-SEC14-931f";
const CLIPBOARD: &str = "SYNTHETIC-CLIPBOARD-SEC14-931f";

fn context() -> ContextSnapshot {
    ContextSnapshot {
        application: APP.into(),
        selected_text: SELECTED.into(),
        clipboard: CLIPBOARD.into(),
    }
}
fn disk_contains(root: &std::path::Path, marker: &str) -> bool {
    std::fs::read_dir(root).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            disk_contains(&path, marker)
        } else if std::fs::metadata(&path).unwrap().len() == 0 {
            // Empty files cannot contain context. In particular Windows refuses
            // reads of the pipeline's exclusively held, empty writer.lock.
            false
        } else {
            String::from_utf8_lossy(&std::fs::read(path).unwrap()).contains(marker)
        }
    })
}
struct Fixed;
#[async_trait::async_trait]
impl processing::Processor for Fixed {
    async fn generate(&self, _: &ModelSelection, _: &str, input: &str) -> Result<String, ApiError> {
        for marker in [APP, SELECTED, CLIPBOARD] {
            assert!(input.contains(marker));
        }
        Ok(serde_json::json!({"text":"Synthetic edited result"}).to_string())
    }
}
async fn bind(service: &ModeService, recording: Uuid) {
    let mut settings = service.settings().await.unwrap();
    let profile = settings
        .profiles
        .iter_mut()
        .find(|p| p.id == "command")
        .unwrap();
    profile.context.application = true;
    profile.context.clipboard = true;
    let settings = service.save(settings).await.unwrap();
    let profile = settings
        .profiles
        .into_iter()
        .find(|p| p.id == "command")
        .unwrap();
    service
        .bind_recording(recording, profile, context())
        .await
        .unwrap();
}

#[tokio::test]
async fn success_erases_both_context_copies_and_replays_without_context_after_restart() {
    let root = Temp::new();
    let (service, pipeline, job, _) = fixture(&root, false).await;
    bind(&service, job.request.recording_id).await;
    let service =
        ModeService::open_with(root.0.join("modes"), pipeline.jobs.clone(), Arc::new(Fixed))
            .await
            .unwrap();
    assert!(disk_contains(&root.0, CLIPBOARD));
    let pending = service.start_recorded(job.id).await.unwrap().unwrap();
    let original_request = pending.request.clone();
    done(&service, job.id, pending.id).await;
    for marker in [APP, SELECTED, CLIPBOARD] {
        assert!(!disk_contains(&root.0, marker));
    }
    let reopened =
        ModeService::open_with(root.0.join("modes"), pipeline.jobs.clone(), Arc::new(Fixed))
            .await
            .unwrap();
    assert!(matches!(
        reopened
            .start_recorded(job.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Succeeded { .. }
    ));
    assert!(reopened
        .start(job.id, original_request.clone())
        .await
        .is_ok());
    let mut new_job = job.request.clone();
    new_job.idempotency_key = Uuid::new_v4();
    let new_job = pipeline.jobs.submit(new_job).await.unwrap();
    pipeline
        .jobs
        .set_status(new_job.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    pipeline
        .jobs
        .set_status(new_job.id, job.status.clone())
        .await
        .unwrap();
    let refusal = reopened.start_recorded(new_job.id).await.unwrap().unwrap();
    assert!(
        matches!(refusal.state, AttemptState::Failed { ref error } if error.message.contains("fresh context"))
    );
    assert!(matches!(
        reopened
            .live_processing(new_job.id)
            .await
            .unwrap()
            .attempt
            .unwrap()
            .state,
        AttemptState::Failed { .. }
    ));
    let mut changed = original_request;
    changed.context.selected_text = "Different synthetic selection".into();
    assert!(matches!(
        reopened.start(job.id, changed).await.err().unwrap().code,
        ErrorCode::IdempotencyConflict
    ));
    assert!(matches!(
        pipeline.jobs.get(job.id).await.unwrap().status,
        JobStatus::Succeeded { .. }
    ));
    assert!(pipeline
        .recordings
        .audio(job.request.recording_id)
        .await
        .is_ok());
}

#[tokio::test]
async fn failed_command_keeps_original_context_for_a_new_job_retry_across_restart() {
    let root = Temp::new();
    let (service, pipeline, job, fake) = fixture(&root, true).await;
    bind(&service, job.request.recording_id).await;
    let pending = service.start_recorded(job.id).await.unwrap().unwrap();
    assert!(matches!(
        done(&service, job.id, pending.id).await.state,
        AttemptState::Failed { .. }
    ));
    let reopened = ModeService::open_with(root.0.join("modes"), pipeline.jobs.clone(), fake)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .captured_mode(job.request.recording_id)
            .await
            .unwrap()
            .unwrap()
            .context
            .selected_text,
        SELECTED
    );
    let mut request = job.request.clone();
    request.idempotency_key = Uuid::new_v4();
    let retry = pipeline.jobs.submit(request).await.unwrap();
    pipeline
        .jobs
        .set_status(retry.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    pipeline
        .jobs
        .set_status(retry.id, job.status.clone())
        .await
        .unwrap();
    let retry = reopened.start_recorded(retry.id).await.unwrap().unwrap();
    assert_eq!(retry.request.context.clipboard, CLIPBOARD);
    done(&reopened, job.id, pending.id).await;
    done(&reopened, retry.job_id, retry.id).await;
    for marker in [APP, SELECTED, CLIPBOARD] {
        assert!(disk_contains(&root.0, marker));
    }
    // Delete-audio keeps retry input and independent raw text.
    pipeline
        .jobs
        .delete_recording(job.request.recording_id)
        .await
        .unwrap();
    assert!(disk_contains(&root.0, SELECTED));
    assert!(pipeline.jobs.get(job.id).await.is_ok());
    reopened
        .begin_history_deletion()
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    for marker in [APP, SELECTED, CLIPBOARD] {
        assert!(!disk_contains(&root.0, marker));
    }
}

#[tokio::test]
async fn cancel_orphan_and_interrupted_atomic_writes_are_cleaned_without_losing_retry_audio() {
    let root = Temp::new();
    let (service, pipeline, job, fake) = fixture(&root, true).await;
    let canceled = Uuid::new_v4();
    bind(&service, canceled).await;
    service.discard_capture(canceled).await.unwrap();
    assert!(!disk_contains(&root.0, CLIPBOARD));
    let orphan = Uuid::new_v4();
    bind(&service, orphan).await;
    bind(&service, job.request.recording_id).await;
    let scratch = root.0.join("modes/captures/abandoned.tmp");
    std::fs::write(&scratch, "SYNTHETIC-ABANDONED-WRITE").unwrap();
    let reopened = ModeService::open_with(root.0.join("modes"), pipeline.jobs.clone(), fake)
        .await
        .unwrap();
    reopened
        .reconcile_captures(&[job.request.recording_id].into_iter().collect())
        .await
        .unwrap();
    assert!(!scratch.exists());
    assert!(reopened.captured_mode(orphan).await.unwrap().is_none());
    assert!(reopened
        .captured_mode(job.request.recording_id)
        .await
        .unwrap()
        .is_some());
    assert!(pipeline
        .recordings
        .audio(job.request.recording_id)
        .await
        .is_ok());
    reopened
        .begin_history_deletion()
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    for marker in [APP, SELECTED, CLIPBOARD] {
        assert!(!disk_contains(&root.0, marker));
    }
}

#[tokio::test]
async fn startup_migrates_legacy_success_and_preserves_interrupted_attempt_context() {
    let root = Temp::new();
    let (service, pipeline, job, fake) = fixture(&root, true).await;
    bind(&service, job.request.recording_id).await;
    let pending = service.start_recorded(job.id).await.unwrap().unwrap();
    let mut legacy = done(&service, job.id, pending.id).await;
    legacy.state = AttemptState::Pending;
    let path = root
        .0
        .join("modes")
        .join(job.id.to_string())
        .join(format!("{}.json", pending.id));
    crate::recordings::disk::write_json(&path, &legacy)
        .await
        .unwrap();
    let restarted =
        ModeService::open_with(root.0.join("modes"), pipeline.jobs.clone(), fake.clone())
            .await
            .unwrap();
    let interrupted = restarted.attempts(job.id).await.unwrap().remove(0);
    assert!(matches!(interrupted.state, AttemptState::Failed { .. }));
    assert_eq!(interrupted.request.context.selected_text, SELECTED);
    legacy.state = AttemptState::Succeeded {
        text: "Synthetic result".into(),
    };
    crate::recordings::disk::write_json(&path, &legacy)
        .await
        .unwrap();
    ModeService::open_with(root.0.join("modes"), pipeline.jobs.clone(), fake)
        .await
        .unwrap();
    for marker in [APP, SELECTED, CLIPBOARD] {
        assert!(!disk_contains(&root.0, marker));
    }
}

struct BlockedProcessor(tokio::sync::Notify);
#[async_trait::async_trait]
impl processing::Processor for BlockedProcessor {
    async fn generate(&self, _: &ModelSelection, _: &str, _: &str) -> Result<String, ApiError> {
        self.0.notified().await;
        Err(processing::invalid("Synthetic blocked-provider failure"))
    }
}

#[tokio::test]
async fn explicit_cancel_waits_for_processing_then_erases_every_context_copy() {
    let root = Temp::new();
    let (_, pipeline, job, _) = fixture(&root, true).await;
    let blocked = Arc::new(BlockedProcessor(tokio::sync::Notify::new()));
    let service =
        ModeService::open_with(root.0.join("modes"), pipeline.jobs.clone(), blocked.clone())
            .await
            .unwrap();
    bind(&service, job.request.recording_id).await;
    let captured = service
        .captured_mode(job.request.recording_id)
        .await
        .unwrap()
        .unwrap();
    let pending = service.start_recorded(job.id).await.unwrap().unwrap();
    assert!(service
        .discard_recording_context(job.request.recording_id, &[job.id])
        .await
        .is_err());
    blocked.0.notify_one();
    done(&service, job.id, pending.id).await;
    let temp = root
        .0
        .join("modes")
        .join(job.id.to_string())
        .join("abandoned.tmp");
    std::fs::write(temp, CLIPBOARD).unwrap();
    service
        .discard_recording_context(job.request.recording_id, &[job.id])
        .await
        .unwrap();
    for marker in [APP, SELECTED, CLIPBOARD] {
        assert!(!disk_contains(&root.0, marker));
    }
    assert!(service
        .captured_mode(job.request.recording_id)
        .await
        .unwrap()
        .is_none());
    // A caller that read the capture just before cancellation cannot restore it.
    let mut late = pending.request;
    late.operation_id = Uuid::new_v4();
    assert!(service
        .start_with_profile(job.id, late, Some(captured.profile))
        .await
        .is_err());
    assert!(pipeline.jobs.get(job.id).await.is_ok());
    assert!(pipeline
        .recordings
        .audio(job.request.recording_id)
        .await
        .is_ok());
}
