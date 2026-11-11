use super::*;

fn mishearing(heard: &str, meant: &str) -> Suggestion {
    Suggestion::Mishearing {
        heard: heard.into(),
        meant: meant.into(),
    }
}
fn term(text: &str) -> Suggestion {
    Suggestion::Term { text: text.into() }
}

#[test]
fn a_correction_proposes_pairs_and_terms() {
    let found = suggest(
        "Please marriage the PR, then ask cloud to check the work tree.",
        "Please merge the PR, then ask Claude to check the worktree.",
        &[],
        &Learning::default(),
    );
    assert_eq!(
        found,
        vec![
            mishearing("marriage", "merge"),
            mishearing("cloud", "Claude"),
            term("Claude"),
            mishearing("work tree", "worktree"),
        ]
    );
}

#[test]
fn a_change_of_case_alone_proposes_only_a_term() {
    let found = suggest(
        "so codex opened it. The fix is small",
        "So Codex opened it. The fix is small",
        &[],
        &Learning::default(),
    );
    // "So" starts the sentence and "The" is unchanged: neither is a term.
    assert_eq!(found, vec![term("Codex")]);
}

#[test]
fn everyday_rewording_proposes_no_term() {
    let found = suggest(
        "I want to by bread",
        "I want to buy bread",
        &[],
        &Learning::default(),
    );
    assert_eq!(found, vec![mishearing("by", "buy")]);
}

