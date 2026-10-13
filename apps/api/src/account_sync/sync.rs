//! Account sync for settings and dictionary: per-account durable state, the
//! outbox, pull/merge/push against a [`Remote`], and conflict resolution.
//!
//! Nothing here runs unless this device explicitly joined sync for a category
//! after a preview. Every account keeps its own file under
//! `<state>/account-sync/accounts/`, keyed by a hash of project and UID, so a
//! second account never sees or sends the first one's work. Local stores stay
//! authoritative for local work: a record is saved locally first, and the
//! outbox is derived from the difference between the local portable picture and
//! the last server-acknowledged records, persisted before anything is sent.
use super::local::{self, Choice, Disposition, Mode, Preview, Receipt, RemoteChange, Source};
use super::wire::{
    Account, Category, Consent, Decision, ExportRecord, Operation, Record, Rejection, SettingKey,
};
use crate::recordings::disk::{private_dir, write_json};
use crate::transcription::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Mutex;
use uuid::Uuid;

/// A failed exchange with the cloud. None of these touch local data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteError {
    /// No network or the service did not answer: retried later.
    Offline,
    /// The service is overloaded or out of quota: retried with backoff.
    Busy,
    /// The rules refused a planned write: local state is stale; re-read first.
    PermissionDenied,
    /// The account's credential needs Google sign-in again.
    ReauthRequired,
    /// The session changed while the request was in flight.
    Superseded,
    /// The service answered with something outside the contract.
    InvalidResponse,
    /// A local text cache/outbox could not be read or durably saved.
    StorageFailed,
}

/// One account's cloud, bound to that account's identity and credential.
#[async_trait::async_trait]
pub trait Remote: Send + Sync {
    async fn account(&self) -> Result<Option<Account>, RemoteError>;
    /// Creates the all-off account document if it does not exist yet.
    async fn create_account(&self) -> Result<Account, RemoteError>;
    /// Replaces consent by revision CAS. `Ok(None)` means another device
    /// changed it first; re-read and preview again.
    async fn set_consent(&self, next: &Consent) -> Result<Option<Account>, RemoteError>;
    /// Every settings and dictionary record, paged; transcripts are skipped.
    async fn records(&self) -> Result<Vec<Record>, RemoteError>;
    /// Bounded transcript page, ordered by record ID; no full-history listener.
    async fn history_page(&self, _after: Option<Uuid>) -> Result<HistoryPage, RemoteError> {
        Err(RemoteError::InvalidResponse)
    }
    /// Runs one operation through the commit planner in a transaction.
    async fn commit(&self, op: &Operation) -> Result<Decision, RemoteError>;
}

pub const HISTORY_PAGE: usize = 5;
#[derive(Clone, Debug)]
pub struct HistoryPage {
    pub records: Vec<Record>,
    pub next: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRef {
    pub project_id: String,
    pub uid: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "category", rename_all = "snake_case")]
