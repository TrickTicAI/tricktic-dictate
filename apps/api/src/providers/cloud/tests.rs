use super::*;
use crate::transcription::{
    CleanupPolicy, ModelSelection, RecordingMetadata, TextLanguage, Vocabulary,
};
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, Uri},
    routing::post,
    Router,
};
use std::sync::Mutex;
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

pub(super) struct AudioFixture(pub(super) SavedAudio);
impl AudioFixture {
    pub(super) fn new() -> Self {
        // Synthetic container-shaped bytes exercise transport, not speech recognition.
        let bytes = b"RIFF\x10\x00\x00\x00WAVEfixture-only";
        let id = Uuid::new_v4();
        let path = std::env::temp_dir().join(format!("writer-cloud-{id}.wav"));
        std::fs::write(&path, bytes).unwrap();
        Self(SavedAudio {
            prepared: None,
            path,
            metadata: RecordingMetadata {
                id,
                created_at_ms: 1,
                mime_type: "audio/wav".into(),
                byte_length: bytes.len() as u64,
                duration_ms: 1000,
                input_language: InputLanguage::Mixed,
                capture_state: CaptureState::Complete,
            },
        })
    }
}
impl Drop for AudioFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0.path);
        let _ = std::fs::remove_file(self.0.path.with_extension("gemini-upload.json"));
    }
}

#[derive(Clone)]
struct Captured {
    headers: HeaderMap,
    uri: String,
    body: Vec<u8>,
}
struct HttpFixture {
    endpoint: String,
    requests: Arc<Mutex<Vec<Captured>>>,
    server: JoinHandle<()>,
}
impl HttpFixture {
    async fn new(status: u16, body: String, delay: Duration) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let app = Router::new()
            .layer(axum::extract::DefaultBodyLimit::disable())
            .fallback(post(move |headers: HeaderMap, uri: Uri, bytes: Bytes| {
                let captured = captured.clone();
                let body = body.clone();
                async move {
                    captured.lock().unwrap().push(Captured {
                        headers,
                        uri: uri.to_string(),
                        body: bytes.to_vec(),
                    });
                    tokio::time::sleep(delay).await;
                    (StatusCode::from_u16(status).unwrap(), body)
                }
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.layer(axum::extract::DefaultBodyLimit::max(510_000_000)),
            )
            .await
            .unwrap();
        });
        Self {
            endpoint,
            requests,
            server,
        }
    }
    fn provider(&self, id: ProviderId) -> CloudProvider {
        CloudProvider::new(
            id,
            Some("fixture-key-never-real".into()),
            self.endpoint.clone(),
            Duration::from_secs(5),
        )
    }
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub(super) fn options(id: ProviderId, language: InputLanguage) -> TranscriptionOptions {
    let model = match id {
        ProviderId::Openai => "gpt-transcribe",
        ProviderId::Grok => "grok-voice-transcribe-2.0",
        _ => "gemini-3.8-flash",
    };
    TranscriptionOptions {
        selection: ModelSelection {
            provider: id,
            model: model.into(),
        },
        input_language: language,
        output_language: OutputLanguage::Preserve {},
        vocabulary: Vocabulary {
            hints: vec!["Metrix".into()],
            corrections: vec![],
            snippets: vec![],
        },
        cleanup: CleanupPolicy::Off {},
    }
}

pub(super) fn success(id: &ProviderId) -> String {
    if matches!(id, ProviderId::Openai | ProviderId::Grok) {
        json!({"text":" שלום Metrix\n"}).to_string()
    } else {
        json!({"candidates":[{"finishReason":"STOP","content":{"parts":[
            {"thought":true,"text":"private reasoning"}, {"text":" שלום "}, {"text":"Metrix\n"}
        ]}}]})
        .to_string()
    }
}

#[tokio::test]
async fn openai_http_preserves_each_language_and_exact_transcript() {
    let audio = AudioFixture::new();
    for language in [InputLanguage::En, InputLanguage::He, InputLanguage::Mixed] {
        let http = HttpFixture::new(200, success(&ProviderId::Openai), Duration::ZERO).await;
        let adapter = http.provider(ProviderId::Openai);
        assert!(adapter.info().configured);
        assert!(!adapter.info().available);
        let settings = options(ProviderId::Openai, language);
        let result = adapter.transcribe(&audio.0, &settings).await.unwrap();
        assert_eq!(result.raw_text, " שלום Metrix\n");
        assert_eq!(result.selection.model, "gpt-transcribe");
        assert!(matches!(
            result.execution_location,
            ExecutionLocation::Cloud
        ));
        assert!(adapter.info().available);
        let requests = http.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.uri, "/audio/transcriptions");
        assert_eq!(
            request.headers["authorization"],
            "Bearer fixture-key-never-real"
        );
        let body = String::from_utf8_lossy(&request.body);
        assert!(body.contains("filename=\"recording.wav\""));
        assert!(body.contains("Content-Type: audio/wav"));
        assert!(body.contains("name=\"keywords[]\"\r\n\r\nMetrix"));
        assert!(!body.contains("name=\"language\""));
        assert_eq!(
            body.matches("name=\"languages[]\"").count(),
            languages(&settings.input_language).len()
        );
        for language in languages(&settings.input_language) {
            assert!(body.contains(&format!("name=\"languages[]\"\r\n\r\n{language}")));
        }
        assert!(body.contains("Do not translate"));
        assert!(!body.contains("fixture-key-never-real"));
        assert!(audio.0.path.exists());
    }
}

