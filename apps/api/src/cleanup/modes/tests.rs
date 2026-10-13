use super::*;
use crate::{
    jobs::{JobStore, Pipeline},
    recordings::RecordingStore,
    transcription::*,
    vocabulary::empty_vocabulary,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer-modes-{}", Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Fake {
    calls: AtomicUsize,
    inputs: Mutex<Vec<String>>,
    fail: bool,
    warmed: std::sync::Mutex<Vec<ModelSelection>>,
}
#[async_trait::async_trait]
impl processing::Processor for Fake {
    fn warm(&self, model: &ModelSelection) {
        self.warmed.lock().unwrap().push(model.clone());
    }
    async fn generate(&self, _: &ModelSelection, _: &str, input: &str) -> Result<String, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs.lock().await.push(input.into());
        tokio::time::sleep(Duration::from_millis(30)).await;
        if self.fail {
            return Err(processing::invalid("fixture failure"));
        }
        let input: serde_json::Value = serde_json::from_str(input).unwrap();
        if let Some(spans) = input["editable_spans"].as_array() {
            return Ok(serde_json::json!({"spans": spans}).to_string());
        }
        let text = if input["Selected Text"].as_str().unwrap().is_empty() {
            input["User Message"].as_str().unwrap()
        } else {
            input["Selected Text"].as_str().unwrap()
        };
        Ok(serde_json::json!({"text": format!("{text}.")}).to_string())
    }
}
async fn fixture(root: &Temp, fail: bool) -> (Arc<ModeService>, Arc<Pipeline>, Job, Arc<Fake>) {
    fixture_with_raw(root, fail, "שלום hello signature").await
}

