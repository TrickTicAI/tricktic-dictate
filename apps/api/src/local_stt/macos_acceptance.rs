//! Opt-in real packaged-runtime qualification. All audio/output stays in private storage.
use super::*;
use managed::{ExecutionBackend, ManagedRuntime, RuntimeLayout, WorkerState};
use std::path::Path;

async fn wait(runtime: &ManagedRuntime, backend: &str, old: Option<u32>) -> u32 {
    let until = Instant::now() + Duration::from_secs(180);
    loop {
        if let WorkerState::Ready {
            backend: used,
            process_id,
            ..
        } = runtime.worker()
        {
            if used == backend && Some(process_id) != old {
                return process_id;
            }
        }
        assert!(
            Instant::now() < until,
            "worker readiness timeout: {:?}",
            runtime.worker()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn stop(runtime: &ManagedRuntime) {
    runtime.select(None).unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    while !matches!(runtime.worker(), WorkerState::Idle) {
        assert!(Instant::now() < until, "worker did not stop");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "Requires WRITER_MAC_ACCEPTANCE JSON manifest with verified runtimes, models and real audio"]
async fn packaged_metal_transcription_recovery_and_automatic_cpu_fallback() {
    let manifest = std::env::var_os("WRITER_MAC_ACCEPTANCE").expect("Set WRITER_MAC_ACCEPTANCE");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let root = PathBuf::from(config["output"].as_str().unwrap());
    std::fs::create_dir(&root).expect("Use a fresh private output directory");
    copy_tree(
        Path::new(config["resources"].as_str().unwrap())
            .join("runtime")
            .as_path(),
        &root.join("runtime"),
    );
    let models = root.join("Models");
    std::fs::create_dir(&models).unwrap();
    let runtime = ManagedRuntime::open_with_catalog(
        RuntimeLayout {
            runtime_dir: Some(root.join("runtime/whisper")),
            ffmpeg: Some(root.join("runtime/ffmpeg/ffmpeg")),
            models_dir: models.clone(),
            state_dir: root.join("State"),
            startup_timeout: Duration::from_secs(180),
        },
        crate::desktop_settings::catalog::specs(),
    )
    .unwrap();
    let mut receipts = Vec::new();
    for case in config["cases"].as_array().unwrap() {
        let id = case["model"].as_str().unwrap();
        let spec = runtime.spec(id).unwrap();
        if !models.join(spec.file_name).exists() {
            std::os::unix::fs::symlink(
                case["weights"].as_str().unwrap(),
                models.join(spec.file_name),
            )
            .unwrap();
            runtime.verify(id).unwrap();
            let until = Instant::now() + Duration::from_secs(180);
            while runtime.installed(id).is_none() {
                assert!(
                    Instant::now() < until,
                    "weights verification failed: {:?}",
                    runtime.weights(id)
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        runtime.save_backend(ExecutionBackend::Gpu).await.unwrap();
        runtime.select(Some(id)).unwrap();
        let start = Instant::now();
        let pid = wait(&runtime, "metal", None).await;
        let ready_ms = start.elapsed().as_millis();
        let catalog: crate::transcription::ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        let audio_path = root.join(format!("{}.wav", case["id"].as_str().unwrap()));
        std::fs::copy(case["audio"].as_str().unwrap(), &audio_path).unwrap();
        let before = managed::hash_file(&audio_path).unwrap();
        let mut metadata = catalog.recording.data.metadata;
        metadata.mime_type = "audio/wav".into();
        metadata.byte_length = before.0;
        metadata.duration_ms = case["duration_ms"].as_u64().unwrap();
        let audio = SavedAudio {
            metadata,
            path: audio_path,
            prepared: None,
        };
        let mut options = crate::desktop_settings::default_settings().transcription;
        options.selection.model = id.into();
        options.input_language = serde_json::from_value(case["language"].clone()).unwrap();
        let provider = LocalProvider::managed(runtime.clone());
        let start = Instant::now();
        let result = provider.transcribe(&audio, &options).await.unwrap();
        assert!(!result.raw_text.trim().is_empty());
        receipts.push(serde_json::json!({"case":case["id"], "worker":runtime.worker(), "warm_ms":ready_ms,
            "inference_ms":start.elapsed().as_millis(), "audio_sha256":before.1, "transcript":result}));
        if case["recover"].as_bool() == Some(true) {
            std::process::Command::new("/bin/kill")
                .args(["-KILL", &pid.to_string()])
                .status()
                .unwrap();
            wait(&runtime, "metal", Some(pid)).await;
            let result = LocalProvider::managed(runtime.clone())
                .transcribe(&audio, &options)
                .await
                .unwrap();
            assert!(!result.raw_text.trim().is_empty());
            assert_eq!(managed::hash_file(&audio.path).unwrap(), before);
            receipts.push(serde_json::json!({"recovery":true, "worker":runtime.worker(), "transcript":result}));
        }
        assert_eq!(managed::hash_file(&audio.path).unwrap(), before);
        stop(&runtime).await;
        if case["fallback"].as_bool() == Some(true) {
            // Only the disposable package copy is modified; the approved input stays intact.
            let executable = root.join("runtime/whisper/whisper-server");
            let hidden = executable.with_extension("unavailable");
            std::fs::rename(&executable, &hidden).unwrap();
            runtime
                .save_backend(ExecutionBackend::Automatic)
                .await
                .unwrap();
            runtime.select(Some(id)).unwrap();
            wait(&runtime, "cpu", None).await;
            let result = LocalProvider::managed(runtime.clone())
                .transcribe(&audio, &options)
                .await
                .unwrap();
            assert!(!result.raw_text.trim().is_empty());
            assert_eq!(managed::hash_file(&audio.path).unwrap(), before);
            assert_eq!(
                runtime.execution().fallback_reason.as_deref(),
                Some("metal_runtime_missing")
            );
            receipts.push(serde_json::json!({"automatic_fallback":true, "worker":runtime.worker(), "transcript":result}));
            stop(&runtime).await;
            std::fs::rename(hidden, executable).unwrap();
        }
        std::fs::write(
            root.join("receipts.json"),
            serde_json::to_vec_pretty(&receipts).unwrap(),
        )
        .unwrap();
    }
}
