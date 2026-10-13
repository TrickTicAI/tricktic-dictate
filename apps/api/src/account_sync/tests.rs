use super::local::{Choice, Disposition, Mode, NotApplied, PreviewError, Receipt, Service};
use super::memory::{Cloud, MemoryRemote};
use super::plan::{plan, Plan, Receipt as PlanReceipt, Stored};
use super::portable::{Capabilities, Local, Modes, Readiness, Stores, SuperPolish, Theme};
use super::sync::{AccountRef, Cycle, Engine, Notice};
use super::wire::{
    Account, Backup, Category, Consent, Data, Decision, Operation, Polish, Record, Rejection,
    Setting, SettingKey, Toggle,
};
use crate::desktop_settings::default_settings;
use crate::transcription::{
    ApiError, ErrorCode, HintEntry, InputLanguage, ModelSelection, ProviderId, Settings,
    SnippetEntry, TextLanguage,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("writer-sync-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The shared UTF-8 vectors, embedded so the test binary also runs natively on
/// Windows and macOS hosts that do not share this checkout's paths.
fn fixture(name: &str) -> Value {
    let bytes: &[u8] = match name {
        "wire.json" => include_bytes!("../../../../docs/testing/account-sync/wire.json"),
        "conflicts.json" => include_bytes!("../../../../docs/testing/account-sync/conflicts.json"),
        _ => unreachable!(),
    };
    super::json::parse(bytes).unwrap()
}

// ---- Shared contract vectors ---------------------------------------------

#[test]
fn wire_fixtures_round_trip_including_transcripts() {
    let wire = fixture("wire.json");
    let catalog = &wire["catalog"];
    let op = Operation::decode(&catalog["outbox"]).unwrap();
    assert_eq!(op.encode(), catalog["outbox"]);
    let consent = Consent::decode(&catalog["consent"]).unwrap();
    assert_eq!(consent.encode(), catalog["consent"]);
    let fenced = Consent::decode(&catalog["cleanup_receipt"]["fenced_consent"]).unwrap();
    assert_eq!(fenced.dictionary.generation, 2);
    let backup_bytes = serde_json::to_vec(&catalog["backup"]).unwrap();
    let backup = Backup::decode(&backup_bytes).unwrap();
    assert_eq!(backup.encode(), catalog["backup"]);
    assert_eq!(
        Record::decode(&catalog["record"]).unwrap().encode(),
        catalog["record"]
    );
    let mut portable = 0;
    for record in wire["records"].as_array().unwrap() {
        match record["category"].as_str().unwrap() {
            "transcripts" => assert_eq!(Record::decode(record).unwrap().encode(), *record),
            _ => {
                assert_eq!(Record::decode(record).unwrap().encode(), *record);
                portable += 1;
            }
        }
    }
    assert!(portable >= 11);
}

#[test]
fn strict_json_refuses_duplicates_unknown_fields_and_versions() {
    let wire = fixture("wire.json");
    let backup = serde_json::to_string(&wire["catalog"]["backup"]).unwrap();
    let duplicated = backup.replacen(
        "\"format_version\":1",
        "\"format_version\":1,\"format_version\":1",
        1,
    );
    assert_eq!(
        Backup::decode(duplicated.as_bytes()).unwrap_err().0,
        "malformed_json"
    );
    let mut newer = wire["catalog"]["backup"].clone();
    newer["format_version"] = json!(2);
    assert_eq!(
        Backup::decode(&serde_json::to_vec(&newer).unwrap())
            .unwrap_err()
            .0,
        "unsupported_version"
    );
    let mut nested = wire["catalog"]["backup"].clone();
    nested["dictionary"][0]["data"]["api_key"] = json!("secret");
    assert!(Backup::decode(&serde_json::to_vec(&nested).unwrap()).is_err());
    let mut extension = wire["catalog"]["backup"].clone();
    extension["extensions"] = json!({ "microphone": "USB" });
    assert!(Backup::decode(&serde_json::to_vec(&extension).unwrap()).is_err());
    let mut unsafe_model = wire["catalog"]["backup"].clone();
    unsafe_model["settings"][3]["data"]["value"]["model"] = json!("../weights");
    assert!(Backup::decode(&serde_json::to_vec(&unsafe_model).unwrap()).is_err());
    let mut blank = wire["catalog"]["backup"].clone();
    blank["dictionary"][0]["data"]["text"] = json!("\u{3000}\u{a0}");
    assert!(Backup::decode(&serde_json::to_vec(&blank).unwrap()).is_err());
    let mut oversized = wire["catalog"]["backup"].clone();
    oversized["dictionary"][0]["data"]["text"] = json!("ש".repeat(4097));
    assert!(Backup::decode(&serde_json::to_vec(&oversized).unwrap()).is_err());
    oversized["dictionary"][0]["data"]["text"] = json!("ש".repeat(4096));
    assert!(Backup::decode(&serde_json::to_vec(&oversized).unwrap()).is_ok());
    assert!(Backup::decode(&vec![b' '; 512 * 1024 + 1]).is_err());
}

fn vector_account(initial: &Value, category: Category) -> Option<Account> {
    let uid = initial["uid"].as_str()?;
    let toggle = Toggle {
        enabled: initial["enabled"].as_bool().unwrap(),
        generation: initial["generation"].as_u64().unwrap(),
    };
    let mut consent = Consent::initial(uid);
    consent.binding.account_epoch = initial["account_epoch"].as_u64().unwrap();
    match category {
        Category::Settings => consent.settings = toggle,
        Category::Dictionary => consent.dictionary = toggle,
        Category::Transcripts => unreachable!(),
    }
    Some(Account {
        transcript_reset: None,
        epoch: consent.binding.account_epoch,
        active: initial["account_state"] == "active",
        consent,
    })
}

/// Rust consumes the same `conflicts.json` as the JavaScript oracle and the
/// Firestore emulator. Transcript cases belong to MW-OUATH-9's adapter.
#[test]
fn conflict_vectors_decide_like_the_oracle() {
    let cases = fixture("conflicts.json");
    let mut ran = 0;
    for case in cases.as_array().unwrap() {
        let steps = case["steps"].as_array().unwrap();
        if steps
            .iter()
            .any(|s| s["operation"]["category"] == "transcripts")
        {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let initial = &case["initial"];
        let category = match steps[0]["operation"]["category"].as_str().unwrap() {
            "settings" => Category::Settings,
            _ => Category::Dictionary,
        };
        let auth = initial["uid"].as_str().unwrap_or("").to_owned();
        let mut account = vector_account(initial, category);
        let mut records: BTreeMap<Uuid, Stored> = BTreeMap::new();
        for value in initial["records"].as_array().unwrap() {
            let record = Record::decode(value).unwrap();
            records.insert(
                record.id,
                Stored {
                    record,
                    operation_id: Uuid::new_v4(),
                    consent_generation: initial["generation"].as_u64().unwrap(),
                },
            );
        }
        let mut receipts: BTreeMap<Uuid, PlanReceipt> = BTreeMap::new();
        for step in steps {
            if let Some(patch) = step.get("authority_patch") {
                let account = account.as_mut().unwrap();
                let generation = patch["generation"].as_u64().unwrap();
                match category {
                    Category::Settings => account.consent.settings.generation = generation,
                    Category::Dictionary => account.consent.dictionary.generation = generation,
                    Category::Transcripts => unreachable!(),
                }
            }
            let op = Operation::decode(&step["operation"]).unwrap();
            let decided = plan(
                &auth,
                &op,
                account.as_ref(),
                receipts.get(&op.operation_id),
                records.get(&op.record_id),
            );
            let decision = match decided {
                Plan::Reply(decision) => decision,
                Plan::Write {
                    stored,
                    receipt,
                    decision,
                    ..
                } => {
                    records.insert(op.record_id, *stored);
                    receipts.insert(op.operation_id, receipt);
                    decision
                }
            };
            let expected = &step["expected"];
            let (status, revision) = match &decision {
                Decision::Acknowledged(r) => ("acknowledged".to_owned(), Some(r.revision)),
                Decision::Conflict(r) => ("conflict".to_owned(), Some(r.revision)),
                Decision::Rejected(reason) => (
                    serde_json::to_value(reason)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    None,
                ),
            };
            assert_eq!(status, expected["status"], "{name}");
            if let Some(expected) = expected.get("revision") {
                assert_eq!(revision, expected.as_u64(), "{name}");
            }
        }
        ran += 1;
    }
    assert_eq!(ran, 16, "every settings/dictionary vector ran");
}

// ---- Local stores ----------------------------------------------------------

#[derive(Default)]
struct Disk {
    local: Option<Local>,
    fail_vocabulary: bool,
    writes: usize,
}
#[derive(Clone, Default)]
struct MemoryStores(Arc<Mutex<Disk>>);
impl MemoryStores {
    fn new(local: Local) -> Self {
        Self(Arc::new(Mutex::new(Disk {
            local: Some(local),
            ..Default::default()
        })))
    }
    fn get(&self) -> Local {
        self.0.lock().unwrap().local.clone().unwrap()
    }
    fn edit(&self, f: impl FnOnce(&mut Local)) {
        f(self.0.lock().unwrap().local.as_mut().unwrap())
    }
}
#[async_trait::async_trait]
impl Stores for MemoryStores {
    async fn load(&self) -> Result<Local, ApiError> {
        Ok(self.get())
    }
    async fn store(&self, before: &Local, after: &Local) -> Result<(), ApiError> {
        let mut disk = self.0.lock().unwrap();
        let current = disk.local.as_mut().unwrap();
        // Stores are written one at a time, like the real ones.
        if serde_json::to_value(&before.settings).unwrap()
            != serde_json::to_value(&after.settings).unwrap()
        {
            current.settings = after.settings.clone();
        }
        if serde_json::to_value(&before.vocabulary).unwrap()
            != serde_json::to_value(&after.vocabulary).unwrap()
        {
            if disk.fail_vocabulary {
                return Err(ApiError {
                    code: ErrorCode::StorageFailed,
                    message: "disk full".into(),
                    retryable: true,
                    retry_after_ms: None,
                });
            }
            disk.local.as_mut().unwrap().vocabulary = after.vocabulary.clone();
        }
        disk.local.as_mut().unwrap().modes = after.modes.clone();
        disk.writes += 1;
        Ok(())
    }
}

#[derive(Default)]
struct Caps {
    keys: Vec<&'static str>,
    installed: Vec<&'static str>,
}
impl Capabilities for Caps {
    fn transcription(&self, model: &ModelSelection) -> Readiness {
        let provider = serde_json::to_value(&model.provider).unwrap();
        match provider.as_str().unwrap() {
            "local" if self.installed.contains(&model.model.as_str()) => Readiness::Ready,
            "local" => Readiness::ModelNotInstalled,
            "openrouter" => Readiness::Unsupported,
            p if self.keys.contains(&p) => Readiness::Ready,
            _ => Readiness::MissingKey,
        }
    }
    fn cleanup(&self, model: &ModelSelection) -> Readiness {
        let provider = serde_json::to_value(&model.provider).unwrap();
        if self.keys.contains(&provider.as_str().unwrap()) {
            Readiness::Ready
        } else {
            Readiness::MissingKey
        }
    }
    fn languages(&self, settings: &Settings) -> bool {
        // Mirrors the real rule: Hebrew-only weights cannot take English.
        !settings.transcription.selection.model.contains("hebrew")
            || matches!(settings.transcription.input_language, InputLanguage::He)
    }
}

fn modes() -> Modes {
    Modes {
        active: "regular".into(),
        profiles: ["regular", "super", "command"]
            .into_iter()
            .map(|k| (k.to_owned(), k.to_owned()))
            .collect(),
        super_polish: Some(SuperPolish {
            instructions: "Synthetic Super instructions.".into(),
            formatting: crate::cleanup::modes::Formatting::default(),
        }),
    }
}
fn device(settings: Settings, hints: &[&str]) -> Local {
    let mut vocabulary = crate::vocabulary::empty_document();
    vocabulary.hints = hints
        .iter()
        .map(|t| HintEntry {
            id: None,
            text: (*t).into(),
            enabled: true,
        })
        .collect();
    Local {
        settings,
        vocabulary,
        modes: Some(modes()),
    }
}
fn hebrew_settings() -> Settings {
    let mut s = default_settings();
    s.ui_language = TextLanguage::He;
    s.transcription.input_language = InputLanguage::Mixed;
    s.transcription.selection = ModelSelection {
        provider: ProviderId::Gemini,
        model: "gemini-3.8-flash".into(),
    };
    s
}

struct Device {
    _dir: TempDir,
    stores: MemoryStores,
    service: Arc<Service>,
    engine: Arc<Engine>,
}
async fn open(local: Local, caps: Caps) -> Device {
    let dir = TempDir::new();
    let stores = MemoryStores::new(local);
    let service = Service::open(
        dir.path().join("account-sync"),
        Arc::new(stores.clone()),
        Arc::new(caps),
    )
    .await
    .unwrap();
    let engine = Engine::open(dir.path().join("account-sync/accounts"), service.clone())
        .await
        .unwrap();
    Device {
        _dir: dir,
        stores,
        service,
        engine,
    }
}
fn all() -> Vec<Category> {
    Category::ALL.to_vec()
}
fn applied(receipt: &Receipt) -> (u64, Uuid) {
    match receipt {
        Receipt::Applied {
            local_revision,
            rollback_snapshot_id,
            ..
        } => (*local_revision, *rollback_snapshot_id),
        other => panic!("not applied: {other:?}"),
    }
}

const MIXED: &str = "שלום Writer, meeting at 10. é / e\u{301}\n\u{200f}עברית\u{200e} English 👋";

#[tokio::test]
async fn export_assigns_stable_ids_once_and_carries_no_device_state() {
    let a = open(
        device(hebrew_settings(), &["Metrix", MIXED]),
        Caps::default(),
    )
    .await;
    let first: Value = serde_json::from_slice(&a.service.export().await.unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&a.service.export().await.unwrap()).unwrap();
    let ids = |v: &Value| {
        v["dictionary"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&first), ids(&second));
    assert_ne!(first["export_id"], second["export_id"]);
    assert!(a
        .stores
        .get()
        .vocabulary
        .hints
        .iter()
        .all(|h| h.id.is_some()));
    assert_eq!(first["dictionary"][1]["data"]["text"], MIXED);
    let text = serde_json::to_string(&first).unwrap();
    for forbidden in [
        "uid",
        "revision",
        "consent",
        "microphone",
        "shortcut",
        "api_key",
        "Ctrl",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }
    // Exactly what a fresh parser accepts.
    Backup::decode(&serde_json::to_vec(&first).unwrap()).unwrap();
}

#[tokio::test]
async fn import_restores_a_fresh_profile_and_rolls_back_atomically() {
    let a = open(
        device(hebrew_settings(), &["Metrix", MIXED]),
        Caps::default(),
    )
    .await;
    a.service
        .set_theme(Theme {
            id: "nord".into(),
            kind: "dark".into(),
        })
        .await
        .unwrap();
    let file = a.service.export().await.unwrap();

    let b = open(
        device(default_settings(), &["local only"]),
        Caps {
            keys: vec!["gemini"],
            ..Default::default()
        },
    )
    .await;
    let original = b.stores.get();
    let preview = b
        .service
        .preview_file(&file, all(), Mode::Merge)
        .await
        .unwrap();
    assert_eq!(preview.dictionary.added.len(), 2);
    assert!(preview.conflict_ids.is_empty());
    assert!(preview
        .settings
        .iter()
        .any(|c| c.key == SettingKey::UiLanguage && c.incoming == "he"));
    // A preview changes nothing.
    assert_eq!(
        serde_json::to_value(&b.stores.get().settings).unwrap(),
        serde_json::to_value(&original.settings).unwrap()
    );
    let (receipt, _) = b
        .service
        .apply(preview.import_id, &BTreeMap::new())
        .await
        .unwrap();
    let (revision, snapshot) = applied(&receipt);
    let restored = b.stores.get();
    assert!(matches!(restored.settings.ui_language, TextLanguage::He));
    assert_eq!(
        restored.settings.transcription.selection.model,
        "gemini-3.8-flash"
    );
    assert_eq!(restored.vocabulary.hints.len(), 3);
    assert!(restored.vocabulary.hints.iter().any(|h| h.text == MIXED));
    assert_eq!(b.service.theme().await.unwrap().unwrap().kind, "dark");
    // The same import ID never applies twice.
    let (again, _) = b
        .service
        .apply(preview.import_id, &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(again, receipt);

    let undone = b
        .service
        .rollback(preview.import_id, snapshot, revision)
        .await
        .unwrap();
    applied(&undone);
    let back = b.stores.get();
    assert!(matches!(back.settings.ui_language, TextLanguage::En));
    assert_eq!(back.vocabulary.hints.len(), 1);
    assert_eq!(back.vocabulary.hints[0].text, "local only");
    // Idempotent by snapshot ID.
    assert_eq!(
        b.service
            .rollback(preview.import_id, snapshot, revision)
            .await
            .unwrap(),
        undone
    );
}

#[tokio::test]
async fn import_keeps_unusable_values_aside_and_round_trips_them() {
    let mut settings = hebrew_settings();
    settings.transcription.cleanup = crate::transcription::CleanupPolicy::Enabled {
        selection: ModelSelection {
            provider: ProviderId::Openai,
            model: "gpt-5-mini".into(),
        },
    };
    let a = open(device(settings, &[]), Caps::default()).await;
    let mut file: Value = serde_json::from_slice(&a.service.export().await.unwrap()).unwrap();
    // Recognized v1 values this desktop cannot apply (as another platform exports them).
    let settings = file["settings"].as_array_mut().unwrap();
    settings.push(
        json!({ "schema_version": 1, "id": SettingKey::Delivery.id(),
        "origin_device_id": "11111111-1111-4111-8111-000000000009", "category": "settings",
        "data": { "kind": "setting", "key": "delivery", "value": "cleaned" } }),
    );
    for record in settings.iter_mut() {
        if record["data"]["key"] == "cleanup" {
            record["data"]["value"]["reasoning"] = json!("low");
        }
    }
    let file = serde_json::to_vec(&file).unwrap();
    // No keys on this device: the Gemini model and the cleanup wait for setup.
    let b = open(device(default_settings(), &[]), Caps::default()).await;
    let preview = b
        .service
        .preview_file(&file, all(), Mode::Merge)
        .await
        .unwrap();
    assert!(preview
        .preserve_ids
        .contains(&SettingKey::TranscriptionModel.id()));
    assert!(preview.preserve_ids.contains(&SettingKey::Delivery.id()));
    let (receipt, _) = b
        .service
        .apply(preview.import_id, &BTreeMap::new())
        .await
        .unwrap();
    let Receipt::Applied { outcomes, .. } = &receipt else {
        panic!()
    };
    assert_eq!(
        outcomes[&SettingKey::TranscriptionModel],
        json!({ "outcome": "preserved", "reason": "missing_key" })
    );
    let local = b.stores.get();
    // Nothing was switched to a provider without a key.
    assert!(matches!(
        local.settings.transcription.selection.provider,
        ProviderId::Local
    ));
    assert!(matches!(local.settings.ui_language, TextLanguage::He));
    // The kept values come back out unchanged.
    let out: Value = serde_json::from_slice(&b.service.export().await.unwrap()).unwrap();
    let value = |key: &str| {
        out["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["data"]["key"] == key)
            .map(|r| r["data"]["value"].clone())
    };
    assert_eq!(value("delivery"), Some(json!("cleaned")));
    assert_eq!(value("transcription_model").unwrap()["provider"], "gemini");
    assert_eq!(value("cleanup").unwrap()["reasoning"], "low");
    // Choosing a model here retires the kept one.
    b.stores
        .edit(|l| l.settings.transcription.selection.model = "whisper-small".into());
    let out: Value = serde_json::from_slice(&b.service.export().await.unwrap()).unwrap();
    assert!(out["settings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["data"]["value"]["model"] == "whisper-small"));
}

#[tokio::test]
async fn stale_previews_conflicts_and_failed_writes_change_nothing() {
    let a = open(device(default_settings(), &["shared"]), Caps::default()).await;
    let file = a.service.export().await.unwrap();
    let exported: Value = serde_json::from_slice(&file).unwrap();
    let shared_id: Uuid = serde_json::from_value(exported["dictionary"][0]["id"].clone()).unwrap();

    // B holds the same entry ID with different content (an earlier import, edited).
    let mut local = device(default_settings(), &[]);
    local.vocabulary.hints.push(HintEntry {
        id: Some(shared_id),
        text: "edited here".into(),
        enabled: true,
    });
    let b = open(local, Caps::default()).await;
    let preview = b
        .service
        .preview_file(&file, all(), Mode::Merge)
        .await
        .unwrap();
    assert_eq!(preview.conflict_ids, vec![shared_id]);
    let (receipt, _) = b
        .service
        .apply(preview.import_id, &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(
        receipt,
        Receipt::NotApplied {
            import_id: preview.import_id,
            reason: NotApplied::Conflict
        }
    );
    // An edit after the preview makes it stale.
    b.stores.edit(|l| l.settings.ui_language = TextLanguage::He);
    let choices = BTreeMap::from([(shared_id, Choice::Incoming)]);
    let (receipt, _) = b.service.apply(preview.import_id, &choices).await.unwrap();
    assert!(matches!(
        receipt,
        Receipt::NotApplied {
            reason: NotApplied::StalePreview,
            ..
        }
    ));

    // A dictionary write failure leaves settings and dictionary as they were.
    let preview = b
        .service
        .preview_file(&file, all(), Mode::Merge)
        .await
        .unwrap();
    let before = b.stores.get();
    b.stores.0.lock().unwrap().fail_vocabulary = true;
    let (receipt, _) = b.service.apply(preview.import_id, &choices).await.unwrap();
    assert!(matches!(
        receipt,
        Receipt::NotApplied {
            reason: NotApplied::StorageFailed,
            ..
        }
    ));
    assert_eq!(
        serde_json::to_value(b.stores.get()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    b.stores.0.lock().unwrap().fail_vocabulary = false;
    let (receipt, _) = b.service.apply(preview.import_id, &choices).await.unwrap();
    applied(&receipt);
    assert_eq!(b.stores.get().vocabulary.hints[0].text, "shared");

    // Unknown versions are reported, not stripped.
    let mut newer = exported.clone();
    newer["format_version"] = json!(2);
    assert!(matches!(
        b.service
            .preview_file(&serde_json::to_vec(&newer).unwrap(), all(), Mode::Merge)
            .await,
        Err(PreviewError::UnsupportedVersion)
    ));
}

#[tokio::test]
async fn replace_removes_only_listed_entries_and_rollback_refuses_intervening_edits() {
    let a = open(device(default_settings(), &["from file"]), Caps::default()).await;
    let file = a.service.export().await.unwrap();
    let b = open(
        device(default_settings(), &["mine one", "mine two"]),
        Caps::default(),
    )
    .await;
    let preview = b
        .service
        .preview_file(&file, vec![Category::Dictionary], Mode::ReplaceSelected)
        .await
        .unwrap();
    assert_eq!(preview.dictionary.removed.len(), 2);
    assert!(preview.settings.is_empty(), "settings were not selected");
    let (receipt, _) = b
        .service
        .apply(preview.import_id, &BTreeMap::new())
        .await
        .unwrap();
    let (revision, snapshot) = applied(&receipt);
    assert_eq!(b.stores.get().vocabulary.hints.len(), 1);
    b.stores.edit(|l| {
        l.vocabulary.snippets.push(SnippetEntry {
            id: None,
            phrase: "sig".into(),
            text: "Alex".into(),
            enabled: true,
        })
    });
    let refused = b
        .service
        .rollback(preview.import_id, snapshot, revision)
        .await
        .unwrap();
    assert!(matches!(
        refused,
        Receipt::NotApplied {
            reason: NotApplied::StalePreview,
            ..
        }
    ));
    assert_eq!(b.stores.get().vocabulary.snippets.len(), 1);
}

#[tokio::test]
async fn an_interrupted_import_is_rolled_back_when_reopened() {
    let dir = TempDir::new();
    let stores = MemoryStores::new(device(default_settings(), &["kept"]));
    let state = dir.path().join("account-sync");
    let service = Service::open(
        state.clone(),
        Arc::new(stores.clone()),
        Arc::new(Caps::default()),
    )
    .await
    .unwrap();
    service.projection().await.unwrap();
    let before = stores.get();
    // Simulate a crash after the journal and the settings write, before commit.
    let journal = json!({ "id": Uuid::new_v4(), "before": {
        "local": before, "theme": null, "shadow": {}, "origins": {} } });
    std::fs::write(
        state.join("journal.json"),
        serde_json::to_vec(&journal).unwrap(),
    )
    .unwrap();
    stores.edit(|l| {
        l.settings.ui_language = TextLanguage::He;
        l.vocabulary.hints.clear();
    });
    drop(service);
    Service::open(
        state.clone(),
        Arc::new(stores.clone()),
        Arc::new(Caps::default()),
    )
    .await
    .unwrap();
    assert!(!state.join("journal.json").exists());
    assert_eq!(
        serde_json::to_value(stores.get()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

// ---- Account sync ------------------------------------------------------------

fn account(uid: &str) -> AccountRef {
    AccountRef {
        project_id: "demo-writer".into(),
        uid: uid.into(),
    }
}
fn yes() -> bool {
    true
}

async fn join(d: &Device, remote: &MemoryRemote, choices: BTreeMap<Uuid, Choice>) {
    let key = account(&remote.auth_uid);
    let preview = d
        .engine
        .preview(&key, remote, all(), Disposition::RestoreAndSync)
        .await
        .unwrap();
    let receipt = d
        .engine
        .confirm(&key, remote, preview.preview.import_id, &choices, &yes)
        .await
        .unwrap();
    applied(&receipt);
    assert_eq!(
        d.engine.run(&key, remote, &yes).await.unwrap(),
        Cycle::Synced
    );
}

#[tokio::test]
async fn signing_in_alone_sends_nothing_and_accounts_stay_apart() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let a_remote = MemoryRemote {
        auth_uid: "user-a".into(),
        cloud: cloud.clone(),
    };
    let d = open(device(default_settings(), &["private"]), Caps::default()).await;
    assert_eq!(
        d.engine
            .run(&account("user-a"), &a_remote, &yes)
            .await
            .unwrap(),
        Cycle::Idle
    );
    assert!(cloud.lock().unwrap().accounts.is_empty());
    // A preview reads; it does not create the account or upload.
    d.engine
        .preview(
            &account("user-a"),
            &a_remote,
            all(),
            Disposition::RestoreAndSync,
        )
        .await
        .unwrap();
    assert!(cloud.lock().unwrap().accounts.is_empty());
    join(&d, &a_remote, BTreeMap::new()).await;
    assert!(cloud.lock().unwrap().commits > 0);
    let status_a = d.engine.status(&account("user-a")).await.unwrap();
    assert_eq!(status_a.pending, 0);
    // Account B on the same device starts from nothing and cannot see A.
    let b_remote = MemoryRemote {
        auth_uid: "user-b".into(),
        cloud: cloud.clone(),
    };
    let status_b = d.engine.status(&account("user-b")).await.unwrap();
    assert!(status_b.joined.is_empty());
    assert!(b_remote.records().await.unwrap().is_empty());
    // After sign-out, A's device state is frozen until it rejoins.
    d.engine.signed_out(&account("user-a")).await.unwrap();
    d.stores.edit(|l| l.settings.ui_language = TextLanguage::He);
    let commits = cloud.lock().unwrap().commits;
    assert_eq!(
        d.engine
            .run(&account("user-a"), &a_remote, &yes)
            .await
            .unwrap(),
        Cycle::Idle
    );
    assert_eq!(cloud.lock().unwrap().commits, commits);
}

use super::sync::Remote as _;

#[tokio::test]
async fn two_devices_restore_edit_offline_and_merge() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let remote = MemoryRemote {
        auth_uid: "user-a".into(),
        cloud: cloud.clone(),
    };
    let key = account("user-a");
    let a = open(
        device(hebrew_settings(), &["Metrix", MIXED]),
        Caps {
            keys: vec!["gemini"],
            ..Default::default()
        },
    )
    .await;
    join(&a, &remote, BTreeMap::new()).await;

    // A fresh second device restores and keeps syncing.
    let b = open(
        device(default_settings(), &[]),
        Caps {
            keys: vec!["gemini"],
            ..Default::default()
        },
    )
    .await;
    let preview = b
        .engine
        .preview(&key, &remote, all(), Disposition::RestoreAndSync)
        .await
        .unwrap();
    assert_eq!(preview.cloud[&Category::Dictionary], 2);
    assert!(preview.enabled[&Category::Settings]);
    let receipt = b
        .engine
        .confirm(
            &key,
            &remote,
            preview.preview.import_id,
            &BTreeMap::new(),
            &yes,
        )
        .await
        .unwrap();
    applied(&receipt);
    b.engine.run(&key, &remote, &yes).await.unwrap();
    let restored = b.stores.get();
    assert!(matches!(restored.settings.ui_language, TextLanguage::He));
    assert!(restored.vocabulary.hints.iter().any(|h| h.text == MIXED));

    // Both edit different things while offline.
    cloud.lock().unwrap().offline = true;
    a.stores.edit(|l| {
        l.vocabulary.hints.push(HintEntry {
            id: None,
            text: "from A".into(),
            enabled: true,
        })
    });
    b.stores
        .edit(|l| l.settings.transcription.input_language = InputLanguage::He);
    assert_eq!(
        a.engine.run(&key, &remote, &yes).await.unwrap(),
        Cycle::Deferred
    );
    assert_eq!(
        a.engine.status(&key).await.unwrap().last_error,
        Some(super::sync::RemoteError::Offline)
    );
    cloud.lock().unwrap().offline = false;
    a.engine.run(&key, &remote, &yes).await.unwrap();
    b.engine.run(&key, &remote, &yes).await.unwrap();
    a.engine.run(&key, &remote, &yes).await.unwrap();
    assert!(b
        .stores
        .get()
        .vocabulary
        .hints
        .iter()
        .any(|h| h.text == "from A"));
    assert!(matches!(
        a.stores.get().settings.transcription.input_language,
        InputLanguage::He
    ));
    assert!(a.engine.status(&key).await.unwrap().conflicts.is_empty());
}

#[tokio::test]
async fn a_lost_reply_is_retried_with_the_same_operation() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let remote = MemoryRemote {
        auth_uid: "user-a".into(),
        cloud: cloud.clone(),
    };
    let key = account("user-a");
    let a = open(device(default_settings(), &[]), Caps::default()).await;
    join(&a, &remote, BTreeMap::new()).await;
    a.stores.edit(|l| l.settings.ui_language = TextLanguage::He);
    cloud.lock().unwrap().lose_next_reply = true;
    assert_eq!(
        a.engine.run(&key, &remote, &yes).await.unwrap(),
        Cycle::Deferred
    );
    assert_eq!(a.engine.status(&key).await.unwrap().pending, 1);
    let commits = cloud.lock().unwrap().commits;
    // Process restart: a new engine over the same files resends the stored op.
    let engine = Engine::open(
        a._dir.path().join("account-sync/accounts"),
        a.service.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        engine.run(&key, &remote, &yes).await.unwrap(),
        Cycle::Synced
    );
    assert_eq!(cloud.lock().unwrap().commits, commits, "no second write");
    let record = remote
        .records()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == SettingKey::UiLanguage.id())
        .unwrap();
    assert_eq!(record.revision, 2);
    assert_eq!(engine.status(&key).await.unwrap().pending, 0);
}

#[tokio::test]
async fn concurrent_edits_of_one_setting_are_kept_for_the_user() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let remote = MemoryRemote {
        auth_uid: "user-a".into(),
        cloud: cloud.clone(),
    };
    let key = account("user-a");
    let a = open(device(default_settings(), &[]), Caps::default()).await;
    join(&a, &remote, BTreeMap::new()).await;
    let b = open(device(default_settings(), &[]), Caps::default()).await;
    join(&b, &remote, BTreeMap::new()).await;
    a.stores
        .edit(|l| l.settings.transcription.input_language = InputLanguage::Mixed);
    b.stores
        .edit(|l| l.settings.transcription.input_language = InputLanguage::En);
    a.engine.run(&key, &remote, &yes).await.unwrap();
    b.engine.run(&key, &remote, &yes).await.unwrap();
    let conflicts = b.engine.status(&key).await.unwrap().conflicts;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].key, Some(SettingKey::InputLanguage));
    assert_eq!(conflicts[0].local, Some(json!("en")));
    assert_eq!(conflicts[0].remote, Some(json!("mixed")));
    // Neither value was overwritten while the conflict is open.
    assert!(matches!(
        b.stores.get().settings.transcription.input_language,
        InputLanguage::En
    ));
    b.engine
        .resolve(&key, conflicts[0].id, Choice::Local)
        .await
        .unwrap();
    b.engine.run(&key, &remote, &yes).await.unwrap();
    a.engine.run(&key, &remote, &yes).await.unwrap();
    assert!(matches!(
        a.stores.get().settings.transcription.input_language,
        InputLanguage::En
    ));
}

#[tokio::test]
async fn a_deleted_entry_is_not_resurrected_by_a_stale_edit() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let remote = MemoryRemote {
        auth_uid: "user-a".into(),
        cloud: cloud.clone(),
    };
    let key = account("user-a");
    let a = open(device(default_settings(), &["doomed"]), Caps::default()).await;
    join(&a, &remote, BTreeMap::new()).await;
    let b = open(device(default_settings(), &[]), Caps::default()).await;
    join(&b, &remote, BTreeMap::new()).await;
    // A deletes; B, not yet having pulled, edits the same entry.
    a.stores.edit(|l| l.vocabulary.hints.clear());
    a.engine.run(&key, &remote, &yes).await.unwrap();
    b.stores
        .edit(|l| l.vocabulary.hints[0].text = "edited offline".into());
    b.engine.run(&key, &remote, &yes).await.unwrap();
    let conflicts = b.engine.status(&key).await.unwrap().conflicts;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].remote, None);
    // Keeping it gives the entry a new identity; the tombstone stays final.
    let old = conflicts[0].id;
    b.engine.resolve(&key, old, Choice::Local).await.unwrap();
    b.engine.run(&key, &remote, &yes).await.unwrap();
    let records = remote.records().await.unwrap();
    assert!(records.iter().any(|r| r.id == old && r.content.is_none()));
    assert!(records.iter().any(|r| r.id != old
        && matches!(&r.content, Some(super::wire::Data::Entry(super::wire::Entry::Hint { text, .. })) if text == "edited offline")));
    a.engine.run(&key, &remote, &yes).await.unwrap();
    assert_eq!(a.stores.get().vocabulary.hints[0].text, "edited offline");
}

#[tokio::test]
async fn turning_sync_off_fences_other_devices_queued_work() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let remote = MemoryRemote {
        auth_uid: "user-a".into(),
        cloud: cloud.clone(),
    };
    let key = account("user-a");
    let a = open(device(default_settings(), &[]), Caps::default()).await;
    join(&a, &remote, BTreeMap::new()).await;
    let b = open(device(default_settings(), &[]), Caps::default()).await;
    join(&b, &remote, BTreeMap::new()).await;
    b.stores.edit(|l| {
        l.vocabulary.hints.push(HintEntry {
            id: None,
            text: "queued while off".into(),
            enabled: true,
        })
    });
    a.engine
        .turn_off(&key, &remote, Category::Dictionary)
        .await
        .unwrap();
    let commits = cloud.lock().unwrap().commits;
    b.engine.run(&key, &remote, &yes).await.unwrap();
    assert_eq!(cloud.lock().unwrap().commits, commits);
    let status = b.engine.status(&key).await.unwrap();
    assert_eq!(status.joined, vec![Category::Settings]);
    assert_eq!(
        status.notices,
        vec![Notice::CategoryStopped(Category::Dictionary)]
    );
    // The entry is still on B.
    assert!(b
        .stores
        .get()
        .vocabulary
        .hints
        .iter()
        .any(|h| h.text == "queued while off"));
}

#[tokio::test]
async fn a_response_after_the_session_changed_is_not_applied() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let remote = MemoryRemote {
        auth_uid: "user-a".into(),
        cloud: cloud.clone(),
    };
    let key = account("user-a");
    let a = open(device(default_settings(), &[]), Caps::default()).await;
    join(&a, &remote, BTreeMap::new()).await;
    a.stores.edit(|l| l.settings.ui_language = TextLanguage::He);
    let no = || false;
    assert_eq!(
        a.engine.run(&key, &remote, &no).await.unwrap(),
        Cycle::Deferred
    );
    // Nothing was sent, and the intent stays queued for this account only.
    let record = remote
        .records()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == SettingKey::UiLanguage.id())
        .unwrap();
    assert_eq!(record.revision, 1);
}

