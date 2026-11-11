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

/// MW-120: only explicitly enumerated weekly jobs; no settings or history writes.
#[tokio::test]
#[ignore = "Requires weekly manifest, native Windows credentials and paid replay consent"]
async fn weekly_long_dictations() {
    let root = std::path::PathBuf::from(std::env::var_os("WRITER_REPLAY_ROOT").unwrap());
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("WRITER_WEEKLY_MANIFEST").unwrap()).unwrap(),
    )
    .unwrap();
    let output = std::env::var_os("WRITER_REPLAY_OUTPUT").unwrap();
    assert!(!std::path::Path::new(&output).exists());
    let models = std::sync::Arc::new(crate::cleanup::custom::CustomModels::open(Some(
        root.join("State/text-models.json"),
    )));
    let capacity = crate::limits::work::WorkCapacity::new(crate::limits::Limits::default());
    let provider = CloudText::replay_credentials(&Credentials::new(&root.join("State")))
        .with_custom(models)
        .with_budget(capacity.account());
    let settings: ModeSettings =
        serde_json::from_slice(&std::fs::read(root.join("State/modes/settings.json")).unwrap())
            .unwrap();
    let profile = settings.profiles.iter().find(|p| p.id == "super").unwrap();
    assert_eq!(profile.polish_check, PolishCheck::Off);
    let mut receipts = vec![];
    for row in manifest.as_array().unwrap() {
        let id: Uuid = row["id"].as_str().unwrap().parse().unwrap();
        assert!(row["audio_ms"].as_u64().unwrap() > 60_000);
        let job: crate::transcription::Job = serde_json::from_slice(
            &std::fs::read(root.join(format!("Data/jobs/{id}.json"))).unwrap(),
        )
        .unwrap();
        let mut attempt: ProcessingAttempt = serde_json::from_slice(
            &std::fs::read(root.join(format!("State/modes/{id}/{id}.json"))).unwrap(),
        )
        .unwrap();
        // Use the user's current selected model and Off check without saving anything.
        attempt.profile = profile.clone();
        let started = std::time::Instant::now();
        let (result, metrics) = diagnostics::measure(tokio::time::timeout(
            std::time::Duration::from_millis(5_000),
            processing::run(&attempt, &job.request.options.vocabulary, &provider),
        ))
        .await;
        let (success, error) = match result {
            Ok(Ok(_)) => (true, None),
            Ok(Err(e)) => (false, Some(e.code)),
            Err(_) => (
                false,
                Some(crate::transcription::ErrorCode::ProviderTimeout),
            ),
        };
        receipts.push(serde_json::json!({"id":id,"success":success,"error":error,"polish_ms":started.elapsed().as_millis(),
            "historical_stop_to_delivery_ms":row["s2d"],"historical_polish_ms":row["m_ms"], "diagnostics":metrics}));
        // Private timings/IDs only; no prompts, text, credentials or provider errors.
        std::fs::write(&output, serde_json::to_vec_pretty(&receipts).unwrap()).unwrap();
        println!(
            "replayed={} failures={}",
            receipts.len(),
            receipts.iter().filter(|r| r["success"] == false).count()
        );
    }
}

#[test]
#[ignore = "Requires weekly job manifest and native read-only saved profiles; no provider calls"]
fn weekly_snippets() {
    let root = std::path::PathBuf::from(std::env::var_os("WRITER_REPLAY_ROOT").unwrap());
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("WRITER_WEEKLY_MANIFEST").unwrap()).unwrap(),
    )
    .unwrap();
    let (mut checked, mut inline_changed, mut standalone) = (0, 0, 0);
    for row in manifest.as_array().unwrap() {
        let id: Uuid = row["id"].as_str().unwrap().parse().unwrap();
        let path = root.join(format!("State/modes/{id}/{id}.json"));
        if !path.exists() {
            continue;
        }
        let job: crate::transcription::Job = serde_json::from_slice(
            &std::fs::read(root.join(format!("Data/jobs/{id}.json"))).unwrap(),
        )
        .unwrap();
        let attempt: ProcessingAttempt =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let vocabulary = &job.request.options.vocabulary;
        let processed = crate::vocabulary::process(&attempt.raw_text, vocabulary).unwrap();
        let mut no_snippets = vocabulary.clone();
        no_snippets.snippets.clear();
        let ordinary = crate::vocabulary::process(&attempt.raw_text, &no_snippets)
            .unwrap()
            .text();
        if processed
            .cleanup_inputs()
            .iter()
            .any(|s| s.chars().any(char::is_alphanumeric))
        {
            assert!(processed.text() == ordinary, "Inline snippet expanded");
            inline_changed += usize::from(attempt.input_text != ordinary);
        } else if processed.text() != ordinary {
            standalone += 1;
        }
        checked += 1;
    }
    println!("checked={checked} inline_expansions_removed={inline_changed} standalone_expansions={standalone}");
    assert!(checked > 0);
}

