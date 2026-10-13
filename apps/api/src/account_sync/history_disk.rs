//! Transactional transcript storage. Legacy JSON is imported once in the same
//! transaction as the migration marker. Only touched collection entries are
//! written thereafter; UUID keyset pages never deserialize the whole document.
use super::sync::RemoteError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;
type Result<T> = std::result::Result<T, RemoteError>;
fn bad<T>(_: T) -> RemoteError {
    RemoteError::StorageFailed
}

#[derive(Clone, Default)]
pub(super) struct Rows {
    data: BTreeMap<Uuid, Value>,
    dirty: BTreeSet<Uuid>,
    families: BTreeMap<Uuid, BTreeSet<Uuid>>,
}
impl<'de> Deserialize<'de> for Rows {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let data = BTreeMap::<Uuid, Value>::deserialize(d)?;
        let mut rows = Self::default();
        for (id, v) in data {
            rows.insert(id, v);
        }
        rows.dirty.clear();
        Ok(rows)
    }
}
impl Deref for Rows {
    type Target = BTreeMap<Uuid, Value>;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl Rows {
    fn original(v: &Value) -> Option<Uuid> {
        v["content"]["data"]["original_record_id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
    }
    pub fn family(&self, id: Uuid) -> Vec<Uuid> {
        self.families
            .get(&id)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }
    pub fn insert(&mut self, id: Uuid, value: Value) {
        if self.data.get(&id) != Some(&value) {
            self.remove(&id);
            if let Some(root) = Self::original(&value) {
                self.families.entry(root).or_default().insert(id);
            }
            self.dirty.insert(id);
            self.data.insert(id, value);
        }
    }
    pub fn remove(&mut self, id: &Uuid) {
        if let Some(v) = self.data.remove(id) {
            self.dirty.insert(*id);
            if let Some(root) = Self::original(&v) {
                if let Some(ids) = self.families.get_mut(&root) {
                    ids.remove(id);
                }
            }
        }
    }
    pub fn clear(&mut self) {
        self.dirty.extend(self.data.keys());
        self.data.clear();
        self.families.clear();
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&Uuid, &mut Value) -> bool) {
        let deleted: Vec<_> = self
            .data
            .iter_mut()
            .filter_map(|(id, v)| (!keep(id, v)).then_some(*id))
            .collect();
        for id in deleted {
            self.remove(&id);
        }
    }
    fn changes(&self) -> Vec<(String, Option<Value>)> {
        self.dirty
            .iter()
            .map(|id| (id.to_string(), self.data.get(id).cloned()))
            .collect()
    }
}
#[derive(Clone, Default, Deserialize)]
#[serde(transparent)]
pub(super) struct Ids {
    data: BTreeSet<Uuid>,
    #[serde(skip)]
    dirty: BTreeSet<Uuid>,
}
impl Deref for Ids {
    type Target = BTreeSet<Uuid>;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl FromIterator<Uuid> for Ids {
    fn from_iter<T: IntoIterator<Item = Uuid>>(it: T) -> Self {
        Self {
            data: it.into_iter().collect(),
            dirty: BTreeSet::new(),
        }
    }
}
impl Ids {
    pub fn insert(&mut self, id: Uuid) {
        if self.data.insert(id) {
            self.dirty.insert(id);
        }
    }
    pub fn clear(&mut self) {
        self.dirty.extend(self.data.iter());
        self.data.clear();
    }
    pub fn extend(&mut self, it: impl IntoIterator<Item = Uuid>) {
        for id in it {
            self.insert(id);
        }
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&Uuid) -> bool) {
        self.data.retain(|id| {
            if keep(id) {
                true
            } else {
                self.dirty.insert(*id);
                false
            }
        });
    }
    pub fn replace(&mut self, data: impl IntoIterator<Item = Uuid>) {
        self.clear();
        self.extend(data);
    }
    fn changes(&self) -> Vec<(String, Option<Value>)> {
        self.dirty
            .iter()
            .map(|id| {
                (
                    id.to_string(),
                    self.data.contains(id).then_some(json!(true)),
                )
            })
            .collect()
    }
}
#[derive(Clone, Default)]
pub(super) struct Queue {
    data: VecDeque<Value>,
    dirty: BTreeMap<String, (u64, Option<Value>)>,
    records: BTreeSet<String>,
    next: u64,
}
impl<'de> Deserialize<'de> for Queue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let data = VecDeque::<Value>::deserialize(d)?;
        let records = data
            .iter()
            .filter_map(|v| v["record_id"].as_str().map(str::to_owned))
            .collect();
        Ok(Self {
            data,
            records,
            ..Self::default()
        })
    }
}
impl Deref for Queue {
    type Target = VecDeque<Value>;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl Queue {
    pub fn first(&self) -> Option<&Value> {
        self.data.front()
    }
    pub fn contains_record(&self, id: Uuid) -> bool {
        self.records.contains(&id.to_string())
    }
    pub fn push(&mut self, v: Value) {
        self.next += 1;
        self.records.insert(v["record_id"].as_str().unwrap().into());
        self.dirty.insert(
            v["operation_id"].as_str().unwrap().into(),
            (self.next, Some(v.clone())),
        );
        self.data.push_back(v);
    }
    pub fn pop(&mut self) {
        if let Some(v) = self.data.pop_front() {
            self.next += 1;
            self.records.remove(v["record_id"].as_str().unwrap());
            self.dirty.insert(
                v["operation_id"].as_str().unwrap().into(),
                (self.next, None),
            );
        }
    }
    pub fn clear(&mut self) {
        while !self.data.is_empty() {
            self.pop();
        }
    }
}
pub(super) type Changes = Vec<(&'static str, Vec<(String, Option<Value>)>)>;
pub(super) trait Document: Default + Serialize + serde::de::DeserializeOwned {
    fn fields() -> &'static [(&'static str, bool)]; // bool: array (sets/outbox), otherwise map
    fn changes(&self) -> Changes;
    fn clean(&mut self);
}
impl Document for Rows {
    fn fields() -> &'static [(&'static str, bool)] {
        &[("records", false)]
    }
    fn changes(&self) -> Changes {
        vec![("records", self.changes())]
    }
    fn clean(&mut self) {
        self.dirty.clear();
    }
}
impl Serialize for Rows {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        json!({}).serialize(s)
    }
}

