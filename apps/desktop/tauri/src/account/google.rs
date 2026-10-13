//! The desktop Google-to-Firebase exchange qualified by MW-OUATH-2.
//!
//! System browser, PKCE S256, random state and nonce, and a single-use
//! `http://127.0.0.1:<ephemeral-port>/callback` return that expires after three
//! minutes. Google's ID token is only an exchange input: Firebase verifies it and
//! issues the Writer session. Ported from `scripts/smoke/account-sync/auth-proof.mjs`
//! and `desktop.mjs`, which passed live on Windows and macOS.
//!
//! No token, code, verifier, callback URL or upstream body is logged or returned
//! in an error; failures collapse to the contract's fixed kinds.
use super::{Display, ErrorKind};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use reqwest::Url;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

pub const ATTEMPT_LIFETIME: Duration = Duration::from_secs(180);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_LIMIT: usize = 64 * 1024;
const CALLBACK_HEAD_LIMIT: usize = 8 * 1024;
const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
const FIREBASE_SIGN_IN: &str = "https://identitytoolkit.googleapis.com/v1/accounts:signInWithIdp";
const FIREBASE_REFRESH: &str = "https://securetoken.googleapis.com/v1/token";

/// The project's public client configuration, compiled in from the private
/// build configuration. `Debug` redacts everything but the project.
pub struct Config {
    pub project_id: String,
    api_key: String,
    client_id: String,
    /// Google issues installed-app ("Desktop") clients a secret that it does not
    /// treat as confidential; the qualified exchange sends it with PKCE.
    client_secret: Option<String>,
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}
impl Config {
    /// The configuration `scripts/desktop/build.mjs --google-sign-in` compiled in.
    pub fn embedded() -> Option<Self> {
        Self::new(
            option_env!("WRITER_GOOGLE_PROJECT_ID")?,
            option_env!("WRITER_FIREBASE_API_KEY")?,
            option_env!("WRITER_GOOGLE_DESKTOP_CLIENT_ID")?,
            option_env!("WRITER_GOOGLE_DESKTOP_CLIENT_SECRET").filter(|s| !s.is_empty()),
        )
    }
    pub fn new(
        project_id: &str,
        api_key: &str,
        client_id: &str,
        client_secret: Option<&str>,
    ) -> Option<Self> {
        let project = project_id.len() >= 6
            && project_id.len() <= 30
            && project_id.starts_with(|c: char| c.is_ascii_lowercase())
            && !project_id.ends_with('-')
            && project_id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let client = client_id
            .strip_suffix(".apps.googleusercontent.com")
            .is_some_and(|id| {
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            });
        let token = |value: &str| {
            !value.is_empty() && value.len() <= 256 && value.chars().all(|c| c.is_ascii_graphic())
        };
        (project
            && client
            && token(api_key)
            && !api_key.contains("REPLACE")
            && client_secret.is_none_or(token))
        .then(|| Self {
            project_id: project_id.into(),
            api_key: api_key.into(),
            client_id: client_id.into(),
            client_secret: client_secret.map(Into::into),
        })
    }
}

pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 244 bits from the OS generator, via two random UUIDs.
fn opaque() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    B64.encode(bytes)
}

/// One browser attempt. Consumed by its first valid-state return, whether that
/// return carries a code or a cancellation, and exchanged at most once.
pub struct Attempt {
    state: String,
    verifier: String,
    nonce: String,
    redirect_uri: String,
    expires_at: Instant,
    consumed: bool,
    exchanged: bool,
}