#[tokio::test]
async fn legacy_openai_uses_singular_language_and_mixed_detection() {
    let audio = AudioFixture::new();
    for language in [InputLanguage::En, InputLanguage::He, InputLanguage::Mixed] {
        let http = HttpFixture::new(200, success(&ProviderId::Openai), Duration::ZERO).await;
        let mut settings = options(ProviderId::Openai, language);
        settings.selection.model = "gpt-4o-mini-transcribe".into();
        http.provider(ProviderId::Openai)
            .transcribe(&audio.0, &settings)
            .await
            .unwrap();
        let requests = http.requests.lock().unwrap();
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(!body.contains("name=\"languages[]\""));
        if matches!(settings.input_language, InputLanguage::Mixed) {
            assert!(!body.contains("name=\"language\""));
            assert!(body.contains("mixes Hebrew and English"));
        } else {
            assert!(body.contains(&format!(
                "name=\"language\"\r\n\r\n{}",
                languages(&settings.input_language)[0]
            )));
        }
    }
}

#[tokio::test]
async fn gemini_http_transcribes_audio_with_original_language_instructions() {
    let audio = AudioFixture::new();
    for language in [InputLanguage::En, InputLanguage::He, InputLanguage::Mixed] {
        let http = HttpFixture::new(200, success(&ProviderId::Gemini), Duration::ZERO).await;
        let adapter = http.provider(ProviderId::Gemini);
        let settings = options(ProviderId::Gemini, language);
        let transcript = adapter.transcribe(&audio.0, &settings).await.unwrap();
        assert_eq!(transcript.raw_text, " שלום Metrix\n");
        assert!(adapter.info().available);
        let requests = http.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.uri, "/models/gemini-3.8-flash:generateContent");
        assert_eq!(request.headers["x-goog-api-key"], "fixture-key-never-real");
        assert!(!request.uri.contains("key"));
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            value["systemInstruction"]["parts"][0]["text"],
            preservation_prompt(&settings)
        );
        assert_eq!(
            value["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "audio/wav"
        );
        let data = value["contents"][0]["parts"][1]["inlineData"]["data"]
            .as_str()
            .unwrap();
        assert_eq!(
            STANDARD.decode(data).unwrap(),
            std::fs::read(&audio.0.path).unwrap()
        );
    }
}

#[tokio::test]
async fn errors_are_structured_sanitized_and_do_not_modify_audio() {
    let audio = AudioFixture::new();
    let original = std::fs::read(&audio.0.path).unwrap();
    for id in [ProviderId::Openai, ProviderId::Gemini, ProviderId::Grok] {
        for (status, code, retryable) in [
            (401, "authentication_failed", false),
            (403, "authentication_failed", false),
            (408, "provider_timeout", true),
            (429, "rate_limited", true),
            (500, "provider_failed", true),
            (400, "invalid_request", false),
            (413, "invalid_request", false),
            (404, "unsupported_capability", false),
            (200, "invalid_provider_response", true),
        ] {
            let http = HttpFixture::new(
                status,
                "fixture-key-never-real SECRET-AUDIO".into(),
                Duration::ZERO,
            )
            .await;
            let adapter = http.provider(id.clone());
            adapter.mark_verified();
            let failure = adapter
                .transcribe(&audio.0, &options(id.clone(), InputLanguage::En))
                .await
                .unwrap_err();
            assert_eq!(serde_json::to_value(&failure.code).unwrap(), code);
            assert_eq!(failure.retryable, retryable);
            let rendered = serde_json::to_string(&failure).unwrap();
            assert!(!rendered.contains("fixture-key") && !rendered.contains("SECRET-AUDIO"));
            if status == 401 || status == 200 {
                assert!(!adapter.info().available);
            }
            assert_eq!(std::fs::read(&audio.0.path).unwrap(), original);
        }
    }
}

#[tokio::test]
async fn slow_provider_times_out_and_clears_verified_availability() {
    let audio = AudioFixture::new();
    for id in [ProviderId::Openai, ProviderId::Gemini] {
        let http = HttpFixture::new(200, success(&id), Duration::from_secs(1)).await;
        let mut adapter = http.provider(id.clone());
        adapter.timeout = Duration::from_millis(50);
        adapter.mark_verified();
        let failure = adapter
            .transcribe(&audio.0, &options(id, InputLanguage::En))
            .await
            .unwrap_err();
        assert!(matches!(failure.code, ErrorCode::ProviderTimeout));
        assert!(failure.retryable);
        assert!(!adapter.info().available);
        assert!(audio.0.path.exists());
    }
}

