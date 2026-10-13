//! Headless lifecycle tests: fixture worker/ffmpeg scripts and an in-process download
//! origin stand in for whisper-server.exe, ffmpeg and Hugging Face. No GPU is used here.
use super::*;
use crate::local_stt::{
    catalog::{ModelSpec, MODELS},
    managed::{read_registry, WorkerState},
    LocalProvider,
};
use crate::providers::{SavedAudio, TranscriptionProvider};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use sha2::Digest;
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;
use uuid::Uuid;

const WORKER: &str = r#"#!/usr/bin/python3
import sys, os, json, http.server, socketserver
args = sys.argv[1:]
capability = sys.stdin.readline().strip()
instance = sys.stdin.readline().strip()
assert len(capability) == 64 and len(instance) == 64
def opt(flag):
    return args[args.index(flag) + 1]
model, port, lang = opt('-m'), int(opt('--port')), opt('-l')
here = os.path.dirname(os.path.abspath(__file__))
print('fake-private-content-' + 'x' * 70000, file=sys.stderr)
print('register_device: registered device Vulkan0 (fake-private-content)', file=sys.stderr)
open(os.path.join(here, 'last-threads'), 'w').write(opt('-t'))
mode = open(os.path.join(here, 'mode')).read().strip() if os.path.exists(os.path.join(here, 'mode')) else 'gpu'
if mode == 'exit':
    print('error: failed to load model', file=sys.stderr); sys.exit(1)
if '--no-gpu' in args:
    pass
elif mode == 'nogpu':
    print('ggml_vulkan: Found 0 Vulkan devices', file=sys.stderr)
else:
    print('ggml_vulkan: Found 1 Vulkan devices:', file=sys.stderr)
    print('register_device: registered device Vulkan0 (Fixture  GPU)', file=sys.stderr)
print("whisper_init_from_file_with_params_no_state: loading model from '%s'" % model, file=sys.stderr)
if '--no-gpu' in args:
    print('whisper_init_with_params_no_state: use gpu    = 0', file=sys.stderr)
elif mode == 'gpu':
    print('whisper_backend_init_gpu: using Vulkan0 backend', file=sys.stderr)
sys.stderr.flush()
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, status, body):
        self.send_response(status); self.send_header('Content-Type', 'application/json')
        self.send_header('X-Writer-Instance', instance if mode != 'imposter' else 'wrong-instance')
        self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
    def authorized(self):
        if 'Origin' in self.headers or 'Sec-Fetch-Site' in self.headers:
            self.reply(403, b'{}'); return False
        if self.headers.get('Authorization') != 'Bearer ' + capability:
            self.reply(401, b'{}'); return False
        return True
    def do_GET(self):
        if not self.authorized(): return
        self.reply(200 if self.path == '/health' else 404, b'{"status":"ok","writer_protocol":1,"warm":true}' if self.path == '/health' else b'{}')
    def do_POST(self):
        if not self.authorized(): return
        self.rfile.read(int(self.headers.get('Content-Length', '0')))
        if self.path == '/crash': os._exit(3)
        if os.path.exists(os.path.join(here, 'inference-fail')):
            self.reply(500, b'{}'); return
        self.reply(200, json.dumps({"text": "שלום managed " + lang}).encode())
socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(('127.0.0.1', port), H) as server:
    if mode != 'legacy':
        print('writer: private listener %d' % server.server_address[1], file=sys.stderr, flush=True)
    server.serve_forever()
"#;
const FFMPEG: &str = r#"#!/usr/bin/python3
import sys
if sys.argv[1:] == ['-version']:
    print('fixture ffmpeg'); sys.exit(0)
sys.stdout.buffer.write(b'\x00\x10' * 1600 if 's16le' in sys.argv else b'RIFF\x04\x00\x00\x00WAVEfixture')
"#;

