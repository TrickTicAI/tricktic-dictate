use super::*;
use crate::transcription::{
    Correction, Envelope, ErrorEnvelope, HintEntry, Snippet, Vocabulary, VocabularyDocument,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;

#[test]
fn shared_android_vocabulary_cases() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/vocabulary/parity.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let raw = case["raw"].as_str().unwrap();
        let vocabulary: Vocabulary = serde_json::from_value(case["vocabulary"].clone()).unwrap();
        let processed = process(raw, &vocabulary).unwrap();
        assert_eq!(
            processed.text(),
            case["expected"].as_str().unwrap(),
            "{raw:?}"
        );
        assert_eq!(processed.raw_text, raw);
    }
}

fn fixture() -> Vocabulary {
    Vocabulary {
        hints: vec!["Metrix".into(), "דניאל".into()],
        corrections: vec![
            Correction {
                from: "met ricks".into(),
                to: "Metrix".into(),
            },
            Correction {
                from: "Writer".into(),
                to: "WRONG".into(),
            },
        ],
        snippets: vec![
            Snippet {
                phrase: "my email".into(),
                text: "Writer.Name+Notes@example.com".into(),
            },
            Snippet {
                phrase: "המייל שלי".into(),
                text: "שלום Writer@example.com!\nבברכה".into(),
            },
        ],
    }
}
/// The same entries as they are saved: every one of them switched on.
fn document() -> VocabularyDocument {
    VocabularyDocument::from_vocabulary(fixture())
}
#[test]
fn literal_repeated_expansions_preserve_raw_and_do_not_cascade() {
    let raw = "Send (my email), my email! met ricks; my emails; xmy email; My email.";
    let result = process(raw, &fixture()).unwrap();
    assert_eq!(result.raw_text, raw);
    assert_eq!(result.text(), "Send (Writer.Name+Notes@example.com), Writer.Name+Notes@example.com! Metrix; my emails; xmy email; Writer.Name+Notes@example.com.");
}
#[test]
fn hebrew_marks_boundaries_and_mixed_text() {
    let raw =
        "המייל שלי, והמייל שלי; המייל שליָ; _my email; my email2; my email\u{301}; 😀my email😀";
    assert_eq!(process(raw, &fixture()).unwrap().text(), "שלום Writer@example.com!\nבברכה, והמייל שלי; המייל שליָ; _my email; my email2; my email\u{301}; 😀Writer.Name+Notes@example.com😀");
}
#[test]
fn precedence_is_snippet_then_longest_then_first_and_left_to_right() {
    let mut v = empty_vocabulary();
    v.snippets = vec![
        Snippet {
            phrase: "a".into(),
            text: "short".into(),
        },
        Snippet {
            phrase: "a b".into(),
            text: "first".into(),
        },
        Snippet {
            phrase: "a b".into(),
            text: "second".into(),
        },
        Snippet {
            phrase: "b c".into(),
            text: "later".into(),
        },
    ];
    v.corrections = vec![Correction {
        from: "a b c".into(),
        to: "correction".into(),
    }];
    assert_eq!(process("a b c a", &v).unwrap().text(), "first c short");
}
#[test]
fn punctuation_and_replacement_whitespace_are_literal() {
    let mut v = empty_vocabulary();
    v.snippets.push(Snippet {
        phrase: "email!".into(),
        text: "  Name+Tag@example.com\n$1 \\ literal  ".into(),
    });
    assert_eq!(
        process("(email!), email!x", &v).unwrap().text(),
        "(  Name+Tag@example.com\n$1 \\ literal  ), email!x"
    );
    assert_eq!(process("", &v).unwrap().text(), "");
}
/// Real cases from 2026-09-18 dictations into payment-form fields:
/// " My email." delivered " alex@example.com." and " my city,"
/// delivered " Springfield,". The stored value must arrive on its own.
#[test]
fn a_dictation_that_is_only_snippets_delivers_them_without_recognition_marks() {
    let mut v = fixture();
    v.snippets.push(Snippet {
        phrase: "my city".into(),
        text: "Springfield".into(),
    });
    for (raw, expected) in [
        (" My email.", "Writer.Name+Notes@example.com"),
        (" my city,", "Springfield"),
        ("My email", "Writer.Name+Notes@example.com"),
        (" my email. ", "Writer.Name+Notes@example.com"),
        // Two spoken triggers keep what was dictated between them.
        (
            " my city, my email.",
            "Springfield, Writer.Name+Notes@example.com",
        ),
        // The stored value's own spacing and punctuation are never trimmed.
        (" המייל שלי.", "שלום Writer@example.com!\nבברכה"),
        // One spoken word outside the snippet makes this an ordinary dictation.
        (
            " My email. Thanks",
            " Writer.Name+Notes@example.com. Thanks",
        ),
    ] {
        assert_eq!(process(raw, &v).unwrap().text(), expected, "raw {raw:?}");
    }
    // Without a snippet there is nothing to isolate; the transcript is unchanged.
    assert_eq!(process(" ... ", &v).unwrap().text(), " ... ");
}
#[test]
fn cleanup_never_receives_or_changes_exact_values() {
    let result = process("Send my email please; met ricks.", &fixture()).unwrap();
    assert_eq!(result.cleanup_inputs(), ["Send ", " please; ", "."]);
    assert_eq!(
        result
            .restore_cleaned(&["Use ".into(), " now: ".into(), "!".into()])
            .unwrap(),
        "Use Writer.Name+Notes@example.com now: Metrix!"
    );
    assert!(result.restore_cleaned(&[]).is_err());
    let exact = process("my email", &fixture()).unwrap();
    assert_eq!(
        exact.restore_cleaned(&[]).unwrap(),
        "Writer.Name+Notes@example.com"
    );
}
#[test]
fn hints_are_explicit_and_validation_rejects_empty_triggers() {
    assert_eq!(
        prepare_hints(&fixture(), true).unwrap(),
        HintPlan::Supported(fixture().hints)
    );
    assert!(matches!(
        prepare_hints(&fixture(), false).unwrap(),
        HintPlan::Unsupported { requested: 2, .. }
    ));
    assert_eq!(
        prepare_hints(&empty_vocabulary(), false).unwrap(),
        HintPlan::Empty
    );
    let mut v = fixture();
    v.snippets[0].phrase = " ".into();
    assert!(process("hello", &v).is_err());
    v = fixture();
    v.hints = vec!["x".into(); 201];
    assert!(validate(&v).is_err());
}

