//! HTTP policy for the self-hosted browser surface. Tauri's asset protocol has
//! its own CSP. HSTS belongs to the HTTPS terminator, never forwarded headers.
use axum::{http::HeaderValue, response::Response};

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; media-src 'self' blob:; connect-src 'self' blob:; object-src 'none'; base-uri 'none'; form-action 'self'; frame-src 'self'; frame-ancestors 'self'";

pub(crate) async fn apply(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in [
        // Deliberately include static assets: no route classification can miss
        // a future sensitive response, redirect, or error.
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("content-security-policy", CSP),
        // Same-origin Settings -> panel preview is intentional.
        ("x-frame-options", "SAMEORIGIN"),
        ("referrer-policy", "no-referrer"),
        (
            "permissions-policy",
            "microphone=(self), camera=(), geolocation=()",
        ),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    fn assert_policy(response: &Response) {
        let headers = response.headers();
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["content-security-policy"], CSP);
        assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert_eq!(
            headers["permissions-policy"],
            "microphone=(self), camera=(), geolocation=()"
        );
        assert!(!headers.contains_key("strict-transport-security"));
    }

    #[tokio::test]
    async fn actual_router_covers_static_api_fallback_and_rejections() {
        let root = std::env::temp_dir().join(format!("writer-headers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(
            root.join("index.html"),
            "<!doctype html><title>Fixture</title>",
        )
        .unwrap();
        std::fs::write(root.join("assets/app.js"), "/* fixture */").unwrap();
        let app = crate::app(Default::default(), root.clone());
        for (method, uri, status) in [
            ("GET", "/", StatusCode::OK),
            ("HEAD", "/", StatusCode::OK),
            ("GET", "/assets/app.js", StatusCode::OK),
            ("GET", "/assets", StatusCode::TEMPORARY_REDIRECT),
            ("GET", "/missing", StatusCode::NOT_FOUND),
            ("GET", "/api/v1/missing", StatusCode::NOT_FOUND),
            ("GET", "/api/v1/health", StatusCode::OK),
            ("POST", "/api/v1/health", StatusCode::METHOD_NOT_ALLOWED),
            (
                "GET",
                "/api/v1/recordings/bad-id/audio",
                StatusCode::BAD_REQUEST,
            ),
            ("POST", "/api/v1/recordings", StatusCode::BAD_REQUEST),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("x-forwarded-proto", "https")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{method} {uri}");
            assert_policy(&response);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn server_errors_override_cacheable_inner_headers() {
        let app = Router::new()
            .route(
                "/",
                get(|| async {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        [("cache-control", "public, max-age=600")],
                    )
                }),
            )
            .layer(axum::middleware::map_response(apply));
        let response = app.oneshot(Request::new(Body::empty())).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_policy(&response);
    }
}