struct Fixture {
    root: PathBuf,
    layout: Layout,
    weights: Vec<u8>,
    sha256: String,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("writer setup בדיקה {}", Uuid::new_v4()));
        let runtime = root.join("runtime");
        crate::helper_security::protect_private_dir(&root).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        for (name, body) in [("whisper-server", WORKER), ("ffmpeg", FFMPEG)] {
            let path = runtime.join(name);
            std::fs::write(&path, crate::test_support::python_fixture(body)).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::write(
            runtime.join(crate::local_stt::managed::VULKAN_BACKEND_LIBRARY),
            b"fixture backend library",
        )
        .unwrap();
        let weights: Vec<u8> = (0..3_000_000_u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let sha256 = format!("{:x}", sha2::Sha256::digest(&weights));
        let layout = Layout {
            mode: Mode::Desktop,
            data_dir: root.join("Data"),
            state_dir: root.join("State"),
            models_dir: root.join("Models"),
            runtime_dir: Some(runtime.clone()),
            ffmpeg: Some(runtime.join("ffmpeg")),
            startup_timeout: Duration::from_secs(20),
        };
        Self {
            root,
            layout,
            weights,
            sha256,
        }
    }
    fn mode(&self, mode: &str) {
        std::fs::write(self.root.join("runtime/mode"), mode).unwrap();
    }
    /// Real catalog with fixture identities for ivrit-turbo (`origin` serves the bytes).
    fn catalog(&self, origin: &str) -> &'static [ModelSpec] {
        let models: Vec<ModelSpec> = MODELS
            .iter()
            .map(|spec| {
                if spec.id == "ivrit-turbo" || spec.id == "whisper-tiny" {
                    ModelSpec {
                        source_url: Box::leak(format!("{origin}/model.bin").into_boxed_str()),
                        bytes: self.weights.len() as u64,
                        sha256: Box::leak(self.sha256.clone().into_boxed_str()),
                        ..*spec
                    }
                } else {
                    *spec
                }
            })
            .collect();
        Box::leak(models.into_boxed_slice())
    }
    fn open(&self, origin: &str, credentials: Arc<Credentials>) -> Arc<Setup> {
        Setup::open_with(self.layout.clone(), credentials, self.catalog(origin)).unwrap()
    }
    fn audio(&self) -> SavedAudio {
        let path = self.root.join("saved audio.wav");
        std::fs::write(&path, b"retained audio").unwrap();
        let catalog: crate::transcription::ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        let mut metadata = catalog.recording.data.metadata;
        metadata.byte_length = 14;
        metadata.mime_type = "audio/wav".into();
        SavedAudio {
            metadata,
            path,
            prepared: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Minimal HTTPS-free origin (the runtime only enforces https for real catalog URLs, so
/// tests bypass the scheme check through a loopback `http` URL rewritten below).
struct Origin {
    url: String,
    ranges: Arc<Mutex<Vec<String>>>,
    cut_after: Arc<Mutex<Option<usize>>>,
    delay_ms: Arc<Mutex<u64>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Origin {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn origin(bytes: Vec<u8>) -> Origin {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let cut_after = Arc::new(Mutex::new(None));
    let delay_ms = Arc::new(Mutex::new(0));
    let bytes = Arc::new(bytes);
    let (r, c, d) = (ranges.clone(), cut_after.clone(), delay_ms.clone());
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let (bytes, r, c, d) = (bytes.clone(), r.clone(), c.clone(), d.clone());
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0_u8];
                while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
                    if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let range = head
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("range")
                            .then(|| value.trim().strip_prefix("bytes="))
                            .flatten()
                    })
                    .map(|v| v.trim_end_matches('-').to_owned());
                r.lock().unwrap().push(range.clone().unwrap_or_default());
                let start: usize = range.and_then(|v| v.parse().ok()).unwrap_or(0);
                let total = bytes.len();
                let header = if start > 0 {
                    format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n", total - start, start, total - 1, total)
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
                    )
                };
                if socket.write_all(header.as_bytes()).await.is_err() {
                    return;
                }
                let cut = *c.lock().unwrap();
                let delay = *d.lock().unwrap();
                let mut offset = start;
                while offset < total {
                    if let Some(cut) = cut {
                        if offset >= cut {
                            return; // Connection dropped mid-body.
                        }
                    }
                    let end = (offset + 65536).min(total);
                    if socket.write_all(&bytes[offset..end]).await.is_err() {
                        return;
                    }
                    offset = end;
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    Origin {
        url,
        ranges,
        cut_after,
        delay_ms,
        task,
    }
}

fn verifier() -> Verifier {
    Arc::new(|_: &ProviderId| false)
}
async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}
fn settings(model: &str, language: &str) -> serde_json::Value {
    serde_json::json!({ "api_version": "1", "data": { "ui_language": "en", "transcription": {
        "selection": { "provider": "local", "model": model }, "input_language": language,
        "output_language": { "mode": "preserve" }, "vocabulary": { "hints": ["x"], "corrections": [], "snippets": [] },
        "cleanup": { "mode": "off" } } } })
}
fn weights_of(view: &serde_json::Value, id: &str) -> serde_json::Value {
    view["data"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap()["weights"]
        .clone()
}
#[tokio::test]
async fn native_import_preserves_source_and_serializes_with_capture_and_deletion() {
    let f = Fixture::new();
    let setup = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    let source = f.root.join("user imported weights.bin");
    std::fs::write(&source, &f.weights).unwrap();
    let capture = setup.runtime.capture_pin();
    assert!(setup
        .runtime
        .import_file("whisper-tiny", source.clone())
        .await
        .is_err());
    assert!(setup.runtime.delete_model("whisper-tiny").await.is_err());
    drop(capture);
    setup
        .runtime
        .import_file("whisper-tiny", source.clone())
        .await
        .unwrap();
    assert!(setup.runtime.installed("whisper-tiny").is_some());
    assert!(setup
        .runtime
        .import_file("whisper-tiny", source.clone())
        .await
        .is_err());
    setup.runtime.delete_model("whisper-tiny").await.unwrap();
    assert_eq!(std::fs::read(source).unwrap(), f.weights);
    assert!(setup.runtime.installed("whisper-tiny").is_none());
}
async fn wait_for(
    app: &Router,
    timeout: Duration,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let (_, view) = call(app, "GET", "/api/v1/setup", None).await;
        if predicate(&view) {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting; last view: {view}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn download_verify_load_transcribe_recover_and_reuse_after_restart() {
    let f = Fixture::new();
    let o = origin(f.weights.clone()).await;
    let setup = f.open(&o.url, Arc::new(Credentials::new(&f.layout.state_dir)));
    let app = routes_with(setup.clone(), verifier());
    let legacy = f.layout.state_dir.join("worker/startup.log");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, b"fake-private-content from a legacy worker").unwrap();
    let (status, view) = call(&app, "GET", "/api/v1/setup", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["data"]["first_run"], true);
    assert_eq!(view["data"]["runtime"]["worker"]["state"], "idle");
    assert_eq!(view["data"]["mode"], "desktop");
    let (status, saved) = call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["data"]["transcription"]["vocabulary"]["hints"],
        serde_json::json!([])
    );
    let view = wait_for(&app, Duration::from_secs(5), |v| {
        v["data"]["runtime"]["worker"]["state"] == "missing_weights"
    })
    .await;
    assert_eq!(view["data"]["first_run"], false);
    assert_eq!(weights_of(&view, "ivrit-turbo")["state"], "not_installed");
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/download",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view = wait_for(&app, Duration::from_secs(30), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "installed"
    })
    .await;
    assert_eq!(weights_of(&view, "ivrit-turbo")["sha256"], f.sha256);
    assert_eq!(weights_of(&view, "ivrit-turbo")["origin"], "download");
    assert_eq!(o.ranges.lock().unwrap().as_slice(), [""]);
    let registry = read_registry(&f.layout.state_dir).await.unwrap();
    assert_eq!(registry["ivrit-turbo"].bytes, f.weights.len() as u64);
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    assert!(!legacy.exists(), "legacy raw diagnostics must be removed");
    let diagnostics = setup.runtime.startup_log();
    assert!(!diagnostics.is_empty());
    assert!(diagnostics.iter().all(|code| [
        "cpu_backend_observed",
        "gpu_backend_observed",
        "gpu_unavailable",
        "expected_model_observed"
    ]
    .contains(&code.as_str())));
    assert!(!view.to_string().contains("fake-private-content"));
    let worker = &view["data"]["runtime"]["worker"];
    assert_eq!(
        worker["device"],
        format!("{} GPU", crate::local_stt::managed::GPU_BACKEND)
    );
    assert_eq!(worker["backend"], crate::local_stt::managed::GPU_BACKEND);
    assert_eq!(
        std::fs::read_to_string(f.root.join("runtime/last-threads")).unwrap(),
        "1"
    );
    assert_eq!(worker["model"], "ivrit-turbo");
    assert!(worker.get("port").is_none() && worker.get("process_id").is_none());
    assert!(
        !view
            .to_string()
            .contains(&f.root.to_string_lossy().to_string()),
        "no paths in the desktop view"
    );

    let provider = LocalProvider::managed(setup.runtime.clone());
    let audio = f.audio();
    let mut options = crate::desktop_settings::default_settings().transcription;
    options.selection.model = "ivrit-turbo".into();
    let transcript = provider.transcribe(&audio, &options).await.unwrap();
    assert_eq!(transcript.raw_text, "שלום managed he");
    assert!(provider.info().available && provider.info().configured);
    options.selection.model = "whisper-large-v3-turbo".into();
    let error = provider.transcribe(&audio, &options).await.unwrap_err();
    assert!(matches!(error.code, ErrorCode::ProviderUnavailable) && error.retryable);
    options.selection.model = "ivrit-turbo".into();

    let old_worker = setup.runtime.loaded().unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let health = format!("http://127.0.0.1:{}/health", old_worker.port);
    assert_eq!(
        client.get(&health).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        old_worker
            .auth
            .authorize(client.get(&health))
            .header("Origin", "null")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    // Worker crash: recovery restarts the same verified weights; saved audio is retried.
    let WorkerState::Ready { process_id, .. } = setup.runtime.worker() else {
        panic!()
    };
    std::process::Command::new("kill")
        .arg("-9")
        .arg(process_id.to_string())
        .status()
        .unwrap();
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
            && v["data"]["runtime"]["worker"]["restarts"] == 1
    })
    .await;
    assert_eq!(view["data"]["runtime"]["worker"]["restarts"], 1);
    let new_worker = setup.runtime.loaded().unwrap();
    let health = format!("http://127.0.0.1:{}/health", new_worker.port);
    assert_eq!(
        old_worker
            .auth
            .authorize(client.get(&health))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(new_worker.auth.matches(
        &new_worker
            .auth
            .authorize(client.get(&health))
            .send()
            .await
            .unwrap()
    ));

    let transcript = provider.transcribe(&audio, &options).await.unwrap();
    assert_eq!(transcript.raw_text, "שלום managed he");
    assert_eq!(std::fs::read(&audio.path).unwrap(), b"retained audio");

    let (status, view) = call(&app, "POST", "/api/v1/setup/runtime/stop", None).await;
    assert_eq!(status, StatusCode::OK);
    let _ = view;
    wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "idle"
    })
    .await;
    assert!(setup.runtime.loaded().is_none());
    call(&app, "POST", "/api/v1/setup/runtime/retry", None).await;
    wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    drop(app);
    let _ = setup.runtime.select(None);
    wait_for(
        &routes_with(setup.clone(), verifier()),
        Duration::from_secs(10),
        |v| v["data"]["runtime"]["worker"]["state"] == "idle",
    )
    .await;
    drop(setup);

    // Relaunch: saved selection and verified registry are reused without re-downloading.
    let setup = f.open(&o.url, Arc::new(Credentials::new(&f.layout.state_dir)));
    let app = routes_with(setup.clone(), verifier());
    let (_, view) = call(&app, "GET", "/api/v1/settings", None).await;
    assert_eq!(
        view["data"]["transcription"]["selection"]["model"],
        "ivrit-turbo"
    );
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    assert_eq!(view["data"]["first_run"], false);
    assert_eq!(weights_of(&view, "ivrit-turbo")["origin"], "download");
    assert_eq!(o.ranges.lock().unwrap().len(), 1, "no second download");
    let _ = setup.runtime.select(None);
    wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "idle"
    })
    .await;
}

