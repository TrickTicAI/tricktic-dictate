#![cfg(target_os = "macos")]
// Explicit local acceptance check. Uses the real updater downloader/installer
// with a mock application handle and a disposable install directory, never the
// user's installed app, data root, permissions, or production feed.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri_plugin_updater::UpdaterExt;

#[tokio::test]
#[ignore = "requires WRITER_UPDATE_TEST_ARCHIVE pointing to a retained signed Mac release"]
async fn signed_mac_archive_installs_and_tampering_is_refused() {
    let archive = std::env::var("WRITER_UPDATE_TEST_ARCHIVE").expect("retained app.tar.gz path");
    let bytes = Arc::new(std::fs::read(&archive).unwrap());
    let signature = std::fs::read_to_string(format!("{archive}.sig")).unwrap();
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let public_key = config["plugins"]["updater"]["pubkey"].as_str().unwrap();
    let temporary =
        std::env::temp_dir().join(format!("writer-update-check-{}", uuid::Uuid::new_v4()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(temporary.clone());
    let app_path = temporary.join("Metrix Writer.app");
    let executable = app_path.join("Contents/MacOS/tricktic-dictate");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"original installation sentinel").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let manifest = serde_json::json!({
        "version": config["version"],
        "platforms": {"darwin-aarch64": {
            "url": format!("http://{address}/archive"), "signature": signature
        }}
    });
    let tamper = Arc::new(AtomicBool::new(false));
    let download_bytes = bytes.clone();
    let download_tamper = tamper.clone();
    let router = axum::Router::new()
        .route(
            "/feed",
            axum::routing::get(move || {
                let manifest = manifest.clone();
                async move { axum::Json(manifest) }
            }),
        )
        .route(
            "/archive",
            axum::routing::get(move || {
                let mut response = download_bytes.as_ref().clone();
                if download_tamper.load(Ordering::Acquire) {
                    let last = response.last_mut().unwrap();
                    *last ^= 1;
                }
                async move { response }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins = serde_json::from_value(config["plugins"].clone()).unwrap();
    let app = tauri::test::mock_builder()
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(public_key)
                .build(),
        )
        .build(context)
        .unwrap();
    let update = app
        .updater_builder()
        .endpoints(vec![format!("http://{address}/feed").parse().unwrap()])
        .unwrap()
        .executable_path(&executable)
        .no_proxy()
        .build()
        .unwrap()
        .check()
        .await
        .unwrap()
        .expect("Mac feed entry");
    assert_eq!(update.version, config["version"].as_str().unwrap());
    let verified = update.download(|_, _| {}, || {}).await.unwrap();
    assert_eq!(&verified, bytes.as_ref());
    tamper.store(true, Ordering::Release);
    let error = update.download(|_, _| {}, || {}).await.unwrap_err();
    assert!(
        matches!(error, tauri_plugin_updater::Error::Minisign(_)),
        "{error}"
    );
    assert_eq!(
        std::fs::read(&executable).unwrap(),
        b"original installation sentinel"
    );
    update.install(&verified).unwrap();
    assert!(std::fs::metadata(&executable).unwrap().len() > 1_000_000);
    let check = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&app_path)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    server.abort();
}
