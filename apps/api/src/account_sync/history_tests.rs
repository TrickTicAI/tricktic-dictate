use super::{
    history::{History, TRUST},
    history_wire::HistoryMode,
    memory::{Cloud, MemoryRemote},
    sync::{AccountRef, Remote, RemoteError},
    wire::{Data, Operation, Record},
};
use crate::transcription::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer-history-{}", Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn key() -> AccountRef {
    AccountRef {
        project_id: "demo-writer".into(),
        uid: "synthetic-a".into(),
    }
}
fn remote() -> MemoryRemote {
    MemoryRemote {
        auth_uid: key().uid,
        cloud: Arc::new(Mutex::new(Cloud::default())),
    }
}
fn job(text: &str) -> (Job, u64) {
    let mut options = crate::desktop_settings::default_settings().transcription;
    options.selection = ModelSelection {
        provider: ProviderId::Gemini,
        model: "synthetic-stt".into(),
    };
    options.input_language = InputLanguage::Mixed;
    let transcript = Transcript {
        raw_text: text.into(),
        selection: options.selection.clone(),
        input_language: options.input_language.clone(),
        output_language: options.output_language.clone(),
        execution_location: ExecutionLocation::Cloud,
    };
    (
        Job {
            audio_preparation: None,
            recovery: None,
            id: Uuid::new_v4(),
            request: TranscriptionRequest {
                recording_id: Uuid::new_v4(),
                idempotency_key: Uuid::new_v4(),
                options,
                retry_of_job_id: None,
            },
            created_at_ms: 10,
            updated_at_ms: 20,
            status: JobStatus::Succeeded {
                result: TranscriptionResult {
                    transcript,
                    cleanup: CleanupResult::Off {},
                },
            },
        },
        5,
    )
}
async fn join(h: &History, r: &dyn Remote, mode: HistoryMode) {
    let p = h.preview(&key(), r, mode, &|| true).await.unwrap();
    h.confirm(&key(), r, p.id, TRUST, &|| true).await.unwrap();
}
async fn restore_all(h: &History, r: &dyn Remote) -> Vec<Value> {
    let mut after = None;
    let mut pages = 0;
    loop {
        let p = h.restore(&key(), r, after, &|| true).await.unwrap();
        pages += 1;
        assert!(p.records.len() <= super::sync::HISTORY_PAGE);
        after = p.next;
        if after.is_none() {
            break;
        }
        assert!(pages < 100);
    }
    h.local_page(Some(&key()), None).unwrap().records
}
#[tokio::test]
async fn history_consent_boundary_uses_finalized_attempts_and_exact_preview() {
    let tmp = Temp::new();
    let h = History::open(tmp.0.clone());
    let r = remote();
    let old = job("old");
    h.observe(std::slice::from_ref(&old)).unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    assert!(r.cloud.lock().unwrap().accounts.is_empty());
    let p = h
        .preview(&key(), &r, HistoryMode::IncludeExisting, &|| true)
        .await
        .unwrap();
    assert_eq!(p.local_records, 1);
    assert_eq!(
        h.confirm(&key(), &r, p.id, "wrong", &|| true).await,
        Err(RemoteError::PermissionDenied)
    );
    assert!(r.cloud.lock().unwrap().accounts.is_empty());
    let during = job("completed while preview open");
    h.observe(&[old.clone(), during.clone()]).unwrap();
    h.confirm(&key(), &r, p.id, TRUST, &|| true).await.unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    assert_eq!(r.records().await.unwrap().len(), 1);
    h.turn_off(&key(), &r, &|| true).await.unwrap();
    join(&h, &r, HistoryMode::FutureOnly).await;
    let mut new = job("new but older wall clock");
    new.0.created_at_ms = 0;
    new.1 = 0;
    h.observe(&[old, during, new]).unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    assert_eq!(r.records().await.unwrap().len(), 2);
    assert!(h.status(&key()).unwrap().joined);
}
#[tokio::test]
async fn history_second_device_joins_the_enrollment_without_fencing_the_first() {
    let (a_dir, b_dir) = (Temp::new(), Temp::new());
    let (a, b) = (
        History::open(a_dir.0.clone()),
        History::open(b_dir.0.clone()),
    );
    let r = remote();
    let first = job("first device");
    a.observe(std::slice::from_ref(&first)).unwrap();
    join(&a, &r, HistoryMode::IncludeExisting).await;
    a.run(&key(), &r, &|| true).await.unwrap();
    let before = r.account().await.unwrap().unwrap().consent;
    // MW-OUATH-11 live: the second device chose the other mode, advanced the
    // generation and fenced the first device's history out of sync.
    b.observe(&[job("second device, before joining")]).unwrap();
    join(&b, &r, HistoryMode::FutureOnly).await;
    let after = r.account().await.unwrap().unwrap().consent;
    assert_eq!(after.transcripts, before.transcripts);
    assert_eq!(after.revision, before.revision);
    a.observe(&[first, job("first device, later")]).unwrap();
    a.run(&key(), &r, &|| true).await.unwrap();
    b.run(&key(), &r, &|| true).await.unwrap();
    assert!(a.status(&key()).unwrap().joined && b.status(&key()).unwrap().joined);
    // The second device's future-only boundary still excludes its old attempt.
    assert_eq!(r.records().await.unwrap().len(), 2);
    // Changing this device's own selection still starts a new enrollment.
    join(&a, &r, HistoryMode::FutureOnly).await;
    let changed = r.account().await.unwrap().unwrap().consent;
    assert!(changed.transcripts_generation() > after.transcripts_generation());
}
#[tokio::test]
async fn history_mapping_unicode_pagination_and_lost_ack_survive_restart() {
    let a = Temp::new();
    let b = Temp::new();
    let h = History::open(a.0.clone());
    let other = History::open(b.0.clone());
    let r = remote();
    let texts = [
        "English e\u{301} / é 👋",
        "עברית שלום",
        "\u{200f}שלום Writer\u{200e}\nmeeting at 10",
    ];
    let mut jobs: Vec<_> = texts.iter().map(|t| job(t)).collect();
    jobs[0]
        .0
        .request
        .options
        .vocabulary
        .corrections
        .push(Correction {
            from: "English".into(),
            to: "Corrected".into(),
        });
    if let JobStatus::Succeeded { result } = &mut jobs[0].0.status {
        result.cleanup = CleanupResult::Succeeded {
            cleaned_text: "Cleaned English".into(),
            selection: result.transcript.selection.clone(),
        };
    }
    jobs.push(job("fourth"));
    h.observe(&jobs).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    r.cloud.lock().unwrap().lose_next_reply = true;
    assert_eq!(h.run(&key(), &r, &|| true).await, Err(RemoteError::Offline));
    assert_eq!(r.cloud.lock().unwrap().commits, 1);
    let h = History::open(a.0.clone());
    h.run(&key(), &r, &|| true).await.unwrap();
    assert_eq!(r.cloud.lock().unwrap().commits, 6);
    let restored = restore_all(&other, &r).await;
    assert_eq!(restored.len(), 6);
    for text in texts {
        assert!(restored
            .iter()
            .any(|v| v["content"]["data"]["text"] == text));
    }
    let raw = restored
        .iter()
        .find(|v| v["content"]["data"]["text"] == texts[0])
        .unwrap();
    let corrected = restored
        .iter()
        .find(|v| v["content"]["data"]["kind"] == "corrected")
        .unwrap();
    let cleaned = restored
        .iter()
        .find(|v| v["content"]["data"]["kind"] == "cleaned")
        .unwrap();
    assert_eq!(raw["content"]["data"]["recorded_at_ms"], 5);
    assert_eq!(corrected["content"]["data"]["parent_record_id"], raw["id"]);
    assert_eq!(
        cleaned["content"]["data"]["parent_record_id"],
        corrected["id"]
    );
    assert!(!other.status(&key()).unwrap().joined);
    other.run(&key(), &r, &|| true).await.unwrap();
    assert_eq!(r.cloud.lock().unwrap().commits, 6);
    assert!(!serde_json::to_string(&restored)
        .unwrap()
        .contains("recording_id"));
}
#[tokio::test]
async fn history_concurrent_edits_and_original_deletion_fence_offline_children() {
    let a = Temp::new();
    let b = Temp::new();
    let h = History::open(a.0.clone());
    let other = History::open(b.0.clone());
    let r = remote();
    h.observe(&[job("original")]).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    let rows = restore_all(&other, &r).await;
    let id = Uuid::parse_str(rows[0]["id"].as_str().unwrap()).unwrap();
    join(&other, &r, HistoryMode::IncludeExisting).await; // Same consent: neither device gets fenced.
    let left = h.edit(&key(), id, "left שלום".into()).unwrap();
    let right = other.edit(&key(), id, "right עברית".into()).unwrap();
    assert_ne!(left, right);
    h.run(&key(), &r, &|| true).await.unwrap();
    other.run(&key(), &r, &|| true).await.unwrap();
    assert_eq!(restore_all(&other, &r).await.len(), 3);
    let pending = other.edit(&key(), id, "offline descendant".into()).unwrap();
    h.delete(&key(), id).unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    other.run(&key(), &r, &|| true).await.unwrap();
    assert!(!r
        .cloud
        .lock()
        .unwrap()
        .records
        .contains_key(&(key().uid, pending)));
    assert!(restore_all(&other, &r).await.is_empty());
    h.run(&key(), &r, &|| true).await.unwrap();
    assert!(restore_all(&h, &r).await.is_empty());
    // Source data remains; a confirmed new enrollment cannot resurrect tombstones.
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    assert!(restore_all(&h, &r).await.is_empty());
}
#[tokio::test]
async fn history_account_isolation_fences_and_keep_remove_do_not_delete_sources() {
    let a = Temp::new();
    let h = History::open(a.0.clone());
    let r = remote();
    h.observe(&[job("kept source")]).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    let b = AccountRef {
        uid: "synthetic-b".into(),
        ..key()
    };
    assert!(h.local_page(Some(&b), None).unwrap().records.is_empty());
    h.keep_copies(&key()).unwrap();
    assert_eq!(h.local_page(None, None).unwrap().records.len(), 1);
    h.suspend(&key());
    h.run(&key(), &r, &|| true).await.unwrap();
    h.detach(&key()).unwrap();
    assert!(!h.status(&key()).unwrap().joined);
    h.remove_copies(&key()).unwrap();
    assert!(h.local_page(Some(&key()), None).unwrap().records.is_empty());
    assert_eq!(h.local_page(None, None).unwrap().records.len(), 1);
    let p = h
        .preview(&key(), &r, HistoryMode::IncludeExisting, &|| true)
        .await
        .unwrap();
    assert_eq!(p.local_records, 1);
    h.confirm(&key(), &r, p.id, TRUST, &|| true).await.unwrap();
    h.fence(&key(), true).unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    assert!(!h.status(&key()).unwrap().joined);
    assert_eq!(
        h.restore(&key(), &r, None, &|| false).await.err(),
        Some(RemoteError::Superseded)
    );
}
#[test]
fn history_wire_rejects_context_and_oversize_without_altering_unicode() {
    let wire: Value = serde_json::from_str(include_str!(
        "../../../../docs/testing/account-sync/wire.json"
    ))
    .unwrap();
    for value in wire["records"].as_array().unwrap() {
        assert_eq!(Record::decode(value).unwrap().encode(), *value);
    }
    let original = wire["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["content"]["data"]["kind"] == "original")
        .unwrap();
    let mut v = original.clone();
    v["content"]["data"]["context"] = json!("never");
    assert!(Record::decode(&v).is_err());
    v = original.clone();
    v["content"]["data"]["text"] = json!("a".repeat(131073));
    assert!(Record::decode(&v).is_err());
    let temp = Temp::new();
    let h = History::open(temp.0.clone());
    h.observe(&[job(&"a".repeat(131073))]).unwrap();
    assert_eq!(h.status(&key()).unwrap().not_synced, 1);
}
#[tokio::test]
async fn history_shared_transcript_conflict_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../../docs/testing/account-sync/conflicts.json"
    ))
    .unwrap();
    let mut count = 0;
    for case in vectors.as_array().unwrap() {
        if !case["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["operation"]["category"] == "transcripts")
        {
            continue;
        }
        count += 1;
        let initial = &case["initial"];
        let r = MemoryRemote {
            auth_uid: initial["uid"].as_str().unwrap().into(),
            cloud: Arc::new(Mutex::new(Cloud::default())),
        };
        let mut a = r.create_account().await.unwrap();
        a.epoch = initial["account_epoch"].as_u64().unwrap();
        a.consent.binding.account_epoch = a.epoch;
        a.consent.transcripts = json!({"enabled":initial["enabled"],"generation":initial["generation"],"history":{"mode":"include_existing","enrollment_id":initial["history_enrollment_id"]}});
        {
            let mut cloud = r.cloud.lock().unwrap();
            cloud.accounts.insert(r.auth_uid.clone(), a);
            for v in initial["records"].as_array().unwrap() {
                let record = Record::decode(v).unwrap();
                cloud.records.insert(
                    (r.auth_uid.clone(), record.id),
                    super::plan::Stored {
                        record: record.clone(),
                        operation_id: Uuid::new_v4(),
                        consent_generation: 1,
                    },
                );
                if let Some(Data::Transcript(t)) = &record.content {
                    match t {
                        super::history_wire::Transcript::Original { attempt_id, .. } => {
                            let l = cloud.links.entry(r.auth_uid.clone()).or_default();
                            l.insert(
                                ("attempts".into(), *attempt_id),
                                json!({"original_record_id":record.id}),
                            );
                            l.insert(
                                ("families".into(), record.id),
                                json!({"member_ids":[],"deleted":false}),
                            );
                        }
                        super::history_wire::Transcript::Derived {
                            original_record_id, ..
                        } => {
                            cloud
                                .links
                                .get_mut(&r.auth_uid)
                                .unwrap()
                                .get_mut(&("families".into(), *original_record_id))
                                .unwrap()["member_ids"]
                                .as_array_mut()
                                .unwrap()
                                .push(json!(record.id));
                        }
                    }
                }
            }
        }
        for step in case["steps"].as_array().unwrap() {
            let op = Operation::decode(&step["operation"]).unwrap();
            let decision = if initial["history_enrolled"] == false {
                super::wire::Decision::Rejected(super::wire::Rejection::HistoryNotEnrolled)
            } else {
                r.commit(&op).await.unwrap()
            };
            let status = match decision {
                super::wire::Decision::Acknowledged(_) => "acknowledged".into(),
                super::wire::Decision::Conflict(_) => "conflict".into(),
                super::wire::Decision::Rejected(x) => serde_json::to_value(x)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            };
            assert_eq!(status, step["expected"]["status"], "{}", case["name"]);
        }
    }
    assert_eq!(count, 6);
}