#[tokio::test]
async fn interrupted_download_resumes_with_range_and_cancel_keeps_partial() {
    let f = Fixture::new();
    let o = origin(f.weights.clone()).await;
    *o.cut_after.lock().unwrap() = Some(1_000_000);
    let setup = f.open(&o.url, Arc::new(Credentials::new(&f.layout.state_dir)));
    let app = routes_with(setup.clone(), verifier());
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/download",
        None,
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(30), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "failed"
    })
    .await;
    let weights = weights_of(&view, "ivrit-turbo");
    assert_eq!(weights["error"], "download_interrupted");
    let resumable = weights["resumable_bytes"].as_u64().unwrap();
    assert!((1_000_000..=1_048_576).contains(&resumable), "{resumable}");
    *o.cut_after.lock().unwrap() = None;
    *o.delay_ms.lock().unwrap() = 40;
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/download",
        None,
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(10), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "downloading"
            && weights_of(v, "ivrit-turbo")["downloaded_bytes"]
                .as_u64()
                .unwrap()
                > resumable
    })
    .await;
    assert_eq!(
        weights_of(&view, "ivrit-turbo")["total_bytes"],
        f.weights.len() as u64
    );
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/cancel",
        None,
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(10), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "failed"
    })
    .await;
    let weights = weights_of(&view, "ivrit-turbo");
    assert_eq!(weights["error"], "download_canceled");
    let kept = weights["resumable_bytes"].as_u64().unwrap();
    assert!(kept > resumable && kept < f.weights.len() as u64);
    assert!(f.layout.models_dir.join("ivrit-turbo.bin.part").is_file());
    *o.delay_ms.lock().unwrap() = 0;
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/download",
        None,
    )
    .await;
    wait_for(&app, Duration::from_secs(30), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "installed"
    })
    .await;
    let ranges = o.ranges.lock().unwrap().clone();
    assert_eq!(ranges[0], "");
    assert_eq!(ranges[1], resumable.to_string());
    assert_eq!(ranges[2], kept.to_string());
    assert!(!f.layout.models_dir.join("ivrit-turbo.bin.part").exists());
    assert_eq!(
        std::fs::read(f.layout.models_dir.join("ivrit-turbo.bin")).unwrap(),
        f.weights
    );
}