#[tokio::test]
async fn rejects_missing_keys_models_translation_mime_and_size_before_http() {
    let mut audio = AudioFixture::new();
    let http = HttpFixture::new(200, success(&ProviderId::Openai), Duration::ZERO).await;
    let mut settings = options(ProviderId::Openai, InputLanguage::En);
    for key in [None, Some(" ".into()), Some("bad\ncredential".into())] {
        let adapter = CloudProvider::new(
            ProviderId::Openai,
            key,
            http.endpoint.clone(),
            Duration::from_secs(1),
        );
        let failure = adapter.transcribe(&audio.0, &settings).await.unwrap_err();
        assert!(matches!(failure.code, ErrorCode::ProviderUnavailable));
        assert!(!adapter.info().available);
    }
    let adapter = http.provider(ProviderId::Openai);
    settings.selection.model = "gpt-realtime".into();
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedCapability
    ));
    settings.selection.model = "gpt-transcribe".into();
    settings.output_language = OutputLanguage::Translate {
        language: TextLanguage::En,
    };
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedCapability
    ));
    settings.output_language = OutputLanguage::Preserve {};
    audio.0.metadata.mime_type = "text/plain".into();
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedCapability
    ));
    audio.0.metadata.mime_type = "audio/wav".into();
    audio.0.metadata.byte_length = OPENAI_FILE_LIMIT + 1;
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedCapability
    ));
    audio.0.metadata.byte_length = 1;
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::AudioNotSaved
    ));
    settings.vocabulary.hints = vec!["invalid\nterm".into()];
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    ));
    assert!(http.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_mislabeled_container_and_gemini_json_overflow_before_http() {
    let mut audio = AudioFixture::new();
    let http = HttpFixture::new(200, success(&ProviderId::Gemini), Duration::ZERO).await;
    let adapter = http.provider(ProviderId::Gemini);
    let settings = options(ProviderId::Gemini, InputLanguage::Mixed);
    audio.0.metadata.mime_type = "audio/webm;codecs=opus".into();
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    ));
    audio.0.metadata.mime_type = "audio/wav".into();
    let length = GEMINI_FILE_LIMIT + 1;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&audio.0.path)
        .unwrap()
        .set_len(length)
        .unwrap();
    audio.0.metadata.byte_length = length;
    assert!(matches!(
        adapter
            .transcribe(&audio.0, &settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedCapability
    ));
    assert!(http.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn gemini_rejects_truncation_blocking_missing_text_and_thought_only() {
    let audio = AudioFixture::new();
    for (body, code) in [
        (
            json!({"candidates":[{"finishReason":"MAX_TOKENS","content":{"parts":[{"text":"partial"}]}}]}),
            "unsupported_capability",
        ),
        (
            json!({"promptFeedback":{"blockReason":"SAFETY"}}),
            "provider_failed",
        ),
        (
            json!({"candidates":[{"finishReason":"SAFETY"}]}),
            "provider_failed",
        ),
        (
            json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"thought":true,"text":"reasoning"}]}}]}),
            "invalid_provider_response",
        ),
        (json!({"candidates":[]}), "invalid_provider_response"),
    ] {
        let http = HttpFixture::new(200, body.to_string(), Duration::ZERO).await;
        let error = http
            .provider(ProviderId::Gemini)
            .transcribe(&audio.0, &options(ProviderId::Gemini, InputLanguage::He))
            .await
            .unwrap_err();
        assert_eq!(serde_json::to_value(error.code).unwrap(), code);
    }
}

#[test]
fn browser_formats_are_normalized_and_unsupported_raw_pcm_is_explicit() {
    for id in [ProviderId::Openai, ProviderId::Gemini] {
        let format = AudioFormat::parse(&id, "audio/webm;codecs=opus").unwrap();
        assert_eq!(format.mime, "audio/webm");
        assert!(format.matches(&[0x1a, 0x45, 0xdf, 0xa3]));
        assert!(AudioFormat::parse(&id, "audio/l16").is_err());
    }
    assert!(AudioFormat::parse(&ProviderId::Openai, "audio/ogg").is_err());
    assert!(AudioFormat::parse(&ProviderId::Gemini, "audio/ogg;codecs=opus").is_ok());
}

#[tokio::test]
async fn grok_http_sends_one_language_and_returns_only_the_transcript_text() {
    let audio = AudioFixture::new();
    let http = HttpFixture::new(
        200,
        json!({
            "text": " שלום Metrix\n",
            "language": "en",
            "duration": 1.00,
            "words": [{"text": "Metrix", "start": 0.1, "end": 0.4}]
        })
        .to_string(),
        Duration::ZERO,
    )
    .await;
    let adapter = http.provider(ProviderId::Grok);
    let mut settings = options(ProviderId::Grok, InputLanguage::En);
    settings.selection.model = "grok-voice-transcribe-2.0".into();
    settings.vocabulary.hints = vec!["Metrix".into(), "TrickTic".into()];
    let transcript = adapter.transcribe(&audio.0, &settings).await.unwrap();
    // Only the transcript is taken; timings and detection are not repurposed as text.
    assert_eq!(transcript.raw_text, " שלום Metrix\n");
    assert!(matches!(
        transcript.execution_location,
        ExecutionLocation::Cloud
    ));
    let requests = http.requests.lock().unwrap();
    let request = &requests[0];
    assert_eq!(request.uri, "/stt");
    assert_eq!(
        request.headers["authorization"],
        "Bearer fixture-key-never-real"
    );
    let body = String::from_utf8_lossy(&request.body);
    assert!(
        body.contains("name=\"model\"\r\n\r\ngrok-voice-transcribe-2.0"),
        "{body}"
    );
    assert!(body.contains("name=\"language\"\r\n\r\nen"), "{body}");
    assert!(body.contains("name=\"format\"\r\n\r\ntrue"));
    assert!(!body.contains("name=\"response_format\""));
    assert!(!body.contains("name=\"prompt\""));
    let names: Vec<_> = body
        .split("Content-Disposition: form-data; name=\"")
        .skip(1)
        .map(|part| part.split('"').next().unwrap())
        .collect();
    assert_eq!(
        names,
        ["model", "language", "format", "keyterm", "keyterm", "file"]
    );
    assert!(body.contains("name=\"keyterm\"\r\n\r\nMetrix"));
    assert!(body.contains("name=\"keyterm\"\r\n\r\nTrickTic"));
}

