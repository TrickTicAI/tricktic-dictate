//! Optional Google sign-in through Firebase Authentication (MW-OUATH-3).
//!
//! The desktop half of `infra/firebase/auth-adapter-contract.md`. Nothing here
//! depends on Tauri, so the state machine, the browser return and the token
//! checks run under `cargo test` on Linux. Account-free use never reaches the
//! network code: a guest snapshot makes no request, and signing in alone never
//! enables sync or uploads anything.
//!
//! Sign-in is compiled only with the `google-sign-in` feature. Without it the
//! service reports `unavailable` and refuses every operation in optional
//! account-free development builds. Published desktop builds require the feature.
// An account-free build compiles the types without the flow; Linux has no shell.
#![cfg_attr(
    not(all(feature = "google-sign-in", any(windows, target_os = "macos"))),
    allow(dead_code)
)]
#[cfg(feature = "google-sign-in")]
pub mod cleanup;
#[cfg(feature = "google-sign-in")]
pub mod google;
pub mod lifecycle;
pub mod vault;

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub const EVENT_VERSION: u8 = 1;

/// What the interface may show. Never carries a token or provider message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Guest,
    SigningIn,
    SignedIn,
    ReauthRequired,
}

/// The contract's fixed error kinds; the UI words each one itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotConfigured,
    Cancelled,
    TimedOut,
    Offline,
    ProviderRejected,
    InvalidResponse,
    ReauthRequired,
    Unavailable,
}
impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Offline => "offline",
            Self::ProviderRejected => "provider_rejected",
            Self::InvalidResponse => "invalid_response",
            Self::ReauthRequired => "reauth_required",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Whether this build can sign in at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Compiled without sign-in: account-free development builds.
    #[cfg_attr(feature = "google-sign-in", allow(dead_code))]
    Unavailable,
    /// Compiled with sign-in but built without the project's client configuration.
    NotConfigured,
    Ready,
}

/// The account identity: Firebase's UID within one project, never an email.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountKey {
    pub project_id: String,
    pub uid: String,
}

/// Presentation only. Never used to identify or merge accounts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub email: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub state: State,
    pub availability: Availability,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_key: Option<AccountKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<Display>,
    /// The sign-in attempt in progress, so a reloaded page can still cancel it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<Uuid>,
    /// Whether the attempt in progress re-authenticates the current account.
    pub reauthenticating: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Cancelled,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Invalidation {
    SignedOut,
    ReauthRequired,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum EventKind {
    AuthSnapshot {
        snapshot: Snapshot,
    },
    SignInStarted {
        attempt_id: Uuid,
        reauthenticate: bool,
    },
    SignInFinished {
        attempt_id: Uuid,
        outcome: Outcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorKind>,
    },
    SessionRefreshed {
        account_key: AccountKey,
    },
    SessionInvalidated {
        account_key: AccountKey,
        reason: Invalidation,
    },
}

/// One ordered lifecycle event. `sequence` orders events within this process;
/// `generation` advances whenever a session or pending sign-in is invalidated.
/// Neither is a durable sync consent generation (those belong to MW-OUATH-1).
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub version: u8,
    pub sequence: u64,
    pub generation: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// A pending sign-in, captured with the generation it started in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub attempt_id: Uuid,
    pub generation: u64,
    /// For re-authentication: the account the result must match.
    pub expected: Option<AccountKey>,
}

struct Pending {
    ticket: Ticket,
    resume: State,
}

/// The pure account state machine. Every transition returns the events it
/// produced, in order; the caller publishes them.
pub struct Machine {
    availability: Availability,
    state: State,
    generation: u64,
    sequence: u64,
    account: Option<(AccountKey, Display)>,
    pending: Option<Pending>,
}

