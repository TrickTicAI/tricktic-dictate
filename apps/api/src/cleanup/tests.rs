use super::*;
use crate::{jobs::Pipeline, recordings::RecordingStore};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer-cleanup-{}", Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> ContractCatalog {
    serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap()
}
fn selection() -> ModelSelection {
    ModelSelection {
        provider: ProviderId::Openai,
        model: "gpt-4.1-mini".into(),
    }
}
async fn saved(root: &Temp, raw: &str, vocabulary: Vocabulary) -> (Arc<Pipeline>, Job) {
    let p = Pipeline::open(root.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let f = fixture();
    let mut metadata = f.recording.data.metadata;
    metadata.byte_length = 16;
    p.recordings
        .save(metadata, Box::new(std::io::Cursor::new(vec![42; 16])))
        .await
        .unwrap();
    let mut request = f.transcription_request.data;
    request.options.vocabulary = vocabulary;
    request.options.cleanup = CleanupPolicy::Off {};
    let job = p.jobs.submit(request).await.unwrap();
    p.jobs
        .set_status(job.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    let JobStatus::Succeeded { mut result } = f.job.data.status else {
        panic!()
    };
    result.transcript.raw_text = raw.into();
    result.cleanup = CleanupResult::Off {};
    let job = p
        .jobs
        .set_status(job.id, JobStatus::Succeeded { result })
        .await
        .unwrap();
    (p, job)
}
struct Fake {
    mode: &'static str,
    calls: AtomicUsize,
    inputs: Mutex<Vec<String>>,
}
impl Fake {
    fn new(mode: &'static str) -> Arc<Self> {
        Arc::new(Self {
            mode,
            calls: AtomicUsize::new(0),
            inputs: Mutex::new(vec![]),
        })
    }
}
#[async_trait::async_trait]
impl TextProvider for Fake {
    async fn clean(&self, _: &ModelSelection, spans: &[String]) -> Result<Vec<String>, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.inputs.lock().await = spans.to_vec();
        match self.mode {
            "slow" => {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(spans.to_vec())
            }
            "pause" => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(spans.to_vec())
            }
            "error" => Err(failure(
                ErrorCode::ProviderFailed,
                "Cleanup synthetic failure",
                true,
            )),
            "panic" => panic!("synthetic cleanup panic"),
            "drift" => Ok(vec!["Translated into English.".into(); spans.len()]),
            "omit" => Ok(vec![]),
            _ => Ok(spans.iter().map(|s| s.trim().to_uppercase()).collect()),
        }
    }
}
fn service_with(
    p: &Pipeline,
    fake: Arc<dyn TextProvider>,
    timeout: Duration,
) -> Arc<CleanupService> {
    Arc::new(CleanupService {
        mobile_only: false,
        jobs: p.jobs.clone(),
        provider: fake,
        active: Mutex::new(BTreeSet::new()),
        timeout,
    })
}
async fn done(service: &CleanupService, id: Uuid) -> Job {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !service.active.lock().await.contains(&id) {
                return service.jobs.get(id).await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
fn result(job: &Job) -> &TranscriptionResult {
    let JobStatus::Succeeded { result } = &job.status else {
        panic!("raw result must remain successful")
    };
    result
}

#[tokio::test]
async fn opt_in_protected_values_success_raw_and_audio_durable() {
    let temp = Temp::new();
    let vocabulary = Vocabulary {
        hints: vec![],
        corrections: vec![Correction {
            from: "metrixx".into(),
            to: "Metrix".into(),
        }],
        snippets: vec![Snippet {
            phrase: "my signature".into(),
            text: "AlEx\nexact@EXAMPLE.com\nשלום".into(),
        }],
    };
    let raw = "hello my signature and mail Foo+tag@EXAMPLE.com metrixx שלום English";
    let (p, job) = saved(&temp, raw, vocabulary).await;
    let fake = Fake::new("success");
    let service = service_with(&p, fake.clone(), TIMEOUT);
    assert!(matches!(
        result(&p.jobs.get(job.id).await.unwrap()).cleanup,
        CleanupResult::Off {}
    ));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    let pending = service.start(job.id, selection()).await.unwrap();
    assert_eq!(result(&pending).transcript.raw_text, raw);
    assert!(matches!(
        result(&pending).cleanup,
        CleanupResult::Pending {}
    ));
    let finished = done(&service, job.id).await;
    let CleanupResult::Succeeded { cleaned_text, .. } = &result(&finished).cleanup else {
        panic!()
    };
    assert_eq!(
        cleaned_text,
        "HELLO AlEx\nexact@EXAMPLE.com\nשלום AND MAIL Foo+tag@EXAMPLE.com Metrix שלום ENGLISH"
    );
    assert_eq!(result(&finished).transcript.raw_text, raw);
    let sent = fake.inputs.lock().await.join("|");
    assert!(!sent.contains('@'));
    assert!(!sent.contains("AlEx"));
    assert!(!sent.contains("Metrix"));
    assert_eq!(
        tokio::fs::read(
            p.recordings
                .audio(job.request.recording_id)
                .await
                .unwrap()
                .path
        )
        .await
        .unwrap(),
        vec![42; 16]
    );
    drop(service);
    drop(p);
    let reopened = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(reopened.jobs.get(job.id).await.unwrap()).unwrap(),
        serde_json::to_value(finished).unwrap()
    );
}
#[tokio::test]
async fn errors_timeout_drift_and_crash_keep_original_retryable() {
    for mode in ["error", "slow", "drift", "omit", "panic"] {
        let temp = Temp::new();
        let (p, job) = saved(&temp, "שלום hello", crate::vocabulary::empty_vocabulary()).await;
        let service = service_with(&p, Fake::new(mode), Duration::from_millis(30));
        service.start(job.id, selection()).await.unwrap();
        assert_eq!(
            result(&p.jobs.get(job.id).await.unwrap())
                .transcript
                .raw_text,
            "שלום hello"
        );
        let failed = done(&service, job.id).await;
        let CleanupResult::Failed { error } = &result(&failed).cleanup else {
            panic!("expected cleanup-only failure")
        };
        if mode == "slow" {
            assert!(matches!(error.code, ErrorCode::ProviderTimeout));
        }
        if mode == "drift" {
            assert!(matches!(error.code, ErrorCode::InvalidProviderResponse));
        }
        assert_eq!(result(&failed).transcript.raw_text, "שלום hello");
        assert!(p.recordings.audio(job.request.recording_id).await.is_ok());
        let retry = service_with(&p, Fake::new("success"), TIMEOUT);
        retry.start(job.id, selection()).await.unwrap();
        assert!(matches!(
            result(&done(&retry, job.id).await).cleanup,
            CleanupResult::Succeeded { .. }
        ));
    }
}
#[tokio::test]
async fn concurrent_posts_share_worker_and_pending_after_restart_can_retry() {
    let temp = Temp::new();
    let (p, job) = saved(&temp, "hello", crate::vocabulary::empty_vocabulary()).await;
    let fake = Fake::new("pause");
    let service = service_with(&p, fake.clone(), TIMEOUT);
    let (a, b) = tokio::join!(
        service.start(job.id, selection()),
        service.start(job.id, selection())
    );
    assert!(a.is_ok() && b.is_ok());
    done(&service, job.id).await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    let mut original = result(&job).clone();
    original.cleanup = CleanupResult::Pending {};
    p.jobs
        .set_status(job.id, JobStatus::Succeeded { result: original })
        .await
        .unwrap();
    drop(service);
    drop(p);
    let p = Pipeline::open(temp.0.clone(), 1_000_000, vec![])
        .await
        .unwrap();
    let fake = Fake::new("success");
    let service = service_with(&p, fake.clone(), TIMEOUT);
    assert_eq!(
        fake.calls.load(Ordering::SeqCst),
        0,
        "restart must not silently bill another call"
    );
    service.start(job.id, selection()).await.unwrap();
    assert!(matches!(
        result(&done(&service, job.id).await).cleanup,
        CleanupResult::Succeeded { .. }
    ));
}
#[tokio::test]
async fn route_returns_accepted_and_rejects_nonversioned_or_secret_fields() {
    let temp = Temp::new();
    let (p, job) = saved(&temp, "hello", crate::vocabulary::empty_vocabulary()).await;
    let service = service_with(&p, Fake::new("pause"), TIMEOUT);
    let router = router(Some(service.clone()));
    for body in [
        json!({"data":selection()}),
        json!({"api_version":"1", "data":{"provider":"openai","model":"gpt-4.1-mini","api_key":"no"}}),
    ] {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/v1/jobs/{}/cleanup", job.id))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        assert_eq!(router.clone().oneshot(request).await.unwrap().status(), 400);
    }
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/v1/jobs/{}/cleanup", job.id))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({"api_version":"1","data":selection()}).to_string(),
        ))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 202);
    let bytes = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    let pending: Envelope<Job> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result(&pending.data).transcript.raw_text, "hello");
    done(&service, job.id).await;
}
#[tokio::test]
async fn cloud_http_contracts_reject_partial_and_redact_errors() {
    for id in [ProviderId::Openai, ProviderId::Gemini, ProviderId::Grok] {
        let openai = matches!(id, ProviderId::Openai);
        let grok = matches!(id, ProviderId::Grok);
        let endpoint = if openai {
            "/responses"
        } else if grok {
            // xAI serves an OpenAI-shaped chat API, not the Responses API.
            "/chat/completions"
        } else {
            "/models/gemini-3.8-flash:generateContent"
        };
        let router = Router::new().route(endpoint, post(move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| async move {
            if openai {
                assert_eq!(headers["authorization"], "Bearer fixture-key");
                assert_eq!(body["store"], false);
                assert!(body["instructions"].as_str().unwrap().contains("Never translate"));
                Json(json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"{\"spans\":[\"Hello.\"]}"}]}]}))
            } else if grok {
                assert_eq!(headers["authorization"], "Bearer fixture-key");
                assert_eq!(body["model"], "grok-4.6");
                assert_eq!(body["response_format"]["type"], "json_object");
                assert!(body["messages"][0]["content"].as_str().unwrap().contains("Never translate"));
                assert_eq!(body["messages"][1]["content"], "{\"spans\":[\"hello\"]}");
                Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"{\"spans\":[\"Hello.\"]}"}}]}))
            } else {
                assert_eq!(headers["x-goog-api-key"], "fixture-key");
                assert_eq!(body["generationConfig"]["responseMimeType"], "application/json");
                Json(json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"{\"spans\":[\"Hello.\"]}"}]}}]}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let provider = provider::CloudText::fixture(format!("http://{address}"));
        let selected = ModelSelection {
            provider: id,
            model: if openai {
                "gpt-4.1-mini"
            } else if grok {
                "grok-4.6"
            } else {
                "gemini-3.8-flash"
            }
            .into(),
        };
        assert_eq!(
            provider.clean(&selected, &["hello".into()]).await.unwrap(),
            vec!["Hello."]
        );
        task.abort();
    }
    for (status, body) in [
        (429, "upstream private text"),
        (200, r#"{"status":"incomplete","output":[]}"#),
        (
            200,
            r#"{"status":"completed","output":[{"type":"reasoning","summary":[]}]}"#,
        ),
        (
            200,
            r#"{"status":"completed","output":[{"type":"reasoning","summary":[]},{"type":"function_call","name":"private"}]}"#,
        ),
        (
            200,
            r#"{"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"private"}]}]}"#,
        ),
    ] {
        let router = Router::new().route(
            "/responses",
            post(move || async move { (axum::http::StatusCode::from_u16(status).unwrap(), body) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let provider = provider::CloudText::fixture(format!("http://{address}"));
        let error = provider
            .clean(&selection(), &["hello".into()])
            .await
            .unwrap_err();
        assert!(!error.message.contains("private"));
        if status == 429 {
            assert!(matches!(error.code, ErrorCode::RateLimited));
        } else {
            assert!(matches!(error.code, ErrorCode::InvalidProviderResponse));
        }
        task.abort();
    }
}

#[tokio::test]
async fn processed_route_expands_snapshot_without_model_or_raw_mutation() {
    let temp = Temp::new();
    let vocabulary = Vocabulary {
        hints: vec![],
        corrections: vec![],
        snippets: vec![Snippet {
            phrase: "my email".into(),
            text: "AlEx@Example.com".into(),
        }],
    };
    let (p, job) = saved(&temp, "send to my email", vocabulary).await;
    let fake = Fake::new("panic");
    let service = service_with(&p, fake.clone(), TIMEOUT);
    let request = axum::http::Request::builder()
        .uri(format!("/api/v1/jobs/{}/processed", job.id))
        .body(axum::body::Body::empty())
        .unwrap();
    let response = router(Some(service)).oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        value,
        json!({"api_version":"1","data":{"text":"send to AlEx@Example.com"}})
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        serde_json::to_value(p.jobs.get(job.id).await.unwrap()).unwrap(),
        serde_json::to_value(job).unwrap()
    );
}

#[tokio::test]
async fn grok_mode_speed_sets_reasoning_and_records_usage() {
    use crate::cleanup::modes::diagnostics;
    for fast in [true, false] {
        let router = Router::new().route("/chat/completions", post(move |Json(body): Json<serde_json::Value>| async move {
            assert_eq!(body["reasoning_effort"], if fast { "low" } else { "medium" });
            assert_eq!(body["response_format"]["type"], "json_object");
            Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"{\"spans\":[\"Hello.\"]}"}}],"usage":{"prompt_tokens":101,"completion_tokens":22,"completion_tokens_details":{"reasoning_tokens":15}}}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let provider = provider::CloudText::fixture(endpoint);
        let selected = ModelSelection {
            provider: ProviderId::Grok,
            model: "grok-4.6".into(),
        };
        let (result, metrics) = diagnostics::measure(provider.generate_with_options(
            &selected,
            "Format spans as JSON",
            "hello",
            fast,
        ))
        .await;
        assert!(result.unwrap().contains("Hello."));
        assert_eq!(metrics.input_tokens, Some(101));
        assert_eq!(metrics.output_tokens, Some(22));
        assert_eq!(metrics.thought_tokens, Some(15));
        task.abort();
    }
}

#[tokio::test]
async fn grok_4_3_turns_reasoning_off_for_fast_only() {
    for fast in [true, false] {
        let router = Router::new().route("/chat/completions", post(move |Json(body): Json<serde_json::Value>| async move {
            assert_eq!(body["model"], "grok-4.3");
            assert_eq!(body["reasoning_effort"], if fast { "none" } else { "medium" });
            Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"{\"spans\":[\"Hello.\"]}"}}]}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let provider = provider::CloudText::fixture(endpoint);
        let selected = ModelSelection {
            provider: ProviderId::Grok,
            model: "grok-4.3".into(),
        };
        let output = provider
            .generate_with_options(&selected, "Format spans as JSON", "hello", fast)
            .await
            .unwrap();
        assert!(output.contains("Hello."));
        task.abort();
    }
}

