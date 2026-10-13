//! Native-only bootstrap over inherited anonymous pipes. No capability in URLs,
//! environment variables, diagnostics, assets or browser-visible responses.
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde::Deserialize;
use std::{io::Read, path::PathBuf, sync::Arc};

pub const ORIGIN: &str = "https://writer.metrix.invalid";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub version: u32,
    pub package_version: String,
    pub instance_id: uuid::Uuid,
    pub capability: String,
    pub data_directory: PathBuf,
    pub web_root: PathBuf,
}
impl Bootstrap {
    pub fn read(reader: &mut impl Read) -> Result<Self, &'static str> {
        let mut bytes = Vec::new();
        for _ in 0..16384 {
            let mut byte = [0];
            if reader.read(&mut byte).map_err(|_| "bootstrap_failed")? != 1 {
                return Err("bootstrap_failed");
            }
            if byte[0] == b'\n' {
                let config: Self =
                    serde_json::from_slice(&bytes).map_err(|_| "bootstrap_invalid")?;
                if config.version != 1
                    || config.package_version != env!("CARGO_PKG_VERSION")
                    || config.capability.len() != 64
                    || !config.capability.bytes().all(|b| b.is_ascii_hexdigit())
                    || !config.data_directory.is_absolute()
                    || !config.web_root.is_absolute()
                    || !config.web_root.join("index.html").is_file()
                {
                    return Err("bootstrap_invalid");
                }
                return Ok(config);
            }
            bytes.push(byte[0]);
        }
        Err("bootstrap_too_large")
    }
}
struct Boundary {
    config: Bootstrap,
    host: String,
}
fn equal_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |diff, (a, b)| diff | (a ^ b)) == 0
}
async fn authorize(
    State(boundary): State<Arc<Boundary>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let exactly = |name: header::HeaderName, expected: &str| {
        headers.get_all(&name).iter().count() == 1
            && headers
                .get_all(&name)
                .iter()
                .next()
                .is_some_and(|h| equal_secret(h.as_bytes(), expected.as_bytes()))
    };
    let authorized = exactly(
        header::AUTHORIZATION,
        &format!("Bearer {}", boundary.config.capability),
    ) && exactly(header::HOST, &boundary.host)
        && exactly(header::ORIGIN, ORIGIN);
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !request.uri().path().starts_with("/api/v1/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if request.uri().path() == "/api/v1/health" && request.method() == "GET" {
        return Json(serde_json::json!({
            "status": "ok", "service": "metrix-writer-api", "desktop_protocol": 1,
            "version": env!("CARGO_PKG_VERSION"), "instance_id": boundary.config.instance_id,
            "process_id": std::process::id()
        }))
        .into_response();
    }
    next.run(request).await
}
pub fn protect(app: Router, config: Bootstrap, address: std::net::SocketAddr) -> Router {
    app.layer(middleware::from_fn_with_state(
        Arc::new(Boundary {
            config,
            host: address.to_string(),
        }),
        authorize,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;
    fn config() -> Bootstrap {
        Bootstrap {
            version: 1,
            package_version: env!("CARGO_PKG_VERSION").into(),
            instance_id: uuid::Uuid::new_v4(),
            capability: "a".repeat(64),
            data_directory: std::env::temp_dir(),
            web_root: std::env::temp_dir(),
        }
    }
    #[tokio::test]
    async fn all_routes_fail_closed_and_health_proves_identity_without_capability() {
        let config = config();
        let id = config.instance_id;
        let app = protect(
            crate::app(Default::default(), PathBuf::from("missing")),
            config,
            "127.0.0.1:12345".parse().unwrap(),
        );
        for path in [
            "/api/v1/health",
            "/api/v1/transcriptions",
            "/api/v1/recordings",
            "/",
            "/api/v1/jobs/anything",
        ] {
            for bad in 0..4 {
                let req = Request::builder()
                    .uri(path)
                    .header(
                        "Host",
                        if bad == 1 {
                            "foreign:12345"
                        } else {
                            "127.0.0.1:12345"
                        },
                    )
                    .header(
                        "Origin",
                        if bad == 2 {
                            "https://evil.example"
                        } else {
                            ORIGIN
                        },
                    );
                let req = if bad == 0 {
                    req
                } else {
                    req.header(
                        "Authorization",
                        if bad == 3 {
                            "Bearer wrong".into()
                        } else {
                            format!("Bearer {}", "a".repeat(64))
                        },
                    )
                };
                assert_eq!(
                    app.clone()
                        .oneshot(req.body(Body::empty()).unwrap())
                        .await
                        .unwrap()
                        .status(),
                    StatusCode::UNAUTHORIZED
                );
            }
        }
        let req = Request::builder()
            .uri("/api/v1/health")
            .header("Host", "127.0.0.1:12345")
            .header("Origin", ORIGIN)
            .header("Authorization", format!("Bearer {}", "a".repeat(64)))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["instance_id"], id.to_string());
        assert_eq!(value["process_id"], std::process::id());
        assert!(!String::from_utf8_lossy(&bytes).contains(&"a".repeat(64)));
    }
    #[test]
    fn bootstrap_is_bounded_and_does_not_echo_input() {
        assert_eq!(
            Bootstrap::read(&mut &b"secret invalid\n"[..]).err(),
            Some("bootstrap_invalid")
        );
        assert_eq!(
            Bootstrap::read(&mut &vec![b'x'; 16385][..]).err(),
            Some("bootstrap_too_large")
        );
        assert_eq!(
            Bootstrap::read(&mut &b""[..]).err(),
            Some("bootstrap_failed")
        );
    }
}