#[tokio::test]
async fn grok_is_registered_for_english_only_because_xai_publishes_no_hebrew() {
    let grok = crate::language::catalog()
        .models
        .into_iter()
        .find(|m| {
            matches!(m.selection.provider, ProviderId::Grok)
                && m.selection.model == "grok-voice-transcribe-2.0"
        })
        .expect("grok speech-to-text is registered");
    assert_eq!(grok.input_languages.len(), 1);
    assert!(matches!(grok.input_languages[0], InputLanguage::En));

    // The capability gate, not a hopeful hint: Hebrew and mixed are refused.
    let mut settings = options(ProviderId::Grok, InputLanguage::He);
    settings.selection.model = "grok-voice-transcribe-2.0".into();
    assert!(crate::language::validate_capabilities(&settings).is_err());
    settings.input_language = InputLanguage::Mixed;
    assert!(crate::language::validate_capabilities(&settings).is_err());
    settings.input_language = InputLanguage::En;
    assert!(crate::language::validate_capabilities(&settings).is_ok());
}

#[tokio::test]
async fn cloud_uploads_the_prepared_16k_mono_wav_instead_of_the_larger_original() {
    let mut audio = AudioFixture::new();
    // The original is a large WebM-labelled container; the prepared sidecar is
    // normalized PCM with a long pause between two speech spans.
    audio.0.metadata.mime_type = "audio/webm".into();
    std::fs::write(
        &audio.0.path,
        [&[0x1a, 0x45, 0xdf, 0xa3][..], &[7u8; 200_000]].concat(),
    )
    .unwrap();
    audio.0.metadata.byte_length = 200_004;
    let pcm: Vec<u8> = (0..16_000u32)
        .flat_map(|i| (((i % 7) as i16) * 100).to_le_bytes())
        .collect();
    let pcm = [&pcm[..], &vec![0; 8 * 32_000], &pcm[..]].concat();
    let expected =
        crate::local_stt::silence::compact(&pcm, &crate::local_stt::silence::removed(&pcm, 32.));
    assert!(expected.len() < pcm.len());
    let prepared = std::env::temp_dir().join(format!("writer-cloud-{}.pcm", audio.0.metadata.id));
    std::fs::write(&prepared, &pcm).unwrap();
    audio.0.prepared = Some(crate::providers::PreparedAudio {
        path: prepared.clone(),
        byte_length: pcm.len() as u64,
    });
    for id in [ProviderId::Openai, ProviderId::Gemini, ProviderId::Grok] {
        let http = HttpFixture::new(200, success(&id), Duration::ZERO).await;
        let adapter = http.provider(id.clone());
        adapter
            .transcribe(&audio.0, &options(id.clone(), InputLanguage::En))
            .await
            .unwrap();
        let requests = http.requests.lock().unwrap();
        let body = &requests[0].body;
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(expected.len() as u32 + 36).to_le_bytes());
        wav.extend_from_slice(
            b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data",
        );
        wav.extend_from_slice(&(expected.len() as u32).to_le_bytes());
        wav.extend_from_slice(&expected);
        if matches!(id, ProviderId::Openai | ProviderId::Grok) {
            let text = String::from_utf8_lossy(body);
            assert!(text.contains("filename=\"recording.wav\""));
            assert!(text.contains("Content-Type: audio/wav"));
            assert!(
                body.windows(wav.len()).any(|w| w == wav.as_slice()),
                "16 kHz mono WAV of the prepared PCM is uploaded"
            );
            assert!(
                !body.windows(200).any(|w| w == &[7u8; 200][..]),
                "the original container is not uploaded"
            );
        } else {
            let text = String::from_utf8_lossy(body);
            assert!(text.contains("\"mimeType\":\"audio/wav\""));
            assert!(text.contains(&STANDARD.encode(&wav)));
        }
    }
    let trace = crate::latency::snapshot(audio.0.metadata.id);
    assert_eq!(trace["details"]["cloud_upload"], "prepared_16k_mono");
    assert_eq!(
        trace["details"]["cloud_upload_bytes"],
        expected.len() as u64 + 44
    );
    assert!(trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["stage"] == "final_inference_done"));
    assert_eq!(std::fs::read(&prepared).unwrap(), pcm);
    // A sidecar whose length no longer matches is ignored in favour of the original.
    audio.0.prepared.as_mut().unwrap().byte_length += 2;
    let http = HttpFixture::new(200, success(&ProviderId::Openai), Duration::ZERO).await;
    http.provider(ProviderId::Openai)
        .transcribe(&audio.0, &options(ProviderId::Openai, InputLanguage::En))
        .await
        .unwrap();
    {
        let requests = http.requests.lock().unwrap();
        assert!(requests[0].body.windows(200).any(|w| w == &[7u8; 200][..]));
        assert!(String::from_utf8_lossy(&requests[0].body).contains("filename=\"recording.webm\""));
        assert_eq!(
            crate::latency::snapshot(audio.0.metadata.id)["details"]["cloud_upload"],
            "original"
        );
    }
    let xai = HttpFixture::new(200, success(&ProviderId::Grok), Duration::ZERO).await;
    assert!(matches!(
        xai.provider(ProviderId::Grok)
            .transcribe(&audio.0, &options(ProviderId::Grok, InputLanguage::En))
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedCapability
    ));
    assert!(xai.requests.lock().unwrap().is_empty());
    let _ = std::fs::remove_file(prepared);
}