pub enum Notice {
    /// Sync for a category was turned off, or its cloud data deleted, elsewhere.
    CategoryStopped(Category),
    /// The Writer account was reset or is being deleted.
    AccountReset,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ConflictState {
    id: Uuid,
    category: Category,
    /// The server's record, or `None` when it was deleted elsewhere.
    remote: Option<Value>,
    /// This device's value, or `None` when it deleted the record.
    local: Option<Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AccountState {
    version: u32,
    project_id: String,
    uid: String,
    account_epoch: Option<u64>,
    consent: Option<Consent>,
    /// Categories this device joined, with the consent generation it joined in.
    joined: BTreeMap<Category, u64>,
    paused: bool,
    signed_out: bool,
    known: BTreeMap<Uuid, Value>,
    outbox: Vec<Value>,
    conflicts: Vec<ConflictState>,
    last_success_ms: Option<u64>,
    last_error: Option<RemoteError>,
    notices: Vec<Notice>,
    /// The cloud records a pending sync preview was computed from.
    pending: Option<PendingCloud>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingCloud {
    import_id: Uuid,
    account_epoch: u64,
    records: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConflictView {
    pub id: Uuid,
    pub category: Category,
    pub key: Option<SettingKey>,
    pub local: Option<Value>,
    pub remote: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub joined: Vec<Category>,
    pub paused: bool,
    pub consent: Option<BTreeMap<Category, bool>>,
    pub pending: usize,
    pub conflicts: Vec<ConflictView>,
    pub last_success_ms: Option<u64>,
    pub last_error: Option<RemoteError>,
    pub notices: Vec<Notice>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SyncPreview {
    pub preview: Preview,
    /// Live cloud records per category.
    pub cloud: BTreeMap<Category, usize>,
    /// Local records a joined category would upload.
    pub uploads: BTreeMap<Category, usize>,
    pub enabled: BTreeMap<Category, bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cycle {
    /// Not joined, paused or signed out: nothing was sent.
    Idle,
    Synced,
    /// Stopped early; the outbox is kept for the next attempt.
    Deferred,
}

#[derive(Debug)]
pub enum SyncError {
    Remote(RemoteError),
    Api(ApiError),
    /// Consent changed on another device between preview and confirmation.
    ConsentChanged,
    NotPreviewed,
}
impl From<RemoteError> for SyncError {
    fn from(e: RemoteError) -> Self {
        Self::Remote(e)
    }
}
impl From<ApiError> for SyncError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

pub type StillCurrent<'a> = &'a (dyn Fn() -> bool + Send + Sync);

pub struct Engine {
    dir: PathBuf,
    local: Arc<local::Service>,
    lock: Mutex<()>,
}

fn decode_records(values: &[Value]) -> Vec<Record> {
    values
        .iter()
        .filter_map(|v| Record::decode(v).ok())
        .collect()
}

impl Engine {
    pub async fn open(dir: PathBuf, local: Arc<local::Service>) -> Result<Arc<Self>, ApiError> {
        private_dir(&dir).await?;
        Ok(Arc::new(Self {
            dir,
            local,
            lock: Mutex::new(()),
        }))
    }
    pub fn local(&self) -> &Arc<local::Service> {
        &self.local
    }
    fn path(&self, key: &AccountRef) -> PathBuf {
        let mut hash = Sha256::new();
        hash.update(key.project_id.as_bytes());
        hash.update([0]);
        hash.update(key.uid.as_bytes());
        let name: String = hash.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.dir.join(format!("{name}.json"))
    }
    async fn load(&self, key: &AccountRef) -> Result<AccountState, ApiError> {
        match tokio::fs::read(self.path(key)).await {
            Ok(bytes) => serde_json::from_slice::<AccountState>(&bytes)
                .ok()
                .filter(|s| s.project_id == key.project_id && s.uid == key.uid)
                .ok_or_else(|| local::invalid("This account's sync state is unreadable.")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AccountState {
                version: 1,
                project_id: key.project_id.clone(),
                uid: key.uid.clone(),
                ..Default::default()
            }),
            Err(_) => Err(local::invalid("This account's sync state is unreadable.")),
        }
    }
    async fn save(&self, key: &AccountRef, state: &AccountState) -> Result<(), ApiError> {
        write_json(&self.path(key), state).await
    }

    pub async fn status(&self, key: &AccountRef) -> Result<Status, ApiError> {
        let _guard = self.lock.lock().await;
        let state = self.load(key).await?;
        Ok(Status {
            joined: state.joined.keys().copied().collect(),
            paused: state.paused,
            consent: state.consent.as_ref().map(|c| {
                Category::ALL
                    .into_iter()
                    .map(|cat| (cat, c.toggle(cat).enabled))
                    .collect()
            }),
            pending: state.outbox.len(),
            conflicts: state
                .conflicts
                .iter()
                .map(|c| ConflictView {
                    id: c.id,
                    category: c.category,
                    key: SettingKey::from_id(&c.id),
                    local: c.local.as_ref().map(view_value),
                    remote: c
                        .remote
                        .as_ref()
                        .and_then(|r| Record::decode(r).ok())
                        .and_then(|r| r.content)
                        .map(|d| view_value(&d.encode())),
                })
                .collect(),
            last_success_ms: state.last_success_ms,
            last_error: state.last_error,
            notices: state.notices.clone(),
        })
    }

    /// Compares this device with the account's cloud records and records a
    /// preview. Reads only; consent and local data are unchanged.
    pub async fn preview(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        categories: Vec<Category>,
        disposition: Disposition,
    ) -> Result<SyncPreview, SyncError> {
        let _guard = self.lock.lock().await;
        let account = remote.account().await?;
        let records = if account.is_some() {
            remote.records().await?
        } else {
            vec![]
        };
        let epoch = account.as_ref().map_or(1, |a| a.epoch);
        let live: Vec<ExportRecord> = records
            .iter()
            .filter(|r| categories.contains(&r.category))
            .filter_map(|r| {
                Some(ExportRecord {
                    id: r.id,
                    origin: r.origin,
                    data: r.content.clone()?,
                })
            })
            .collect();
        let mut cloud = BTreeMap::new();
        for record in &live {
            *cloud.entry(record.data.category()).or_insert(0) += 1;
        }
        let preview = self
            .local
            .preview(
                Source::Cloud {
                    uid: key.uid.clone(),
                    account_epoch: epoch,
                },
                Uuid::new_v4(),
                live.clone(),
                categories.clone(),
                Mode::Merge,
                disposition,
            )
            .await?;
        let projection = self.local.projection().await?;
        let in_cloud: BTreeSet<Uuid> = live.iter().map(|r| r.id).collect();
        let mut uploads = BTreeMap::new();
        for (id, data) in &projection.records {
            if categories.contains(&data.category()) && !in_cloud.contains(id) {
                *uploads.entry(data.category()).or_insert(0) += 1;
            }
        }
        let enabled = Category::ALL
            .into_iter()
            .map(|cat| {
                (
                    cat,
                    account
                        .as_ref()
                        .is_some_and(|a| a.consent.toggle(cat).enabled),
                )
            })
            .collect();
        let mut state = self.load(key).await?;
        state.pending = Some(PendingCloud {
            import_id: preview.import_id,
            account_epoch: epoch,
            records: records.iter().map(Record::encode).collect(),
        });
        self.save(key, &state).await?;
        Ok(SyncPreview {
            preview,
            cloud,
            uploads,
            enabled,
        })
    }

    /// Confirms a sync preview. `RestoreOnce` changes only this device.
    /// `RestoreAndSync` first turns the categories on for the account (by CAS),
    /// then applies the restore and joins this device; the next cycle uploads
    /// what this device kept.
    pub async fn confirm(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        import_id: Uuid,
        choices: &BTreeMap<Uuid, Choice>,
        still_current: StillCurrent<'_>,
    ) -> Result<Receipt, SyncError> {
        let _guard = self.lock.lock().await;
        let mut state = self.load(key).await?;
        let pending = state
            .pending
            .clone()
            .filter(|p| p.import_id == import_id)
            .ok_or(SyncError::NotPreviewed)?;
        let Some(Source::Cloud { uid, account_epoch }) =
            self.local.pending_source(import_id).await?
        else {
            return Err(SyncError::NotPreviewed);
        };
        if uid != key.uid || account_epoch != pending.account_epoch {
            return Err(SyncError::NotPreviewed);
        }
        let preview_view = self.local.pending_preview(import_id).await?;
        let Some(preview_view) = preview_view else {
            return Err(SyncError::NotPreviewed);
        };
        let mut consent = None;
        if preview_view.disposition == Disposition::RestoreAndSync {
            let mut account = match remote.account().await? {
                Some(account) => account,
                None => remote.create_account().await?,
            };
            if account.epoch != pending.account_epoch {
                return Err(SyncError::ConsentChanged);
            }
            for category in &preview_view.categories {
                if !account.consent.toggle(*category).enabled {
                    let next = account.consent.next(*category, true);
                    account = remote
                        .set_consent(&next)
                        .await?
                        .ok_or(SyncError::ConsentChanged)?;
                }
            }
            consent = Some(account);
        }
        if !still_current() {
            return Err(SyncError::Remote(RemoteError::Superseded));
        }
        let (receipt, _) = self.local.apply(import_id, choices).await?;
        if let (Receipt::Applied { .. }, Some(account)) = (&receipt, consent) {
            let categories: BTreeSet<Category> = preview_view.categories.iter().copied().collect();
            for category in &categories {
                state
                    .joined
                    .insert(*category, account.consent.toggle(*category).generation);
            }
            state.account_epoch = Some(account.epoch);
            state.consent = Some(account.consent.clone());
            state.paused = false;
            state.signed_out = false;
            // A fresh join supersedes anything left from an earlier session.
            state.outbox.retain(|op| {
                Operation::decode(op).is_ok_and(|op| !categories.contains(&op.category))
            });
            state
                .conflicts
                .retain(|c| !categories.contains(&c.category));
            state.notices.clear();
            let keep: Vec<Record> = decode_records(&pending.records)
                .into_iter()
                .filter(|r| categories.contains(&r.category))
                .collect();
            state
                .known
                .retain(|_, v| Record::decode(v).is_ok_and(|r| !categories.contains(&r.category)));
            for record in keep {
                state.known.insert(record.id, record.encode());
            }
        }
        state.pending = None;
        self.save(key, &state).await?;
        Ok(receipt)
    }

    /// One pull/merge/push cycle. Local capture and delivery never wait on it.
    pub async fn run(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        still_current: StillCurrent<'_>,
    ) -> Result<Cycle, ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.load(key).await?;
        if state.signed_out || state.paused || state.joined.is_empty() {
            return Ok(Cycle::Idle);
        }
        let result = self.cycle(key, &mut state, remote, still_current).await;
        match result {
            Ok(cycle) => {
                if cycle == Cycle::Synced {
                    state.last_success_ms = Some(local::now_ms());
                    state.last_error = None;
                }
                self.save(key, &state).await?;
                Ok(cycle)
            }
            Err(SyncError::Remote(error)) => {
                state.last_error = Some(error);
                self.save(key, &state).await?;
                Ok(Cycle::Deferred)
            }
            Err(SyncError::Api(error)) => {
                self.save(key, &state).await?;
                Err(error)
            }
            Err(_) => Ok(Cycle::Deferred),
        }
    }

    async fn cycle(
        &self,
        key: &AccountRef,
        state: &mut AccountState,
        remote: &dyn Remote,
        still_current: StillCurrent<'_>,
    ) -> Result<Cycle, SyncError> {
        let Some(account) = remote.account().await? else {
            Self::stop_all(state, Notice::AccountReset);
            return Ok(Cycle::Idle);
        };
        if !still_current() {
            return Err(RemoteError::Superseded.into());
        }
        if !account.active || state.account_epoch.is_some_and(|e| e != account.epoch) {
            Self::stop_all(state, Notice::AccountReset);
            return Ok(Cycle::Idle);
        }
        state.account_epoch = Some(account.epoch);
        for (category, generation) in state.joined.clone() {
            let toggle = account.consent.toggle(category);
            if !toggle.enabled || toggle.generation != generation {
                Self::stop(state, category);
            }
        }
        state.consent = Some(account.consent.clone());
        self.save(key, state).await?;
        if state.joined.is_empty() {
            return Ok(Cycle::Idle);
        }

        // Pull: adopt remote changes this device has not edited since.
        let records: Vec<Record> = remote
            .records()
            .await?
            .into_iter()
            .filter(|r| state.joined.contains_key(&r.category))
            .collect();
        if !still_current() {
            return Err(RemoteError::Superseded.into());
        }
        let projection = self.local.projection().await?;
        let queued: BTreeSet<Uuid> = decode_ops(&state.outbox)
            .iter()
            .map(|o| o.record_id)
            .collect();
        let mut changes = vec![];
        for record in records {
            let known = state
                .known
                .get(&record.id)
                .and_then(|v| Record::decode(v).ok());
            if known
                .as_ref()
                .is_some_and(|k| k.revision == record.revision)
            {
                continue;
            }
            if queued.contains(&record.id) {
                continue;
            }
            if let Some(conflict) = state.conflicts.iter_mut().find(|c| c.id == record.id) {
                conflict.remote = Some(record.encode());
                state.known.insert(record.id, record.encode());
                continue;
            }
            let mine = projection.records.get(&record.id).cloned();
            let untouched = match &known {
                Some(k) => k.content == mine,
                None => mine.is_none(),
            };
            if mine == record.content {
                // Already the same here.
            } else if untouched {
                changes.push(RemoteChange {
                    id: record.id,
                    origin: record.origin,
                    content: record.content.clone(),
                });
            } else {
                state.conflicts.push(ConflictState {
                    id: record.id,
                    category: record.category,
                    remote: Some(record.encode()),
                    local: mine.map(|d| d.encode()),
                });
            }
            state.known.insert(record.id, record.encode());
        }
        if !changes.is_empty() {
            if !still_current() {
                return Err(RemoteError::Superseded.into());
            }
            self.local.apply_remote(changes).await?;
        }
        self.save(key, state).await?;

        // Queue: persist every intended operation before sending any.
        let projection = self.local.projection().await?;
        let generation_of = |state: &AccountState, category| state.joined.get(&category).copied();
        let mut queued: BTreeSet<Uuid> = decode_ops(&state.outbox)
            .iter()
            .map(|o| o.record_id)
            .collect();
        let conflicted: BTreeSet<Uuid> = state.conflicts.iter().map(|c| c.id).collect();
        let mut fresh = vec![];
        let binding = super::wire::Binding {
            uid: key.uid.clone(),
            account_epoch: account.epoch,
        };
        let op = |state: &AccountState, id: Uuid, category, base, origin, content| {
            generation_of(state, category).map(|generation| Operation {
                binding: binding.clone(),
                consent_generation: generation,
                history_enrollment_id: None,
                category,
                operation_id: Uuid::new_v4(),
                device_id: projection.device_id,
                record_id: id,
                origin,
                base_revision: base,
                content,
            })
        };
        for (id, data) in &projection.records {
            let category = data.category();
            if !state.joined.contains_key(&category)
                || queued.contains(id)
                || conflicted.contains(id)
            {
                continue;
            }
            let known = state.known.get(id).and_then(|v| Record::decode(v).ok());
            let next = match known {
                None => op(
                    state,
                    *id,
                    category,
                    0,
                    projection.origin(id),
                    Some(data.clone()),
                ),
                Some(k) if k.content.as_ref() == Some(data) => None,
                // Deleted elsewhere but still here: recreating needs a fresh ID.
                Some(k) if k.content.is_none() => None,
                Some(k) => op(
                    state,
                    *id,
                    category,
                    k.revision,
                    k.origin,
                    Some(data.clone()),
                ),
            };
            if let Some(next) = next {
                queued.insert(*id);
                fresh.push(next);
            }
        }
        for (id, value) in &state.known {
            let Ok(k) = Record::decode(value) else {
                continue;
            };
            if k.category != Category::Dictionary
                || k.content.is_none()
                || !state.joined.contains_key(&k.category)
                || projection.records.contains_key(id)
                || queued.contains(id)
                || conflicted.contains(id)
            {
                continue;
            }
            if let Some(next) = op(state, *id, k.category, k.revision, k.origin, None) {
                fresh.push(next);
            }
        }
        state.outbox.extend(fresh.iter().map(Operation::encode));
        self.save(key, state).await?;

        // Push, re-checking the binding before each request and each response.
        for op in decode_ops(&state.outbox.clone()) {
            if !still_current() {
                return Err(RemoteError::Superseded.into());
            }
            if state.joined.get(&op.category) != Some(&op.consent_generation)
                || op.binding != binding
            {
                // Fenced: never resubmitted automatically.
                drop_op(state, &op.operation_id);
                continue;
            }
            let decision = remote.commit(&op).await?;
            if !still_current() {
                return Err(RemoteError::Superseded.into());
            }
            drop_op(state, &op.operation_id);
            match decision {
                Decision::Acknowledged(record) => {
                    state.known.insert(record.id, record.encode());
                }
                Decision::Conflict(current) => {
                    let mine = op.content.clone();
                    if mine != current.content {
                        state.conflicts.push(ConflictState {
                            id: current.id,
                            category: current.category,
                            remote: Some(current.encode()),
                            local: mine.map(|d| d.encode()),
                        });
                    }
                    state.known.insert(current.id, current.encode());
                }
                Decision::Rejected(
                    Rejection::StaleGeneration
                    | Rejection::ConsentRequired
                    | Rejection::StaleEpoch
                    | Rejection::AccountUnavailable,
                ) => {
                    Self::stop(state, op.category);
                }
                Decision::Rejected(Rejection::Deleted) => {
                    state.conflicts.push(ConflictState {
                        id: op.record_id,
                        category: op.category,
                        remote: None,
                        local: op.content.map(|d| d.encode()),
                    });
                }
                Decision::Rejected(Rejection::InvalidBase) => {
                    // Our record of the server was wrong; the next pull re-reads it.
                    state.known.remove(&op.record_id);
                }
                Decision::Rejected(Rejection::QuotaExceeded) => {
                    state.outbox.insert(0, op.encode());
                    self.save(key, state).await?;
                    return Err(RemoteError::Busy.into());
                }
                Decision::Rejected(Rejection::WrongAccount) => {
                    state.outbox.insert(0, op.encode());
                    self.save(key, state).await?;
                    return Err(RemoteError::Superseded.into());
                }
                Decision::Rejected(_) => {
                    state.last_error = Some(RemoteError::InvalidResponse);
                }
            }
            self.save(key, state).await?;
        }
        Ok(Cycle::Synced)
    }

    fn stop(state: &mut AccountState, category: Category) {
        state.joined.remove(&category);
        state
            .outbox
            .retain(|op| Operation::decode(op).is_ok_and(|op| op.category != category));
        state.conflicts.retain(|c| c.category != category);
        let notice = Notice::CategoryStopped(category);
        if !state.notices.contains(&notice) {
            state.notices.push(notice);
        }
    }
    fn stop_all(state: &mut AccountState, notice: Notice) {
        state.joined.clear();
        state.outbox.clear();
        state.conflicts.clear();
        state.known.clear();
        state.account_epoch = None;
        if !state.notices.contains(&notice) {
            state.notices.push(notice);
        }
    }

    /// Resolves one conflict. `Local` keeps this device's value: the next cycle
    /// sends it as a new operation against the revision that was shown.
    /// `Incoming` adopts the cloud's value here.
    pub async fn resolve(
        &self,
        key: &AccountRef,
        id: Uuid,
        choice: Choice,
    ) -> Result<(), ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.load(key).await?;
        let Some(index) = state.conflicts.iter().position(|c| c.id == id) else {
            return Ok(());
        };
        let conflict = state.conflicts.remove(index);
        let remote = conflict
            .remote
            .as_ref()
            .and_then(|r| Record::decode(r).ok());
        match (choice, remote) {
            (Choice::Incoming, Some(record)) => {
                self.local
                    .apply_remote(vec![RemoteChange {
                        id: record.id,
                        origin: record.origin,
                        content: record.content,
                    }])
                    .await?;
            }
            (Choice::Incoming, None) => {
                self.local
                    .apply_remote(vec![RemoteChange {
                        id,
                        origin: id,
                        content: None,
                    }])
                    .await?;
            }
            // Deleted elsewhere: keeping it here means a new identity.
            (Choice::Local, ref remote) if remote.as_ref().is_none_or(|r| r.content.is_none()) => {
                self.local.rekey_entry(id).await?;
            }
            (Choice::Local, _) => {}
        }
        self.save(key, &state).await
    }

    pub async fn set_paused(&self, key: &AccountRef, paused: bool) -> Result<(), ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.load(key).await?;
        state.paused = paused;
        self.save(key, &state).await
    }

    /// Turns a category off for the whole account (a consent CAS that fences
    /// every device's queued work) and stops it here. Cloud data stays until
    /// it is explicitly deleted (MW-OUATH-8).
    pub async fn turn_off(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        category: Category,
    ) -> Result<(), SyncError> {
        let _guard = self.lock.lock().await;
        let mut state = self.load(key).await?;
        if let Some(account) = remote.account().await? {
            if account.consent.toggle(category).enabled {
                let next = account.consent.next(category, false);
                let account = remote
                    .set_consent(&next)
                    .await?
                    .ok_or(SyncError::ConsentChanged)?;
                state.consent = Some(account.consent);
            }
        }
        Self::stop(&mut state, category);
        state
            .notices
            .retain(|n| *n != Notice::CategoryStopped(category));
        self.save(key, &state).await?;
        Ok(())
    }

    /// Sign-out or account switch: freeze this account's work. Nothing it
    /// queued is sent again unless the user rejoins through a new preview.
    pub async fn signed_out(&self, key: &AccountRef) -> Result<(), ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.load(key).await?;
        if !state.joined.is_empty() || !state.outbox.is_empty() {
            state.signed_out = true;
            state.joined.clear();
            self.save(key, &state).await?;
        }
        Ok(())
    }

    pub async fn dismiss_notices(&self, key: &AccountRef) -> Result<(), ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.load(key).await?;
        state.notices.clear();
        self.save(key, &state).await
    }
}

fn decode_ops(values: &[Value]) -> Vec<Operation> {
    values
        .iter()
        .filter_map(|v| Operation::decode(v).ok())
        .collect()
}
fn drop_op(state: &mut AccountState, id: &Uuid) {
    state
        .outbox
        .retain(|v| Operation::decode(v).map_or(true, |op| op.operation_id != *id));
}

/// A setting's bare value, or a dictionary entry as it is.
fn view_value(data: &Value) -> Value {
    if data.get("kind") == Some(&Value::from("setting")) {
        data["value"].clone()
    } else {
        data.clone()
    }
}
