use super::*;
use crate::recordings::RecordingStore;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};

async fn setup() -> (PathBuf, Arc<Mobile>, Uuid, String, String) {
    let root = std::env::temp_dir().join(format!("writer-mobile-test-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let a = issue(&root, &root.join("a.token")).await.unwrap();
    issue(&root, &root.join("b.token")).await.unwrap();
    let token_a = tokio::fs::read_to_string(root.join("a.token"))
        .await
        .unwrap();
    let token_b = tokio::fs::read_to_string(root.join("b.token"))
        .await
        .unwrap();
    let providers: Vec<Arc<dyn TranscriptionProvider>> = vec![Arc::new(
        crate::providers::UnavailableProvider(crate::providers::ProviderInfo {
            id: ProviderId::Openai,
            execution_location: ExecutionLocation::Cloud,
            configured: false,
            available: false,
        }),
    )];
    let service = Mobile::open(root.clone(), 1024 * 1024, providers)
        .await
        .unwrap();
    (root, service, a, token_a, token_b)
}
async fn request(
    service: &Arc<Mobile>,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    service
        .router()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}
async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
#[tokio::test]
async fn every_mobile_route_requires_auth_and_desktop_routes_stay_absent() {
    let (root, service, id, a, b) = setup().await;
    assert!(Mobile::open(root.clone(), 1024, vec![]).await.is_err());
    let uuid = Uuid::new_v4();
    let allowed = vec![
        ("GET", "/api/v1/mobile/connection".into()),
        ("GET", "/api/v1/language-presets".into()),
        ("GET", "/api/v1/cleanup/providers".into()),
        ("GET", "/api/v1/vocabulary".into()),
        ("PUT", "/api/v1/vocabulary".into()),
        ("POST", "/api/v1/recordings".into()),
        ("GET", format!("/api/v1/recordings/{uuid}")),
        ("GET", format!("/api/v1/recordings/{uuid}/audio")),
        ("DELETE", format!("/api/v1/recordings/{uuid}")),
        ("POST", "/api/v1/transcriptions".into()),
        ("GET", format!("/api/v1/jobs/{uuid}")),
        ("GET", format!("/api/v1/jobs/{uuid}/processed")),
        ("POST", format!("/api/v1/jobs/{uuid}/cleanup")),
    ];
    for (method, path) in &allowed {
        for token in [None, Some("wrong")] {
            assert_eq!(
                request(&service, method, path, token, "{}").await.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
    }
    for path in [
        "/",
        "/api/v1/health",
        "/api/v1/settings",
        "/api/v1/setup",
        "/api/v1/setup/credentials",
        "/api/v1/setup/models",
        "/api/v1/local/preview",
        "/api/v1/history",
        "/api/v1/modes",
        "/api/v1/cleanup/local/models",
        "/api/v1/providers",
    ] {
        for method in ["GET", "POST", "PUT", "DELETE", "OPTIONS"] {
            assert_eq!(
                request(&service, method, path, Some(&a), "{}")
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{method} {path}"
            );
        }
    }
    let connection =
        json(request(&service, "GET", "/api/v1/mobile/connection", Some(&a), "").await).await;
    assert_eq!(connection["data"]["device_id"], id.to_string());
    for model in connection["data"]["models"].as_array().unwrap() {
        assert_eq!(model["selection"]["provider"], "openai");
        assert_eq!(model["status"], "not_configured");
        assert_eq!(model["configured"], false);
    }
    assert_eq!(
        request(
            &service,
            "GET",
            "/api/v1/mobile/connection?token=ignored",
            Some(&a),
            ""
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let browser = Request::builder()
        .uri("/api/v1/mobile/connection")
        .header("Authorization", format!("Bearer {a}"))
        .header("Origin", "https://example.invalid")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        service.router().oneshot(browser).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    revoke(&root, id).await.unwrap();
    for (method, path) in &allowed {
        assert_eq!(
            request(&service, method, path, Some(&a), "{}")
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        request(&service, "GET", "/api/v1/mobile/connection", Some(&b), "")
            .await
            .status(),
        StatusCode::OK
    );
    rotate(&root, id, &root.join("rotated.token"))
        .await
        .unwrap();
    let rotated = tokio::fs::read_to_string(root.join("rotated.token"))
        .await
        .unwrap();
    assert_eq!(
        request(&service, "GET", "/api/v1/mobile/connection", Some(&a), "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json(
            request(
                &service,
                "GET",
                "/api/v1/mobile/connection",
                Some(&rotated),
                ""
            )
            .await
        )
        .await["data"]["device_id"],
        id.to_string()
    );
    assert!(root.join("data").join(id.to_string()).is_dir());
}
#[tokio::test]
async fn recordings_jobs_retry_cleanup_and_vocabulary_are_device_scoped() {
    let (root, service, a_id, a, b) = setup().await;
    let mut fixture: ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let meta = &mut fixture.recording.data.metadata;
    meta.byte_length = 4;
    meta.mime_type = "audio/wav".into();
    let recording_id = meta.id;
    // Exercise the shared upload handler through the authenticated route, not a store shortcut.
    let body = format!("--test\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{}\r\n--test\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"recording.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF\r\n--test--\r\n", serde_json::to_string(&Envelope::new(meta.clone())).unwrap());
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/recordings")
        .header("Authorization", format!("Bearer {a}"))
        .header("Content-Type", "multipart/form-data; boundary=test")
        .body(Body::from(body))
        .unwrap();
    assert_eq!(
        service.router().oneshot(req).await.unwrap().status(),
        StatusCode::CREATED
    );
    for suffix in ["", "/audio"] {
        let path = format!("/api/v1/recordings/{recording_id}{suffix}");
        assert_eq!(
            request(&service, "GET", &path, Some(&a), "").await.status(),
            StatusCode::OK
        );
        assert_eq!(
            request(&service, "GET", &path, Some(&b), "").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let payload = serde_json::to_string(&fixture.transcription_request).unwrap();
    assert_eq!(
        request(
            &service,
            "POST",
            "/api/v1/transcriptions",
            Some(&b),
            &payload
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let submitted = request(
        &service,
        "POST",
        "/api/v1/transcriptions",
        Some(&a),
        &payload,
    )
    .await;
    assert_eq!(submitted.status(), StatusCode::ACCEPTED);
    let job = json(submitted).await;
    let job_id = job["data"]["id"].as_str().unwrap();
    assert_eq!(
        json(
            request(
                &service,
                "POST",
                "/api/v1/transcriptions",
                Some(&a),
                &payload
            )
            .await
        )
        .await["data"]["id"],
        job_id
    );
    for suffix in ["", "/processed", "/cleanup"] {
        let method = if suffix == "/cleanup" { "POST" } else { "GET" };
        let cleanup = r#"{"api_version":"1","data":{"provider":"openai","model":"gpt-4.1-mini"}}"#;
        assert_eq!(
            request(
                &service,
                method,
                &format!("/api/v1/jobs/{job_id}{suffix}"),
                Some(&b),
                cleanup
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    let mut retry = fixture.transcription_request.clone();
    retry.data.idempotency_key = Uuid::new_v4();
    retry.data.retry_of_job_id = Some(Uuid::parse_str(job_id).unwrap());
    // A foreign prior job cannot authorize a retry, even if B uploads its own audio with the same recording ID.
    let b_id = Uuid::parse_str(b.split('.').nth(1).unwrap()).unwrap();
    let store = crate::recordings::FileRecordingStore::open(
        root.join("data").join(b_id.to_string()).join("recordings"),
        1024 * 1024,
    )
    .await
    .unwrap();
    store
        .save(
            meta.clone(),
            Box::new(std::io::Cursor::new(b"RIFF".to_vec())),
        )
        .await
        .unwrap();
    assert_eq!(
        request(
            &service,
            "POST",
            "/api/v1/transcriptions",
            Some(&b),
            &serde_json::to_string(&retry).unwrap()
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &service,
            "DELETE",
            &format!("/api/v1/recordings/{recording_id}"),
            Some(&b),
            ""
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &service,
            "GET",
            &format!("/api/v1/recordings/{recording_id}/audio"),
            Some(&a),
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );
    let vocabulary = r#"{"api_version":"1","data":{"hints":[{"text":"private hint","enabled":true}],"corrections":[],"snippets":[]}}"#;
    assert_eq!(
        request(&service, "PUT", "/api/v1/vocabulary", Some(&a), vocabulary)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        json(request(&service, "GET", "/api/v1/vocabulary", Some(&b), "").await).await["data"]
            ["hints"],
        serde_json::json!([])
    );
    assert!(root
        .join("data")
        .join(a_id.to_string())
        .join("recordings")
        .is_dir());
}