#[tokio::test]
async fn openrouter_routes_by_latency_with_json_and_reasoning_required() {
    use crate::cleanup::modes::diagnostics;
    for fast in [true, false] {
        let router = Router::new().route("/chat/completions", post(move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| async move {
            assert_eq!(headers["authorization"], "Bearer fixture-key");
            assert_eq!(body["model"], "anthropic/claude-haiku-4.5");
            assert_eq!(body["messages"][0]["content"], "Format spans as JSON");
            assert_eq!(body["messages"][1]["content"], "hello");
            assert_eq!(body["response_format"]["type"], "json_object");
            assert_eq!(body["reasoning"]["effort"], if fast { "none" } else { "medium" });
            assert_eq!(body["provider"], json!({"sort": "latency", "require_parameters": true}));
            // Either would exclude hosts that do not accept it.
            assert!(body.get("max_tokens").is_none() && body.get("temperature").is_none());
            Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"{\"spans\":[\"Hello.\"]}"}}],"usage":{"prompt_tokens":90,"completion_tokens":12}}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let provider = provider::CloudText::fixture(endpoint);
        let selected = ModelSelection {
            provider: ProviderId::Openrouter,
            model: "anthropic/claude-haiku-4.5".into(),
        };
        let (result, metrics) = diagnostics::measure(provider.generate_with_options(
            &selected,
            "Format spans as JSON",
            "hello",
            fast,
        ))
        .await;
        assert!(result.unwrap().contains("Hello."));
        assert_eq!(metrics.input_tokens, Some(90));
        assert_eq!(metrics.output_tokens, Some(12));
        task.abort();
    }
    // Claude through OpenRouter fences its JSON despite json_object mode.
    let router = Router::new().route(
        "/chat/completions",
        post(|| async { Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"```json\n{\n  \"spans\": [\"Hello.\"]\n}\n```"}}]})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let provider = provider::CloudText::fixture(endpoint);
    let haiku = ModelSelection {
        provider: ProviderId::Openrouter,
        model: "anthropic/claude-haiku-4.5".into(),
    };
    assert_eq!(
        provider.clean(&haiku, &["hello".into()]).await.unwrap(),
        vec!["Hello."]
    );
    task.abort();
    // A fence is not a licence for prose around it.
    let router = Router::new().route(
        "/chat/completions",
        post(|| async { Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"Here you go:\n```json\n{\"spans\": [\"Hello.\"]}\n```"}}]})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let provider = provider::CloudText::fixture(endpoint);
    assert!(provider.clean(&haiku, &["hello".into()]).await.is_err());
    task.abort();
    // A routed error inside a 200 body, or a slug Writer does not offer, is never text.
    let router = Router::new().route(
        "/chat/completions",
        post(|| async { Json(json!({"error":{"code":502,"message":"private upstream"}})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let provider = provider::CloudText::fixture(endpoint);
    let routed = |model: &str| ModelSelection {
        provider: ProviderId::Openrouter,
        model: model.into(),
    };
    let error = provider
        .clean(&routed("openai/gpt-5.6-luna"), &["hello".into()])
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::ProviderFailed));
    assert!(!error.message.contains("private"));
    let error = provider
        .clean(&routed("some/unlisted-model"), &["hello".into()])
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::UnsupportedCapability));
    task.abort();
}