impl Machine {
    pub fn new(availability: Availability) -> Self {
        Self {
            availability,
            state: State::Guest,
            generation: 1,
            sequence: 0,
            account: None,
            pending: None,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state,
            availability: self.availability,
            generation: self.generation,
            account_key: self.account.as_ref().map(|(key, _)| key.clone()),
            display: self.account.as_ref().map(|(_, display)| display.clone()),
            attempt_id: self.pending.as_ref().map(|p| p.ticket.attempt_id),
            reauthenticating: self
                .pending
                .as_ref()
                .is_some_and(|p| p.ticket.expected.is_some()),
        }
    }
    fn event(&mut self, kind: EventKind) -> Event {
        self.sequence += 1;
        Event {
            version: EVENT_VERSION,
            sequence: self.sequence,
            generation: self.generation,
            kind,
        }
    }
    fn snapshot_event(&mut self) -> Event {
        let snapshot = self.snapshot();
        self.event(EventKind::AuthSnapshot { snapshot })
    }
    /// The current state as an ordered event, for a listener that just attached.
    pub fn announce(&mut self) -> Vec<Event> {
        vec![self.snapshot_event()]
    }
    /// Adopts a session read back from protected storage at startup.
    pub fn restore(&mut self, key: AccountKey, display: Display) -> Vec<Event> {
        if self.state != State::Guest || self.pending.is_some() {
            return Vec::new();
        }
        self.account = Some((key, display));
        self.state = State::SignedIn;
        vec![self.snapshot_event()]
    }
    pub fn begin(
        &mut self,
        attempt_id: Uuid,
        reauthenticate: bool,
    ) -> Result<(Ticket, Vec<Event>), ErrorKind> {
        match self.availability {
            Availability::Unavailable => return Err(ErrorKind::Unavailable),
            Availability::NotConfigured => return Err(ErrorKind::NotConfigured),
            Availability::Ready => {}
        }
        if self.pending.is_some() {
            return Err(ErrorKind::Unavailable);
        }
        let expected = if reauthenticate {
            match (&self.account, self.state) {
                (Some((key, _)), State::SignedIn | State::ReauthRequired) => Some(key.clone()),
                _ => return Err(ErrorKind::Unavailable),
            }
        } else if self.state == State::Guest {
            None
        } else {
            // Sign out A before signing in as B.
            return Err(ErrorKind::Unavailable);
        };
        let ticket = Ticket {
            attempt_id,
            generation: self.generation,
            expected,
        };
        self.pending = Some(Pending {
            ticket: ticket.clone(),
            resume: self.state,
        });
        self.state = State::SigningIn;
        let started = self.event(EventKind::SignInStarted {
            attempt_id,
            reauthenticate,
        });
        let snapshot = self.snapshot_event();
        Ok((ticket, vec![started, snapshot]))
    }
    pub fn is_current(&self, ticket: &Ticket) -> bool {
        self.generation == ticket.generation
            && self.pending.as_ref().is_some_and(|p| &p.ticket == ticket)
    }
    fn settle(
        &mut self,
        attempt_id: Uuid,
        outcome: Outcome,
        error: Option<ErrorKind>,
    ) -> Vec<Event> {
        let finished = self.event(EventKind::SignInFinished {
            attempt_id,
            outcome,
            error,
        });
        vec![finished, self.snapshot_event()]
    }
    /// Cancels only the named attempt. Its late result can no longer establish
    /// a session, because the generation it captured is gone.
    pub fn cancel(&mut self, attempt_id: Uuid) -> Vec<Event> {
        let Some(pending) = self.pending.take_if(|p| p.ticket.attempt_id == attempt_id) else {
            return Vec::new();
        };
        self.generation += 1;
        self.state = pending.resume;
        self.settle(attempt_id, Outcome::Cancelled, None)
    }
    pub fn fail(&mut self, ticket: &Ticket, kind: ErrorKind) -> Vec<Event> {
        if !self.is_current(ticket) {
            return Vec::new();
        }
        let pending = self.pending.take().expect("current ticket is pending");
        self.state = pending.resume;
        if kind == ErrorKind::Cancelled {
            // A cancellation on Google's page ends the pending login too.
            self.generation += 1;
            self.settle(ticket.attempt_id, Outcome::Cancelled, None)
        } else {
            self.settle(ticket.attempt_id, Outcome::Error, Some(kind))
        }
    }
    /// Checks a result before it is persisted. Re-authentication must return the
    /// same Firebase account; choosing another one authorizes nothing.
    pub fn accepts(&self, ticket: &Ticket, key: &AccountKey) -> Result<(), ErrorKind> {
        if !self.is_current(ticket) {
            return Err(ErrorKind::Cancelled);
        }
        match &ticket.expected {
            Some(expected) if expected != key => Err(ErrorKind::ReauthRequired),
            _ => Ok(()),
        }
    }
    pub fn complete(&mut self, ticket: &Ticket, key: AccountKey, display: Display) -> Vec<Event> {
        if self.accepts(ticket, &key).is_err() {
            return Vec::new();
        }
        self.pending = None;
        self.account = Some((key, display));
        self.state = State::SignedIn;
        self.settle(ticket.attempt_id, Outcome::Success, None)
    }
    /// Invalidates the session before anything else happens. Pending account
    /// work captured the old generation and is rejected from here on.
    pub fn sign_out(&mut self) -> Vec<Event> {
        let pending = self.pending.take();
        let previous = self.account.take();
        if pending.is_none() && previous.is_none() {
            return Vec::new();
        }
        self.generation += 1;
        self.state = State::Guest;
        let mut events = Vec::new();
        if let Some((account_key, _)) = previous {
            events.push(self.event(EventKind::SessionInvalidated {
                account_key,
                reason: Invalidation::SignedOut,
            }));
        }
        if let Some(pending) = pending {
            events.push(self.event(EventKind::SignInFinished {
                attempt_id: pending.ticket.attempt_id,
                outcome: Outcome::Cancelled,
                error: None,
            }));
        }
        events.push(self.snapshot_event());
        events
    }
    /// Signs the current account out and starts a sign-in from guest in one
    /// transition, so nothing can run under A once B's attempt exists. Returns
    /// the attempt's ticket, every event in order and the account left behind.
    pub fn switch(
        &mut self,
        attempt_id: Uuid,
    ) -> Result<(Ticket, Vec<Event>, AccountKey), ErrorKind> {
        match self.availability {
            Availability::Unavailable => return Err(ErrorKind::Unavailable),
            Availability::NotConfigured => return Err(ErrorKind::NotConfigured),
            Availability::Ready => {}
        }
        let previous = match (&self.account, &self.pending) {
            (Some((key, _)), None) => key.clone(),
            _ => return Err(ErrorKind::Unavailable),
        };
        let mut events = self.sign_out();
        let (ticket, started) = self.begin(attempt_id, false)?;
        events.extend(started);
        Ok((ticket, events, previous))
    }
    /// Whether work captured for this account and generation may still proceed.
    pub fn current(&self, key: &AccountKey, generation: u64) -> bool {
        let signed_in = self.state == State::SignedIn
            || (self.state == State::SigningIn
                && self
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.resume == State::SignedIn));
        signed_in
            && self.generation == generation
            && self.account.as_ref().is_some_and(|(k, _)| k == key)
    }
    pub fn refreshed(&mut self, key: &AccountKey, generation: u64) -> Vec<Event> {
        if !self.current(key, generation) {
            return Vec::new();
        }
        vec![self.event(EventKind::SessionRefreshed {
            account_key: key.clone(),
        })]
    }
    /// Firebase refused the stored credential. Keeps the account visible so the
    /// same user can sign in again; its old work is invalidated.
    pub fn reauth_required(&mut self, key: &AccountKey, generation: u64) -> Vec<Event> {
        if !self.current(key, generation) {
            return Vec::new();
        }
        self.generation += 1;
        if let Some(pending) = self.pending.as_mut() {
            pending.resume = State::ReauthRequired;
        } else {
            self.state = State::ReauthRequired;
        }
        let invalidated = self.event(EventKind::SessionInvalidated {
            account_key: key.clone(),
            reason: Invalidation::ReauthRequired,
        });
        vec![invalidated, self.snapshot_event()]
    }
}