#[tokio::test]
async fn corrupt_or_mismatched_weights_are_never_installed_or_loaded() {
    let f = Fixture::new();
    let mut wrong = f.weights.clone();
    wrong[1234] ^= 0xff;
    let o = origin(wrong.clone()).await;
    let setup = f.open(&o.url, Arc::new(Credentials::new(&f.layout.state_dir)));
    let app = routes_with(setup.clone(), verifier());
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/download",
        None,
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(30), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "failed"
    })
    .await;
    assert_eq!(
        weights_of(&view, "ivrit-turbo")["error"],
        "checksum_mismatch"
    );
    assert!(!f.layout.models_dir.join("ivrit-turbo.bin").exists());
    assert_eq!(
        view["data"]["runtime"]["worker"]["state"],
        "missing_weights"
    );
    // Imported file of the wrong size at the fixed path.
    std::fs::write(f.layout.models_dir.join("ivrit-turbo.bin"), b"short").unwrap();
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/verify",
        None,
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(10), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "failed"
            && weights_of(v, "ivrit-turbo")["error"] == "size_mismatch"
    })
    .await;
    assert_eq!(
        view["data"]["runtime"]["worker"]["state"],
        "missing_weights"
    );
    // Correct import at the fixed path verifies and loads.
    std::fs::write(f.layout.models_dir.join("ivrit-turbo.bin"), &f.weights).unwrap();
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/verify",
        None,
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    assert_eq!(weights_of(&view, "ivrit-turbo")["origin"], "import");
    assert!(read_registry(&f.layout.state_dir)
        .await
        .unwrap()
        .contains_key("ivrit-turbo"));
    let _ = setup.runtime.select(None);
    wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "idle"
    })
    .await;
}

#[tokio::test]
async fn gpu_failure_without_cpu_package_is_explicit_and_retryable() {
    let f = Fixture::new();
    std::fs::write(f.layout.models_dir.join("ivrit-turbo.bin"), &f.weights).ok();
    std::fs::create_dir_all(&f.layout.models_dir).unwrap();
    std::fs::write(f.layout.models_dir.join("ivrit-turbo.bin"), &f.weights).unwrap();
    let setup = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    let app = routes_with(setup.clone(), verifier());
    call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/verify",
        None,
    )
    .await;
    wait_for(&app, Duration::from_secs(10), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "installed"
    })
    .await;
    for (mode, error) in [
        ("cpu", "gpu_backend_not_used"),
        ("nogpu", "gpu_unavailable"),
        ("exit", "worker_exited_during_startup"),
    ] {
        f.mode(mode);
        call(
            &app,
            "PUT",
            "/api/v1/settings",
            Some(settings("ivrit-turbo", "he")),
        )
        .await;
        call(&app, "POST", "/api/v1/setup/runtime/retry", None).await;
        let view = wait_for(&app, Duration::from_secs(30), |v| {
            v["data"]["runtime"]["worker"]["state"] == "failed"
                && v["data"]["runtime"]["worker"]["restarts"] == 3
        })
        .await;
        assert_eq!(view["data"]["runtime"]["worker"]["error"], error, "{mode}");
        let provider = LocalProvider::managed(setup.runtime.clone());
        assert!(!provider.info().available);
        assert!(matches!(
            provider.readiness().await,
            crate::local_stt::Readiness::Unreachable
        ));
        let mut options = crate::desktop_settings::default_settings().transcription;
        options.selection.model = "ivrit-turbo".into();
        let failure = provider.transcribe(&f.audio(), &options).await.unwrap_err();
        assert!(matches!(failure.code, ErrorCode::ProviderUnavailable) && failure.retryable);
        assert!(!setup.runtime.startup_log().is_empty() || mode == "exit");
        let _ = setup.runtime.select(None);
        wait_for(&app, Duration::from_secs(10), |v| {
            v["data"]["runtime"]["worker"]["state"] == "idle"
        })
        .await;
    }
    // Runtime problems are explicit states, not silent CPU or cloud substitutes.
    std::fs::remove_file(f.root.join("runtime/ffmpeg")).unwrap();
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "missing_runtime"
    })
    .await;
    assert_eq!(
        view["data"]["runtime"]["worker"]["detail"],
        "ffmpeg_missing"
    );
    assert_eq!(view["data"]["runtime"]["ffmpeg_present"], false);
    let _ = setup.runtime.select(None);
    let mut layout = f.layout.clone();
    layout.runtime_dir = None;
    let unpackaged = Setup::open_with(
        layout,
        Arc::new(Credentials::new(&f.layout.state_dir)),
        f.catalog("http://127.0.0.1:1"),
    )
    .unwrap();
    let app = routes_with(unpackaged.clone(), verifier());
    let view = wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "missing_runtime"
    })
    .await;
    assert_eq!(
        view["data"]["runtime"]["worker"]["detail"],
        "runtime_not_packaged"
    );
    let _ = unpackaged.runtime.select(None);
}