pub fn begin(client_id: &str, port: u16) -> (Attempt, String) {
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let attempt = Attempt {
        state: opaque(),
        verifier: opaque(),
        nonce: opaque(),
        redirect_uri,
        expires_at: Instant::now() + ATTEMPT_LIFETIME,
        consumed: false,
        exchanged: false,
    };
    let challenge = B64.encode(Sha256::digest(attempt.verifier.as_bytes()));
    let url = Url::parse_with_params(
        GOOGLE_AUTHORIZE,
        [
            ("client_id", client_id),
            ("redirect_uri", attempt.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("state", attempt.state.as_str()),
            ("nonce", attempt.nonce.as_str()),
            ("prompt", "select_account"),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
        ],
    )
    .expect("fixed authorization endpoint")
    .to_string();
    (attempt, url)
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Validates one request to the loopback listener. A request with the wrong
/// state, path or host is refused without ending the attempt, so a stray or
/// forged request cannot cancel a real sign-in; the first valid-state return
/// consumes the attempt even when it reports an error.
pub fn consume(
    attempt: &mut Attempt,
    target: &str,
    host: &str,
    now: Instant,
) -> Result<String, ErrorKind> {
    if attempt.consumed || now >= attempt.expires_at {
        return Err(ErrorKind::TimedOut);
    }
    let expected = Url::parse(&attempt.redirect_uri).map_err(|_| ErrorKind::InvalidResponse)?;
    let authority = format!(
        "{}:{}",
        expected.host_str().unwrap_or_default(),
        expected.port().unwrap_or_default()
    );
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.len() > 8192
        || host != authority
    {
        return Err(ErrorKind::InvalidResponse);
    }
    let url = expected
        .join(target)
        .map_err(|_| ErrorKind::InvalidResponse)?;
    if url.origin() != expected.origin()
        || url.path() != expected.path()
        || url.fragment().is_some()
    {
        return Err(ErrorKind::InvalidResponse);
    }
    let mut state = Vec::new();
    let mut code = Vec::new();
    let mut error = Vec::new();
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "state" => state.push(value.into_owned()),
            "code" => code.push(value.into_owned()),
            "error" => error.push(value.into_owned()),
            _ => {}
        }
    }
    if state.len() > 1 || code.len() > 1 || error.len() > 1 {
        return Err(ErrorKind::InvalidResponse);
    }
    if !same(
        state.first().map(String::as_bytes).unwrap_or_default(),
        attempt.state.as_bytes(),
    ) {
        return Err(ErrorKind::InvalidResponse);
    }
    let code = code.pop().filter(|c| !c.is_empty());
    let error = error.pop().filter(|e| !e.is_empty());
    if code.is_some() == error.is_some() || code.as_ref().is_some_and(|c| c.len() > 4096) {
        return Err(ErrorKind::InvalidResponse);
    }
    // Consume before any exchange, including a matching cancellation.
    attempt.consumed = true;
    match (code, error) {
        (Some(code), None) => Ok(code),
        (_, Some(error)) if error == "access_denied" => Err(ErrorKind::Cancelled),
        _ => Err(ErrorKind::ProviderRejected),
    }
}

const RETURNED: &str = concat!(
    "Sign-in received. You can close this tab and return to ",
    product_name!(),
    "."
);
const REFUSED: &str = concat!(
    "Sign-in was cancelled or this return was not valid. Return to ",
    product_name!(),
    "."
);

fn page(status: &str, text: &str) -> Vec<u8> {
    // Replace the callback URL, which holds the one-time code, in the tab's history.
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>{}</title><script>history.replaceState(null,\"\",\"/done\")</script><p>{}</p>",
        product_name!(),
        text
    );
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Reads one request head. `None` for anything that is not a complete, small
/// HTTP/1.x head, including a browser's idle preconnection.
async fn read_head(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut head = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    let read = async {
        loop {
            let n = stream.read(&mut buffer).await.ok()?;
            if n == 0 {
                return None;
            }
            head.extend_from_slice(&buffer[..n]);
            if head.windows(4).any(|w| w == b"\r\n\r\n") {
                return Some(());
            }
            if head.len() > CALLBACK_HEAD_LIMIT {
                return None;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .ok()??;
    let text = std::str::from_utf8(&head).ok()?;
    let mut lines = text.split("\r\n");
    let mut request = lines.next()?.split(' ');
    let (method, target, version) = (request.next()?, request.next()?, request.next()?);
    if request.next().is_some() || !version.starts_with("HTTP/1.") {
        return None;
    }
    let mut hosts = lines.filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("host")
            .then(|| value.trim().to_string())
    });
    let host = hosts.next()?;
    if hosts.next().is_some() {
        return None;
    }
    Some((method.into(), target.into(), host))
}

/// Waits for the browser to return to the loopback listener, then answers it.
/// Connections are read concurrently so an idle one cannot delay the real return.
pub async fn receive(
    listener: TcpListener,
    attempt: &mut Attempt,
    cancel: &CancellationToken,
    lifetime: Duration,
) -> Result<String, ErrorKind> {
    let (heads, mut arrivals) =
        tokio::sync::mpsc::channel::<(TcpStream, String, String, String)>(8);
    let deadline = tokio::time::sleep(lifetime);
    tokio::pin!(deadline);
    let reader = CancellationToken::new();
    let _stop_readers = reader.clone().drop_guard();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Err(ErrorKind::Cancelled),
            _ = &mut deadline => return Err(ErrorKind::TimedOut),
            accepted = listener.accept() => {
                let Ok((mut stream, _)) = accepted else { continue };
                let heads = heads.clone();
                let stop = reader.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        head = read_head(&mut stream) => {
                            if let Some((method, target, host)) = head {
                                let _ = heads.send((stream, method, target, host)).await;
                            }
                        }
                        _ = stop.cancelled() => {}
                    }
                });
            }
            Some((mut stream, method, target, host)) = arrivals.recv() => {
                let result = if method == "GET" {
                    consume(attempt, &target, &host, Instant::now())
                } else {
                    Err(ErrorKind::InvalidResponse)
                };
                let response = match &result {
                    Ok(_) => page("200 OK", RETURNED),
                    Err(_) => page("400 Bad Request", REFUSED),
                };
                let _ = tokio::time::timeout(Duration::from_secs(5), async {
                    stream.write_all(&response).await?;
                    stream.shutdown().await
                })
                .await;
                if attempt.consumed {
                    return result;
                }
            }
        }
    }
}

