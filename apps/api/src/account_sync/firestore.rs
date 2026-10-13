//! Firestore REST transport for one signed-in account (MW-OUATH-5 protocol).
//!
//! Every request carries the user's Firebase ID token, so the deployed rules
//! authorize it; no service credential exists in the app. Each operation runs
//! as one read-write transaction: read the account, the receipt and the target
//! record, decide with [`plan_with_links`], then commit the record and its receipt
//! together. Tokens and response bodies are never logged or surfaced.
use super::plan::{plan_with_links, Links, Plan, Receipt, Stored};
use super::sync::{HistoryPage, Remote, RemoteError, HISTORY_PAGE};
use super::wire::{Account, Consent, Decision, Operation, Record};
use super::{history_wire::Transcript, wire::Data};
use reqwest::{Method, StatusCode};
use serde_json::{json, Map, Value};
use std::time::Duration;
use uuid::Uuid;

const PAGE: usize = 500;
const REPLY_LIMIT: usize = 4 * 1024 * 1024;

pub struct Firestore {
    base: String,
    project: String,
    uid: String,
    token: String,
    http: reqwest::Client,
    current: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// Firebase UIDs are opaque; refuse anything that could change a path's shape.
pub fn checked_uid(uid: &str) -> Option<&str> {
    let reserved = uid.starts_with("__") && uid.ends_with("__");
    (!uid.is_empty()
        && uid.len() <= 128
        && !uid.contains('/')
        && uid != "."
        && uid != ".."
        && !reserved)
        .then_some(uid)
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
}

impl Firestore {
    /// `base` is the Firestore REST root (`…/v1`); the desktop host supplies the
    /// production URL only in builds with sign-in, tests an emulator's.
    pub fn new(
        base: &str,
        project: &str,
        uid: &str,
        token: String,
        http: reqwest::Client,
    ) -> Option<Self> {
        let project_ok = !project.is_empty()
            && project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        (project_ok && checked_uid(uid).is_some()).then(|| Self {
            base: base.trim_end_matches('/').to_owned(),
            project: project.to_owned(),
            uid: uid.to_owned(),
            token,
            http,
            current: None,
        })
    }
    pub fn guarded(mut self, current: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.current = Some(std::sync::Arc::new(current));
        self
    }
    fn root(&self) -> String {
        format!("projects/{}/databases/(default)/documents", self.project)
    }
    fn name(&self, collection: Option<&str>, id: Option<Uuid>) -> String {
        let mut name = format!("{}/users/{}", self.root(), self.uid);
        if let (Some(collection), Some(id)) = (collection, id) {
            name.push_str(&format!("/{collection}/{id}"));
        }
        name
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Failure> {
        if self.current.as_ref().is_some_and(|f| !f()) {
            return Err(Failure::Remote(RemoteError::Superseded));
        }
        let url = format!("{}/{}", self.base, path);
        let mut request = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| Failure::Remote(RemoteError::Offline))?;
        if self.current.as_ref().is_some_and(|f| !f()) {
            return Err(Failure::Remote(RemoteError::Superseded));
        }
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Failure::Remote(RemoteError::Offline))?
        {
            if bytes.len().saturating_add(chunk.len()) > REPLY_LIMIT {
                return Err(Failure::Remote(RemoteError::InvalidResponse));
            }
            bytes.extend_from_slice(&chunk);
        }
        if self.current.as_ref().is_some_and(|f| !f()) {
            return Err(Failure::Remote(RemoteError::Superseded));
        }
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value);
        }
        let code = value["error"]["status"].as_str().unwrap_or_default();
        Err(match (status, code) {
            (StatusCode::NOT_FOUND, _) => Failure::NotFound,
            (_, "ABORTED") | (StatusCode::CONFLICT, _) => Failure::Aborted,
            (_, "FAILED_PRECONDITION") | (_, "ALREADY_EXISTS") => Failure::Precondition,
            (StatusCode::UNAUTHORIZED, _) => Failure::Remote(RemoteError::ReauthRequired),
            (StatusCode::FORBIDDEN, _) => Failure::Remote(RemoteError::PermissionDenied),
            (StatusCode::TOO_MANY_REQUESTS, _) | (_, "RESOURCE_EXHAUSTED") => {
                Failure::Remote(RemoteError::Busy)
            }
            (s, _) if s.is_server_error() => Failure::Remote(RemoteError::Offline),
            _ => Failure::Remote(RemoteError::InvalidResponse),
        })
    }

    async fn read_account(&self) -> Result<Option<(Account, String)>, RemoteError> {
        match self.call(Method::GET, &self.name(None, None), None).await {
            Ok(document) => {
                let update = document["updateTime"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                Ok(Some((decode_account(&fields(&document))?, update)))
            }
            Err(Failure::NotFound) => Ok(None),
            Err(other) => Err(other.remote()),
        }
    }

    /// A family fence hides every version immediately; erase descendant text
    /// in bounded, idempotent writes. A failed sweep keeps the original delete
    /// in the durable outbox, so retrying its receipt resumes this cleanup.
    async fn erase_family(&self, id: Uuid) -> Result<(), Failure> {
        let family = match self
            .call(Method::GET, &self.name(Some("families"), Some(id)), None)
            .await
        {
            Ok(doc) => fields(&doc),
            Err(Failure::NotFound) => return Ok(()),
            Err(e) => return Err(e),
        };
        if family.get("deleted") != Some(&Value::Bool(true)) {
            return Ok(());
        }
        let members = family
            .get("member_ids")
            .and_then(Value::as_array)
            .ok_or(Failure::Remote(RemoteError::InvalidResponse))?;
        if members.len() > 200 {
            return Err(Failure::Remote(RemoteError::InvalidResponse));
        }
        for member in members {
            let id = member
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or(Failure::Remote(RemoteError::InvalidResponse))?;
            let name = self.name(Some("records"), Some(id));
            let doc = match self.call(Method::GET, &name, None).await {
                Ok(v) => v,
                Err(Failure::NotFound) => continue,
                Err(e) => return Err(e),
            };
            let mut stored = decode_stored(&fields(&doc))?;
            if stored.record.content.is_none() {
                continue;
            }
            stored.record.content = None;
            stored.record.revision += 1;
            self.call(Method::POST,&format!("{}:commit",self.root()),Some(json!({"writes":[{"update":{"name":name,"fields":encode_fields(&stored_fields(&stored))},"currentDocument":{"updateTime":doc["updateTime"]}}]}))).await?;
        }
        Ok(())
    }

    /// One optimistic commit attempt. Production Firestore refuses read-write
    /// `beginTransaction` for rules-authenticated (end-user) tokens with 403;
    /// only the emulator allows it. So, like the Firebase client SDKs, read
    /// without a transaction and commit every write with a precondition on the
    /// version read, plus a `verify` for each document read but not written.
    /// Any concurrent change fails the commit with `FAILED_PRECONDITION`, which
    /// `commit` retries from fresh reads. The rules still gate consent, epoch
    /// and ownership against the committed state.
    async fn transact(&self, op: &Operation) -> Result<Decision, Failure> {
        let names = [
            self.name(None, None),
            self.name(Some("receipts"), Some(op.operation_id)),
            self.name(Some("records"), Some(op.record_id)),
        ];
        // Document name -> the updateTime read, or None when it was missing.
        let mut seen: std::collections::BTreeMap<String, Option<String>> = Default::default();
        let mut found: std::collections::BTreeMap<String, Map<String, Value>> = Default::default();
        let wanted = match &op.content {
            Some(Data::Transcript(Transcript::Original { attempt_id, .. })) => {
                vec![("attempts", *attempt_id)]
            }
            Some(Data::Transcript(Transcript::Derived {
                original_record_id,
                parent_record_id,
                ..
            })) => vec![
                ("families", *original_record_id),
                ("records", *parent_record_id),
            ],
            None if op.category == super::wire::Category::Transcripts => {
                vec![("families", op.record_id)]
            }
            _ => vec![],
        };
        let mut documents: Vec<String> = names.to_vec();
        documents.extend(
            wanted
                .iter()
                .map(|(collection, id)| self.name(Some(collection), Some(*id))),
        );
        let read = self
            .call(
                Method::POST,
                &format!("{}:batchGet", self.root()),
                Some(json!({ "documents": documents })),
            )
            .await?;
        for item in read.as_array().into_iter().flatten() {
            if let Some(document) = item.get("found") {
                let name = document["name"]
                    .as_str()
                    .ok_or(Failure::Remote(RemoteError::InvalidResponse))?;
                let update = document["updateTime"]
                    .as_str()
                    .ok_or(Failure::Remote(RemoteError::InvalidResponse))?;
                seen.insert(name.to_owned(), Some(update.to_owned()));
                found.insert(name.to_owned(), fields(document));
            } else if let Some(name) = item["missing"].as_str() {
                seen.insert(name.to_owned(), None);
            }
        }
        if documents.iter().any(|name| !seen.contains_key(name)) {
            return Err(Failure::Remote(RemoteError::InvalidResponse));
        }
        let account = found
            .get(&names[0])
            .map(decode_account)
            .transpose()
            .map_err(Failure::Remote)?;
        let receipt = found.get(&names[1]).map(decode_receipt).transpose()?;
        let current = found.get(&names[2]).map(decode_stored).transpose()?;
        let mut links = Links::new();
        for (collection, id) in wanted {
            if let Some(doc) = found.get(&self.name(Some(collection), Some(id))) {
                links.insert((collection.into(), id), Value::Object(doc.clone()));
            }
        }
        let decided = plan_with_links(
            &self.uid,
            op,
            account.as_ref(),
            receipt.as_ref(),
            current.as_ref(),
            &links,
        );
        match decided {
            Plan::Reply(decision) => Ok(decision),
            Plan::Write {
                stored,
                receipt,
                decision,
                links,
            } => {
                let record = stored_fields(&stored);
                let receipt =
                    json!({ "operation": receipt.operation, "revision": receipt.revision });
                let mut updates = vec![
                    (names[2].clone(), encode_fields(&record)),
                    (names[1].clone(), encode_fields(&receipt)),
                ];
                for ((collection, id), value) in links {
                    updates.push((
                        self.name(Some(&collection), Some(id)),
                        encode_fields(&value),
                    ));
                }
                let precondition = |name: &str| match seen.get(name) {
                    Some(Some(update)) => Some(json!({ "updateTime": update })),
                    Some(None) => Some(json!({ "exists": false })),
                    None => None,
                };
                let mut writes = Vec::new();
                for (name, fields) in &updates {
                    let mut write = json!({ "update": { "name": name, "fields": fields } });
                    if let Some(p) = precondition(name) {
                        write["currentDocument"] = p;
                    }
                    writes.push(write);
                }
                for name in seen.keys() {
                    if updates.iter().all(|(written, _)| written != name) {
                        writes
                            .push(json!({ "verify": name, "currentDocument": precondition(name) }));
                    }
                }
                self.call(
                    Method::POST,
                    &format!("{}:commit", self.root()),
                    Some(json!({ "writes": writes })),
                )
                .await?;
                Ok(decision)
            }
        }
    }
}