#[tokio::test]
async fn hebrew_only_weights_never_receive_english_or_mixed_and_invalid_documents_fail_closed() {
    let f = Fixture::new();
    let setup = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    let app = routes_with(setup.clone(), verifier());
    for language in ["en", "mixed"] {
        let (status, body) = call(
            &app,
            "PUT",
            "/api/v1/settings",
            Some(settings("ivrit-turbo", language)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{language}");
        assert_eq!(body["error"]["code"], "unsupported_capability");
        assert!(!f.layout.state_dir.join("settings.json").exists());
        assert!(setup.first_run());
    }
    for language in ["en", "he", "mixed"] {
        let (status, _) = call(
            &app,
            "PUT",
            "/api/v1/settings",
            Some(settings("whisper-large-v3-turbo", language)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{language}");
    }
    let (status, body) = call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(serde_json::json!({ "api_version": "1", "data": { "ui_language": "en" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("not-a-model", "he")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(&app, "POST", "/api/v1/setup/models/..%2Fetc/download", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/delete-everything",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, view) = call(&app, "GET", "/api/v1/settings", None).await;
    assert_eq!(view["data"]["transcription"]["input_language"], "mixed");
    assert_eq!(
        view["data"]["transcription"]["selection"]["model"],
        "whisper-large-v3-turbo"
    );
    let _ = setup.runtime.select(None);
}

struct SecretSource;
impl credentials::CredentialSource for SecretSource {
    fn read(&self, provider: &ProviderId) -> Option<String> {
        matches!(provider, ProviderId::Gemini).then(|| "AIza-fixture-secret-value".to_owned())
    }
}

#[tokio::test]
async fn credentials_are_reported_by_presence_only_and_never_written_or_echoed() {
    let f = Fixture::new();
    let credentials = Arc::new(Credentials::with_source(
        &f.layout.state_dir,
        Arc::new(SecretSource),
    ));
    let setup = f.open("http://127.0.0.1:1", credentials.clone());
    let verified: Verifier = Arc::new(|p: &ProviderId| matches!(p, ProviderId::Gemini));
    let app = routes_with(setup.clone(), verified);
    let (status, _) = call(&app, "PUT", "/api/v1/settings", Some(serde_json::json!({ "api_version": "1", "data": { "ui_language": "he", "transcription": {
        "selection": { "provider": "gemini", "model": "gemini-3.8-flash" }, "input_language": "mixed", "output_language": { "mode": "preserve" },
        "vocabulary": { "hints": [], "corrections": [], "snippets": [] }, "cleanup": { "mode": "off" } } } }))).await;
    assert_eq!(status, StatusCode::OK);
    let view = wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["settings"]["ui_language"] == "he"
    })
    .await;
    let cloud = view["data"]["cloud"].as_array().unwrap();
    let gemini = cloud.iter().find(|c| c["provider"] == "gemini").unwrap();
    let openai = cloud.iter().find(|c| c["provider"] == "openai").unwrap();
    assert_eq!(gemini["credential"], "protected_store");
    assert_eq!(gemini["verified"], true);
    assert_eq!(gemini["selected"], true);
    assert_eq!(openai["credential"], "absent");
    assert_eq!(openai["verified"], false);
    assert_eq!(
        view["data"]["runtime"]["worker"]["state"], "idle",
        "cloud selection loads no local worker"
    );
    let rendered = view.to_string();
    assert!(!rendered.contains("fixture-secret"));
    for file in ["settings.json", "models.json"] {
        if let Ok(bytes) = std::fs::read(f.layout.state_dir.join(file)) {
            assert!(
                !String::from_utf8_lossy(&bytes).contains("fixture-secret"),
                "{file}"
            );
        }
    }
    let adapters = crate::providers::cloud::adapters_with(credentials.clone());
    let gemini = adapters
        .iter()
        .find(|a| matches!(a.info().id, ProviderId::Gemini))
        .unwrap();
    assert!(gemini.info().configured && !gemini.info().available);
    let openai = adapters
        .iter()
        .find(|a| matches!(a.info().id, ProviderId::Openai))
        .unwrap();
    assert!(!openai.info().configured);
}

#[tokio::test]
async fn switching_between_installed_models_replaces_the_worker_without_getting_stuck() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.layout.models_dir).unwrap();
    for name in ["ivrit-turbo.bin", "ggml-tiny.bin"] {
        std::fs::write(f.layout.models_dir.join(name), &f.weights).unwrap();
    }
    let setup = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    let app = routes_with(setup.clone(), verifier());
    for id in ["ivrit-turbo", "whisper-tiny"] {
        call(
            &app,
            "POST",
            &format!("/api/v1/setup/models/{id}/verify"),
            None,
        )
        .await;
        wait_for(&app, Duration::from_secs(10), |v| {
            weights_of(v, id)["state"] == "installed"
        })
        .await;
    }
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
            && v["data"]["runtime"]["worker"]["model"] == "ivrit-turbo"
    })
    .await;
    let WorkerState::Ready {
        process_id: first, ..
    } = setup.runtime.worker()
    else {
        panic!()
    };
    // Switch while loaded: the old worker stops, the new one starts, nothing stays "starting".
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("whisper-tiny", "mixed")),
    )
    .await;
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
            && v["data"]["runtime"]["worker"]["model"] == "whisper-tiny"
    })
    .await;
    let WorkerState::Ready {
        process_id: second, ..
    } = setup.runtime.worker()
    else {
        panic!()
    };
    assert_ne!(first, second);
    assert_eq!(
        view["data"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["loaded"] == true)
            .count(),
        1
    );
    let provider = LocalProvider::managed(setup.runtime.clone());
    let mut options = crate::desktop_settings::default_settings().transcription;
    options.selection.model = "whisper-tiny".into();
    options.input_language = crate::transcription::InputLanguage::Mixed;
    assert_eq!(
        provider
            .transcribe(&f.audio(), &options)
            .await
            .unwrap()
            .raw_text,
        "שלום managed auto"
    );
    // Rapid back-and-forth converges on the last saved choice.
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("whisper-tiny", "en")),
    )
    .await;
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    wait_for(&app, Duration::from_secs(30), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
            && v["data"]["runtime"]["worker"]["model"] == "ivrit-turbo"
    })
    .await;
    let _ = setup.runtime.select(None);
    wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "idle"
    })
    .await;
    assert!(
        std::process::Command::new("kill")
            .arg("-0")
            .arg(second.to_string())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true),
        "replaced worker exited"
    );
}

fn cpu_package(f: &Fixture) {
    let cpu = f.root.join("runtime/cpu");
    std::fs::create_dir_all(&cpu).unwrap();
    std::fs::copy(
        f.root.join("runtime/whisper-server"),
        cpu.join("whisper-server"),
    )
    .unwrap();
}