#[test]
fn setting_ids_match_the_contract() {
    assert_eq!(
        SettingKey::UiLanguage.id().to_string(),
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        SettingKey::Theme.id().to_string(),
        "00000000-0000-4000-8000-000000000008"
    );
    assert!(crate::vocabulary::is_reserved_id(&SettingKey::Theme.id()));
    let _ = Rejection::Deleted;
}

/// The desktop REST transport against a private Firestore emulator running the
/// real `firestore.rules`. Run through
/// `node scripts/smoke/account-sync/firestore-rules.mjs --rust`; skipped otherwise.
mod emulator {
    use super::*;
    use crate::account_sync::firestore::{client, Firestore};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    const PROJECT: &str = "demo-metrix-writer";

    fn host() -> Option<String> {
        std::env::var("FIRESTORE_EMULATOR_HOST").ok()
    }
    /// An unsigned emulator token, as the Firebase test SDK issues them.
    fn token(uid: &str) -> String {
        let now = local_now() / 1000;
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let claims = json!({ "iss": format!("https://securetoken.google.com/{PROJECT}"),
            "aud": PROJECT, "auth_time": now, "iat": now, "exp": now + 3600, "sub": uid,
            "user_id": uid, "firebase": { "sign_in_provider": "google.com", "identities": {} } });
        format!(
            "{header}.{}.",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        )
    }
    fn local_now() -> u64 {
        crate::account_sync::local::now_ms()
    }
    fn remote(path_uid: &str, token_uid: &str) -> Firestore {
        Firestore::new(
            &format!("http://{}/v1", host().unwrap()),
            PROJECT,
            path_uid,
            token(token_uid),
            client(),
        )
        .unwrap()
    }
    fn key(uid: &str) -> AccountRef {
        AccountRef {
            project_id: PROJECT.into(),
            uid: uid.into(),
        }
    }
    async fn join(d: &Device, uid: &str, remote: &Firestore) {
        let preview = d
            .engine
            .preview(&key(uid), remote, all(), Disposition::RestoreAndSync)
            .await
            .unwrap();
        let receipt = d
            .engine
            .confirm(
                &key(uid),
                remote,
                preview.preview.import_id,
                &BTreeMap::new(),
                &yes,
            )
            .await
            .unwrap();
        applied(&receipt);
        assert_eq!(
            d.engine.run(&key(uid), remote, &yes).await.unwrap(),
            Cycle::Synced
        );
    }