enum Failure {
    NotFound,
    Aborted,
    Precondition,
    Remote(RemoteError),
}
impl Failure {
    fn remote(self) -> RemoteError {
        match self {
            Self::Remote(e) => e,
            Self::NotFound | Self::Precondition => RemoteError::PermissionDenied,
            Self::Aborted => RemoteError::Busy,
        }
    }
}

#[async_trait::async_trait]
impl Remote for Firestore {
    async fn account(&self) -> Result<Option<Account>, RemoteError> {
        Ok(self.read_account().await?.map(|(a, _)| a))
    }
    async fn create_account(&self) -> Result<Account, RemoteError> {
        let initial = Consent::initial(&self.uid);
        let document = json!({ "schema_version": 1, "account_epoch": 1, "account_state": "active",
            "consent": initial.encode() });
        let created = self
            .call(
                Method::POST,
                &format!("{}:commit", self.root()),
                Some(json!({ "writes": [{
                    "update": { "name": self.name(None, None), "fields": encode_fields(&document) },
                    "currentDocument": { "exists": false },
                }] })),
            )
            .await;
        match created {
            Ok(_) | Err(Failure::Precondition) => {
                self.account().await?.ok_or(RemoteError::InvalidResponse)
            }
            Err(other) => Err(other.remote()),
        }
    }
    async fn set_consent(&self, next: &Consent) -> Result<Option<Account>, RemoteError> {
        let Some((account, update_time)) = self.read_account().await? else {
            return Err(RemoteError::PermissionDenied);
        };
        if account.consent.revision + 1 != next.revision || update_time.is_empty() {
            return Ok(None);
        }
        let consent = json!({ "consent": next.encode() });
        let written = self
            .call(
                Method::POST,
                &format!("{}:commit", self.root()),
                Some(json!({ "writes": [{
                    "update": { "name": self.name(None, None), "fields": encode_fields(&consent) },
                    "updateMask": { "fieldPaths": ["consent"] },
                    "currentDocument": { "updateTime": update_time },
                }] })),
            )
            .await;
        match written {
            Ok(_) => self.account().await,
            Err(Failure::Precondition) => Ok(None),
            Err(other) => Err(other.remote()),
        }
    }
    async fn records(&self) -> Result<Vec<Record>, RemoteError> {
        let parent = self.name(None, None);
        let mut out = vec![];
        let mut after: Option<String> = None;
        loop {
            let mut query = json!({
                "from": [{ "collectionId": "records" }],
                "orderBy": [{ "field": { "fieldPath": "__name__" }, "direction": "ASCENDING" }],
                "limit": PAGE,
                "where": {"fieldFilter":{"field":{"fieldPath":"category"},"op":"IN","value":{"arrayValue":{"values":[{"stringValue":"settings"},{"stringValue":"dictionary"}]}}}},
            });
            if let Some(last) = &after {
                query["startAt"] =
                    json!({ "values": [{ "referenceValue": last }], "before": false });
            }
            let rows = self
                .call(
                    Method::POST,
                    &format!("{parent}:runQuery"),
                    Some(json!({ "structuredQuery": query })),
                )
                .await
                .map_err(Failure::remote)?;
            let mut count = 0;
            for row in rows.as_array().into_iter().flatten() {
                let Some(document) = row.get("document") else {
                    continue;
                };
                count += 1;
                after = document["name"].as_str().map(str::to_owned);
                let fields = fields(document);
                // Transcript records belong to MW-OUATH-9; leave them untouched.
                if fields.get("category").and_then(Value::as_str) == Some("transcripts") {
                    continue;
                }
                out.push(decode_stored(&fields).map_err(Failure::remote)?.record);
            }
            if count < PAGE {
                return Ok(out);
            }
        }
    }
    async fn history_page(&self, after: Option<Uuid>) -> Result<HistoryPage, RemoteError> {
        let mut query = json!({"from":[{"collectionId":"records"}],"where":{"fieldFilter":{"field":{"fieldPath":"category"},"op":"EQUAL","value":{"stringValue":"transcripts"}}},"orderBy":[{"field":{"fieldPath":"__name__"},"direction":"ASCENDING"}],"limit":HISTORY_PAGE});
        if let Some(id) = after {
            query["startAt"] = json!({"values":[{"referenceValue":self.name(Some("records"),Some(id))}],"before":false});
        }
        let rows = self
            .call(
                Method::POST,
                &format!("{}:runQuery", self.name(None, None)),
                Some(json!({"structuredQuery":query})),
            )
            .await
            .map_err(Failure::remote)?;
        let mut records = vec![];
        for row in rows.as_array().ok_or(RemoteError::InvalidResponse)? {
            if let Some(doc) = row.get("document") {
                let mut record = decode_stored(&fields(doc)).map_err(Failure::remote)?.record;
                // The original deletion fences descendants atomically. Never expose
                // a live child even if physical tombstoning is still pending.
                if let Some(Data::Transcript(Transcript::Derived {
                    original_record_id, ..
                })) = &record.content
                {
                    let family = self
                        .call(
                            Method::GET,
                            &self.name(Some("families"), Some(*original_record_id)),
                            None,
                        )
                        .await
                        .map_err(Failure::remote)?;
                    if fields(&family).get("deleted") == Some(&Value::Bool(true)) {
                        record.content = None;
                    }
                }
                records.push(record);
            }
        }
        let next = (records.len() == HISTORY_PAGE).then(|| records.last().unwrap().id);
        Ok(HistoryPage { records, next })
    }
    async fn commit(&self, op: &Operation) -> Result<Decision, RemoteError> {
        if op.binding.uid != self.uid {
            return Ok(Decision::Rejected(super::wire::Rejection::WrongAccount));
        }
        for _ in 0..4 {
            match self.transact(op).await {
                // Contention, or a concurrent identical retry that created the
                // receipt first: the next attempt reads it and replies from it.
                Err(Failure::Aborted | Failure::Precondition) => continue,
                Ok(decision) => {
                    if op.category == super::wire::Category::Transcripts
                        && op.content.is_none()
                        && matches!(decision, Decision::Acknowledged(_))
                    {
                        self.erase_family(op.record_id)
                            .await
                            .map_err(Failure::remote)?;
                    }
                    return Ok(decision);
                }
                Err(e) => return Err(e.remote()),
            }
        }
        Err(RemoteError::Busy)
    }
}

