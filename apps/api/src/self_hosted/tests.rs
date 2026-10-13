use super::*;
use crate::recordings::RecordingStore;
use axum::{
    body::Body,
    http::Request,
    routing::{get, post},
};
use tower::ServiceExt;

struct Fixture {
    directory: std::path::PathBuf,
    key: String,
    boundary: Arc<Boundary>,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("writer-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("owner-key");
        issue_key(&path).unwrap();
        let key = std::fs::read_to_string(&path).unwrap();
        let boundary = Boundary::load(
            "127.0.0.1:8791".parse().unwrap(),
            "http://127.0.0.1:8791",
            &path,
        )
        .unwrap();
        Self {
            directory,
            key,
            boundary,
        }
    }
    fn router(&self) -> Router {
        self.boundary.protect(
            Router::new()
                .route("/api/v1/settings", get(|| async { "private" }))
                .route("/api/v1/setup/runtime/stop", post(|| async { "stopped" }))
                .route("/index.html", get(|| async { "public" })),
        )
    }
    async fn login(&self, router: &Router) -> String {
        let response = router
            .clone()
            .oneshot(
                request("POST", SESSION_PATH, None)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"owner_key":self.key}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        body["data"]["token"].as_str().unwrap().to_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn request(method: &str, path: &str, token: Option<&str>) -> axum::http::request::Builder {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("Host", "127.0.0.1:8791")
        .header(REQUEST_HEADER, "1");
    if let Some(token) = token {
        request.header("Authorization", format!("Bearer {token}"))
    } else {
        request
    }
}
async fn status(router: &Router, request: axum::http::request::Builder) -> StatusCode {
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn host_origin_and_simple_posts_fail_before_handlers_even_with_session() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let token = fixture.login(&router).await;
    for (method, path) in [
        ("GET", "/api/v1/settings"),
        ("POST", "/api/v1/setup/runtime/stop"),
    ] {
        assert_eq!(
            status(&router, request(method, path, None)).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(&router, request(method, path, Some(&token))).await,
            StatusCode::OK
        );
        for (name, value) in [
            ("Host", "untrusted.example"),
            ("Origin", "https://untrusted.example"),
            ("Origin", "null"),
            ("Sec-Fetch-Site", "cross-site"),
            ("Sec-Fetch-Site", "same-site"),
            ("X-Writer-Request", "1"),
        ] {
            assert_eq!(
                status(
                    &router,
                    request(method, path, Some(&token)).header(name, value)
                )
                .await,
                StatusCode::FORBIDDEN,
                "{name}"
            );
        }
        assert_eq!(
            status(
                &router,
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("Host", "untrusted.example")
                    .header("Origin", "https://untrusted.example")
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(
                &router,
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("Host", "127.0.0.1:8791")
                    .header("Authorization", format!("Bearer {token}"))
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(
                &router,
                request(method, path, Some(&token))
                    .header("Origin", "http://127.0.0.1:8791")
                    .header("Sec-Fetch-Site", "same-origin")
            )
            .await,
            StatusCode::OK
        );
    }
    // DNS rebinding, forged forwarding and duplicate headers never select another origin.
    for host in ["localhost:8791", "attacker.example:8791", "127.0.0.1:8792"] {
        assert_eq!(
            status(
                &router,
                Request::builder()
                    .uri("/index.html")
                    .header("Host", host)
                    .header("X-Forwarded-Host", "127.0.0.1:8791")
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        status(
            &router,
            request("GET", "/api/v1/settings", Some(&token))
                .header("Origin", "http://127.0.0.1:8791")
                .header("Origin", "http://127.0.0.1:8791")
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(
            &router,
            Request::builder()
                .uri("/api/v1/settings")
                .header(REQUEST_HEADER, "1")
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(
            &router,
            request("GET", "/api/v1/settings", Some(&token))
                .header("Authorization", format!("Bearer {token}"))
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(
            &router,
            request(
                "GET",
                "/api/v1/settings?token=not-a-credential",
                Some(&token)
            )
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(&router, request("GET", "/index.html", None)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn login_logout_expiry_restart_and_credentials_are_separate() {
    let fixture = Fixture::new();
    let router = fixture.router();
    for body in [
        "{}".to_owned(),
        serde_json::json!({"owner_key":"wrong"}).to_string(),
        serde_json::json!({"owner_key":fixture.key,"extra":true}).to_string(),
        "x".repeat(1025),
    ] {
        let response = router
            .clone()
            .oneshot(
                request("POST", SESSION_PATH, None)
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_client_error());
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains(&fixture.key));
    }
    assert_eq!(
        status(
            &router,
            request("POST", SESSION_PATH, None).header("Content-Type", "text/plain")
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(
            &router,
            request("POST", SESSION_PATH, None)
                .header("Content-Type", "application/json")
                .header("Cookie", "owner=unused")
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    let a = fixture.login(&router).await;
    let b = fixture.login(&router).await;
    assert_ne!(a, b);
    assert_ne!(a, fixture.key);
    assert_eq!(
        status(&router, request("GET", SESSION_PATH, Some(&fixture.key))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&router, request("DELETE", SESSION_PATH, Some(&a))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&router, request("GET", SESSION_PATH, Some(&a))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&router, request("GET", SESSION_PATH, Some(&b))).await,
        StatusCode::OK
    );
    fixture
        .boundary
        .sessions
        .lock()
        .unwrap()
        .insert(digest(&b), Instant::now() - Duration::from_secs(1));
    assert_eq!(
        status(&router, request("GET", SESSION_PATH, Some(&b))).await,
        StatusCode::UNAUTHORIZED
    );
    let c = fixture.login(&router).await;
    let restarted = Boundary::load(
        "127.0.0.1:8791".parse().unwrap(),
        "http://127.0.0.1:8791",
        &fixture.directory.join("owner-key"),
    )
    .unwrap();
    assert_eq!(
        status(
            &restarted.protect(Router::new()),
            request("GET", SESSION_PATH, Some(&c))
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn listener_configuration_fails_closed_and_does_not_overwrite_credentials() {
    let fixture = Fixture::new();
    let key = fixture.directory.join("owner-key");
    assert!(issue_key(&key).is_err());
    assert_eq!(std::fs::read_to_string(&key).unwrap(), fixture.key);
    for origin in [
        "http://writer.example",
        "https://writer.example/",
        "https://writer.example/path",
        "https://writer.example?key=secret",
        "https://user:secret@writer.example",
        "null",
        "ftp://writer.example",
    ] {
        assert!(
            Boundary::load("127.0.0.1:8791".parse().unwrap(), origin, &key).is_err(),
            "{origin}"
        );
    }
    assert!(Boundary::load(
        "0.0.0.0:8791".parse().unwrap(),
        "http://localhost:8791",
        &key
    )
    .is_err());
    assert!(Boundary::load(
        "0.0.0.0:8791".parse().unwrap(),
        "https://writer.example",
        &key
    )
    .is_ok());
    assert!(Boundary::load(
        "127.0.0.1:8791".parse().unwrap(),
        "https://writer.example",
        &key
    )
    .is_ok());
    assert!(Boundary::load("[::1]:8791".parse().unwrap(), "http://[::1]:8791", &key).is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Boundary::load(
            "127.0.0.1:8791".parse().unwrap(),
            "http://127.0.0.1:8791",
            &key
        )
        .is_err());
    }
}

#[tokio::test]
async fn login_rate_and_session_memory_are_bounded() {
    let fixture = Fixture::new();
    let router = fixture.router();
    *fixture.boundary.login_window.lock().unwrap() = (Instant::now(), 20);
    assert_eq!(
        status(
            &router,
            request("POST", SESSION_PATH, None).header("Content-Type", "application/json")
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS
    );
    *fixture.boundary.login_window.lock().unwrap() = (Instant::now() - Duration::from_secs(61), 20);
    fixture.login(&router).await;
    for _ in 0..MAX_SESSIONS {
        fixture
            .boundary
            .sessions
            .lock()
            .unwrap()
            .insert(digest(&random_secret()), Instant::now() + LIFETIME);
    }
    let response = router
        .oneshot(
            request("POST", SESSION_PATH, None)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"owner_key":fixture.key}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn saved_audio_survives_revocation_and_authorized_retry() {
    let fixture = Fixture::new();
    let pipeline =
        crate::jobs::Pipeline::open(fixture.directory.join("recordings"), 1_000_000, vec![])
            .await
            .unwrap();
    let contracts: crate::transcription::ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let mut metadata = contracts.recording.data.metadata;
    metadata.byte_length = 4;
    let id = metadata.id;
    pipeline
        .recordings
        .save(metadata, Box::new(std::io::Cursor::new(b"test".to_vec())))
        .await
        .unwrap();
    let router = fixture.boundary.protect(
        Router::new()
            .merge(crate::recordings::routes_with_pipeline(Some(
                pipeline.clone(),
            )))
            .merge(crate::jobs::routes_with_pipeline(Some(pipeline.clone()))),
    );
    let token = fixture.login(&router).await;
    let path = format!("/api/v1/recordings/{id}/audio");
    assert_eq!(
        status(&router, request("GET", &path, Some(&token))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&router, request("DELETE", SESSION_PATH, Some(&token))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&router, request("GET", &path, Some(&token))).await,
        StatusCode::UNAUTHORIZED
    );
    let replacement = fixture.login(&router).await;
    let response = router
        .clone()
        .oneshot(
            request("GET", &path, Some(&replacement))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        to_bytes(response.into_body(), 10).await.unwrap().as_ref(),
        b"test"
    );
    // A provider-less retry fails truthfully without touching the retained audio.
    let response = router
        .clone()
        .oneshot(
            request("POST", "/api/v1/transcriptions", Some(&replacement))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&contracts.transcription_request).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let job: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    pipeline.run_pending().await.unwrap();
    let job_path = format!("/api/v1/jobs/{}", job["data"]["id"].as_str().unwrap());
    let response = router
        .clone()
        .oneshot(
            request("GET", &job_path, Some(&replacement))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let result: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(result["data"]["status"]["state"], "failed");
    assert_eq!(
        status(&router, request("GET", &path, Some(&replacement))).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn public_https_and_dev_proxy_hosts_require_exact_configured_origin() {
    let fixture = Fixture::new();
    for origin in ["https://writer.example", "http://127.0.0.1:5174"] {
        let boundary = Boundary::load(
            "127.0.0.1:8791".parse().unwrap(),
            origin,
            &fixture.directory.join("owner-key"),
        )
        .unwrap();
        let router = boundary.protect(Router::new());
        let login = |host: &str, source: &str| {
            Request::builder()
                .method("POST")
                .uri(SESSION_PATH)
                .header("Host", host)
                .header("Origin", source)
                .header(REQUEST_HEADER, "1")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"owner_key":fixture.key}).to_string(),
                ))
                .unwrap()
        };
        assert_eq!(
            router
                .clone()
                .oneshot(login(&boundary.host, origin))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            router
                .clone()
                .oneshot(login("127.0.0.1:8791", origin))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let mut spoofed = login("foreign.example", "https://foreign.example");
        spoofed
            .headers_mut()
            .insert("x-forwarded-host", boundary.host.parse().unwrap());
        spoofed
            .headers_mut()
            .insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(
            router.oneshot(spoofed).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
}
