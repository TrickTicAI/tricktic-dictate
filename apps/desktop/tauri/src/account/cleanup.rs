//! Sensitive account operations (MW-OUATH-8): session revocation, cloud data
//! deletion and Writer account deletion through the trusted cleanup service
//! (`infra/firebase/trusted-cleanup.md`, "Service HTTP API v1").
//!
//! Each operation needs a Google sign-in no older than [`FRESH_SECONDS`] for
//! the same Firebase UID, so an older session is reauthenticated first. The
//! operation is bound to the account key and auth generation captured when the
//! user confirmed; if either changes, it is dropped before sending and its
//! response is never applied. A started cleanup is stored in the account's
//! workspace without any token and retried under the same operation ID.
use super::lifecycle::{Category, Fence, LocalCopyChoice, CATEGORIES};
use super::{google, AccountKey, ErrorKind, Service, State as AuthState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// The service accepts a sign-in up to 300 s old; this leaves room for the
/// request itself and some clock skew.
pub const FRESH_SECONDS: u64 = 240;
const STATUS_INTERVAL: Duration = Duration::from_secs(3);
const STATUS_ATTEMPTS: u32 = 200;
const RESPONSE_LIMIT: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const FIRESTORE: &str = "https://firestore.googleapis.com/v1";

/// What the user asked for. `Retry` resends a stored cleanup unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    RevokeSessions,
    DeleteCloud {
        categories: Vec<Category>,
        everywhere: bool,
    },
    DeleteAccount,
    Retry {
        operation_id: Uuid,
    },
}

/// Fixed failure kinds; the interface words each one itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    NotConfigured,
    ReauthRequired,
    /// The account changed, or the user cancelled reauthentication.
    Cancelled,
    Offline,
    Unavailable,
    /// The account changed on another device; preview and start again.
    Stale,
    /// The account is already being deleted.
    AccountUnavailable,
    RateLimited,
    Refused,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Progress {
    Reauthenticating,
    Sending,
    Pending,
    Running,
    FailedRetryable,
    Completed,
    Failed,
}

/// One operation's progress, published as `writer-account-task`. Never
/// carries a token, UID or service text.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub version: u8,
    pub operation_id: Uuid,
    pub action: Action,
    pub state: Progress,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
}

/// A stored cleanup: the exact request, and the last receipt the service
/// returned. Retrying sends the same request, so the service returns the same job.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Job {
    schema_version: u8,
    request: Value,
    receipt: Option<Value>,
    local_copy: LocalCopyChoice,
}

type Reply<'a> = Pin<Box<dyn Future<Output = Result<(u16, Value), Failure>> + Send + 'a>>;
/// One HTTPS request with a bounded JSON reply. Injected so tests reach no service.
pub trait Api: Send + Sync {
    fn send<'a>(
        &'a self,
        method: &'static str,
        url: String,
        bearer: &'a str,
        body: Option<Value>,
    ) -> Reply<'a>;
}

struct Http(reqwest::Client);
impl Api for Http {
    fn send<'a>(
        &'a self,
        method: &'static str,
        url: String,
        bearer: &'a str,
        body: Option<Value>,
    ) -> Reply<'a> {
        Box::pin(async move {
            let request = match method {
                "POST" => self.0.post(&url),
                _ => self.0.get(&url),
            }
            .bearer_auth(bearer);
            let request = match body {
                Some(body) => request.json(&body),
                None => request,
            };
            let mut response = request.send().await.map_err(|_| Failure::Offline)?;
            let status = response.status().as_u16();
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Offline)? {
                if bytes.len() + chunk.len() > RESPONSE_LIMIT {
                    return Err(Failure::InvalidResponse);
                }
                bytes.extend_from_slice(&chunk);
            }
            let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            Ok((status, value))
        })
    }
}

/// The service URL compiled in by `scripts/desktop/build.mjs`. Only HTTPS, or
/// plain HTTP to this computer for a local test service.
fn service_url(value: Option<&str>) -> Option<String> {
    let url = reqwest::Url::parse(value?.trim_end_matches('/')).ok()?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let secure = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    (secure && url.username().is_empty() && url.query().is_none() && url.fragment().is_none())
        .then(|| url.as_str().trim_end_matches('/').to_string())
}

struct Config {
    service: Option<String>,
    firestore: String,
    api: Arc<dyn Api>,
}

struct Op {
    key: AccountKey,
    generation: u64,
    operation_id: Uuid,
    action: Action,
    choice: LocalCopyChoice,
    /// The stored request when retrying.
    request: Option<Value>,
}

/// Per-service state for sensitive operations. One runs at a time.
pub struct State {
    config: Mutex<Config>,
    /// The operation waiting for its reauthentication attempt.
    awaiting: Mutex<Option<(Uuid, Op)>>,
    active: Mutex<Option<Uuid>>,
}
impl Default for State {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("static client configuration");
        Self {
            config: Mutex::new(Config {
                service: service_url(option_env!("WRITER_ACCOUNT_SERVICE_URL")),
                firestore: FIRESTORE.into(),
                api: Arc::new(Http(client)),
            }),
            awaiting: Mutex::new(None),
            active: Mutex::new(None),
        }
    }
}