pub(super) fn row_changes(rows: &Rows) -> Vec<(String, Option<Value>)> {
    rows.changes()
}
pub(super) fn id_changes(ids: &Ids) -> Vec<(String, Option<Value>)> {
    ids.changes()
}
pub(super) fn queue_changes(queue: &Queue) -> Vec<(String, Option<Value>)> {
    {
        let mut changes: Vec<_> = queue
            .dirty
            .iter()
            .map(|(k, (seq, v))| (*seq, k.clone(), v.clone()))
            .collect();
        changes.sort_by_key(|(seq, _, _)| *seq);
        changes.into_iter().map(|(_, k, v)| (k, v)).collect()
    }
}
pub(super) fn clean_rows(rows: &mut Rows) {
    rows.dirty.clear();
}
pub(super) fn clean_ids(ids: &mut Ids) {
    ids.dirty.clear();
}
pub(super) fn clean_queue(queue: &mut Queue) {
    queue.dirty.clear();
}

fn connection(path: &Path, fields: &[(&str, bool)]) -> Result<Connection> {
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent).map_err(bad)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(bad)?;
    }
    let db_path = path.with_extension("sqlite3");
    let mut db = Connection::open(&db_path).map_err(bad)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600)).map_err(bad)?;
    }
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(bad)?;
    db.execute_batch("PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; CREATE TABLE IF NOT EXISTS meta (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS clock (id INTEGER PRIMARY KEY CHECK(id=1), seq INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS entries (field TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, live INTEGER NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY(field,key)); CREATE INDEX IF NOT EXISTS entry_sequence ON entries(seq); CREATE INDEX IF NOT EXISTS field_sequence ON entries(field,seq); CREATE INDEX IF NOT EXISTS live_page ON entries(field,live,key);").map_err(bad)?;
    let exists: bool = db
        .query_row("SELECT EXISTS(SELECT 1 FROM meta)", [], |r| r.get(0))
        .map_err(bad)?;
    if !exists {
        // A crash rolls this transaction back and leaves the legacy file readable.
        let mut legacy: Value = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(bad)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(bad(e)),
        };
        // Kept copies historically used a bare map.
        if fields == [("records", false)] {
            legacy = json!({"records":legacy});
        }
        let tx = db.transaction().map_err(bad)?;
        for (field, array) in fields {
            if let Some(data) = legacy.get_mut(*field) {
                if *array {
                    if let Some(values) = data.as_array() {
                        for (seq, v) in values.iter().enumerate() {
                            let key = v["operation_id"]
                                .as_str()
                                .or_else(|| v.as_str())
                                .ok_or(RemoteError::StorageFailed)?;
                            put(
                                &tx,
                                field,
                                key,
                                if v.is_string() { &json!(true) } else { v },
                                seq as i64,
                            )?;
                        }
                    }
                } else if let Some(values) = data.as_object() {
                    for (seq, (key, v)) in values.iter().enumerate() {
                        put(&tx, field, key, v, seq as i64)?;
                    }
                }
            }
            legacy
                .as_object_mut()
                .ok_or(RemoteError::StorageFailed)?
                .remove(*field);
        }
        tx.execute("INSERT INTO meta VALUES (1,?1)", [legacy.to_string()])
            .map_err(bad)?;
        tx.commit().map_err(bad)?;
    }
    // A cursor is a durable high-water mark, independent of surviving rows.
    // Deleting the newest entry must never reuse its sequence on the next write.
    db.execute(
        "INSERT OR IGNORE INTO clock SELECT 1, COALESCE(MAX(seq),0) FROM entries",
        [],
    )
    .map_err(bad)?;
    // The imported source must not retain text erased through the new store.
    if path.exists() {
        std::fs::remove_file(path).map_err(bad)?;
    }
    Ok(db)
}
fn put(db: &Connection, field: &str, key: &str, value: &Value, seq: i64) -> Result<()> {
    let live = value["content"]["state"] == "live";
    db.execute("INSERT INTO entries(field,key,value,live,seq) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(field,key) DO UPDATE SET value=excluded.value,live=excluded.live,seq=excluded.seq", params![field,key,value.to_string(),live,seq]).map_err(bad)?;
    Ok(())
}
pub(super) fn read<T: Document>(path: &Path) -> Result<T> {
    let db = connection(path, T::fields())?;
    let text: String = db
        .query_row("SELECT value FROM meta WHERE id=1", [], |r| r.get(0))
        .map_err(bad)?;
    let mut value: Value = serde_json::from_str(&text).map_err(bad)?;
    for (field, array) in T::fields() {
        let mut stmt = db
            .prepare("SELECT key,value FROM entries WHERE field=?1 ORDER BY seq,key")
            .map_err(bad)?;
        let entries = stmt
            .query_map([field], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(bad)?;
        let mut values = Vec::new();
        let mut map = serde_json::Map::new();
        for entry in entries {
            let (key, v) = entry.map_err(bad)?;
            let v: Value = serde_json::from_str(&v).map_err(bad)?;
            if *array {
                values.push(if v == json!(true) { json!(key) } else { v });
            } else {
                map.insert(key, v);
            }
        }
        value[*field] = if *array {
            json!(values)
        } else {
            Value::Object(map)
        };
    }
    if T::fields() == [("records", false)] {
        value = value["records"].take();
    }
    serde_json::from_value(value).map_err(bad)
}
pub(super) fn write<T: Document>(path: &Path, value: &mut T) -> Result<()> {
    let mut db = connection(path, T::fields())?;
    let tx = db.transaction().map_err(bad)?;
    let mut seq: i64 = tx
        .query_row("SELECT seq FROM clock WHERE id=1", [], |r| r.get(0))
        .map_err(bad)?;
    for (field, entries) in value.changes() {
        for (key, v) in entries {
            if let Some(v) = v {
                seq += 1;
                put(&tx, field, &key, &v, seq)?;
            } else {
                tx.execute(
                    "DELETE FROM entries WHERE field=?1 AND key=?2",
                    params![field, key],
                )
                .map_err(bad)?;
            }
        }
    }
    tx.execute("UPDATE clock SET seq=?1 WHERE id=1", [seq])
        .map_err(bad)?;
    let meta = serde_json::to_string(value).map_err(bad)?;
    tx.execute("UPDATE meta SET value=?1 WHERE id=1 AND value<>?1", [meta])
        .map_err(bad)?;
    tx.commit().map_err(bad)?;
    value.clean();
    Ok(())
}
pub(super) fn page<T: Document>(
    path: &Path,
    field: &str,
    after: Option<Uuid>,
    limit: usize,
) -> Result<Vec<Value>> {
    let db = connection(path, T::fields())?;
    let mut stmt = db
        .prepare(
            "SELECT value FROM entries WHERE field=?1 AND live=1 AND key>?2 ORDER BY key LIMIT ?3",
        )
        .map_err(bad)?;
    let rows = stmt
        .query_map(
            params![
                field,
                after.map(|id| id.to_string()).unwrap_or_default(),
                limit as i64
            ],
            |r| r.get::<_, String>(0),
        )
        .map_err(bad)?;
    rows.map(|v| serde_json::from_str(&v.map_err(bad)?).map_err(bad))
        .collect()
}

/// Move a cached document into a lease instead of copying its text on every
/// receipt. An unsaved mutation (including failed persistence) evicts it.
pub(super) struct Lease<'a, T> {
    cache: &'a Mutex<BTreeMap<PathBuf, T>>,
    path: PathBuf,
    value: Option<T>,
    changed: bool,
}
impl<T> Deref for Lease<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value.as_ref().unwrap()
    }
}
impl<T> DerefMut for Lease<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.changed = true;
        self.value.as_mut().unwrap()
    }
}
impl<T> Drop for Lease<'_, T> {
    fn drop(&mut self) {
        if !self.changed {
            self.cache
                .lock()
                .unwrap()
                .insert(self.path.clone(), self.value.take().unwrap());
        }
    }
}
impl<'a, T: Document> Lease<'a, T> {
    pub fn open(cache: &'a Mutex<BTreeMap<PathBuf, T>>, path: PathBuf) -> Result<Self> {
        let value = cache
            .lock()
            .unwrap()
            .remove(&path)
            .map(Ok)
            .unwrap_or_else(|| read(&path))?;
        Ok(Self {
            cache,
            path,
            value: Some(value),
            changed: false,
        })
    }
    pub fn save(&mut self) -> Result<()> {
        write(&self.path, self.value.as_mut().unwrap())?;
        self.changed = false;
        Ok(())
    }
}