    #[tokio::test]
    #[ignore = "needs the Firestore emulator"]
    async fn desktop_sync_through_the_emulator_rules() {
        assert!(host().is_some(), "run through firestore-rules.mjs --rust");
        let uid = format!("user-{}", Uuid::new_v4().simple());
        let a_remote = remote(&uid, &uid);
        let caps = || Caps {
            keys: vec!["gemini"],
            ..Default::default()
        };
        // Nothing exists until the user turns sync on.
        assert!(a_remote.account().await.unwrap().is_none());
        let a = open(device(hebrew_settings(), &["Metrix", MIXED]), caps()).await;
        assert_eq!(
            a.engine.run(&key(&uid), &a_remote, &yes).await.unwrap(),
            Cycle::Idle
        );
        assert!(a_remote.account().await.unwrap().is_none());
        join(&a, &uid, &a_remote).await;
        let account = a_remote.account().await.unwrap().unwrap();
        assert!(account.consent.settings.enabled && account.consent.dictionary.enabled);
        assert_eq!(account.consent.settings.generation, 2);
        let records = a_remote.records().await.unwrap();
        assert!(records.iter().any(|r| matches!(&r.content,
            Some(super::super::wire::Data::Entry(super::super::wire::Entry::Hint { text, .. })) if text == MIXED)));

        // Another user can neither read nor write A's tree.
        let intruder = remote(&uid, "someone-else");
        assert!(intruder.records().await.is_err());
        assert!(intruder.account().await.is_err());

        // A fresh device restores and keeps syncing; exact text survives.
        let b = open(device(default_settings(), &[]), caps()).await;
        join(&b, &uid, &a_remote).await;
        assert!(b
            .stores
            .get()
            .vocabulary
            .hints
            .iter()
            .any(|h| h.text == MIXED));
        assert!(matches!(
            b.stores.get().settings.ui_language,
            TextLanguage::He
        ));

        // Same field edited on both: one wins by CAS, the other keeps a conflict.
        a.stores
            .edit(|l| l.settings.transcription.input_language = InputLanguage::He);
        b.stores
            .edit(|l| l.settings.transcription.input_language = InputLanguage::En);
        a.engine.run(&key(&uid), &a_remote, &yes).await.unwrap();
        b.engine.run(&key(&uid), &a_remote, &yes).await.unwrap();
        let conflicts = b.engine.status(&key(&uid)).await.unwrap().conflicts;
        assert_eq!(conflicts.len(), 1);
        b.engine
            .resolve(&key(&uid), conflicts[0].id, Choice::Incoming)
            .await
            .unwrap();
        assert!(matches!(
            b.stores.get().settings.transcription.input_language,
            InputLanguage::He
        ));

        // A replayed operation is acknowledged once, without a second write.
        b.stores.edit(|l| {
            l.vocabulary.hints.push(HintEntry {
                id: None,
                text: "from B".into(),
                enabled: true,
            })
        });
        b.engine.run(&key(&uid), &a_remote, &yes).await.unwrap();
        let entry = a_remote
            .records()
            .await
            .unwrap()
            .into_iter()
            .find(|r| matches!(&r.content, Some(super::super::wire::Data::Entry(e)) if e.texts()[0] == "from B"))
            .unwrap();
        let replay = Operation {
            binding: super::super::wire::Binding {
                uid: uid.clone(),
                account_epoch: 1,
            },
            consent_generation: 2,
            history_enrollment_id: None,
            category: Category::Dictionary,
            operation_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            record_id: entry.id,
            origin: entry.origin,
            base_revision: entry.revision,
            content: None,
        };
        let first = a_remote.commit(&replay).await.unwrap();
        assert_eq!(a_remote.commit(&replay).await.unwrap(), first);
        assert!(matches!(first, Decision::Acknowledged(ref r) if r.revision == entry.revision + 1));
        a.engine.run(&key(&uid), &a_remote, &yes).await.unwrap();
        assert!(!a
            .stores
            .get()
            .vocabulary
            .hints
            .iter()
            .any(|h| h.text == "from B"));

        // Turning dictionary sync off fences B's queued work at the rules.
        b.stores.edit(|l| {
            l.vocabulary.hints.push(HintEntry {
                id: None,
                text: "late".into(),
                enabled: true,
            })
        });
        a.engine
            .turn_off(&key(&uid), &a_remote, Category::Dictionary)
            .await
            .unwrap();
        b.engine.run(&key(&uid), &a_remote, &yes).await.unwrap();
        let status = b.engine.status(&key(&uid)).await.unwrap();
        assert_eq!(status.joined, vec![Category::Settings]);
        assert!(!a_remote
            .records()
            .await
            .unwrap()
            .iter()
            .any(|r| matches!(&r.content, Some(super::super::wire::Data::Entry(e)) if e.texts()[0] == "late")));
        let stale = Operation {
            operation_id: Uuid::new_v4(),
            record_id: Uuid::new_v4(),
            base_revision: 0,
            content: Some(super::super::wire::Data::Entry(
                super::super::wire::Entry::Hint {
                    enabled: true,
                    text: "stale".into(),
                },
            )),
            ..replay
        };
        assert_eq!(
            a_remote.commit(&stale).await.unwrap(),
            Decision::Rejected(Rejection::StaleGeneration)
        );
    }
}

