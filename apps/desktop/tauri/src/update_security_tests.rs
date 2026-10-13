use super::*;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NOW: u64 = 1_800_000_000;
const PLATFORM: &str = "windows-x86_64";
fn fixture() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/update-release.json")).unwrap()
}
fn envelope(name: &str) -> Envelope {
    serde_json::from_value(fixture()[name].clone()).unwrap()
}
fn manifest(name: &str) -> Manifest {
    envelope(name)
        .authenticate(fixture()["public_key"].as_str().unwrap())
        .unwrap()
}
fn feed() -> Value {
    let f = fixture();
    json!({"version":"0.21.0", "platforms": {"windows-x86_64": {
        "url": format!("https://updates.tricktic.com/writer/releases/0.21.0/{}/setup.exe", manifest("valid").sha256),
        "signature":f["artifact_signature"], "writer_release":f["legacy"], "dictate_release":f["valid"] }}})
}
#[test]
fn authenticates_cross_language_signatures_and_binds_artifact_identity() {
    let f = fixture();
    let key = f["public_key"].as_str().unwrap();
    let bytes = STANDARD.decode(f["artifact"].as_str().unwrap()).unwrap();
    let release = release_from_feed(&feed(), PLATFORM, key).unwrap();
    release
        .manifest
        .validate("0.20.0", PLATFORM, NOW, None)
        .unwrap();
    release.manifest.verify_bytes(&bytes).unwrap();
    verify_signature(&bytes, f["artifact_signature"].as_str().unwrap(), key).unwrap();
    assert!(release.manifest.verify_bytes(b"MZ tampered").is_err());
    assert!(verify_signature(
        b"MZ tampered",
        f["artifact_signature"].as_str().unwrap(),
        key
    )
    .is_err());
    for name in ["wrong_app", "too_large", "too_long"] {
        assert!(envelope(name).authenticate(key).is_err(), "{name}");
    }
    assert!(manifest("mac")
        .validate("0.20.0", PLATFORM, NOW, None)
        .is_err());
    assert!(manifest("older")
        .validate("0.20.0", PLATFORM, NOW, None)
        .is_err());
}
#[test]
fn rejects_feed_relabeling_and_old_valid_signature_with_new_claimed_version() {
    let f = fixture();
    let key = f["public_key"].as_str().unwrap();
    let mut changed = feed();
    changed["version"] = json!("9.0.0");
    assert!(release_from_feed(&changed, PLATFORM, key).is_err());
    let mut changed = feed();
    changed["platforms"][PLATFORM][FEED_FIELD] = f["older"].clone();
    assert!(release_from_feed(&changed, PLATFORM, key).is_err());
    let mut tampered = envelope("valid");
    let mut payload: Value =
        serde_json::from_slice(&STANDARD.decode(&tampered.payload).unwrap()).unwrap();
    payload["version"] = json!("9.0.0");
    tampered.payload = STANDARD.encode(serde_json::to_vec(&payload).unwrap());
    assert!(tampered.authenticate(key).is_err());
    let mut legacy = feed();
    legacy["platforms"][PLATFORM]
        .as_object_mut()
        .unwrap()
        .remove(FEED_FIELD);
    assert!(release_from_feed(&legacy, PLATFORM, key).is_err());
}
#[test]
fn reads_only_the_new_identity_envelope_and_keeps_the_carried_over_floor() {
    let f = fixture();
    let key = f["public_key"].as_str().unwrap();
    // The Metrix Writer envelope is for installed 0.39-and-earlier copies only.
    assert!(envelope("legacy").authenticate(key).is_err());
    let mut swapped = feed();
    swapped["platforms"][PLATFORM][FEED_FIELD] = f["legacy"].clone();
    assert!(release_from_feed(&swapped, PLATFORM, key).is_err());
    let mut only_legacy = feed();
    only_legacy["platforms"][PLATFORM]
        .as_object_mut()
        .unwrap()
        .remove(FEED_FIELD);
    assert!(release_from_feed(&only_legacy, PLATFORM, key).is_err());
    // A rollback record carried over from Metrix Writer still sets the floor.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("State/update-release.json");
    remember(&path, &envelope("legacy_newer")).unwrap();
    let floor = previous(&path, key).unwrap().unwrap();
    assert_eq!((floor.app.as_str(), floor.version.as_str()), (LEGACY_APP, "0.22.0"));
    assert!(manifest("valid")
        .validate("0.20.0", PLATFORM, NOW, Some(&floor))
        .is_err());
    remember(&path, &envelope("legacy")).unwrap();
    let floor = previous(&path, key).unwrap().unwrap();
    manifest("valid")
        .validate("0.20.0", PLATFORM, NOW, Some(&floor))
        .unwrap();
    // Any other app id is still refused there.
    remember(&path, &envelope("wrong_app")).unwrap();
    assert!(previous(&path, key).is_err());
}
#[test]
fn expiry_rollback_equivocation_and_clock_checks_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state/release.json");
    let f = fixture();
    let key = f["public_key"].as_str().unwrap();
    assert!(previous(&path, key).unwrap().is_none());
    remember(&path, &envelope("valid")).unwrap();
    let old = previous(&path, key).unwrap().unwrap();
    manifest("valid")
        .validate("0.20.0", PLATFORM, NOW, Some(&old))
        .unwrap();
    assert!(manifest("valid")
        .validate("0.20.0", PLATFORM, old.expires_at, Some(&old))
        .is_err());
    assert!(manifest("valid")
        .validate("0.20.0", PLATFORM, NOW - 301, None)
        .is_err());
    assert!(manifest("replaced")
        .validate("0.20.0", PLATFORM, NOW, Some(&old))
        .is_err());
    remember(&path, &envelope("newer")).unwrap();
    let newer = previous(&path, key).unwrap().unwrap();
    assert!(old.validate("0.20.0", PLATFORM, NOW, Some(&newer)).is_err());
    assert!(manifest("older")
        .validate("0.18.0", PLATFORM, NOW, Some(&old))
        .is_err());
    let renewed = manifest("renewed");
    renewed
        .validate("0.20.0", PLATFORM, NOW + 100, Some(&old))
        .unwrap();
    assert!(old
        .validate("0.20.0", PLATFORM, NOW + 100, Some(&renewed))
        .is_err());
    // A crash leaving an uncommitted temporary record cannot replace the floor.
    std::fs::write(path.with_extension("tmp"), b"interrupted write").unwrap();
    assert_eq!(previous(&path, key).unwrap().unwrap().version, "0.22.0");
    std::fs::write(&path, b"corrupted").unwrap();
    assert!(previous(&path, key).is_err());
}
#[test]
fn denies_unapproved_destinations_and_artifact_paths() {
    for url in [
        "http://updates.tricktic.com/writer/x",
        "https://evil.test/writer/x",
        "https://updates.tricktic.com.evil.test/writer/x",
        "https://user@updates.tricktic.com/writer/x",
        "https://updates.tricktic.com:444/writer/x",
        "https://updates.tricktic.com/other/x",
        "https://updates.tricktic.com/writer/x?token=anything",
        "https://127.0.0.1/writer/x",
    ] {
        assert!(allowed_url(url).is_err(), "{url}");
    }
    let f = fixture();
    let mut changed = feed();
    changed["platforms"][PLATFORM]["url"] = json!("https://updates.tricktic.com/writer/other.exe");
    assert!(release_from_feed(&changed, PLATFORM, f["public_key"].as_str().unwrap()).is_err());
}