async fn installed_fixture(f: &Fixture) -> (Arc<Setup>, Router) {
    let setup = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    std::fs::write(f.layout.models_dir.join("ivrit-turbo.bin"), &f.weights).unwrap();
    let app = routes_with(setup.clone(), verifier());
    setup.runtime.verify("ivrit-turbo").unwrap();
    wait_for(&app, Duration::from_secs(10), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "installed"
    })
    .await;
    (setup, app)
}

#[tokio::test]
async fn automatic_fallback_keeps_identity_cpu_persists_and_explicit_gpu_fails_closed() {
    let f = Fixture::new();
    cpu_package(&f);
    f.mode("nogpu");
    let (setup, app) = installed_fixture(&f).await;
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    assert_eq!(view["data"]["runtime"]["worker"]["backend"], "cpu");
    assert_eq!(view["data"]["runtime"]["worker"]["model"], "ivrit-turbo");
    assert_eq!(
        view["data"]["runtime"]["execution"]["fallback_reason"],
        "gpu_unavailable"
    );
    let provider = LocalProvider::managed(setup.runtime.clone());
    let audio = f.audio();
    let mut options = default_settings().transcription;
    options.selection.model = "ivrit-turbo".into();
    assert_eq!(
        provider
            .transcribe(&audio, &options)
            .await
            .unwrap()
            .raw_text,
        "שלום managed he"
    );
    // No automatic fallback when the user explicitly requires GPU.
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/setup/execution",
        Some(serde_json::json!({"api_version":"1","data":"gpu"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "failed"
            && v["data"]["runtime"]["worker"]["restarts"] == 3
    })
    .await;
    assert_eq!(
        view["data"]["runtime"]["execution"]["fallback_reason"],
        serde_json::Value::Null
    );
    setup
        .runtime
        .save_backend(ExecutionBackend::Cpu)
        .await
        .unwrap();
    wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    setup.runtime.select(None).unwrap();
    wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "idle"
    })
    .await;
    drop(provider);
    drop(app);
    drop(setup);
    let reopened = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    assert_eq!(
        reopened.runtime.execution().requested,
        ExecutionBackend::Cpu
    );
    let app = routes_with(reopened.clone(), verifier());
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    assert_eq!(view["data"]["runtime"]["worker"]["backend"], "cpu");
    assert_eq!(std::fs::read(audio.path).unwrap(), b"retained audio");
    reopened.runtime.select(None).unwrap();
}

#[tokio::test]
async fn cpu_only_runtime_starts_without_a_vulkan_library() {
    let f = Fixture::new();
    std::fs::remove_file(
        f.root
            .join("runtime")
            .join(crate::local_stt::managed::VULKAN_BACKEND_LIBRARY),
    )
    .unwrap();
    let (setup, app) = installed_fixture(&f).await;
    setup.runtime.select(Some("ivrit-turbo")).unwrap();
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    assert_eq!(view["data"]["runtime"]["worker"]["backend"], "cpu");
    let expected = std::thread::available_parallelism()
        .map(|n| n.get().min(16))
        .unwrap_or(4)
        .to_string();
    assert_eq!(
        std::fs::read_to_string(f.root.join("runtime/last-threads")).unwrap(),
        expected
    );
    assert_eq!(view["data"]["runtime"]["execution"]["gpu_available"], false);
    setup.runtime.select(None).unwrap();
}

#[tokio::test]
async fn deletion_unloads_removes_registry_and_partials_retains_sources_and_reinstalls() {
    let f = Fixture::new();
    let (setup, app) = installed_fixture(&f).await;
    let imported = f.root.join("original-import.bin");
    std::fs::write(&imported, &f.weights).unwrap();
    let audio = f.audio();
    call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("ivrit-turbo", "he")),
    )
    .await;
    wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    let partial = f.layout.models_dir.join("ivrit-turbo.bin.part");
    std::fs::write(&partial, b"partial").unwrap();
    let lease = setup.runtime.inference_lease().unwrap();
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/delete",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(setup
        .runtime
        .save_backend(ExecutionBackend::Cpu)
        .await
        .is_err());
    assert!(setup
        .save(
            serde_json::from_value::<Envelope<Settings>>(settings("whisper-tiny", "en"))
                .unwrap()
                .data
        )
        .await
        .is_err());
    assert!(setup.runtime.verify("ivrit-turbo").is_err());
    drop(lease);
    let (status, view) = call(
        &app,
        "POST",
        "/api/v1/setup/models/ivrit-turbo/delete",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert!(setup.runtime.loaded().is_none());
    assert!(!partial.exists());
    assert!(!f.layout.models_dir.join("ivrit-turbo.bin").exists());
    assert!(!read_registry(&f.layout.state_dir)
        .await
        .unwrap()
        .contains_key("ivrit-turbo"));
    assert_eq!(
        setup.settings().transcription.selection.model,
        "ivrit-turbo"
    );
    assert_eq!(std::fs::read(&imported).unwrap(), f.weights);
    assert_eq!(std::fs::read(audio.path).unwrap(), b"retained audio");
    // Deletion is idempotent, and a deliberate reinstall becomes ready again.
    setup.runtime.delete_model("ivrit-turbo").await.unwrap();
    std::fs::copy(imported, f.layout.models_dir.join("ivrit-turbo.bin")).unwrap();
    setup.runtime.verify("ivrit-turbo").unwrap();
    wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    setup.runtime.select(None).unwrap();
}

#[tokio::test]
async fn active_download_must_cancel_and_drain_before_deletion() {
    let f = Fixture::new();
    let o = origin(f.weights.clone()).await;
    *o.delay_ms.lock().unwrap() = 100;
    let setup = f.open(&o.url, Arc::new(Credentials::new(&f.layout.state_dir)));
    let app = routes_with(setup.clone(), verifier());
    setup.runtime.download("ivrit-turbo").unwrap();
    assert!(setup.runtime.delete_model("ivrit-turbo").await.is_err());
    setup.runtime.cancel_download("ivrit-turbo").unwrap();
    wait_for(&app, Duration::from_secs(10), |v| {
        weights_of(v, "ivrit-turbo")["state"] == "failed"
    })
    .await;
    setup.runtime.delete_model("ivrit-turbo").await.unwrap();
    assert!(setup.runtime.installed("ivrit-turbo").is_none());
    assert!(!f.layout.models_dir.join("ivrit-turbo.bin.part").exists());
}