/// MW-123: polish configurations compared on the personal gold set. Each clip's
/// saved transcript is polished by every configuration in turn (the order
/// rotates per clip), with the production deadline and backup rules. Paid
/// requests for the listed gold clips only; the profile, its credentials and
/// its history are read, never written.
#[tokio::test]
#[ignore = "Requires a gold polish plan, native Windows credentials and paid replay consent"]
async fn gold_polish() {
    #[derive(Deserialize)]
    struct Config {
        name: String,
        model: ModelSelection,
        #[serde(default)]
        fallback: Option<ModelSelection>,
        #[serde(default)]
        fallback_after_ms: Option<u64>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        glossary: bool,
    }
    #[derive(Deserialize)]
    struct Clip {
        job: Uuid,
        #[serde(default)]
        mishearings: Vec<crate::vocabulary::learning::Mishearing>,
        /// The delivered dictation's stop-to-delivery and polish times, so a
        /// replay can estimate its own stop-to-delivery.
        #[serde(default)]
        s2d_ms: Option<u64>,
        #[serde(default)]
        polish_ms: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Plan {
        configs: Vec<Config>,
        clips: Vec<Clip>,
    }
    let root = std::path::PathBuf::from(std::env::var_os("WRITER_REPLAY_ROOT").unwrap());
    let plan: Plan = serde_json::from_slice(
        &std::fs::read(std::env::var_os("WRITER_GOLD_POLISH").unwrap()).unwrap(),
    )
    .unwrap();
    let output = std::env::var_os("WRITER_REPLAY_OUTPUT").unwrap();
    assert!(!std::path::Path::new(&output).exists());
    let models = std::sync::Arc::new(crate::cleanup::custom::CustomModels::open(Some(
        root.join("State/text-models.json"),
    )));
    let capacity = crate::limits::work::WorkCapacity::new(crate::limits::Limits::default());
    let provider = CloudText::replay_credentials(&Credentials::new(&root.join("State")))
        .with_custom(models)
        .with_budget(capacity.account());
    let settings: ModeSettings =
        serde_json::from_slice(&std::fs::read(root.join("State/modes/settings.json")).unwrap())
            .unwrap();
    let base = settings.profiles.iter().find(|p| p.id == "super").unwrap();
    let mut runs: std::collections::BTreeMap<&str, serde_json::Map<String, serde_json::Value>> =
        plan.configs
            .iter()
            .map(|c| (c.name.as_str(), Default::default()))
            .collect();
    for (index, clip) in plan.clips.iter().enumerate() {
        let id = clip.job;
        let job: crate::transcription::Job = serde_json::from_slice(
            &std::fs::read(root.join(format!("Data/jobs/{id}.json"))).unwrap(),
        )
        .unwrap();
        let Ok(saved) = std::fs::read(root.join(format!("State/modes/{id}/{id}.json"))) else {
            continue;
        };
        let mut attempt: ProcessingAttempt = serde_json::from_slice(&saved).unwrap();
        let vocabulary = &job.request.options.vocabulary;
        for turn in 0..plan.configs.len() {
            let config = &plan.configs[(index + turn) % plan.configs.len()];
            let mut profile = base.clone();
            profile.text_model = Some(config.model.clone());
            profile.fallback_model = config.fallback.clone();
            profile.fallback_after_ms = config.fallback_after_ms;
            profile.polish_timeout_ms = config.timeout_ms.or(profile.polish_timeout_ms);
            attempt.profile = profile;
            attempt.glossary = if config.glossary {
                Glossary::new(vocabulary, clip.mishearings.clone())
            } else {
                Glossary::default()
            };
            let timeout = processing::polish_timeout(&attempt.profile, &provider);
            let metrics = std::sync::Arc::new(std::sync::Mutex::new(Diagnostics::default()));
            let started = std::time::Instant::now();
            let (result, diagnostics) = diagnostics::measure_into(
                metrics.clone(),
                processing::within_deadline(
                    timeout,
                    &metrics,
                    processing::run(&attempt, vocabulary, &provider),
                ),
            )
            .await;
            let polish_ms = started.elapsed().as_millis() as u64;
            let (polished, error) = match result {
                Some(Ok(text)) => (text, None),
                Some(Err(e)) => (attempt.input_text.clone(), Some(e.code)),
                None => (
                    attempt.input_text.clone(),
                    Some(crate::transcription::ErrorCode::ProviderTimeout),
                ),
            };
            let s2d_ms = clip
                .s2d_ms
                .zip(clip.polish_ms)
                .map(|(s2d, before)| s2d.saturating_sub(before) + polish_ms);
            runs.get_mut(config.name.as_str()).unwrap().insert(
                id.to_string(),
                serde_json::json!({"text": attempt.raw_text, "polished": polished, "polish_ms": polish_ms,
                    "s2d_ms": s2d_ms, "original_used": error.is_some(), "error_code": error,
                    "fallback_used": diagnostics.fallback_used}),
            );
        }
        // Private texts stay in the output file; the console shows counts only.
        let out: serde_json::Map<String, serde_json::Value> = plan
            .configs
            .iter()
            .map(|c| {
                (
                    c.name.clone(),
                    serde_json::json!({"config": {"name": c.name, "model": c.model, "fallback": c.fallback,
                        "glossary": c.glossary, "timeout_ms": c.timeout_ms}, "clips": runs[c.name.as_str()]}),
                )
            })
            .collect();
        std::fs::write(&output, serde_json::to_vec(&out).unwrap()).unwrap();
        println!("replayed={} of {}", index + 1, plan.clips.len());
    }
}
