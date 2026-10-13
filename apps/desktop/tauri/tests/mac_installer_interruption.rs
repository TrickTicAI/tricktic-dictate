#![cfg(target_os = "macos")]
// Explicit private installer acceptance. No networking, GUI, or live app access.
#[macro_use]
#[path = "../src/product.rs"]
mod product;
#[allow(dead_code)]
#[path = "../src/update_security.rs"]
mod security;
use std::{path::PathBuf, time::SystemTime};
use tauri_plugin_updater::UpdaterExt;

#[tokio::test]
#[ignore = "Requires retained signed archives, explicit candidate version and a scratch root"]
async fn retained_update_replay_expiry_and_cancel_preserve_scratch_data() {
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;
    let root = PathBuf::from(std::env::var_os("WRITER_INSTALL_FIXTURE_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    assert!(root.to_string_lossy().contains("/.local/native-"));
    assert_eq!(
        std::fs::read_to_string(root.join(".fixture")).unwrap(),
        "MW-SEC-22 disposable installer\n"
    );
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let key = config["plugins"]["updater"]["pubkey"].as_str().unwrap();
    let load = |name: &str| {
        let path = PathBuf::from(std::env::var_os(name).unwrap());
        let bytes = std::fs::read(&path).unwrap();
        // A new-identity archive has its own envelope; a retained Metrix Writer
        // archive (0.39 and earlier) only the legacy one.
        let sidecar = ["dictate-release", "writer-release"]
            .map(|kind| PathBuf::from(format!("{}.{kind}.json", path.display())))
            .into_iter()
            .find(|p| p.is_file())
            .unwrap();
        let envelope: security::Envelope =
            serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
        let manifest = envelope.authenticate_any(key).unwrap();
        manifest.verify_bytes(&bytes).unwrap();
        security::verify_signature(
            &bytes,
            &std::fs::read_to_string(format!("{}.sig", path.display())).unwrap(),
            key,
        )
        .unwrap();
        (envelope, manifest)
    };
    let (old_envelope, old) = load("WRITER_FIXTURE_ARCHIVE_037");
    let new_input = if std::env::var_os("WRITER_FIXTURE_ARCHIVE_NEW").is_some() {
        "WRITER_FIXTURE_ARCHIVE_NEW"
    } else {
        "WRITER_FIXTURE_ARCHIVE_038"
    };
    let (new_envelope, new) = load(new_input);
    let expected_version =
        std::env::var("WRITER_FIXTURE_NEW_VERSION").unwrap_or_else(|_| "0.38.0".into());
    assert_eq!(old.version, "0.37.0");
    assert_eq!(new.version, expected_version);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    old.validate("0.37.0", "darwin-aarch64", now, None).unwrap();
    new.validate("0.37.0", "darwin-aarch64", now, Some(&old))
        .unwrap();
    let state = root.join("profile/State/update-release.json");
    let sentinel = root.join("profile/saved-audio.fixture");
    let transcript = root.join("profile/raw-text.fixture");
    let before = [
        std::fs::read(&sentinel).unwrap(),
        std::fs::read(&transcript).unwrap(),
    ];
    security::remember(&state, &old_envelope).unwrap();
    security::remember(&state, &new_envelope).unwrap();
    // Reopen the actual authenticated durable file rather than reusing memory.
    let floor = security::previous(&state, key).unwrap().unwrap();
    let replay_error = old
        .validate("0.37.0", "darwin-aarch64", now, Some(&floor))
        .unwrap_err();
    let installed_floor_error = old
        .validate(&new.version, "darwin-aarch64", now, None)
        .unwrap_err();
    // Only the verifier's explicit time argument changes; never the Mac clock.
    let expiry_error = new
        .validate("0.37.0", "darwin-aarch64", new.expires_at, Some(&floor))
        .unwrap_err();
    assert_eq!(
        security::previous(&state, key).unwrap().unwrap().version,
        new.version
    );
    // Optional offline renewal uses the same real protected-key signature and
    // immutable candidate bytes. Only this verifier's clock argument advances;
    // no deployed metadata or system clock is changed.
    let renewal = std::env::var_os("WRITER_FIXTURE_RENEWED_METADATA").map(|path| {
        let envelope: security::Envelope =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let renewed = envelope.authenticate_any(key).unwrap();
        assert_eq!(renewed.version, new.version);
        assert_eq!(renewed.sha256, new.sha256);
        assert_eq!(renewed.size, new.size);
        assert!(renewed.issued_at > new.issued_at);
        assert!(renewed.expires_at > new.expires_at);
        renewed
            .verify_bytes(&std::fs::read(std::env::var_os(new_input).unwrap()).unwrap())
            .unwrap();
        renewed
            .validate("0.37.0", "darwin-aarch64", now, Some(&new))
            .unwrap();
        renewed
            .validate("0.37.0", "darwin-aarch64", new.expires_at, Some(&new))
            .unwrap();
        security::remember(&state, &envelope).unwrap();
        let floor = security::previous(&state, key).unwrap().unwrap();
        assert_eq!(floor.issued_at, renewed.issued_at);
        let stale_error = new
            .validate("0.37.0", "darwin-aarch64", now, Some(&floor))
            .unwrap_err();
        serde_json::json!({"manifest":renewed,"old_expiry_recovered":true,
            "durable_renewal_reopened":true,"stale_same_version_error":stale_error,
            "offline_only":true,"deployed_renewal":false})
    });
    let url = security::allowed_url(&format!(
        "https://updates.tricktic.com/writer/releases/{}/{}/Metrix.Writer.app.tar.gz",
        old.version, old.sha256
    ))
    .unwrap();
    let cancel = CancellationToken::new();
    let mut received = 0;
    let started = std::time::Instant::now();
    let canceled = security::fetch(
        &security::client().unwrap(),
        url.clone(),
        old.size as usize,
        &cancel,
        |count| {
            received = count;
            if count >= 65536 {
                cancel.cancel();
            }
        },
    )
    .await
    .unwrap_err();
    assert!(canceled.contains("canceled"));
    assert!(received >= 65536 && received < old.size as usize);
    let cancel_seconds = started.elapsed().as_secs_f64();
    // A fresh bounded transfer after cancellation must authenticate completely.
    let downloaded = security::fetch(
        &security::client().unwrap(),
        url,
        old.size as usize,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    old.verify_bytes(&downloaded).unwrap();
    assert_eq!(
        before,
        [
            std::fs::read(&sentinel).unwrap(),
            std::fs::read(&transcript).unwrap()
        ]
    );
    let receipt = serde_json::json!({
        "schema":1,"checked_at_unix":now,"old":old,"new":new,"renewal":renewal,
        "replay_error":replay_error,"installed_floor_error":installed_floor_error,
        "expiry_error":expiry_error,"clock_argument_only":true,"durable_floor_retained":true,
        "cancel_error":canceled,"cancel_received_bytes":received,"cancel_seconds":cancel_seconds,
        "fresh_download_sha256":format!("{:x}",Sha256::digest(&downloaded)),
        "downloaded_version":old.version,
        "saved_data_unchanged":true,"installer_invoked":false,
        "limits":["native production verifier/transport harness, not installed GUI flow","optional offline signed renewal only; no deployed renewal or feed write","scratch sentinels, not a real profile upgrade"]
    });
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "Run only through docs/testing/fixtures/macos-installer-interruption.py"]
fn retained_signed_archive_installs_in_private_copy() {
    let root = PathBuf::from(std::env::var_os("WRITER_INSTALL_FIXTURE_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join(".fixture")).unwrap(),
        "MW-SEC-22 disposable installer\n"
    );
    let app_path = root.join("install/TrickTic Dictate.app");
    assert_eq!(app_path.canonicalize().unwrap(), app_path);
    assert!(root.join("profile/saved-audio.fixture").is_file());
    let archive = PathBuf::from(std::env::var_os("WRITER_INSTALL_FIXTURE_ARCHIVE").unwrap());
    let bytes = std::fs::read(&archive).unwrap();
    let signature = std::fs::read_to_string(format!("{}.sig", archive.display())).unwrap();
    let envelope: security::Envelope = serde_json::from_slice(
        &std::fs::read(format!("{}.dictate-release.json", archive.display())).unwrap(),
    )
    .unwrap();
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let key = config["plugins"]["updater"]["pubkey"].as_str().unwrap();
    let manifest = envelope.authenticate(key).unwrap();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let old_version = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(app_path.join("Contents/Info.plist"))
        .output()
        .unwrap();
    assert!(old_version.status.success());
    let old_version = String::from_utf8(old_version.stdout).unwrap();
    manifest
        .validate(old_version.trim(), "darwin-aarch64", now, None)
        .unwrap();
    manifest.verify_bytes(&bytes).unwrap();
    let feed = serde_json::json!({"version":manifest.version,"platforms":{"darwin-aarch64":{
        "url":format!("https://updates.tricktic.com/writer/releases/{}/{}/Metrix.Writer.app.tar.gz",manifest.version,manifest.sha256),
        "signature":signature,(security::FEED_FIELD):envelope}}});
    security::release_from_feed(&feed, "darwin-aarch64", key).unwrap();
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins = serde_json::from_value(config["plugins"].clone()).unwrap();
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().pubkey(key).build())
        .build(context)
        .unwrap();
    let updater = app
        .updater_builder()
        .target("darwin-aarch64")
        .endpoints(vec!["https://unused.invalid/feed".parse().unwrap()])
        .unwrap()
        .executable_path(app_path.join("Contents/MacOS/tricktic-dictate"))
        .build()
        .unwrap();
    let update = updater.update_from_response(feed).unwrap().unwrap();
    update.verify(&bytes).unwrap();
    std::fs::write(
        root.join("verified-before-install.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "old_version":old_version.trim(),"manifest":manifest,
            "metadata_signature":true,"artifact_signature":true,"hash_size":true,
            "current_clock_freshness":true,"network_used":false
        }))
        .unwrap(),
    )
    .unwrap();
    // The test-only interposer stops this actual unmodified native installer
    // after its old-bundle rename. The controller kills only this test process.
    update.install(&bytes).unwrap();
    std::fs::write(root.join("installation-returned"), b"success\n").unwrap();
}
