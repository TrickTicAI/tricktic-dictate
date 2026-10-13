use super::*;
pub(super) const EXECUTABLE: &str = if cfg!(windows) {
    "llama-server.exe"
} else {
    "llama-server"
};
use serde_json::json;
use std::sync::atomic::{AtomicI32, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

pub(super) async fn generate(
    root: &std::path::Path,
    models: &std::path::Path,
    spec: &ModelSpec,
    gpu: bool,
    instructions: &str,
    input: &str,
) -> Result<String, ApiError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|_| unavailable("No local text-worker port is available."))?;
    let port = listener
        .local_addr()
        .map_err(|_| unavailable("Local worker port unavailable."))?
        .port();
    drop(listener);
    let token = uuid::Uuid::new_v4().to_string();
    let name = format!(".writer-text-key-{}", uuid::Uuid::new_v4());
    let key_path = models.join(&name);
    struct KeyFile(PathBuf);
    impl Drop for KeyFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _key = KeyFile(key_path.clone());
    let mut file = tokio::fs::OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    file.mode(0o600);
    let mut file = file
        .open(&key_path)
        .await
        .map_err(|_| unavailable("Local worker credential storage is unavailable."))?;
    tokio::io::AsyncWriteExt::write_all(&mut file, token.as_bytes())
        .await
        .map_err(|_| unavailable("Local worker credential could not be written."))?;
    file.sync_all()
        .await
        .map_err(|_| unavailable("Local worker credential could not be committed."))?;
    drop(file);
    let alias = format!("writer-{}", uuid::Uuid::new_v4());
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(1)
        .to_string();
    let mut command = crate::local_stt::process::owned_command(&root.join(EXECUTABLE));
    command
        .args(["--host", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .args([
            "-m",
            spec.file_name,
            "-c",
            "8192",
            "-t",
            &threads,
            "-ngl",
            if gpu { "999" } else { "0" },
            "-lv",
            "4",
            "--parallel",
            "1",
            "--no-webui",
            "--alias",
            &alias,
            "--api-key-file",
            &name,
        ])
        .current_dir(models)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| unavailable("The local text runtime could not start."))?;
    #[cfg(any(windows, target_os = "macos"))]
    let _job = crate::local_stt::worker_job::WorkerJob::attach(child.id().unwrap_or(0))
        .ok_or_else(|| unavailable("Could not establish local text-worker ownership."))?;
    let layers = Arc::new(AtomicI32::new(-1));
    let observed = layers.clone();
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| unavailable("Local worker output is unavailable."))?;
    let reader = tokio::spawn(async move {
        let mut stream = BufReader::new(stderr);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match (&mut stream)
                .take(16_385)
                .read_until(b'\n', &mut buffer)
                .await
            {
                Ok(0) | Err(_) => break,
                _ => {}
            }
            // Keep only the numeric backend proof. Never retain/log prompt text,
            // generated output, context, paths or credentials from helper stderr.
            let line = String::from_utf8_lossy(&buffer);
            if line.contains("layers to GPU") {
                if let Some(value) = line
                    .split("offloaded ")
                    .nth(1)
                    .and_then(|s| s.split('/').next())
                    .and_then(|s| s.parse::<i32>().ok())
                {
                    observed.store(value, Ordering::Release);
                }
            }
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|_| unavailable("Local text client could not initialize."))?;
    let base = format!("http://127.0.0.1:{port}");
    let outcome = async {
        let until = tokio::time::Instant::now() + Duration::from_secs(180);
        let mut serving_since = None;
        loop {
            if child.try_wait().ok().flatten().is_some() { return Err(unavailable("Local text worker exited during model loading.")); }
            if tokio::time::Instant::now() >= until { return Err(unavailable("Local text model loading timed out.")); }
            if let Ok(response) = client.get(format!("{base}/v1/models")).bearer_auth(&token).timeout(Duration::from_secs(2)).send().await {
                if response.status().is_success() {
                    if let Ok(value) = response.json::<serde_json::Value>().await {
                        if value["data"].as_array().is_some_and(|models| models.iter().any(|m| m["id"] == alias)) {
                            let offloaded = layers.load(Ordering::Acquire);
                            // A build with no offload-capable device never reports layers (-1); it runs on the CPU.
                            if (gpu && offloaded > 0) || (!gpu && offloaded <= 0) { break; }
                            // Serving with no layer report for 2 s: the build cannot offload at all.
                            let silent = serving_since.get_or_insert_with(tokio::time::Instant::now).elapsed() >= Duration::from_secs(2);
                            if offloaded >= 0 || silent { return Err(unavailable("The local text runtime did not use the requested processor.")); }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let mut response = client.post(format!("{base}/v1/chat/completions")).bearer_auth(&token)
            .json(&json!({"model": alias, "temperature": 0, "max_tokens": 4096, "response_format": {"type": "json_object"}, "json_schema": output_schema(input),
                "messages": [{"role": "system", "content": instructions}, {"role": "user", "content": input}]}))
            .send().await.map_err(|_| unavailable("Local text inference failed. Your original transcript is retained."))?;
        if !response.status().is_success() {
            return Err(super::super::failure(if response.status().is_client_error() { ErrorCode::UnsupportedCapability } else { ErrorCode::ProviderFailed },
                "Local text request failed. The model's combined context is 8192 tokens; shorten the processing input or select another text model.", true));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable("Local text response was interrupted."))? {
            if body.len() + chunk.len() > 1_000_000 { return Err(invalid()); }
            body.extend_from_slice(&chunk);
        }
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| invalid())?;
        if value["choices"].as_array().is_none_or(|c| c.len() != 1) || value["choices"][0]["finish_reason"] != "stop" { return Err(invalid()); }
        value["choices"][0]["message"]["content"].as_str().map(str::to_owned).ok_or_else(invalid)
    }.await;
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    reader.abort();
    outcome
}
fn invalid() -> ApiError {
    super::super::failure(ErrorCode::InvalidProviderResponse, "The local text model returned incomplete or invalid output. The original transcript is retained.", true)
}
pub(crate) fn output_schema(input: &str) -> serde_json::Value {
    let input = serde_json::from_str::<serde_json::Value>(input).unwrap_or_default();
    let spans = input
        .get("editable_spans")
        .or_else(|| input.get("spans"))
        .and_then(|s| s.as_array());
    if let Some(spans) = spans {
        json!({"type":"object", "properties":{"spans":{"type":"array", "items":{"type":"string"}, "minItems":spans.len(), "maxItems":spans.len()}}, "required":["spans"], "additionalProperties":false})
    } else {
        json!({"type":"object", "properties":{"text":{"type":"string", "minLength":1}}, "required":["text"], "additionalProperties":false})
    }
}