fn decode_account(fields: &Map<String, Value>) -> Result<Account, RemoteError> {
    let bad = RemoteError::InvalidResponse;
    let epoch = fields
        .get("account_epoch")
        .and_then(Value::as_u64)
        .ok_or(bad)?;
    let state = fields
        .get("account_state")
        .and_then(Value::as_str)
        .ok_or(bad)?;
    let consent = Consent::decode(fields.get("consent").ok_or(bad)?).map_err(|_| bad)?;
    Ok(Account {
        transcript_reset: fields
            .get("resets")
            .and_then(|r| r.get("transcripts"))
            .and_then(|r| r.get("generation"))
            .and_then(Value::as_u64),
        epoch,
        active: state == "active",
        consent,
    })
}
fn decode_receipt(fields: &Map<String, Value>) -> Result<Receipt, Failure> {
    Ok(Receipt {
        operation: fields
            .get("operation")
            .cloned()
            .ok_or(Failure::Remote(RemoteError::InvalidResponse))?,
        revision: fields
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or(Failure::Remote(RemoteError::InvalidResponse))?,
    })
}
fn decode_stored(fields: &Map<String, Value>) -> Result<Stored, Failure> {
    let bad = || Failure::Remote(RemoteError::InvalidResponse);
    let mut record = Value::Object(fields.clone());
    let map = record.as_object_mut().ok_or_else(bad)?;
    let operation_id = map
        .remove("operation_id")
        .and_then(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
        .ok_or_else(bad)?;
    let consent_generation = map
        .remove("consent_generation")
        .and_then(|v| v.as_u64())
        .ok_or_else(bad)?;
    Ok(Stored {
        record: Record::decode(&record).map_err(|_| bad())?,
        operation_id,
        consent_generation,
    })
}
fn stored_fields(stored: &Stored) -> Value {
    let mut value = stored.record.encode();
    value["operation_id"] = json!(stored.operation_id);
    value["consent_generation"] = json!(stored.consent_generation);
    value
}
// ---- Firestore typed values ---------------------------------------------------

fn fields(document: &Value) -> Map<String, Value> {
    document["fields"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), decode_value(v)))
                .collect()
        })
        .unwrap_or_default()
}
fn encode_fields(value: &Value) -> Value {
    match encode_value(value) {
        Value::Object(mut map) => map
            .remove("mapValue")
            .and_then(|m| m.get("fields").cloned())
            .unwrap_or_else(|| json!({})),
        _ => json!({}),
    }
}
pub fn encode_value(value: &Value) -> Value {
    match value {
        Value::Null => json!({ "nullValue": null }),
        Value::Bool(b) => json!({ "booleanValue": b }),
        Value::Number(n) if n.is_u64() || n.is_i64() => json!({ "integerValue": n.to_string() }),
        Value::Number(n) => json!({ "doubleValue": n.as_f64() }),
        Value::String(s) => json!({ "stringValue": s }),
        Value::Array(items) => {
            json!({ "arrayValue": { "values": items.iter().map(encode_value).collect::<Vec<_>>() } })
        }
        Value::Object(map) => json!({ "mapValue": { "fields": map.iter()
            .map(|(k, v)| (k.clone(), encode_value(v))).collect::<Map<_, _>>() } }),
    }
}
pub fn decode_value(value: &Value) -> Value {
    let Some((kind, inner)) = value.as_object().and_then(|m| m.iter().next()) else {
        return Value::Null;
    };
    match kind.as_str() {
        "booleanValue" => inner.clone(),
        "integerValue" => inner
            .as_str()
            .and_then(|s| {
                s.parse::<u64>()
                    .ok()
                    .map(Value::from)
                    .or_else(|| s.parse::<i64>().ok().map(Value::from))
            })
            .unwrap_or(Value::Null),
        "doubleValue" => inner.clone(),
        "stringValue" => inner.clone(),
        "arrayValue" => Value::Array(
            inner["values"]
                .as_array()
                .map(|items| items.iter().map(decode_value).collect())
                .unwrap_or_default(),
        ),
        "mapValue" => Value::Object(
            inner["fields"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), decode_value(v)))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        _ => Value::Null,
    }
}