type Reply<'a> = Pin<Box<dyn Future<Output = Result<Value, ErrorKind>> + Send + 'a>>;
pub enum Body {
    Form(Vec<(&'static str, String)>),
    Json(Value),
}
/// One POST with a bounded JSON reply. Injected so tests never reach Google.
pub trait Transport: Send + Sync {
    fn post<'a>(&'a self, url: &'a str, body: Body) -> Reply<'a>;
}

/// Maps a provider refusal to a fixed kind without keeping its text.
fn refusal(status: u16, value: &Value) -> ErrorKind {
    let code = value["error"]["message"]
        .as_str()
        .or_else(|| value["error"].as_str())
        .unwrap_or_default();
    let code = code.split([' ', ':']).next().unwrap_or_default();
    match code {
        "INVALID_REFRESH_TOKEN" | "TOKEN_EXPIRED" | "USER_DISABLED" | "USER_NOT_FOUND" => {
            ErrorKind::ReauthRequired
        }
        _ if status == 429 || status >= 500 => ErrorKind::Unavailable,
        _ => ErrorKind::ProviderRejected,
    }
}

pub struct Http(reqwest::Client);
impl Http {
    pub fn new() -> Self {
        Self(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("static client configuration"),
        )
    }
}
impl Transport for Http {
    fn post<'a>(&'a self, url: &'a str, body: Body) -> Reply<'a> {
        Box::pin(async move {
            let request = self.0.post(url);
            let request = match body {
                Body::Form(fields) => request.form(&fields),
                Body::Json(value) => request.json(&value),
            };
            let mut response = request.send().await.map_err(|_| ErrorKind::Offline)?;
            let status = response.status();
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ErrorKind::Offline)? {
                if bytes.len() + chunk.len() > RESPONSE_LIMIT {
                    return Err(ErrorKind::InvalidResponse);
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
                if status.is_success() {
                    ErrorKind::InvalidResponse
                } else {
                    refusal(status.as_u16(), &Value::Null)
                }
            })?;
            if !status.is_success() {
                return Err(refusal(status.as_u16(), &value));
            }
            if !value.is_object() {
                return Err(ErrorKind::InvalidResponse);
            }
            Ok(value)
        })
    }
}

/// Decodes a JWT payload for consistency checks only. This is NOT a signature
/// verifier: Google's token is authenticated by Firebase's exchange, and
/// Firebase's tokens are read only from its own authenticated HTTPS reply.
fn claims(token: &str) -> Result<Value, ErrorKind> {
    let parts: Vec<_> = token.split('.').collect();
    if token.is_empty()
        || token.len() > 32768
        || parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    {
        return Err(ErrorKind::InvalidResponse);
    }
    let bytes = B64
        .decode(parts[1])
        .map_err(|_| ErrorKind::InvalidResponse)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ErrorKind::InvalidResponse)?;
    value
        .is_object()
        .then_some(value)
        .ok_or(ErrorKind::InvalidResponse)
}

fn unexpired(value: &Value) -> Option<u64> {
    value["exp"].as_u64().filter(|exp| *exp > now_seconds())
}

fn check_google(token: &str, audience: &str, nonce: &str) -> Result<(), ErrorKind> {
    let value = claims(token)?;
    let valid = value["aud"] == audience
        && matches!(
            value["iss"].as_str(),
            Some("accounts.google.com" | "https://accounts.google.com")
        )
        && value["sub"].as_str().is_some_and(|s| !s.is_empty())
        && unexpired(&value).is_some()
        && value["nonce"] == nonce;
    valid.then_some(()).ok_or(ErrorKind::InvalidResponse)
}

fn text(value: &Value, limit: usize) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= limit && !s.chars().any(char::is_control))
        .map(Into::into)
}

/// A Firebase session. `Debug` redacts both tokens.
pub struct Session {
    pub project_id: String,
    pub uid: String,
    pub id_token: String,
    pub expires_at: u64,
    /// The token's `auth_time`: the last Google sign-in, which sensitive
    /// operations require to be recent. Zero when the token had none.
    pub auth_time: u64,
    pub refresh_token: String,
    pub display: Display,
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

pub struct Client {
    config: Config,
    transport: Arc<dyn Transport>,
}
impl Client {
    pub fn new(config: Config, transport: Arc<dyn Transport>) -> Self {
        Self { config, transport }
    }
    pub fn project_id(&self) -> &str {
        &self.config.project_id
    }
    pub fn client_id(&self) -> &str {
        &self.config.client_id
    }