#[tokio::test]
async fn transient_http_errors_preserve_retry_after_without_exposing_the_header() {
    for status in [429, 503] {
        let app = Router::new().fallback(post(move || async move {
            (
                StatusCode::from_u16(status).unwrap(),
                [(reqwest::header::RETRY_AFTER, "60")],
                "private provider response must not be exposed",
            )
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.layer(axum::extract::DefaultBodyLimit::max(510_000_000)),
            )
            .await
            .unwrap();
        });
        let adapter = CloudProvider::new(
            ProviderId::Openai,
            Some("fixture-key-never-real".into()),
            url,
            Duration::from_secs(5),
        );
        let error = adapter
            .transcribe(
                &AudioFixture::new().0,
                &options(ProviderId::Openai, InputLanguage::En),
            )
            .await
            .unwrap_err();
        server.abort();
        assert_eq!(error.retry_after_ms, Some(60_000), "{:?}", error);
        assert!(adapter.automatic_retry(&error));
        assert!(!error.message.contains("private provider response"));
    }
    assert_eq!(retry_after("invalid"), None);
    assert_eq!(retry_after("2"), Some(2000));
    let future = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(60));
    assert!((58_000..=60_000).contains(&retry_after(&future).unwrap()));
}

#[tokio::test]
async fn hard_provider_quota_is_actionable_and_excluded_from_automatic_retry() {
    let http = HttpFixture::new(
        429,
        json!({"error":{"code":"insufficient_quota","message":"private billing details"}})
            .to_string(),
        Duration::ZERO,
    )
    .await;
    let adapter = http.provider(ProviderId::Openai);
    let error = adapter
        .transcribe(
            &AudioFixture::new().0,
            &options(ProviderId::Openai, InputLanguage::En),
        )
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::RateLimited));
    assert!(!adapter.automatic_retry(&error));
    assert!(error.message.contains("quota is exhausted"));
    assert!(!error.message.contains("private billing"));
}

