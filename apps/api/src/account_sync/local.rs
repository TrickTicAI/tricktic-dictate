//! The device's portable state: export, previewed import with atomic apply and
//! rollback, and the local revision that stale previews are checked against.
//!
//! Private files under `<state>/account-sync/`:
//! - `portable.json`: installation device ID, local revision, shadow values,
//!   record origins, the theme mirror, the pending preview, the one rollback
//!   snapshot and bounded import/rollback receipts.
//! - `journal.json`: exists only while a change spanning several stores is
//!   being written. Opening the service finishes or undoes it.
use super::portable::{self, Applied, Capabilities, Local, Outcome, Reason, Shadow, Stores, Theme};
use super::wire::{Backup, Category, Content, Data, Entry, ExportRecord, Setting, SettingKey};
use crate::recordings::disk::{private_dir, write_json};
use crate::transcription::{ApiError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Mutex;
use uuid::Uuid;

const RECEIPT_LIMIT: usize = 32;

/// Store contents a snapshot or journal can put back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Image {
    pub local: Local,
    pub theme: Option<Theme>,
    pub shadow: BTreeMap<SettingKey, Shadow>,
    pub origins: BTreeMap<Uuid, Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Snapshot {
    id: Uuid,
    import_id: Uuid,
    /// The local revision the import (or rollback) produced.
    revision: u64,
    touched: BTreeSet<Category>,
    before: Image,
}

/// Where previewed records came from. A cloud preview is bound to its account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    File,
    Cloud { uid: String, account_epoch: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Merge,
    ReplaceSelected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    RestoreOnce,
    RestoreAndSync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Incoming,
    Local,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    view: Preview,
    source: Source,
    records: Vec<Value>,
}

/// What an import would change, for the confirmation screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub import_id: Uuid,
    pub export_id: Uuid,
    pub mode: Mode,
    pub categories: Vec<Category>,
    pub disposition: Disposition,
    pub expected_local_revision: u64,
    pub settings: Vec<SettingChange>,
    pub dictionary: DictionaryPreview,
    /// The contract's disjoint, exhaustive ID lists over the selected records.
    pub apply_ids: Vec<Uuid>,
    pub preserve_ids: Vec<Uuid>,
    pub conflict_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingChange {
    pub id: Uuid,
    pub key: SettingKey,
    pub current: Option<Value>,
    pub incoming: Value,
    /// What applying it would do on this device.
    pub outcome: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DictionaryPreview {
    pub added: Vec<EntryView>,
    pub unchanged: usize,
    pub conflicts: Vec<EntryConflict>,
    /// Entries `replace_selected` removes; always listed before confirmation.
    pub removed: Vec<EntryView>,
    /// Incoming entries whose phrase or text matches a different local entry.
    pub overlapping: usize,
    pub reordered: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntryView {
    pub id: Uuid,
    pub entry: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntryConflict {
    pub id: Uuid,
    pub local: Value,
    pub incoming: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotApplied {
    Invalid,
    UnsupportedVersion,
    Conflict,
    StalePreview,
    StorageFailed,
}

/// The contract's `ImportReceipt`, plus what happened to each setting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Receipt {
    Applied {
        import_id: Uuid,
        local_revision: u64,
        rollback_snapshot_id: Uuid,
        #[serde(default)]
        outcomes: BTreeMap<SettingKey, Value>,
        #[serde(default)]
        theme: Option<Theme>,
    },
    NotApplied {
        import_id: Uuid,
        reason: NotApplied,
    },
}
impl Receipt {
    /// The exact `ImportReceipt` wire document.
    pub fn wire(&self) -> Value {
        match self {
            Self::Applied {
                import_id,
                local_revision,
                rollback_snapshot_id,
                ..
            } => json!({ "schema_version": 1, "import_id": import_id, "state": "applied",
                "local_revision": local_revision, "rollback_snapshot_id": rollback_snapshot_id }),
            Self::NotApplied { import_id, reason } => json!({ "schema_version": 1,
                "import_id": import_id, "state": "not_applied", "reason": reason }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredReceipt {
    key: Uuid,
    receipt: Receipt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    version: u32,
    device_id: Uuid,
    local_revision: u64,
    digest: String,
    #[serde(default)]
    origins: BTreeMap<Uuid, Uuid>,
    #[serde(default)]
    shadow: BTreeMap<SettingKey, Shadow>,
    #[serde(default)]
    theme: Option<Theme>,
    #[serde(default)]
    preview: Option<Pending>,
    #[serde(default)]
    snapshot: Option<Snapshot>,
    #[serde(default)]
    receipts: Vec<StoredReceipt>,
    /// The last multi-store change that reached its commit point.
    #[serde(default)]
    committed: Option<Uuid>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    id: Uuid,
    before: Image,
}

/// The current portable picture of this device, for export and sync.
#[derive(Clone, Debug)]
pub struct Projection {
    pub device_id: Uuid,
    pub revision: u64,
    pub records: BTreeMap<Uuid, Data>,
    pub origins: BTreeMap<Uuid, Uuid>,
    pub shadow: BTreeMap<SettingKey, Shadow>,
}
impl Projection {
    pub fn origin(&self, id: &Uuid) -> Uuid {
        self.origins.get(id).copied().unwrap_or(self.device_id)
    }
}

/// A remote change for [`Service::apply_remote`]: `None` deletes.
pub struct RemoteChange {
    pub id: Uuid,
    pub origin: Uuid,
    pub content: Content,
}

pub struct Service {
    dir: PathBuf,
    stores: Arc<dyn Stores>,
    caps: Arc<dyn Capabilities>,
    lock: Mutex<()>,
    changed: tokio::sync::watch::Sender<u64>,
}

fn storage() -> ApiError {
    ApiError {
        code: ErrorCode::StorageFailed,
        message: "Backup and sync state could not be saved. Your settings and dictionary were not intentionally changed.".into(),
        retryable: true,
        retry_after_ms: None,
    }
}
pub fn invalid(message: &str) -> ApiError {
    ApiError {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
        retryable: false,
        retry_after_ms: None,
    }
}

impl Service {
    /// Opens the state directory and finishes or undoes an interrupted change.
    pub async fn open(
        dir: PathBuf,
        stores: Arc<dyn Stores>,
        caps: Arc<dyn Capabilities>,
    ) -> Result<Arc<Self>, ApiError> {
        private_dir(&dir).await?;
        let service = Arc::new(Self {
            dir,
            stores,
            caps,
            lock: Mutex::new(()),
            changed: tokio::sync::watch::channel(0).0,
        });
        service.recover().await?;
        Ok(service)
    }
    /// Advances whenever this service changes the device's portable state.
    pub fn watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }
    fn state_path(&self) -> PathBuf {
        self.dir.join("portable.json")
    }
    fn journal_path(&self) -> PathBuf {
        self.dir.join("journal.json")
    }
    async fn read_state(&self) -> Result<State, ApiError> {
        match tokio::fs::read(self.state_path()).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| storage()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State {
                version: 1,
                device_id: Uuid::new_v4(),
                local_revision: 0,
                digest: String::new(),
                origins: BTreeMap::new(),
                shadow: BTreeMap::new(),
                theme: None,
                preview: None,
                snapshot: None,
                receipts: vec![],
                committed: None,
            }),
            Err(_) => Err(storage()),
        }
    }
    async fn write_state(&self, state: &State) -> Result<(), ApiError> {
        write_json(&self.state_path(), state).await
    }

    async fn recover(&self) -> Result<(), ApiError> {
        let _guard = self.lock.lock().await;
        let bytes = match tokio::fs::read(self.journal_path()).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(storage()),
        };
        let state = self.read_state().await?;
        if let Ok(journal) = serde_json::from_slice::<Journal>(&bytes) {
            if state.committed != Some(journal.id) {
                // Never committed: put every store back as it was.
                let current = self.stores.load().await?;
                self.stores.store(&current, &journal.before.local).await?;
            }
        }
        tokio::fs::remove_file(self.journal_path())
            .await
            .map_err(|_| storage())
    }

    /// Loads the stores, gives unidentified dictionary entries their stable
    /// IDs once, retires shadows the user has overridden and advances the local
    /// revision if the portable picture changed. Called under the lock.
    async fn refresh(&self, state: &mut State) -> Result<Local, ApiError> {
        let mut local = self.stores.load().await?;
        let mut dirty = state.digest.is_empty();
        let mut identified = local.clone();
        if identified.vocabulary.assign_ids() {
            crate::vocabulary::validate_document(&identified.vocabulary)
                .map_err(|_| invalid("The dictionary is too large to give every entry an identity. Remove some entries, then try again."))?;
            self.stores.store(&local, &identified).await?;
            local = identified;
        }
        dirty |= portable::retire_changed(&mut state.shadow, &local, state.theme.as_ref());
        let digest = digest(&portable::project(
            &local,
            state.theme.as_ref(),
            &state.shadow,
        ));
        if digest != state.digest {
            state.digest = digest;
            state.local_revision += 1;
            dirty = true;
        }
        if dirty {
            self.write_state(state).await?;
        }
        Ok(local)
    }

    pub async fn projection(&self) -> Result<Projection, ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        let local = self.refresh(&mut state).await?;
        Ok(Projection {
            device_id: state.device_id,
            revision: state.local_revision,
            records: portable::project(&local, state.theme.as_ref(), &state.shadow),
            origins: state.origins,
            shadow: state.shadow,
        })
    }

    pub async fn theme(&self) -> Result<Option<Theme>, ApiError> {
        let _guard = self.lock.lock().await;
        Ok(self.read_state().await?.theme)
    }
    /// Records the theme the WebView applied, so export and sync see it.
    pub async fn set_theme(&self, theme: Theme) -> Result<(), ApiError> {
        if theme.id.is_empty()
            || theme.id.len() > 64
            || !theme
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !["light", "dark"].contains(&theme.kind.as_str())
        {
            return Err(invalid("Unknown theme."));
        }
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        if state.theme.as_ref() != Some(&theme) {
            state.theme = Some(theme);
            self.write_state(&state).await?;
            self.refresh(&mut state).await?;
            self.changed.send_modify(|n| *n += 1);
        }
        Ok(())
    }

    /// A versioned portable file of this device's settings and dictionary.
    /// Dictionary entries keep the user's order; nothing account-bound is in it.
    pub async fn export(&self) -> Result<Vec<u8>, ApiError> {
        let projection = self.projection().await?;
        let local = self.stores.load().await?;
        let settings = SettingKey::ALL
            .into_iter()
            .filter_map(|key| {
                let data = projection.records.get(&key.id())?.clone();
                Some(ExportRecord {
                    id: key.id(),
                    origin: projection.origin(&key.id()),
                    data,
                })
            })
            .collect();
        let dictionary = portable::entries(&local.vocabulary)
            .into_iter()
            .filter_map(|(id, entry)| {
                let id = id?;
                Some(ExportRecord {
                    id,
                    origin: projection.origin(&id),
                    data: Data::Entry(entry),
                })
            })
            .collect();
        let backup = Backup {
            export_id: Uuid::new_v4(),
            created_at_ms: now_ms(),
            settings,
            dictionary,
        };
        backup.check().map_err(|_| {
            invalid("The dictionary is larger than a portable file allows (200 entries of each kind, 256 KiB). Nothing was removed.")
        })?;
        serde_json::to_vec_pretty(&backup.encode()).map_err(|_| storage())
    }

    /// Validates a portable file completely, then records a preview of what
    /// importing it would change. Nothing else is written.
    pub async fn preview_file(
        &self,
        bytes: &[u8],
        categories: Vec<Category>,
        mode: Mode,
    ) -> Result<Preview, PreviewError> {
        let backup = Backup::decode(bytes).map_err(|e| {
            if e.0 == "unsupported_version" {
                PreviewError::UnsupportedVersion
            } else {
                PreviewError::Invalid(e.0)
            }
        })?;
        let records = backup
            .settings
            .into_iter()
            .chain(backup.dictionary)
            .collect();
        self.preview(
            Source::File,
            backup.export_id,
            records,
            categories,
            mode,
            Disposition::RestoreOnce,
        )
        .await
        .map_err(PreviewError::Api)
    }

    /// Records a preview of applying `records` from `source`.
    pub async fn preview(
        &self,
        source: Source,
        export_id: Uuid,
        records: Vec<ExportRecord>,
        categories: Vec<Category>,
        mode: Mode,
        disposition: Disposition,
    ) -> Result<Preview, ApiError> {
        let categories: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|c| categories.contains(c))
            .collect();
        if categories.is_empty() {
            return Err(invalid("Choose settings, dictionary or both."));
        }
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        let local = self.refresh(&mut state).await?;
        let selected: Vec<ExportRecord> = records
            .into_iter()
            .filter(|r| categories.contains(&r.data.category()))
            .collect();
        let view = self.compute(
            &state,
            &local,
            &selected,
            &categories,
            mode,
            disposition,
            export_id,
        );
        state.preview = Some(Pending {
            view: view.clone(),
            source,
            records: selected.iter().map(ExportRecord::encode).collect(),
        });
        self.write_state(&state).await?;
        Ok(view)
    }

    #[allow(clippy::too_many_arguments)]
    fn compute(
        &self,
        state: &State,
        local: &Local,
        records: &[ExportRecord],
        categories: &[Category],
        mode: Mode,
        disposition: Disposition,
        export_id: Uuid,
    ) -> Preview {
        let mut view = Preview {
            import_id: Uuid::new_v4(),
            export_id,
            mode,
            categories: categories.to_vec(),
            disposition,
            expected_local_revision: state.local_revision,
            settings: vec![],
            dictionary: DictionaryPreview::default(),
            apply_ids: vec![],
            preserve_ids: vec![],
            conflict_ids: vec![],
        };
        let settings: Vec<Setting> = records
            .iter()
            .filter_map(|r| match &r.data {
                Data::Setting(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let trial = portable::apply_settings(
            local,
            state.theme.as_ref(),
            &state.shadow,
            &settings,
            self.caps.as_ref(),
        );
        let current = portable::project(local, state.theme.as_ref(), &state.shadow);
        for setting in &settings {
            let key = setting.key();
            let outcome = trial
                .outcomes
                .get(&key)
                .cloned()
                .unwrap_or(Outcome::Unchanged);
            match outcome {
                Outcome::Preserved(_) => view.preserve_ids.push(key.id()),
                _ => view.apply_ids.push(key.id()),
            }
            let before = current.get(&key.id()).map(Data::encode);
            let after = setting.encode();
            if before.as_ref() != Some(&after) {
                view.settings.push(SettingChange {
                    id: key.id(),
                    key,
                    current: before.map(|v| v["value"].clone()),
                    incoming: after["value"].clone(),
                    outcome: serde_json::to_value(&outcome).unwrap_or_default(),
                });
            }
        }
        let own: BTreeMap<Uuid, Entry> = portable::entries(&local.vocabulary)
            .into_iter()
            .filter_map(|(id, e)| Some((id?, e)))
            .collect();
        let incoming_ids: BTreeSet<Uuid> = records.iter().map(|r| r.id).collect();
        for record in records {
            let Data::Entry(entry) = &record.data else {
                continue;
            };
            match own.get(&record.id) {
                None => {
                    let overlaps = own.values().any(|e| {
                        e.kind() == entry.kind() && e.texts().first() == entry.texts().first()
                    });
                    view.dictionary.overlapping += usize::from(overlaps);
                    view.dictionary.added.push(EntryView {
                        id: record.id,
                        entry: entry.encode(),
                    });
                    view.apply_ids.push(record.id);
                }
                Some(mine) if mine == entry => {
                    view.dictionary.unchanged += 1;
                    view.apply_ids.push(record.id);
                }
                Some(mine) => {
                    view.dictionary.conflicts.push(EntryConflict {
                        id: record.id,
                        local: mine.encode(),
                        incoming: entry.encode(),
                    });
                    view.conflict_ids.push(record.id);
                }
            }
        }
        if categories.contains(&Category::Dictionary) {
            if mode == Mode::ReplaceSelected {
                for (id, entry) in &own {
                    if !incoming_ids.contains(id) {
                        view.dictionary.removed.push(EntryView {
                            id: *id,
                            entry: entry.encode(),
                        });
                    }
                }
            }
            view.dictionary.reordered = !view.dictionary.added.is_empty()
                || !view.dictionary.removed.is_empty()
                || !sorted(&local.vocabulary);
        }
        view
    }

    /// Applies the recorded preview atomically. Every conflict needs a choice;
    /// a setting change may be declined with `Choice::Local`. An import that
    /// already applied returns its original receipt.
    pub async fn apply(
        &self,
        import_id: Uuid,
        choices: &BTreeMap<Uuid, Choice>,
    ) -> Result<(Receipt, Option<Applying>), ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        if let Some(done) = state.receipts.iter().find(|r| r.key == import_id) {
            return Ok((done.receipt.clone(), None));
        }
        let not_applied = |reason| Receipt::NotApplied { import_id, reason };
        let Some(pending) = state
            .preview
            .clone()
            .filter(|p| p.view.import_id == import_id)
        else {
            return Ok((not_applied(NotApplied::StalePreview), None));
        };
        let local = self.refresh(&mut state).await?;
        if state.local_revision != pending.view.expected_local_revision {
            return Ok((not_applied(NotApplied::StalePreview), None));
        }
        if pending
            .view
            .conflict_ids
            .iter()
            .any(|id| !choices.contains_key(id))
        {
            return Ok((not_applied(NotApplied::Conflict), None));
        }
        let records: Vec<ExportRecord> = pending
            .records
            .iter()
            .filter_map(decode_export)
            .filter(|r| choices.get(&r.id) != Some(&Choice::Local))
            .collect();
        let settings: Vec<Setting> = records
            .iter()
            .filter_map(|r| match &r.data {
                Data::Setting(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let Applied {
            local: mut next,
            theme,
            shadow,
            outcomes,
        } = portable::apply_settings(
            &local,
            state.theme.as_ref(),
            &state.shadow,
            &settings,
            self.caps.as_ref(),
        );
        let mut origins = state.origins.clone();
        let touched: BTreeSet<Category> = pending.view.categories.iter().copied().collect();
        if touched.contains(&Category::Dictionary) {
            for id in pending.view.dictionary.removed.iter().map(|e| e.id) {
                portable::remove_entry(&mut next.vocabulary, id);
            }
            for record in &records {
                if let Data::Entry(entry) = &record.data {
                    portable::upsert_entry(&mut next.vocabulary, record.id, entry);
                    origins.insert(record.id, record.origin);
                }
            }
            portable::sort_entries(&mut next.vocabulary);
            if crate::vocabulary::validate_document(&next.vocabulary).is_err() {
                return Ok((not_applied(NotApplied::Invalid), None));
            }
        }
        for record in &records {
            if let Data::Setting(_) = record.data {
                origins.insert(record.id, record.origin);
            }
        }
        let before = Image {
            local: local.clone(),
            theme: state.theme.clone(),
            shadow: state.shadow.clone(),
            origins: state.origins.clone(),
        };
        let after = Image {
            local: next,
            theme,
            shadow,
            origins,
        };
        let snapshot_id = Uuid::new_v4();
        let applied_theme = after
            .theme
            .clone()
            .filter(|t| Some(t) != before.theme.as_ref());
        let result = self
            .commit(&mut state, &before, &after, import_id, |state, revision| {
                state.preview = None;
                state.snapshot = Some(Snapshot {
                    id: snapshot_id,
                    import_id,
                    revision,
                    touched: touched.clone(),
                    before: before.clone(),
                });
                Receipt::Applied {
                    import_id,
                    local_revision: revision,
                    rollback_snapshot_id: snapshot_id,
                    outcomes: outcomes
                        .iter()
                        .map(|(k, o)| (*k, serde_json::to_value(o).unwrap_or_default()))
                        .collect(),
                    theme: applied_theme.clone(),
                }
            })
            .await;
        match result {
            Ok(receipt) => Ok((
                receipt,
                Some(Applying {
                    source: pending.source,
                    disposition: pending.view.disposition,
                    categories: pending.view.categories,
                }),
            )),
            Err(_) => Ok((not_applied(NotApplied::StorageFailed), None)),
        }
    }

    /// Puts back the portable state an import changed, if nothing has changed
    /// since. The rollback is itself undoable and idempotent by snapshot ID.
    pub async fn rollback(
        &self,
        import_id: Uuid,
        snapshot_id: Uuid,
        expected_local_revision: u64,
    ) -> Result<Receipt, ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        if let Some(done) = state.receipts.iter().find(|r| r.key == snapshot_id) {
            return Ok(done.receipt.clone());
        }
        let not_applied = |reason| Receipt::NotApplied { import_id, reason };
        let Some(snapshot) = state
            .snapshot
            .clone()
            .filter(|s| s.id == snapshot_id && s.import_id == import_id)
        else {
            return Ok(not_applied(NotApplied::Invalid));
        };
        let local = self.refresh(&mut state).await?;
        if state.local_revision != expected_local_revision {
            return Ok(not_applied(NotApplied::StalePreview));
        }
        let before = Image {
            local: local.clone(),
            theme: state.theme.clone(),
            shadow: state.shadow.clone(),
            origins: state.origins.clone(),
        };
        // Only the categories the import touched go back.
        let mut after = before.clone();
        if snapshot.touched.contains(&Category::Settings) {
            after.local.settings = snapshot.before.local.settings.clone();
            if let (Some(modes), Some(old)) =
                (after.local.modes.as_mut(), &snapshot.before.local.modes)
            {
                if modes.profiles.iter().any(|(id, _)| *id == old.active) {
                    modes.active = old.active.clone();
                }
            }
            after.theme = snapshot.before.theme.clone();
            after.shadow = snapshot.before.shadow.clone();
        }
        if snapshot.touched.contains(&Category::Dictionary) {
            after.local.vocabulary = snapshot.before.local.vocabulary.clone();
            after.origins = snapshot.before.origins.clone();
        }
        let undo = Uuid::new_v4();
        let restored_theme = after
            .theme
            .clone()
            .filter(|t| Some(t) != before.theme.as_ref());
        let touched = snapshot.touched.clone();
        self.commit(
            &mut state,
            &before,
            &after,
            snapshot_id,
            |state, revision| {
                state.snapshot = Some(Snapshot {
                    id: undo,
                    import_id,
                    revision,
                    touched,
                    before: before.clone(),
                });
                Receipt::Applied {
                    import_id,
                    local_revision: revision,
                    rollback_snapshot_id: undo,
                    outcomes: BTreeMap::new(),
                    theme: restored_theme.clone(),
                }
            },
        )
        .await
        .or_else(|_| Ok(not_applied(NotApplied::StorageFailed)))
    }

    /// Applies records pulled from the cloud through the same journal. Returns
    /// each setting's outcome. No snapshot is kept: sync is not a restore point.
    pub async fn apply_remote(
        &self,
        changes: Vec<RemoteChange>,
    ) -> Result<BTreeMap<Uuid, Value>, ApiError> {
        if changes.is_empty() {
            return Ok(BTreeMap::new());
        }
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        let local = self.refresh(&mut state).await?;
        let settings: Vec<Setting> = changes
            .iter()
            .filter_map(|c| match &c.content {
                Some(Data::Setting(s)) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let applied = portable::apply_settings(
            &local,
            state.theme.as_ref(),
            &state.shadow,
            &settings,
            self.caps.as_ref(),
        );
        let mut next = applied.local;
        let mut origins = state.origins.clone();
        let mut dictionary = false;
        for change in &changes {
            origins.insert(change.id, change.origin);
            match &change.content {
                Some(Data::Entry(entry)) => {
                    portable::upsert_entry(&mut next.vocabulary, change.id, entry);
                    dictionary = true;
                }
                None if SettingKey::from_id(&change.id).is_none() => {
                    dictionary |= portable::remove_entry(&mut next.vocabulary, change.id);
                    origins.remove(&change.id);
                }
                _ => {}
            }
        }
        if dictionary {
            portable::sort_entries(&mut next.vocabulary);
            crate::vocabulary::validate_document(&next.vocabulary).map_err(|_| {
                invalid("Synced dictionary entries would exceed this device's dictionary limits.")
            })?;
        }
        let before = Image {
            local: local.clone(),
            theme: state.theme.clone(),
            shadow: state.shadow.clone(),
            origins: state.origins.clone(),
        };
        let after = Image {
            local: next,
            theme: applied.theme,
            shadow: applied.shadow,
            origins,
        };
        let key = Uuid::new_v4();
        self.commit(&mut state, &before, &after, key, |_, revision| {
            Receipt::Applied {
                import_id: key,
                local_revision: revision,
                rollback_snapshot_id: key,
                outcomes: BTreeMap::new(),
                theme: None,
            }
        })
        .await?;
        state.receipts.retain(|r| r.key != key);
        self.write_state(&state).await?;
        Ok(applied
            .outcomes
            .iter()
            .map(|(k, o)| (k.id(), serde_json::to_value(o).unwrap_or_default()))
            .collect())
    }

    /// Discards a pending preview, e.g. when the user cancels.
    pub async fn discard_preview(&self, import_id: Uuid) -> Result<(), ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        if state
            .preview
            .as_ref()
            .is_some_and(|p| p.view.import_id == import_id)
        {
            state.preview = None;
            self.write_state(&state).await?;
        }
        Ok(())
    }

    pub async fn pending_preview(&self, import_id: Uuid) -> Result<Option<Preview>, ApiError> {
        let _guard = self.lock.lock().await;
        Ok(self
            .read_state()
            .await?
            .preview
            .filter(|p| p.view.import_id == import_id)
            .map(|p| p.view))
    }

    /// Gives a dictionary entry a fresh identity: an entry deleted in the
    /// cloud can only come back as a new record, never under its old ID.
    pub async fn rekey_entry(&self, id: Uuid) -> Result<Option<Uuid>, ApiError> {
        let _guard = self.lock.lock().await;
        let mut state = self.read_state().await?;
        let local = self.refresh(&mut state).await?;
        let Some((_, entry)) = portable::entries(&local.vocabulary)
            .into_iter()
            .find(|(entry_id, _)| *entry_id == Some(id))
        else {
            return Ok(None);
        };
        let fresh = Uuid::new_v4();
        let mut next = local.clone();
        portable::remove_entry(&mut next.vocabulary, id);
        portable::upsert_entry(&mut next.vocabulary, fresh, &entry);
        portable::sort_entries(&mut next.vocabulary);
        let before = Image {
            local,
            theme: state.theme.clone(),
            shadow: state.shadow.clone(),
            origins: state.origins.clone(),
        };
        let mut after = before.clone();
        after.local = next;
        after.origins.remove(&id);
        let key = Uuid::new_v4();
        self.commit(&mut state, &before, &after, key, |_, revision| {
            Receipt::Applied {
                import_id: key,
                local_revision: revision,
                rollback_snapshot_id: key,
                outcomes: BTreeMap::new(),
                theme: None,
            }
        })
        .await?;
        state.receipts.retain(|r| r.key != key);
        self.write_state(&state).await?;
        Ok(Some(fresh))
    }

    /// The pending preview's source, so a caller can re-check the account.
    pub async fn pending_source(&self, import_id: Uuid) -> Result<Option<Source>, ApiError> {
        let _guard = self.lock.lock().await;
        Ok(self
            .read_state()
            .await?
            .preview
            .filter(|p| p.view.import_id == import_id)
            .map(|p| p.source))
    }

    pub async fn last_import(&self) -> Result<Option<LastImport>, ApiError> {
        let _guard = self.lock.lock().await;
        let state = self.read_state().await?;
        Ok(state.snapshot.map(|s| LastImport {
            import_id: s.import_id,
            rollback_snapshot_id: s.id,
            local_revision: s.revision,
            current_revision: state.local_revision,
            categories: s.touched.into_iter().collect(),
        }))
    }

    /// Journal → stores → state (the commit point) → journal removed.
    async fn commit(
        &self,
        state: &mut State,
        before: &Image,
        after: &Image,
        key: Uuid,
        finish: impl FnOnce(&mut State, u64) -> Receipt,
    ) -> Result<Receipt, ApiError> {
        write_json(
            &self.journal_path(),
            &Journal {
                id: key,
                before: before.clone(),
            },
        )
        .await?;
        if let Err(error) = self.stores.store(&before.local, &after.local).await {
            // Put back whatever part was written, then forget the attempt.
            let _ = self.stores.store(&after.local, &before.local).await;
            let _ = tokio::fs::remove_file(self.journal_path()).await;
            return Err(error);
        }
        let mut next = state.clone();
        next.theme = after.theme.clone();
        next.shadow = after.shadow.clone();
        next.origins = after.origins.clone();
        next.digest = digest(&portable::project(
            &after.local,
            after.theme.as_ref(),
            &after.shadow,
        ));
        next.local_revision += 1;
        let revision = next.local_revision;
        let receipt = finish(&mut next, revision);
        next.receipts.push(StoredReceipt {
            key,
            receipt: receipt.clone(),
        });
        if next.receipts.len() > RECEIPT_LIMIT {
            next.receipts.remove(0);
        }
        next.committed = Some(key);
        if let Err(error) = self.write_state(&next).await {
            let _ = self.stores.store(&after.local, &before.local).await;
            let _ = tokio::fs::remove_file(self.journal_path()).await;
            return Err(error);
        }
        *state = next;
        let _ = tokio::fs::remove_file(self.journal_path()).await;
        self.changed.send_modify(|n| *n += 1);
        Ok(receipt)
    }
}

/// What a confirmed preview asked for beyond the local change.
pub struct Applying {
    pub source: Source,
    pub disposition: Disposition,
    pub categories: Vec<Category>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LastImport {
    pub import_id: Uuid,
    pub rollback_snapshot_id: Uuid,
    pub local_revision: u64,
    pub current_revision: u64,
    pub categories: Vec<Category>,
}

#[derive(Debug)]
pub enum PreviewError {
    Invalid(&'static str),
    UnsupportedVersion,
    Api(ApiError),
}

pub fn decode_export(value: &Value) -> Option<ExportRecord> {
    let category = match value.get("category")?.as_str()? {
        "settings" => Category::Settings,
        "dictionary" => Category::Dictionary,
        _ => return None,
    };
    ExportRecord::decode(value, category).ok()
}

fn sorted(doc: &crate::transcription::VocabularyDocument) -> bool {
    doc.hints.windows(2).all(|w| w[0].id <= w[1].id)
        && doc.corrections.windows(2).all(|w| w[0].id <= w[1].id)
        && doc.snippets.windows(2).all(|w| w[0].id <= w[1].id)
}

fn digest(records: &BTreeMap<Uuid, Data>) -> String {
    let encoded: Vec<(String, Value)> = records
        .iter()
        .map(|(id, data)| (id.to_string(), data.encode()))
        .collect();
    let bytes = serde_json::to_vec(&encoded).unwrap_or_default();
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// What an unapplied (shadow) value means, for the settings screen.
#[derive(Clone, Debug, Serialize)]
pub struct Kept {
    pub key: SettingKey,
    pub value: Value,
    pub reason: Reason,
}
pub fn kept(shadow: &BTreeMap<SettingKey, Shadow>) -> Vec<Kept> {
    shadow
        .iter()
        .map(|(key, s)| Kept {
            key: *key,
            value: s.value["value"].clone(),
            reason: s.reason,
        })
        .collect()
}