/// Publishes events. Called with the account lock held so listeners observe
/// the exact transition order; it must not call back into the service.
pub type Emit = Arc<dyn Fn(&Event) + Send + Sync>;
/// Opens a URL in the system browser; `false` when no browser could be started.
pub type Opener = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[cfg(feature = "google-sign-in")]
#[derive(Clone)]
struct Live {
    key: AccountKey,
    display: Display,
    refresh_token: String,
    id_token: Option<(String, u64)>,
    /// The last Google sign-in for this session, from Firebase's token.
    auth_time: u64,
}

struct Inner {
    machine: Mutex<Machine>,
    vault: Box<dyn vault::Vault>,
    emit: Emit,
    events: tokio::sync::broadcast::Sender<Event>,
    #[cfg(feature = "google-sign-in")]
    google: Option<Arc<google::Client>>,
    #[cfg(feature = "google-sign-in")]
    opener: Opener,
    #[cfg(feature = "google-sign-in")]
    attempt: Mutex<Option<(Uuid, tokio_util::sync::CancellationToken)>>,
    #[cfg(feature = "google-sign-in")]
    live: Mutex<Option<Live>>,
    /// Serializes token refreshes so one rotation cannot overwrite another.
    #[cfg(feature = "google-sign-in")]
    refreshing: tokio::sync::Mutex<()>,
    participants: lifecycle::Participants,
    workspace: Mutex<Option<lifecycle::Workspace>>,
    #[cfg(feature = "google-sign-in")]
    tasks: tokio::sync::broadcast::Sender<cleanup::Task>,
    #[cfg(feature = "google-sign-in")]
    cleanup: cleanup::State,
}

/// The account service the shell and later sync adapters share.
#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
}

