use super::*;
use crate::providers::{ProviderInfo, SavedAudio, TranscriptionProvider};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer-batch-{}", Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Fake {
    calls: Arc<AtomicUsize>,
    block: bool,
}
#[async_trait::async_trait]
impl TranscriptionProvider for Fake {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::Gemini,
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.block {
            std::future::pending::<()>().await;
        }
        if tokio::fs::read(&audio.path).await.unwrap()[0] == 0 {
            return Err(error(
                ErrorCode::ProviderFailed,
                "Synthetic independent failure",
                false,
            ));
        }
        Ok(Transcript {
            raw_text: "hello שלום 42".into(),
            selection: options.selection.clone(),
            execution_location: ExecutionLocation::Cloud,
            input_language: options.input_language.clone(),
            output_language: options.output_language.clone(),
        })
    }
}
fn options() -> TranscriptionOptions {
    serde_json::from_str::<ContractCatalog>(include_str!(
        "../../../../../packages/contracts/examples.json"
    ))
    .unwrap()
    .transcription_request
    .data
    .options
}
async fn pipeline(root: &Temp, calls: Arc<AtomicUsize>, block: bool) -> Arc<Pipeline> {
    Pipeline::open(
        root.0.clone(),
        1_000_000,
        vec![Arc::new(Fake { calls, block })],
    )
    .await
    .unwrap()
}
async fn source(p: &Pipeline, byte: u8) -> Source {
    let mut metadata = serde_json::from_str::<ContractCatalog>(include_str!(
        "../../../../../packages/contracts/examples.json"
    ))
    .unwrap()
    .recording
    .data
    .metadata;
    metadata.id = Uuid::new_v4();
    metadata.byte_length = 100;
    p.recordings
        .save(
            metadata.clone(),
            Box::new(std::io::Cursor::new(vec![byte; 100])),
        )
        .await
        .unwrap();
    Source {
        recording_id: metadata.id,
        job_id: None,
        version_id: None,
    }
}
fn request(sources: Vec<Source>) -> PlanRequest {
    let mut options = options();
    options.selection.provider = ProviderId::Gemini;
    PlanRequest {
        id: Uuid::new_v4(),
        sources,
        operation: Operation::Transcribe,
        options,
        profile: None,
    }
}
async fn finished(s: &BatchService, id: Uuid) -> Batch {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let b = s.get(id).await.unwrap();
            if b.state != "running" && s.active.lock().await.is_none() {
                return b;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn mixed_items_reuse_invalidation_and_original_retention() {
    let root = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = pipeline(&root, calls.clone(), false).await;
    let good = source(&p, 1).await;
    let bad = source(&p, 0).await;
    let missing = Source {
        recording_id: Uuid::new_v4(),
        job_id: None,
        version_id: None,
    };
    let s = p.batch_service().await.unwrap();
    let req = request(vec![good.clone(), bad.clone(), missing]);
    let b = s.plan(&p, req.clone()).await.unwrap();
    assert_eq!(b.items[2].state, "unavailable");
    assert_eq!(b.likely_paid_items, 2);
    assert!(s.start(p.clone(), b.id, false).await.is_err());
    s.start(p.clone(), b.id, true).await.unwrap();
    let first = finished(&s, b.id).await;
    assert_eq!(
        first
            .items
            .iter()
            .map(|i| i.state.as_str())
            .collect::<Vec<_>>(),
        vec!["succeeded", "failed", "unavailable"]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let original = tokio::fs::read(p.recordings.audio(good.recording_id).await.unwrap().path)
        .await
        .unwrap();
    let mut second_req = req.clone();
    second_req.id = Uuid::new_v4();
    let second = s.plan(&p, second_req).await.unwrap();
    assert!(second.items[0].reuse_available);
    assert!(!second.items[1].reuse_available);
    s.start(p.clone(), second.id, true).await.unwrap();
    let second = finished(&s, second.id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let a = first.items[0].version.as_ref().unwrap();
    let b = second.items[0].version.as_ref().unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(b.reused_from, Some(a.id));
    assert_eq!(b.source.recording_id, good.recording_id);
    for change in ["language", "vocabulary", "model", "output"] {
        let mut changed = request(vec![good.clone()]);
        match change {
            "language" => changed.options.input_language = InputLanguage::He,
            "vocabulary" => changed.options.vocabulary.hints.push("different".into()),
            "model" => changed.options.selection.model = "changed-model".into(),
            _ => {
                changed.options.output_language = OutputLanguage::Translate {
                    language: TextLanguage::En,
                }
            }
        }
        let plan = s.plan(&p, changed).await.unwrap();
        assert!(!plan.items[0].reuse_available, "{change}");
    }
    assert_eq!(
        original,
        tokio::fs::read(p.recordings.audio(good.recording_id).await.unwrap().path)
            .await
            .unwrap()
    );
    assert!(p.jobs.all().await.unwrap().is_empty());
    assert_eq!(
        s.get(first.id).await.unwrap().items[0]
            .version
            .as_ref()
            .unwrap()
            .raw_text,
        "hello שלום 42"
    );
}
#[tokio::test]
async fn cancellation_stops_inflight_and_does_not_start_later_items() {
    let root = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = pipeline(&root, calls.clone(), true).await;
    let s = p.batch_service().await.unwrap();
    let b = s
        .plan(&p, request(vec![source(&p, 1).await, source(&p, 2).await]))
        .await
        .unwrap();
    s.start(p.clone(), b.id, true).await.unwrap();
    while calls.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(s.begin_deletion().await.is_err());
    s.cancel(b.id).await.unwrap();
    let b = finished(&s, b.id).await;
    assert_eq!(b.state, "canceled");
    assert!(b.items.iter().all(|i| i.state == "canceled"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(b.items.iter().all(|i| i.version.is_none()));
}
#[tokio::test]
async fn restart_pauses_and_resume_reuses_committed_versions_only() {
    let root = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = pipeline(&root, calls.clone(), false).await;
    let s = p.batch_service().await.unwrap();
    let b = s
        .plan(&p, request(vec![source(&p, 1).await]))
        .await
        .unwrap();
    let id = b.id;
    let mut persisted = b;
    persisted.state = "running".into();
    persisted.items[0].state = "running".into();
    s.save(&persisted).await.unwrap();
    drop(s);
    drop(p);
    let p = pipeline(&root, calls.clone(), false).await;
    let s = p.batch_service().await.unwrap();
    let recovered = s.get(id).await.unwrap();
    assert_eq!(recovered.state, "paused");
    assert_eq!(recovered.items[0].state, "interrupted");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    s.start(p.clone(), id, true).await.unwrap();
    assert_eq!(finished(&s, id).await.items[0].state, "succeeded");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn cleanup_without_audio_retains_raw_and_separate_outputs() {
    let root = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = pipeline(&root, calls.clone(), false).await;
    let mut src = source(&p, 1).await;
    let job = p
        .jobs
        .submit(TranscriptionRequest {
            recording_id: src.recording_id,
            idempotency_key: Uuid::new_v4(),
            retry_of_job_id: None,
            options: request(vec![]).options,
        })
        .await
        .unwrap();
    p.run_pending().await.unwrap();
    src.job_id = Some(job.id);
    let before = p.jobs.get(job.id).await.unwrap();
    p.recordings.delete(src.recording_id).await.unwrap();
    let s = p.batch_service().await.unwrap();
    let mut req = request(vec![src.clone()]);
    req.operation = Operation::Cleanup;
    req.profile = Some(
        p.mode_service()
            .await
            .unwrap()
            .settings()
            .await
            .unwrap()
            .profiles[0]
            .clone(),
    );
    req.options.vocabulary.corrections.push(Correction {
        from: "hello".into(),
        to: "hi".into(),
    });
    let b = s.plan(&p, req).await.unwrap();
    assert!(!b.paid_work);
    s.start(p.clone(), b.id, false).await.unwrap();
    let b = finished(&s, b.id).await;
    let version = b.items[0].version.as_ref().unwrap();
    assert_eq!(version.raw_text, "hello שלום 42");
    assert_eq!(version.text, "hi שלום 42");
    assert_eq!(version.source.job_id, Some(job.id));
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(p.jobs.get(job.id).await.unwrap()).unwrap()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    s.clear_selected(&[src.recording_id]).await.unwrap();
    assert!(s.list().await.unwrap().is_empty());
}
#[tokio::test]
async fn bounds_idempotency_and_hash_changes() {
    let root = Temp::new();
    let p = pipeline(&root, Arc::new(AtomicUsize::new(0)), false).await;
    let s = p.batch_service().await.unwrap();
    let src = source(&p, 1).await;
    assert!(s.plan(&p, request(vec![])).await.is_err());
    assert!(s.plan(&p, request(vec![src.clone(); 33])).await.is_err());
    assert!(s.plan(&p, request(vec![src.clone(); 2])).await.is_err());
    let req = request(vec![src.clone()]);
    let b = s.plan(&p, req.clone()).await.unwrap();
    assert_eq!(s.plan(&p, req.clone()).await.unwrap().id, b.id);
    let mut conflict = req;
    conflict.options.selection.model = "different".into();
    assert!(s.plan(&p, conflict).await.is_err());
    let audio = p.recordings.audio(src.recording_id).await.unwrap();
    tokio::fs::write(audio.path, vec![2; 100]).await.unwrap();
    s.start(p.clone(), b.id, true).await.unwrap();
    assert_eq!(finished(&s, b.id).await.items[0].state, "failed");
}

#[tokio::test]
async fn historical_raw_versions_can_be_selected_for_cleanup_without_live_job_events() {
    let root = Temp::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = pipeline(&root, calls.clone(), false).await;
    let source = source(&p, 1).await;
    let mut events = p.jobs.subscribe();
    let s = p.batch_service().await.unwrap();
    let raw = s.plan(&p, request(vec![source.clone()])).await.unwrap();
    s.start(p.clone(), raw.id, true).await.unwrap();
    let raw = finished(&s, raw.id).await;
    let raw_version = raw.items[0].version.as_ref().unwrap();
    let mut next = request(vec![Source {
        recording_id: source.recording_id,
        job_id: None,
        version_id: Some(raw_version.id),
    }]);
    next.operation = Operation::Cleanup;
    next.profile = Some(
        p.mode_service()
            .await
            .unwrap()
            .settings()
            .await
            .unwrap()
            .profiles[0]
            .clone(),
    );
    next.options.vocabulary.corrections.push(Correction {
        from: "hello".into(),
        to: "Hi".into(),
    });
    let cleanup = s.plan(&p, next).await.unwrap();
    s.start(p.clone(), cleanup.id, false).await.unwrap();
    let cleanup = finished(&s, cleanup.id).await;
    let result = cleanup.items[0].version.as_ref().unwrap();
    assert_eq!(result.text, "Hi שלום 42");
    assert_eq!(result.raw_text, "hello שלום 42");
    assert_eq!(result.source.version_id, Some(raw_version.id));
    assert!(events.try_recv().is_err());
    assert!(p.jobs.all().await.unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