#[tokio::test]
#[ignore = "needs the Firestore emulator"]
async fn history_emulator_rest_rules_paging_versions_and_erasure() {
    use super::firestore::{client, Firestore};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let host = std::env::var("FIRESTORE_EMULATOR_HOST").expect("run firestore-rules.mjs --rust");
    let uid = key().uid;
    let project = "demo-metrix-writer";
    let now = super::local::now_ms() / 1000;
    let token=format!("{}.{}.",URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#),URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"iss":format!("https://securetoken.google.com/{project}"),"aud":project,"auth_time":now,"iat":now,"exp":now+3600,"sub":uid,"user_id":uid,"firebase":{"sign_in_provider":"google.com","identities":{}}})).unwrap()));
    let r = Firestore::new(
        &format!("http://{host}/v1"),
        project,
        &uid,
        token.clone(),
        client(),
    )
    .unwrap();
    let a = Temp::new();
    let b = Temp::new();
    let h = History::open(a.0.clone());
    let other = History::open(b.0.clone());
    assert!(r.account().await.unwrap().is_none());
    let mut jobs = vec![
        job("שלום English 👋"),
        job("two"),
        job("three"),
        job("four"),
        job("five"),
        job("six"),
    ];
    jobs[0]
        .0
        .request
        .options
        .vocabulary
        .corrections
        .push(Correction {
            from: "English".into(),
            to: "עברית".into(),
        });
    h.observe(&jobs).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    assert!(
        r.records().await.unwrap().is_empty(),
        "portable query must exclude all transcript content at the server"
    );
    let rows = restore_all(&other, &r).await;
    assert_eq!(rows.len(), 7);
    let original = rows
        .iter()
        .find(|v| v["content"]["data"]["text"] == "שלום English 👋")
        .unwrap();
    let id = Uuid::parse_str(original["id"].as_str().unwrap()).unwrap();
    join(&other, &r, HistoryMode::IncludeExisting).await;
    h.edit(&key(), id, "edit A".into()).unwrap();
    other.edit(&key(), id, "edit B".into()).unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    other.run(&key(), &r, &|| true).await.unwrap();
    let all = restore_all(&other, &r).await;
    assert_eq!(all.len(), 9);
    let children: Vec<_> = all
        .iter()
        .filter(|v| v["content"]["data"]["original_record_id"] == id.to_string())
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect();
    h.delete(&key(), id).unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    assert_eq!(restore_all(&other, &r).await.len(), 5);
    for child in children {
        let doc:Value=client().get(format!("http://{host}/v1/projects/{project}/databases/(default)/documents/users/{uid}/records/{child}")).bearer_auth(&token).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
        assert_eq!(
            super::firestore::decode_value(&doc["fields"]["content"]),
            json!({"state":"deleted"}),
            "child text is physically removed, not just hidden by a family fence"
        );
    }
    h.turn_off(&key(), &r, &|| true).await.unwrap();
    let before = other.status(&key()).unwrap().pending;
    other.run(&key(), &r, &|| true).await.unwrap();
    assert!(!other.status(&key()).unwrap().joined);
    assert_eq!(before, 0);
}

struct InterruptedPage {
    inner: MemoryRemote,
    history: Arc<History>,
    change_consent: bool,
}
#[async_trait::async_trait]
impl Remote for InterruptedPage {
    async fn account(&self) -> Result<Option<super::wire::Account>, RemoteError> {
        self.inner.account().await
    }
    async fn create_account(&self) -> Result<super::wire::Account, RemoteError> {
        self.inner.create_account().await
    }
    async fn set_consent(
        &self,
        c: &super::wire::Consent,
    ) -> Result<Option<super::wire::Account>, RemoteError> {
        self.inner.set_consent(c).await
    }
    async fn records(&self) -> Result<Vec<Record>, RemoteError> {
        self.inner.records().await
    }
    async fn commit(&self, o: &Operation) -> Result<super::wire::Decision, RemoteError> {
        self.inner.commit(o).await
    }
    async fn history_page(
        &self,
        after: Option<Uuid>,
    ) -> Result<super::sync::HistoryPage, RemoteError> {
        let page = self.inner.history_page(after).await?;
        if self.change_consent {
            let mut c = self.inner.cloud.lock().unwrap();
            let a = c.accounts.get_mut(&key().uid).unwrap();
            a.consent = a.consent.next(super::wire::Category::Transcripts, false);
        } else {
            self.history.suspend(&key());
        }
        Ok(page)
    }
}
#[tokio::test]
async fn history_late_page_after_account_or_consent_change_never_applies() {
    let a = Temp::new();
    let b = Temp::new();
    let h = History::open(a.0.clone());
    let other = History::open(b.0.clone());
    let r = remote();
    h.observe(&[job("private account A")]).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    for change_consent in [false, true] {
        let delayed = InterruptedPage {
            inner: r.clone(),
            history: other.clone(),
            change_consent,
        };
        assert!(other
            .restore(&key(), &delayed, None, &|| true)
            .await
            .is_err());
        assert!(other
            .local_page(Some(&key()), None)
            .unwrap()
            .records
            .is_empty());
    }
}
#[tokio::test]
async fn history_everywhere_reset_on_reconnect_erases_copies_and_fences_outbox() {
    let a = Temp::new();
    let h = History::open(a.0.clone());
    let r = remote();
    h.observe(&[job("original remains in source store")])
        .unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    {
        let mut cloud = r.cloud.lock().unwrap();
        let a = cloud.accounts.get_mut(&key().uid).unwrap();
        a.consent = a.consent.next(super::wire::Category::Transcripts, false);
        a.transcript_reset = Some(a.consent.transcripts_generation());
    }
    h.run(&key(), &r, &|| true).await.unwrap();
    assert!(!h.status(&key()).unwrap().joined);
    assert!(h.local_page(Some(&key()), None).unwrap().records.is_empty());
    assert_eq!(
        h.preview(&key(), &r, HistoryMode::IncludeExisting, &|| true)
            .await
            .unwrap()
            .local_records,
        1
    );
}

#[tokio::test]
async fn history_bulk_local_delete_clears_caches_and_rejects_old_source_snapshots() {
    let a = Temp::new();
    let h = History::open(a.0.clone());
    let r = remote();
    let jobs = vec![job("erase local copies")];
    h.observe(&jobs).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    h.keep_copies(&key()).unwrap();
    let stale = h.source_epoch();
    h.clear_local_history().await.unwrap();
    assert_eq!(h.observe_at(stale, &jobs), Err(RemoteError::Superseded));
    assert!(h.local_page(None, None).unwrap().records.is_empty());
    assert!(h.local_page(Some(&key()), None).unwrap().records.is_empty());
    assert!(!h.status(&key()).unwrap().joined);
    assert_eq!(
        h.preview(&key(), &r, HistoryMode::IncludeExisting, &|| true)
            .await
            .unwrap()
            .local_records,
        0
    );
    assert_eq!(
        r.cloud.lock().unwrap().records.len(),
        1,
        "local deletion sends no cloud tombstone"
    );
}

#[tokio::test]
async fn history_date_delete_keeps_midnight_and_newer_and_removes_later_edits_of_old_audio() {
    let a = Temp::new();
    let h = History::open(a.0.clone());
    let r = remote();
    let mut old = job("yesterday");
    old.1 = 99;
    let mut midnight = job("today at midnight");
    midnight.1 = 100;
    let mut newer = job("today later");
    newer.1 = 101;
    let jobs = vec![old, midnight, newer];
    h.observe(&jobs).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    let rows = restore_all(&h, &r).await;
    let old_id = Uuid::parse_str(
        rows.iter()
            .find(|v| v["content"]["data"]["text"] == "yesterday")
            .unwrap()["id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    h.edit(&key(), old_id, "edited much later".into()).unwrap();
    h.keep_copies(&key()).unwrap();
    let stale = h.source_epoch();
    let cloud_before = r.records().await.unwrap();
    h.clear_local_before(100).await.unwrap();
    assert_eq!(h.observe_at(stale, &jobs), Err(RemoteError::Superseded));
    for account in [None, Some(&key())] {
        let rows = h.local_page(account, None).unwrap().records;
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|v| v["content"]["data"]["text"] != "yesterday"
                && v["content"]["data"]["text"] != "edited much later"));
    }
    assert!(!h.status(&key()).unwrap().joined);
    assert_eq!(h.status(&key()).unwrap().pending, 0);
    assert_eq!(r.records().await.unwrap(), cloud_before);
}

#[tokio::test]
async fn history_old_create_receipt_cannot_revive_a_pulled_tombstone() {
    let a = Temp::new();
    let b = Temp::new();
    let h = History::open(a.0.clone());
    let other = History::open(b.0.clone());
    let r = remote();
    h.observe(&[job("lost acknowledgement")]).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    r.cloud.lock().unwrap().lose_next_reply = true;
    assert!(h.run(&key(), &r, &|| true).await.is_err());
    let rows = restore_all(&other, &r).await;
    let id = Uuid::parse_str(rows[0]["id"].as_str().unwrap()).unwrap();
    join(&other, &r, HistoryMode::IncludeExisting).await;
    other.delete(&key(), id).unwrap();
    other.run(&key(), &r, &|| true).await.unwrap();
    assert!(restore_all(&h, &r).await.is_empty());
    h.run(&key(), &r, &|| true).await.unwrap();
    assert!(h.local_page(Some(&key()), None).unwrap().records.is_empty());
    assert_eq!(h.status(&key()).unwrap().pending, 0);
}

#[test]
fn history_fixed_1k_10k_catalog_merge_profile() {
    use std::collections::BTreeMap;
    for n in [1000, 10000] {
        let temp = Temp::new();
        let h = History::open(temp.0.clone());
        let jobs: Vec<_> = (0..n)
            .map(|i| {
                let mut j = job(&format!("synthetic {i} עברית"));
                j.0.id = Uuid::from_u128(i as u128 + 1);
                j
            })
            .collect();
        // The former observe algorithm scanned every existing record per job.
        let mut baseline: BTreeMap<Uuid, Value> = BTreeMap::new();
        let mut comparisons = 0u64;
        let before = std::time::Instant::now();
        for (j, _) in &jobs {
            let existing = baseline.iter().find_map(|(id, v)| {
                comparisons += 1;
                (v["content"]["data"]["attempt_id"] == j.id.to_string()).then_some(*id)
            });
            if existing.is_none() {
                baseline.insert(j.id, json!({"content":{"data":{"attempt_id":j.id}}}));
            }
        }
        let baseline_us = before.elapsed().as_micros();
        let after = std::time::Instant::now();
        h.observe(&jobs).unwrap();
        let current_us = after.elapsed().as_micros();
        assert_eq!(comparisons, (n as u64) * (n as u64 - 1) / 2);
        // A repeat preserves assigned identities and doesn't rewrite record text.
        h.observe(&jobs).unwrap();
        eprintln!("MW112 merge records={n} before_comparisons={comparisons} after_index_probes={n} before_us={baseline_us} after_including_commit_us={current_us}");
    }
}

#[tokio::test]
async fn incremental_source_projection_preserves_unmentioned_originals_and_versions() {
    let temp = Temp::new();
    let h = History::open(temp.0.clone());
    let r = remote();
    let initial = vec![job("first"), job("second")];
    h.observe(&initial).unwrap();
    join(&h, &r, HistoryMode::IncludeExisting).await;
    h.run(&key(), &r, &|| true).await.unwrap();
    let before = h.local_page(Some(&key()), None).unwrap().records;
    h.observe_changes(h.source_epoch(), &[job("third")])
        .unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    let after = h.local_page(Some(&key()), None).unwrap().records;
    assert_eq!(after.len(), 3);
    for record in before {
        assert!(after.contains(&record));
    }
    h.observe_changes(h.source_epoch(), &initial[..1]).unwrap();
    h.run(&key(), &r, &|| true).await.unwrap();
    assert_eq!(h.local_page(Some(&key()), None).unwrap().records.len(), 3);
}