#[test]
fn polish_instructions_decode_reject_and_bind_to_their_fixed_id() {
    let wire = fixture("wire.json");
    let record = wire["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "00000000-0000-4000-8000-000000000009")
        .unwrap();
    assert_eq!(Record::decode(record).unwrap().encode(), *record);
    let setting = Setting::decode(&record["content"]["data"]).unwrap();
    assert_eq!(setting.key(), SettingKey::PolishInstructions);
    let Setting::PolishInstructions(polish) = &setting else {
        panic!()
    };
    assert_eq!(polish.instructions, "Synthetic polish instructions.");
    assert_eq!(
        SettingKey::PolishInstructions.id().to_string(),
        "00000000-0000-4000-8000-000000000009"
    );
    let good = record["content"]["data"]["value"].clone();
    let mut variants: Vec<Value> = Vec::new();
    for instructions in ["", "   \n\t", &"x".repeat(16385), "ok\0", "\u{a0}"] {
        let mut bad = good.clone();
        bad["instructions"] = json!(instructions);
        variants.push(bad);
    }
    // "\u{a0}" above is a lone no-break space: blank under trim, so rejected.
    for (key, value) in [
        ("rewrite", json!("flowery")),
        ("layout", json!("two_columns")),
        ("lists", json!("yes")),
    ] {
        let mut bad = good.clone();
        bad["formatting"][key] = value;
        variants.push(bad);
    }
    let mut missing = good.clone();
    missing
        .as_object_mut()
        .unwrap()
        .get_mut("formatting")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("spoken_punctuation");
    variants.push(missing);
    let mut extra = good.clone();
    extra["prompt_versions"] = json!([]);
    variants.push(extra);
    let mut bare = good.clone();
    bare["instructions"] = json!(42);
    variants.push(bare);
    for bad in &variants {
        assert!(
            Setting::decode(&json!({
                "kind": "setting", "key": "polish_instructions", "value": bad
            }))
            .is_err(),
            "accepted {bad}"
        );
    }
    // The setting is bound to its own fixed ID.
    let mut moved = record.clone();
    moved["id"] = json!("00000000-0000-4000-8000-000000000008");
    assert!(Record::decode(&moved).is_err());
    let mut tenth = wire["catalog"]["backup"].clone();
    let ninth = tenth["settings"][8].clone();
    tenth["settings"].as_array_mut().unwrap().push(ninth);
    assert_eq!(
        Backup::decode(&serde_json::to_vec(&tenth).unwrap())
            .unwrap_err()
            .0,
        "too_many_records"
    );
}

#[test]
fn polish_instructions_project_apply_and_shadow_like_other_settings() {
    use super::portable::{apply_settings, local_setting, project, Outcome, Reason};
    use crate::cleanup::modes::{Formatting, Layout, Rewrite};
    let local = device(default_settings(), &[]);
    // The Super profile's wording is this device's portable value.
    let Setting::PolishInstructions(projected) =
        local_setting(&local, None, SettingKey::PolishInstructions).unwrap()
    else {
        panic!()
    };
    assert_eq!(projected.instructions, "Synthetic Super instructions.");
    assert_eq!(projected.formatting, Formatting::default());
    // A new wording applies onto the Super profile.
    let incoming = Setting::PolishInstructions(Polish {
        instructions: "Phone wording.".into(),
        formatting: Formatting {
            rewrite: Rewrite::Concise,
            layout: Layout::SingleLine,
            ..Formatting::default()
        },
    });
    let applied = apply_settings(
        &local,
        None,
        &BTreeMap::new(),
        std::slice::from_ref(&incoming),
        &Caps::default(),
    );
    assert_eq!(
        applied.outcomes[&SettingKey::PolishInstructions],
        Outcome::Applied
    );
    let modes = applied.local.modes.unwrap();
    assert_eq!(modes.super_polish.unwrap().instructions, "Phone wording.");
    // Without a Super profile the wording is kept aside and still exports.
    let mut bare = local.clone();
    bare.modes.as_mut().unwrap().super_polish = None;
    assert!(local_setting(&bare, None, SettingKey::PolishInstructions).is_none());
    let kept = apply_settings(&bare, None, &BTreeMap::new(), &[incoming], &Caps::default());
    assert_eq!(
        kept.outcomes[&SettingKey::PolishInstructions],
        Outcome::Preserved(Reason::NoMatchingMode)
    );
    let projected = project(&bare, None, &kept.shadow);
    let Data::Setting(Setting::PolishInstructions(kept_value)) =
        &projected[&SettingKey::PolishInstructions.id()]
    else {
        panic!()
    };
    assert_eq!(kept_value.instructions, "Phone wording.");
    // Without any mode library the setting is unsupported.
    let mut none = local.clone();
    none.modes = None;
    let kept = apply_settings(
        &none,
        None,
        &BTreeMap::new(),
        &[Setting::decode(&json!({
            "kind": "setting", "key": "polish_instructions",
            "value": {
                "instructions": "Phone wording.",
                "formatting": {
                    "rewrite": "polished", "layout": "automatic", "lists": true,
                    "remove_fillers": true, "self_corrections": true,
                    "spoken_punctuation": true,
                },
            },
        }))
        .unwrap()],
        &Caps::default(),
    );
    assert_eq!(
        kept.outcomes[&SettingKey::PolishInstructions],
        Outcome::Preserved(Reason::Unsupported)
    );
}

/// The sync planner's fixtures use MemoryStores. This adapter exercises the
/// actual dictionary persistence/ETags through sync cycles on two devices.
struct DiskVocabularyStores {
    memory: MemoryStores,
    vocabulary: Arc<crate::vocabulary::VocabularyStore>,
}
#[async_trait::async_trait]
impl Stores for DiskVocabularyStores {
    async fn load(&self) -> Result<Local, ApiError> {
        let mut local = self.memory.load().await?;
        local.vocabulary = self.vocabulary.load().await?;
        Ok(local)
    }
    async fn store(&self, before: &Local, after: &Local) -> Result<(), ApiError> {
        if serde_json::to_value(&before.vocabulary).unwrap()
            != serde_json::to_value(&after.vocabulary).unwrap()
        {
            self.vocabulary.save(&after.vocabulary).await?;
        }
        self.memory.store(before, after).await
    }
}
async fn disk_device(local: Local) -> (Device, Arc<crate::vocabulary::VocabularyStore>) {
    let dir = TempDir::new();
    let vocabulary = Arc::new(crate::vocabulary::VocabularyStore::new(
        dir.path().join("vocabulary.json"),
    ));
    vocabulary.save(&local.vocabulary).await.unwrap();
    let stores = MemoryStores::new(local);
    let service = Service::open(
        dir.path().join("portable"),
        Arc::new(DiskVocabularyStores {
            memory: stores.clone(),
            vocabulary: vocabulary.clone(),
        }),
        Arc::new(Caps::default()),
    )
    .await
    .unwrap();
    let engine = Engine::open(dir.path().join("accounts"), service.clone())
        .await
        .unwrap();
    (
        Device {
            _dir: dir,
            stores,
            service,
            engine,
        },
        vocabulary,
    )
}

#[tokio::test]
async fn dictionary_edits_keep_etags_and_cloud_identities_through_sync_cycles() {
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let remote = MemoryRemote {
        auth_uid: "dictionary-user".into(),
        cloud: cloud.clone(),
    };
    let key = account("dictionary-user");
    let mut initial = device(default_settings(), &["שלום Writer"]);
    initial
        .vocabulary
        .corrections
        .push(crate::transcription::CorrectionEntry {
            id: None,
            from: "met ricks".into(),
            to: "Metrix".into(),
            enabled: true,
        });
    initial.vocabulary.snippets.push(SnippetEntry {
        id: None,
        phrase: "signature".into(),
        text: "שלום\nWriter ✓".into(),
        enabled: true,
    });
    let (a, a_store) = disk_device(initial).await;
    join(&a, &remote, BTreeMap::new()).await;
    let (b, b_store) = disk_device(device(default_settings(), &[])).await;
    join(&b, &remote, BTreeMap::new()).await;
    let (mut draft, mut etag) = a_store.load_tagged().await.unwrap();
    let original_ids = draft.ids();
    let dictionary_records = || {
        cloud
            .lock()
            .unwrap()
            .records
            .values()
            .filter(|record| record.record.category == Category::Dictionary)
            .count()
    };
    assert_eq!(dictionary_records(), 3);
    for step in 0..5 {
        match step {
            0 => draft.hints[0].text = "Edited שלום".into(),
            1 => draft.corrections[0].to = "New עברית".into(),
            2 => draft.snippets[0].text = "Exact\nשלום ✓".into(),
            3 => draft.snippets[0].enabled = false,
            4 => draft.hints.push(HintEntry {
                id: None,
                text: "Added word".into(),
                enabled: true,
            }),
            _ => unreachable!(),
        }
        (draft, etag) = a_store.save_if(&draft, Some(&etag)).await.unwrap().unwrap();
        a.engine.run(&key, &remote, &yes).await.unwrap();
        assert_eq!(
            a_store.load_tagged().await.unwrap().1,
            etag,
            "own sync invalidated step {step}"
        );
        b.engine.run(&key, &remote, &yes).await.unwrap();
        let restored = b_store.load().await.unwrap();
        assert!(restored
            .hints
            .iter()
            .any(|entry| entry.id == original_ids[0] && entry.text == draft.hints[0].text));
        assert_eq!(restored.corrections[0].id, original_ids[1]);
        assert_eq!(restored.snippets[0].id, original_ids[2]);
        assert_eq!(
            dictionary_records(),
            if step == 4 { 4 } else { 3 },
            "an edit recreated cloud identities"
        );
    }
    let added_id = draft.hints.pop().unwrap().id.unwrap();
    let (_, deletion_etag) = a_store.save_if(&draft, Some(&etag)).await.unwrap().unwrap();
    a.engine.run(&key, &remote, &yes).await.unwrap();
    b.engine.run(&key, &remote, &yes).await.unwrap();
    assert_eq!(a_store.load_tagged().await.unwrap().1, deletion_etag);
    assert!(!b_store
        .load()
        .await
        .unwrap()
        .ids()
        .contains(&Some(added_id)));
    assert!(remote
        .records()
        .await
        .unwrap()
        .iter()
        .find(|record| record.id == added_id)
        .unwrap()
        .content
        .is_none());

    // A real edit from B still invalidates A's older version.
    let mut remote_edit = b_store.load().await.unwrap();
    remote_edit.hints[0].text = "Changed on B".into();
    b_store.save(&remote_edit).await.unwrap();
    b.engine.run(&key, &remote, &yes).await.unwrap();
    a.engine.run(&key, &remote, &yes).await.unwrap();
    assert!(a_store
        .save_if(&draft, Some(&deletion_etag))
        .await
        .unwrap()
        .is_err());
    assert_eq!(a_store.load().await.unwrap().hints[0].text, "Changed on B");
}
