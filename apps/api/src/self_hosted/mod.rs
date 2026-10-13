//! HTTP-only single-owner boundary. Tauri's in-process router and mobile's
//! separately authenticated service deliberately do not use this middleware.
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const SESSION_PATH: &str = "/api/v1/session";
const LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_SESSIONS: usize = 32;
const REQUEST_HEADER: &str = "x-writer-request";

fn random_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn digest(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}
fn equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter()
        .zip(b)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}
fn valid_secret(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// New file only; never place a credential in output, arguments or URLs.
pub fn issue_key(path: &Path) -> Result<(), &'static str> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Cannot create owner key file; choose a new path in a private directory")?;
    file.write_all(random_secret().as_bytes())
        .map_err(|_| "Cannot write owner key file")?;
    file.sync_all().map_err(|_| "Cannot save owner key file")
}

pub struct Boundary {
    origin: String,
    host: String,
    owner_digest: [u8; 32],
    sessions: Mutex<HashMap<[u8; 32], Instant>>,
    login_window: Mutex<(Instant, u32)>,
}
impl Boundary {
    pub fn from_env(address: SocketAddr) -> Result<Arc<Self>, &'static str> {
        let path = std::env::var_os("WRITER_OWNER_KEY_FILE").ok_or(
            "Set WRITER_OWNER_KEY_FILE to a private key file; create it with --init-owner-key <path>",
        )?;
        let origin =
            match std::env::var("WRITER_HTTP_ORIGIN") {
                Ok(origin) => origin,
                Err(std::env::VarError::NotPresent) if address.ip().is_loopback() => {
                    format!("http://{address}")
                }
                _ => return Err(
                    "Non-loopback HTTP requires WRITER_HTTP_ORIGIN with the public HTTPS origin",
                ),
            };
        Self::load(address, &origin, Path::new(&path))
    }

    pub fn load(
        address: SocketAddr,
        origin: &str,
        key_path: &Path,
    ) -> Result<Arc<Self>, &'static str> {
        let url = reqwest::Url::parse(origin).map_err(|_| "Invalid WRITER_HTTP_ORIGIN")?;
        let canonical = url.origin().ascii_serialization();
        let loopback_origin = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if origin != canonical
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https"
                || (url.scheme() == "http" && address.ip().is_loopback() && loopback_origin))
        {
            return Err("WRITER_HTTP_ORIGIN must be an exact HTTPS origin, or HTTP localhost on a loopback listener (no path or trailing slash)");
        }
        let file = std::fs::File::open(key_path).map_err(|_| "Cannot read owner key file")?;
        let metadata = file
            .metadata()
            .map_err(|_| "Cannot inspect owner key file")?;
        if !metadata.is_file() || metadata.len() > 65 {
            return Err("Invalid owner key file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err("Owner key file must be private (chmod 600)");
            }
        }
        let mut key = String::new();
        file.take(66)
            .read_to_string(&mut key)
            .map_err(|_| "Cannot read owner key file")?;
        let key = key.trim_end_matches('\n');
        if !valid_secret(key) {
            return Err("Invalid owner key file; use --init-owner-key to generate a key");
        }
        Ok(Arc::new(Self {
            host: canonical.split_once("://").unwrap().1.to_owned(),
            origin: canonical,
            owner_digest: digest(key),
            sessions: Mutex::new(HashMap::new()),
            login_window: Mutex::new((Instant::now(), 0)),
        }))
    }

    pub fn protect(self: &Arc<Self>, router: Router) -> Router {
        router.layer(middleware::from_fn_with_state(self.clone(), authorize))
    }

    fn session(&self, headers: &HeaderMap) -> Option<[u8; 32]> {
        let value = single(headers, header::AUTHORIZATION.as_str())?.strip_prefix("Bearer ")?;
        if !valid_secret(value) {
            return None;
        }
        let key = digest(value);
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, expires| *expires > Instant::now());
        sessions.contains_key(&key).then_some(key)
    }
}
fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}
fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"api_version":"1", "error":{
            "code":code, "message":message, "retryable":false
        }})),
    )
        .into_response()
}
fn unauthorized() -> Response {
    error(
        StatusCode::UNAUTHORIZED,
        "session_unauthorized",
        "Sign in to TrickTic Dictate again. Saved audio remains available.",
    )
}
async fn authorize(
    State(boundary): State<Arc<Boundary>>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = dispatch(boundary, request, next).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
async fn dispatch(boundary: Arc<Boundary>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    // Forwarded headers never establish trust. The reverse proxy must preserve
    // the one configured public Host, and be the only network path to HTTP.
    let origin_ok = !headers.contains_key(header::ORIGIN)
        || single(headers, "origin") == Some(boundary.origin.as_str());
    let fetch_ok = !headers.contains_key("sec-fetch-site")
        || matches!(
            single(headers, "sec-fetch-site"),
            Some("same-origin" | "none")
        );
    if single(headers, "host") != Some(boundary.host.as_str())
        || !origin_ok
        || !fetch_ok
        || request
            .uri()
            .authority()
            .is_some_and(|authority| authority.as_str() != boundary.host)
    {
        return error(
            StatusCode::FORBIDDEN,
            "request_forbidden",
            "This request origin is not allowed.",
        );
    }
    let path = request.uri().path();
    if !path.starts_with("/api/") {
        // Public assets contain no credentials or private state. An encoded API
        // path cannot route to a handler after this test: Axum matches raw paths.
        return next.run(request).await;
    }
    // No API currently takes query parameters. Reject before tracing to prevent
    // accidental URL credentials from being logged or interpreted by handlers.
    if request.uri().query().is_some() || single(headers, REQUEST_HEADER) != Some("1") {
        return error(
            StatusCode::FORBIDDEN,
            "request_forbidden",
            "Use the TrickTic Dictate application to access this API.",
        );
    }
    if path == SESSION_PATH && request.method() == Method::POST {
        if headers.contains_key(header::AUTHORIZATION)
            || headers.contains_key(header::COOKIE)
            || single(headers, "content-type") != Some("application/json")
        {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Invalid sign-in request.",
            );
        }
        {
            let mut window = boundary.login_window.lock().unwrap();
            if window.0.elapsed() >= Duration::from_secs(60) {
                *window = (Instant::now(), 0);
            }
            if window.1 >= 20 {
                return error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "login_throttled",
                    "Wait a minute before signing in again.",
                );
            }
            window.1 += 1;
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Login {
            owner_key: String,
        }
        let bytes =
            match tokio::time::timeout(Duration::from_secs(5), to_bytes(request.into_body(), 1024))
                .await
            {
                Ok(Ok(bytes)) => bytes,
                _ => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        "Invalid sign-in request.",
                    )
                }
            };
        let login = match serde_json::from_slice::<Login>(&bytes) {
            Ok(login) => login,
            Err(_) => return unauthorized(),
        };
        if !valid_secret(&login.owner_key)
            || !equal(&boundary.owner_digest, &digest(&login.owner_key))
        {
            return unauthorized();
        }
        let token = random_secret();
        let mut sessions = boundary.sessions.lock().unwrap();
        sessions.retain(|_, expires| *expires > Instant::now());
        if sessions.len() >= MAX_SESSIONS {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "session_limit",
                "Too many active sessions. Sign out of another tab or restart TrickTic Dictate's server.",
            );
        }
        sessions.insert(digest(&token), Instant::now() + LIFETIME);
        return Json(serde_json::json!({"api_version":"1", "data":{"token":token, "expires_in_seconds":LIFETIME.as_secs()}})).into_response();
    }
    let Some(session) = boundary.session(headers) else {
        return unauthorized();
    };
    if path == SESSION_PATH {
        return match *request.method() {
            Method::GET => {
                Json(serde_json::json!({"api_version":"1","data":{"authenticated":true}}))
                    .into_response()
            }
            Method::DELETE => {
                boundary.sessions.lock().unwrap().remove(&session);
                StatusCode::NO_CONTENT.into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }
    // Do not let inner tracing or handlers see authentication material.
    let mut request = request;
    request.headers_mut().remove(header::AUTHORIZATION);
    request.headers_mut().remove(header::COOKIE);
    next.run(request).await
}

#[cfg(test)]
mod tests;
