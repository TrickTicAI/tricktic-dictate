//! Explicit native diagnostic: reads saved inputs, never mutates the source profile.
use super::*;
use crate::{cleanup::provider::CloudText, desktop_settings::credentials::Credentials};

struct Replay {
    provider: CloudText,
    output: std::sync::Mutex<Option<String>>,
    times: processing::ResponseTimes,
}
impl Replay {
    fn new(root: &std::path::Path) -> Self {
        Self {
            provider: CloudText::replay_credentials(&Credentials::new(&root.join("State"))),
            output: Default::default(),
            times: Default::default(),
        }
    }
}
#[async_trait::async_trait]
impl processing::Processor for Replay {
    fn response_times(&self) -> &processing::ResponseTimes {
        &self.times
    }
    async fn generate(
        &self,
        model: &ModelSelection,
        instructions: &str,
        input: &str,
    ) -> Result<String, ApiError> {
        self.generate_mode(model, instructions, input, false).await
    }
    async fn generate_mode(
        &self,
        model: &ModelSelection,
        instructions: &str,
        input: &str,
        fast: bool,
    ) -> Result<String, ApiError> {
        let result = self
            .provider
            .generate_with_options(model, instructions, input, fast)
            .await;
        *self.output.lock().unwrap() = result.as_ref().ok().cloned();
        result
    }
}

#[tokio::test]
#[ignore = "requires explicit Windows profile, job IDs, and paid-provider replay consent"]
async fn saved_polishing() {
    let root =
        std::path::PathBuf::from(std::env::var_os("WRITER_REPLAY_ROOT").expect("profile root"));
    let ids = std::env::var("WRITER_REPLAY_JOBS").expect("comma-separated job IDs");
    let provider = Replay::new(&root);
    let mut receipts = vec![];
    for id in ids.split(',') {
        let id: Uuid = id.parse().expect("job UUID");
        let job: crate::transcription::Job = serde_json::from_slice(
            &std::fs::read(root.join(format!("Data/jobs/{id}.json"))).unwrap(),
        )
        .unwrap();
        let saved: ProcessingAttempt = serde_json::from_slice(
            &std::fs::read(root.join(format!("State/modes/{id}/{id}.json"))).unwrap(),
        )
        .unwrap();
        for role in ["primary", "fallback"] {
            let mut attempt = saved.clone();
            if role == "fallback" {
                attempt.profile.text_model = attempt.profile.fallback_model.clone();
            }
            attempt.profile.fallback_model = None;
            let (result, metrics) = diagnostics::measure(processing::run(
                &attempt,
                &job.request.options.vocabulary,
                &provider,
            ))
            .await;
            println!(
                "{}",
                serde_json::json!({
                    "job_id": id, "role": role, "succeeded": result.is_ok(),
                    "error": result.as_ref().err(), "diagnostics": metrics,
                    "changed": result.as_ref().ok().map(|text| text != &saved.input_text),
                })
            );
            receipts.push(serde_json::json!({"job_id": id, "role": role,
                "raw_text": saved.raw_text, "upstream_output": provider.output.lock().unwrap().clone(),
                "result": result, "diagnostics": metrics}));
        }
    }
    if let Some(path) = std::env::var_os("WRITER_REPLAY_OUTPUT") {
        let mut options = std::fs::OpenOptions::new();
        use std::io::Write;
        let mut file = options
            .write(true)
            .create_new(true)
            .open(path)
            .expect("new private receipt path");
        file.write_all(&serde_json::to_vec_pretty(&receipts).unwrap())
            .unwrap();
    }
}