#[tokio::test]
async fn a_warmed_connection_is_reused_by_the_request_at_release() {
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;
    let seen: Arc<std::sync::Mutex<Vec<(String, SocketAddr, String)>>> = Default::default();
    let (key_seen, chat_seen) = (seen.clone(), seen.clone());
    let router = Router::new()
        .route(
            "/key",
            axum::routing::get(
                move |ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: axum::http::HeaderMap| async move {
                    let auth = headers["authorization"].to_str().unwrap().to_owned();
                    key_seen.lock().unwrap().push(("key".into(), peer, auth));
                    Json(json!({"data": {"label": "fixture"}}))
                },
            ),
        )
        .route(
            "/chat/completions",
            post(move |ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                chat_seen.lock().unwrap().push(("chat".into(), peer, String::new()));
                Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"{\"spans\":[\"Hello.\"]}"}}]}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let provider = provider::CloudText::fixture(endpoint);
    let mistral = ModelSelection {
        provider: ProviderId::Openrouter,
        model: "mistralai/mistral-medium-3-5".into(),
    };
    provider.warm(&mistral);
    for _ in 0..200 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    // Give the warm-up's connection time to return to the pool after its body.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        provider.clean(&mistral, &["hello".into()]).await.unwrap(),
        vec!["Hello."]
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].0, "key");
    assert_eq!(seen[0].2, "Bearer fixture-key");
    assert_eq!(seen[1].0, "chat");
    assert_eq!(
        seen[0].1, seen[1].1,
        "the release request must use the warmed connection"
    );
    // Local models and unlisted slugs have nothing to warm.
    provider.warm(&ModelSelection {
        provider: ProviderId::Openrouter,
        model: "some/unlisted-model".into(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    task.abort();
}

#[tokio::test]
async fn luna_uses_explicit_reasoning_and_parses_reasoning_items_without_exposing_them() {
    use crate::cleanup::modes::diagnostics;
    for model in ["gpt-4.1-mini", "gpt-5.6-luna"] {
        let selection = ModelSelection {
            provider: ProviderId::Openai,
            model: model.into(),
        };
        assert!(provider::validate(&selection).is_ok());
        assert!(provider::discovery()
            .iter()
            .any(|c| matches!(c.selection.provider, ProviderId::Openai)
                && c.selection.model == model));
        for fast in [true, false] {
            let router = Router::new().route("/responses", post(move |Json(body): Json<serde_json::Value>| async move {
                assert_eq!(body["model"], model);
                assert_eq!(body["input"][0]["role"], "developer");
                assert!(body["input"][0]["content"].as_str().unwrap().contains("JSON"));
                assert_eq!(body["input"][1], json!({"role":"user","content":"hello"}));
                assert_eq!(body["store"], false);
                assert_eq!(body["text"]["format"]["type"], "json_object");
                if model == "gpt-5.6-luna" {
                    assert_eq!(body["reasoning"]["effort"], if fast { "none" } else { "medium" });
                } else {
                    assert!(body.get("reasoning").is_none(), "legacy model must not receive unsupported reasoning");
                }
                Json(json!({"status":"completed","error":null,"output":[
                    {"type":"reasoning","summary":[{"type":"summary_text","text":"private reasoning"}]},
                    {"type":"message","content":[{"type":"output_text","text":"{\"spans\":[\"Hello.\"]}"}]}
                ],"usage":{"input_tokens":101,"output_tokens":23,"output_tokens_details":{"reasoning_tokens":12}}}))
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let provider =
                provider::CloudText::fixture(format!("http://{}", listener.local_addr().unwrap()));
            let task = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let (result, metrics) = diagnostics::measure(provider.generate_with_options(
                &selection,
                "Return JSON.",
                "hello",
                fast,
            ))
            .await;
            assert_eq!(result.unwrap(), r#"{"spans":["Hello."]}"#);
            assert_eq!(metrics.thought_tokens, Some(12));
            assert_eq!(metrics.input_tokens, Some(101));
            assert_eq!(metrics.output_tokens, Some(23));
            task.abort();
        }
    }
    // Spark is not generally API-accessible; never advertise an untested alias.
    assert!(provider::validate(&ModelSelection {
        provider: ProviderId::Openai,
        model: "gpt-5.3-codex-spark".into()
    })
    .is_err());
}

fn custom(provider: ProviderId, model: &str, fast: &str, balanced: &str) -> serde_json::Value {
    json!({"provider": provider, "model": model, "fast": fast, "balanced": balanced, "only_hosts": [], "ignore_hosts": []})
}
async fn put_custom(router: &Router, data: serde_json::Value) -> (u16, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/v1/modes/custom-models")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({"api_version": "1", "data": data}).to_string(),
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
async fn custom_models_for_every_cloud_provider_are_checked_saved_and_listed() {
    let temp = Temp::new();
    let path = temp.0.join("state/text-models.json");
    let store = Arc::new(custom::CustomModels::open(Some(path.clone())));
    let router = custom::router(store.clone());
    let models = json!([
        custom(ProviderId::Openai, "gpt-5.6-terra", "off", "medium"),
        custom(
            ProviderId::Gemini,
            "gemini-3.6-flash",
            "off",
            "provider_default"
        ),
        custom(
            ProviderId::Grok,
            "grok-4",
            "provider_default",
            "provider_default"
        ),
        custom(
            ProviderId::Openrouter,
            "meta-llama/llama-4-maverick:free",
            "provider_default",
            "high"
        ),
    ]);
    let (status, saved) = put_custom(&router, models.clone()).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["data"], models);

    let request = axum::http::Request::builder()
        .uri("/api/v1/modes/custom-models")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(listed["data"], models);
    // The list survives a restart.
    let reopened = custom::CustomModels::open(Some(path));
    assert_eq!(serde_json::to_value(reopened.list()).unwrap(), models);

    // Every model a mode can pick, listed first, then the user's with their speeds.
    let choices = serde_json::to_value(provider::discovery_with(&store)).unwrap();
    let choices = choices.as_array().unwrap();
    let luna = choices
        .iter()
        .find(|c| c["selection"]["model"] == "gpt-5.6-luna")
        .unwrap();
    assert!(luna.get("custom").is_none());
    let terra = choices
        .iter()
        .find(|c| c["selection"]["model"] == "gpt-5.6-terra")
        .unwrap();
    assert_eq!(terra["selection"]["provider"], "openai");
    assert_eq!(terra["execution_location"], "cloud");
    assert_eq!(
        terra["custom"],
        json!({"fast": "off", "balanced": "medium"})
    );
    assert_eq!(choices.len(), 10 + 4);

    for (rejected, reason) in [
        (
            json!([custom(
                ProviderId::Local,
                "qwen3-4b-instruct-2507-q4",
                "off",
                "off"
            )]),
            "local",
        ),
        (
            json!([custom(
                ProviderId::Openrouter,
                "llama-4-maverick",
                "off",
                "off"
            )]),
            "publisher/model",
        ),
        (
            json!([custom(
                ProviderId::Openai,
                "openai/gpt-5.6-terra",
                "off",
                "off"
            )]),
            "slash",
        ),
        (
            json!([custom(ProviderId::Gemini, "..", "off", "off")]),
            "dots",
        ),
        (
            json!([custom(ProviderId::Gemini, "gemini flash", "off", "off")]),
            "space",
        ),
        (
            json!([custom(ProviderId::Grok, " grok-4", "off", "off")]),
            "untrimmed",
        ),
        (
            json!([custom(ProviderId::Openai, "gpt-5.6-luna", "off", "off")]),
            "listed",
        ),
        (
            json!([
                custom(ProviderId::Openai, "gpt-5.6-terra", "off", "off"),
                custom(ProviderId::Openai, "gpt-5.6-terra", "low", "low")
            ]),
            "duplicate",
        ),
        (
            json!([custom(ProviderId::Openai, &"a".repeat(129), "off", "off")]),
            "long",
        ),
        (
            json!((0..51)
                .map(|i| custom(ProviderId::Openai, &format!("m{i}"), "off", "off"))
                .collect::<Vec<_>>()),
            "count",
        ),
        (
            json!([custom(
                ProviderId::Openai,
                "gpt-5.6-terra",
                "extreme",
                "off"
            )]),
            "unknown reasoning",
        ),
        (
            json!([{"provider": "openai", "model": "gpt-5.6-terra", "fast": "off", "balanced": "off", "api_key": "no"}]),
            "extra field",
        ),
    ] {
        let (status, _) = put_custom(&router, rejected).await;
        assert_eq!(status, 400, "{reason}");
    }
    // A refused save leaves the saved list alone.
    assert_eq!(serde_json::to_value(store.list()).unwrap(), models);
    // Removing is saving the shorter list.
    let (status, _) = put_custom(&router, json!([])).await;
    assert_eq!(status, 200);
    assert!(store.list().is_empty());
}

#[test]
fn a_removed_custom_model_stays_assigned_but_an_unusable_one_does_not() {
    let removed = ModelSelection {
        provider: ProviderId::Openai,
        model: "gpt-5.6-terra".into(),
    };
    assert!(provider::validate(&removed).is_err());
    assert!(provider::assignable(&removed).is_ok());
    for unusable in [
        ModelSelection {
            provider: ProviderId::Local,
            model: "some-download".into(),
        },
        ModelSelection {
            provider: ProviderId::Gemini,
            model: "../key".into(),
        },
    ] {
        assert!(provider::assignable(&unusable).is_err());
    }
}

#[tokio::test]
async fn custom_models_send_exactly_the_reasoning_chosen_for_each_speed() {
    let seen = Arc::new(std::sync::Mutex::new(
        Vec::<(String, serde_json::Value)>::new(),
    ));
    let recorded = seen.clone();
    let router = Router::new().fallback(move |uri: axum::http::Uri, body: axum::body::Bytes| {
        let recorded = recorded.clone();
        async move {
            let path = uri.path().to_owned();
            recorded.lock().unwrap().push((
                path.clone(),
                serde_json::from_slice(&body).unwrap_or_default(),
            ));
            let spans = "{\"spans\":[\"Hello.\"]}";
            if path.ends_with("/responses") {
                Json(json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":spans}]}]}))
            } else if path.ends_with("/chat/completions") {
                Json(json!({"choices":[{"finish_reason":"stop","message":{"content":spans}}]}))
            } else if path.ends_with(":generateContent") {
                Json(json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":spans}]}}]}))
            } else {
                Json(json!({"id": "fixture"}))
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let temp = Temp::new();
    let store = Arc::new(custom::CustomModels::open(Some(
        temp.0.join("text-models.json"),
    )));
    let added: Vec<custom::CustomModel> = serde_json::from_value(json!([
        custom(
            ProviderId::Openai,
            "gpt-5.6-terra",
            "off",
            "provider_default"
        ),
        custom(ProviderId::Grok, "grok-4", "minimal", "provider_default"),
        custom(
            ProviderId::Openrouter,
            "meta-llama/llama-4-maverick",
            "provider_default",
            "high"
        ),
        custom(
            ProviderId::Gemini,
            "gemini-3.6-flash",
            "off",
            "provider_default"
        ),
        custom(ProviderId::Gemini, "gemini-3-pro", "low", "high"),
    ]))
    .unwrap();
    store.save(added).await.unwrap();
    let provider = provider::CloudText::fixture(endpoint).with_custom(store);
    let pick = |provider: ProviderId, model: &str| ModelSelection {
        provider,
        model: model.into(),
    };
    let body = |selection: ModelSelection, fast: bool| {
        let provider = &provider;
        let seen = seen.clone();
        async move {
            seen.lock().unwrap().clear();
            let output = provider
                .generate_with_options(&selection, "Return JSON.", "hello", fast)
                .await
                .unwrap();
            assert_eq!(output, "{\"spans\":[\"Hello.\"]}");
            let seen = seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 1);
            seen[0].1.clone()
        }
    };
    let terra = pick(ProviderId::Openai, "gpt-5.6-terra");
    assert_eq!(
        body(terra.clone(), true).await["reasoning"],
        json!({"effort": "none"})
    );
    assert!(body(terra, false).await.get("reasoning").is_none());

    let grok = pick(ProviderId::Grok, "grok-4");
    assert_eq!(
        body(grok.clone(), true).await["reasoning_effort"],
        "minimal"
    );
    assert!(body(grok, false).await.get("reasoning_effort").is_none());

    let maverick = pick(ProviderId::Openrouter, "meta-llama/llama-4-maverick");
    let fast = body(maverick.clone(), true).await;
    assert!(fast.get("reasoning").is_none());
    // JSON mode is still required of the host that serves it.
    assert_eq!(
        fast["provider"],
        json!({"sort": "latency", "require_parameters": true})
    );
    assert_eq!(
        body(maverick, false).await["reasoning"],
        json!({"effort": "high"})
    );

    let flash = pick(ProviderId::Gemini, "gemini-3.6-flash");
    assert_eq!(
        body(flash.clone(), true).await["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget": 0})
    );
    let balanced = body(flash, false).await;
    assert!(balanced["generationConfig"].get("thinkingConfig").is_none());
    assert_eq!(
        balanced["generationConfig"]["responseMimeType"],
        "application/json"
    );
    let pro = pick(ProviderId::Gemini, "gemini-3-pro");
    assert_eq!(
        body(pro.clone(), true).await["generationConfig"]["thinkingConfig"],
        json!({"thinkingLevel": "LOW"})
    );
    assert_eq!(
        body(pro, false).await["generationConfig"]["thinkingConfig"],
        json!({"thinkingLevel": "HIGH"})
    );

    // A custom model is warmed like a listed one.
    seen.lock().unwrap().clear();
    provider.warm(&pick(ProviderId::Openai, "gpt-5.6-terra"));
    for _ in 0..200 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(seen.lock().unwrap()[0].0, "/models/gpt-5.6-terra");

    // A model that is not added is never sent.
    seen.lock().unwrap().clear();
    let error = provider
        .generate_with_options(
            &pick(ProviderId::Openai, "gpt-5.6-sol"),
            "Return JSON.",
            "hello",
            true,
        )
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::UnsupportedCapability));
    assert!(seen.lock().unwrap().is_empty());
    task.abort();
}

#[tokio::test]
async fn a_rejected_custom_model_says_what_to_change_without_repeating_the_provider() {
    let router = Router::new().route(
        "/responses",
        post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": "Unsupported value: 'none' for private request text"}})),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let temp = Temp::new();
    let store = Arc::new(custom::CustomModels::open(Some(
        temp.0.join("text-models.json"),
    )));
    store
        .save(
            serde_json::from_value(json!([custom(
                ProviderId::Openai,
                "gpt-5.6-terra",
                "off",
                "off"
            )]))
            .unwrap(),
        )
        .await
        .unwrap();
    let provider = provider::CloudText::fixture(endpoint).with_custom(store);
    let terra = ModelSelection {
        provider: ProviderId::Openai,
        model: "gpt-5.6-terra".into(),
    };
    let error = provider.clean(&terra, &["hello".into()]).await.unwrap_err();
    assert!(matches!(error.code, ErrorCode::UnsupportedCapability));
    assert!(error
        .message
        .contains("reasoning to Provider default in Modes → Manage custom models"));
    assert!(!error.message.contains("private"));
    // A listed model keeps its own message.
    let error = provider
        .clean(&selection(), &["hello".into()])
        .await
        .unwrap_err();
    assert!(!error.message.contains("custom"));
    task.abort();
}

#[tokio::test]
async fn pinned_hosts_reach_the_request_and_are_offered_from_openrouter_s_own_list() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let sent = seen.clone();
    let listed = Arc::new(AtomicUsize::new(0));
    let counted = listed.clone();
    let router = Router::new()
        .route(
            "/chat/completions",
            post(move |Json(body): Json<serde_json::Value>| {
                let sent = sent.clone();
                async move {
                    sent.lock().unwrap().push(body["provider"].clone());
                    Json(json!({"provider": "DeepInfra", "choices": [{"finish_reason": "stop", "message": {"content": "{\"spans\":[\"Hello.\"]}"}}]}))
                }
            }),
        )
        .route(
            "/models/deepseek/deepseek-v4.1-flash/endpoints",
            get(move || {
                let counted = counted.clone();
                async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"data": {"endpoints": [
                        {"tag": "deepinfra/fp8", "provider_name": "DeepInfra", "uptime_last_30m": 99.9, "quantization": "fp8"},
                        {"tag": "together", "provider_name": "Together", "uptime_last_30m": 99.0, "quantization": "unknown"},
                        {"tag": "bad host", "provider_name": "Rejected", "uptime_last_30m": 50.0, "quantization": "fp4"}
                    ]}}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let temp = Temp::new();
    let store = Arc::new(
        custom::CustomModels::open(Some(temp.0.join("text-models.json")))
            .with_endpoint(endpoint.clone()),
    );

    // The host list comes from OpenRouter and is cached, and an unusable tag is dropped.
    let hosts = store.hosts("deepseek/deepseek-v4.1-flash").await.unwrap();
    assert_eq!(
        serde_json::to_value(&hosts).unwrap(),
        json!([
            {"tag": "deepinfra/fp8", "name": "DeepInfra", "uptime": 99.9, "quantization": "fp8"},
            {"tag": "together", "name": "Together", "uptime": 99.0, "quantization": null}
        ])
    );
    store.hosts("deepseek/deepseek-v4.1-flash").await.unwrap();
    assert_eq!(listed.load(Ordering::SeqCst), 1);
    assert!(store.hosts("not-a-routed-slug").await.is_err());

    let router = custom::router(store.clone());
    // Host discovery uses a JSON body, preserving the self-hosted ban on API queries.
    let host_response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/modes/custom-models/hosts")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({"api_version":"1", "data":{"model":"deepseek/deepseek-v4.1-flash"}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(host_response.status(), 200);
    let host_body = axum::body::to_bytes(host_response.into_body(), 16384)
        .await
        .unwrap();
    let host_body: serde_json::Value = serde_json::from_slice(&host_body).unwrap();
    assert_eq!(host_body["data"], serde_json::to_value(&hosts).unwrap());
    let (status, body) = put_custom(
        &router,
        json!([{"provider": "openrouter", "model": "deepseek/deepseek-v4.1-flash", "fast": "off", "balanced": "provider_default", "only_hosts": ["deepinfra/fp8", "together"], "ignore_hosts": []}]),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let provider = provider::CloudText::fixture(endpoint).with_custom(store.clone());
    let selection = ModelSelection {
        provider: ProviderId::Openrouter,
        model: "deepseek/deepseek-v4.1-flash".into(),
    };
    let (result, metrics) =
        crate::cleanup::modes::diagnostics::measure(provider.clean(&selection, &["hello".into()]))
            .await;
    assert_eq!(result.unwrap(), vec!["Hello."]);
    // The host that answered is recorded, so a slow one can be recognised later.
    assert_eq!(metrics.served_by.as_deref(), Some("DeepInfra"));
    assert_eq!(
        seen.lock().unwrap()[0],
        json!({"sort": "latency", "require_parameters": true, "only": ["deepinfra/fp8", "together"]})
    );

    let (status, _) = put_custom(
        &router,
        json!([{"provider": "openrouter", "model": "deepseek/deepseek-v4.1-flash", "fast": "off", "balanced": "provider_default", "only_hosts": [], "ignore_hosts": ["relace/fp4"]}]),
    )
    .await;
    assert_eq!(status, 200);
    provider.clean(&selection, &["hello".into()]).await.unwrap();
    assert_eq!(
        seen.lock().unwrap()[1],
        json!({"sort": "latency", "require_parameters": true, "ignore": ["relace/fp4"]})
    );

    for (rejected, reason) in [
        (
            json!([{"provider": "openrouter", "model": "deepseek/deepseek-v4.1-flash", "fast": "off", "balanced": "off", "only_hosts": ["a"], "ignore_hosts": ["b"]}]),
            "both lists",
        ),
        (
            json!([{"provider": "openai", "model": "gpt-5.6-terra", "fast": "off", "balanced": "off", "only_hosts": ["azure"], "ignore_hosts": []}]),
            "one host only",
        ),
        (
            json!([{"provider": "openrouter", "model": "deepseek/deepseek-v4.1-flash", "fast": "off", "balanced": "off", "only_hosts": ["../etc"], "ignore_hosts": []}]),
            "unusable tag",
        ),
    ] {
        let (status, _) = put_custom(&router, rejected).await;
        assert_eq!(status, 400, "{reason}");
    }
    task.abort();
}

#[tokio::test]
async fn text_rate_limits_honor_retry_after_and_suppress_repeat_calls_per_model() {
    use axum::{routing::post, Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let router = Router::new().route("/responses", post(move || {
        let seen = seen.clone();
        async move { seen.fetch_add(1, Ordering::SeqCst); (axum::http::StatusCode::TOO_MANY_REQUESTS, [("retry-after", "60")], Json(json!({"error":{"message":"private upstream payload"}}))) }
    })).route("/chat/completions", post(|| async { Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"{\"spans\":[\"Hello.\"]}"}}]})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let provider = provider::CloudText::fixture(endpoint);
    let primary = ModelSelection {
        provider: ProviderId::Openai,
        model: "gpt-4.1-mini".into(),
    };
    for _ in 0..2 {
        let error = provider
            .clean(&primary, &["hello".into()])
            .await
            .err()
            .unwrap();
        assert!(matches!(error.code, ErrorCode::RateLimited));
        assert!(!error.message.contains("private upstream"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(provider.cooldown_remaining(&primary).unwrap() >= std::time::Duration::from_secs(58));
    let backup = ModelSelection {
        provider: ProviderId::Openrouter,
        model: "openai/gpt-5.6-luna".into(),
    };
    assert!(provider.clean(&backup, &["hello".into()]).await.is_ok());
    assert!(provider.cooldown_remaining(&backup).is_none());
    task.abort();
}

#[tokio::test]
async fn routed_http_200_errors_are_classified_before_any_text_is_accepted() {
    use axum::{routing::post, Json, Router};
    let router = Router::new().route(
        "/chat/completions",
        post(|| async { Json(json!({"error":{"code":429,"message":"private routed failure"}})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let provider = provider::CloudText::fixture(endpoint);
    let selection = ModelSelection {
        provider: ProviderId::Openrouter,
        model: "openai/gpt-5.6-luna".into(),
    };
    let error = provider
        .clean(&selection, &["hello".into()])
        .await
        .err()
        .unwrap();
    assert!(matches!(error.code, ErrorCode::RateLimited));
    assert!(!error.message.contains("private routed"));
    assert!(provider.cooldown_remaining(&selection).is_some());
    task.abort();
}

#[tokio::test]
async fn strict_cleanup_rejects_numeric_separator_drift_with_original_and_audio_retained() {
    struct Output(String);
    #[async_trait::async_trait]
    impl TextProvider for Output {
        async fn clean(&self, _: &ModelSelection, _: &[String]) -> Result<Vec<String>, ApiError> {
            Ok(vec![self.0.clone()])
        }
    }
    for output in [
        "Do not pay 5.25 at 09:30",
        "Do not pay -5,25 at 09:30",
        "Do not pay -5.25 at 09.30",
        "Do pay -5.25 at 09:30",
    ] {
        let root = Temp::new();
        let raw = "do not pay -5.25 at 09:30";
        let (pipeline, job) = saved(&root, raw, crate::vocabulary::empty_vocabulary()).await;
        let service = service_with(&pipeline, Arc::new(Output(output.into())), TIMEOUT);
        service.start(job.id, selection()).await.unwrap();
        let finished = done(&service, job.id).await;
        assert!(
            matches!(&result(&finished).cleanup, CleanupResult::Failed { error } if matches!(error.code, ErrorCode::InvalidProviderResponse))
        );
        assert_eq!(result(&finished).transcript.raw_text, raw);
        let audio = pipeline
            .recordings
            .audio(job.request.recording_id)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(audio.path).await.unwrap(), vec![42; 16]);
    }
}