    /// Checks that a Firebase reply names this project, the Google provider and,
    /// on refresh, the same user.
    fn session(
        &self,
        id_token: &Value,
        refresh_token: &Value,
        local_id: &Value,
        expected_uid: Option<&str>,
    ) -> Result<(String, u64, String, String, u64), ErrorKind> {
        let id_token = text(id_token, 32768).ok_or(ErrorKind::InvalidResponse)?;
        let refresh_token = text(refresh_token, 4096).ok_or(ErrorKind::InvalidResponse)?;
        let value = claims(&id_token)?;
        let project = &self.config.project_id;
        let uid = value["sub"].as_str().unwrap_or_default().to_string();
        let expires_at = unexpired(&value).ok_or(ErrorKind::InvalidResponse)?;
        let valid = value["aud"] == project.as_str()
            && value["iss"] == format!("https://securetoken.google.com/{project}")
            && !uid.is_empty()
            && uid.len() <= 128
            && local_id.as_str() == Some(uid.as_str())
            && value["firebase"]["sign_in_provider"] == "google.com";
        if !valid || expected_uid.is_some_and(|expected| expected != uid) {
            return Err(ErrorKind::InvalidResponse);
        }
        // When Google last authenticated this user; refreshes keep it unchanged.
        let auth_time = value["auth_time"].as_u64().unwrap_or(0);
        Ok((uid, expires_at, id_token, refresh_token, auth_time))
    }

    /// Exchanges the browser's one-time code with PKCE, then asks Firebase to
    /// authenticate the resulting Google ID token.
    pub async fn exchange_code(
        &self,
        attempt: &mut Attempt,
        code: &str,
    ) -> Result<Session, ErrorKind> {
        if !attempt.consumed || attempt.exchanged || Instant::now() >= attempt.expires_at {
            return Err(ErrorKind::TimedOut);
        }
        attempt.exchanged = true;
        let mut form = vec![
            ("client_id", self.config.client_id.clone()),
            ("code", code.to_string()),
            ("code_verifier", attempt.verifier.clone()),
            ("redirect_uri", attempt.redirect_uri.clone()),
            ("grant_type", "authorization_code".to_string()),
        ];
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let google = self.transport.post(GOOGLE_TOKEN, Body::Form(form)).await?;
        let google_token = text(&google["id_token"], 32768).ok_or(ErrorKind::InvalidResponse)?;
        check_google(&google_token, &self.config.client_id, &attempt.nonce)?;
        let url = Url::parse_with_params(FIREBASE_SIGN_IN, [("key", self.config.api_key.as_str())])
            .map_err(|_| ErrorKind::NotConfigured)?;
        let mut form = Url::parse("http://localhost/").expect("static URL");
        form.query_pairs_mut()
            .append_pair("id_token", &google_token)
            .append_pair("providerId", "google.com");
        let post_body = form.query().unwrap_or_default().to_string();
        let reply = self
            .transport
            .post(
                url.as_str(),
                Body::Json(json!({
                    "requestUri": attempt.redirect_uri,
                    "postBody": post_body,
                    "returnSecureToken": true,
                    "returnIdpCredential": false,
                })),
            )
            .await?;
        if reply["needConfirmation"] == true || !reply["mfaPendingCredential"].is_null() {
            return Err(ErrorKind::ProviderRejected);
        }
        if reply["providerId"] != "google.com" {
            return Err(ErrorKind::InvalidResponse);
        }
        let (uid, expires_at, id_token, refresh_token, auth_time) = self.session(
            &reply["idToken"],
            &reply["refreshToken"],
            &reply["localId"],
            None,
        )?;
        Ok(Session {
            project_id: self.config.project_id.clone(),
            uid,
            id_token,
            expires_at,
            auth_time,
            refresh_token,
            display: Display {
                email: text(&reply["email"], 320),
                name: text(&reply["displayName"], 256),
            },
        })
    }