#[tokio::test]
async fn xai_rejects_unsupported_fields_and_stale_models_before_network() {
    let audio = AudioFixture::new();
    let http = HttpFixture::new(200, json!({"text":"unused"}).to_string(), Duration::ZERO).await;
    let mut settings = options(ProviderId::Grok, InputLanguage::En);
    settings.selection.model = "grok-voice-transcribe-2.0".into();
    for hints in [vec!["a".repeat(51)], vec!["term".into(); 101]] {
        settings.vocabulary.hints = hints;
        assert!(matches!(
            http.provider(ProviderId::Grok)
                .transcribe(&audio.0, &settings)
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        ));
    }
    settings.vocabulary.hints = vec![];
    for language in [InputLanguage::He, InputLanguage::Mixed] {
        settings.input_language = language;
        assert!(http
            .provider(ProviderId::Grok)
            .transcribe(&audio.0, &settings)
            .await
            .is_err());
    }
    settings.input_language = InputLanguage::En;
    settings.output_language = OutputLanguage::Translate {
        language: TextLanguage::En,
    };
    assert!(http
        .provider(ProviderId::Grok)
        .transcribe(&audio.0, &settings)
        .await
        .is_err());
    settings.output_language = OutputLanguage::Preserve {};
    settings.selection.model = "grok-speech-to-text".into();
    assert!(http
        .provider(ProviderId::Grok)
        .transcribe(&audio.0, &settings)
        .await
        .is_err());
    assert!(http.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn multipart_providers_require_string_text_and_preserve_empty_results() {
    let audio = AudioFixture::new();
    for id in [ProviderId::Openai, ProviderId::Grok] {
        let mut settings = options(id.clone(), InputLanguage::En);
        if matches!(id, ProviderId::Grok) {
            settings.selection.model = "grok-voice-transcribe-2.0".into();
        }
        for body in [json!({}), json!({"text":null}), json!({"text":42})] {
            let http = HttpFixture::new(200, body.to_string(), Duration::ZERO).await;
            assert!(matches!(
                http.provider(id.clone())
                    .transcribe(&audio.0, &settings)
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidProviderResponse
            ));
            assert_eq!(http.requests.lock().unwrap().len(), 1); // No hidden model/provider fallback.
        }
        let http = HttpFixture::new(200, json!({"text":""}).to_string(), Duration::ZERO).await;
        assert_eq!(
            http.provider(id)
                .transcribe(&audio.0, &settings)
                .await
                .unwrap()
                .raw_text,
            ""
        );
    }
}

#[tokio::test]
async fn xai_uploads_above_gemini_and_openai_limits_with_retained_audio_unchanged() {
    let mut audio = AudioFixture::new();
    let mut bytes = std::fs::read(&audio.0.path).unwrap();
    bytes.resize(25_000_001, 0);
    std::fs::write(&audio.0.path, &bytes).unwrap();
    audio.0.metadata.byte_length = bytes.len() as u64;
    let http = HttpFixture::new(200, json!({"text":"fixture"}).to_string(), Duration::ZERO).await;
    let mut settings = options(ProviderId::Grok, InputLanguage::En);
    settings.selection.model = "grok-voice-transcribe-2.0".into();
    // Axum's body limit is raised in the fixture; this is transport, not recognition.
    assert_eq!(
        http.provider(ProviderId::Grok)
            .transcribe(&audio.0, &settings)
            .await
            .unwrap()
            .raw_text,
        "fixture"
    );
    assert_eq!(std::fs::read(&audio.0.path).unwrap(), bytes);
}

#[tokio::test]
async fn legacy_openai_models_use_prompt_hints_without_new_model_only_fields() {
    let audio = AudioFixture::new();
    for model in ["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "whisper-1"] {
        let http = HttpFixture::new(200, success(&ProviderId::Openai), Duration::ZERO).await;
        let mut settings = options(ProviderId::Openai, InputLanguage::He);
        settings.selection.model = model.into();
        http.provider(ProviderId::Openai)
            .transcribe(&audio.0, &settings)
            .await
            .unwrap();
        let captured = http.requests.lock().unwrap();
        let body = String::from_utf8_lossy(&captured[0].body);
        assert!(body.contains("name=\"model\"\r\n\r\n") && body.contains(model));
        assert!(body.contains("name=\"response_format\"\r\n\r\njson"));
        assert!(body.contains("name=\"language\"\r\n\r\nhe"));
        assert!(
            !body.contains("keywords[]")
                && !body.contains("languages[]")
                && !body.contains("keyterm")
        );
        if model == "whisper-1" {
            assert!(body.contains("name=\"prompt\"\r\n\r\nMetrix\r\n"));
            assert!(!body.contains("Do not translate"));
        } else {
            assert!(body.contains("Possible spellings (use only if spoken)"));
        }
    }
}
#[test]
fn inline_threshold_counts_escaped_utf8_prompt_and_padding() {
    let mut opts = options(ProviderId::Gemini, InputLanguage::Mixed);
    opts.vocabulary.hints = vec!["שלום \"term\" \\ spelling".into()];
    let (head, tail) = upload::inline_envelope(&opts, "audio/wav").unwrap();
    for length in [0, 1, 2, 3, 48 * 1024 - 1, 48 * 1024, 48 * 1024 + 1] {
        let bytes = vec![7; length];
        let expected = serde_json::to_vec(&upload::gemini_body(
            &opts,
            json!({"inlineData":{"mimeType":"audio/wav","data":STANDARD.encode(bytes)}}),
        ))
        .unwrap();
        assert_eq!(
            upload::inline_size(length as u64, head.len(), tail.len()).unwrap(),
            expected.len() as u64
        );
    }
    let edge = ((GEMINI_REQUEST_LIMIT - head.len() - tail.len()) / 4 * 3) as u64;
    assert!(
        upload::inline_size(edge, head.len(), tail.len()).unwrap() <= GEMINI_REQUEST_LIMIT as u64
    );
    assert!(
        upload::inline_size(edge + 1, head.len(), tail.len()).unwrap()
            > GEMINI_REQUEST_LIMIT as u64
    );
    assert!(upload::inline_size(u64::MAX, 1, 1).is_err());
}

#[tokio::test]
async fn streamed_inline_bytes_match_across_chunk_boundaries_and_changed_files_fail() {
    use futures_util::TryStreamExt;
    use http_body_util::BodyExt;
    for length in [48 * 1024 - 1, 48 * 1024, 48 * 1024 + 1, 3 * 48 * 1024 + 2] {
        let mut audio = AudioFixture::new();
        let mut bytes = vec![7; length];
        bytes[..12].copy_from_slice(b"RIFF\0\0\0\0WAVE");
        std::fs::write(&audio.0.path, &bytes).unwrap();
        audio.0.metadata.byte_length = bytes.len() as u64;
        let mut upload = Upload::open(
            &audio.0,
            AudioFormat::parse(&ProviderId::Gemini, "audio/wav").unwrap(),
            GEMINI_FILE_LIMIT,
        )
        .await
        .unwrap();
        let (head, tail) = upload::inline_envelope(
            &options(ProviderId::Gemini, InputLanguage::Mixed),
            upload.format.mime,
        )
        .unwrap();
        let chunks: Vec<_> = upload
            .inline_body(head, tail)
            .await
            .unwrap()
            .into_data_stream()
            .try_collect()
            .await
            .unwrap();
        assert!(chunks.iter().all(|c| c.len() <= 64 * 1024));
        let body: Vec<_> = chunks.into_iter().flatten().collect();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            STANDARD
                .decode(
                    json["contents"][0]["parts"][1]["inlineData"]["data"]
                        .as_str()
                        .unwrap()
                )
                .unwrap(),
            bytes
        );
    }
    for change in [1, -1] {
        let mut audio = AudioFixture::new();
        let mut upload = Upload::open(
            &audio.0,
            AudioFormat::parse(&ProviderId::Openai, "audio/wav").unwrap(),
            OPENAI_FILE_LIMIT,
        )
        .await
        .unwrap();
        let length = (audio.0.metadata.byte_length as i64 + change) as u64;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&audio.0.path)
            .unwrap()
            .set_len(length)
            .unwrap();
        match upload.body().await {
            Err(_) => {}
            Ok(body) => assert!(body
                .into_data_stream()
                .try_collect::<Vec<_>>()
                .await
                .is_err()),
        }
        audio.0.metadata.byte_length = length;
    }
}