pub(super) fn changes<T: Document>(
    path: &Path,
    field: &str,
    after: i64,
    limit: usize,
) -> Result<Vec<(i64, Value)>> {
    let db = connection(path, T::fields())?;
    let mut stmt = db
        .prepare("SELECT seq,value FROM entries WHERE field=?1 AND seq>?2 ORDER BY seq LIMIT ?3")
        .map_err(bad)?;
    let rows = stmt
        .query_map(params![field, after, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(bad)?;
    rows.map(|v| {
        let (seq, v) = v.map_err(bad)?;
        Ok((seq, serde_json::from_str(&v).map_err(bad)?))
    })
    .collect()
}

pub(super) fn metadata<T: Document>(path: &Path) -> Result<T> {
    let db = connection(path, T::fields())?;
    let text: String = db
        .query_row("SELECT value FROM meta WHERE id=1", [], |r| r.get(0))
        .map_err(bad)?;
    serde_json::from_str(&text).map_err(bad)
}
pub(super) fn count<T: Document>(path: &Path, field: &str, live: bool) -> Result<usize> {
    let db = connection(path, T::fields())?;
    db.query_row(
        "SELECT count(*) FROM entries WHERE field=?1 AND (?2=0 OR live=1)",
        params![field, live],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .map_err(bad)
}