    /// Refreshes a Firebase session, which must remain the same user.
    pub async fn refresh(&self, refresh_token: &str, uid: &str) -> Result<Session, ErrorKind> {
        let url = Url::parse_with_params(FIREBASE_REFRESH, [("key", self.config.api_key.as_str())])
            .map_err(|_| ErrorKind::NotConfigured)?;
        let reply = self
            .transport
            .post(
                url.as_str(),
                Body::Form(vec![
                    ("grant_type", "refresh_token".into()),
                    ("refresh_token", refresh_token.into()),
                ]),
            )
            .await?;
        let (uid, expires_at, id_token, refresh_token, auth_time) = self.session(
            &reply["id_token"],
            &reply["refresh_token"],
            &reply["user_id"],
            Some(uid),
        )?;
        Ok(Session {
            project_id: self.config.project_id.clone(),
            uid,
            id_token,
            expires_at,
            auth_time,
            refresh_token,
            display: Display::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    //! Synthetic provider replies and a real loopback listener. No request
    //! reaches Google or Firebase, and no fixture is live evidence.
    use super::*;
    use crate::account::{
        vault, vault::Vault as _, Availability, EventKind, Outcome, Service, State,
    };
    use std::sync::Mutex;

    const PROJECT: &str = "metrix-writer";
    const CLIENT: &str = "synthetic-desktop.apps.googleusercontent.com";

    fn config() -> Config {
        Config::new(
            PROJECT,
            "synthetic-api-key",
            CLIENT,
            Some("synthetic-secret"),
        )
        .unwrap()
    }
    fn jwt(payload: Value) -> String {
        format!(
            "{}.{}.{}",
            B64.encode(br#"{"alg":"none"}"#),
            B64.encode(payload.to_string()),
            B64.encode(b"signature")
        )
    }
    fn google_token(nonce: &str) -> String {
        jwt(
            json!({"aud": CLIENT, "iss": "https://accounts.google.com", "sub": "google-subject", "exp": now_seconds() + 600, "nonce": nonce}),
        )
    }
    fn firebase_token(uid: &str) -> String {
        jwt(
            json!({"aud": PROJECT, "iss": format!("https://securetoken.google.com/{PROJECT}"), "sub": uid, "exp": now_seconds() + 3600, "firebase": {"sign_in_provider": "google.com"}}),
        )
    }

    /// Replays scripted replies in order and records which endpoints were used.
    #[derive(Default)]
    struct Script {
        replies: Mutex<Vec<Result<Value, ErrorKind>>>,
        calls: Mutex<Vec<String>>,
        nonce: Mutex<Option<String>>,
    }
    impl Transport for Script {
        fn post<'a>(&'a self, url: &'a str, body: Body) -> Reply<'a> {
            let endpoint = url.split('?').next().unwrap().to_string();
            let _ = body;
            self.calls.lock().unwrap().push(endpoint);
            let mut reply = self.replies.lock().unwrap().remove(0);
            if let Ok(value) = &mut reply {
                if value["id_token"] == "GOOGLE" {
                    value["id_token"] =
                        json!(google_token(self.nonce.lock().unwrap().as_deref().unwrap()));
                }
            }
            Box::pin(async move { reply })
        }
    }

    #[test]
    fn configuration_is_validated_and_redacted() {
        assert!(Config::new(PROJECT, "REPLACE_LOCALLY", CLIENT, None).is_none());
        assert!(Config::new(PROJECT, "key", "not-a-client", None).is_none());
        assert!(Config::new("Bad_Project", "key", CLIENT, None).is_none());
        assert!(Config::new(PROJECT, "key", CLIENT, Some("")).is_none());
        let text = format!("{:?}", config());
        assert!(!text.contains("synthetic-api-key") && !text.contains("synthetic-secret"));
    }

    #[test]
    fn authorization_request_uses_pkce_s256_and_loopback() {
        let (attempt, url) = begin(CLIENT, 49152);
        let url = Url::parse(&url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.host_str(), Some("accounts.google.com"));
        assert_eq!(query["redirect_uri"], "http://127.0.0.1:49152/callback");
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["code_challenge"],
            B64.encode(Sha256::digest(attempt.verifier.as_bytes()))
        );
        assert_eq!(query["scope"], "openid email profile");
        assert!(!url.as_str().contains(&attempt.verifier));
        assert_ne!(attempt.state, attempt.nonce);
        assert!(attempt.state.len() >= 43);
    }

    #[test]
    fn callback_rejects_forgery_replay_expiry_and_duplicates() {
        let host = "127.0.0.1:49152";
        let fresh = || begin(CLIENT, 49152).0;
        let mut a = fresh();
        let state = a.state.clone();
        let good = format!("/callback?state={state}&code=abc");
        assert_eq!(
            consume(&mut a, &good, "localhost:49152", Instant::now()).unwrap_err(),
            ErrorKind::InvalidResponse
        );
        assert_eq!(
            consume(
                &mut a,
                "/callback?state=wrong&code=abc",
                host,
                Instant::now()
            )
            .unwrap_err(),
            ErrorKind::InvalidResponse
        );
        assert_eq!(
            consume(
                &mut a,
                &format!("/other?state={state}&code=abc"),
                host,
                Instant::now()
            )
            .unwrap_err(),
            ErrorKind::InvalidResponse
        );
        assert_eq!(
            consume(
                &mut a,
                &format!("//evil/callback?state={state}&code=abc"),
                host,
                Instant::now()
            )
            .unwrap_err(),
            ErrorKind::InvalidResponse
        );
        let dup = format!("/callback?state={state}&state={state}&code=abc");
        assert_eq!(
            consume(&mut a, &dup, host, Instant::now()).unwrap_err(),
            ErrorKind::InvalidResponse
        );
        let both = format!("/callback?state={state}&code=abc&error=access_denied");
        assert_eq!(
            consume(&mut a, &both, host, Instant::now()).unwrap_err(),
            ErrorKind::InvalidResponse
        );
        assert!(!a.consumed, "stray requests never end a real attempt");
        assert_eq!(consume(&mut a, &good, host, Instant::now()).unwrap(), "abc");
        assert_eq!(
            consume(&mut a, &good, host, Instant::now()).unwrap_err(),
            ErrorKind::TimedOut,
            "replay"
        );

        let mut expired = fresh();
        let late = format!("/callback?state={}&code=abc", expired.state);
        assert_eq!(
            consume(&mut expired, &late, host, Instant::now() + ATTEMPT_LIFETIME).unwrap_err(),
            ErrorKind::TimedOut
        );

        let mut cancelled = fresh();
        let deny = format!("/callback?state={}&error=access_denied", cancelled.state);
        assert_eq!(
            consume(&mut cancelled, &deny, host, Instant::now()).unwrap_err(),
            ErrorKind::Cancelled
        );
        assert!(cancelled.consumed);
        let mut refused = fresh();
        let other = format!("/callback?state={}&error=server_error", refused.state);
        assert_eq!(
            consume(&mut refused, &other, host, Instant::now()).unwrap_err(),
            ErrorKind::ProviderRejected
        );
    }

    async fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(
                format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        reply
    }

    #[tokio::test]
    async fn loopback_returns_code_after_ignoring_idle_and_stray_connections() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (mut attempt, _) = begin(CLIENT, port);
        let state = attempt.state.clone();
        let cancel = CancellationToken::new();
        let browser = tokio::spawn(async move {
            // An idle preconnection must not hold up the real return.
            let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let stray = get(port, "/callback?state=forged&code=x").await;
            assert!(stray.starts_with("HTTP/1.1 400"));
            let good = get(port, &format!("/callback?state={state}&code=real-code")).await;
            assert!(good.starts_with("HTTP/1.1 200"));
            assert!(good.contains("no-store") && good.contains("no-referrer"));
            assert!(
                !good.contains("real-code"),
                "the page never echoes the code"
            );
        });
        let code = receive(listener, &mut attempt, &cancel, Duration::from_secs(30)).await;
        browser.await.unwrap();
        assert_eq!(code.unwrap(), "real-code");
    }

    #[tokio::test]
    async fn loopback_cancel_and_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let (mut attempt, _) = begin(CLIENT, listener.local_addr().unwrap().port());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            receive(listener, &mut attempt, &cancel, Duration::from_secs(30))
                .await
                .unwrap_err(),
            ErrorKind::Cancelled
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let (mut attempt, _) = begin(CLIENT, listener.local_addr().unwrap().port());
        let result = receive(
            listener,
            &mut attempt,
            &CancellationToken::new(),
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(result.unwrap_err(), ErrorKind::TimedOut);
    }

    fn consumed_attempt() -> Attempt {
        let (mut attempt, _) = begin(CLIENT, 49152);
        let target = format!("/callback?state={}&code=c", attempt.state);
        consume(&mut attempt, &target, "127.0.0.1:49152", Instant::now()).unwrap();
        attempt
    }
    fn sign_in_replies(uid: &str) -> Vec<Result<Value, ErrorKind>> {
        vec![
            Ok(json!({"id_token": "GOOGLE"})),
            Ok(
                json!({"providerId": "google.com", "idToken": firebase_token(uid), "refreshToken": "refresh-1", "localId": uid, "email": "person@example.test", "displayName": "Synthetic Person"}),
            ),
        ]
    }

    #[tokio::test]
    async fn exchange_checks_nonce_project_provider_and_single_use() {
        let script = Arc::new(Script::default());
        let client = Client::new(config(), script.clone());
        let mut attempt = consumed_attempt();
        *script.nonce.lock().unwrap() = Some(attempt.nonce.clone());
        *script.replies.lock().unwrap() = sign_in_replies("uid-a");
        let session = client.exchange_code(&mut attempt, "c").await.unwrap();
        assert_eq!(session.uid, "uid-a");
        assert_eq!(
            session.display.email.as_deref(),
            Some("person@example.test")
        );
        assert!(!format!("{session:?}").contains("refresh-1"));
        assert_eq!(
            *script.calls.lock().unwrap(),
            [GOOGLE_TOKEN, FIREBASE_SIGN_IN]
        );
        assert_eq!(
            client.exchange_code(&mut attempt, "c").await.unwrap_err(),
            ErrorKind::TimedOut,
            "code replay"
        );

        // Wrong nonce: the Google token was not minted for this attempt.
        let mut attempt = consumed_attempt();
        *script.nonce.lock().unwrap() = Some("another".into());
        *script.replies.lock().unwrap() = sign_in_replies("uid-a");
        assert_eq!(
            client.exchange_code(&mut attempt, "c").await.unwrap_err(),
            ErrorKind::InvalidResponse
        );

        // Wrong project, provider or mismatched localId.
        for reply in [
            json!({"providerId": "google.com", "idToken": jwt(json!({"aud": "other-project", "iss": "https://securetoken.google.com/other-project", "sub": "u", "exp": now_seconds() + 60, "firebase": {"sign_in_provider": "google.com"}})), "refreshToken": "r", "localId": "u"}),
            json!({"providerId": "password", "idToken": firebase_token("u"), "refreshToken": "r", "localId": "u"}),
            json!({"providerId": "google.com", "idToken": firebase_token("u"), "refreshToken": "r", "localId": "v"}),
            json!({"providerId": "google.com", "needConfirmation": true}),
        ] {
            let mut attempt = consumed_attempt();
            *script.nonce.lock().unwrap() = Some(attempt.nonce.clone());
            *script.replies.lock().unwrap() = vec![Ok(json!({"id_token": "GOOGLE"})), Ok(reply)];
            assert!(client.exchange_code(&mut attempt, "c").await.is_err());
        }
        // An unconsumed attempt can never be exchanged.
        let (mut fresh, _) = begin(CLIENT, 49152);
        assert!(client.exchange_code(&mut fresh, "c").await.is_err());
    }

    #[tokio::test]
    async fn refresh_keeps_the_same_user_and_maps_revocation() {
        let script = Arc::new(Script::default());
        let client = Client::new(config(), script.clone());
        *script.replies.lock().unwrap() = vec![Ok(
            json!({"id_token": firebase_token("uid-a"), "refresh_token": "refresh-2", "user_id": "uid-a"}),
        )];
        assert_eq!(
            client
                .refresh("refresh-1", "uid-a")
                .await
                .unwrap()
                .refresh_token,
            "refresh-2"
        );
        *script.replies.lock().unwrap() = vec![Ok(
            json!({"id_token": firebase_token("uid-b"), "refresh_token": "r", "user_id": "uid-b"}),
        )];
        assert_eq!(
            client.refresh("refresh-1", "uid-a").await.unwrap_err(),
            ErrorKind::InvalidResponse
        );
        assert_eq!(
            refusal(400, &json!({"error": {"message": "INVALID_REFRESH_TOKEN"}})),
            ErrorKind::ReauthRequired
        );
        assert_eq!(
            refusal(
                400,
                &json!({"error": {"message": "TOKEN_EXPIRED : detail"}})
            ),
            ErrorKind::ReauthRequired
        );
        assert_eq!(
            refusal(400, &json!({"error": "invalid_grant"})),
            ErrorKind::ProviderRejected
        );
        assert_eq!(refusal(503, &Value::Null), ErrorKind::Unavailable);
    }

    #[test]
    fn claims_decoder_rejects_malformed_tokens() {
        for token in [
            "",
            "a.b",
            "a.b.c.d",
            "a.%%.c",
            &format!("a.{}.c", B64.encode("[1]")),
        ] {
            assert!(claims(token).is_err(), "{token}");
        }
    }

    /// The whole service against a synthetic provider and a real loopback return:
    /// sign in, persist, restart from protected storage, refresh, sign out.
    #[tokio::test]
    async fn service_lifecycle_persists_restores_and_signs_out() {
        let script = Arc::new(Script::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let opened = Arc::new(Mutex::new(None::<String>));
        let browser = opened.clone();
        let vault = Arc::new(vault::Memory::default());
        struct Shared(Arc<vault::Memory>);
        impl vault::Vault for Shared {
            fn load(&self) -> Result<Option<vault::Stored>, ()> {
                self.0.load()
            }
            fn save(&self, s: &vault::Stored) -> Result<(), ()> {
                self.0.save(s)
            }
            fn clear(&self) -> Result<(), ()> {
                self.0.clear()
            }
        }
        let service = Service::with_google(
            Box::new(Shared(vault.clone())),
            Arc::new(move |event| {
                recorded
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(event).unwrap())
            }),
            Arc::new(move |url| {
                *browser.lock().unwrap() = Some(url.to_string());
                true
            }),
            Some(Arc::new(Client::new(config(), script.clone()))),
        );
        assert_eq!(service.snapshot().availability, Availability::Ready);
        service.start().await;
        assert!(
            script.calls.lock().unwrap().is_empty(),
            "a guest makes no request"
        );

        let attempt_id = uuid::Uuid::new_v4();
        service.sign_in(attempt_id, false).unwrap();
        let url = loop {
            if let Some(url) = opened.lock().unwrap().clone() {
                break url;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let query: std::collections::HashMap<_, _> = Url::parse(&url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        let redirect = Url::parse(&query["redirect_uri"]).unwrap();
        *script.nonce.lock().unwrap() = Some(query["nonce"].clone());
        *script.replies.lock().unwrap() = sign_in_replies("uid-a");
        let reply = get(
            redirect.port().unwrap(),
            &format!("/callback?state={}&code=real", query["state"]),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"));
        while service.snapshot().state != State::SignedIn {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let stored = vault.load().unwrap().unwrap();
        assert_eq!(
            (stored.uid.as_str(), stored.refresh_token.as_str()),
            ("uid-a", "refresh-1")
        );
        let published = serde_json::to_string(&*events.lock().unwrap()).unwrap();
        assert!(
            !published.contains("refresh-1")
                && !published.contains("real")
                && !published.contains(&query["state"])
        );
        assert!(published.contains("\"outcome\":\"success\""));

        // Restart: a new process restores from protected storage, then refreshes.
        let restarted = Service::with_google(
            Box::new(Shared(vault.clone())),
            Arc::new(|_| {}),
            Arc::new(|_| panic!("restoring never opens a browser")),
            Some(Arc::new(Client::new(config(), script.clone()))),
        );
        *script.replies.lock().unwrap() = vec![Ok(
            json!({"id_token": firebase_token("uid-a"), "refresh_token": "refresh-2", "user_id": "uid-a"}),
        )];
        restarted.start().await;
        let snapshot = restarted.snapshot();
        assert_eq!(snapshot.state, State::SignedIn);
        assert_eq!(
            snapshot.account_key.as_ref().unwrap().uid,
            "uid-a",
            "returning login keeps the UID"
        );
        while vault.load().unwrap().unwrap().refresh_token != "refresh-2" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let key = snapshot.account_key.clone().unwrap();
        assert!(
            restarted
                .id_token(&key, snapshot.generation, false)
                .await
                .is_ok(),
            "cached token"
        );
        assert_eq!(
            restarted
                .id_token(&key, snapshot.generation + 1, false)
                .await
                .unwrap_err(),
            ErrorKind::Unavailable
        );

        // Revocation moves the account to reauth_required without deleting it.
        *script.replies.lock().unwrap() = vec![Err(ErrorKind::ReauthRequired)];
        assert_eq!(
            restarted
                .id_token(&key, snapshot.generation, true)
                .await
                .unwrap_err(),
            ErrorKind::ReauthRequired
        );
        assert_eq!(restarted.snapshot().state, State::ReauthRequired);
        assert!(vault.load().unwrap().is_some());

        restarted
            .sign_out(crate::account::lifecycle::LocalCopyChoice::Leave)
            .await
            .unwrap();
        assert_eq!(restarted.snapshot().state, State::Guest);
        assert!(vault.load().unwrap().is_none());
    }

    #[tokio::test]
    async fn cancelled_service_attempt_ignores_a_late_return() {
        let script = Arc::new(Script::default());
        let opened = Arc::new(Mutex::new(None::<String>));
        let browser = opened.clone();
        let finished = Arc::new(Mutex::new(Vec::new()));
        let record = finished.clone();
        let service = Service::with_google(
            Box::new(vault::Memory::default()),
            Arc::new(move |event| {
                if let EventKind::SignInFinished { outcome, .. } = &event.kind {
                    record.lock().unwrap().push(*outcome)
                }
            }),
            Arc::new(move |url| {
                *browser.lock().unwrap() = Some(url.to_string());
                true
            }),
            Some(Arc::new(Client::new(config(), script.clone()))),
        );
        let id = uuid::Uuid::new_v4();
        service.sign_in(id, false).unwrap();
        let url = loop {
            if let Some(url) = opened.lock().unwrap().clone() {
                break url;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        service.cancel(id).unwrap();
        let query: std::collections::HashMap<_, _> = Url::parse(&url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        let port = Url::parse(&query["redirect_uri"]).unwrap().port().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The listener is gone with the attempt; a late return reaches nothing.
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
        assert_eq!(service.snapshot().state, State::Guest);
        assert_eq!(*finished.lock().unwrap(), [Outcome::Cancelled]);
        assert!(script.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn browser_failure_is_reported_without_a_session() {
        let service = Service::with_google(
            Box::new(vault::Memory::default()),
            Arc::new(|_| {}),
            Arc::new(|_| false),
            Some(Arc::new(Client::new(config(), Arc::new(Script::default())))),
        );
        service.sign_in(uuid::Uuid::new_v4(), false).unwrap();
        while service.snapshot().state == State::SigningIn {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(service.snapshot().state, State::Guest);
    }
}
