//! MW-123: the polish is told the speaker's words and learned mishearings.
use super::*;
use crate::vocabulary::learning::Mishearing;

fn pair(heard: &str, meant: &str, when: Option<&str>) -> Mishearing {
    Mishearing {
        heard: heard.into(),
        meant: meant.into(),
        when: when.map(Into::into),
    }
}

struct Recorder(std::sync::Mutex<Vec<(String, String)>>);
#[async_trait::async_trait]
impl processing::Processor for Recorder {
    async fn generate(
        &self,
        _: &ModelSelection,
        instructions: &str,
        input: &str,
    ) -> Result<String, ApiError> {
        self.0
            .lock()
            .unwrap()
            .push((instructions.into(), input.into()));
        let input: serde_json::Value = serde_json::from_str(input).unwrap();
        Ok(serde_json::json!({"spans": input["editable_spans"]}).to_string())
    }
}

#[test]
fn the_glossary_holds_words_and_pairs_but_never_snippet_values() {
    let vocabulary = Vocabulary {
        hints: vec!["Claude".into(), "worktree".into(), " claude ".into()],
        corrections: vec![Correction {
            from: "met ricks".into(),
            to: "Metrix".into(),
        }],
        snippets: vec![Snippet {
            phrase: "my email".into(),
            text: "PRIVATE VALUE".into(),
        }],
    };
    let glossary = Glossary::new(
        &vocabulary,
        vec![
            pair("marriage", "merge", Some("near PR or branch")),
            pair("cloud", "Claude", None),
        ],
    );
    assert_eq!(glossary.terms, vec!["Claude", "worktree", "merge"]);
    assert_eq!(glossary.mishearings.len(), 2);
    assert!(!serde_json::to_string(&glossary)
        .unwrap()
        .contains("PRIVATE VALUE"));
    assert!(Glossary::new(&empty_vocabulary(), vec![]).is_empty());
}

#[tokio::test]
async fn the_polish_receives_the_glossary_as_data_with_fixed_rules() {
    let root = Temp::new();
    let (service, _, job, _) = fixture_with_raw(&root, false, "please marriage the PR").await;
    let request = request(&service, "super").await;
    let mut attempt = service.start(job.id, request.clone()).await.unwrap();
    done(&service, job.id, request.operation_id).await;
    attempt.glossary = Glossary::new(
        &Vocabulary {
            hints: vec!["Claude Code".into()],
            ..empty_vocabulary()
        },
        vec![pair("marriage", "merge", Some("near PR or branch"))],
    );
    let recorder = Recorder(Default::default());
    processing::run(&attempt, &empty_vocabulary(), &recorder)
        .await
        .unwrap();
    attempt.glossary = Glossary::default();
    processing::run(&attempt, &empty_vocabulary(), &recorder)
        .await
        .unwrap();
    let calls = recorder.0.lock().unwrap();
    let (instructions, input) = &calls[0];
    assert!(instructions.contains("Change a word only to fix a clear recognition error"));
    assert!(instructions.contains("never replace it with a different plausible word"));
    let input: serde_json::Value = serde_json::from_str(input).unwrap();
    assert_eq!(
        input["Glossary"],
        serde_json::json!(["Claude Code", "merge"])
    );
    assert_eq!(
        input["Known Mishearings"],
        serde_json::json!([{"heard": "marriage", "meant": "merge", "when": "near PR or branch"}])
    );
    // Without anything learned the request carries no glossary, and the
    // instructions stay the same so a provider's prompt cache still applies.
    let (plain_instructions, plain) = &calls[1];
    assert_eq!(plain_instructions, instructions);
    let plain: serde_json::Value = serde_json::from_str(plain).unwrap();
    assert!(plain.get("Glossary").is_none() && plain.get("Known Mishearings").is_none());
}

#[tokio::test]
async fn an_attempt_snapshots_what_was_learned_when_it_started() {
    let root = Temp::new();
    let (service, _, job, fake) = fixture_with_raw(&root, false, "please marriage the PR").await;
    service
        .learning
        .set_mishearings(vec![pair("marriage", "merge", None)])
        .await
        .unwrap();
    let request = request(&service, "super").await;
    service.start(job.id, request.clone()).await.unwrap();
    let saved = done(&service, job.id, request.operation_id).await;
    assert_eq!(
        saved.glossary.mishearings,
        vec![pair("marriage", "merge", None)]
    );
    let inputs = fake.inputs.lock().await;
    let sent: serde_json::Value = serde_json::from_str(&inputs[0]).unwrap();
    assert_eq!(sent["Glossary"], serde_json::json!(["merge"]));
    // Learning more later does not rewrite what an earlier attempt was given.
    service.learning.set_mishearings(vec![]).await.unwrap();
    let again = service.attempts(job.id).await.unwrap();
    assert_eq!(again[0].glossary.mishearings.len(), 1);
}