#[tokio::test]
async fn gemini_files_stream_reuse_and_credential_and_content_identity() {
    use axum::routing::any;
    use http_body_util::BodyExt;
    let requests = Arc::new(Mutex::new(Vec::<(String, HeaderMap, u64, [u8; 32])>::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let endpoint = format!("{origin}/v1beta");
    let captured = requests.clone();
    let host = origin.clone();
    let app = Router::new().fallback(any(move |headers: HeaderMap, uri: Uri, mut body: axum::body::Body| {
        let captured = captured.clone(); let host = host.clone();
        async move {
            let mut length = 0; let mut hash = Sha256::new();
            while let Some(frame) = body.frame().await { let frame = frame.unwrap(); if let Some(bytes) = frame.data_ref() { length += bytes.len() as u64; hash.update(bytes); } }
            let command = headers.get("x-goog-upload-command").and_then(|v|v.to_str().ok()).unwrap_or("").to_owned();
            captured.lock().unwrap().push((uri.to_string(), headers, length, hash.finalize().into()));
            if uri.path() == "/upload/v1beta/files" {
                ([("X-Goog-Upload-URL", format!("{host}/upload/session"))], String::new())
            } else if uri.path() == "/upload/session" {
                ([("X-Goog-Upload-Status", if command == "finalize" {"final"} else {"active"}.to_owned())], json!({"file":{"name":"files/fixture","uri":format!("{host}/v1beta/files/fixture"),"state":"ACTIVE"}}).to_string())
            } else if uri.path() == "/v1beta/files/fixture" {
                ([("X-Goog-Upload-URL", String::new())], json!({"name":"files/fixture","uri":format!("{host}/v1beta/files/fixture"),"state":"ACTIVE"}).to_string())
            } else { ([("X-Goog-Upload-URL", String::new())], success(&ProviderId::Gemini)) }
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut audio = AudioFixture::new();
    // Non-decodable synthetic WAV keeps normalization out of a transport test.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&audio.0.path)
        .unwrap()
        .set_len(16_000_000)
        .unwrap();
    audio.0.metadata.byte_length = 16_000_000;
    let mut adapter = CloudProvider::new(
        ProviderId::Gemini,
        Some("fixture-key".into()),
        endpoint,
        Duration::from_secs(10),
    );
    let opts = options(ProviderId::Gemini, InputLanguage::Mixed);
    adapter.transcribe(&audio.0, &opts).await.unwrap();
    adapter.transcribe(&audio.0, &opts).await.unwrap();
    {
        let captured = requests.lock().unwrap();
        assert_eq!(
            captured.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            [
                "/upload/v1beta/files",
                "/upload/session",
                "/upload/session",
                "/upload/session",
                "/v1beta/models/gemini-3.8-flash:generateContent",
                "/v1beta/files/fixture",
                "/v1beta/models/gemini-3.8-flash:generateContent"
            ]
        );
        assert_eq!(captured[1].1["content-length"], "8388608");
        assert_eq!(captured[1].1["x-goog-upload-offset"], "0");
        assert_eq!(captured[2].1["x-goog-upload-offset"], "8388608");
        assert_eq!(captured[1].2 + captured[2].2, audio.0.metadata.byte_length);
        assert_eq!(captured[3].1["x-goog-upload-command"], "finalize");
        assert_eq!(captured[3].2, 0);
        assert_eq!(
            captured[1].3,
            <[u8; 32]>::from(Sha256::digest(
                &std::fs::read(&audio.0.path).unwrap()[..8388608]
            ))
        );
    }
    adapter.source = Arc::new(StaticCredential(Some("changed-fixture-key".into())));
    adapter.transcribe(&audio.0, &opts).await.unwrap();
    {
        use std::io::{Seek, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&audio.0.path)
            .unwrap();
        file.seek(std::io::SeekFrom::Start(50)).unwrap();
        file.write_all(b"changed").unwrap();
    }
    adapter.transcribe(&audio.0, &opts).await.unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.0 == "/upload/session")
            .count(),
        9
    );
    server.abort();
}

#[tokio::test]
async fn disk_normalization_matches_existing_pcm_and_silence_semantics() {
    use futures_util::TryStreamExt;
    use http_body_util::BodyExt;
    for (rate, channels) in [
        (8000u32, 1u16),
        (16000, 1),
        (44100, 2),
        (48000, 2),
        (96000, 4),
    ] {
        let mut audio = AudioFixture::new();
        let data: Vec<_> = (0..rate / 4)
            .flat_map(|i| {
                std::iter::repeat_n(
                    (((i as f64 * 0.23).sin() * 2000.) as i16).to_le_bytes(),
                    channels as usize,
                )
                .flatten()
            })
            .collect();
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((data.len() as u32 + 36).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * channels as u32 * 2).to_le_bytes());
        bytes.extend((channels * 2).to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(&data);
        std::fs::write(&audio.0.path, &bytes).unwrap();
        audio.0.metadata.byte_length = bytes.len() as u64;
        let baseline = crate::local_stt::pcm::convert(&audio.0.path, 256 * 1024 * 1024, true)
            .await
            .unwrap()
            .unwrap();
        let normalized = normalize::normalize(&audio.0).await.unwrap();
        let path = normalized.audio.prepared.as_ref().unwrap().path.clone();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            baseline,
            "{rate} / {channels}"
        );
        drop(normalized);
        assert!(!path.exists());
        let expected = crate::local_stt::silence::wav(&crate::local_stt::silence::prepare(
            audio.0.metadata.id,
            &baseline,
        ));
        let mut upload = Upload::open(
            &audio.0,
            AudioFormat::parse(&ProviderId::Openai, "audio/wav").unwrap(),
            OPENAI_FILE_LIMIT,
        )
        .await
        .unwrap();
        let chunks: Vec<_> = upload
            .body()
            .await
            .unwrap()
            .into_data_stream()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(chunks.into_iter().flatten().collect::<Vec<_>>(), expected);
        assert_eq!(std::fs::read(&audio.0.path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn gemini_rejects_foreign_upload_url_and_slow_start_can_be_cancelled() {
    use axum::routing::any;
    let audio = AudioFixture::new();
    let hash = Sha256::digest(std::fs::read(&audio.0.path).unwrap());
    for slow in [false, true] {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = count.clone();
        let app = Router::new().fallback(any(move || {
            let observed = observed.clone();
            async move {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if slow {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                (
                    [("X-Goog-Upload-URL", "https://foreign.invalid/upload")],
                    "{}",
                )
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1beta", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let adapter = CloudProvider::new(
            ProviderId::Gemini,
            Some("fixture".into()),
            endpoint,
            Duration::from_millis(200),
        );
        let mut upload = Upload::open(
            &audio.0,
            AudioFormat::parse(&ProviderId::Gemini, "audio/wav").unwrap(),
            GEMINI_FILE_LIMIT,
        )
        .await
        .unwrap();
        let credential = adapter.credential().unwrap().0;
        let result = tokio::time::timeout(
            adapter.timeout,
            adapter.gemini_file(&mut upload, &credential),
        )
        .await;
        if slow {
            assert!(match result {
                Err(_) => true,
                Ok(Err(error)) => matches!(error.code, ErrorCode::ProviderTimeout),
                Ok(Ok(_)) => false,
            });
        } else {
            assert!(matches!(
                result.unwrap().unwrap_err().code,
                ErrorCode::InvalidProviderResponse
            ));
        }
        assert!(adapter.files.lock().unwrap().is_empty());
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(Sha256::digest(std::fs::read(&audio.0.path).unwrap()), hash);
        server.abort();
    }
}

#[tokio::test]
async fn cancelled_file_send_queries_confirmed_offset_and_errors_keep_audio() {
    use axum::routing::any;
    use http_body_util::BodyExt;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let audio = AudioFixture::new();
    let original = std::fs::read(&audio.0.path).unwrap();
    let offsets = Arc::new(Mutex::new(Vec::new()));
    let captured = offsets.clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = entered.clone();
    let fail = Arc::new(AtomicBool::new(false));
    let reject = fail.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let host = origin.clone();
    let app = Router::new().fallback(any(
        move |headers: HeaderMap, uri: Uri, mut body: axum::body::Body| {
            let captured = captured.clone();
            let signal = signal.clone();
            let reject = reject.clone();
            let host = host.clone();
            async move {
                if uri.path() == "/upload/v1beta/files" {
                    return (
                        StatusCode::OK,
                        [
                            ("X-Goog-Upload-URL", format!("{host}/upload/session")),
                            ("X-Goog-Upload-Status", "active".to_owned()),
                        ],
                        String::new(),
                    );
                }
                if headers
                    .get("x-goog-upload-command")
                    .is_some_and(|v| v == "query")
                {
                    return (
                        StatusCode::OK,
                        [
                            ("X-Goog-Upload-Status", "active".to_owned()),
                            ("X-Goog-Upload-Size-Received", "0".to_owned()),
                        ],
                        String::new(),
                    );
                }
                captured
                    .lock()
                    .unwrap()
                    .push(headers["x-goog-upload-offset"].to_str().unwrap().to_owned());
                body.frame().await.unwrap().unwrap();
                signal.notify_one();
                if !reject.load(SeqCst) {
                    std::future::pending::<()>().await;
                }
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [
                        ("X-Goog-Upload-URL", String::new()),
                        ("X-Goog-Upload-Status", "active".to_owned()),
                    ],
                    "private provider response".to_owned(),
                )
            }
        },
    ));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let adapter = CloudProvider::new(
        ProviderId::Gemini,
        Some("fixture".into()),
        format!("{origin}/v1beta"),
        Duration::from_secs(5),
    );
    let credential = adapter.credential().unwrap().0;
    {
        let mut upload = Upload::open(
            &audio.0,
            AudioFormat::parse(&ProviderId::Gemini, "audio/wav").unwrap(),
            GEMINI_FILE_LIMIT,
        )
        .await
        .unwrap();
        let future = adapter.gemini_file(&mut upload, &credential);
        tokio::pin!(future);
        tokio::select! { _=entered.notified()=>{}, result=&mut future=>panic!("Upload ended before cancellation: {result:?}") }
        assert!(tokio::time::timeout(Duration::from_millis(30), &mut future)
            .await
            .is_err());
        // Dropping this scope drops the request, stream and file reader.
    }
    assert!(adapter.files.lock().unwrap().is_empty());
    fail.store(true, SeqCst);
    let mut retry = Upload::open(
        &audio.0,
        AudioFormat::parse(&ProviderId::Gemini, "audio/wav").unwrap(),
        GEMINI_FILE_LIMIT,
    )
    .await
    .unwrap();
    let error = adapter
        .gemini_file(&mut retry, &credential)
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::ProviderFailed));
    assert!(error.retryable);
    assert!(!error.message.contains("private"));
    assert_eq!(*offsets.lock().unwrap(), ["0", "0"]);
    assert_eq!(std::fs::read(&audio.0.path).unwrap(), original);
    server.abort();
}