/// MW-117 before/after on saved dictations: the fixed one-second backup wait that
/// shipped before against the adaptive wait, with the profile's current chosen
/// and backup models, alternating which goes first. Release drafts overlap speech
/// recognition, which is not replayed here. Receipts hold timings only, no text.
#[tokio::test]
#[ignore = "requires explicit Windows profile, job IDs, and paid-provider replay consent"]
async fn backup_wait_before_after() {
    let root =
        std::path::PathBuf::from(std::env::var_os("WRITER_REPLAY_ROOT").expect("profile root"));
    let ids = std::env::var("WRITER_REPLAY_JOBS").expect("comma-separated job IDs");
    let output = std::env::var_os("WRITER_REPLAY_OUTPUT").expect("new private receipt path");
    let settings: ModeSettings =
        serde_json::from_slice(&std::fs::read(root.join("State/modes/settings.json")).unwrap())
            .unwrap();
    let current = settings.profiles.iter().find(|p| p.id == "super").unwrap();
    assert!(current.text_model.is_some() && current.fallback_model.is_some());
    // A usual time this short leaves the old fixed wait in charge. Each run gets a
    // fresh one, since every answer would otherwise teach it the adaptive wait.
    let fixed = || {
        let before = Replay::new(&root);
        for _ in 0..3 {
            before.times.record(
                current.text_model.as_ref().unwrap(),
                current.fast_processing,
                1,
            );
        }
        before
    };
    let after = Replay::new(&root);
    let mut receipts = vec![];
    for (index, id) in ids.split(',').enumerate() {
        let id: Uuid = id.parse().expect("job UUID");
        let job: crate::transcription::Job = serde_json::from_slice(
            &std::fs::read(root.join(format!("Data/jobs/{id}.json"))).unwrap(),
        )
        .unwrap();
        let mut attempt: ProcessingAttempt = serde_json::from_slice(
            &std::fs::read(root.join(format!("State/modes/{id}/{id}.json"))).unwrap(),
        )
        .unwrap();
        attempt.profile.text_model = current.text_model.clone();
        attempt.profile.fallback_model = current.fallback_model.clone();
        attempt.profile.fast_processing = current.fast_processing;
        attempt.profile.polish_timeout_ms = current.polish_timeout_ms;
        let language = serde_json::to_value(&job.request.options.input_language).unwrap();
        let order = if index % 2 == 0 {
            ["before", "after"]
        } else {
            ["after", "before"]
        };
        for policy in order {
            let before = fixed();
            let (processor, wait) = if policy == "before" {
                (&before, Some(1_000))
            } else {
                (&after, current.fallback_after_ms)
            };
            attempt.profile.fallback_after_ms = wait;
            let backup_after_ms = processing::fallback_wait(&attempt.profile, processor)
                .map(|(_, wait)| wait.as_millis() as u64);
            let timeout = processing::polish_timeout(&attempt.profile, processor);
            let (result, metrics) = diagnostics::measure(async {
                tokio::time::timeout(
                    timeout,
                    processing::run(&attempt, &job.request.options.vocabulary, processor),
                )
                .await
                .unwrap_or_else(|_| {
                    diagnostics::update(processing::timed_out);
                    Err(processing::invalid("timed out"))
                })
            })
            .await;
            let receipt = serde_json::json!({
                "job_id": id, "language": language, "policy": policy,
                "input_bytes": attempt.raw_text.len(),
                "backup_after_ms": backup_after_ms, "timeout_ms": timeout.as_millis() as u64,
                "ms": metrics.processing_ms, "succeeded": result.is_ok(),
                "error": result.as_ref().err().map(|e| e.code.clone()),
                "requests": metrics.model_attempts.len(),
                "fallback_used": metrics.fallback_used,
                "calls": metrics.model_attempts.iter().map(|c| serde_json::json!({
                    "role": c.role, "state": c.state, "elapsed_ms": c.elapsed_ms,
                    "started_after_ms": c.started_after_ms,
                })).collect::<Vec<_>>(),
            });
            println!("{receipt}");
            receipts.push(receipt);
        }
    }
    let mut options = std::fs::OpenOptions::new();
    use std::io::Write;
    options
        .write(true)
        .create_new(true)
        .open(output)
        .expect("new private receipt path")
        .write_all(&serde_json::to_vec_pretty(&receipts).unwrap())
        .unwrap();
}