#[test]
fn a_switched_off_entry_is_kept_in_place_and_never_reaches_transcription() {
    let mut saved = document();
    saved.hints[0].enabled = false;
    saved.snippets[0].enabled = false;
    let active = saved.active();
    assert_eq!(active.hints, vec!["דניאל".to_string()]);
    // The switched-off snippet stops expanding; the rule below it still runs.
    assert_eq!(
        process("my email met ricks", &active).unwrap().text(),
        "my email Metrix"
    );
    // It is still stored, in the position its precedence depends on.
    assert_eq!(saved.hints.len(), 2);
    assert_eq!(saved.snippets[0].phrase, "my email");
    assert_eq!(saved.active().snippets.len(), 1);
}
#[test]
fn a_switched_off_entry_is_still_held_to_the_stored_limits() {
    let mut saved = document();
    saved.corrections[0].enabled = false;
    saved.corrections[0].to = " ".into();
    assert!(validate_document(&saved).is_err());
    saved = document();
    saved.hints = vec![
        HintEntry {
            id: None,
            text: "x".into(),
            enabled: false,
        };
        201
    ];
    assert!(validate_document(&saved).is_err());
}

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer-vocabulary-test-{}", uuid::Uuid::new_v4())))
    }
    fn store(&self) -> VocabularyStore {
        VocabularyStore::new(self.0.join("vocabulary.json"))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn storage_survives_recreation_and_rejects_invalid_updates_without_loss() {
    let temp = Temp::new();
    let store = temp.store();
    assert!(store.load().await.unwrap().hints.is_empty());
    store.save(&document()).await.unwrap();
    assert_eq!(
        temp.store().load().await.unwrap().snippets[0].text,
        fixture().snippets[0].text
    );
    let mut invalid = document();
    invalid.snippets[0].text = "".into();
    assert!(store.save(&invalid).await.is_err());
    assert_eq!(
        store.load().await.unwrap().snippets[0].text,
        fixture().snippets[0].text
    );
    store.save(&empty_document()).await.unwrap();
    assert!(temp.store().load().await.unwrap().snippets.is_empty());
    std::fs::write(temp.0.join("vocabulary.json"), b"broken").unwrap();
    assert!(store.load().await.is_err());
}

#[tokio::test]
async fn concurrent_saves_leave_one_complete_document() {
    let temp = Temp::new();
    let store = Arc::new(temp.store());
    let mut tasks = vec![];
    for n in 0..12 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            let mut v = document();
            v.hints = vec![HintEntry {
                id: None,
                text: n.to_string(),
                enabled: true,
            }];
            store.save(&v).await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(store.load().await.unwrap().snippets.len(), 2);
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
}

#[tokio::test]
async fn a_dictionary_saved_before_switches_loads_with_every_entry_on() {
    let temp = Temp::new();
    std::fs::create_dir_all(&temp.0).unwrap();
    std::fs::write(
        temp.0.join("vocabulary.json"),
        serde_json::to_vec(&fixture()).unwrap(),
    )
    .unwrap();
    let loaded = temp.store().load().await.unwrap();
    assert!(loaded.hints.iter().all(|hint| hint.enabled));
    assert_eq!(loaded.active().hints, fixture().hints);
    assert_eq!(loaded.active().snippets.len(), fixture().snippets.len());
    // Saving rewrites it in the shape that can carry a switch, without asking.
    temp.store().save(&loaded).await.unwrap();
    let stored = std::fs::read_to_string(temp.0.join("vocabulary.json")).unwrap();
    assert!(stored.contains("enabled"));
    assert_eq!(
        temp.store().load().await.unwrap().active().snippets.len(),
        fixture().snippets.len()
    );
}

#[tokio::test]
async fn http_persists_edits_deletion_and_returns_versioned_errors() {
    let temp = Temp::new();
    let app = routes_with_store(Arc::new(temp.store()));
    for value in [document(), empty_document()] {
        let body = serde_json::to_vec(&Envelope::new(value.clone())).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/vocabulary")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = routes_with_store(Arc::new(temp.store()))
            .oneshot(
                Request::builder()
                    .uri("/api/v1/vocabulary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let saved: Envelope<VocabularyDocument> =
            serde_json::from_slice(&to_bytes(response.into_body(), 300000).await.unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(saved.data.active()).unwrap(),
            serde_json::to_value(value.active()).unwrap()
        );
    }
    for (body, status) in [
        (r#"{"api_version":"2","data":{}}"#.to_string(), StatusCode::BAD_REQUEST),
        (r#"{"api_version":"1","data":{"hints":[{"text":" ","enabled":true}],"corrections":[],"snippets":[]}}"#.into(), StatusCode::UNPROCESSABLE_ENTITY),
        (r#"{"api_version":"1","data":{"hints":[],"corrections":[],"snippets":[],"extra":true}}"#.into(), StatusCode::BAD_REQUEST),
        // Reading a dictionary written before the switch existed is deliberate;
        // writing one back is not, so the entry shape is required on the way in.
        (r#"{"api_version":"1","data":{"hints":["Metrix"],"corrections":[],"snippets":[]}}"#.into(), StatusCode::BAD_REQUEST),
        ("x".repeat(300000), StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let response = app.clone().oneshot(Request::builder().method("PUT").uri("/api/v1/vocabulary").header("content-type", "application/json").body(Body::from(body)).unwrap()).await.unwrap();
        assert_eq!(response.status(), status);
        let _: ErrorEnvelope = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    }
    std::fs::write(temp.0.join("vocabulary.json"), b"invalid").unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/vocabulary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let error: ErrorEnvelope =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(matches!(
        error.error.code,
        crate::transcription::ErrorCode::StorageFailed
    ));
}

#[test]
fn spoken_snippets_ignore_case_and_spacing_without_rewriting_raw_or_corrections() {
    let mut v = fixture();
    v.snippets.push(Snippet {
        phrase: "my number".into(),
        text: "+1 202 555 0100".into(),
    });
    v.snippets.push(Snippet {
        phrase: "Émail שלי".into(),
        text: "Exact\nVALUE".into(),
    });
    let raw = "My number. MY   NUMBER! my\nnumber? éMAIL שלי; Met ricks; my numbers; xmy number.";
    let output = process(raw, &v).unwrap();
    assert_eq!(output.raw_text, raw);
    assert_eq!(output.text(), "+1 202 555 0100. +1 202 555 0100! +1 202 555 0100? Exact\nVALUE; Met ricks; my numbers; xmy number.");
}

/// MW-OUATH-11 live: sync applied another device's edit while this device was
/// typing, and the page's whole-document save overwrote it. A save that names
/// the version it was edited from must not replace a newer stored document.
#[tokio::test]
async fn http_refuses_a_save_based_on_a_changed_dictionary() {
    let temp = Temp::new();
    let store = Arc::new(temp.store());
    let app = routes_with_store(store.clone());
    let get = || {
        app.clone().oneshot(
            Request::builder()
                .uri("/api/v1/vocabulary")
                .body(Body::empty())
                .unwrap(),
        )
    };
    let put = |value: VocabularyDocument, tag: Option<String>| {
        let mut request = Request::builder()
            .method("PUT")
            .uri("/api/v1/vocabulary")
            .header("content-type", "application/json");
        if let Some(tag) = tag {
            request = request.header("if-match", tag);
        }
        app.clone().oneshot(
            request
                .body(Body::from(
                    serde_json::to_vec(&Envelope::new(value)).unwrap(),
                ))
                .unwrap(),
        )
    };
    let etag = |r: &axum::response::Response| {
        r.headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    let loaded = etag(&get().await.unwrap());

    // Sync (or another window) stores a different dictionary meanwhile.
    let mut synced = document();
    synced.hints[0].text = "from another device".into();
    store.save(&synced).await.unwrap();

    let mut typed = document();
    typed.hints[0].text = "typed here".into();
    let refused = put(typed.clone(), Some(loaded)).await.unwrap();
    assert_eq!(refused.status(), StatusCode::PRECONDITION_FAILED);
    let _: ErrorEnvelope =
        serde_json::from_slice(&to_bytes(refused.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        store.load().await.unwrap().hints[0].text,
        "from another device"
    );

    // Edited from the current version: saved, and the reply carries the new tag.
    let current = etag(&get().await.unwrap());
    let saved = put(typed.clone(), Some(current.clone())).await.unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let next = etag(&saved);
    assert_ne!(next, current);
    assert_eq!(etag(&get().await.unwrap()), next);
    // Without If-Match the last writer still wins, as before.
    assert_eq!(put(synced, None).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn http_returns_persisted_ids_before_sync_and_repeated_edits_keep_them() {
    let temp = Temp::new();
    std::fs::create_dir_all(&temp.0).unwrap();
    // An older installation has no identities yet. GET migrates before exposing a tag.
    std::fs::write(
        temp.0.join("vocabulary.json"),
        serde_json::to_vec(&document()).unwrap(),
    )
    .unwrap();
    let store = Arc::new(temp.store());
    let app = routes_with_store(store.clone());
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/vocabulary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut version = first.headers()["etag"].to_str().unwrap().to_owned();
    let mut edited: Envelope<VocabularyDocument> =
        serde_json::from_slice(&to_bytes(first.into_body(), 300000).await.unwrap()).unwrap();
    let original_ids = edited.data.ids();
    assert!(original_ids.iter().all(Option::is_some));
    assert!(
        !edited.data.assign_ids(),
        "sync must have no metadata rewrite left to perform"
    );
    assert_eq!(store.load_tagged().await.unwrap().1, version);
    assert_eq!(temp.store().load().await.unwrap().ids(), original_ids);

    // Edit each kind, add a word, then delete and undo it. Each response is the
    // exact persisted document; later sync reads cannot invalidate its version.
    for step in 0..6 {
        match step {
            0 => edited.data.hints[0].text = "שלום Writer".into(),
            1 => edited.data.corrections[0].to = "Mixed עברית".into(),
            2 => edited.data.snippets[0].enabled = false,
            3 => edited.data.hints.push(HintEntry {
                id: None,
                text: "new word".into(),
                enabled: true,
            }),
            4 => {
                edited.data.hints.pop();
            }
            5 => edited.data.hints.push(HintEntry {
                id: None,
                text: "new word".into(),
                enabled: true,
            }),
            _ => unreachable!(),
        }
        let reply = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/vocabulary")
                    .header("content-type", "application/json")
                    .header("if-match", &version)
                    .body(Body::from(serde_json::to_vec(&edited).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::OK, "step {step}");
        version = reply.headers()["etag"].to_str().unwrap().to_owned();
        edited =
            serde_json::from_slice(&to_bytes(reply.into_body(), 300000).await.unwrap()).unwrap();
        assert!(edited.data.ids().iter().all(Option::is_some));
        assert!(!edited.data.assign_ids());
        let (persisted, current) = store.load_tagged().await.unwrap();
        assert_eq!(current, version);
        assert_eq!(
            serde_json::to_value(&persisted).unwrap(),
            serde_json::to_value(&edited.data).unwrap()
        );
        assert_eq!(persisted.hints[0].id, original_ids[0]);
        assert_eq!(persisted.corrections[0].id, original_ids[2]);
        assert_eq!(persisted.snippets[0].id, original_ids[4]);
    }
}

#[tokio::test]
async fn oversized_identity_migration_keeps_the_old_dictionary_readable_and_unchanged() {
    let temp = Temp::new();
    std::fs::create_dir_all(&temp.0).unwrap();
    let mut old = empty_document();
    old.hints = (0..32)
        .map(|_| HintEntry {
            id: None,
            text: "x".repeat(8120),
            enabled: false,
        })
        .collect();
    validate_document(&old).unwrap();
    let mut migration = old.clone();
    migration.assign_ids();
    assert!(validate_document(&migration).is_err());
    let bytes = serde_json::to_vec(&old).unwrap();
    std::fs::write(temp.0.join("vocabulary.json"), &bytes).unwrap();
    let (loaded, tag) = temp.store().load_tagged().await.unwrap();
    assert_eq!(loaded.ids(), old.ids());
    assert_eq!(
        std::fs::read(temp.0.join("vocabulary.json")).unwrap(),
        bytes
    );
    // Reducing the old document allows a normal conditional save and migration.
    let mut reduced = loaded;
    reduced.hints.pop();
    let (saved, _) = temp
        .store()
        .save_if(&reduced, Some(&tag))
        .await
        .unwrap()
        .unwrap();
    assert!(saved.ids().iter().all(Option::is_some));
}