/// Choosing a cloud provider is not a model change, so a download in flight is
/// not a reason to refuse it. This is the guide's own order of events: the local
/// weights are still arriving when you decide to use Gemini instead.
#[tokio::test]
async fn a_cloud_selection_saves_while_local_weights_are_still_downloading() {
    let f = Fixture::new();
    let o = origin(f.weights.clone()).await;
    *o.delay_ms.lock().unwrap() = 100;
    let credentials = Arc::new(Credentials::with_source(
        &f.layout.state_dir,
        Arc::new(SecretSource),
    ));
    let setup = f.open(&o.url, credentials);
    let app = routes_with(setup.clone(), verifier());
    setup.runtime.download("ivrit-turbo").unwrap();
    let cloud = serde_json::json!({ "api_version": "1", "data": { "ui_language": "en", "transcription": {
        "selection": { "provider": "gemini", "model": "gemini-3.8-flash" }, "input_language": "mixed",
        "output_language": { "mode": "preserve" }, "vocabulary": { "hints": [], "corrections": [], "snippets": [] },
        "cleanup": { "mode": "off" } } } });
    let (status, body) = call(&app, "PUT", "/api/v1/settings", Some(cloud)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The download is untouched by the save, and a local selection still waits.
    assert_eq!(
        weights_of(
            &call(&app, "GET", "/api/v1/setup", None).await.1,
            "ivrit-turbo"
        )["state"],
        "downloading"
    );
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/settings",
        Some(settings("whisper-tiny", "en")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    setup.runtime.cancel_download("ivrit-turbo").unwrap();
}

#[tokio::test]
async fn direct_jobs_apply_every_catalog_language_restriction_before_loading() {
    let f = Fixture::new();
    let setup = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    let provider = LocalProvider::managed(setup.runtime.clone());
    let audio = f.audio();
    for model in catalog::models() {
        for language in [InputLanguage::En, InputLanguage::He, InputLanguage::Mixed] {
            let mut options = default_settings().transcription;
            options.selection.model = model.id.clone();
            options.input_language = language.clone();
            let error = provider.transcribe(&audio, &options).await.unwrap_err();
            let supported = model
                .input_languages
                .iter()
                .any(|l| std::mem::discriminant(l) == std::mem::discriminant(&language));
            assert!(
                if supported {
                    matches!(error.code, ErrorCode::ProviderUnavailable)
                } else {
                    matches!(error.code, ErrorCode::UnsupportedCapability)
                },
                "{} {language:?}",
                model.id
            );
        }
    }
}

#[tokio::test]
async fn failed_gpu_request_prepares_cpu_and_preserves_audio_for_explicit_retry() {
    let f = Fixture::new();
    cpu_package(&f);
    let (setup, app) = installed_fixture(&f).await;
    setup.runtime.select(Some("ivrit-turbo")).unwrap();
    wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
    })
    .await;
    std::fs::write(f.root.join("runtime/inference-fail"), b"fixture failure").unwrap();
    let provider = LocalProvider::managed(setup.runtime.clone());
    let audio = f.audio();
    let mut options = default_settings().transcription;
    options.selection.model = "ivrit-turbo".into();
    assert!(matches!(
        provider
            .transcribe(&audio, &options)
            .await
            .unwrap_err()
            .code,
        ErrorCode::ProviderFailed
    ));
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "ready"
            && v["data"]["runtime"]["worker"]["backend"] == "cpu"
    })
    .await;
    assert_eq!(
        view["data"]["runtime"]["execution"]["fallback_reason"],
        "gpu_inference_failed"
    );
    assert_eq!(setup.runtime.desired().as_deref(), Some("ivrit-turbo"));
    assert_eq!(
        provider
            .transcribe(&audio, &options)
            .await
            .unwrap()
            .raw_text,
        "שלום managed he"
    );
    assert_eq!(std::fs::read(audio.path).unwrap(), b"retained audio");
    setup.runtime.select(None).unwrap();
}

/// A stand-in for a provider's model-listing endpoint. It records the headers it
/// was given so the test can prove the key travels in the right one, and answers
/// with whatever status the test is exercising.
struct ProviderFixture {
    url: String,
    status: Arc<Mutex<u16>>,
    seen: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ProviderFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn provider_fixture() -> ProviderFixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let status = Arc::new(Mutex::new(200_u16));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s, r) = (status.clone(), seen.clone());
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let (s, r) = (s.clone(), r.clone());
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0_u8];
                while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
                    if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                r.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&head).to_string());
                let code = *s.lock().unwrap();
                let body = b"{\"data\":[]}";
                let response = format!(
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(body).await;
            });
        }
    });
    ProviderFixture {
        url,
        status,
        seen,
        task,
    }
}

struct MutableKey(Mutex<Option<String>>);
impl credentials::CredentialSource for MutableKey {
    fn read(&self, provider: &ProviderId) -> Option<String> {
        matches!(provider, ProviderId::Gemini)
            .then(|| self.0.lock().unwrap().clone())
            .flatten()
    }
}