impl Service {
    /// The build's service. Without the `google-sign-in` feature, or without the
    /// project configuration compiled in, sign-in is refused with a fixed kind.
    pub fn new(vault: Box<dyn vault::Vault>, emit: Emit, opener: Opener) -> Self {
        #[cfg(feature = "google-sign-in")]
        let google = google::Config::embedded()
            .map(|config| Arc::new(google::Client::new(config, Arc::new(google::Http::new()))));
        #[cfg(feature = "google-sign-in")]
        return Self::with_google(vault, emit, opener, google);
        #[cfg(not(feature = "google-sign-in"))]
        {
            let _ = opener;
            Self::build(vault, emit, Availability::Unavailable)
        }
    }
    #[cfg(feature = "google-sign-in")]
    pub fn with_google(
        vault: Box<dyn vault::Vault>,
        emit: Emit,
        opener: Opener,
        google: Option<Arc<google::Client>>,
    ) -> Self {
        let availability = if google.is_some() {
            Availability::Ready
        } else {
            Availability::NotConfigured
        };
        let (events, _) = tokio::sync::broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                machine: Mutex::new(Machine::new(availability)),
                vault,
                emit,
                events,
                google,
                opener,
                attempt: Mutex::new(None),
                live: Mutex::new(None),
                refreshing: tokio::sync::Mutex::new(()),
                participants: Default::default(),
                workspace: Mutex::new(None),
                tasks: tokio::sync::broadcast::channel(64).0,
                cleanup: Default::default(),
            }),
        }
    }
    #[cfg(not(feature = "google-sign-in"))]
    fn build(vault: Box<dyn vault::Vault>, emit: Emit, availability: Availability) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                machine: Mutex::new(Machine::new(availability)),
                vault,
                emit,
                events,
                participants: Default::default(),
                workspace: Mutex::new(None),
            }),
        }
    }
    fn publish(&self, events: Vec<Event>) {
        for event in events {
            (self.inner.emit)(&event);
            let _ = self.inner.events.send(event);
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.inner.machine.lock().unwrap().snapshot()
    }
    /// The current snapshot plus every later event, for a Rust consumer such as
    /// a sync adapter. A `signed_in` event never implies sync consent.
    #[allow(dead_code)] // For the MW-OUATH-6 sync adapter.
    pub fn subscribe(&self) -> (Snapshot, tokio::sync::broadcast::Receiver<Event>) {
        let machine = self.inner.machine.lock().unwrap();
        (machine.snapshot(), self.inner.events.subscribe())
    }

    /// Where sync adapters keep per-account data, below the app's data root.
    pub fn set_workspace(&self, workspace: lifecycle::Workspace) {
        *self.inner.workspace.lock().unwrap() = Some(workspace);
    }
    #[allow(dead_code)] // For the MW-OUATH-6/9 sync adapters.
    pub fn workspace(&self, key: &AccountKey) -> Option<std::path::PathBuf> {
        self.inner
            .workspace
            .lock()
            .unwrap()
            .as_ref()
            .map(|w| w.dir(key))
    }
    /// Registers a sync adapter's lifecycle hooks.
    #[allow(dead_code)] // For the MW-OUATH-6/9 sync adapters.
    pub fn register(&self, participant: Arc<dyn lifecycle::Participant>) {
        self.inner.participants.register(participant);
    }
    /// What the current account keeps on this device, for the sign-out dialog.
    pub fn local_copy(&self) -> Result<lifecycle::LocalCopy, ErrorKind> {
        let key = self.snapshot().account_key;
        match key {
            None => Ok(lifecycle::LocalCopy::default()),
            Some(key) => self
                .inner
                .participants
                .local_copy(&key)
                .map_err(|_| ErrorKind::Unavailable),
        }
    }
    /// Applies a local-copy choice for an account that is already invalidated.
    fn settle(
        &self,
        key: &AccountKey,
        choice: lifecycle::LocalCopyChoice,
    ) -> Result<(), ErrorKind> {
        self.inner
            .participants
            .settle(key, choice)
            .map_err(|_| ErrorKind::Unavailable)
    }

    /// Restores a protected session. Only the storage is validated before the
    /// restored state is published; a background refresh then confirms it with
    /// Firebase, and a refusal moves the account to `reauth_required`.
    pub async fn start(&self) {
        #[cfg(feature = "google-sign-in")]
        if let Some(google) = self.inner.google.clone() {
            let service = self.clone();
            let loaded = tokio::task::spawn_blocking(move || service.inner.vault.load())
                .await
                .unwrap_or(Err(()));
            let mut machine = self.inner.machine.lock().unwrap();
            match loaded {
                Ok(Some(stored)) if stored.project_id == google.project_id() => {
                    let key = AccountKey {
                        project_id: stored.project_id.clone(),
                        uid: stored.uid.clone(),
                    };
                    let display = Display {
                        email: stored.email.clone(),
                        name: stored.name.clone(),
                    };
                    let events = machine.restore(key.clone(), display.clone());
                    if !events.is_empty() {
                        *self.inner.live.lock().unwrap() = Some(Live {
                            key: key.clone(),
                            display,
                            refresh_token: stored.refresh_token,
                            id_token: None,
                            auth_time: 0,
                        });
                        let generation = machine.snapshot().generation;
                        self.publish(events);
                        drop(machine);
                        let service = self.clone();
                        tokio::spawn(async move {
                            let _ = service.id_token(&key, generation, true).await;
                        });
                        return;
                    }
                }
                Ok(None) => {}
                // Another project's session, or one this user can no longer
                // decrypt: it can never be used, so it is not kept.
                Ok(Some(_)) | Err(()) => {
                    let _ = self.inner.vault.clear();
                }
            }
            let events = machine.announce();
            self.publish(events);
            return;
        }
        let events = self.inner.machine.lock().unwrap().announce();
        self.publish(events);
    }

    /// Starts one explicit sign-in in the system browser. Returns once the
    /// attempt is registered; its result arrives as `sign_in_finished`.
    pub fn sign_in(&self, attempt_id: Uuid, reauthenticate: bool) -> Result<(), ErrorKind> {
        #[cfg(feature = "google-sign-in")]
        {
            let mut machine = self.inner.machine.lock().unwrap();
            let google = self.inner.google.clone().ok_or(ErrorKind::NotConfigured)?;
            let (ticket, events) = machine.begin(attempt_id, reauthenticate)?;
            self.launch(google, ticket, events);
            Ok(())
        }
        #[cfg(not(feature = "google-sign-in"))]
        {
            let _ = (attempt_id, reauthenticate);
            Err(ErrorKind::Unavailable)
        }
    }

    /// Publishes a started attempt and runs it in the background. Called with
    /// the account lock held, so the attempt is registered before anyone can
    /// cancel it.
    #[cfg(feature = "google-sign-in")]
    fn launch(&self, google: Arc<google::Client>, ticket: Ticket, events: Vec<Event>) {
        let token = tokio_util::sync::CancellationToken::new();
        *self.inner.attempt.lock().unwrap() = Some((ticket.attempt_id, token.clone()));
        self.publish(events);
        let service = self.clone();
        tokio::spawn(async move {
            let result = service.attempt(&google, token).await;
            service.finish(&ticket, result);
        });
    }

    /// Switches accounts: A is suspended and invalidated, its protected session
    /// removed, and only then does B's browser sign-in start. A's local-copy
    /// choice is applied after the invalidation is published.
    pub async fn switch_account(
        &self,
        attempt_id: Uuid,
        choice: lifecycle::LocalCopyChoice,
    ) -> Result<(), ErrorKind> {
        #[cfg(feature = "google-sign-in")]
        {
            let previous = {
                let mut machine = self.inner.machine.lock().unwrap();
                let google = self.inner.google.clone().ok_or(ErrorKind::NotConfigured)?;
                let leaving = machine
                    .snapshot()
                    .account_key
                    .ok_or(ErrorKind::Unavailable)?;
                if machine.snapshot().attempt_id.is_some() {
                    return Err(ErrorKind::Unavailable);
                }
                self.inner.participants.suspend(&leaving);
                let (ticket, events, previous) = machine.switch(attempt_id)?;
                *self.inner.live.lock().unwrap() = None;
                self.cancel_sensitive();
                if self.inner.vault.clear().is_err() {
                    // The old account is already invalidated in memory; a stale
                    // protected copy is removed again at the next sign-out.
                    self.publish(events);
                    return Err(ErrorKind::Unavailable);
                }
                self.launch(google, ticket, events);
                previous
            };
            let service = self.clone();
            tokio::task::spawn_blocking(move || service.settle(&previous, choice))
                .await
                .unwrap_or(Err(ErrorKind::Unavailable))
        }
        #[cfg(not(feature = "google-sign-in"))]
        {
            let _ = (attempt_id, choice);
            Err(ErrorKind::Unavailable)
        }
    }

    #[cfg(feature = "google-sign-in")]
    async fn attempt(
        &self,
        google: &google::Client,
        token: tokio_util::sync::CancellationToken,
    ) -> Result<google::Session, ErrorKind> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| ErrorKind::Unavailable)?;
        let port = listener
            .local_addr()
            .map_err(|_| ErrorKind::Unavailable)?
            .port();
        let (mut attempt, url) = google::begin(google.client_id(), port);
        let opener = self.inner.opener.clone();
        let opened = tokio::task::spawn_blocking(move || opener(&url))
            .await
            .unwrap_or(false);
        if !opened {
            return Err(ErrorKind::Unavailable);
        }
        let code =
            google::receive(listener, &mut attempt, &token, google::ATTEMPT_LIFETIME).await?;
        tokio::select! {
            result = google.exchange_code(&mut attempt, &code) => result,
            _ = token.cancelled() => Err(ErrorKind::Cancelled),
        }
    }

    /// Commits a sign-in result only if its attempt is still the current one.
    /// The session is persisted before success is reported.
    #[cfg(feature = "google-sign-in")]
    fn finish(&self, ticket: &Ticket, result: Result<google::Session, ErrorKind>) {
        let mut machine = self.inner.machine.lock().unwrap();
        {
            let mut attempt = self.inner.attempt.lock().unwrap();
            if attempt
                .as_ref()
                .is_some_and(|(id, _)| *id == ticket.attempt_id)
            {
                *attempt = None;
            }
        }
        if !machine.is_current(ticket) {
            return;
        }
        let events = match result {
            Err(kind) => machine.fail(ticket, kind),
            Ok(session) => {
                let key = AccountKey {
                    project_id: session.project_id.clone(),
                    uid: session.uid.clone(),
                };
                match machine.accepts(ticket, &key) {
                    Err(kind) => machine.fail(ticket, kind),
                    Ok(()) => {
                        let stored =
                            vault::Stored::new(&key, &session.display, &session.refresh_token);
                        if self.inner.vault.save(&stored).is_err() {
                            machine.fail(ticket, ErrorKind::Unavailable)
                        } else {
                            *self.inner.live.lock().unwrap() = Some(Live {
                                key: key.clone(),
                                display: session.display.clone(),
                                refresh_token: session.refresh_token,
                                id_token: Some((session.id_token, session.expires_at)),
                                auth_time: session.auth_time,
                            });
                            machine.complete(ticket, key, session.display)
                        }
                    }
                }
            }
        };
        let succeeded = events.iter().any(|e| {
            matches!(
                e.kind,
                EventKind::SignInFinished {
                    outcome: Outcome::Success,
                    ..
                }
            )
        });
        self.publish(events);
        drop(machine);
        self.reauthenticated(ticket.attempt_id, succeeded);
    }

    pub fn cancel(&self, attempt_id: Uuid) -> Result<(), ErrorKind> {
        #[cfg(feature = "google-sign-in")]
        {
            let mut machine = self.inner.machine.lock().unwrap();
            if let Some((id, token)) = self.inner.attempt.lock().unwrap().as_ref() {
                if *id == attempt_id {
                    token.cancel();
                }
            }
            let events = machine.cancel(attempt_id);
            self.publish(events);
            Ok(())
        }
        #[cfg(not(feature = "google-sign-in"))]
        {
            let _ = attempt_id;
            Err(ErrorKind::Unavailable)
        }
    }

    /// Suspends the account's sync, invalidates the session, then removes the
    /// protected copy. Local recordings, transcripts and settings are untouched;
    /// the account's downloaded copies follow `choice` afterwards.
    pub async fn sign_out(&self, choice: lifecycle::LocalCopyChoice) -> Result<(), ErrorKind> {
        #[cfg(feature = "google-sign-in")]
        {
            let (previous, cleared) = self.sign_out_now();
            let settled = match previous {
                Some(previous) => {
                    let service = self.clone();
                    tokio::task::spawn_blocking(move || service.settle(&previous, choice))
                        .await
                        .unwrap_or(Err(ErrorKind::Unavailable))
                }
                None => Ok(()),
            };
            cleared.and(settled)
        }
        #[cfg(not(feature = "google-sign-in"))]
        {
            let _ = choice;
            Err(ErrorKind::Unavailable)
        }
    }

    /// The synchronous part of signing out: nothing after it can act as the
    /// old account. Returns that account, if any, and whether its protected
    /// copy was removed.
    #[cfg(feature = "google-sign-in")]
    fn sign_out_now(&self) -> (Option<AccountKey>, Result<(), ErrorKind>) {
        let mut machine = self.inner.machine.lock().unwrap();
        if let Some((_, token)) = self.inner.attempt.lock().unwrap().take() {
            token.cancel();
        }
        let previous = machine.snapshot().account_key;
        if let Some(previous) = &previous {
            self.inner.participants.suspend(previous);
        }
        let events = machine.sign_out();
        *self.inner.live.lock().unwrap() = None;
        self.cancel_sensitive();
        let cleared = self.inner.vault.clear().map_err(|_| ErrorKind::Unavailable);
        self.publish(events);
        (previous, cleared)
    }

    /// A Firebase ID token for exactly this account and generation. Native and
    /// internal only: it never crosses to the WebView, and it never returns a
    /// Google token. A refresh never enables sync or changes data categories.
    #[cfg(feature = "google-sign-in")]
    pub async fn id_token(
        &self,
        key: &AccountKey,
        generation: u64,
        force_refresh: bool,
    ) -> Result<String, ErrorKind> {
        let google = self.inner.google.clone().ok_or(ErrorKind::NotConfigured)?;
        let _serial = self.inner.refreshing.lock().await;
        let live = {
            let machine = self.inner.machine.lock().unwrap();
            if !machine.current(key, generation) {
                return Err(ErrorKind::Unavailable);
            }
            self.inner.live.lock().unwrap().clone()
        }
        .filter(|live| &live.key == key)
        .ok_or(ErrorKind::ReauthRequired)?;
        if !force_refresh {
            if let Some((token, expires_at)) = &live.id_token {
                if *expires_at > google::now_seconds() + 300 {
                    return Ok(token.clone());
                }
            }
        }
        let refreshed = google.refresh(&live.refresh_token, &key.uid).await;
        let mut machine = self.inner.machine.lock().unwrap();
        // Signed out or switched while the request was in flight: never adopt it.
        if !machine.current(key, generation) {
            return Err(ErrorKind::Unavailable);
        }
        match refreshed {
            Ok(session) => {
                if session.refresh_token != live.refresh_token {
                    let stored = vault::Stored::new(key, &live.display, &session.refresh_token);
                    self.inner
                        .vault
                        .save(&stored)
                        .map_err(|_| ErrorKind::Unavailable)?;
                }
                *self.inner.live.lock().unwrap() = Some(Live {
                    refresh_token: session.refresh_token,
                    id_token: Some((session.id_token.clone(), session.expires_at)),
                    auth_time: session.auth_time.max(live.auth_time),
                    ..live
                });
                let events = machine.refreshed(key, generation);
                self.publish(events);
                Ok(session.id_token)
            }
            Err(ErrorKind::ReauthRequired) => {
                if let Some(live) = self.inner.live.lock().unwrap().as_mut() {
                    live.id_token = None;
                }
                self.inner.participants.suspend(key);
                let events = machine.reauth_required(key, generation);
                self.publish(events);
                Err(ErrorKind::ReauthRequired)
            }
            Err(kind) => Err(kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(uid: &str) -> AccountKey {
        AccountKey {
            project_id: "metrix-writer".into(),
            uid: uid.into(),
        }
    }
    fn kinds(events: &[Event]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match &e.kind {
                EventKind::AuthSnapshot { .. } => "auth_snapshot",
                EventKind::SignInStarted { .. } => "sign_in_started",
                EventKind::SignInFinished { .. } => "sign_in_finished",
                EventKind::SessionRefreshed { .. } => "session_refreshed",
                EventKind::SessionInvalidated { .. } => "session_invalidated",
            })
            .collect()
    }

    #[test]
    fn unavailable_and_unconfigured_builds_refuse_sign_in() {
        let mut public = Machine::new(Availability::Unavailable);
        assert_eq!(
            public.begin(Uuid::new_v4(), false).unwrap_err(),
            ErrorKind::Unavailable
        );
        let mut bare = Machine::new(Availability::NotConfigured);
        assert_eq!(
            bare.begin(Uuid::new_v4(), false).unwrap_err(),
            ErrorKind::NotConfigured
        );
        assert_eq!(public.snapshot().state, State::Guest);
    }

    #[test]
    fn sign_in_success_is_ordered_and_leaves_sync_to_consent() {
        let mut m = Machine::new(Availability::Ready);
        let id = Uuid::new_v4();
        let (ticket, events) = m.begin(id, false).unwrap();
        assert_eq!(kinds(&events), ["sign_in_started", "auth_snapshot"]);
        assert_eq!(m.snapshot().state, State::SigningIn);
        assert_eq!(m.snapshot().attempt_id, Some(id));
        let events = m.complete(&ticket, key("a"), Display::default());
        assert_eq!(kinds(&events), ["sign_in_finished", "auth_snapshot"]);
        assert!(events.windows(2).all(|w| w[0].sequence < w[1].sequence));
        let snapshot = m.snapshot();
        assert_eq!(snapshot.state, State::SignedIn);
        assert_eq!(snapshot.account_key, Some(key("a")));
        let value = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(value["type"], "auth_snapshot");
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["snapshot"]["accountKey"]["projectId"],
            "metrix-writer"
        );
        // The event vocabulary has no sync switch: nothing here can enable uploads.
        assert!(!value.to_string().contains("sync"));
    }

    #[test]
    fn one_attempt_at_a_time_and_no_switch_without_sign_out() {
        let mut m = Machine::new(Availability::Ready);
        let (ticket, _) = m.begin(Uuid::new_v4(), false).unwrap();
        assert_eq!(
            m.begin(Uuid::new_v4(), false).unwrap_err(),
            ErrorKind::Unavailable
        );
        m.complete(&ticket, key("a"), Display::default());
        assert_eq!(
            m.begin(Uuid::new_v4(), false).unwrap_err(),
            ErrorKind::Unavailable
        );
    }

    #[test]
    fn cancelled_attempt_cannot_complete_late() {
        let mut m = Machine::new(Availability::Ready);
        let id = Uuid::new_v4();
        let (ticket, _) = m.begin(id, false).unwrap();
        let before = m.snapshot().generation;
        assert!(
            m.cancel(Uuid::new_v4()).is_empty(),
            "only the named attempt"
        );
        let events = m.cancel(id);
        assert_eq!(kinds(&events), ["sign_in_finished", "auth_snapshot"]);
        assert!(m.snapshot().generation > before);
        assert!(!m.is_current(&ticket));
        assert!(m.complete(&ticket, key("a"), Display::default()).is_empty());
        assert_eq!(m.snapshot().state, State::Guest);
        assert!(m.fail(&ticket, ErrorKind::TimedOut).is_empty());
    }

    #[test]
    fn failure_returns_to_previous_state_with_fixed_kind() {
        let mut m = Machine::new(Availability::Ready);
        let (ticket, _) = m.begin(Uuid::new_v4(), false).unwrap();
        let events = m.fail(&ticket, ErrorKind::Offline);
        let value = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(value["outcome"], "error");
        assert_eq!(value["error"], "offline");
        assert_eq!(m.snapshot().state, State::Guest);
        // A cancellation reported by Google's page is a cancellation, not an error.
        let (ticket, _) = m.begin(Uuid::new_v4(), false).unwrap();
        let generation = m.snapshot().generation;
        let events = m.fail(&ticket, ErrorKind::Cancelled);
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap()["outcome"],
            "cancelled"
        );
        assert!(m.snapshot().generation > generation);
    }

    #[test]
    fn sign_out_invalidates_before_publishing_guest() {
        let mut m = Machine::new(Availability::Ready);
        m.restore(key("a"), Display::default());
        let generation = m.snapshot().generation;
        assert!(m.current(&key("a"), generation));
        let events = m.sign_out();
        assert_eq!(kinds(&events), ["session_invalidated", "auth_snapshot"]);
        assert!(!m.current(&key("a"), generation));
        assert_eq!(m.snapshot().state, State::Guest);
        assert!(m.snapshot().account_key.is_none());
        assert!(m.refreshed(&key("a"), generation).is_empty());
        assert!(m.sign_out().is_empty(), "signing out twice changes nothing");
    }

    #[test]
    fn switching_invalidates_a_before_b_can_start() {
        let mut m = Machine::new(Availability::Ready);
        assert_eq!(
            m.switch(Uuid::new_v4()).unwrap_err(),
            ErrorKind::Unavailable
        );
        let (ticket, _) = m.begin(Uuid::new_v4(), false).unwrap();
        m.complete(&ticket, key("a"), Display::default());
        let before = m.snapshot().generation;
        let attempt = Uuid::new_v4();
        let (b, events, previous) = m.switch(attempt).unwrap();
        assert_eq!(previous, key("a"));
        assert_eq!(
            kinds(&events),
            [
                "session_invalidated",
                "auth_snapshot",
                "sign_in_started",
                "auth_snapshot"
            ]
        );
        assert!(events.windows(2).all(|w| w[0].sequence < w[1].sequence));
        assert!(b.generation > before && b.expected.is_none());
        // A's work is stale, and the attempt in progress cannot be switched again.
        assert!(!m.current(&key("a"), before));
        assert_eq!(
            m.switch(Uuid::new_v4()).unwrap_err(),
            ErrorKind::Unavailable
        );
        let snapshot = m.snapshot();
        assert_eq!(
            (snapshot.state, snapshot.account_key),
            (State::SigningIn, None)
        );
        m.complete(&b, key("b"), Display::default());
        assert!(m.current(&key("b"), b.generation));
        assert!(!m.current(&key("a"), b.generation));
    }

    #[test]
    fn switching_is_refused_in_builds_without_sign_in() {
        assert_eq!(
            Machine::new(Availability::NotConfigured)
                .switch(Uuid::new_v4())
                .unwrap_err(),
            ErrorKind::NotConfigured
        );
    }

    #[test]
    fn sign_out_during_pending_sign_in_rejects_its_result() {
        let mut m = Machine::new(Availability::Ready);
        let (ticket, _) = m.begin(Uuid::new_v4(), false).unwrap();
        let events = m.sign_out();
        assert_eq!(kinds(&events), ["sign_in_finished", "auth_snapshot"]);
        assert!(m.complete(&ticket, key("b"), Display::default()).is_empty());
        assert_eq!(m.snapshot().state, State::Guest);
    }

    #[test]
    fn reauthentication_requires_the_same_uid() {
        let mut m = Machine::new(Availability::Ready);
        m.restore(key("a"), Display::default());
        let generation = m.snapshot().generation;
        let events = m.reauth_required(&key("a"), generation);
        assert_eq!(kinds(&events), ["session_invalidated", "auth_snapshot"]);
        assert_eq!(m.snapshot().state, State::ReauthRequired);
        assert_eq!(
            m.begin(Uuid::new_v4(), false).unwrap_err(),
            ErrorKind::Unavailable
        );
        let (ticket, _) = m.begin(Uuid::new_v4(), true).unwrap();
        assert!(m.snapshot().reauthenticating);
        assert_eq!(
            m.accepts(&ticket, &key("b")).unwrap_err(),
            ErrorKind::ReauthRequired
        );
        assert!(m.complete(&ticket, key("b"), Display::default()).is_empty());
        m.fail(&ticket, ErrorKind::ReauthRequired);
        assert_eq!(m.snapshot().state, State::ReauthRequired);
        assert_eq!(m.snapshot().account_key, Some(key("a")));
        let (ticket, _) = m.begin(Uuid::new_v4(), true).unwrap();
        m.complete(&ticket, key("a"), Display::default());
        assert_eq!(m.snapshot().state, State::SignedIn);
    }

    #[test]
    fn stale_generation_cannot_refresh_or_invalidate() {
        let mut m = Machine::new(Availability::Ready);
        m.restore(key("a"), Display::default());
        let old = m.snapshot().generation;
        m.reauth_required(&key("a"), old);
        assert!(m.reauth_required(&key("a"), old).is_empty());
        assert!(m.refreshed(&key("a"), old).is_empty());
    }

    #[test]
    fn restore_only_from_a_quiet_guest() {
        let mut m = Machine::new(Availability::Ready);
        m.begin(Uuid::new_v4(), false).unwrap();
        assert!(m.restore(key("a"), Display::default()).is_empty());
    }

    #[test]
    fn public_service_refuses_every_operation() {
        #[cfg(not(feature = "google-sign-in"))]
        {
            let service = Service::new(
                Box::new(vault::Memory::default()),
                Arc::new(|_| {}),
                Arc::new(|_| panic!("a public build never opens a browser")),
            );
            assert_eq!(service.snapshot().availability, Availability::Unavailable);
            assert_eq!(
                service.sign_in(Uuid::new_v4(), false).unwrap_err(),
                ErrorKind::Unavailable
            );
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let choice = lifecycle::LocalCopyChoice::Leave;
            assert_eq!(
                runtime.block_on(service.sign_out(choice)).unwrap_err(),
                ErrorKind::Unavailable
            );
            assert_eq!(
                runtime
                    .block_on(service.switch_account(Uuid::new_v4(), choice))
                    .unwrap_err(),
                ErrorKind::Unavailable
            );
            assert_eq!(
                service.cancel(Uuid::new_v4()).unwrap_err(),
                ErrorKind::Unavailable
            );
        }
    }
}
