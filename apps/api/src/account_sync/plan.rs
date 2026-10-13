//! The MW-OUATH-5 commit planner (`account-sync.firestore.mjs` `planOperation`)
//! for settings and dictionary records. Both the in-memory test remote and the
//! Firestore transport decide through this one function, and the rules enforce
//! the same writes independently.
use super::wire::{Account, Category, Decision, Operation, Record, Rejection, SettingKey};
use super::{history_wire::Transcript, wire::Data};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use uuid::Uuid;
pub type Links = BTreeMap<(String, Uuid), Value>;

/// A stored record document: the record plus the operation that wrote it and
/// the generation it was written in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub record: Record,
    pub operation_id: Uuid,
    pub consent_generation: u64,
}

/// An idempotency receipt: the exact accepted envelope and its revision.
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    pub operation: serde_json::Value,
    pub revision: u64,
}

pub enum Plan {
    /// Nothing is written.
    Reply(Decision),
    /// Commit the record and its receipt together, then reply.
    Write {
        stored: Box<Stored>,
        receipt: Receipt,
        decision: Decision,
        links: Links,
    },
}

/// Decides one validated operation against the documents a transaction read:
/// the account, the receipt for `op.operation_id` and the target record.
pub fn plan(
    auth_uid: &str,
    op: &Operation,
    account: Option<&Account>,
    receipt: Option<&Receipt>,
    current: Option<&Stored>,
) -> Plan {
    plan_with_links(auth_uid, op, account, receipt, current, &Links::new())
}

pub fn plan_with_links(
    auth_uid: &str,
    op: &Operation,
    account: Option<&Account>,
    receipt: Option<&Receipt>,
    current: Option<&Stored>,
    links: &Links,
) -> Plan {
    let reject = |reason| Plan::Reply(Decision::Rejected(reason));
    if auth_uid.is_empty() || auth_uid != op.binding.uid {
        return reject(Rejection::WrongAccount);
    }
    let Some(account) = account else {
        return reject(Rejection::ConsentRequired);
    };
    if !account.active {
        return reject(Rejection::AccountUnavailable);
    }
    if account.epoch != op.binding.account_epoch {
        return reject(Rejection::StaleEpoch);
    }
    let consent = account.consent.toggle(op.category);
    if consent.generation != op.consent_generation {
        return reject(Rejection::StaleGeneration);
    }
    if !consent.enabled {
        return reject(Rejection::ConsentRequired);
    }
    if op.category == Category::Transcripts
        && op.history_enrollment_id.map(|id| id.to_string()).as_deref()
            != account.consent.transcripts["history"]["enrollment_id"].as_str()
    {
        return reject(Rejection::HistoryNotEnrolled);
    }
    if let Some(receipt) = receipt {
        // Semantic comparison: decoding ignores key order and escaping.
        return match Operation::decode(&receipt.operation) {
            Ok(accepted) if accepted == *op => {
                Plan::Reply(Decision::Acknowledged(op.record(receipt.revision)))
            }
            _ => reject(Rejection::IdempotencyConflict),
        };
    }
    let fixed = SettingKey::from_id(&op.record_id).is_some();
    if (op.category == Category::Settings) != fixed {
        return reject(Rejection::InvalidRecord);
    }
    if let Some(current) = current {
        let record = &current.record;
        if record.category != op.category || record.origin != op.origin {
            return reject(Rejection::InvalidRecord);
        }
        // A fixed setting may be recreated only in a newer confirmed generation.
        if record.content.is_none()
            && !(op.category == Category::Settings
                && current.consent_generation < op.consent_generation)
        {
            return reject(Rejection::Deleted);
        }
        if let (Some(before), Some(after)) = (&record.content, &op.content) {
            if op.category == Category::Dictionary
                && std::mem::discriminant(before) == std::mem::discriminant(after)
                && kind(before) != kind(after)
            {
                return reject(Rejection::InvalidRecord);
            }
        }
    }
    if let Some(data) = &op.content {
        if data.check_id(&op.record_id).is_err() {
            return reject(Rejection::InvalidRecord);
        }
    }
    let base = current.map_or(0, |c| c.record.revision);
    if current.is_none() && op.base_revision != 0 {
        return reject(Rejection::InvalidBase);
    }
    if let Some(current) = current {
        if current.record.revision != op.base_revision {
            return Plan::Reply(Decision::Conflict(current.record.clone()));
        }
    }
    let mut writes = Links::new();
    if op.category == Category::Transcripts {
        if current.is_some() && op.content.is_some() {
            return reject(Rejection::Immutable);
        }
        match &op.content {
            Some(Data::Transcript(Transcript::Original { attempt_id, .. })) => {
                if links.contains_key(&("attempts".into(), *attempt_id)) {
                    return reject(Rejection::InvalidRecord);
                }
                writes.insert(
                    ("attempts".into(), *attempt_id),
                    json!({"original_record_id":op.record_id}),
                );
                writes.insert(
                    ("families".into(), op.record_id),
                    json!({"member_ids":[],"deleted":false}),
                );
            }
            Some(Data::Transcript(Transcript::Derived {
                original_record_id,
                parent_record_id,
                ..
            })) => {
                let Some(family) = links.get(&("families".into(), *original_record_id)) else {
                    return reject(Rejection::InvalidRecord);
                };
                if family["deleted"] == true {
                    return reject(Rejection::Deleted);
                }
                let Some(members) = family["member_ids"].as_array() else {
                    return reject(Rejection::InvalidRecord);
                };
                let parent = links.get(&("records".into(), *parent_record_id));
                if parent.is_none_or(|p| {
                    p["category"] != "transcripts" || p["content"]["state"] != "live"
                }) || (*parent_record_id != *original_record_id
                    && !members.contains(&json!(parent_record_id)))
                {
                    return reject(Rejection::InvalidRecord);
                }
                if members.len() >= 200 {
                    return reject(Rejection::QuotaExceeded);
                }
                let mut members = members.clone();
                members.push(json!(op.record_id));
                writes.insert(
                    ("families".into(), *original_record_id),
                    json!({"member_ids":members,"deleted":false}),
                );
            }
            None => {
                if let Some(family) = links.get(&("families".into(), op.record_id)) {
                    writes.insert(
                        ("families".into(), op.record_id),
                        json!({"member_ids":family["member_ids"],"deleted":true}),
                    );
                }
            }
            _ => return reject(Rejection::InvalidRecord),
        }
    }
    let revision = base + 1;
    let record = op.record(revision);
    Plan::Write {
        stored: Box::new(Stored {
            record: record.clone(),
            operation_id: op.operation_id,
            consent_generation: op.consent_generation,
        }),
        receipt: Receipt {
            operation: op.encode(),
            revision,
        },
        decision: Decision::Acknowledged(record),
        links: writes,
    }
}

fn kind(data: &super::wire::Data) -> &'static str {
    match data {
        super::wire::Data::Entry(e) => e.kind(),
        super::wire::Data::Setting(_) => "setting",
        super::wire::Data::Transcript(_) => "transcript",
    }
}