fn failure(status: u16, body: &Value) -> Failure {
    match body["error"].as_str() {
        Some("reauth_required" | "unauthenticated") => Failure::ReauthRequired,
        Some("stale_epoch" | "stale_consent" | "idempotency_conflict") => Failure::Stale,
        Some("account_unavailable") => Failure::AccountUnavailable,
        Some("rate_limited") => Failure::RateLimited,
        Some("unavailable") => Failure::Unavailable,
        Some("not_owner" | "invalid_request" | "not_found") => Failure::Refused,
        _ if status == 429 || status >= 500 => Failure::Unavailable,
        _ => Failure::InvalidResponse,
    }
}

fn receipt_state(receipt: &Value) -> Option<Progress> {
    Some(match receipt["state"].as_str()? {
        "pending" => Progress::Pending,
        "running" => Progress::Running,
        "failed_retryable" => Progress::FailedRetryable,
        "completed" => Progress::Completed,
        _ => return None,
    })
}

fn category_name(category: Category) -> &'static str {
    match category {
        Category::Settings => "settings",
        Category::Dictionary => "dictionary",
        Category::Transcripts => "transcripts",
    }
}

/// The action a stored request stands for.
fn action_of(request: &Value) -> Option<Action> {
    match request["scope"]["kind"].as_str()? {
        "account" => Some(Action::DeleteAccount),
        "categories" => Some(Action::DeleteCloud {
            categories: serde_json::from_value(request["scope"]["categories"].clone()).ok()?,
            everywhere: request["effect"] == "everywhere",
        }),
        _ => None,
    }
}

/// The request for a new cleanup. `epoch` and `revision` come from the
/// account document just read.
fn request(op: &Op, epoch: u64, revision: u64) -> Option<Value> {
    let (scope, effect) = match &op.action {
        Action::DeleteAccount => (json!({ "kind": "account" }), "cloud_only"),
        Action::DeleteCloud {
            categories,
            everywhere,
        } => {
            // In contract order, each once.
            let chosen: Vec<_> = CATEGORIES
                .iter()
                .filter(|c| categories.contains(c))
                .map(|c| category_name(*c))
                .collect();
            if chosen.is_empty() {
                return None;
            }
            let effect = if *everywhere {
                "everywhere"
            } else {
                "cloud_only"
            };
            (
                json!({ "kind": "categories", "categories": chosen }),
                effect,
            )
        }
        _ => return None,
    };
    Some(json!({
        "schema_version": 1,
        "uid": op.key.uid,
        "account_epoch": epoch,
        "operation_id": op.operation_id,
        "expected_consent_revision": revision,
        "scope": scope,
        "effect": effect,
    }))
}

/// Firestore's typed integer, which REST returns as a decimal string.
fn integer(value: &Value) -> Option<u64> {
    value["integerValue"].as_str()?.parse().ok()
}

impl Service {
    fn task(&self, op: &Op, state: Progress, error: Option<Failure>) {
        let _ = self.inner.tasks.send(Task {
            version: 1,
            operation_id: op.operation_id,
            action: op.action.clone(),
            state,
            error,
        });
    }

    /// Every later operation update, for the shell to forward to the page.
    pub fn tasks(&self) -> tokio::sync::broadcast::Receiver<Task> {
        self.inner.tasks.subscribe()
    }

    /// Whether this build knows where the cleanup service is.
    pub fn cleanup_configured(&self) -> bool {
        self.inner.cleanup.config.lock().unwrap().service.is_some()
    }

    #[cfg(test)]
    fn configure_cleanup(&self, service: Option<&str>, firestore: &str, api: Arc<dyn Api>) {
        *self.inner.cleanup.config.lock().unwrap() = Config {
            service: service_url(service),
            firestore: firestore.into(),
            api,
        };
    }