#[test]
fn long_rewrites_teach_nothing() {
    let found = suggest(
        "and then we will do the thing with the stuff",
        "Ship it.",
        &[],
        &Learning::default(),
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn known_learned_and_declined_suggestions_are_never_proposed() {
    let learning = Learning {
        mishearings: vec![Mishearing {
            heard: "Marriage".into(),
            meant: "merge".into(),
            when: None,
        }],
        declined: vec![mishearing("cloud", "claude").key()],
    };
    let found = suggest(
        "marriage it and tell cloud",
        "merge it and tell Claude",
        &["claude".into()],
        &learning,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn identifiers_with_digits_and_inner_capitals_are_terms() {
    let found = suggest(
        "look at CRM two thirty six on git hub",
        "look at CRM-236 on GitHub",
        &[],
        &Learning::default(),
    );
    assert!(found.contains(&term("CRM-236")), "{found:?}");
    assert!(found.contains(&term("GitHub")), "{found:?}");
    assert!(
        found.contains(&mishearing("git hub", "GitHub")),
        "{found:?}"
    );
}

struct Temp(std::path::PathBuf);
impl Temp {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn stores() -> (Temp, LearningStore, VocabularyStore) {
    let dir =
        Temp(std::env::temp_dir().join(format!("writer-learning-test-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(dir.path()).unwrap();
    let learning = LearningStore::new(dir.path().join("vocabulary-learning.json"));
    let dictionary = VocabularyStore::new(dir.path().join("vocabulary.json"));
    (dir, learning, dictionary)
}

#[tokio::test]
async fn accepting_and_declining_is_remembered() {
    let (_dir, learning, dictionary) = stores();
    learning
        .decide(&mishearing("marriage", "merge"), true, &dictionary)
        .await
        .unwrap();
    learning
        .decide(&term("Claude"), true, &dictionary)
        .await
        .unwrap();
    // Accepting a term twice keeps one dictionary word.
    learning
        .decide(&term("claude"), true, &dictionary)
        .await
        .unwrap();
    let saved = learning
        .decide(&mishearing("cloud", "Claude"), false, &dictionary)
        .await
        .unwrap();
    assert_eq!(
        saved.mishearings,
        vec![Mishearing {
            heard: "marriage".into(),
            meant: "merge".into(),
            when: None
        }]
    );
    let words: Vec<_> = dictionary
        .load()
        .await
        .unwrap()
        .hints
        .into_iter()
        .map(|h| (h.text, h.enabled))
        .collect();
    assert_eq!(words, vec![("Claude".to_string(), true)]);

    // A new store reads the same file: nothing is proposed again after restart.
    let reopened = LearningStore::new(learning.path.clone());
    let terms: Vec<String> = dictionary
        .load()
        .await
        .unwrap()
        .hints
        .into_iter()
        .map(|h| h.text)
        .collect();
    let again = suggest(
        "marriage it, cloud",
        "merge it, Claude",
        &terms,
        &reopened.load().await.unwrap(),
    );
    assert!(again.is_empty(), "{again:?}");
}

#[tokio::test]
async fn editing_pairs_keeps_declined_suggestions() {
    let (_dir, learning, dictionary) = stores();
    learning
        .decide(&mishearing("cloud", "Claude"), false, &dictionary)
        .await
        .unwrap();
    learning
        .decide(&mishearing("marriage", "merge"), true, &dictionary)
        .await
        .unwrap();
    let saved = learning.set_mishearings(vec![]).await.unwrap();
    assert!(saved.mishearings.is_empty());
    assert_eq!(saved.declined, vec![mishearing("cloud", "claude").key()]);
    assert!(learning
        .set_mishearings(vec![Mishearing {
            heard: "same".into(),
            meant: "Same".into(),
            when: None
        }])
        .await
        .is_err());
}

#[tokio::test]
async fn an_unreadable_store_gives_the_polish_no_pairs() {
    let (dir, learning, _) = stores();
    std::fs::write(dir.path().join("vocabulary-learning.json"), b"{not json").unwrap();
    assert!(learning.mishearings().await.is_empty());
    assert!(learning.load().await.is_err());
}

#[tokio::test]
async fn routes_suggest_and_decide() {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;
    let (dir, _, _) = stores();
    let app = routes_with(
        Arc::new(LearningStore::new(
            dir.path().join("vocabulary-learning.json"),
        )),
        Arc::new(VocabularyStore::new(dir.path().join("vocabulary.json"))),
    );
    let post = |path: &str, body: serde_json::Value| {
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"api_version": "1", "data": body}).to_string(),
            ))
            .unwrap()
    };
    let response = app
        .clone()
        .oneshot(post(
            "/api/v1/vocabulary/suggestions",
            serde_json::json!({"heard": "ask cloud", "corrected": "ask Claude"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(
        body["data"],
        serde_json::json!([
            {"kind": "mishearing", "heard": "cloud", "meant": "Claude"},
            {"kind": "term", "text": "Claude"}
        ])
    );
    let response = app
        .clone()
        .oneshot(post(
            "/api/v1/vocabulary/suggestions/decision",
            serde_json::json!({"suggestion": {"kind": "term", "text": "Claude"}, "accept": false}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(post(
            "/api/v1/vocabulary/suggestions",
            serde_json::json!({"heard": "ask cloud", "corrected": "ask Claude"}),
        ))
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(
        body["data"],
        serde_json::json!([{"kind": "mishearing", "heard": "cloud", "meant": "Claude"}])
    );
}

/// Writes what each gold correction would propose, for the scorecard's
/// cross-fitted polish replay (`scorecard.py polish-plan`). Private input and
/// output; prints counts only.
#[test]
#[ignore = "Requires a private file of {clip: {heard, corrected}} and an output path"]
fn gold_suggestions() {
    let input = std::env::var_os("WRITER_GOLD_CORRECTIONS").expect("corrections file");
    let output = std::env::var_os("WRITER_GOLD_SUGGESTIONS").expect("new output path");
    let corrections: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let proposed: std::collections::BTreeMap<_, _> = corrections
        .iter()
        .map(|(clip, c)| {
            let found = suggest(
                c["heard"].as_str().unwrap(),
                c["corrected"].as_str().unwrap(),
                &[],
                &Learning::default(),
            );
            (clip.clone(), found)
        })
        .collect();
    std::fs::write(&output, serde_json::to_vec(&proposed).unwrap()).unwrap();
    println!(
        "clips={} suggestions={}",
        proposed.len(),
        proposed.values().map(Vec::len).sum::<usize>()
    );
}