#[tokio::test]
async fn empty_dictation_never_polishes_or_edits_selected_text() {
    let root = Temp::new();
    let (service, _, job, fake) = fixture_with_raw(&root, false, " \n").await;
    for mode in ["regular", "super", "command"] {
        let mut request = request(&service, mode).await;
        if mode == "command" {
            request.context.selected_text = "Selected text must stay unchanged".into();
        }
        let attempt = service.start(job.id, request.clone()).await.unwrap();
        let output = processing::run(&attempt, &empty_vocabulary(), &*fake)
            .await
            .unwrap();
        assert_eq!(output, "");
        done(&service, job.id, request.operation_id).await;
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}
async fn fixture_with_raw(
    root: &Temp,
    fail: bool,
    raw: &str,
) -> (Arc<ModeService>, Arc<Pipeline>, Job, Arc<Fake>) {
    fixture_with_raw_and_snippet(root, fail, raw, "PRIVATE EXACT VALUE").await
}
async fn fixture_with_raw_and_snippet(
    root: &Temp,
    fail: bool,
    raw: &str,
    exact: &str,
) -> (Arc<ModeService>, Arc<Pipeline>, Job, Arc<Fake>) {
    let p = Pipeline::open(root.0.join("data"), 1_000_000, vec![])
        .await
        .unwrap();
    let f: ContractCatalog = serde_json::from_str(include_str!(
        "../../../../../packages/contracts/examples.json"
    ))
    .unwrap();
    let mut metadata = f.recording.data.metadata;
    metadata.byte_length = 16;
    p.recordings
        .save(metadata, Box::new(std::io::Cursor::new(vec![42; 16])))
        .await
        .unwrap();
    let mut request = f.transcription_request.data;
    request.options.vocabulary = Vocabulary {
        snippets: vec![Snippet {
            phrase: "signature".into(),
            text: exact.into(),
        }],
        ..empty_vocabulary()
    };
    let job = p.jobs.submit(request).await.unwrap();
    p.jobs
        .set_status(job.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    let JobStatus::Succeeded { mut result } = f.job.data.status else {
        panic!()
    };
    result.transcript.raw_text = raw.into();
    let job = p
        .jobs
        .set_status(job.id, JobStatus::Succeeded { result })
        .await
        .unwrap();
    let fake = Arc::new(Fake {
        calls: AtomicUsize::new(0),
        inputs: Mutex::new(vec![]),
        fail,
        warmed: Default::default(),
    });
    let service = ModeService::open_with(root.0.join("modes"), p.jobs.clone(), fake.clone())
        .await
        .unwrap();
    let mut settings = service.settings().await.unwrap();
    for profile in &mut settings.profiles {
        if profile.kind != ModeKind::Regular {
            profile.text_model = Some(ModelSelection {
                provider: ProviderId::Openai,
                model: "gpt-4.1-mini".into(),
            });
        }
    }
    service.save(settings).await.unwrap();
    (service, p, job, fake)
}
async fn request(service: &ModeService, id: &str) -> ProcessRequest {
    let settings = service.settings().await.unwrap();
    ProcessRequest {
        operation_id: Uuid::new_v4(),
        profile_id: id.into(),
        profile_revision: settings
            .profiles
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .revision,
        context: ContextSnapshot::default(),
    }
}
#[tokio::test]
async fn oversized_text_processing_has_a_saved_failure_while_regular_remains_usable() {
    let root = Temp::new();
    let raw = "hello ".repeat(17_000);
    let (service, pipeline, job, fake) = fixture_with_raw(&root, false, &raw).await;
    for profile in ["super", "regular"] {
        let attempt = service
            .start(job.id, request(&service, profile).await)
            .await
            .unwrap();
        let attempt = done(&service, job.id, attempt.id).await;
        assert!(matches!(attempt.state, AttemptState::Succeeded { ref text } if text == &raw));
        assert_eq!(attempt.diagnostics.original_used, profile == "super");
        if profile == "super" {
            assert!(matches!(
                attempt.diagnostics.original_reason,
                Some(ErrorCode::InvalidRequest)
            ));
        }
        assert_eq!(attempt.raw_text, raw);
    }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        serde_json::to_value(pipeline.jobs.get(job.id).await.unwrap()).unwrap(),
        serde_json::to_value(job).unwrap()
    );
}
#[tokio::test]
async fn only_a_new_binding_with_a_text_model_warms_the_provider() {
    let root = Temp::new();
    let (service, _, _, fake) = fixture(&root, false).await;
    let settings = service.settings().await.unwrap();
    let profile = |id: &str| {
        settings
            .profiles
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .clone()
    };
    let regular = Uuid::new_v4();
    service
        .bind_recording(regular, profile("regular"), ContextSnapshot::default())
        .await
        .unwrap();
    assert!(fake.warmed.lock().unwrap().is_empty());
    let super_recording = Uuid::new_v4();
    for _ in 0..2 {
        // The second, identical binding is the idempotent replay of the first.
        service
            .bind_recording(
                super_recording,
                profile("super"),
                ContextSnapshot::default(),
            )
            .await
            .unwrap();
    }
    assert_eq!(fake.warmed.lock().unwrap().len(), 1);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn capture_snapshot_survives_profile_edits_and_processing_reuses_its_identity() {
    let root = Temp::new();
    let (service, p, job, fake) = fixture(&root, false).await;
    let before = service.settings().await.unwrap();
    let profile = before
        .profiles
        .iter()
        .find(|p| p.id == "super")
        .unwrap()
        .clone();
    service
        .bind_recording(
            job.request.recording_id,
            profile.clone(),
            ContextSnapshot {
                clipboard: "not enabled".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut edited = before.clone();
    edited
        .profiles
        .iter_mut()
        .find(|p| p.id == "super")
        .unwrap()
        .instructions = "Different later instructions".into();
    // Binding prepared the provider connection once, before any text existed.
    assert_eq!(fake.warmed.lock().unwrap().len(), 1);
    assert_eq!(fake.warmed.lock().unwrap()[0].model, "gpt-4.1-mini");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    service.save(edited).await.unwrap();
    let pending = service.start_recorded(job.id).await.unwrap().unwrap();
    assert_eq!(pending.profile.instructions, profile.instructions);
    assert_eq!(pending.profile.revision, profile.revision);
    assert!(pending.request.context.clipboard.is_empty());
    let duplicate = service.start_recorded(job.id).await.unwrap().unwrap();
    assert_eq!(pending.id, duplicate.id);
    done(&service, job.id, pending.id).await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let reopened = ModeService::open_with(root.0.join("modes"), p.jobs.clone(), fake)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .captured_mode(job.request.recording_id)
            .await
            .unwrap()
            .unwrap()
            .profile
            .instructions,
        profile.instructions
    );
    let live = reopened.live_processing(job.id).await.unwrap();
    assert!(live.bound && matches!(live.attempt.unwrap().state, AttemptState::Succeeded { .. }));
    reopened
        .begin_history_deletion()
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(reopened
        .captured_mode(job.request.recording_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        reopened.settings().await.unwrap().revision,
        before.revision + 1
    );
}
async fn done(service: &ModeService, job: Uuid, id: Uuid) -> ProcessingAttempt {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let attempts = service.attempts(job).await.unwrap();
            let attempt = attempts.into_iter().find(|a| a.id == id).unwrap();
            if !matches!(attempt.state, AttemptState::Pending) {
                return attempt;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn regular_is_local_and_super_preserves_exact_values_without_sending_them() {
    let root = Temp::new();
    let (service, p, job, fake) = fixture(&root, false).await;
    let regular = service
        .start(job.id, request(&service, "regular").await)
        .await
        .unwrap();
    let regular = done(&service, job.id, regular.id).await;
    assert!(
        matches!(regular.state, AttemptState::Succeeded { ref text } if text == "שלום hello PRIVATE EXACT VALUE")
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    let mut req = request(&service, "super").await;
    req.context.clipboard = "PRIVATE CLIPBOARD".into();
    let attempt = service.start(job.id, req.clone()).await.unwrap();
    let duplicate = service.start(job.id, req).await.unwrap();
    assert_eq!(duplicate.id, attempt.id);
    let attempt = done(&service, job.id, attempt.id).await;
    assert!(
        matches!(attempt.state, AttemptState::Succeeded { ref text } if text == "שלום hello PRIVATE EXACT VALUE")
    );
    assert!(attempt.request.context.clipboard.is_empty());
    let sent = fake.inputs.lock().await.join("");
    assert!(!sent.contains("PRIVATE EXACT VALUE") && !sent.contains("PRIVATE CLIPBOARD"));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let after = p.jobs.get(job.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(job).unwrap()
    );
}
#[tokio::test]
async fn commands_require_target_and_attempts_keep_previous_results_on_failure() {
    let root = Temp::new();
    let (service, p, job, fake) = fixture(&root, false).await;
    let mut req = request(&service, "command").await;
    assert!(service.start(job.id, req.clone()).await.is_err());
    req.context.selected_text = "Target text".into();
    let first = service.start(job.id, req.clone()).await.unwrap();
    let first = done(&service, job.id, first.id).await;
    assert!(matches!(first.state, AttemptState::Succeeded { ref text } if text == "Target text."));
    req.context.selected_text = "Changed target".into();
    assert!(matches!(
        service.start(job.id, req).await.err().unwrap().code,
        ErrorCode::IdempotencyConflict
    ));
    let failed_provider = Arc::new(Fake {
        calls: AtomicUsize::new(0),
        inputs: Mutex::new(vec![]),
        fail: true,
        warmed: Default::default(),
    });
    let reopened = ModeService::open_with(root.0.join("modes"), p.jobs.clone(), failed_provider)
        .await
        .unwrap();
    let second = reopened
        .start(job.id, request(&reopened, "super").await)
        .await
        .unwrap();
    let recovered = done(&reopened, job.id, second.id).await;
    assert!(matches!(recovered.state, AttemptState::Succeeded { .. }));
    assert!(recovered.diagnostics.original_used);
    assert_eq!(reopened.attempts(job.id).await.unwrap().len(), 2);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn profile_updates_are_versioned_and_stale_writes_are_rejected() {
    let root = Temp::new();
    let (service, _, job, _) = fixture(&root, false).await;
    let old_request = request(&service, "super").await;
    let mut settings = service.settings().await.unwrap();
    let stale = settings.clone();
    settings.profiles[1].instructions = "Format paragraphs clearly".into();
    let saved = service.save(settings).await.unwrap();
    assert_eq!(saved.revision, stale.revision + 1);
    assert_eq!(saved.profiles[1].revision, stale.profiles[1].revision + 1);
    assert!(service.save(stale).await.is_err());
    assert!(service.start(job.id, old_request).await.is_err());
}
#[tokio::test]
async fn interrupted_attempt_is_reported_without_automatically_resending_text() {
    let root = Temp::new();
    let (service, p, job, fake) = fixture(&root, false).await;
    let attempt = service
        .start(job.id, request(&service, "regular").await)
        .await
        .unwrap();
    let mut attempt = done(&service, job.id, attempt.id).await;
    attempt.state = AttemptState::Pending;
    crate::recordings::disk::write_json(
        &root
            .0
            .join("modes")
            .join(job.id.to_string())
            .join(format!("{}.json", attempt.id)),
        &attempt,
    )
    .await
    .unwrap();
    let reopened = ModeService::open_with(root.0.join("modes"), p.jobs.clone(), fake.clone())
        .await
        .unwrap();
    assert!(matches!(
        reopened.attempts(job.id).await.unwrap()[0].state,
        AttemptState::Failed { .. }
    ));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn http_rejects_invalid_envelopes_and_returns_durable_attempts() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let root = Temp::new();
    let (service, _, job, _) = fixture(&root, false).await;
    let router = routes::router(Some(service.clone()));
    let bad = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/modes")
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"api_version":"2","data":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let req = request(&service, "regular").await;
    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/processing", job.id))
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&Envelope::new(req)).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let body = to_bytes(created.into_body(), 1_000_000).await.unwrap();
    let attempt: Envelope<ProcessingAttempt> = serde_json::from_slice(&body).unwrap();
    done(&service, job.id, attempt.data.id).await;
    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/jobs/{}/processing", job.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let results: Envelope<Vec<ProcessingAttempt>> = serde_json::from_slice(&body).unwrap();
    assert_eq!(results.data.len(), 1);
    assert!(matches!(
        results.data[0].state,
        AttemptState::Succeeded { .. }
    ));
}

#[tokio::test]
async fn super_rejects_lost_scripts_and_missing_protected_values() {
    struct Lost;
    #[async_trait::async_trait]
    impl processing::Processor for Lost {
        async fn generate(&self, _: &ModelSelection, _: &str, _: &str) -> Result<String, ApiError> {
            Ok(r#"{"text":"Invented English output"}"#.into())
        }
    }
    let root = Temp::new();
    let (service, p, job, _) = fixture(&root, false).await;
    let s = ModeService::open_with(root.0.join("modes"), p.jobs.clone(), Arc::new(Lost))
        .await
        .unwrap();
    let attempt = s
        .start(job.id, request(&service, "super").await)
        .await
        .unwrap();
    let recovered = done(&s, job.id, attempt.id).await;
    assert!(
        matches!(recovered.state, AttemptState::Succeeded { ref text } if text == "שלום hello PRIVATE EXACT VALUE")
    );
    assert!(recovered.diagnostics.original_used);
    assert!(matches!(
        recovered.diagnostics.original_reason,
        Some(ErrorCode::InvalidProviderResponse)
    ));
    assert_eq!(
        serde_json::to_value(p.jobs.get(job.id).await.unwrap()).unwrap(),
        serde_json::to_value(job).unwrap()
    );
}

#[tokio::test]
async fn history_deletion_waits_for_active_processing_and_keeps_profiles() {
    let root = Temp::new();
    let (service, _, job, _) = fixture(&root, false).await;
    let attempt = service
        .start(job.id, request(&service, "super").await)
        .await
        .unwrap();
    assert!(service.begin_history_deletion().await.is_err());
    done(&service, job.id, attempt.id).await;
    let before = service.settings().await.unwrap();
    let deletion = service.begin_history_deletion().await.unwrap();
    assert_eq!(deletion.commit().await.unwrap(), 1);
    assert!(service.attempts(job.id).await.unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(service.settings().await.unwrap()).unwrap()
    );
}

#[tokio::test]
async fn native_delivery_waits_for_captured_mode_and_regular_finishes_synchronously() {
    for failed in [false, true] {
        let root = Temp::new();
        let (service, pipeline, job, fake) =
            fixture_with_raw(&root, failed, "hello SIGNATURE").await;
        let settings = service.settings().await.unwrap();
        let profile = settings
            .profiles
            .iter()
            .find(|p| p.id == if failed { "super" } else { "regular" })
            .unwrap()
            .clone();
        service
            .bind_recording(
                job.request.recording_id,
                profile,
                ContextSnapshot::default(),
            )
            .await
            .unwrap();
        let delivered = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            service.delivery_text(job.id),
        )
        .await
        .unwrap();
        if failed {
            assert_eq!(delivered.unwrap(), "hello PRIVATE EXACT VALUE");
            assert!(
                service
                    .prepare_delivery(job.id)
                    .await
                    .unwrap()
                    .original_used
            );
        } else {
            assert_eq!(delivered.unwrap(), "hello PRIVATE EXACT VALUE");
            let again = service.start_recorded(job.id).await.unwrap().unwrap();
            assert!(matches!(again.state, AttemptState::Succeeded { .. }));
            assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        }
        let stored = pipeline.jobs.get(job.id).await.unwrap();
        assert_eq!(
            serde_json::to_value(stored).unwrap(),
            serde_json::to_value(job).unwrap()
        );
    }
}

#[tokio::test]
async fn factory_prompt_migrates_once_without_changing_custom_prompts_or_old_captures() {
    let root = Temp::new();
    let (service, _, job, _) = fixture(&root, false).await;
    let mut settings = service.settings().await.unwrap();
    settings.profiles[1].instructions = style::LEGACY_SUPER.into();
    settings.profiles[2].instructions = "My personal instructions".into();
    let old = service.save(settings).await.unwrap();
    service
        .bind_recording(
            job.request.recording_id,
            old.profiles[1].clone(),
            ContextSnapshot::default(),
        )
        .await
        .unwrap();
    let updated = service.settings().await.unwrap();
    assert_eq!(updated.profiles[1].instructions, style::POLISHED_SUPER);
    assert_eq!(updated.profiles[1].revision, old.profiles[1].revision + 1);
    assert_eq!(updated.revision, old.revision + 1);
    assert_eq!(updated.profiles[2].instructions, "My personal instructions");
    assert_eq!(service.settings().await.unwrap().revision, updated.revision);
    assert_eq!(
        service
            .captured_mode(job.request.recording_id)
            .await
            .unwrap()
            .unwrap()
            .profile
            .instructions,
        style::LEGACY_SUPER
    );
}

#[tokio::test]
async fn exact_inflight_draft_is_reused_but_changed_transcript_is_processed_again() {
    for changed in [false, true] {
        let root = Temp::new();
        let (service, _, job, fake) = fixture(&root, false).await;
        let mut profile = service.settings().await.unwrap().profiles[1].clone();
        profile.process_while_speaking = true;
        service
            .bind_recording(
                job.request.recording_id,
                profile,
                ContextSnapshot::default(),
            )
            .await
            .unwrap();
        let JobStatus::Succeeded { result } = &job.status else {
            panic!()
        };
        let raw = if changed {
            "שלום hello Tuesday signature"
        } else {
            &result.transcript.raw_text
        };
        service
            .prepare_draft(
                job.request.recording_id,
                raw.into(),
                job.request.options.vocabulary.clone(),
            )
            .await;
        service
            .prepare_draft(
                job.request.recording_id,
                raw.into(),
                job.request.options.vocabulary.clone(),
            )
            .await;
        assert_eq!(
            fake.calls.load(Ordering::SeqCst),
            0,
            "nothing while speaking"
        );
        service.release_draft(job.request.recording_id).await;
        // Let the release draft enter generate but leave it in flight as
        // finalization starts.
        tokio::time::sleep(Duration::from_millis(150)).await;
        service.start_recorded(job.id).await.unwrap();
        let attempt = done(&service, job.id, job.id).await;
        assert_eq!(attempt.diagnostics.reused_draft, !changed);
        assert_eq!(
            fake.calls.load(Ordering::SeqCst),
            if changed { 2 } else { 1 }
        );
        let AttemptState::Succeeded { text } = attempt.state else {
            panic!()
        };
        assert!(text.contains("PRIVATE EXACT VALUE"));
        assert!(!text.contains("Tuesday"));
        assert!(!fake
            .inputs
            .lock()
            .await
            .iter()
            .any(|s| s.contains("PRIVATE EXACT VALUE")));
    }
}

#[tokio::test]
async fn canceled_draft_cannot_be_resurrected_and_disabled_mode_never_sends() {
    for enabled in [false, true] {
        let root = Temp::new();
        let (service, _, job, fake) = fixture(&root, false).await;
        let mut profile = service.settings().await.unwrap().profiles[1].clone();
        profile.process_while_speaking = enabled;
        service
            .bind_recording(
                job.request.recording_id,
                profile,
                ContextSnapshot::default(),
            )
            .await
            .unwrap();
        service.discard_draft(job.request.recording_id).await;
        service
            .prepare_draft(
                job.request.recording_id,
                "hello this is a late preview".into(),
                empty_vocabulary(),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn draft_preview_preserves_settings_and_saved_input_and_rejects_unpermitted_context() {
    let root = Temp::new();
    let (service, _, job, fake) = fixture(&root, false).await;
    let original = service.settings().await.unwrap();
    let mut profile = original.profiles[1].clone();
    profile.instructions = "My unsaved instruction".into();
    service
        .preview(super::preview::PreviewRequest {
            profile,
            text: "ignored when using saved input".into(),
            job_id: Some(job.id),
            context: ContextSnapshot {
                clipboard: "do not send".into(),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    assert_eq!(
        service.settings().await.unwrap().revision,
        original.revision
    );
    assert_eq!(
        service.settings().await.unwrap().profiles[1].instructions,
        original.profiles[1].instructions
    );
    assert!(service.attempts(job.id).await.unwrap().is_empty());
    let inputs = fake.inputs.lock().await;
    assert!(!inputs[0].contains("do not send"));
    assert!(!inputs[0].contains("PRIVATE EXACT VALUE"));
}

/// Dictating only a snippet into a form field delivers the stored value alone,
/// without the recognizer's leading space or sentence mark, and without a model.
#[tokio::test]
async fn super_delivers_a_snippet_only_dictation_exactly_as_stored() {
    let root = Temp::new();
    let (service, _, job, fake) = fixture_with_raw(&root, false, " Signature.").await;
    let request = request(&service, "super").await;
    let attempt = service.start(job.id, request.clone()).await.unwrap();
    let mut vocabulary = empty_vocabulary();
    vocabulary.snippets.push(Snippet {
        phrase: "signature".into(),
        text: "PRIVATE EXACT VALUE".into(),
    });
    let output = processing::run(&attempt, &vocabulary, &*fake)
        .await
        .unwrap();
    assert_eq!(output, "PRIVATE EXACT VALUE");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    done(&service, job.id, request.operation_id).await;
}
#[tokio::test]
async fn natural_layout_is_preserved_and_single_line_does_not_flatten_exact_snippets() {
    struct Lines;
    #[async_trait::async_trait]
    impl processing::Processor for Lines {
        async fn generate(&self, _: &ModelSelection, _: &str, _: &str) -> Result<String, ApiError> {
            Ok(serde_json::json!({"spans":["hello:\n- milk\n- bread"]}).to_string())
        }
    }
    let root = Temp::new();
    let (service, _, job, _) = fixture_with_raw(&root, false, "hello milk bread signature").await;
    let request = request(&service, "super").await;
    let attempt = service.start(job.id, request.clone()).await.unwrap();
    let mut vocabulary = empty_vocabulary();
    vocabulary.snippets.push(Snippet {
        phrase: "signature".into(),
        text: "EXACT\nMULTILINE".into(),
    });
    let mut input = attempt;
    let output = processing::run(&input, &vocabulary, &Lines).await.unwrap();
    assert!(output.contains("hello:\n- milk\n- bread"));
    input.profile.formatting.layout = Layout::SingleLine;
    let output = processing::run(&input, &vocabulary, &Lines).await.unwrap();
    assert!(output.contains("hello: - milk - bread"));
    assert!(output.contains("EXACT\nMULTILINE"));
    done(&service, job.id, request.operation_id).await;
}

#[tokio::test]
async fn migration_enables_agreed_early_processing_only_when_no_explicit_choice_exists() {
    for explicit in [false, true] {
        let root = Temp::new();
        let (service, _, _, _) = fixture(&root, false).await;
        let mut document = serde_json::to_value(service.settings().await.unwrap()).unwrap();
        document["profiles"][1]["instructions"] = style::LEGACY_SUPER.into();
        if explicit {
            document["profiles"][1]["process_while_speaking"] = false.into();
        } else {
            document["profiles"][1]
                .as_object_mut()
                .unwrap()
                .remove("process_while_speaking");
        }
        crate::recordings::disk::write_json(&service.root.join("settings.json"), &document)
            .await
            .unwrap();
        let profile = service.settings().await.unwrap().profiles[1].clone();
        assert_eq!(profile.process_while_speaking, !explicit);
    }
}

#[tokio::test]
async fn formatting_cues_can_be_the_only_words_in_a_script() {
    struct Quotes;
    #[async_trait::async_trait]
    impl processing::Processor for Quotes {
        async fn generate(&self, _: &ModelSelection, _: &str, _: &str) -> Result<String, ApiError> {
            Ok(serde_json::json!({"spans":["הוא אמר: \"שלום\"."]}).to_string())
        }
    }
    let root = Temp::new();
    let (service, _, job, _) =
        fixture_with_raw(&root, false, "הוא אמר open quote שלום close quote").await;
    let request = request(&service, "super").await;
    let attempt = service.start(job.id, request.clone()).await.unwrap();
    assert!(processing::run(&attempt, &empty_vocabulary(), &Quotes)
        .await
        .unwrap()
        .contains("שלום"));
    let mut literal = attempt.clone();
    literal.profile.formatting.spoken_punctuation = false;
    assert!(processing::run(&literal, &empty_vocabulary(), &Quotes)
        .await
        .is_err());
    literal = attempt;
    literal.raw_text.push_str(" important English content");
    assert!(processing::run(&literal, &empty_vocabulary(), &Quotes)
        .await
        .is_err());
    done(&service, job.id, request.operation_id).await;
}

#[tokio::test]
async fn legacy_attempt_does_not_claim_fast_thinking_or_zero_millisecond_measurements() {
    let root = Temp::new();
    let (service, _, job, _) = fixture(&root, false).await;
    let request = request(&service, "super").await;
    service.start(job.id, request.clone()).await.unwrap();
    let attempt = done(&service, job.id, request.operation_id).await;
    let mut legacy = serde_json::to_value(attempt).unwrap();
    legacy.as_object_mut().unwrap().remove("diagnostics");
    legacy["profile"]
        .as_object_mut()
        .unwrap()
        .remove("fast_processing");
    let restored: ProcessingAttempt = serde_json::from_value(legacy).unwrap();
    assert!(!restored.profile.fast_processing);
    assert!(serde_json::to_value(restored)
        .unwrap()
        .get("diagnostics")
        .is_none());
}

/// Live connection-setup probe; ignored by default and never run by validation.
///
/// Every trial builds a fresh client, which is what a dictation paid after the
/// former 90-second idle eviction. A cold trial sends the Super request at once.
/// A warmed trial calls `warm` as recording start does, waits for the simulated
/// speech, then sends the same request. Synthetic text only. Configure the key
/// in the environment and `WRITER_WARM_MODEL=provider:model`; optional
/// `WRITER_WARM_TRIALS` (default 12) and `WRITER_WARM_SPEECH_MS` (default 3000).
/// Run the release test binary with
/// `--ignored --exact cleanup::modes::tests::live_cold_versus_warmed_connection --nocapture`.
#[tokio::test]
#[ignore]
async fn live_cold_versus_warmed_connection() {
    let spec = std::env::var("WRITER_WARM_MODEL").expect("WRITER_WARM_MODEL=provider:model");
    let (provider, model) = spec.split_once(':').expect("provider:model");
    let selection = ModelSelection {
        provider: serde_json::from_value(serde_json::json!(provider)).expect("provider id"),
        model: model.into(),
    };
    let variable = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let trials = variable("WRITER_WARM_TRIALS", 12) as usize;
    let speech = Duration::from_millis(variable("WRITER_WARM_SPEECH_MS", 3000));
    let samples = [
        "Please buy these items apples pears rice and coffee",
        "אני צריך לקנות את הדברים הבאים חלב לחם וביצים פסקה חדשה נקבע ליום שלישי בעצם ליום חמישי בשעה שלוש",
        "The first issue is the microphone it disconnects whenever I plug in the headphones new paragraph the second issue is the settings page I want a save button that stays visible",
    ];
    let formatting = ModeSettings::default()
        .profiles
        .into_iter()
        .find(|p| p.id == "super")
        .unwrap()
        .formatting;
    let instructions = format!(
        "Improve the readability and paragraph formatting of each editable span. Return a JSON object with exactly one field named spans: an array of strings.\nMode instructions:\n{}\nFormatting preferences:\n{}",
        style::POLISHED_SUPER,
        formatting.instructions()
    );
    let (mut cold, mut warmed) = (Vec::new(), Vec::new());
    for trial in 0..trials {
        let input =
            serde_json::json!({"editable_spans": [samples[trial % samples.len()]]}).to_string();
        let order = if trial % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        };
        for warm in order {
            let client = crate::cleanup::provider::CloudText::from_env();
            if warm {
                client.warm(&selection);
                tokio::time::sleep(speech).await;
            }
            let started = std::time::Instant::now();
            let result = client
                .generate_with_options(&selection, &instructions, &input, true)
                .await;
            let elapsed = started.elapsed().as_millis() as u64;
            assert!(
                result.is_ok(),
                "trial {trial} warm={warm}: {:?}",
                result.err().map(|e| e.code)
            );
            println!("trial={trial} warm={warm} ms={elapsed}");
            if warm { &mut warmed } else { &mut cold }.push(elapsed);
        }
    }
    let median = |mut v: Vec<u64>| {
        v.sort_unstable();
        (v[(v.len() - 1) / 2] + v[v.len() / 2]) / 2
    };
    println!(
        "cold median {} ms, warmed median {} ms, trials {trials}",
        median(cold),
        median(warmed)
    );
}

/// Answers at a pace chosen per model, so a race has a known winner.
struct Paced {
    delays: Vec<(&'static str, Duration, bool)>,
    calls: std::sync::Mutex<Vec<String>>,
    times: processing::ResponseTimes,
}
#[async_trait::async_trait]
impl processing::Processor for Paced {
    fn response_times(&self) -> &processing::ResponseTimes {
        &self.times
    }
    async fn generate(
        &self,
        model: &ModelSelection,
        _: &str,
        input: &str,
    ) -> Result<String, ApiError> {
        self.calls.lock().unwrap().push(model.model.clone());
        let (_, delay, fails) = *self
            .delays
            .iter()
            .find(|(name, _, _)| *name == model.model)
            .unwrap();
        tokio::time::sleep(delay).await;
        if fails {
            return Err(processing::invalid("fixture failure"));
        }
        let input: serde_json::Value = serde_json::from_str(input).unwrap();
        Ok(serde_json::json!({"spans": input["editable_spans"]}).to_string())
    }
}
/// A chosen model that usually answers in 100 ms, so the mode's own minimum
/// wait is what decides when its backup joins.
async fn raced(
    root: &Temp,
    delays: Vec<(&'static str, Duration, bool)>,
    after_ms: Option<u64>,
) -> (Arc<ModeService>, Uuid, Arc<Paced>) {
    paced_service(root, delays, after_ms, Some(100)).await
}
fn chosen() -> ModelSelection {
    ModelSelection {
        provider: ProviderId::Openai,
        model: "chosen".into(),
    }
}
async fn paced_service(
    root: &Temp,
    delays: Vec<(&'static str, Duration, bool)>,
    after_ms: Option<u64>,
    usual_ms: Option<u64>,
) -> (Arc<ModeService>, Uuid, Arc<Paced>) {
    let (service, p, job, _) = fixture(root, false).await;
    let paced = Arc::new(Paced {
        delays,
        calls: Default::default(),
        times: Default::default(),
    });
    for ms in usual_ms.into_iter().flat_map(|ms| [ms; 3]) {
        paced.times.record(&chosen(), true, ms);
    }
    let raced = ModeService::open_with(root.0.join("raced"), p.jobs.clone(), paced.clone())
        .await
        .unwrap();
    let mut settings = raced.settings().await.unwrap();
    for profile in &mut settings.profiles {
        if profile.kind == ModeKind::Super {
            profile.text_model = Some(chosen());
            profile.fallback_model = Some(ModelSelection {
                provider: ProviderId::Openrouter,
                model: "fixture/standby".into(),
            });
            profile.fallback_after_ms = after_ms;
        }
    }
    raced.save(settings).await.unwrap();
    let _ = service;
    (raced, job.id, paced)
}

#[tokio::test]
async fn a_slow_model_is_overtaken_by_the_fallback_while_a_fast_one_is_never_joined() {
    let root = Temp::new();
    // The chosen model is slower than the wait; its fallback answers first.
    let (service, job, paced) = raced(
        &root,
        vec![
            ("chosen", Duration::from_millis(900), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        Some(250),
    )
    .await;
    let started = request(&service, "super").await;
    let attempt = done(&service, job, service.start(job, started).await.unwrap().id).await;
    assert!(matches!(attempt.state, AttemptState::Succeeded { .. }));
    assert!(attempt.diagnostics.fallback_used);
    assert_eq!(
        *paced.calls.lock().unwrap(),
        vec!["chosen".to_owned(), "fixture/standby".to_owned()]
    );
    // Both requests were sent, but the wait is what it cost, not the slow model.
    assert!(attempt.diagnostics.processing_ms < 900);

    let root = Temp::new();
    let (service, job, paced) = raced(
        &root,
        vec![
            ("chosen", Duration::from_millis(20), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        Some(400),
    )
    .await;
    let started = request(&service, "super").await;
    let attempt = done(&service, job, service.start(job, started).await.unwrap().id).await;
    assert!(matches!(attempt.state, AttemptState::Succeeded { .. }));
    assert!(!attempt.diagnostics.fallback_used);
    assert_eq!(*paced.calls.lock().unwrap(), vec!["chosen".to_owned()]);
}

#[tokio::test]
async fn a_failed_model_hands_over_at_once_and_a_failed_fallback_never_replaces_a_good_answer() {
    let root = Temp::new();
    // Failure is not worth waiting out: the fallback starts when it happens.
    let (service, job, paced) = raced(
        &root,
        vec![
            ("chosen", Duration::from_millis(10), true),
            ("fixture/standby", Duration::from_millis(10), false),
        ],
        Some(5_000),
    )
    .await;
    let started = request(&service, "super").await;
    let attempt = done(&service, job, service.start(job, started).await.unwrap().id).await;
    assert!(matches!(attempt.state, AttemptState::Succeeded { .. }));
    assert!(attempt.diagnostics.fallback_used);
    assert_eq!(paced.calls.lock().unwrap().len(), 2);
    assert!(attempt.diagnostics.processing_ms < 5_000);

    // A fallback that fails first leaves the chosen model to finish the work.
    let root = Temp::new();
    let (service, job, _) = raced(
        &root,
        vec![
            ("chosen", Duration::from_millis(900), false),
            ("fixture/standby", Duration::from_millis(10), true),
        ],
        Some(300),
    )
    .await;
    let started = request(&service, "super").await;
    let attempt = done(&service, job, service.start(job, started).await.unwrap().id).await;
    assert!(matches!(attempt.state, AttemptState::Succeeded { .. }));
    assert!(!attempt.diagnostics.fallback_used);
}

/// Paid requests for one automatic Super dictation, and which model delivered.
async fn dictate(service: &Arc<ModeService>, job: Uuid, paced: &Paced) -> (Vec<String>, bool) {
    paced.calls.lock().unwrap().clear();
    let started = request(service, "super").await;
    let attempt = done(service, job, service.start(job, started).await.unwrap().id).await;
    assert!(matches!(attempt.state, AttemptState::Succeeded { .. }));
    assert!(!attempt.diagnostics.original_used);
    (
        paced.calls.lock().unwrap().clone(),
        attempt.diagnostics.fallback_used,
    )
}

#[tokio::test]
async fn one_dictation_is_one_request_unless_the_chosen_model_fails_or_is_clearly_slow() {
    let standby = || "fixture/standby".to_owned();
    // No backup: one request, however slow.
    let root = Temp::new();
    let (service, job, paced) = paced_service(
        &root,
        vec![("chosen", Duration::from_millis(400), false)],
        None,
        None,
    )
    .await;
    let mut settings = service.settings().await.unwrap();
    settings.profiles[1].fallback_model = None;
    service.save(settings).await.unwrap();
    assert_eq!(
        dictate(&service, job, &paced).await,
        (vec!["chosen".into()], false)
    );

    // A backup and a healthy chosen model that has not answered before: the old
    // fixed one-second wait would have paid for both; nothing is known yet, so
    // the backup waits for twice the assumed usual time.
    let root = Temp::new();
    let (service, job, paced) = paced_service(
        &root,
        vec![
            ("chosen", Duration::from_millis(1_200), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        None,
        None,
    )
    .await;
    assert_eq!(
        dictate(&service, job, &paced).await,
        (vec!["chosen".into()], false)
    );
    // Its answers become its usual time.
    assert_eq!(
        paced.times.usual(&chosen(), true),
        None,
        "one answer is not yet a usual time"
    );

    // A chosen model that usually answers in 400 ms: a little slower is still
    // ordinary, twice as slow brings in the backup, which then delivers.
    let root = Temp::new();
    let (service, job, paced) = paced_service(
        &root,
        vec![
            ("chosen", Duration::from_millis(550), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        Some(250),
        Some(400),
    )
    .await;
    assert_eq!(
        dictate(&service, job, &paced).await,
        (vec!["chosen".into()], false)
    );
    let root = Temp::new();
    let (service, job, paced) = paced_service(
        &root,
        vec![
            ("chosen", Duration::from_millis(2_000), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        Some(250),
        Some(400),
    )
    .await;
    let started = std::time::Instant::now();
    assert_eq!(
        dictate(&service, job, &paced).await,
        (vec!["chosen".into(), standby()], true)
    );
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(800) && elapsed < Duration::from_millis(2_000));
    // A slow spell the backup covered does not become the chosen model's usual pace.
    assert_eq!(paced.times.usual(&chosen(), true), Some(400));

    // A failing chosen model: the backup starts at once.
    let root = Temp::new();
    let (service, job, paced) = paced_service(
        &root,
        vec![
            ("chosen", Duration::from_millis(10), true),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        None,
        None,
    )
    .await;
    let started = std::time::Instant::now();
    assert_eq!(
        dictate(&service, job, &paced).await,
        (vec!["chosen".into(), standby()], true)
    );
    assert!(started.elapsed() < Duration::from_millis(1_000));
}

/// The automatic dictation of a recording whose newest preview at release was
/// `preview`, after waiting `before_final` for the final recognition.
async fn dictate_with_draft(
    service: &Arc<ModeService>,
    job: &Job,
    paced: &Paced,
    preview: Option<&str>,
    before_final: Duration,
) -> (Vec<String>, ProcessingAttempt) {
    paced.calls.lock().unwrap().clear();
    let mut profile = service.settings().await.unwrap().profiles[1].clone();
    profile.process_while_speaking = true;
    service
        .bind_recording(
            job.request.recording_id,
            profile,
            ContextSnapshot::default(),
        )
        .await
        .unwrap();
    let JobStatus::Succeeded { result } = &job.status else {
        panic!()
    };
    let raw = preview.unwrap_or(&result.transcript.raw_text).to_owned();
    for _ in 0..3 {
        service
            .prepare_draft(
                job.request.recording_id,
                raw.clone(),
                job.request.options.vocabulary.clone(),
            )
            .await;
    }
    assert!(
        paced.calls.lock().unwrap().is_empty(),
        "nothing while speaking"
    );
    service.release_draft(job.request.recording_id).await;
    tokio::time::sleep(before_final).await;
    let before = paced.calls.lock().unwrap().clone();
    service.start_recorded(job.id).await.unwrap();
    let attempt = done(service, job.id, job.id).await;
    (before, attempt)
}

#[tokio::test]
async fn release_drafts_cost_one_request_and_never_start_the_backup_themselves() {
    let calls = |paced: &Paced| paced.calls.lock().unwrap().clone();
    // A matching release draft is the dictation's only request.
    let root = Temp::new();
    let (service, id, paced) = paced_service(
        &root,
        vec![
            ("chosen", Duration::from_millis(40), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        Some(250),
        Some(100),
    )
    .await;
    let job = service.jobs.get(id).await.unwrap();
    let (before, attempt) =
        dictate_with_draft(&service, &job, &paced, None, Duration::from_millis(150)).await;
    assert_eq!(before, vec!["chosen".to_owned()]);
    assert!(attempt.diagnostics.reused_draft && attempt.diagnostics.prepared_at_release);
    assert_eq!(calls(&paced), vec!["chosen".to_owned()]);

    // A draft whose chosen model is clearly slow: the draft alone never asks the
    // backup, however long it runs; the final processing brings the backup in
    // against that same request rather than asking the chosen model again.
    let root = Temp::new();
    let (service, id, paced) = paced_service(
        &root,
        vec![
            ("chosen", Duration::from_millis(1_500), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        Some(250),
        Some(100),
    )
    .await;
    let job = service.jobs.get(id).await.unwrap();
    let (before, attempt) =
        dictate_with_draft(&service, &job, &paced, None, Duration::from_millis(700)).await;
    assert_eq!(before, vec!["chosen".to_owned()]);
    assert!(attempt.diagnostics.fallback_used && !attempt.diagnostics.reused_draft);
    assert_eq!(
        calls(&paced),
        vec!["chosen".to_owned(), "fixture/standby".to_owned()]
    );

    // A preview that differs from the final transcript costs the draft and the
    // final request.
    let root = Temp::new();
    let (service, id, paced) = paced_service(
        &root,
        vec![
            ("chosen", Duration::from_millis(40), false),
            ("fixture/standby", Duration::from_millis(20), false),
        ],
        Some(250),
        Some(100),
    )
    .await;
    let job = service.jobs.get(id).await.unwrap();
    let (_, attempt) = dictate_with_draft(
        &service,
        &job,
        &paced,
        Some("שלום hello Tuesday signature"),
        Duration::from_millis(150),
    )
    .await;
    assert!(!attempt.diagnostics.reused_draft && !attempt.diagnostics.fallback_used);
    assert_eq!(
        calls(&paced),
        vec!["chosen".to_owned(), "chosen".to_owned()]
    );
}

#[tokio::test]
async fn a_fallback_must_be_a_different_cloud_model_within_the_supported_wait() {
    let root = Temp::new();
    let (service, _, _, _) = fixture(&root, false).await;
    let settings = service.settings().await.unwrap();
    let chosen = |profile: &ModeProfile| profile.text_model.clone();
    for (fallback, after, ok) in [
        (
            Some(ModelSelection {
                provider: ProviderId::Openrouter,
                model: "fixture/standby".into(),
            }),
            Some(1_000),
            true,
        ),
        // A local model answers one request at a time, so it cannot overlap.
        (
            Some(ModelSelection {
                provider: ProviderId::Local,
                model: "qwen3-4b-instruct-2507-q4".into(),
            }),
            None,
            false,
        ),
        (None, Some(20), false),
        (None, Some(60_000), false),
    ] {
        let mut next = settings.clone();
        for profile in &mut next.profiles {
            if profile.kind == ModeKind::Super {
                profile.fallback_model = fallback.clone().or_else(|| {
                    Some(ModelSelection {
                        provider: ProviderId::Openrouter,
                        model: "fixture/standby".into(),
                    })
                });
                profile.fallback_after_ms = after;
            }
        }
        assert_eq!(
            service.save(next).await.is_ok(),
            ok,
            "{fallback:?} {after:?}"
        );
    }
    // The fallback cannot be the model it is standing in for.
    let mut next = service.settings().await.unwrap();
    for profile in &mut next.profiles {
        if profile.kind == ModeKind::Super {
            profile.fallback_model = chosen(profile);
        }
    }
    assert!(service.save(next).await.is_err());
}
#[path = "tests_retention.rs"]
mod retention;

#[tokio::test]
async fn both_failures_select_original_and_manual_retry_never_changes_delivery() {
    let root = Temp::new();
    let (service, id, paced) = raced(
        &root,
        vec![
            ("chosen", Duration::from_millis(10), true),
            ("fixture/standby", Duration::from_millis(10), true),
        ],
        Some(5_000),
    )
    .await;
    let job = service.jobs.get(id).await.unwrap();
    let profile = service.settings().await.unwrap().profiles[1].clone();
    service
        .bind_recording(
            job.request.recording_id,
            profile,
            ContextSnapshot::default(),
        )
        .await
        .unwrap();
    let delivered = service.prepare_delivery(id).await.unwrap();
    assert_eq!(delivered.text, "שלום hello PRIVATE EXACT VALUE");
    assert!(delivered.original_used);
    assert!(!delivered.fallback_used);
    let auto = service.live_processing(id).await.unwrap().attempt.unwrap();
    assert_eq!(auto.diagnostics.model_attempts.len(), 2);
    assert!(auto
        .diagnostics
        .model_attempts
        .iter()
        .all(|m| m.state == "failed"));
    assert!(auto.diagnostics.processing_ms < 1_000);
    service
        .record_delivery(id, true, true, Some(700), Some("Slack".into()))
        .await
        .unwrap();
    let reopened =
        ModeService::open_with(service.root.clone(), service.jobs.clone(), paced.clone())
            .await
            .unwrap();
    let mut settings = reopened.settings().await.unwrap();
    settings.profiles[1].kind = ModeKind::Regular;
    settings.profiles[1].text_model = None;
    settings.profiles[1].fallback_model = None;
    reopened.save(settings).await.unwrap();
    let retry = reopened
        .start(id, request(&reopened, "super").await)
        .await
        .unwrap();
    let live = reopened.live_processing(id).await.unwrap();
    assert_eq!(live.selected_attempt.unwrap().id, retry.id);
    let receipt = live.delivery.unwrap();
    assert!(receipt.original_used && receipt.pasted);
    // Where the words went survives a reopen, by the application's own name.
    assert_eq!(receipt.app.as_deref(), Some("Slack"));
    assert_eq!(receipt.attempt_id, auto.id);
    assert_eq!(receipt.stop_to_delivery_ms, Some(700));
    assert!(reopened.prepare_delivery(id).await.unwrap().original_used);
    assert_eq!(paced.calls.lock().unwrap().len(), 2);
    assert_eq!(
        serde_json::to_value(reopened.jobs.get(id).await.unwrap()).unwrap(),
        serde_json::to_value(job).unwrap()
    );
}

#[tokio::test]
async fn both_timeouts_wait_for_backup_then_persist_original_without_late_replacement() {
    let root = Temp::new();
    let (service, id, paced) = raced(
        &root,
        vec![
            ("chosen", Duration::from_secs(10), false),
            ("fixture/standby", Duration::from_secs(10), false),
        ],
        Some(250),
    )
    .await;
    let mut settings = service.settings().await.unwrap();
    settings.profiles[1].polish_timeout_ms = Some(1_000);
    service.save(settings).await.unwrap();
    let profile = service.settings().await.unwrap().profiles[1].clone();
    assert_eq!(profile.polish_timeout_ms(), 2_250); // Always give a configured backup two seconds.
    let job = service.jobs.get(id).await.unwrap();
    service
        .bind_recording(
            job.request.recording_id,
            profile,
            ContextSnapshot::default(),
        )
        .await
        .unwrap();
    let start = std::time::Instant::now();
    let text = service.prepare_delivery(id).await.unwrap();
    assert!(text.original_used);
    assert_eq!(text.text, "שלום hello PRIVATE EXACT VALUE");
    assert!(start.elapsed() >= Duration::from_millis(2_200));
    assert!(start.elapsed() < Duration::from_secs(4));
    let attempt = service.live_processing(id).await.unwrap().attempt.unwrap();
    assert!(matches!(
        attempt.diagnostics.original_reason,
        Some(ErrorCode::ProviderTimeout)
    ));
    assert!(attempt.diagnostics.fallback_attempted);
    assert_eq!(attempt.diagnostics.model_attempts.len(), 2);
    assert!(attempt
        .diagnostics
        .model_attempts
        .iter()
        .all(|m| m.state == "timed_out" && m.elapsed_ms >= 1_900));
    let reopened =
        ModeService::open_with(service.root.clone(), service.jobs.clone(), paced.clone())
            .await
            .unwrap();
    assert_eq!(reopened.delivery_text(id).await.unwrap(), text.text);
    assert_eq!(paced.calls.lock().unwrap().len(), 2);
}

struct RejectedPrimary {
    response: Option<String>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl processing::Processor for RejectedPrimary {
    async fn generate(
        &self,
        model: &ModelSelection,
        _: &str,
        input: &str,
    ) -> Result<String, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if matches!(model.provider, ProviderId::Openai) {
            return self.response.clone().ok_or_else(|| {
                crate::cleanup::failure(ErrorCode::RateLimited, "Model quota reached.", true)
            });
        }
        let input: serde_json::Value = serde_json::from_str(input).unwrap();
        Ok(serde_json::json!({"spans":input["editable_spans"]}).to_string())
    }
}
#[tokio::test]
async fn rate_limit_malformed_and_lost_language_responses_start_backup_immediately() {
    for response in [
        None,
        Some("not json".into()),
        Some(r#"{"spans":["Invented English"]}"#.into()),
        Some(r#"{"spans":["שלום hello 5"]}"#.into()),
    ] {
        let root = Temp::new();
        let (service, id, _) = raced(&root, vec![], Some(5_000)).await;
        let processor = Arc::new(RejectedPrimary {
            response,
            calls: AtomicUsize::new(0),
        });
        let service = ModeService::open_with(
            service.root.clone(),
            service.jobs.clone(),
            processor.clone(),
        )
        .await
        .unwrap();
        let req = request(&service, "super").await;
        let attempt = done(&service, id, service.start(id, req).await.unwrap().id).await;
        assert!(
            matches!(attempt.state, AttemptState::Succeeded { ref text } if text == "שלום hello PRIVATE EXACT VALUE")
        );
        assert!(attempt.diagnostics.fallback_used && !attempt.diagnostics.original_used);
        assert_eq!(
            attempt.diagnostics.selected_model.unwrap().model,
            "fixture/standby"
        );
        assert_eq!(processor.calls.load(Ordering::SeqCst), 2);
        assert!(attempt.diagnostics.processing_ms < 1_000);
        assert_eq!(attempt.diagnostics.model_attempts[0].state, "failed");
        assert_eq!(attempt.diagnostics.model_attempts[1].state, "succeeded");
    }
}
#[tokio::test]
async fn interrupted_super_recovers_original_without_resending_and_old_settings_keep_their_wait() {
    let root = Temp::new();
    let (service, p, job, fake) = fixture(&root, false).await;
    let mut attempt = service
        .start(job.id, request(&service, "regular").await)
        .await
        .unwrap();
    attempt.profile.kind = ModeKind::Super;
    attempt.state = AttemptState::Pending;
    crate::recordings::disk::write_json(
        &service
            .root
            .join(job.id.to_string())
            .join(format!("{}.json", attempt.id)),
        &attempt,
    )
    .await
    .unwrap();
    let reopened = ModeService::open_with(service.root.clone(), p.jobs.clone(), fake.clone())
        .await
        .unwrap();
    let recovered = reopened.attempts(job.id).await.unwrap().remove(0);
    assert!(recovered.diagnostics.original_used);
    assert!(matches!(
        recovered.diagnostics.original_reason,
        Some(ErrorCode::Interrupted)
    ));
    assert!(
        matches!(recovered.state, AttemptState::Succeeded { ref text } if text == "שלום hello PRIVATE EXACT VALUE")
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    let mut document =
        serde_json::to_value(service.settings().await.unwrap().profiles[1].clone()).unwrap();
    document
        .as_object_mut()
        .unwrap()
        .remove("polish_timeout_ms");
    document["fallback_after_ms"] = serde_json::json!(5000);
    document["fallback_model"] =
        serde_json::json!({"provider":"openrouter","model":"fixture/standby"});
    let older: ModeProfile = serde_json::from_value(document).unwrap();
    assert_eq!(older.fallback().unwrap().1, Duration::from_millis(5000));
    assert_eq!(older.polish_timeout_ms(), 7000);
}

#[tokio::test]
async fn a_configured_backup_still_runs_when_primary_selection_is_missing() {
    let root = Temp::new();
    let (service, id, paced) = raced(
        &root,
        vec![("fixture/standby", Duration::from_millis(10), false)],
        Some(5_000),
    )
    .await;
    let mut settings = service.settings().await.unwrap();
    settings.profiles[1].text_model = None;
    service.save(settings).await.unwrap();
    let req = request(&service, "super").await;
    let attempt = done(&service, id, service.start(id, req).await.unwrap().id).await;
    assert!(attempt.diagnostics.fallback_used && !attempt.diagnostics.original_used);
    assert_eq!(attempt.diagnostics.model_attempts.len(), 1);
    assert_eq!(attempt.diagnostics.model_attempts[0].role, "fallback");
    assert_eq!(
        *paced.calls.lock().unwrap(),
        vec!["fixture/standby".to_string()]
    );
}

#[test]
fn only_regular_or_a_mode_with_a_configured_text_model_is_usable() {
    let mut settings = ModeSettings::default();
    let regular = settings
        .profiles
        .iter()
        .position(|p| p.kind == ModeKind::Regular)
        .unwrap();
    let polish = settings
        .profiles
        .iter()
        .position(|p| p.kind == ModeKind::Super)
        .unwrap();
    assert!(settings.profiles[polish].text_model.is_none());
    let mut unknown = settings.profiles[polish].clone();
    unknown.id = "unknown-model".into();
    // No discovery lists this model, configured or not.
    unknown.text_model = Some(ModelSelection {
        provider: crate::transcription::ProviderId::Openai,
        model: "not-a-model-anyone-offers".into(),
    });
    settings.profiles.push(unknown);
    let usable = usable(&settings.profiles);
    assert_eq!(usable.len(), settings.profiles.len());
    assert!(usable[regular]);
    assert!(!usable[polish]);
    assert!(!usable[settings.profiles.len() - 1]);
}

/// The exact same JSON is bundled into Android's instrumentation-only assets.
#[tokio::test]
async fn shared_meaning_cases_reach_durable_delivery_with_raw_audio_and_exact_snippets() {
    struct Output(String);
    #[async_trait::async_trait]
    impl processing::Processor for Output {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            assert!(!input.contains("EXACT -5.25"));
            Ok(serde_json::json!({"spans":[self.0]}).to_string())
        }
    }
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/cleanup/meaning.json"
    ))
    .unwrap();
    let exact = cases["exact_snippet"]["text"].as_str().unwrap();
    let polished: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/cleanup/desktop-polish.json"
    ))
    .unwrap();
    for case in cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(polished["cases"].as_array().unwrap())
    {
        let root = Temp::new();
        let raw = format!(
            "{} {}",
            case["raw"].as_str().unwrap(),
            cases["exact_snippet"]["phrase"].as_str().unwrap()
        );
        let (service, pipeline, job, _) =
            fixture_with_raw_and_snippet(&root, false, &raw, exact).await;
        let mut settings = service.settings().await.unwrap();
        let profile = settings
            .profiles
            .iter_mut()
            .find(|p| p.id == "super")
            .unwrap();
        profile.formatting.self_corrections = case["self_corrections"].as_bool().unwrap_or(false);
        profile.formatting.spoken_punctuation = case["spoken"].as_bool().unwrap();
        profile.formatting.remove_fillers = case["fillers"].as_bool().unwrap();
        service.save(settings).await.unwrap();
        let service = ModeService::open_with(
            service.root.clone(),
            pipeline.jobs.clone(),
            Arc::new(Output(case["output"].as_str().unwrap().into())),
        )
        .await
        .unwrap();
        let profile = service
            .settings()
            .await
            .unwrap()
            .profiles
            .into_iter()
            .find(|p| p.id == "super")
            .unwrap();
        service
            .bind_recording(
                job.request.recording_id,
                profile,
                ContextSnapshot::default(),
            )
            .await
            .unwrap();
        let attempt = service.start_recorded(job.id).await.unwrap().unwrap();
        let attempt = done(&service, job.id, attempt.id).await;
        let accepted = case["accepted"].as_bool().unwrap();
        let expected = format!(
            "{} {exact}",
            case[if accepted { "output" } else { "raw" }]
                .as_str()
                .unwrap()
        );
        assert!(
            matches!(&attempt.state, AttemptState::Succeeded { text } if text == &expected),
            "{}",
            case["id"]
        );
        assert_eq!(attempt.raw_text, raw);
        assert_eq!(
            attempt.diagnostics.original_used, !accepted,
            "{}",
            case["id"]
        );
        if !accepted {
            assert!(matches!(
                attempt.diagnostics.original_reason,
                Some(ErrorCode::InvalidProviderResponse)
            ));
        }
        // Production delivery selection and restart read the same durable outcome.
        let reopened = ModeService::open_with(
            service.root.clone(),
            pipeline.jobs.clone(),
            Arc::new(Output("should never run".into())),
        )
        .await
        .unwrap();
        let selected = reopened.prepare_delivery(job.id).await.unwrap();
        assert_eq!(selected.text, expected, "{}", case["id"]);
        assert_eq!(selected.original_used, !accepted);
        assert_eq!(selected.notice.is_some(), !accepted);
        let saved = pipeline.jobs.get(job.id).await.unwrap();
        let JobStatus::Succeeded { result } = saved.status else {
            panic!()
        };
        assert_eq!(result.transcript.raw_text, raw);
        let audio = pipeline
            .recordings
            .audio(job.request.recording_id)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(audio.path).await.unwrap(), vec![42; 16]);
    }
}

#[tokio::test]
async fn explicit_command_rewriting_uses_selected_text_and_its_own_validation_boundary() {
    struct Rewrite;
    #[async_trait::async_trait]
    impl processing::Processor for Rewrite {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            assert_eq!(input["Selected Text"], "Do not pay 5 dollars");
            Ok(serde_json::json!({"text":"Pay 10 dollars"}).to_string())
        }
    }
    let root = Temp::new();
    let (service, _, job, _) = fixture_with_raw(&root, false, "replace with Pay 10 dollars").await;
    let mut req = request(&service, "command").await;
    req.context.selected_text = "Do not pay 5 dollars".into();
    let attempt = service.start(job.id, req).await.unwrap();
    assert_eq!(
        processing::run(&attempt, &empty_vocabulary(), &Rewrite)
            .await
            .unwrap(),
        "Pay 10 dollars"
    );
    done(&service, job.id, attempt.id).await;
}

#[tokio::test]
async fn pending_context_keeps_raw_available_and_filters_consent_before_processing() {
    let root = Temp::new();
    let (service, pipeline, job, fake) = fixture(&root, false).await;
    let mut profile = service.settings().await.unwrap().profiles.remove(1);
    profile.context.application = true;
    service
        .begin_context_capture(job.request.recording_id, profile)
        .await
        .unwrap();
    service
        .prepare_draft(
            job.request.recording_id,
            "raw preview".into(),
            empty_vocabulary(),
        )
        .await;
    assert_eq!(
        fake.calls.load(Ordering::SeqCst),
        0,
        "pending context must not reach the provider"
    );
    let delivery = {
        let service = service.clone();
        tokio::spawn(async move { service.delivery_text(job.id).await.unwrap() })
    };
    tokio::task::yield_now().await;
    assert!(!delivery.is_finished());
    assert_eq!(
        serde_json::to_value(pipeline.jobs.get(job.id).await.unwrap()).unwrap(),
        serde_json::to_value(&job).unwrap()
    );
    assert!(service
        .complete_context_capture(
            job.request.recording_id,
            Some(ContextSnapshot {
                application: "permitted app".into(),
                selected_text: "unconsented selection".into(),
                clipboard: "unconsented clipboard".into(),
            })
        )
        .await
        .unwrap());
    delivery.await.unwrap();
    let inputs = fake.inputs.lock().await;
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].contains("permitted app"));
    assert!(!inputs[0].contains("unconsented"));
    assert!(!service
        .complete_context_capture(
            job.request.recording_id,
            Some(ContextSnapshot {
                application: "late replacement".into(),
                ..Default::default()
            })
        )
        .await
        .unwrap());
}

#[tokio::test]
async fn failed_context_preserves_raw_and_command_refuses_unverified_selection() {
    for mode in [ModeKind::Super, ModeKind::Command] {
        let root = Temp::new();
        let (service, pipeline, job, fake) = fixture(&root, false).await;
        let profile = service
            .settings()
            .await
            .unwrap()
            .profiles
            .into_iter()
            .find(|p| p.kind == mode)
            .unwrap();
        service
            .begin_context_capture(job.request.recording_id, profile)
            .await
            .unwrap();
        assert!(service
            .complete_context_capture(job.request.recording_id, None)
            .await
            .unwrap());
        let attempt = service.start_recorded(job.id).await.unwrap().unwrap();
        if mode == ModeKind::Command {
            assert!(matches!(attempt.state, AttemptState::Failed { .. }));
            assert!(service.delivery_text(job.id).await.is_err());
        } else {
            assert!(attempt.diagnostics.original_used);
            assert!(service.delivery_text(job.id).await.is_ok());
        }
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            serde_json::to_value(pipeline.jobs.get(job.id).await.unwrap()).unwrap(),
            serde_json::to_value(&job).unwrap()
        );
        assert!(pipeline
            .recordings
            .audio(job.request.recording_id)
            .await
            .is_ok());
    }
}

#[tokio::test]
async fn discarded_pending_context_cannot_reappear_or_overwrite_a_newer_capture() {
    let root = Temp::new();
    let (service, _, job, _) = fixture(&root, false).await;
    let profile = service.settings().await.unwrap().profiles.remove(2);
    let older = job.request.recording_id;
    let newer = Uuid::new_v4();
    service
        .begin_context_capture(older, profile.clone())
        .await
        .unwrap();
    service.discard_capture(older).await.unwrap();
    service.begin_context_capture(newer, profile).await.unwrap();
    let context = ContextSnapshot {
        selected_text: "new selection".into(),
        ..Default::default()
    };
    service
        .complete_context_capture(newer, Some(context.clone()))
        .await
        .unwrap();
    assert!(!service
        .complete_context_capture(
            older,
            Some(ContextSnapshot {
                selected_text: "old selection".into(),
                ..Default::default()
            })
        )
        .await
        .unwrap());
    assert!(service.captured_mode(older).await.unwrap().is_none());
    assert!(service.captured_mode(newer).await.unwrap().unwrap().context == context);
}

#[tokio::test]
async fn interrupted_pending_context_recovers_with_a_bounded_wait_and_rejects_late_result() {
    let root = Temp::new();
    let (service, _, job, fake) = fixture(&root, false).await;
    let profile = service.settings().await.unwrap().profiles.remove(1);
    service
        .begin_context_capture(job.request.recording_id, profile)
        .await
        .unwrap();
    let reopened = ModeService::open_with(service.root.clone(), service.jobs.clone(), fake.clone())
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let delivery = tokio::time::timeout(Duration::from_secs(7), reopened.prepare_delivery(job.id))
        .await
        .unwrap()
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(delivery.original_used);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(!service
        .complete_context_capture(job.request.recording_id, Some(ContextSnapshot::default()))
        .await
        .unwrap());
}

#[test]
fn the_backup_waits_for_twice_the_chosen_models_usual_time() {
    struct Times(processing::ResponseTimes);
    #[async_trait::async_trait]
    impl processing::Processor for Times {
        async fn generate(&self, _: &ModelSelection, _: &str, _: &str) -> Result<String, ApiError> {
            unreachable!()
        }
        fn response_times(&self) -> &processing::ResponseTimes {
            &self.0
        }
    }
    let mut profile = ModeSettings::default().profiles[1].clone();
    profile.text_model = Some(chosen());
    profile.fallback_model = Some(ModelSelection {
        provider: ProviderId::Openrouter,
        model: "fixture/standby".into(),
    });
    let wait = |profile: &ModeProfile, times: &Times| {
        processing::fallback_wait(profile, times)
            .unwrap()
            .1
            .as_millis() as u64
    };
    let times = Times(Default::default());
    // Nothing known yet: 1.5 s is assumed, so the backup waits 3 s.
    assert_eq!(wait(&profile, &times), 3_000);
    assert_eq!(
        processing::polish_timeout(&profile, &times),
        Duration::from_millis(5_000)
    );
    for ms in [1_300, 1_472, 1_900] {
        times.0.record(&chosen(), true, ms);
    }
    assert_eq!(wait(&profile, &times), 2_944);
    // Balanced is a different pace and has its own record.
    profile.fast_processing = false;
    assert_eq!(wait(&profile, &times), 3_000);
    profile.fast_processing = true;
    // A fast model still waits at least the mode's minimum, 1.5 s by default.
    let fast = Times(Default::default());
    for _ in 0..3 {
        fast.0.record(&chosen(), true, 400);
    }
    assert_eq!(wait(&profile, &fast), 1_500);
    profile.fallback_after_ms = Some(5_000);
    assert_eq!(wait(&profile, &fast), 5_000);
    // A very slow model is capped at the longest wait.
    profile.fallback_after_ms = None;
    for _ in 0..3 {
        fast.0.record(&chosen(), true, 60_000);
    }
    assert_eq!(wait(&profile, &fast), 15_000);
    // Without a backup there is nothing to wait for.
    profile.fallback_model = None;
    assert!(processing::fallback_wait(&profile, &fast).is_none());
}
