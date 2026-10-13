use super::*;
use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Router,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

struct Fixture {
    dir: PathBuf,
    config: LocalConfig,
    audio: SavedAudio,
    options: TranscriptionOptions,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("writer-local-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let model_path = dir.join("model.bin");
        std::fs::write(&model_path, b"fixture weights, not a model").unwrap();
        let audio_path = dir.join("saved $(never execute); audio.wav");
        std::fs::write(&audio_path, b"retained audio").unwrap();
        let catalog: crate::transcription::ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        let mut metadata = catalog.recording.data.metadata;
        metadata.byte_length = 14;
        metadata.mime_type = "audio/wav".into();
        let mut options = catalog.transcription_request.data.options;
        options.selection.provider = ProviderId::Local;
        options.selection.model = "ivrit-turbo".into();
        options.input_language = InputLanguage::He;
        options.output_language = OutputLanguage::Preserve {};
        let f = Self {
            config: LocalConfig {
                worker_auth: None,
                inference_url: "http://127.0.0.1:1/inference".into(),
                model: "ivrit-turbo".into(),
                model_path,
                ffmpeg: dir.join("ffmpeg"),
                timeout: Duration::from_secs(3),
                audio_limit: 1024 * 1024,
            },
            audio: SavedAudio {
                prepared: None,
                path: audio_path,
                metadata,
            },
            options,
            dir,
        };
        f.runtime("success");
        f
    }
    fn runtime(&self, mode: &str) {
        let code = format!(
            r#"#!/usr/bin/python3
import json, sys, time
if sys.argv[1:] == ['-version']:
    print('fixture ffmpeg')
    sys.exit(0)
with open(__file__ + '.args', 'w') as f:
    json.dump(sys.argv[1:], f)
mode = {mode:?}
if mode == 'crash':
    print('PRIVATE UPSTREAM ERROR', file=sys.stderr)
    sys.exit(7)
if mode == 'timeout':
    time.sleep(10)
if mode == 'oversize':
    sys.stdout.buffer.write(b'x' * 2000000)
else:
    sys.stdout.buffer.write(bytes([0,16,0,32]) * 16000)
"#
        );
        std::fs::write(
            &self.config.ffmpeg,
            crate::test_support::python_fixture(&code),
        )
        .unwrap();
        std::fs::set_permissions(&self.config.ffmpeg, std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    fn retained(&self) {
        assert_eq!(std::fs::read(&self.audio.path).unwrap(), b"retained audio");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Clone)]
struct ServerState {
    health_status: StatusCode,
    health: String,
    status: StatusCode,
    output: String,
    delay: Duration,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    calls: Arc<AtomicUsize>,
}
struct Server {
    url: String,
    state: ServerState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(
    health_status: StatusCode,
    health: &str,
    status: StatusCode,
    output: &str,
    delay: Duration,
) -> Server {
    let state = ServerState {
        health_status,
        health: health.into(),
        status,
        output: output.into(),
        delay,
        bodies: Arc::new(Mutex::new(Vec::new())),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route(
            "/health",
            get(|State(s): State<ServerState>| async move { (s.health_status, s.health) }),
        )
        .route(
            "/inference",
            post(|State(s): State<ServerState>, body: Bytes| async move {
                s.calls.fetch_add(1, Ordering::SeqCst);
                s.bodies.lock().unwrap().push(body.to_vec());
                tokio::time::sleep(s.delay).await;
                (s.status, s.output)
            }),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/inference", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { url, state, task }
}
async fn healthy(output: &str) -> Server {
    server(
        StatusCode::OK,
        r#"{"status":"ok"}"#,
        StatusCode::OK,
        output,
        Duration::ZERO,
    )
    .await
}

#[tokio::test]
async fn nemo_receives_finite_wav_and_only_its_supported_context_fields() {
    let s = healthy(r#"{"text":"fixture speech"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    let mut config = f.config.clone();
    config.model = "parakeet-tdt-0-6b-v3".into();
    f.options.input_language = InputLanguage::En;
    f.options.vocabulary.hints.clear();
    // Stand in for ffmpeg's normalized PCM output, with no streaming WAV header.
    std::fs::write(&f.config.ffmpeg, crate::test_support::python_fixture("#!/usr/bin/python3\nimport sys\nif sys.argv[1:] == ['-version']: print('fixture ffmpeg')\nelse: sys.stdout.buffer.write(bytes([0,16,0,32]))\n")).unwrap();
    p.execute(&config, &f.audio, &f.options, false)
        .await
        .unwrap();
    f.options.vocabulary.hints.push("country".into());
    p.infer(
        &config,
        mixed::wav(&[0, 16]),
        &f.options,
        false,
        f.audio.metadata.id,
        None,
    )
    .await
    .unwrap();
    let bodies = s.state.bodies.lock().unwrap();
    let first = &bodies[0];
    let offset = first.windows(4).position(|v| v == b"RIFF").unwrap();
    assert_eq!(
        u32::from_le_bytes(first[offset + 4..offset + 8].try_into().unwrap()),
        40
    );
    assert_eq!(
        u32::from_le_bytes(first[offset + 40..offset + 44].try_into().unwrap()),
        4
    );
    let first = String::from_utf8_lossy(first);
    for unsupported in ["speech_contexts", "translate", "temperature", "prompt"] {
        assert!(!first.contains(&format!("name=\"{unsupported}\"")));
    }
    assert!(first.contains("name=\"verbatim\"\r\n\r\ntrue"));
    let second = String::from_utf8_lossy(&bodies[1]);
    assert!(second.contains("name=\"speech_contexts\""));
    assert!(second.contains(r#""phrases":["country"]"#));
    f.retained();
}

#[tokio::test]
async fn success_is_raw_hebrew_with_fixed_multipart_and_shell_free_conversion() {
    let s = healthy(r#"{"text":" שלום\n Metrix\n"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert!(!p.info().available);
    let transcript = p.transcribe(&f.audio, &f.options).await.unwrap();
    assert_eq!(transcript.raw_text, " שלום Metrix");
    assert!(matches!(
        transcript.execution_location,
        ExecutionLocation::SelfHostedServer
    ));
    assert!(p.info().available);
    let bodies = s.state.bodies.lock().unwrap();
    let body = String::from_utf8_lossy(&bodies[0]);
    for value in [
        "filename=\"recording.wav\"",
        "name=\"language\"\r\n\r\nhe",
        "name=\"translate\"\r\n\r\nfalse",
        "name=\"response_format\"\r\n\r\njson",
    ] {
        assert!(body.contains(value), "missing {value}");
    }
    assert!(!body.contains("model.bin"));
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(f.config.ffmpeg.with_extension("args")).unwrap())
            .unwrap();
    // The provider resolves symlinks before spawning; macOS temporary paths
    // commonly enter through /var and resolve under /private/var.
    let canonical_audio = f.audio.path.canonicalize().unwrap();
    assert!(args
        .windows(2)
        .any(|v| v == ["-i", canonical_audio.to_str().unwrap()]));
    assert!(args
        .windows(2)
        .any(|v| v == ["-protocol_whitelist", "file,pipe"]));
    f.retained();
}

#[tokio::test]
async fn readiness_distinguishes_missing_weights_runtime_loading_and_bad_health() {
    let f = Fixture::new();
    let mut config = f.config.clone();
    config.model_path = f.dir.join("missing.bin");
    let p = LocalProvider::new(Some(config.clone())).unwrap();
    assert_eq!(p.readiness().await, Readiness::MissingModel);
    assert!(p.info().configured);
    assert!(!p.info().available);
    config.model_path = f.config.model_path.clone();
    config.ffmpeg = f.dir.join("missing-runtime");
    assert_eq!(
        LocalProvider::new(Some(config)).unwrap().readiness().await,
        Readiness::MissingRuntime
    );
    assert_eq!(
        LocalProvider::new(None).unwrap().readiness().await,
        Readiness::Unconfigured
    );
    for (status, health, expected) in [
        (
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"status":"loading model"}"#,
            Readiness::LoadingModel,
        ),
        (StatusCode::OK, "not JSON", Readiness::InvalidResponse),
        (StatusCode::OK, r#"{"status":"ok"}"#, Readiness::Ready),
    ] {
        let s = server(status, health, StatusCode::OK, "{}", Duration::ZERO).await;
        let mut config = f.config.clone();
        config.inference_url = s.url.clone();
        assert_eq!(
            LocalProvider::new(Some(config)).unwrap().readiness().await,
            expected
        );
    }
    assert_eq!(
        LocalProvider::new(Some(f.config.clone()))
            .unwrap()
            .readiness()
            .await,
        Readiness::Unreachable
    );
    f.retained();
}

#[tokio::test]
async fn conversion_crash_timeout_and_output_overflow_preserve_audio() {
    let s = healthy(r#"{"text":"unused"}"#).await;
    for (mode, expected) in [
        ("crash", ErrorCode::ProviderFailed),
        ("timeout", ErrorCode::ProviderTimeout),
        ("oversize", ErrorCode::UnsupportedCapability),
    ] {
        let mut f = Fixture::new();
        f.config.inference_url = s.url.clone();
        f.config.timeout = if matches!(expected, ErrorCode::ProviderTimeout) {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(3)
        };
        f.runtime(mode);
        let p = LocalProvider::new(Some(f.config.clone())).unwrap();
        let e = p.transcribe(&f.audio, &f.options).await.unwrap_err();
        assert_eq!(
            std::mem::discriminant(&e.code),
            std::mem::discriminant(&expected)
        );
        assert!(!e.message.contains("PRIVATE"));
        assert!(!p.info().available);
        f.retained();
    }
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn upstream_failure_timeout_malformed_and_oversized_outputs_allow_retry() {
    let oversized = format!("{{\"text\":\"{}\"}}", "a".repeat(RESPONSE_LIMIT));
    for (status, output, delay, expected) in [
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "PRIVATE",
            Duration::ZERO,
            ErrorCode::ProviderFailed,
        ),
        (
            StatusCode::OK,
            "not json",
            Duration::ZERO,
            ErrorCode::InvalidProviderResponse,
        ),
        (
            StatusCode::OK,
            r#"{"text":5}"#,
            Duration::ZERO,
            ErrorCode::InvalidProviderResponse,
        ),
        (
            StatusCode::OK,
            r#"{"text":"fake","error":"failed"}"#,
            Duration::ZERO,
            ErrorCode::InvalidProviderResponse,
        ),
        (
            StatusCode::OK,
            oversized.as_str(),
            Duration::ZERO,
            ErrorCode::InvalidProviderResponse,
        ),
        (
            StatusCode::OK,
            r#"{"text":"late"}"#,
            Duration::from_secs(3),
            ErrorCode::ProviderTimeout,
        ),
    ] {
        let s = server(StatusCode::OK, r#"{"status":"ok"}"#, status, output, delay).await;
        let mut f = Fixture::new();
        f.config.inference_url = s.url.clone();
        f.config.timeout = if matches!(expected, ErrorCode::ProviderTimeout) {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(3)
        };
        let e = LocalProvider::new(Some(f.config.clone()))
            .unwrap()
            .transcribe(&f.audio, &f.options)
            .await
            .unwrap_err();
        assert_eq!(
            std::mem::discriminant(&e.code),
            std::mem::discriminant(&expected)
        );
        assert!(e.retryable);
        assert!(!e.message.contains("PRIVATE"));
        f.retained();
        let good = healthy(r#"{"text":"retried"}"#).await;
        f.config.inference_url = good.url.clone();
        f.config.timeout = Duration::from_secs(3);
        assert_eq!(
            LocalProvider::new(Some(f.config.clone()))
                .unwrap()
                .transcribe(&f.audio, &f.options)
                .await
                .unwrap()
                .raw_text,
            "retried"
        );
        f.retained();
    }
}

#[tokio::test]
async fn rejects_unallowlisted_models_translation_and_ivrit_english_before_io() {
    let f = Fixture::new();
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    let mut options = f.options.clone();
    options.selection.model = ";$(touch arbitrary)".into();
    assert!(matches!(
        p.transcribe(&f.audio, &options).await.unwrap_err().code,
        ErrorCode::InvalidRequest
    ));
    options = f.options.clone();
    options.input_language = InputLanguage::En;
    assert!(matches!(
        p.transcribe(&f.audio, &options).await.unwrap_err().code,
        ErrorCode::UnsupportedCapability
    ));
    options.input_language = InputLanguage::Mixed;
    assert!(matches!(
        p.transcribe(&f.audio, &options).await.unwrap_err().code,
        ErrorCode::UnsupportedCapability
    ));
    options = f.options.clone();
    options.output_language = OutputLanguage::Translate {
        language: crate::transcription::TextLanguage::En,
    };
    assert!(matches!(
        p.transcribe(&f.audio, &options).await.unwrap_err().code,
        ErrorCode::UnsupportedCapability
    ));
    f.retained();
}

#[test]
fn configuration_rejects_browser_controlled_paths_urls_and_model_names() {
    let f = Fixture::new();
    for url in [
        "file:///inference",
        "http://user:password@localhost/inference",
        "http://localhost/load",
        "http://localhost/inference?model=bad",
    ] {
        let mut c = f.config.clone();
        c.inference_url = url.into();
        assert!(LocalProvider::new(Some(c)).is_err());
    }
    let mut c = f.config.clone();
    c.model = "download-me".into();
    assert!(LocalProvider::new(Some(c)).is_err());
}

#[tokio::test]
async fn real_ffmpeg_converts_synthetic_audio_without_modifying_source() {
    let mut f = Fixture::new();
    if !Path::new("/usr/bin/ffmpeg").exists() {
        eprintln!("ffmpeg absent: skipped real conversion smoke");
        return;
    }
    // 0.1 seconds of synthetic PCM silence; no private speech fixture.
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend(3236u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(16000u32.to_le_bytes());
    wav.extend(32000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(3200u32.to_le_bytes());
    wav.resize(3244, 0);
    std::fs::write(&f.audio.path, &wav).unwrap();
    f.audio.metadata.byte_length = wav.len() as u64;
    let s = healthy(r#"{"text":""}"#).await;
    f.config.inference_url = s.url.clone();
    f.config.ffmpeg = "/usr/bin/ffmpeg".into();
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        ""
    );
    assert_eq!(std::fs::read(&f.audio.path).unwrap(), wav);
}

/// Operator-run proof against a separately started, GPU-verified server.
#[tokio::test]
#[ignore = "requires configured GPU whisper.cpp, model mount and an explicit saved clip"]
async fn live_local_transcription() {
    let config = LocalConfig::from_env().expect("local server configuration");
    let clip =
        PathBuf::from(std::env::var_os("WRITER_LOCAL_STT_TEST_AUDIO").expect("saved WAV clip"));
    let mut f = Fixture::new();
    f.audio.path = clip;
    f.audio.metadata.byte_length = std::fs::metadata(&f.audio.path).unwrap().len();
    f.options.selection.model = config.model.clone();
    f.options.input_language = if config.model == "ivrit-turbo" {
        InputLanguage::He
    } else {
        InputLanguage::En
    };
    let before = std::fs::read(&f.audio.path).unwrap();
    let start = Instant::now();
    let p = LocalProvider::new(Some(config)).unwrap();
    assert_eq!(p.readiness().await, Readiness::Ready);
    let result = p.transcribe(&f.audio, &f.options).await.unwrap();
    assert!(
        !result.raw_text.trim().is_empty(),
        "Use a clip containing speech"
    );
    assert_eq!(std::fs::read(&f.audio.path).unwrap(), before);
    eprintln!(
        "local model={} elapsed_ms={} transcript_bytes={}",
        result.selection.model,
        start.elapsed().as_millis(),
        result.raw_text.len()
    );
    if let Some(path) = std::env::var_os("WRITER_LOCAL_STT_TEST_OUTPUT") {
        std::fs::write(path, result.raw_text).unwrap();
    }
}

#[tokio::test]
async fn real_ffmpeg_handles_browser_webm_and_general_model_language_modes() {
    if !Path::new("/usr/bin/ffmpeg").exists() {
        return;
    }
    let mut f = Fixture::new();
    // Generate a browser-compatible WebM/Opus container from synthetic silence.
    let output = std::process::Command::new("/usr/bin/ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=mono",
            "-t",
            "0.1",
            "-c:a",
            "libopus",
            "-f",
            "webm",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(&f.audio.path, &output.stdout).unwrap();
    f.audio.metadata.byte_length = output.stdout.len() as u64;
    f.audio.metadata.mime_type = "audio/webm;codecs=opus".into();
    f.config.ffmpeg = "/usr/bin/ffmpeg".into();
    f.config.model = "whisper-tiny".into();
    f.options.selection.model = "whisper-tiny".into();
    let s = healthy(r#"{"text":""}"#).await;
    f.config.inference_url = s.url.clone();
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    for (language, wire) in [
        (InputLanguage::En, "en"),
        (InputLanguage::He, "he"),
        (InputLanguage::Mixed, "auto"),
    ] {
        f.options.input_language = language;
        p.transcribe(&f.audio, &f.options).await.unwrap();
        let bodies = s.state.bodies.lock().unwrap();
        let body = String::from_utf8_lossy(bodies.last().unwrap());
        assert!(body.contains(&format!("name=\"language\"\r\n\r\n{wire}")));
    }
    assert_eq!(std::fs::read(&f.audio.path).unwrap(), output.stdout);
}

#[tokio::test]
async fn redirected_inference_does_not_forward_audio() {
    let target = healthy(r#"{"text":"must not receive audio"}"#).await;
    let destination = target.url.clone();
    let app = Router::new()
        .route("/health", get(|| async { r#"{"status":"ok"}"# }))
        .route(
            "/inference",
            post(move || {
                let destination = destination.clone();
                async move {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(axum::http::header::LOCATION, destination)],
                        "",
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut f = Fixture::new();
    f.config.inference_url = format!("http://{}/inference", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let result = LocalProvider::new(Some(f.config.clone()))
        .unwrap()
        .transcribe(&f.audio, &f.options)
        .await;
    task.abort();
    assert!(matches!(
        result.unwrap_err().code,
        ErrorCode::ProviderFailed
    ));
    assert_eq!(target.state.calls.load(Ordering::SeqCst), 0);
    f.retained();
}

#[tokio::test]
async fn mixed_requests_redetect_each_phrase_and_never_return_partial_success() {
    for mode in ["success", "failed", "oversized", "timeout"] {
        let mut f = Fixture::new();
        f.config.model = "whisper-tiny".into();
        f.options.selection.model = f.config.model.clone();
        f.options.input_language = InputLanguage::Mixed;
        if mode == "timeout" {
            f.config.timeout = Duration::from_secs(3);
        }
        let mut pcm: Vec<u8> = (0..32000).flat_map(|_| 12000_i16.to_le_bytes()).collect();
        pcm.extend(vec![0; 12800]);
        pcm.extend((0..32000).flat_map(|_| (-12000_i16).to_le_bytes()));
        std::fs::write(f.dir.join("ffmpeg.pcm"), &pcm).unwrap();
        std::fs::write(&f.config.ffmpeg, crate::test_support::python_fixture("#!/usr/bin/python3\nimport pathlib,sys\nif sys.argv[1:] == ['-version']: print('fixture ffmpeg')\nelse: sys.stdout.buffer.write(pathlib.Path(__file__).with_suffix('.pcm').read_bytes())\n")).unwrap();
        let bodies = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let seen = bodies.clone();
        let app = Router::new()
            .route("/health", get(|| async { r#"{"status":"ok"}"# }))
            .route(
                "/inference",
                post(move |body: Bytes| {
                    let seen = seen.clone();
                    async move {
                        let index = {
                            let mut bodies = seen.lock().unwrap();
                            bodies.push(body.to_vec());
                            bodies.len()
                        };
                        if mode == "timeout" {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                        if mode == "oversized" {
                            (
                                StatusCode::OK,
                                serde_json::json!({"text":"x".repeat(RESPONSE_LIMIT / 2)})
                                    .to_string(),
                            )
                        } else if index == 1 {
                            (StatusCode::OK, r#"{"text":"fixture Hebrew"}"#.to_owned())
                        } else if mode == "failed" {
                            (StatusCode::INTERNAL_SERVER_ERROR, "failure".to_owned())
                        } else {
                            (StatusCode::OK, r#"{"text":"fixture English"}"#.to_owned())
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        f.config.inference_url = format!("http://{}/inference", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = LocalProvider::new(Some(f.config.clone())).unwrap();
        let result = provider.transcribe(&f.audio, &f.options).await;
        if mode == "success" {
            assert_eq!(result.unwrap().raw_text, "fixture Hebrew fixture English");
        } else {
            let error = result.unwrap_err();
            if mode == "timeout" {
                assert!(matches!(error.code, ErrorCode::ProviderTimeout));
            }
            if mode == "oversized" {
                assert!(matches!(error.code, ErrorCode::InvalidProviderResponse));
            }
        }
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        let mut retained_pcm = Vec::new();
        for body in bodies.iter() {
            let text = String::from_utf8_lossy(body);
            assert!(text.contains("name=\"language\"\r\n\r\nauto"));
            assert!(text.contains("name=\"translate\"\r\n\r\nfalse"));
            let start = body.windows(4).position(|v| v == b"RIFF").unwrap();
            let length =
                u32::from_le_bytes(body[start + 40..start + 44].try_into().unwrap()) as usize;
            retained_pcm.extend_from_slice(&body[start + 44..start + 44 + length]);
        }
        assert_eq!(
            retained_pcm, pcm,
            "phrase requests must preserve every sample exactly once"
        );
        f.retained();
        task.abort();
    }
}

#[tokio::test]
async fn live_segments_are_reused_by_final_job_without_losing_tail_or_crossing_options() {
    if !Path::new("/usr/bin/ffmpeg").exists() {
        return;
    }
    let mut f = Fixture::new();
    let server = healthy(r#"{"text":"שלום hello"}"#).await;
    f.config.inference_url = server.url.clone();
    f.config.ffmpeg = "/usr/bin/ffmpeg".into();
    let pcm: Vec<u8> = (0..6 * 16000)
        .flat_map(|i| {
            (if (2 * 16000..3 * 16000).contains(&i) {
                0_i16
            } else if i % 2 == 0 {
                12000_i16
            } else {
                -12000_i16
            })
            .to_le_bytes()
        })
        .collect();
    let wav = mixed::wav(&pcm);
    std::fs::write(&f.audio.path, &wav).unwrap();
    f.audio.metadata.byte_length = wav.len() as u64;
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(p.preview(&f.audio, &f.options).await.unwrap(), "שלום hello");
    let first = server.state.calls.load(Ordering::SeqCst);
    assert_eq!(first, 1);
    assert_eq!(p.preview(&f.audio, &f.options).await.unwrap(), "שלום hello");
    assert_eq!(server.state.calls.load(Ordering::SeqCst), first);
    let text = p.transcribe(&f.audio, &f.options).await.unwrap().raw_text;
    assert_eq!(text, "שלום hello שלום hello");
    assert_eq!(
        server.state.calls.load(Ordering::SeqCst),
        first + 1,
        "only final tail should require inference"
    );
    let mut with_cleanup = f.options.clone();
    with_cleanup.cleanup = crate::transcription::CleanupPolicy::Enabled {
        selection: crate::transcription::ModelSelection {
            provider: ProviderId::Openai,
            model: "cleanup-fixture".into(),
        },
    };
    p.transcribe(&f.audio, &with_cleanup).await.unwrap();
    assert_eq!(
        server.state.calls.load(Ordering::SeqCst),
        first + 1,
        "post-recognition cleanup must not invalidate precomputed speech"
    );
    let mut options = f.options.clone();
    options.vocabulary.hints.push("different context".into());
    p.transcribe(&f.audio, &options).await.unwrap();
    assert_eq!(
        server.state.calls.load(Ordering::SeqCst),
        first + 3,
        "changed options must not reuse segments"
    );
    assert_eq!(std::fs::read(&f.audio.path).unwrap(), wav);
}

#[tokio::test]
async fn growing_preview_closes_a_pause_before_speech_resumes_and_reuses_it() {
    if !Path::new("/usr/bin/ffmpeg").exists() {
        return;
    }
    let mut f = Fixture::new();
    let server = healthy(r#"{"text":"phrase"}"#).await;
    f.config.inference_url = server.url.clone();
    f.config.ffmpeg = "/usr/bin/ffmpeg".into();
    f.config.audio_limit = 2 * 1024 * 1024;
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    for (frames, expected) in [(35200usize, 0usize), (36800, 1), (48000, 1), (64000, 1)] {
        let pcm: Vec<u8> = (0..frames)
            .flat_map(|i| {
                (if (32000..48000).contains(&i) {
                    0_i16
                } else if i % 2 == 0 {
                    12000_i16
                } else {
                    -12000_i16
                })
                .to_le_bytes()
            })
            .collect();
        let wav = mixed::wav(&pcm);
        std::fs::write(&f.audio.path, &wav).unwrap();
        f.audio.metadata.byte_length = wav.len() as u64;
        p.preview(&f.audio, &f.options).await.unwrap();
        assert_eq!(
            server.state.calls.load(Ordering::SeqCst),
            expected,
            "frames={frames}"
        );
    }
    p.transcribe(&f.audio, &f.options).await.unwrap();
    assert_eq!(
        server.state.calls.load(Ordering::SeqCst),
        2,
        "final inference should reuse the stable first phrase"
    );
}

#[tokio::test]
async fn prepared_capture_reuses_closed_phrases_and_only_decodes_the_final_tail() {
    let s = healthy(r#"{"text":"fixture phrase"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    // Deliberately exceed the ordinary upload/conversion buffer limit. The native
    // journal supplies bounded committed PCM windows without loading that file.
    f.config.audio_limit = 1024;
    let mut pcm: Vec<u8> = (0..3 * 16000)
        .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
        .collect();
    pcm.extend(vec![0; 16000]);
    let path = f.dir.join("prepared.pcm");
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path: path.clone(),
        byte_length: pcm.len() as u64,
    });
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(
        p.preview(&f.audio, &f.options).await.unwrap(),
        "fixture phrase"
    );
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    pcm.extend(
        (0..16000).flat_map(|i| (if i % 2 == 0 { 10000i16 } else { -10000i16 }).to_le_bytes()),
    );
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    let result = p.transcribe(&f.audio, &f.options).await.unwrap();
    assert_eq!(result.raw_text, "fixture phrase fixture phrase");
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 2);
    for body in s.state.bodies.lock().unwrap().iter() {
        assert!(body.len() < 8 * 32000 + 16384);
        let body = String::from_utf8_lossy(body);
        assert!(body.contains("name=\"audio_ctx\"\r\n\r\n768"));
        assert!(body.contains("name=\"no_timestamps\"\r\n\r\ntrue"));
    }
    assert!(!f.config.ffmpeg.with_extension("args").exists());
    f.retained();
}

#[tokio::test]
async fn pcm16_wav_does_not_launch_ffmpeg_for_conversion() {
    let s = healthy(r#"{"text":"fixture"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    f.runtime("crash"); // health -version still succeeds, actual conversion cannot.
    let wav = mixed::wav(&speech::fixtures::voiced(16_000));
    std::fs::write(&f.audio.path, &wav).unwrap();
    f.audio.metadata.byte_length = wav.len() as u64;
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        "fixture"
    );
    assert_eq!(std::fs::read(&f.audio.path).unwrap(), wav);
}

#[tokio::test]
async fn finalization_interrupts_preview_before_waiting_for_the_provider_slot() {
    let started = Arc::new(tokio::sync::Notify::new());
    let canceled = Arc::new(tokio::sync::Notify::new());
    let marker = b"name=\"writer_preview\"\r\n\r\ntrue";
    let app = {
        let started = started.clone();
        let canceled = canceled.clone();
        let signal = canceled.clone();
        Router::new()
            .route(
                "/health",
                get(|| async { r#"{"status":"ok","writer_protocol":1,"warm":true}"# }),
            )
            .route(
                "/writer/finalize",
                post(move |headers: axum::http::HeaderMap, body: Bytes| {
                    let signal = signal.clone();
                    async move {
                        assert_eq!(
                            headers.get("content-length").and_then(|v| v.to_str().ok()),
                            Some("0"),
                            "the worker requires an explicitly delimited empty POST"
                        );
                        assert!(body.is_empty());
                        signal.notify_one();
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/inference",
                post(move |body: Bytes| {
                    let started = started.clone();
                    let canceled = canceled.clone();
                    async move {
                        if body.windows(marker.len()).any(|b| b == marker) {
                            started.notify_one();
                            canceled.notified().await;
                            (StatusCode::CONFLICT, r#"{"error":"preview canceled"}"#)
                        } else {
                            (StatusCode::OK, r#"{"text":"final fixture"}"#)
                        }
                    }
                }),
            )
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut f = Fixture::new();
    f.config.inference_url = format!("http://{}/inference", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut pcm: Vec<_> = (0..3 * 16000)
        .flat_map(|_| 12000i16.to_le_bytes())
        .collect();
    pcm.extend(vec![0; 16000]);
    let wav = mixed::wav(&pcm);
    std::fs::write(&f.audio.path, &wav).unwrap();
    f.audio.metadata.byte_length = wav.len() as u64;
    let p = Arc::new(LocalProvider::new(Some(f.config.clone())).unwrap());
    let preview = {
        let p = p.clone();
        let audio = f.audio.clone();
        let options = f.options.clone();
        tokio::spawn(async move { p.preview(&audio, &options).await })
    };
    tokio::time::timeout(Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    let final_result =
        tokio::time::timeout(Duration::from_secs(2), p.transcribe(&f.audio, &f.options))
            .await
            .unwrap()
            .unwrap();
    assert!(preview.await.unwrap().is_err());
    assert_eq!(final_result.raw_text, "final fixture");
    assert_eq!(std::fs::read(&f.audio.path).unwrap(), wav);
    server.abort();
}

#[tokio::test]
async fn prepared_open_phrase_is_speculated_and_reused_only_for_identical_audio() {
    let s = healthy(r#"{"text":"open phrase"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    let path = f.dir.join("prepared.pcm");
    let pcm: Vec<_> = (0..16000)
        .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
        .collect();
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path: path.clone(),
        byte_length: pcm.len() as u64,
    });
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(
        p.preview(&f.audio, &f.options).await.unwrap(),
        "open phrase"
    );
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        "open phrase"
    );
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        1,
        "exact open result should survive Stop"
    );
    let mut changed = pcm.clone();
    changed[1000] ^= 1;
    std::fs::write(&path, changed).unwrap();
    p.transcribe(&f.audio, &f.options).await.unwrap();
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        2,
        "changed PCM must be decoded"
    );
    let mut options = f.options.clone();
    options.vocabulary.hints.push("changed".into());
    p.transcribe(&f.audio, &options).await.unwrap();
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        3,
        "changed options must be decoded"
    );
}

#[tokio::test]
async fn prepared_cached_phrase_skips_silence_but_decodes_resumed_soft_speech() {
    let s = healthy(r#"{"text":"phrase"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    let path = f.dir.join("prepared.pcm");
    let mut pcm: Vec<_> = (0..48000)
        .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
        .collect();
    pcm.extend(vec![0; 16000]);
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path: path.clone(),
        byte_length: pcm.len() as u64,
    });
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    p.preview(&f.audio, &f.options).await.unwrap();
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        "phrase"
    );
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        1,
        "silence-only tail must not run the model"
    );
    pcm.extend((0..4000).flat_map(|i| (if i % 2 == 0 { 10i16 } else { -10i16 }).to_le_bytes()));
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        "phrase phrase"
    );
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        2,
        "quiet resumed input is not silence"
    );
    assert_eq!(std::fs::read(&path).unwrap(), pcm);
}

#[tokio::test]
async fn stop_retains_matching_inflight_preview_and_reuses_its_result() {
    let s = server(
        StatusCode::OK,
        r#"{"status":"ok","writer_protocol":1,"warm":true}"#,
        StatusCode::OK,
        r#"{"text":"complete phrase"}"#,
        Duration::from_millis(150),
    )
    .await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    let pcm: Vec<_> = (0..16000)
        .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
        .collect();
    let path = f.dir.join("prepared.pcm");
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path,
        byte_length: pcm.len() as u64,
    });
    let p = Arc::new(LocalProvider::new(Some(f.config.clone())).unwrap());
    assert_eq!(p.readiness().await, Readiness::Ready);
    let preview = {
        let p = p.clone();
        let audio = f.audio.clone();
        let options = f.options.clone();
        tokio::spawn(async move { p.preview(&audio, &options).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while s.state.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let result = p.transcribe(&f.audio, &f.options).await.unwrap();
    preview.await.unwrap().unwrap();
    assert_eq!(result.raw_text, "complete phrase");
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    let trace = crate::latency::snapshot(f.audio.metadata.id);
    assert!(trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["stage"] == "matching_preview_retained"));
}

#[tokio::test]
async fn digital_silence_retains_audio_without_hallucinating_a_transcript() {
    let s = healthy(r#"{"text":"must not appear"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    let path = f.dir.join("prepared.pcm");
    let pcm = vec![0; 32_000];
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path: path.clone(),
        byte_length: 32_000,
    });
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(p.preview(&f.audio, &f.options).await.unwrap(), "");
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        ""
    );
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(path).unwrap(), pcm);
    f.retained();
}

#[tokio::test]
async fn room_tone_skips_recognition_in_previews_final_jobs_and_saved_wav_retries() {
    let s = healthy(r#"{"text":" you"}"#).await;
    for language in [InputLanguage::En, InputLanguage::He, InputLanguage::Mixed] {
        for prepared in [false, true] {
            let mut f = Fixture::new();
            f.options.input_language = language.clone();
            f.options.selection.model = "whisper-tiny".into();
            f.config.model = f.options.selection.model.clone();
            f.config.inference_url = s.url.clone();
            let pcm = speech::fixtures::room(3_200);
            let wav = mixed::wav(&pcm);
            std::fs::write(&f.audio.path, &wav).unwrap();
            f.audio.metadata.byte_length = wav.len() as u64;
            if prepared {
                let path = f.dir.join("prepared.pcm");
                std::fs::write(&path, &pcm).unwrap();
                f.audio.prepared = Some(crate::providers::PreparedAudio {
                    path,
                    byte_length: pcm.len() as u64,
                });
            }
            let p = LocalProvider::new(Some(f.config.clone())).unwrap();
            assert_eq!(p.preview(&f.audio, &f.options).await.unwrap(), "");
            assert_eq!(
                p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
                ""
            );
            assert_eq!(std::fs::read(&f.audio.path).unwrap(), wav);
        }
    }
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn quiet_preview_does_not_commit_noise_and_discard_a_later_short_word() {
    let s = healthy(r#"{"text":" you"}"#).await;
    let mut f = Fixture::new();
    f.config.inference_url = s.url.clone();
    let path = f.dir.join("prepared.pcm");
    let mut pcm = speech::fixtures::room(8_000);
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path: path.clone(),
        byte_length: pcm.len() as u64,
    });
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(p.preview(&f.audio, &f.options).await.unwrap(), "");
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 0);
    pcm.extend(speech::fixtures::voiced(3_200));
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    assert_eq!(p.preview(&f.audio, &f.options).await.unwrap(), " you");
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        " you"
    );
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(path).unwrap(), pcm);
    f.retained();
}

#[test]
fn every_whisper_model_gets_the_short_window_profile_for_bounded_windows_only() {
    let profile = |model: &str, len: usize| {
        let p = whisper_profile(model, len);
        (p.audio_ctx, p.no_timestamps)
    };
    // Qualified on public English (30 clips, 3-9.4 s): context 512 matched 768.
    assert_eq!(
        profile("whisper-large-v3-turbo-q5-0", 5 * 32_000 + 44),
        (512, true)
    );
    assert_eq!(
        profile("whisper-large-v3-turbo-q5-0", SHORT_CONTEXT_BYTES),
        (512, true)
    );
    assert_eq!(
        profile("whisper-large-v3-turbo-q5-0", SHORT_CONTEXT_BYTES + 2),
        (768, true)
    );
    // Below 1.5 s only 768 is qualified; ivrit keeps 768 (512 broke a 0.76 s clip).
    assert_eq!(
        profile("whisper-large-v3-turbo-q5-0", 32_000 + 44),
        (768, true)
    );
    assert_eq!(profile("whisper-tiny", SHORT_WINDOW_BYTES), (768, true));
    assert_eq!(profile("ivrit-turbo", 3 * 32_000 + 44), (768, true));
    // Batch input beyond the bounded live window keeps Whisper's full context.
    assert_eq!(
        profile("whisper-large-v3-turbo-q5-0", SHORT_WINDOW_BYTES + 2),
        (0, false)
    );
    assert_eq!(profile("ivrit-turbo", 16 * 32_000 + 44), (0, true));
    assert_eq!(TEMPERATURE_INC, "0.5");
}

#[tokio::test]
async fn english_whisper_requests_send_the_capped_context_for_short_windows_only() {
    let s = healthy(r#"{"text":"english"}"#).await;
    let mut f = Fixture::new();
    f.audio.metadata.id = Uuid::new_v4(); // own latency trace
    f.config.inference_url = s.url.clone();
    f.config.model = "whisper-tiny".into();
    f.options.selection.model = "whisper-tiny".into();
    f.options.input_language = InputLanguage::En;
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    for seconds in [3usize, 16] {
        let wav = mixed::wav(&speech::fixtures::voiced(seconds * 16_000));
        std::fs::write(&f.audio.path, &wav).unwrap();
        f.audio.metadata.byte_length = wav.len() as u64;
        assert_eq!(
            p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
            "english"
        );
    }
    let bodies = s.state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let short = String::from_utf8_lossy(&bodies[0]);
    assert!(
        short.contains("name=\"audio_ctx\"\r\n\r\n512"),
        "short English window is capped"
    );
    assert!(short.contains("name=\"no_timestamps\"\r\n\r\ntrue"));
    assert!(
        short.contains("name=\"temperature_inc\"\r\n\r\n0.5"),
        "fallback retries are bounded"
    );
    let long = String::from_utf8_lossy(&bodies[1]);
    assert!(
        long.contains("name=\"audio_ctx\"\r\n\r\n0"),
        "long batch input keeps full context"
    );
    assert!(long.contains("name=\"no_timestamps\"\r\n\r\nfalse"));
    let trace = crate::latency::snapshot(f.audio.metadata.id);
    assert_eq!(trace["details"]["audio_ctx"], 0);
    assert_eq!(trace["details"]["no_timestamps"], false);
    assert_eq!(trace["details"]["final_inference_calls"], 2);
    assert_eq!(trace["details"]["model"], "whisper-tiny");
    assert!(trace["details"].get("raw_text").is_none());
}

fn open_window_fixture(f: &mut Fixture) -> (PathBuf, Vec<u8>) {
    // One second of audio then 256 ms of digital silence: too short to close a
    // phrase, so a preview speculates the whole open window.
    let mut pcm: Vec<u8> = (0..16000)
        .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
        .collect();
    pcm.extend(vec![0; 8_192]);
    let path = f.dir.join("prepared.pcm");
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path: path.clone(),
        byte_length: pcm.len() as u64,
    });
    (path, pcm)
}

#[tokio::test]
async fn stop_after_a_pause_reuses_the_speculated_open_window_without_another_pass() {
    let s = healthy(r#"{"text":"open window"}"#).await;
    let mut f = Fixture::new();
    f.audio.metadata.id = Uuid::new_v4(); // own latency trace
    f.config.inference_url = s.url.clone();
    let (path, mut pcm) = open_window_fixture(&mut f);
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    assert_eq!(
        p.preview(&f.audio, &f.options).await.unwrap(),
        "open window"
    );
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    // Released after the pause: only quiet frames arrived after the window.
    pcm.extend(vec![0; 16_384]);
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        "open window"
    );
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        1,
        "a quiet tail needs no final pass"
    );
    let trace = crate::latency::snapshot(f.audio.metadata.id);
    let events: Vec<_> = trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["stage"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        events.contains(&"provisional_window_reused".to_owned()),
        "{events:?}"
    );
    assert!(!events.contains(&"final_inference_start".to_owned()));
    assert_eq!(trace["details"]["prepared_bytes"], pcm.len() as u64);
    assert_eq!(trace["details"]["committed_bytes_at_final"], 0);
    assert_eq!(trace["details"]["recognition_cache_hits"], 1);
    // Speech that resumes after the speculated window is decoded, never skipped.
    pcm.extend((0..8000).flat_map(|i| (if i % 2 == 0 { 9000i16 } else { -9000i16 }).to_le_bytes()));
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    assert_eq!(
        p.transcribe(&f.audio, &f.options).await.unwrap().raw_text,
        "open window open window"
    );
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(&path).unwrap(), pcm);
    f.retained();
}

#[tokio::test]
async fn stop_keeps_an_inflight_preview_whose_tail_is_quiet_and_reuses_it() {
    let s = server(
        StatusCode::OK,
        r#"{"status":"ok","writer_protocol":1,"warm":true}"#,
        StatusCode::OK,
        r#"{"text":"kept"}"#,
        Duration::from_millis(150),
    )
    .await;
    let mut f = Fixture::new();
    f.audio.metadata.id = Uuid::new_v4(); // own latency trace
    f.config.inference_url = s.url.clone();
    let (path, mut pcm) = open_window_fixture(&mut f);
    let p = Arc::new(LocalProvider::new(Some(f.config.clone())).unwrap());
    assert_eq!(p.readiness().await, Readiness::Ready);
    let preview = {
        let p = p.clone();
        let audio = f.audio.clone();
        let options = f.options.clone();
        tokio::spawn(async move { p.preview(&audio, &options).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while s.state.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // More audio arrived while the preview ran, but all of it is quiet.
    pcm.extend(vec![0; 16_384]);
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    let result = p.transcribe(&f.audio, &f.options).await.unwrap();
    preview.await.unwrap().unwrap();
    assert_eq!(result.raw_text, "kept");
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    let trace = crate::latency::snapshot(f.audio.metadata.id);
    assert_eq!(trace["details"]["preview_retention"], "quiet_tail");
    assert_eq!(trace["details"]["preview_tail_ms_at_stop"], 512);
    let events: Vec<_> = trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["stage"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        events.contains(&"matching_preview_retained".to_owned()),
        "{events:?}"
    );
    assert!(
        events.contains(&"provisional_window_reused".to_owned()),
        "{events:?}"
    );
}

#[tokio::test]
async fn stop_cancels_an_inflight_preview_when_speech_continued_after_it() {
    let s = server(
        StatusCode::OK,
        r#"{"status":"ok","writer_protocol":1,"warm":true}"#,
        StatusCode::OK,
        r#"{"text":"part"}"#,
        Duration::from_millis(150),
    )
    .await;
    let mut f = Fixture::new();
    f.audio.metadata.id = Uuid::new_v4(); // own latency trace
    f.config.inference_url = s.url.clone();
    let (path, mut pcm) = open_window_fixture(&mut f);
    let p = Arc::new(LocalProvider::new(Some(f.config.clone())).unwrap());
    assert_eq!(p.readiness().await, Readiness::Ready);
    let preview = {
        let p = p.clone();
        let audio = f.audio.clone();
        let options = f.options.clone();
        tokio::spawn(async move { p.preview(&audio, &options).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while s.state.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    pcm.extend((0..8000).flat_map(|i| (if i % 2 == 0 { 9000i16 } else { -9000i16 }).to_le_bytes()));
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    let result = p.transcribe(&f.audio, &f.options).await.unwrap();
    let _ = preview.await.unwrap();
    assert_eq!(result.raw_text, "part");
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        2,
        "voiced tail must be decoded"
    );
    let trace = crate::latency::snapshot(f.audio.metadata.id);
    assert_eq!(
        trace["details"]["preview_retention"],
        "tail_not_proven_quiet"
    );
    assert_eq!(trace["details"]["preview_tail_ms_at_stop"], 500);
    let events: Vec<_> = trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["stage"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !events.contains(&"matching_preview_retained".to_owned()),
        "{events:?}"
    );
}

#[tokio::test]
async fn a_final_pass_started_at_stop_is_reused_by_the_durable_job() {
    let s = server(
        StatusCode::OK,
        r#"{"status":"ok","writer_protocol":1,"warm":true}"#,
        StatusCode::OK,
        r#"{"text":"ahead"}"#,
        Duration::from_millis(150),
    )
    .await;
    let mut f = Fixture::new();
    f.audio.metadata.id = Uuid::new_v4(); // own latency trace
    f.config.inference_url = s.url.clone();
    open_window_fixture(&mut f);
    let p = Arc::new(LocalProvider::new(Some(f.config.clone())).unwrap());
    assert_eq!(p.readiness().await, Readiness::Ready);
    let ahead = {
        let p = p.clone();
        let audio = f.audio.clone();
        let options = f.options.clone();
        tokio::spawn(async move { p.transcribe(&audio, &options).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while s.state.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let job = p.transcribe(&f.audio, &f.options).await.unwrap();
    assert_eq!(ahead.await.unwrap().unwrap().raw_text, "ahead");
    assert_eq!(job.raw_text, "ahead");
    assert_eq!(
        s.state.calls.load(Ordering::SeqCst),
        1,
        "the job reuses the pass that started at Stop"
    );
    f.retained();
}

#[test]
fn worker_segment_lines_become_one_spoken_line() {
    // whisper-server ends every decoder segment with a newline, so one sentence
    // arrives as several lines. They are segment boundaries, not dictated ones.
    assert_eq!(join_segments(" and then I stop\n"), " and then I stop");
    assert_eq!(
        join_segments(" first part\n second part\n third\n"),
        " first part second part third"
    );
    assert_eq!(join_segments(" שלום\n עולם\n"), " שלום עולם");
    assert_eq!(join_segments("\n \n"), "");
    // The leading space survives: it separates this dictation from text already
    // in the field, which is what a second dictation relies on today.
    assert!(join_segments(" first part\n").starts_with(' '));
    // A worker that returns no word space still never runs two windows together.
    let mut text = String::from("fixture Hebrew");
    append_phrase(&mut text, "fixture English");
    assert_eq!(text, "fixture Hebrew fixture English");
    // A window that already starts with its own space contributes exactly one.
    append_phrase(&mut text, " and more");
    append_phrase(&mut text, "   ");
    assert_eq!(text, "fixture Hebrew fixture English and more");
}

#[test]
fn unavailable_errors_explain_the_observed_worker_state_without_echoing_logs() {
    use super::managed::WorkerState;
    let starting = worker_unavailable(&WorkerState::Starting {
        model: "test".into(),
        since_ms: 0,
    });
    assert!(starting.message.contains("still starting"));
    let handshake = worker_unavailable(&WorkerState::Failed {
        model: "test".into(),
        error: "worker_handshake_failed".into(),
        restarts: 0,
    });
    assert!(handshake.message.contains("authentication handshake"));
    assert!(handshake.message.contains("saved audio"));
    let timed_out = worker_unavailable(&WorkerState::Failed {
        model: "test".into(),
        error: "worker_start_timeout".into(),
        restarts: 0,
    });
    assert!(timed_out.message.contains("startup deadline"));
    let unknown = worker_unavailable(&WorkerState::Failed {
        model: "private model path".into(),
        error: "secret raw child output".into(),
        restarts: 0,
    });
    assert!(!unknown.message.contains("secret"));
    assert!(!unknown.message.contains("private model"));
    for result in [starting, handshake, timed_out, unknown] {
        assert!(result.retryable);
        assert!(matches!(result.code, ErrorCode::ProviderUnavailable));
    }
}

#[tokio::test]
async fn quiet_growing_previews_reuse_exact_speech_but_resumed_speech_decodes() {
    let s = healthy(r#"{"text":"שלום hello"}"#).await;
    let mut f = Fixture::new();
    f.audio.metadata.id = Uuid::new_v4();
    f.config.inference_url = s.url.clone();
    let (path, mut pcm) = open_window_fixture(&mut f);
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    p.preview(&f.audio, &f.options).await.unwrap();
    for _ in 0..4 {
        pcm.extend(vec![0; 16_000]);
        std::fs::write(&path, &pcm).unwrap();
        f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
        assert_eq!(p.preview(&f.audio, &f.options).await.unwrap(), "שלום hello");
    }
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 1);
    // A reused preview never advances its committed offset through the pause.
    pcm.extend(speech::fixtures::voiced(8_000));
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    p.preview(&f.audio, &f.options).await.unwrap();
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 2);
    p.transcribe(&f.audio, &f.options).await.unwrap();
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 2);
    let trace = crate::latency::snapshot(f.audio.metadata.id);
    assert_eq!(trace["details"]["preview_inference_calls"], 2);
    assert!(trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["stage"] == "provisional_window_reused"));
    assert!(!trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["stage"] == "final_inference_start"));
}

#[tokio::test]
async fn same_length_replaced_audio_is_never_reused_as_a_quiet_provisional_window() {
    let s = healthy(r#"{"text":"phrase"}"#).await;
    let mut f = Fixture::new();
    f.audio.metadata.id = Uuid::new_v4();
    f.config.inference_url = s.url.clone();
    let mut pcm = speech::fixtures::voiced(2 * 16_000);
    let path = f.dir.join("prepared.pcm");
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared = Some(crate::providers::PreparedAudio {
        path: path.clone(),
        byte_length: pcm.len() as u64,
    });
    let p = LocalProvider::new(Some(f.config.clone())).unwrap();
    p.preview(&f.audio, &f.options).await.unwrap();
    for sample in pcm.as_chunks_mut::<2>().0 {
        *sample = (i16::from_le_bytes(*sample) / 2).to_le_bytes();
    }
    pcm.extend(speech::fixtures::room(8_000));
    std::fs::write(&path, &pcm).unwrap();
    f.audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
    p.transcribe(&f.audio, &f.options).await.unwrap();
    assert_eq!(s.state.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn inflight_preview_retention_requires_the_same_options_and_exact_audio() {
    for changed_audio in [false, true] {
        let s = server(
            StatusCode::OK,
            r#"{"status":"ok","writer_protocol":1,"warm":true}"#,
            StatusCode::OK,
            r#"{"text":"phrase"}"#,
            Duration::from_millis(150),
        )
        .await;
        let mut f = Fixture::new();
        f.audio.metadata.id = Uuid::new_v4();
        f.config.inference_url = s.url.clone();
        let mut pcm = speech::fixtures::voiced(2 * 16_000);
        let path = f.dir.join("prepared.pcm");
        std::fs::write(&path, &pcm).unwrap();
        f.audio.prepared = Some(crate::providers::PreparedAudio {
            path: path.clone(),
            byte_length: pcm.len() as u64,
        });
        let p = Arc::new(LocalProvider::new(Some(f.config.clone())).unwrap());
        p.readiness().await;
        let preview = {
            let p = p.clone();
            let audio = f.audio.clone();
            let options = f.options.clone();
            tokio::spawn(async move { p.preview(&audio, &options).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while s.state.calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if changed_audio {
            for sample in pcm.as_chunks_mut::<2>().0 {
                *sample = (i16::from_le_bytes(*sample) / 2).to_le_bytes();
            }
            std::fs::write(&path, &pcm).unwrap();
        } else {
            f.options
                .vocabulary
                .hints
                .push("changed recognition context".into());
        }
        p.transcribe(&f.audio, &f.options).await.unwrap();
        let _ = preview.await.unwrap();
        assert_eq!(s.state.calls.load(Ordering::SeqCst), 2);
        let trace = crate::latency::snapshot(f.audio.metadata.id);
        assert!(!trace["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["stage"] == "matching_preview_retained"));
    }
}