    fn jobs_dir(&self, key: &AccountKey) -> Option<std::path::PathBuf> {
        self.workspace(key).map(|dir| dir.join("cleanup"))
    }
    fn save_job(&self, key: &AccountKey, id: Uuid, job: &Job) -> Result<(), Failure> {
        let dir = self.jobs_dir(key).ok_or(Failure::Unavailable)?;
        std::fs::create_dir_all(&dir).map_err(|_| Failure::Unavailable)?;
        crate::storage::atomic_json(&dir.join(format!("{id}.json")), job)
            .map_err(|_| Failure::Unavailable)
    }
    fn load_job(&self, key: &AccountKey, id: Uuid) -> Option<Job> {
        let bytes = std::fs::read(self.jobs_dir(key)?.join(format!("{id}.json"))).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    fn forget_job(&self, key: &AccountKey, id: Uuid) {
        if let Some(dir) = self.jobs_dir(key) {
            let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
        }
    }

    /// The current account's unfinished cleanups, so the page can offer Retry.
    pub fn unfinished(&self) -> Vec<Task> {
        let Some(key) = self.snapshot().account_key else {
            return Vec::new();
        };
        let Some(entries) = self.jobs_dir(&key).and_then(|d| std::fs::read_dir(d).ok()) else {
            return Vec::new();
        };
        let mut tasks: Vec<Task> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let id = Uuid::parse_str(name.strip_suffix(".json")?).ok()?;
                let job = self.load_job(&key, id)?;
                let state = job
                    .receipt
                    .as_ref()
                    .and_then(receipt_state)
                    .unwrap_or(Progress::Sending);
                let action = action_of(&job.request)?;
                (state != Progress::Completed).then_some(Task {
                    version: 1,
                    operation_id: id,
                    action,
                    state,
                    error: None,
                })
            })
            .collect();
        tasks.sort_by_key(|t| t.operation_id);
        tasks
    }

    /// A Firebase ID token proving a Google sign-in within [`FRESH_SECONDS`],
    /// for exactly this account and generation. Never leaves native code.
    fn fresh_token(&self, key: &AccountKey, generation: u64) -> Option<String> {
        let machine = self.inner.machine.lock().unwrap();
        if !machine.current(key, generation) {
            return None;
        }
        let live = self.inner.live.lock().unwrap();
        let live = live.as_ref().filter(|live| &live.key == key)?;
        let now = google::now_seconds();
        let (token, expires_at) = live.id_token.as_ref()?;
        (live.auth_time + FRESH_SECONDS >= now && *expires_at > now + 60).then(|| token.clone())
    }

    fn still(&self, op: &Op) -> bool {
        self.inner
            .machine
            .lock()
            .unwrap()
            .current(&op.key, op.generation)
    }

    /// Starts a sensitive operation for the signed-in account. Reauthenticates
    /// first unless Google's last sign-in is recent; `attempt_id` names that
    /// browser attempt. Progress arrives as tasks; returns the operation ID.
    pub fn sensitive(
        &self,
        attempt_id: Uuid,
        action: Action,
        choice: LocalCopyChoice,
    ) -> Result<Uuid, ErrorKind> {
        let machine = self.inner.machine.lock().unwrap();
        let google = self.inner.google.clone().ok_or(ErrorKind::NotConfigured)?;
        if !self.cleanup_configured() {
            return Err(ErrorKind::NotConfigured);
        }
        let snapshot = machine.snapshot();
        let key = snapshot.account_key.clone().ok_or(ErrorKind::Unavailable)?;
        if snapshot.attempt_id.is_some()
            || self.inner.cleanup.awaiting.lock().unwrap().is_some()
            || self.inner.cleanup.active.lock().unwrap().is_some()
        {
            return Err(ErrorKind::Unavailable);
        }
        let op = match action {
            Action::Retry { operation_id } => {
                let job = self
                    .load_job(&key, operation_id)
                    .ok_or(ErrorKind::Unavailable)?;
                Op {
                    action: action_of(&job.request).ok_or(ErrorKind::Unavailable)?,
                    request: Some(job.request),
                    choice: job.local_copy,
                    operation_id,
                    key,
                    generation: snapshot.generation,
                }
            }
            Action::DeleteCloud { ref categories, .. } if categories.is_empty() => {
                return Err(ErrorKind::Unavailable)
            }
            action => Op {
                operation_id: Uuid::new_v4(),
                action,
                choice,
                request: None,
                key,
                generation: snapshot.generation,
            },
        };
        let operation_id = op.operation_id;
        drop(machine);
        if snapshot.state == AuthState::SignedIn {
            if let Some(token) = self.fresh_token(&op.key, op.generation) {
                self.begin_operation(op, token);
                return Ok(operation_id);
            }
        }
        let mut machine = self.inner.machine.lock().unwrap();
        // The account may have changed while the lock was released.
        if !machine.snapshot().account_key.is_some_and(|k| k == op.key) {
            return Err(ErrorKind::Unavailable);
        }
        let (ticket, events) = machine.begin(attempt_id, true)?;
        self.task(&op, Progress::Reauthenticating, None);
        *self.inner.cleanup.awaiting.lock().unwrap() = Some((attempt_id, op));
        self.launch(google, ticket, events);
        Ok(operation_id)
    }

    /// Continues the operation waiting on `attempt_id` once its reauthentication settled.
    pub(super) fn reauthenticated(&self, attempt_id: Uuid, succeeded: bool) {
        let waiting = {
            let mut awaiting = self.inner.cleanup.awaiting.lock().unwrap();
            match awaiting.take() {
                Some((id, op)) if id == attempt_id => Some(op),
                other => {
                    *awaiting = other;
                    None
                }
            }
        };
        let Some(op) = waiting else { return };
        // Reauthentication keeps the generation, so the captured binding still holds.
        match self
            .fresh_token(&op.key, op.generation)
            .filter(|_| succeeded)
        {
            Some(token) => self.begin_operation(op, token),
            None => self.task(&op, Progress::Failed, Some(Failure::Cancelled)),
        }
    }

    /// Drops an operation still waiting for reauthentication. Called with the
    /// account lock held, when the account is signed out or switched.
    pub(super) fn cancel_sensitive(&self) {
        if let Some((_, op)) = self.inner.cleanup.awaiting.lock().unwrap().take() {
            self.task(&op, Progress::Failed, Some(Failure::Cancelled));
        }
    }

    fn begin_operation(&self, op: Op, token: String) {
        *self.inner.cleanup.active.lock().unwrap() = Some(op.operation_id);
        self.task(&op, Progress::Sending, None);
        let service = self.clone();
        tokio::spawn(async move {
            let outcome = service.run(&op, &token).await;
            *service.inner.cleanup.active.lock().unwrap() = None;
            match outcome {
                Ok(state) => service.task(&op, state, None),
                Err(kind) => service.task(&op, Progress::Failed, Some(kind)),
            }
        });
    }

    async fn call(
        &self,
        method: &'static str,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value, Failure> {
        let (url, api) = {
            let config = self.inner.cleanup.config.lock().unwrap();
            let base = config.service.clone().ok_or(Failure::NotConfigured)?;
            (format!("{base}{path}"), config.api.clone())
        };
        let (status, body) = api.send(method, url, token, body).await?;
        if !(200..300).contains(&status) {
            return Err(failure(status, &body));
        }
        body.is_object()
            .then_some(body)
            .ok_or(Failure::InvalidResponse)
    }

    /// Reads the account document's epoch and consent revision. An account
    /// that never enabled sync has none: the service treats it as the initial one.
    async fn account(&self, key: &AccountKey, token: &str) -> Result<(u64, u64), Failure> {
        let (url, api) = {
            let config = self.inner.cleanup.config.lock().unwrap();
            let mut url =
                reqwest::Url::parse(&config.firestore).map_err(|_| Failure::NotConfigured)?;
            url.path_segments_mut()
                .map_err(|_| Failure::NotConfigured)?
                .extend([
                    "projects",
                    &key.project_id,
                    "databases",
                    "(default)",
                    "documents",
                    "users",
                    &key.uid,
                ]);
            (url.to_string(), config.api.clone())
        };
        let (status, body) = api.send("GET", url, token, None).await?;
        match status {
            404 => return Ok((1, 1)),
            401 | 403 => return Err(Failure::ReauthRequired),
            200 => {}
            _ if status == 429 || status >= 500 => return Err(Failure::Unavailable),
            _ => return Err(Failure::InvalidResponse),
        }
        let fields = &body["fields"];
        if fields["account_state"]["stringValue"] != "active" {
            return Err(Failure::AccountUnavailable);
        }
        let epoch = integer(&fields["account_epoch"]).ok_or(Failure::InvalidResponse)?;
        let revision = integer(&fields["consent"]["mapValue"]["fields"]["revision"])
            .ok_or(Failure::InvalidResponse)?;
        Ok((epoch, revision))
    }

    async fn run(&self, op: &Op, token: &str) -> Result<Progress, Failure> {
        if matches!(op.action, Action::RevokeSessions) {
            let body = json!({ "schema_version": 1, "uid": op.key.uid });
            let reply = self
                .call("POST", "/v1/sessions/revoke", token, Some(body))
                .await?;
            if reply["revoked"] != true {
                return Err(Failure::InvalidResponse);
            }
            // This device's session is one of the revoked ones.
            self.leave(op).await?;
            return Ok(Progress::Completed);
        }
        let request = match &op.request {
            Some(request) => request.clone(),
            None => {
                let (epoch, revision) = self.account(&op.key, token).await?;
                let request = request(op, epoch, revision).ok_or(Failure::Refused)?;
                // Stored before sending, so a lost reply is retried, not restarted.
                let job = Job {
                    schema_version: 1,
                    request: request.clone(),
                    receipt: None,
                    local_copy: op.choice,
                };
                self.save_job(&op.key, op.operation_id, &job)?;
                request
            }
        };
        if !self.still(op) {
            return Err(Failure::Cancelled);
        }
        let reply = self
            .call("POST", "/v1/cleanup", token, Some(request.clone()))
            .await;
        let mut receipt = match reply {
            Ok(reply) => reply["receipt"].clone(),
            Err(kind @ (Failure::Stale | Failure::Refused | Failure::AccountUnavailable)) => {
                // Never accepted, and resending it cannot succeed.
                self.forget_job(&op.key, op.operation_id);
                return Err(kind);
            }
            Err(kind) => return Err(kind),
        };
        for attempt in 0.. {
            let state = self.accept(op, &request, &receipt)?;
            if state != Progress::Pending && state != Progress::Running {
                if state == Progress::Completed && op.action == Action::DeleteAccount {
                    self.leave(op).await?;
                }
                return Ok(state);
            }
            if attempt >= STATUS_ATTEMPTS {
                return Ok(state);
            }
            self.task(op, state, None);
            tokio::time::sleep(STATUS_INTERVAL).await;
            let path = format!("/v1/cleanup/{}", op.operation_id);
            receipt = self.call("GET", &path, token, None).await?["receipt"].clone();
        }
        unreachable!()
    }

    /// Checks a receipt against the request and the captured account, stores
    /// it, and applies its fence to this device's sync participants.
    fn accept(&self, op: &Op, request: &Value, receipt: &Value) -> Result<Progress, Failure> {
        let state = receipt_state(receipt).ok_or(Failure::InvalidResponse)?;
        if receipt["operation_id"] != request["operation_id"] || receipt["uid"] != request["uid"] {
            return Err(Failure::InvalidResponse);
        }
        // Never apply a response under another account or generation. What
        // follows only touches this account's own workspace and participants.
        if !self.still(op) {
            return Err(Failure::Cancelled);
        }
        let job = Job {
            schema_version: 1,
            request: request.clone(),
            receipt: Some(receipt.clone()),
            local_copy: op.choice,
        };
        self.save_job(&op.key, op.operation_id, &job)?;
        let everywhere = request["effect"] == "everywhere";
        let fenced: Vec<Category> = match &op.action {
            Action::DeleteCloud { categories, .. } => categories.clone(),
            _ => CATEGORIES.to_vec(),
        };
        if op.action == Action::DeleteAccount {
            self.inner.participants.suspend(&op.key);
        }
        for category in fenced {
            let generation =
                receipt["fenced_consent"][category_name(category)]["generation"].as_u64();
            let Some(generation) = generation else {
                continue;
            };
            let fence = Fence {
                category,
                generation,
                everywhere,
            };
            // A participant that cannot apply it now does so on reconnect,
            // when it reads the same fence from the account document.
            let _ = self.inner.participants.apply_fence(&op.key, &fence);
        }
        Ok(state)
    }

    /// Signs this device out after a revocation or a completed account deletion.
    async fn leave(&self, op: &Op) -> Result<(), Failure> {
        if !self.still(op) {
            return Err(Failure::Cancelled);
        }
        let (previous, _) = self.sign_out_now();
        if let Some(previous) = previous {
            let service = self.clone();
            let choice = op.choice;
            let _ = tokio::task::spawn_blocking(move || service.settle(&previous, choice)).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! A scripted service and Firestore. No request leaves the process.
    use super::*;
    use crate::account::lifecycle::{self, Participant};
    use crate::account::{vault, Display, Live};
    use std::collections::VecDeque;

    const PROJECT: &str = "metrix-writer";

    enum Scripted {
        Fixed(Result<(u16, Value), Failure>),
        /// A receipt for the last cleanup request sent, in this state.
        Receipt(&'static str, u64),
    }

    #[derive(Default)]
    struct Script {
        replies: Mutex<VecDeque<Scripted>>,
        calls: Mutex<Vec<(String, String, Option<Value>)>>,
        request: Mutex<Option<Value>>,
    }
    impl Script {
        fn reply(&self, status: u16, body: Value) {
            self.replies
                .lock()
                .unwrap()
                .push_back(Scripted::Fixed(Ok((status, body))));
        }
        fn fail(&self, kind: Failure) {
            self.replies
                .lock()
                .unwrap()
                .push_back(Scripted::Fixed(Err(kind)));
        }
        fn receipt(&self, state: &'static str, generation: u64) {
            self.replies
                .lock()
                .unwrap()
                .push_back(Scripted::Receipt(state, generation));
        }
        fn calls(&self) -> Vec<(String, String, Option<Value>)> {
            self.calls.lock().unwrap().clone()
        }
        fn cleanup_requests(&self) -> Vec<Value> {
            self.calls()
                .into_iter()
                .filter(|(m, path, _)| m == "POST" && path == "/v1/cleanup")
                .filter_map(|(_, _, body)| body)
                .collect()
        }
    }
    impl Api for Script {
        fn send<'a>(
            &'a self,
            method: &'static str,
            url: String,
            bearer: &'a str,
            body: Option<Value>,
        ) -> Reply<'a> {
            assert_eq!(bearer, "fresh-token");
            let path = url
                .replace("https://cleanup.test", "")
                .replace("https://firestore.test", "firestore:");
            if path == "/v1/cleanup" {
                *self.request.lock().unwrap() = body.clone();
            }
            self.calls.lock().unwrap().push((method.into(), path, body));
            let reply = match self.replies.lock().unwrap().pop_front() {
                Some(Scripted::Fixed(reply)) => reply,
                Some(Scripted::Receipt(state, generation)) => Ok((
                    200,
                    receipt(
                        self.request.lock().unwrap().as_ref().unwrap(),
                        state,
                        generation,
                    ),
                )),
                None => Err(Failure::Offline),
            };
            Box::pin(async move { reply })
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);
    impl Participant for Recorder {
        fn suspend(&self, _: &AccountKey) {
            self.0.lock().unwrap().push("suspend".into());
        }
        fn local_copy(&self, _: &AccountKey) -> Result<lifecycle::LocalCopy, ()> {
            Ok(Default::default())
        }
        fn keep_local_copy(&self, _: &AccountKey) -> Result<(), ()> {
            self.0.lock().unwrap().push("keep".into());
            Ok(())
        }
        fn remove_local_copy(&self, _: &AccountKey) -> Result<(), ()> {
            self.0.lock().unwrap().push("remove".into());
            Ok(())
        }
        fn apply_fence(&self, _: &AccountKey, fence: &Fence) -> Result<(), ()> {
            let log = format!(
                "fence {:?} {} {}",
                fence.category, fence.generation, fence.everywhere
            );
            self.0.lock().unwrap().push(log);
            Ok(())
        }
    }

    fn key(uid: &str) -> AccountKey {
        AccountKey {
            project_id: PROJECT.into(),
            uid: uid.into(),
        }
    }

    struct Fixture {
        service: Service,
        script: Arc<Script>,
        recorder: Arc<Recorder>,
        tasks: tokio::sync::broadcast::Receiver<Task>,
        _dir: tempfile::TempDir,
    }
    impl Fixture {
        /// The next final state: completed, failed or failed_retryable.
        async fn settled(&mut self) -> Task {
            loop {
                let task = tokio::time::timeout(Duration::from_secs(5), self.tasks.recv())
                    .await
                    .unwrap()
                    .unwrap();
                if matches!(
                    task.state,
                    Progress::Completed | Progress::Failed | Progress::FailedRetryable
                ) {
                    return task;
                }
            }
        }
        fn log(&self) -> Vec<String> {
            self.recorder.0.lock().unwrap().clone()
        }
    }

    /// A signed-in service whose last Google sign-in was `age` seconds ago.
    fn fixture(age: u64) -> Fixture {
        let config = google::Config::new(
            PROJECT,
            "synthetic-api-key",
            "synthetic.apps.googleusercontent.com",
            None,
        )
        .unwrap();
        let google = Arc::new(google::Client::new(config, Arc::new(google::Http::new())));
        let service = Service::with_google(
            Box::new(vault::Memory::default()),
            Arc::new(|_| {}),
            // The browser "opens" but never returns, so a reauthentication stays pending.
            Arc::new(|_| true),
            Some(google),
        );
        let dir = tempfile::tempdir().unwrap();
        service.set_workspace(lifecycle::Workspace::new(dir.path()));
        let script = Arc::new(Script::default());
        service.configure_cleanup(
            Some("https://cleanup.test"),
            "https://firestore.test",
            script.clone(),
        );
        let recorder = Arc::new(Recorder::default());
        service.register(recorder.clone());
        let now = google::now_seconds();
        service
            .inner
            .machine
            .lock()
            .unwrap()
            .restore(key("uid-a"), Display::default());
        *service.inner.live.lock().unwrap() = Some(Live {
            key: key("uid-a"),
            display: Display::default(),
            refresh_token: "refresh".into(),
            id_token: Some(("fresh-token".into(), now + 3600)),
            auth_time: now - age,
        });
        let tasks = service.tasks();
        Fixture {
            service,
            script,
            recorder,
            tasks,
            _dir: dir,
        }
    }

    fn receipt(request: &Value, state: &str, generation: u64) -> Value {
        json!({ "receipt": {
            "schema_version": 1, "uid": request["uid"], "account_epoch": request["account_epoch"],
            "operation_id": request["operation_id"], "job_id": Uuid::new_v4(), "state": state,
            "fenced_account_epoch": 1, "scope": request["scope"], "effect": request["effect"],
            "fenced_consent": { "settings": { "generation": generation }, "dictionary": { "generation": generation },
                "transcripts": { "generation": generation } },
        }})
    }

    fn account_document(epoch: u64, revision: u64) -> Value {
        json!({ "fields": {
            "account_state": { "stringValue": "active" },
            "account_epoch": { "integerValue": epoch.to_string() },
            "consent": { "mapValue": { "fields": { "revision": { "integerValue": revision.to_string() } } } },
        }})
    }

    fn delete(categories: &[Category], everywhere: bool) -> Action {
        Action::DeleteCloud {
            categories: categories.to_vec(),
            everywhere,
        }
    }

    #[test]
    fn only_https_or_loopback_service_urls_are_accepted() {
        assert_eq!(
            service_url(Some("https://cleanup.example/")).as_deref(),
            Some("https://cleanup.example")
        );
        assert!(service_url(Some("http://127.0.0.1:8787")).is_some());
        assert!(service_url(Some("http://cleanup.example")).is_none());
        assert!(service_url(Some("https://user@cleanup.example")).is_none());
        assert!(service_url(Some("https://cleanup.example/?x=1")).is_none());
        assert!(service_url(None).is_none());
    }

    #[tokio::test]
    async fn category_deletion_reads_the_account_stores_the_job_and_applies_the_fence() {
        let mut f = fixture(10);
        f.script.reply(200, account_document(3, 7));
        f.script.receipt("completed", 4);
        let action = delete(&[Category::Dictionary, Category::Settings], true);
        let id = f
            .service
            .sensitive(Uuid::new_v4(), action, LocalCopyChoice::Leave)
            .unwrap();
        let task = f.settled().await;
        assert_eq!(task.state, Progress::Completed);
        let calls = f.script.calls();
        assert_eq!(
            calls[0].1,
            "firestore:/projects/metrix-writer/databases/(default)/documents/users/uid-a"
        );
        let request = &f.script.cleanup_requests()[0];
        assert_eq!(request["account_epoch"], 3);
        assert_eq!(request["expected_consent_revision"], 7);
        assert_eq!(
            request["scope"]["categories"],
            json!(["settings", "dictionary"])
        );
        assert_eq!(request["effect"], "everywhere");
        assert_eq!(request["operation_id"], json!(id));
        let log = f.log();
        assert!(log.contains(&"fence Settings 4 true".to_string()));
        assert!(log.contains(&"fence Dictionary 4 true".to_string()));
        assert!(!log
            .iter()
            .any(|l| l.contains("Transcripts") || l == "suspend"));
        assert!(f.service.unfinished().is_empty());
        assert_eq!(f.service.snapshot().state, AuthState::SignedIn);
    }

    #[tokio::test]
    async fn a_lost_reply_is_retried_under_the_same_operation() {
        let mut f = fixture(10);
        f.script
            .reply(404, json!({ "error": { "status": "NOT_FOUND" } }));
        f.script.fail(Failure::Offline);
        let id = f
            .service
            .sensitive(
                Uuid::new_v4(),
                delete(&[Category::Transcripts], false),
                LocalCopyChoice::Leave,
            )
            .unwrap();
        let task = f.settled().await;
        assert_eq!(
            (task.state, task.error),
            (Progress::Failed, Some(Failure::Offline))
        );
        let sent = f.script.cleanup_requests()[0].clone();
        // No account document: the initial account.
        assert_eq!(
            (
                sent["account_epoch"].as_u64(),
                sent["expected_consent_revision"].as_u64()
            ),
            (Some(1), Some(1))
        );
        let unfinished = f.service.unfinished();
        assert_eq!((unfinished.len(), unfinished[0].operation_id), (1, id));

        f.script.receipt("failed_retryable", 2);
        let retried = f
            .service
            .sensitive(
                Uuid::new_v4(),
                Action::Retry { operation_id: id },
                LocalCopyChoice::Remove,
            )
            .unwrap();
        assert_eq!(retried, id);
        let task = f.settled().await;
        assert_eq!(task.state, Progress::FailedRetryable);
        assert_eq!(
            f.script.calls().len(),
            3,
            "a retry never rereads the account or makes a new request"
        );
        assert_eq!(f.script.cleanup_requests(), [sent.clone(), sent]);
        assert_eq!(f.service.unfinished()[0].state, Progress::FailedRetryable);
    }

    #[tokio::test]
    async fn account_deletion_polls_to_completion_then_signs_out_with_the_choice() {
        let mut f = fixture(10);
        f.script.reply(200, account_document(1, 2));
        f.script.receipt("pending", 2);
        f.script.receipt("running", 2);
        f.script.receipt("completed", 2);
        let id = f
            .service
            .sensitive(
                Uuid::new_v4(),
                Action::DeleteAccount,
                LocalCopyChoice::Remove,
            )
            .unwrap();
        let task = tokio::time::timeout(Duration::from_secs(15), f.settled())
            .await
            .unwrap();
        assert_eq!(task.state, Progress::Completed);
        let request = &f.script.cleanup_requests()[0];
        assert_eq!(request["scope"], json!({ "kind": "account" }));
        assert_eq!(request["effect"], "cloud_only");
        let calls = f.script.calls();
        assert_eq!(calls.len(), 4);
        assert!(calls[2..]
            .iter()
            .all(|(m, path, _)| m == "GET" && path == &format!("/v1/cleanup/{id}")));
        assert_eq!(f.service.snapshot().state, AuthState::Guest);
        let log = f.log();
        let suspended = log.iter().position(|l| l == "suspend").unwrap();
        let removed = log.iter().position(|l| l == "remove").unwrap();
        assert!(
            suspended < removed,
            "sync stops before anything is removed: {log:?}"
        );
        assert!(log.contains(&"fence Transcripts 2 false".to_string()));
    }

    #[tokio::test]
    async fn revocation_signs_this_device_out() {
        let mut f = fixture(10);
        f.script.reply(200, json!({ "revoked": true }));
        f.service
            .sensitive(
                Uuid::new_v4(),
                Action::RevokeSessions,
                LocalCopyChoice::Keep,
            )
            .unwrap();
        let task = f.settled().await;
        assert_eq!(task.state, Progress::Completed);
        assert_eq!(f.script.calls()[0].1, "/v1/sessions/revoke");
        assert_eq!(
            f.script.calls()[0].2,
            Some(json!({ "schema_version": 1, "uid": "uid-a" }))
        );
        assert_eq!(f.service.snapshot().state, AuthState::Guest);
        assert_eq!(f.log(), ["suspend", "keep"]);
    }

    #[tokio::test]
    async fn an_old_sign_in_reauthenticates_first_and_sign_out_cancels_it() {
        let mut f = fixture(FRESH_SECONDS + 5);
        let attempt = Uuid::new_v4();
        f.service
            .sensitive(attempt, Action::RevokeSessions, LocalCopyChoice::Leave)
            .unwrap();
        assert_eq!(
            f.tasks.recv().await.unwrap().state,
            Progress::Reauthenticating
        );
        let snapshot = f.service.snapshot();
        assert!(snapshot.reauthenticating);
        assert_eq!(snapshot.attempt_id, Some(attempt));
        assert!(
            f.script.calls().is_empty(),
            "nothing is sent before reauthentication"
        );
        f.service.sign_out(LocalCopyChoice::Leave).await.unwrap();
        let task = f.settled().await;
        assert_eq!(
            (task.state, task.error),
            (Progress::Failed, Some(Failure::Cancelled))
        );
        assert!(f.script.calls().is_empty());
    }

    #[tokio::test]
    async fn a_successful_reauthentication_continues_the_operation() {
        let mut f = fixture(FRESH_SECONDS + 5);
        let attempt = Uuid::new_v4();
        f.service
            .sensitive(attempt, Action::RevokeSessions, LocalCopyChoice::Leave)
            .unwrap();
        let ticket = crate::account::Ticket {
            attempt_id: attempt,
            generation: f.service.snapshot().generation,
            expected: Some(key("uid-a")),
        };
        f.script.reply(200, json!({ "revoked": true }));
        let now = google::now_seconds();
        f.service.finish(
            &ticket,
            Ok(google::Session {
                project_id: PROJECT.into(),
                uid: "uid-a".into(),
                id_token: "fresh-token".into(),
                expires_at: now + 3600,
                auth_time: now,
                refresh_token: "rotated".into(),
                display: Display::default(),
            }),
        );
        let task = f.settled().await;
        assert_eq!(task.state, Progress::Completed);
        assert_eq!(f.script.calls().len(), 1);
    }

    #[tokio::test]
    async fn another_account_in_the_reauthentication_authorizes_nothing() {
        let mut f = fixture(FRESH_SECONDS + 5);
        let attempt = Uuid::new_v4();
        f.service
            .sensitive(attempt, Action::DeleteAccount, LocalCopyChoice::Leave)
            .unwrap();
        let ticket = crate::account::Ticket {
            attempt_id: attempt,
            generation: f.service.snapshot().generation,
            expected: Some(key("uid-a")),
        };
        let now = google::now_seconds();
        f.service.finish(
            &ticket,
            Ok(google::Session {
                project_id: PROJECT.into(),
                uid: "uid-b".into(),
                id_token: "fresh-token".into(),
                expires_at: now + 3600,
                auth_time: now,
                refresh_token: "other".into(),
                display: Display::default(),
            }),
        );
        let task = f.settled().await;
        assert_eq!(
            (task.state, task.error),
            (Progress::Failed, Some(Failure::Cancelled))
        );
        assert!(f.script.calls().is_empty());
        assert_eq!(f.service.snapshot().account_key, Some(key("uid-a")));
    }

    #[tokio::test]
    async fn a_reply_after_switching_accounts_is_never_applied() {
        let f = fixture(10);
        f.script.receipt("completed", 2);
        let request = json!({
            "schema_version": 1, "uid": "uid-a", "account_epoch": 1, "operation_id": Uuid::new_v4(),
            "expected_consent_revision": 1, "scope": { "kind": "categories", "categories": ["settings"] },
            "effect": "cloud_only",
        });
        let op = Op {
            key: key("uid-a"),
            generation: f.service.snapshot().generation,
            operation_id: serde_json::from_value(request["operation_id"].clone()).unwrap(),
            action: delete(&[Category::Settings], false),
            choice: LocalCopyChoice::Leave,
            request: Some(request),
        };
        // Account A leaves and B arrives before A's operation runs.
        f.service.sign_out(LocalCopyChoice::Leave).await.unwrap();
        f.service
            .inner
            .machine
            .lock()
            .unwrap()
            .restore(key("uid-b"), Display::default());
        assert_eq!(
            f.service.run(&op, "fresh-token").await,
            Err(Failure::Cancelled)
        );
        assert!(f.script.calls().is_empty(), "nothing is sent for A under B");
        assert!(!f.log().iter().any(|l| l.starts_with("fence")));
    }

    #[tokio::test]
    async fn refused_requests_are_forgotten_and_need_a_new_preview() {
        let mut f = fixture(10);
        f.script.reply(200, account_document(1, 1));
        f.script.reply(409, json!({ "error": "stale_consent" }));
        f.service
            .sensitive(
                Uuid::new_v4(),
                delete(&[Category::Settings], false),
                LocalCopyChoice::Leave,
            )
            .unwrap();
        let task = f.settled().await;
        assert_eq!(
            (task.state, task.error),
            (Progress::Failed, Some(Failure::Stale))
        );
        assert!(f.service.unfinished().is_empty());
    }

    #[tokio::test]
    async fn a_deleting_account_is_not_asked_again() {
        let mut f = fixture(10);
        let mut document = account_document(2, 3);
        document["fields"]["account_state"] = json!({ "stringValue": "deleting" });
        f.script.reply(200, document);
        f.service
            .sensitive(
                Uuid::new_v4(),
                Action::DeleteAccount,
                LocalCopyChoice::Leave,
            )
            .unwrap();
        let task = f.settled().await;
        assert_eq!(task.error, Some(Failure::AccountUnavailable));
        assert_eq!(f.script.calls().len(), 1);
    }

    #[tokio::test]
    async fn switching_suspends_a_removes_its_session_and_starts_b() {
        let f = fixture(10);
        let stored = vault::Stored::new(&key("uid-a"), &Display::default(), "refresh");
        f.service.inner.vault.save(&stored).unwrap();
        let before = f.service.snapshot().generation;
        let attempt = Uuid::new_v4();
        f.service
            .switch_account(attempt, LocalCopyChoice::Remove)
            .await
            .unwrap();
        assert_eq!(f.log(), ["suspend", "remove"]);
        assert!(f.service.inner.vault.load().unwrap().is_none());
        assert!(f.service.inner.live.lock().unwrap().is_none());
        let snapshot = f.service.snapshot();
        assert_eq!(
            (
                snapshot.state,
                snapshot.attempt_id,
                snapshot.reauthenticating
            ),
            (AuthState::SigningIn, Some(attempt), false)
        );
        assert!(snapshot.account_key.is_none());
        assert_eq!(
            f.service
                .id_token(&key("uid-a"), before, false)
                .await
                .unwrap_err(),
            ErrorKind::Unavailable
        );
    }

    #[tokio::test]
    async fn an_unconfigured_build_refuses_and_sends_nothing() {
        let f = fixture(10);
        f.service
            .configure_cleanup(None, "https://firestore.test", f.script.clone());
        assert_eq!(
            f.service.sensitive(
                Uuid::new_v4(),
                Action::DeleteAccount,
                LocalCopyChoice::Leave
            ),
            Err(ErrorKind::NotConfigured)
        );
        assert!(f.script.calls().is_empty());
    }
}