// HTTP is enabled only in this test client; production's client is HTTPS-only.
async fn response(parts: Vec<Vec<u8>>, stall: bool) -> (Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await;
        for part in parts {
            if socket.write_all(&part).await.is_err() {
                return;
            }
        }
        if stall {
            std::future::pending::<()>().await;
        }
    });
    (Url::parse(&format!("http://{address}/test")).unwrap(), task)
}
fn test_client() -> Client {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}
#[tokio::test]
async fn bounds_declared_and_chunked_bodies_and_refuses_redirects_or_interruption() {
    for (wire, expected) in [
        ("HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n", "byte limit"),
        ("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n1234\r\n4\r\n5678\r\n0\r\n\r\n", "byte limit"),
        ("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/internal\r\nContent-Length: 0\r\n\r\n", "redirect"),
        ("HTTP/1.1 206 Partial Content\r\nContent-Length: 0\r\n\r\n", "HTTP 206"),
        ("HTTP/1.1 204 No Content\r\n\r\n", "HTTP 204"),
        ("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n12", "interrupted"),
    ] {
        let (url,task)=response(vec![wire.as_bytes().to_vec()],false).await;
        let mut observed=0;
        let error=fetch(&test_client(),url,5,&CancellationToken::new(),|n| observed=n).await.unwrap_err();
        assert!(error.contains(expected),"{error}"); assert!(observed<=5); task.await.unwrap();
    }
}
#[tokio::test]
async fn stages_exact_signed_bytes_and_cancels_stalled_transfer_without_an_installer() {
    let f = fixture();
    let artifact = STANDARD.decode(f["artifact"].as_str().unwrap()).unwrap();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        artifact.len()
    );
    let (url, task) = response(vec![header.into_bytes(), artifact.clone()], false).await;
    let downloaded = fetch(
        &test_client(),
        url,
        artifact.len(),
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    manifest("valid").verify_bytes(&downloaded).unwrap();
    task.await.unwrap();
    let (url, task) = response(
        vec![b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n1".to_vec()],
        true,
    )
    .await;
    let cancel = CancellationToken::new();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        fetch(&test_client(), url, 5, &cancel, |_| cancel.cancel()),
    )
    .await
    .unwrap();
    assert!(result.unwrap_err().contains("canceled"));
    task.abort();
    // The production transport rejects HTTP before connecting.
    let url = Url::parse("http://127.0.0.1:1/no-request").unwrap();
    assert!(fetch(
        &client().unwrap(),
        url,
        5,
        &CancellationToken::new(),
        |_| {}
    )
    .await
    .is_err());
}

