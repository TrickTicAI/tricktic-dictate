use super::*;
use crate::vocabulary::empty_vocabulary;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Format {
    calls: AtomicUsize,
    ending: &'static str,
}
#[async_trait::async_trait]
impl processing::Processor for Format {
    async fn generate(&self, _: &ModelSelection, _: &str, input: &str) -> Result<String, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let input: serde_json::Value = serde_json::from_str(input).unwrap();
        let spans: Vec<_> = input["editable_spans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| format!("{}{}", s.as_str().unwrap(), self.ending))
            .collect();
        Ok(serde_json::json!({"spans": spans}).to_string())
    }
}
fn input(raw: &str) -> ProcessingAttempt {
    let mut attempt = super::tests::attempt();
    attempt.raw_text = raw.into();
    attempt
}
async fn prepared(
    raw: &str,
    ending: &'static str,
    vocabulary: Vocabulary,
) -> (Drafts, Uuid, Arc<Format>) {
    let cache = Drafts::default();
    let id = Uuid::new_v4();
    let processor = Arc::new(Format {
        calls: AtomicUsize::new(0),
        ending,
    });
    cache.begin(id).await;
    cache
        .released(id, input(raw), vocabulary, processor.clone())
        .await;
    super::tests::finish(&cache, id).await;
    (cache, id, processor)
}
#[tokio::test]
async fn completed_sentence_ending_reuses_without_release_rebilling() {
    for (raw, ending) in [
        ("can we make it faster", "?"),
        ("this is ready now", "."),
        ("that looks excellent", "!"),
        ("אפשר לעשות את זה מהר יותר", "?"),
        ("אפשר לשלוח את ה pull request", "?"),
    ] {
        let (cache, id, provider) = prepared(raw, ending, empty_vocabulary()).await;
        let final_input = input(&format!("{raw}{ending}"));
        // A later preview after the release draft started sends nothing more.
        cache
            .offer(
                id,
                final_input.clone(),
                empty_vocabulary(),
                provider.clone(),
            )
            .await;
        cache.catch_up(id, provider.clone()).await;
        let (result, metrics) = super::tests::taken(&cache, id, &final_input, &empty_vocabulary())
            .await
            .unwrap();
        assert_eq!(result.unwrap(), final_input.raw_text);
        assert!(metrics.reused_draft && metrics.prepared_at_release);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            crate::latency::snapshot(id)["details"]["mode_draft_reuse_match"],
            "sentence_ending"
        );
    }
}
#[tokio::test]
async fn endings_do_not_relax_words_punctuation_or_output_agreement() {
    for (raw, final_raw, output_ending) in [
        ("can we make it faster", "can we make it faster?", "."),
        ("can we make it faster", "can we make it faster?", "??"),
        ("can we make it faster", "can we make it faster?", ""),
        ("can we make it faster", "can we make it faster?!", "?!"),
        ("can we make it faster", "can we make it faster...", "..."),
        ("can we make it faster", "can we make it faster…", "…"),
        ("can we make it faster", "can we make it faster:", ":"),
        ("can we make it faster", "can we make it slower?", "?"),
        ("can we make it faster", "can we, make it faster?", "?"),
        ("can we make it faster", "can we make it faster ?", "?"),
        ("can we make it faster?", "can we make it faster", ""),
        ("the version is 3", "the version is 3.", "."),
        ("please visit example.com", "please visit example.com.", "."),
        ("please say 'hello'", "please say 'hello'.", "."),
        ("use the function call()", "use the function call().", "."),
    ] {
        let (cache, id, _) = prepared(raw, output_ending, empty_vocabulary()).await;
        assert!(
            cache
                .take(id, &input(final_raw), &empty_vocabulary())
                .await
                .is_none(),
            "{raw} -> {final_raw}"
        );
    }
}
#[tokio::test]
async fn ending_reuse_requires_unchanged_full_request_and_vocabulary_boundaries() {
    for mutation in 0..5 {
        let (cache, id, _) = prepared("can we make it faster", "?", empty_vocabulary()).await;
        let mut final_input = input("can we make it faster?");
        let mut vocabulary = empty_vocabulary();
        match mutation {
            0 => final_input.profile.instructions.push_str(" More formal."),
            1 => final_input.profile.formatting.lists = false,
            2 => final_input.request.context.clipboard = "new reference".into(),
            3 => final_input.prompt_version += 1,
            _ => vocabulary.hints.push("technical word".into()),
        }
        assert!(cache.take(id, &final_input, &vocabulary).await.is_none());
    }
    for phrase in ["faster", "faster?"] {
        let mut vocabulary = empty_vocabulary();
        vocabulary.snippets.push(crate::transcription::Snippet {
            phrase: phrase.into(),
            text: "PROTECTED?".into(),
        });
        let (cache, id, _) = prepared("can we make it faster", "?", vocabulary.clone()).await;
        assert!(cache
            .take(id, &input("can we make it faster?"), &vocabulary)
            .await
            .is_none());
    }
    let mut vocabulary = empty_vocabulary();
    vocabulary.snippets.push(crate::transcription::Snippet {
        phrase: "signature".into(),
        text: "Exact\nSnippet".into(),
    });
    let (cache, id, _) = prepared("signature can we make it faster", "?", vocabulary.clone()).await;
    let text = super::tests::taken(
        &cache,
        id,
        &input("signature can we make it faster?"),
        &vocabulary,
    )
    .await
    .unwrap()
    .0
    .unwrap();
    assert_eq!(text, "Exact\nSnippet can we make it faster?");
}
#[tokio::test]
async fn punctuation_never_waits_for_an_unproven_inflight_result() {
    let cache = Drafts::default();
    let id = Uuid::new_v4();
    cache.begin(id).await;
    let processor = Arc::new(super::release_tests::Slow(AtomicUsize::new(0)));
    cache
        .released(
            id,
            input("can we make it faster"),
            empty_vocabulary(),
            processor,
        )
        .await;
    // Candidate exists, but its provider hasn't returned any validated text yet.
    assert!(cache
        .take(id, &input("can we make it faster?"), &empty_vocabulary())
        .await
        .is_none());
}
