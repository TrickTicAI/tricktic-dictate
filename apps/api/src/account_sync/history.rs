//! Opt-in, text-only history. Account caches/outboxes are separate from jobs;
//! receiving a record has no path to capture, providers, cleanup or insertion.
use super::history_disk::{self as disk, Document, Ids, Lease, Queue, Rows};
use super::{
    history_wire::{HistoryMode, Transcript},
    sync::{AccountRef, Remote, RemoteError, StillCurrent},
    wire::{Account, Binding, Category, Data, Decision, Operation, Record},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

pub const TRUST: &str = "firebase-service-readable-v1";
type Result<T> = std::result::Result<T, RemoteError>;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Catalog {
    device: Uuid,
    #[serde(skip_serializing)]
    records: Rows,
    #[serde(skip)]
    indexes_ready: bool,
    #[serde(skip)]
    attempt_index: BTreeMap<String, Uuid>,
    #[serde(skip)]
    version_index: BTreeMap<(String, String, String), Uuid>,
    #[serde(default)]
    #[serde(skip_serializing)]
    skipped: Ids,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    epoch: u64,
    generation: u64,
    enrollment: Option<Uuid>,
    joined: bool,
    trust: Option<String>,
    #[serde(skip_serializing)]
    excluded_attempts: Ids,
    #[serde(skip_serializing)]
    eligible: Ids,
    #[serde(skip_serializing)]
    rows: Rows,
    #[serde(skip_serializing)]
    outbox: Queue,
    #[serde(skip_serializing)]
    hidden: Ids,
    preview: Option<Preview>,
    last_success_ms: Option<u64>,
    error: Option<RemoteError>,
    #[serde(default)]
    poll_cursor: Option<Uuid>,
    #[serde(default)]
    catalog_cursor: i64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Preview {
    pub id: Uuid,
    pub mode: HistoryMode,
    pub local_records: usize,
    pub trust: String,
    attempts: BTreeSet<Uuid>,
    epoch: u64,
    revision: u64,
}
#[derive(Serialize)]
pub struct Status {
    pub joined: bool,
    pub pending: usize,
    pub copies: usize,
    pub last_success_ms: Option<u64>,
    pub error: Option<RemoteError>,
    pub not_synced: usize,
}
#[derive(Serialize)]
pub struct Page {
    pub records: Vec<Value>,
    pub next: Option<Uuid>,
}
/// Synchronous local transactions let lifecycle hooks fence dispatch before a
/// sign-out event is emitted. Network I/O never holds the local mutex.
pub struct History {
    root: PathBuf,
    account_root: PathBuf,
    source_epoch: std::sync::atomic::AtomicU64,
    lock: Mutex<()>,
    serial: tokio::sync::Mutex<()>,
    suspended: Mutex<BTreeMap<String, u64>>,
    blocked: Mutex<BTreeSet<String>>,
    states: Mutex<BTreeMap<PathBuf, State>>,
    catalogs: Mutex<BTreeMap<PathBuf, Catalog>>,
    source_version: Mutex<Option<(u64, u64)>>,
}
fn bad<T>(_: T) -> RemoteError {
    RemoteError::InvalidResponse
}
fn decode(v: &Value) -> Result<Record> {
    Record::decode(v).map_err(bad)
}
fn transcript(r: &Record) -> Option<&Transcript> {
    match &r.content {
        Some(Data::Transcript(t)) => Some(t),
        _ => None,
    }
}
impl History {
    pub fn open(root: PathBuf) -> Arc<Self> {
        Self::open_with_accounts(root.clone(), root.join("accounts"))
    }
    pub fn open_with_accounts(root: PathBuf, account_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            account_root,
            source_epoch: std::sync::atomic::AtomicU64::new(0),
            lock: Mutex::new(()),
            serial: tokio::sync::Mutex::new(()),
            suspended: Mutex::new(BTreeMap::new()),
            blocked: Mutex::new(BTreeSet::new()),
            states: Mutex::new(BTreeMap::new()),
            catalogs: Mutex::new(BTreeMap::new()),
            source_version: Mutex::new(None),
        })
    }
    fn name(key: &AccountRef) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!("{}\0{}", key.project_id, key.uid).as_bytes())
        )
    }
    fn path(&self, key: &AccountRef) -> PathBuf {
        // Matches the desktop account lifecycle Workspace without putting UIDs
        // in filenames. Guest originals and explicit kept copies live elsewhere.
        let digest = Sha256::digest(key.uid.as_bytes());
        let hash: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        let project: String = key
            .project_id
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .collect();
        self.account_root
            .join(project)
            .join(hash)
            .join("history.json")
    }
    fn catalog(&self) -> Result<Lease<'_, Catalog>> {
        let mut c = Lease::open(&self.catalogs, self.root.join("catalog.json"))?;
        if c.device.is_nil() {
            c.device = Uuid::new_v4();
            c.save()?;
        }
        Ok(c)
    }
    fn state(&self, key: &AccountRef) -> Result<Lease<'_, State>> {
        Lease::open(&self.states, self.path(key))
    }
    fn save(&self, _key: &AccountRef, s: &mut Lease<'_, State>) -> Result<()> {
        s.save()
    }
    fn ticket(&self, key: &AccountRef) -> u64 {
        *self
            .suspended
            .lock()
            .unwrap()
            .get(&Self::name(key))
            .unwrap_or(&0)
    }
    fn current(&self, key: &AccountRef, ticket: u64, current: StillCurrent<'_>) -> Result<()> {
        if current() && self.ticket(key) == ticket {
            Ok(())
        } else {
            Err(RemoteError::Superseded)
        }
    }
    pub fn suspend(&self, key: &AccountRef) {
        self.blocked.lock().unwrap().insert(Self::name(key));
        let mut guard = self.suspended.lock().unwrap();
        *guard.entry(Self::name(key)).or_default() += 1;
    }
    pub fn detach(&self, key: &AccountRef) -> Result<()> {
        self.suspend(key);
        let _g = self.lock.lock().unwrap();
        let mut s = self.state(key)?;
        s.joined = false;
        s.preview = None;
        self.save(key, &mut s)
    }
    pub fn copies(&self, key: &AccountRef) -> Result<(u64, u64)> {
        let _g = self.lock.lock().unwrap();
        Ok((
            disk::count::<State>(&self.path(key), "rows", true)? as u64,
            disk::count::<State>(&self.path(key), "outbox", false)? as u64,
        ))
    }

    pub fn remove_copies(&self, key: &AccountRef) -> Result<()> {
        self.suspend(key);
        let _g = self.lock.lock().unwrap();
        let mut s = self.state(key)?;
        s.joined = false;
        let pending: BTreeSet<_> = s
            .outbox
            .iter()
            .filter_map(|o| {
                o["record_id"]
                    .as_str()
                    .and_then(|id| Uuid::parse_str(id).ok())
            })
            .collect();
        s.rows.retain(|id, _| pending.contains(id));
        s.preview = None;
        self.save(key, &mut s)
    }
    pub fn keep_copies(&self, key: &AccountRef) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let s = self.state(key)?;
        let mut kept: Rows = disk::read(&self.root.join("kept.json"))?;
        for (id, v) in s.rows.iter() {
            if v["content"]["state"] == "live" {
                kept.insert(*id, v.clone());
            }
        }
        disk::write(&self.root.join("kept.json"), &mut kept)
    }
    pub fn fence(&self, key: &AccountRef, remove: bool) -> Result<()> {
        self.suspend(key);
        let _g = self.lock.lock().unwrap();
        let mut s = self.state(key)?;
        s.joined = false;
        s.enrollment = None;
        s.outbox.clear();
        s.eligible.clear();
        s.preview = None;
        if remove {
            s.rows.clear();
        }
        self.save(key, &mut s)
    }
    /// Explicit bulk local deletion also clears these additional text stores.
    /// It disables local sync for every account, and sends no cloud tombstones.
    pub async fn clear_local_history(&self) -> Result<()> {
        let _serial = self.serial.lock().await;
        let _g = self.lock.lock().unwrap();
        self.source_epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.account_root.exists() {
            for project in std::fs::read_dir(&self.account_root).map_err(bad)? {
                let project = project.map_err(bad)?;
                if !project.file_type().map_err(bad)?.is_dir() {
                    continue;
                }
                for account in std::fs::read_dir(project.path()).map_err(bad)? {
                    let account = account.map_err(bad)?;
                    if !account.file_type().map_err(bad)?.is_dir() {
                        continue;
                    }
                    let path = account.path().join("history.json");
                    if path.exists() || path.with_extension("sqlite3").exists() {
                        let mut state: State = disk::read(&path)?;
                        state.rows.clear();
                        state.outbox.clear();
                        state.eligible.clear();
                        state.excluded_attempts.clear();
                        state.hidden.clear();
                        let mut reset = State::default();
                        std::mem::swap(&mut reset.rows, &mut state.rows);
                        std::mem::swap(&mut reset.outbox, &mut state.outbox);
                        std::mem::swap(&mut reset.eligible, &mut state.eligible);
                        std::mem::swap(&mut reset.excluded_attempts, &mut state.excluded_attempts);
                        std::mem::swap(&mut reset.hidden, &mut state.hidden);
                        disk::write(&path, &mut reset)?;
                    }
                }
            }
        }
        self.states.lock().unwrap().clear();
        let mut c = self.catalog()?;
        c.records.clear();
        c.skipped.clear();
        c.save()?;
        let mut kept: Rows = disk::read(&self.root.join("kept.json"))?;
        kept.clear();
        disk::write(&self.root.join("kept.json"), &mut kept)
    }
    /// Local-only date deletion. Keep newer families, stop enrollment on this
    /// device and fence in-flight fetches; no deletion is sent to Firestore.
    pub async fn clear_local_before(&self, cutoff: u64) -> Result<()> {
        let _serial = self.serial.lock().await;
        let _g = self.lock.lock().unwrap();
        self.source_epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut c = self.catalog()?;
        let mut kept: Rows = disk::read(&self.root.join("kept.json"))?;
        let mut states = Vec::new();
        if self.account_root.exists() {
            for project in std::fs::read_dir(&self.account_root).map_err(bad)? {
                let project = project.map_err(bad)?;
                if !project.file_type().map_err(bad)?.is_dir() {
                    continue;
                }
                for account in std::fs::read_dir(project.path()).map_err(bad)? {
                    let account = account.map_err(bad)?;
                    if !account.file_type().map_err(bad)?.is_dir() {
                        continue;
                    }
                    let path = account.path().join("history.json");
                    if path.exists() || path.with_extension("sqlite3").exists() {
                        states.push((path.clone(), disk::read::<State>(&path)?));
                    }
                }
            }
        }
        // Classify by the ORIGINAL recording date, including edits made today
        // to yesterday's recordings. Unknown/orphan dates are retained.
        let mut old = BTreeSet::new();
        let mut attempts = BTreeSet::new();
        for (id, value) in c
            .records
            .iter()
            .chain(kept.iter())
            .chain(states.iter().flat_map(|(_, s)| s.rows.iter()))
        {
            if let Some(Transcript::Original {
                recorded_at_ms,
                attempt_id,
                ..
            }) = transcript(&decode(value)?)
            {
                if *recorded_at_ms < cutoff {
                    old.insert(*id);
                    attempts.insert(*attempt_id);
                }
            }
        }
        let keep = |id: &Uuid, value: &mut Value| -> bool {
            if old.contains(id) {
                return false;
            }
            decode(value)
                .ok()
                .and_then(|r| transcript(&r).map(|t| !old.contains(&t.original(*id))))
                .unwrap_or(true)
        };
        c.records.retain(&keep);
        kept.retain(&keep);
        for (path, mut s) in states {
            s.rows.retain(&keep);
            s.joined = false;
            s.enrollment = None;
            s.outbox.clear();
            s.eligible.clear();
            s.preview = None;
            s.excluded_attempts.extend(attempts.iter().copied());
            disk::write(&path, &mut s)?;
        }
        self.states.lock().unwrap().clear();
        c.skipped.retain(|id| !attempts.contains(id));
        c.save()?;
        disk::write(&self.root.join("kept.json"), &mut kept)
    }
    pub fn status(&self, key: &AccountRef) -> Result<Status> {
        let _g = self.lock.lock().unwrap();
        let s: State = disk::metadata(&self.path(key))?;
        Ok(Status {
            joined: s.joined,
            pending: disk::count::<State>(&self.path(key), "outbox", false)?,
            copies: disk::count::<State>(&self.path(key), "rows", false)?,
            last_success_ms: s.last_success_ms,
            error: s.error,
            not_synced: disk::count::<Catalog>(&self.root.join("catalog.json"), "skipped", false)?,
        })
    }

    /// Called only after the local job is durable. Identity is assigned once
    /// and saved before preview, enrollment or queueing. No whole job is copied.
    pub fn observed_revision(&self, epoch: u64) -> Option<u64> {
        self.source_version
            .lock()
            .unwrap()
            .filter(|(e, _)| *e == epoch)
            .map(|(_, r)| r)
    }
    pub fn source_is_current(&self, epoch: u64, revision: u64) -> bool {
        *self.source_version.lock().unwrap() == Some((epoch, revision))
    }
    pub fn mark_source_current(&self, epoch: u64, revision: u64) {
        if self.source_epoch() == epoch {
            *self.source_version.lock().unwrap() = Some((epoch, revision));
        }
    }
    pub fn source_epoch(&self) -> u64 {
        self.source_epoch.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn observe(&self, jobs: &[(crate::transcription::Job, u64)]) -> Result<()> {
        self.observe_at(self.source_epoch(), jobs)
    }
    pub fn observe_at(&self, epoch: u64, jobs: &[(crate::transcription::Job, u64)]) -> Result<()> {
        self.project(epoch, jobs, true)
    }
    pub fn observe_changes(
        &self,
        epoch: u64,
        jobs: &[(crate::transcription::Job, u64)],
    ) -> Result<()> {
        self.project(epoch, jobs, false)
    }
    fn project(
        &self,
        epoch: u64,
        jobs: &[(crate::transcription::Job, u64)],
        prune: bool,
    ) -> Result<()> {
        use crate::transcription::{CleanupResult, JobStatus};
        let _g = self.lock.lock().unwrap();
        if epoch != self.source_epoch() {
            return Err(RemoteError::Superseded);
        }
        let mut c = self.catalog()?;
        let device = c.device;
        let mut originals = std::mem::take(&mut c.attempt_index);
        let mut versions = std::mem::take(&mut c.version_index);
        if !c.indexes_ready {
            originals.clear();
            versions.clear();
            for (id, v) in c.records.iter() {
                let d = &v["content"]["data"];
                if let Some(attempt) = d["attempt_id"].as_str() {
                    originals.insert(attempt.to_owned(), *id);
                } else {
                    versions.insert(
                        (
                            d["kind"].as_str().unwrap_or_default().to_owned(),
                            d["original_record_id"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                            d["text"].as_str().unwrap_or_default().to_owned(),
                        ),
                        *id,
                    );
                }
            }
        }
        for (job, recorded_at) in jobs {
            let JobStatus::Succeeded { result } = &job.status else {
                continue;
            };
            let existing = originals.get(&job.id.to_string()).copied();
            let id = existing.unwrap_or_else(Uuid::new_v4);
            let raw = json!({"kind":"original","attempt_id":job.id,"recorded_at_ms":recorded_at,"input_language":result.transcript.input_language,"output_language":result.transcript.output_language,"selection":result.transcript.selection,"text":result.transcript.raw_text});
            let Ok(raw) = Transcript::decode(&raw) else {
                c.skipped.insert(job.id);
                continue;
            };
            if existing.is_none() {
                originals.insert(job.id.to_string(), id);
                c.records.insert(
                    id,
                    Record {
                        id,
                        origin: device,
                        category: Category::Transcripts,
                        revision: 1,
                        content: Some(Data::Transcript(raw)),
                    }
                    .encode(),
                );
            }
            let corrected = crate::vocabulary::process(
                &result.transcript.raw_text,
                &job.request.options.vocabulary,
            )
            .ok()
            .map(|p| p.text());
            let mut parent = id;
            for (kind, text) in [
                ("corrected", corrected),
                (
                    "cleaned",
                    match &result.cleanup {
                        CleanupResult::Succeeded { cleaned_text, .. } => Some(cleaned_text.clone()),
                        _ => None,
                    },
                ),
            ] {
                let Some(text) = text else { continue };
                if kind == "corrected" && text == result.transcript.raw_text {
                    continue;
                }
                let version_key = (kind.to_owned(), id.to_string(), text.clone());
                let existing = versions.get(&version_key).copied();
                let rid = existing.unwrap_or_else(Uuid::new_v4);
                let d = json!({"kind":kind,"original_record_id":id,"parent_record_id":parent,"created_at_ms":job.updated_at_ms,"text":text});
                if let Ok(t) = Transcript::decode(&d) {
                    if existing.is_none() {
                        versions.insert(version_key, rid);
                        c.records.insert(
                            rid,
                            Record {
                                id: rid,
                                origin: device,
                                category: Category::Transcripts,
                                revision: 1,
                                content: Some(Data::Transcript(t)),
                            }
                            .encode(),
                        );
                    }
                    parent = rid;
                } else {
                    c.skipped.insert(job.id);
                }
            }
        }
        if prune {
            let attempts: BTreeSet<_> = jobs.iter().map(|(j, _)| j.id).collect();
            let originals: BTreeSet<_> = c
                .records
                .iter()
                .filter_map(|(id, v)| {
                    v["content"]["data"]["attempt_id"]
                        .as_str()
                        .and_then(|a| Uuid::parse_str(a).ok())
                        .filter(|a| attempts.contains(a))
                        .map(|_| *id)
                })
                .collect();
            c.records.retain(|id, v| {
                decode(v)
                    .ok()
                    .and_then(|r| transcript(&r).map(|t| t.original(*id)))
                    .is_some_and(|root| originals.contains(&root))
            });
            c.skipped.retain(|id| attempts.contains(id));
            // Rebuild after explicit authoritative removals; ordinary changes retain indexes.
            c.indexes_ready = false;
        } else {
            c.indexes_ready = true;
        }
        c.attempt_index = originals;
        c.version_index = versions;
        c.save()
    }
    pub async fn preview(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        mode: HistoryMode,
        current: StillCurrent<'_>,
    ) -> Result<Preview> {
        let _serial = self.serial.lock().await;
        let ticket = self.ticket(key);
        self.current(key, ticket, current)?;
        let account = remote.account().await?;
        self.current(key, ticket, current)?;
        let _g = self.lock.lock().unwrap();
        let c = self.catalog()?;
        let attempts = c
            .records
            .values()
            .filter_map(|v| {
                v["content"]["data"]["attempt_id"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
            })
            .collect();
        let p = Preview {
            id: Uuid::new_v4(),
            mode,
            local_records: if mode == HistoryMode::IncludeExisting {
                c.records.len()
            } else {
                0
            },
            trust: TRUST.into(),
            attempts,
            epoch: account.as_ref().map_or(1, |a| a.epoch),
            revision: account.as_ref().map_or(1, |a| a.consent.revision),
        };
        let mut s = self.state(key)?;
        s.preview = Some(p.clone());
        self.save(key, &mut s)?;
        Ok(p)
    }
    pub async fn confirm(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        id: Uuid,
        trust: &str,
        current: StillCurrent<'_>,
    ) -> Result<()> {
        let _serial = self.serial.lock().await;
        let ticket = self.ticket(key);
        self.current(key, ticket, current)?;
        let p = {
            let _g = self.lock.lock().unwrap();
            self.state(key)?
                .preview
                .clone()
                .filter(|p| p.id == id)
                .ok_or(RemoteError::PermissionDenied)?
        };
        if trust != TRUST {
            return Err(RemoteError::PermissionDenied);
        }
        let account = match remote.account().await? {
            Some(a) => a,
            None => {
                self.current(key, ticket, current)?;
                remote.create_account().await?
            }
        };
        self.current(key, ticket, current)?;
        if !account.active || account.epoch != p.epoch || account.consent.revision != p.revision {
            return Err(RemoteError::PermissionDenied);
        }
        let mut consent = account.consent.clone();
        // Another device already enabled history: join its enrollment. The
        // chosen mode is this device's own boundary (`excluded_attempts` and
        // the previewed attempts below), so it must not replace the account's
        // enrollment and fence the devices already syncing. Only a device
        // that is itself joined and changes its selection starts a new one.
        let joined_here = {
            let _g = self.lock.lock().unwrap();
            let s = self.state(key)?;
            s.joined
                && s.enrollment.map(|i| i.to_string()).as_deref()
                    == consent.transcripts["history"]["enrollment_id"].as_str()
        };
        let a = if consent.transcripts["enabled"] == true
            && (!joined_here || consent.transcripts["history"]["mode"] == json!(p.mode))
        {
            account
        } else {
            consent.revision += 1;
            consent.transcripts = json!({"enabled":true,"generation":consent.transcripts_generation()+1,"history":{"mode":p.mode,"enrollment_id":Uuid::new_v4()}});
            self.current(key, ticket, current)?;
            remote
                .set_consent(&consent)
                .await?
                .ok_or(RemoteError::PermissionDenied)?
        };
        self.current(key, ticket, current)?;
        let enrollment = Uuid::parse_str(
            a.consent.transcripts["history"]["enrollment_id"]
                .as_str()
                .ok_or(RemoteError::InvalidResponse)?,
        )
        .map_err(bad)?;
        let _g = self.lock.lock().unwrap();
        let c = self.catalog()?;
        let mut s = self.state(key)?;
        s.epoch = a.epoch;
        s.generation = a.consent.transcripts_generation();
        s.enrollment = Some(enrollment);
        s.trust = Some(TRUST.into());
        s.joined = true;
        s.outbox.clear();
        s.eligible.clear();
        s.catalog_cursor = 0;
        // Future-only excludes every already finalized attempt at confirmation,
        // including jobs completed while the preview was open. No wall clocks.
        let excluded_attempts: BTreeSet<Uuid> = c
            .records
            .values()
            .filter_map(|v| {
                v["content"]["data"]["attempt_id"]
                    .as_str()
                    .and_then(|x| Uuid::parse_str(x).ok())
            })
            .collect();
        s.excluded_attempts.replace(excluded_attempts);
        if p.mode == HistoryMode::IncludeExisting {
            for v in c.records.values() {
                let r = decode(v)?;
                if let Some(Transcript::Original { attempt_id, .. }) = transcript(&r) {
                    if p.attempts.contains(attempt_id) {
                        s.eligible.insert(r.id);
                    }
                }
            }
        }
        s.preview = None;
        self.save(key, &mut s)?;
        self.blocked.lock().unwrap().remove(&Self::name(key));
        Ok(())
    }
    fn matches(s: &State, a: &Account) -> bool {
        a.active
            && s.epoch == a.epoch
            && s.generation == a.consent.transcripts_generation()
            && a.consent.transcripts["enabled"] == true
            && s.enrollment.map(|i| i.to_string()).as_deref()
                == a.consent.transcripts["history"]["enrollment_id"].as_str()
    }
    pub async fn run(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        current: StillCurrent<'_>,
    ) -> Result<()> {
        let _serial = self.serial.lock().await;
        let ticket = self.ticket(key);
        if !self.status(key)?.joined || self.blocked.lock().unwrap().contains(&Self::name(key)) {
            return Ok(());
        }
        let outcome = async {
            self.send(key, remote, ticket, current).await?;
            if self.status(key)?.joined {
                let after = {
                    let _g = self.lock.lock().unwrap();
                    self.state(key)?.poll_cursor
                };
                let page = self.restore_page(key, remote, after, current).await?;
                self.current(key, ticket, current)?;
                let _g = self.lock.lock().unwrap();
                let mut s = self.state(key)?;
                s.poll_cursor = page.next;
                self.save(key, &mut s)?;
            }
            Ok(())
        }
        .await;
        if self.current(key, ticket, current).is_ok() {
            let _g = self.lock.lock().unwrap();
            let mut s = self.state(key)?;
            s.error = outcome.as_ref().err().copied();
            if outcome.is_ok() {
                s.last_success_ms = Some(super::local::now_ms());
            }
            self.save(key, &mut s)?;
        }
        outcome
    }
    async fn send(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        ticket: u64,
        current: StillCurrent<'_>,
    ) -> Result<()> {
        self.current(key, ticket, current)?;
        let account = remote.account().await?;
        self.current(key, ticket, current)?;
        {
            let _g = self.lock.lock().unwrap();
            let mut s = self.state(key)?;
            if !account.as_ref().is_some_and(|a| Self::matches(&s, a)) {
                if account
                    .as_ref()
                    .is_some_and(|a| a.transcript_reset.is_some_and(|g| g > s.generation))
                {
                    s.rows.clear();
                }
                s.joined = false;
                s.outbox.clear();
                s.eligible.clear();
                self.save(key, &mut s)?;
                return Ok(());
            }
            let c = self.catalog()?;
            let changes = disk::changes::<Catalog>(
                &self.root.join("catalog.json"),
                "records",
                s.catalog_cursor,
                100,
            )?;
            // Include a selected record's parent chain before it, even when a legacy
            // catalog imported in UUID order places the child on an earlier page.
            fn ancestors(
                c: &Catalog,
                id: Uuid,
                seen: &mut BTreeSet<Uuid>,
                out: &mut Vec<Record>,
            ) -> Result<()> {
                if !seen.insert(id) {
                    return Ok(());
                }
                let Some(v) = c.records.get(&id) else {
                    return Ok(());
                };
                let r = decode(v)?;
                if let Some(Transcript::Derived {
                    parent_record_id, ..
                }) = transcript(&r)
                {
                    ancestors(c, *parent_record_id, seen, out)?;
                }
                out.push(r);
                Ok(())
            }
            let mut candidates = Vec::new();
            let mut seen = BTreeSet::new();
            for (_, v) in &changes {
                ancestors(&c, decode(v)?.id, &mut seen, &mut candidates)?;
            }
            for r in candidates {
                let Some(t) = transcript(&r) else { continue };
                let original = t.original(r.id);
                if let Some(Transcript::Original { attempt_id, .. }) = c
                    .records
                    .get(&original)
                    .and_then(|v| decode(v).ok())
                    .as_ref()
                    .and_then(transcript)
                {
                    if !s.excluded_attempts.contains(attempt_id) {
                        s.eligible.insert(original);
                    }
                }
                if !s.eligible.contains(&original)
                    || s.hidden.contains(&original)
                    || s.hidden.contains(&r.id)
                    || s.rows.contains_key(&r.id)
                    || s.outbox.contains_record(r.id)
                {
                    continue;
                }
                let operation = Operation {
                    binding: Binding {
                        uid: key.uid.clone(),
                        account_epoch: s.epoch,
                    },
                    consent_generation: s.generation,
                    history_enrollment_id: s.enrollment,
                    category: Category::Transcripts,
                    operation_id: Uuid::new_v4(),
                    device_id: c.device,
                    record_id: r.id,
                    origin: r.origin,
                    base_revision: 0,
                    content: r.content,
                }
                .encode();
                s.outbox.push(operation);
            }
            if let Some((sequence, _)) = changes.last() {
                s.catalog_cursor = *sequence;
            }
            self.save(key, &mut s)?;
        }
        // Bounded batches; local originals remain authoritative if saving fails.
        for _ in 0..50 {
            self.current(key, ticket, current)?;
            let op = {
                let _g = self.lock.lock().unwrap();
                self.state(key)?
                    .outbox
                    .first()
                    .map(Operation::decode)
                    .transpose()
                    .map_err(bad)?
            };
            let Some(op) = op else { break };
            let reply = remote.commit(&op).await?;
            self.current(key, ticket, current)?;
            let _g = self.lock.lock().unwrap();
            let mut s = self.state(key)?;
            match reply {
                Decision::Acknowledged(r) => {
                    // A retried receipt can predate a tombstone we already pulled.
                    // It acknowledges delivery, never revives the deleted copy.
                    let hidden = s.hidden.contains(&r.id)
                        || transcript(&r).is_some_and(|t| s.hidden.contains(&t.original(r.id)));
                    let newer = s
                        .rows
                        .get(&r.id)
                        .and_then(|v| v["revision"].as_u64())
                        .is_some_and(|revision| revision > r.revision);
                    if !hidden && !newer {
                        s.rows.insert(r.id, r.encode());
                    }
                }
                Decision::Rejected(super::wire::Rejection::Deleted) => {
                    s.hidden.insert(op.record_id);
                    s.rows.remove(&op.record_id);
                }
                Decision::Conflict(r) if r.content == op.content => {
                    s.rows.insert(r.id, r.encode());
                }
                Decision::Rejected(
                    super::wire::Rejection::StaleEpoch
                    | super::wire::Rejection::StaleGeneration
                    | super::wire::Rejection::ConsentRequired
                    | super::wire::Rejection::HistoryNotEnrolled
                    | super::wire::Rejection::AccountUnavailable,
                ) => {
                    s.joined = false;
                    s.outbox.clear();
                    self.save(key, &mut s)?;
                    return Ok(());
                }
                _ => return Err(RemoteError::PermissionDenied),
            }
            s.outbox.pop();
            self.save(key, &mut s)?;
        }
        Ok(())
    }
    /// Explicit restore/query only. At most one cloud page per user action.
    pub async fn restore(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        after: Option<Uuid>,
        current: StillCurrent<'_>,
    ) -> Result<Page> {
        let _serial = self.serial.lock().await;
        self.restore_page(key, remote, after, current).await
    }
    async fn restore_page(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        after: Option<Uuid>,
        current: StillCurrent<'_>,
    ) -> Result<Page> {
        let ticket = self.ticket(key);
        self.current(key, ticket, current)?;
        let before = remote
            .account()
            .await?
            .ok_or(RemoteError::PermissionDenied)?;
        self.current(key, ticket, current)?;
        if !before.active {
            return Err(RemoteError::PermissionDenied);
        }
        let page = remote.history_page(after).await?;
        self.current(key, ticket, current)?;
        let after_account = remote
            .account()
            .await?
            .ok_or(RemoteError::PermissionDenied)?;
        self.current(key, ticket, current)?;
        if before != after_account {
            return Err(RemoteError::PermissionDenied);
        }
        let _g = self.lock.lock().unwrap();
        let mut s = self.state(key)?;
        let mut records = vec![];
        for r in page.records {
            if r.category != Category::Transcripts {
                return Err(RemoteError::InvalidResponse);
            }
            if r.content.is_none() {
                s.hidden.insert(r.id);
                let family = s.rows.family(r.id);
                for child in family {
                    s.rows.remove(&child);
                }
                s.rows.remove(&r.id);
            } else if !s.hidden.contains(&r.id)
                && !transcript(&r).is_some_and(|t| s.hidden.contains(&t.original(r.id)))
            {
                s.rows.insert(r.id, r.encode());
                records.push(r.encode());
            }
        }
        records.retain(|v| {
            s.rows
                .contains_key(&Uuid::parse_str(v["id"].as_str().unwrap()).unwrap())
        });
        self.save(key, &mut s)?;
        Ok(Page {
            records,
            next: page.next,
        })
    }
    pub fn local_page(&self, key: Option<&AccountRef>, after: Option<Uuid>) -> Result<Page> {
        let _g = self.lock.lock().unwrap();
        let records = match key {
            Some(k) => disk::page::<State>(&self.path(k), "rows", after, 20)?,
            None => disk::page::<Rows>(&self.root.join("kept.json"), "records", after, 20)?,
        };
        let next = (records.len() == 20)
            .then(|| Uuid::parse_str(records.last().unwrap()["id"].as_str().unwrap()).unwrap());
        Ok(Page { records, next })
    }
    /// Edits always append a sibling version. They never replace raw text or
    /// choose a winner using timestamps, including concurrent offline edits.
    pub fn edit(&self, key: &AccountRef, parent: Uuid, text: String) -> Result<Uuid> {
        let _g = self.lock.lock().unwrap();
        let mut s = self.state(key)?;
        if !s.joined {
            return Err(RemoteError::PermissionDenied);
        }
        let p = decode(s.rows.get(&parent).ok_or(RemoteError::InvalidResponse)?)?;
        let t = transcript(&p).ok_or(RemoteError::InvalidResponse)?;
        let id = Uuid::new_v4();
        let data=Transcript::decode(&json!({"kind":"edited","original_record_id":t.original(parent),"parent_record_id":parent,"created_at_ms":super::local::now_ms(),"text":text})).map_err(bad)?;
        let c = self.catalog()?;
        let r = Record {
            id,
            origin: c.device,
            category: Category::Transcripts,
            revision: 1,
            content: Some(Data::Transcript(data)),
        };
        let operation = Operation {
            binding: Binding {
                uid: key.uid.clone(),
                account_epoch: s.epoch,
            },
            consent_generation: s.generation,
            history_enrollment_id: s.enrollment,
            category: Category::Transcripts,
            operation_id: Uuid::new_v4(),
            device_id: c.device,
            record_id: id,
            origin: r.origin,
            base_revision: 0,
            content: r.content.clone(),
        }
        .encode();
        s.outbox.push(operation);
        s.rows.insert(id, r.encode());
        self.save(key, &mut s)?;
        Ok(id)
    }
    pub fn delete(&self, key: &AccountRef, id: Uuid) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut s = self.state(key)?;
        if !s.joined {
            return Err(RemoteError::PermissionDenied);
        }
        let r = decode(s.rows.get(&id).ok_or(RemoteError::InvalidResponse)?)?;
        if s.outbox.iter().any(|o| o["record_id"] == id.to_string()) {
            return Err(RemoteError::Busy);
        }
        let c = self.catalog()?;
        let operation = Operation {
            binding: Binding {
                uid: key.uid.clone(),
                account_epoch: s.epoch,
            },
            consent_generation: s.generation,
            history_enrollment_id: s.enrollment,
            category: Category::Transcripts,
            operation_id: Uuid::new_v4(),
            device_id: c.device,
            record_id: id,
            origin: r.origin,
            base_revision: r.revision,
            content: None,
        }
        .encode();
        s.outbox.push(operation);
        s.hidden.insert(id);
        let family = s.rows.family(id);
        for child in family {
            s.rows.remove(&child);
        }
        s.rows.remove(&id);
        self.save(key, &mut s)
    }
    pub async fn turn_off(
        &self,
        key: &AccountRef,
        remote: &dyn Remote,
        current: StillCurrent<'_>,
    ) -> Result<()> {
        self.detach(key)?;
        let _serial = self.serial.lock().await;
        let ticket = self.ticket(key);
        self.current(key, ticket, current)?;
        if let Some(a) = remote.account().await? {
            self.current(key, ticket, current)?;
            if a.consent.transcripts["enabled"] == true {
                remote
                    .set_consent(&a.consent.next(Category::Transcripts, false))
                    .await?
                    .ok_or(RemoteError::PermissionDenied)?;
            }
        }
        self.current(key, ticket, current)?;
        self.fence(key, false)
    }
}

impl Document for Catalog {
    fn fields() -> &'static [(&'static str, bool)] {
        &[("records", false), ("skipped", true)]
    }
    fn changes(&self) -> disk::Changes {
        vec![
            ("records", disk::row_changes(&self.records)),
            ("skipped", disk::id_changes(&self.skipped)),
        ]
    }
    fn clean(&mut self) {
        disk::clean_rows(&mut self.records);
        disk::clean_ids(&mut self.skipped);
    }
}

impl Document for State {
    fn fields() -> &'static [(&'static str, bool)] {
        &[
            ("rows", false),
            ("outbox", true),
            ("excluded_attempts", true),
            ("eligible", true),
            ("hidden", true),
        ]
    }
    fn changes(&self) -> disk::Changes {
        vec![
            ("rows", disk::row_changes(&self.rows)),
            ("outbox", disk::queue_changes(&self.outbox)),
            (
                "excluded_attempts",
                disk::id_changes(&self.excluded_attempts),
            ),
            ("eligible", disk::id_changes(&self.eligible)),
            ("hidden", disk::id_changes(&self.hidden)),
        ]
    }
    fn clean(&mut self) {
        disk::clean_rows(&mut self.rows);
        disk::clean_queue(&mut self.outbox);
        disk::clean_ids(&mut self.excluded_attempts);
        disk::clean_ids(&mut self.eligible);
        disk::clean_ids(&mut self.hidden);
    }
}

#[cfg(test)]
#[path = "history_scale_tests.rs"]
mod scale_tests;
