//! Opt-in cloud text replay, independent of microphone, ASR, settings and paste.
use super::*;
#[derive(Deserialize)]
struct Sample {
    name: String,
    draft: String,
    final_text: String,
    profile: ModeProfile,
    context: ContextSnapshot,
    vocabulary: Vocabulary,
}
#[derive(Deserialize)]
struct Manifest {
    output: std::path::PathBuf,
    samples: Vec<Sample>,
    repeats: usize,
}
#[tokio::test]
#[ignore = "Requires explicitly configured Gemini and private WRITER_SUPER_REUSE_MANIFEST; see super-reuse-efficiency.md"]
async fn completed_draft_replay() {
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(std::env::var_os("WRITER_SUPER_REUSE_MANIFEST").unwrap()).unwrap(),
    )
    .unwrap();
    assert!((1..=3).contains(&manifest.repeats));
    assert!(!manifest.output.exists(), "Use a fresh private output file");
    let provider = Arc::new(crate::cleanup::provider::CloudText::from_env());
    let mut receipts = vec![];
    for repeat in 0..manifest.repeats {
        for sample in &manifest.samples {
            assert!(sample
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'));
            let mut draft = super::tests::attempt();
            draft.raw_text = sample.draft.clone();
            draft.profile = sample.profile.clone();
            draft.request.context = sample.context.clone();
            assert!(matches!(
                draft.profile.text_model.as_ref().unwrap().provider,
                ProviderId::Gemini
            ));
            let mut final_input = draft.clone();
            final_input.raw_text = sample.final_text.clone();
            for reuse in if repeat % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let id = Uuid::new_v4();
                let cache = Drafts::default();
                let mut preparation_ms = 0;
                if reuse {
                    cache.begin(id).await;
                    let start = Instant::now();
                    cache
                        .released(
                            id,
                            draft.clone(),
                            sample.vocabulary.clone(),
                            provider.clone(),
                        )
                        .await;
                    super::tests::finish(&cache, id).await;
                    preparation_ms = start.elapsed().as_millis() as u64;
                }
                let start = Instant::now();
                let prepared = cache.take(id, &final_input, &sample.vocabulary).await;
                let measured = diagnostics::measure(processing::run_prepared(
                    &final_input,
                    &sample.vocabulary,
                    provider.as_ref(),
                    prepared,
                ))
                .await;
                let wait_ms = start.elapsed().as_millis() as u64;
                let text = measured.0.unwrap();
                println!(
                    "{}",
                    serde_json::json!({"sample":sample.name,"repeat":repeat,"prepared":reuse,"preparation_ms":preparation_ms,"final_wait_ms":wait_ms,"reused":measured.1.reused_draft})
                );
                receipts.push(serde_json::json!({"sample":sample.name,"repeat":repeat,"prepared":reuse,"preparation_ms":preparation_ms,"final_wait_ms":wait_ms,"text":text,"diagnostics":measured.1,"trace":crate::latency::snapshot(id)}));
                std::fs::write(
                    &manifest.output,
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
    }
}
