#![cfg(any(windows, target_os = "macos"))]
// Test keys and inert bytes only; never accesses production feeds or an installer.
#[macro_use]
#[path = "../src/product.rs"]
mod product;
#[path = "../src/update_security.rs"]
mod security;
use base64::{engine::general_purpose::STANDARD, Engine};
use tauri_plugin_updater::UpdaterExt;

#[test]
fn tauri_handle_uses_authenticated_response_and_preserves_publisher_verification() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/update-release.json")).unwrap();
    let key = fixture["public_key"].as_str().unwrap();
    let envelope: security::Envelope = serde_json::from_value(fixture["valid"].clone()).unwrap();
    let manifest = envelope.authenticate(key).unwrap();
    let feed = serde_json::json!({"version":"0.21.0","platforms":{"windows-x86_64":{
        "url":format!("https://updates.tricktic.com/writer/releases/0.21.0/{}/setup.exe",manifest.sha256),
        "signature":fixture["artifact_signature"],"writer_release":fixture["legacy"],"dictate_release":fixture["valid"]}}});
    let temporary = tempfile::tempdir().unwrap();
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins =
        serde_json::from_value(serde_json::json!({"updater":{"pubkey":key}})).unwrap();
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().pubkey(key).build())
        .build(context)
        .unwrap();
    let updater = app
        .updater_builder()
        .target("windows-x86_64")
        .endpoints(vec!["https://unused.invalid/feed".parse().unwrap()])
        .unwrap()
        .executable_path(temporary.path().join("Writer.exe"))
        .build()
        .unwrap();
    let release = security::release_from_feed(&feed, "windows-x86_64", key).unwrap();
    let update = updater.update_from_response(feed.clone()).unwrap().unwrap();
    assert_eq!(update.version, release.manifest.version);
    assert_eq!(update.download_url.as_str(), release.url.as_str());
    assert_eq!(
        release.envelope.authenticate(key).unwrap().sha256,
        manifest.sha256
    );
    let bytes = STANDARD
        .decode(fixture["artifact"].as_str().unwrap())
        .unwrap();
    update.verify(&bytes).unwrap();
    release.manifest.verify_bytes(&bytes).unwrap();
    assert!(update.verify(b"tampered").is_err());
    // Native dependency proof: the original plugin's version comparator plus
    // publisher signature alone accept relabeling. Writer's signed binding stops it.
    let mut relabeled = feed.clone();
    relabeled["version"] = serde_json::json!("99.0.0");
    let legacy = updater
        .update_from_response(relabeled.clone())
        .unwrap()
        .unwrap();
    legacy.verify(&bytes).unwrap();
    assert!(security::release_from_feed(&relabeled, "windows-x86_64", key).is_err());
    let mut changed_signature = feed;
    changed_signature["platforms"]["windows-x86_64"]["signature"] = serde_json::json!("invalid");
    let invalid = updater
        .update_from_response(changed_signature)
        .unwrap()
        .unwrap();
    assert!(invalid.verify(&bytes).is_err());
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
    #[cfg(windows)]
    {
        // A cryptographically valid but interrupted/broken installer must fail
        // before OS launch and leave the old installation and saved data intact.
        let installed = temporary.path().join("Writer.exe");
        let saved = temporary.path().join("saved-work.fixture");
        std::fs::write(&installed, b"old installation sentinel").unwrap();
        std::fs::write(&saved, b"saved work sentinel").unwrap();
        for _ in 0..2 {
            update.verify(&bytes).unwrap();
            assert!(matches!(
                update.install(&bytes),
                Err(tauri_plugin_updater::Error::InvalidUpdaterFormat)
            ));
            assert_eq!(
                std::fs::read(&installed).unwrap(),
                b"old installation sentinel"
            );
            assert_eq!(std::fs::read(&saved).unwrap(), b"saved work sentinel");
        }
    }
}
