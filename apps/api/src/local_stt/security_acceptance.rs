//! Explicit, isolated native GPU/codec acceptance. Never reads personal audio.
use super::*;
use std::path::Path;

#[tokio::test]
#[ignore = "requires WRITER_HELPER_ACCEPTANCE manifest with text_worker path"]
async fn native_text_worker_still_sees_gpu() {
    let manifest = std::env::var_os("WRITER_HELPER_ACCEPTANCE").expect("acceptance manifest");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let mut command = process::owned_command(Path::new(config["text_worker"].as_str().unwrap()));
    command
        .arg("--list-devices")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().unwrap();
    let _job = worker_job::WorkerJob::attach(child.id().unwrap()).unwrap();
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "Text worker device enumeration failed"
    );
    let output = [output.stdout, output.stderr].concat();
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.lines().any(|line| {
            let line = line.trim_start();
            if cfg!(windows) {
                line.starts_with("Vulkan0:")
            } else {
                line.starts_with("MTL0:") || line.starts_with("Metal:")
            }
        }),
        "Requested GPU backend was not enumerated"
    );
    // Never persist raw output or interpolate it into a test failure.
}

#[tokio::test]
#[ignore = "requires WRITER_HELPER_ACCEPTANCE manifest with verified runtime and model paths"]
async fn native_gpu_and_malformed_media_keep_working_under_helper_policy() {
    let manifest = std::env::var_os("WRITER_HELPER_ACCEPTANCE").expect("acceptance manifest");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let root = PathBuf::from(config["output"].as_str().unwrap());
    assert!(!root.exists(), "Use a new isolated output directory");
    crate::helper_security::protect_private_dir(&root).unwrap();
    let models = root.join("Models");
    crate::helper_security::protect_private_dir(&models).unwrap();
    let model = config["model"].as_str().unwrap();
    let spec = crate::desktop_settings::catalog::specs()
        .iter()
        .find(|s| s.id == model)
        .unwrap();
    std::fs::copy(
        config["weights"].as_str().unwrap(),
        models.join(spec.file_name),
    )
    .unwrap();
    let runtime = managed::ManagedRuntime::open_with_catalog(
        managed::RuntimeLayout {
            runtime_dir: Some(config["runtime"].as_str().unwrap().into()),
            ffmpeg: Some(config["ffmpeg"].as_str().unwrap().into()),
            models_dir: models,
            state_dir: root.join("State"),
            startup_timeout: Duration::from_secs(120),
        },
        crate::desktop_settings::catalog::specs(),
    )
    .unwrap();
    runtime.verify(model).unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    while runtime.installed(model).is_none() {
        assert!(
            Instant::now() < deadline,
            "weights verification did not finish"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    runtime
        .save_backend(managed::ExecutionBackend::Gpu)
        .await
        .unwrap();
    runtime.select(Some(model)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let managed::WorkerState::Ready { backend, .. } = runtime.worker() {
            assert_eq!(backend, managed::GPU_BACKEND);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "GPU worker not ready: {:?}",
            runtime.worker()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Fixed WebM demuxer, no network protocols and bounded output. Failure must
    // neither expose stderr nor remove the saved source.
    let malformed = root.join("malformed.webm");
    std::fs::write(&malformed, b"synthetic invalid media").unwrap();
    let args: Vec<std::ffi::OsString> = [
        "-nostdin",
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "webm",
        "-i",
    ]
    .into_iter()
    .map(Into::into)
    .chain(std::iter::once(malformed.clone().into_os_string()))
    .chain(["-f", "s16le", "pipe:1"].into_iter().map(Into::into))
    .collect();
    let result = process::run(
        Path::new(config["ffmpeg"].as_str().unwrap()),
        &args,
        1_000_000,
        Duration::from_secs(10),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(&malformed).unwrap(),
        b"synthetic invalid media"
    );
    assert!(!root.join("State/worker/startup.log").exists());
    runtime.select(None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !matches!(runtime.worker(), managed::WorkerState::Idle) {
        assert!(Instant::now() < deadline, "worker did not stop");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Retain only pass/fail evidence. Copied weights and synthetic input are
    // disposable; no installed runtime, live session or user recording changed.
    std::fs::remove_dir_all(root).unwrap();
}
