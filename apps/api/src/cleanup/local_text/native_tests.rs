//! Opt-in native runs of the packaged text runtime through the production adapter.
use super::*;

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "Requires WRITER_MAC_TEXT_CPU_RUNTIME and WRITER_MAC_TEXT_MODELS with pinned Qwen weights"]
async fn packaged_cpu_text_adapter_runs_and_removes_private_key() {
    packaged_text_adapter(
        std::env::var_os("WRITER_MAC_TEXT_CPU_RUNTIME").unwrap().into(),
        std::env::var_os("WRITER_MAC_TEXT_MODELS").unwrap().into(),
        false,
    )
    .await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "Requires WRITER_MAC_TEXT_RUNTIME and WRITER_MAC_TEXT_MODELS with pinned Qwen weights"]
async fn packaged_metal_text_adapter_runs_and_removes_private_key() {
    packaged_text_adapter(
        std::env::var_os("WRITER_MAC_TEXT_RUNTIME").unwrap().into(),
        std::env::var_os("WRITER_MAC_TEXT_MODELS").unwrap().into(),
        true,
    )
    .await;
}

/// MW-SEC-26: the source-built Windows llama-server through the production
/// adapter. WRITER_WINDOWS_TEXT_GPU=0 selects the CPU runtime folder.
#[cfg(windows)]
#[tokio::test]
#[ignore = "Requires WRITER_WINDOWS_TEXT_RUNTIME and WRITER_WINDOWS_TEXT_MODELS with pinned Qwen weights"]
async fn packaged_windows_text_adapter_runs_and_removes_private_key() {
    packaged_text_adapter(
        std::env::var_os("WRITER_WINDOWS_TEXT_RUNTIME")
            .unwrap()
            .into(),
        std::env::var_os("WRITER_WINDOWS_TEXT_MODELS")
            .unwrap()
            .into(),
        std::env::var("WRITER_WINDOWS_TEXT_GPU").as_deref() != Ok("0"),
    )
    .await;
}

async fn packaged_text_adapter(runtime_root: PathBuf, models: PathBuf, gpu: bool) {
    let spec = &MODELS[0];
    assert_eq!(
        crate::local_stt::managed::hash_file(&models.join(spec.file_name)).unwrap(),
        (spec.bytes, spec.sha256.to_owned())
    );
    let result = runtime::generate(&runtime_root, &models, spec, gpu,
        "Apply the editing instruction to selected_text. Preserve all other text. Return a JSON object with exactly one string field named text.",
        r#"{"instruction":"Replace Tuesday with Wednesday","selected_text":"The meeting is on Tuesday at 14:30."}"#).await.unwrap();
    let value: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(value["text"], "The meeting is on Wednesday at 14:30.");
    assert!(std::fs::read_dir(models).unwrap().all(|file| !file
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".writer-text-key-")));
}