#[test]
fn platform_feeds_and_errors_distinguish_missing_releases_from_verification() {
    let endpoint = "https://updates.tricktic.com/writer/channels/{{platform}}-stable.json";
    assert_eq!(
        channel_url(endpoint, "darwin-aarch64").unwrap().path(),
        "/writer/channels/darwin-aarch64-stable.json"
    );
    assert_eq!(
        channel_url(endpoint, PLATFORM).unwrap().path(),
        "/writer/channels/windows-x86_64-stable.json"
    );
    assert!(channel_url(endpoint, "unsupported").is_err());
    assert!(channel_url("https://evil.test/writer/{{platform}}", PLATFORM).is_err());
    let f = fixture();
    let key = f["public_key"].as_str().unwrap();
    let missing = release_from_feed(&feed(), "darwin-aarch64", key)
        .err()
        .unwrap();
    assert!(missing.contains("no release for macOS"));
    assert!(!missing.contains("connection"));
    let mut legacy = feed();
    legacy["platforms"][PLATFORM]
        .as_object_mut()
        .unwrap()
        .remove(FEED_FIELD);
    assert!(release_from_feed(&legacy, PLATFORM, key)
        .err()
        .unwrap()
        .contains(concat!("signed ", product_name!(), " release metadata")));
    assert!(release_from_feed(&json!({}), PLATFORM, key)
        .err()
        .unwrap()
        .contains("malformed"));
}

#[tokio::test]
async fn server_failures_report_the_actual_http_status() {
    for status in [404, 403, 429, 503] {
        let wire = format!("HTTP/1.1 {status} Error\r\nContent-Length: 0\r\n\r\n");
        let (url, task) = response(vec![wire.into_bytes()], false).await;
        let error = fetch(&test_client(), url, 10, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert!(error.contains(&format!("HTTP {status}")), "{error}");
        assert!(!error.contains("connection"));
        task.await.unwrap();
    }
}