#[tokio::test]
async fn a_key_check_reports_what_the_provider_answered_and_never_echoes_the_key() {
    let f = Fixture::new();
    let source = Arc::new(MutableKey(Mutex::new(Some(
        "AIza-fixture-secret-value".into(),
    ))));
    let credentials = Arc::new(Credentials::with_source(
        &f.layout.state_dir,
        source.clone(),
    ));
    let upstream = provider_fixture().await;
    let setup = f.open("http://127.0.0.1:1", credentials);
    setup.set_cloud_endpoints(cloud_check::Endpoints {
        openai: upstream.url.clone(),
        gemini: upstream.url.clone(),
        grok: upstream.url.clone(),
        openrouter: upstream.url.clone(),
    });
    let app = routes_with(setup.clone(), verifier());

    // Nothing is claimed before anyone asks.
    let (_, view) = call(&app, "GET", "/api/v1/setup", None).await;
    let gemini = |v: &serde_json::Value| {
        v["data"]["cloud"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["provider"] == "gemini")
            .unwrap()
            .clone()
    };
    assert_eq!(gemini(&view)["check"], serde_json::Value::Null);

    let (status, view) = call(&app, "POST", "/api/v1/setup/cloud/gemini/test", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gemini(&view)["check"]["outcome"], "accepted");
    assert!(gemini(&view)["check"]["checked_at_ms"].as_u64().unwrap() > 0);
    // Accepted is not verified: a key that works is still not a transcript.
    assert_eq!(gemini(&view)["verified"], false);
    assert!(
        !view.to_string().contains("fixture-secret"),
        "the checked key must not appear in the view"
    );

    // The key travelled in the provider's own header, and only there.
    let seen = upstream.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("GET /models "), "{}", seen[0]);
    assert!(seen[0].contains("x-goog-api-key: AIza-fixture-secret-value"));

    // A refusal is a result the page can show, not a failed request.
    *upstream.status.lock().unwrap() = 401;
    let (status, view) = call(&app, "POST", "/api/v1/setup/cloud/gemini/test", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gemini(&view)["check"]["outcome"], "rejected");

    // Anything else is honestly "no answer", never a pass.
    *upstream.status.lock().unwrap() = 500;
    let (_, view) = call(&app, "POST", "/api/v1/setup/cloud/gemini/test", None).await;
    assert_eq!(gemini(&view)["check"]["outcome"], "unreachable");

    // A replaced key retires the old answer instead of inheriting it.
    *upstream.status.lock().unwrap() = 200;
    call(&app, "POST", "/api/v1/setup/cloud/gemini/test", None).await;
    *source.0.lock().unwrap() = Some("AIza-a-different-key".into());
    let (_, view) = call(&app, "GET", "/api/v1/setup", None).await;
    assert_eq!(
        gemini(&view)["check"],
        serde_json::Value::Null,
        "a check belongs to the key it was made with"
    );
    assert_eq!(gemini(&view)["credential"], "protected_store");
}

struct OpenrouterKey;
impl credentials::CredentialSource for OpenrouterKey {
    fn read(&self, provider: &ProviderId) -> Option<String> {
        matches!(provider, ProviderId::Openrouter).then(|| "sk-or-fixture-secret".into())
    }
}

#[tokio::test]
async fn openrouter_is_a_text_only_key_row_checked_against_the_key_route() {
    let f = Fixture::new();
    let credentials = Arc::new(Credentials::with_source(
        &f.layout.state_dir,
        Arc::new(OpenrouterKey),
    ));
    let upstream = provider_fixture().await;
    let setup = f.open("http://127.0.0.1:1", credentials);
    setup.set_cloud_endpoints(cloud_check::Endpoints {
        openrouter: upstream.url.clone(),
        ..Default::default()
    });
    let app = routes_with(setup, verifier());
    let (_, view) = call(&app, "GET", "/api/v1/setup", None).await;
    let row = view["data"]["cloud"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["provider"] == "openrouter")
        .unwrap()
        .clone();
    // A key row for the text-model page, with nothing to pick for speech.
    assert_eq!(row["models"], serde_json::json!([]));
    assert_eq!(row["credential"], "protected_store");

    let (status, view) = call(&app, "POST", "/api/v1/setup/cloud/openrouter/test", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!view.to_string().contains("fixture-secret"));
    let seen = upstream.seen.lock().unwrap().clone();
    // The public model list would accept any key, so the check asks about the key.
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("GET /key "), "{}", seen[0]);
    assert!(seen[0].contains("authorization: Bearer sk-or-fixture-secret"));
}

#[tokio::test]
async fn checking_a_provider_with_no_key_or_no_such_provider_is_refused() {
    let f = Fixture::new();
    let setup = f.open(
        "http://127.0.0.1:1",
        Arc::new(Credentials::new(&f.layout.state_dir)),
    );
    let app = routes_with(setup, verifier());
    let (status, body) = call(&app, "POST", "/api/v1/setup/cloud/openai/test", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "provider_unavailable");
    let (status, body) = call(&app, "POST", "/api/v1/setup/cloud/local/test", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");
    let (status, _) = call(&app, "POST", "/api/v1/setup/cloud/..%2Fetc/test", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn spoofed_health_never_publishes_a_ready_worker() {
    let f = Fixture::new();
    f.mode("imposter");
    let (setup, app) = installed_fixture(&f).await;
    setup.runtime.select(Some("ivrit-turbo")).unwrap();
    let view = wait_for(&app, Duration::from_secs(20), |v| {
        v["data"]["runtime"]["worker"]["state"] == "failed"
            && v["data"]["runtime"]["worker"]["restarts"] == 3
    })
    .await;
    assert_eq!(
        view["data"]["runtime"]["worker"]["error"],
        "worker_identity_mismatch"
    );
    assert!(setup.runtime.loaded().is_none());
    assert_eq!(std::fs::read(f.audio().path).unwrap(), b"retained audio");
    setup.runtime.select(None).unwrap();
}

#[tokio::test]
async fn legacy_listener_without_private_handshake_fails_closed() {
    let mut f = Fixture::new();
    f.layout.startup_timeout = Duration::from_millis(300);
    f.mode("legacy");
    let (setup, app) = installed_fixture(&f).await;
    setup.runtime.select(Some("ivrit-turbo")).unwrap();
    let view = wait_for(&app, Duration::from_secs(10), |v| {
        v["data"]["runtime"]["worker"]["state"] == "failed"
            && v["data"]["runtime"]["worker"]["restarts"] == 3
    })
    .await;
    assert_eq!(
        view["data"]["runtime"]["worker"]["error"],
        "worker_start_timeout"
    );
    assert!(setup.runtime.loaded().is_none());
    setup.runtime.select(None).unwrap();
}
