use super::*;
use std::time::Instant;
fn row(id: Uuid) -> Value {
    json!({"schema_version":1,"id":id,"category":"transcripts","origin_device_id":Uuid::from_u128(999),"revision":1,"content":{"state":"live","data":{"kind":"original","attempt_id":id,"recorded_at_ms":10,"input_language":"mixed","output_language":{"mode":"preserve"},"selection":{"provider":"gemini","model":"synthetic-stt"},"text":"English עברית 🌍".repeat(32)}}})
}
#[test]
fn history_fixed_1k_10k_indexed_pages_migration_and_incremental_bytes() {
    for n in [1000u128, 10000] {
        let root = std::env::temp_dir().join(format!("writer-history-scale-{}", Uuid::new_v4()));
        let h = History::open(root.clone());
        let key = AccountRef {
            project_id: "demo-writer".into(),
            uid: "synthetic-scale".into(),
        };
        let path = h.path(&key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let rows: BTreeMap<_, _> = (1..=n)
            .map(|i| {
                let id = Uuid::from_u128(i);
                (id.to_string(), row(id))
            })
            .collect();
        let legacy = json!({"joined":true,"epoch":1,"generation":1,"enrollment":Uuid::from_u128(998),"rows":rows,"outbox":[],"hidden":[],"eligible":[],"excluded_attempts":[]});
        let bytes = serde_json::to_vec(&legacy).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let before = Instant::now();
        let old: Value = serde_json::from_slice(&bytes).unwrap();
        let old_page: Vec<_> = old["rows"]
            .as_object()
            .unwrap()
            .values()
            .take(20)
            .cloned()
            .collect();
        let old_latency = before.elapsed();
        // Import is deliberately outside the steady-state timing. Cold reopen
        // below has no in-memory rows, proving it uses the persisted index.
        assert_eq!(h.local_page(Some(&key), None).unwrap().records.len(), 20);
        assert!(!path.exists());
        drop(h);
        let h = History::open(root.clone());
        let after = Instant::now();
        let page = h.local_page(Some(&key), None).unwrap();
        let latency = after.elapsed();
        assert_eq!(page.records, old_page);
        let db = Connection::open(path.with_extension("sqlite3")).unwrap();
        let plan:String=db.query_row("EXPLAIN QUERY PLAN SELECT value FROM entries WHERE field='rows' AND live=1 AND key>'00000000-0000-0000-0000-000000000000' ORDER BY key LIMIT 20",[],|r|r.get(3)).unwrap();
        assert!(plan.contains("live_page"), "{plan}");
        drop(db);
        let mut seen = BTreeSet::new();
        let mut cursor = None;
        loop {
            let p = h.local_page(Some(&key), cursor).unwrap();
            for v in p.records {
                assert!(seen.insert(v["id"].as_str().unwrap().to_owned()));
            }
            cursor = p.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), n as usize);
        // Hydrate once, then measure actual file-page changes for one offline edit.
        let source = Uuid::from_u128(1);
        let _ = h.state(&key).unwrap();
        let before_file = std::fs::read(path.with_extension("sqlite3")).unwrap();
        let id = h.edit(&key, source, "offline edited עברית".into()).unwrap();
        let after_file = std::fs::read(path.with_extension("sqlite3")).unwrap();
        let changed_pages = before_file
            .chunks(4096)
            .zip(after_file.chunks(4096))
            .filter(|(a, b)| a != b)
            .count()
            + after_file
                .len()
                .saturating_sub(before_file.len())
                .div_ceil(4096);
        assert!(
            changed_pages * 4096 < bytes.len() / 2,
            "single edit must not rewrite the library"
        );
        drop(h);
        let h = History::open(root.clone());
        let state = h.state(&key).unwrap();
        let edit = state.rows.get(&id).unwrap();
        assert_eq!(
            edit["content"]["data"]["parent_record_id"],
            source.to_string()
        );
        assert_eq!(state.rows.len(), n as usize + 1);
        drop(state);
        // Concurrent arrivals below a cursor wait for refresh; rows above it
        // stay reachable. Deletion doesn't move an immutable UUID cursor.
        let first = h.local_page(Some(&key), None).unwrap();
        let cursor = first.next.unwrap();
        let mut s = h.state(&key).unwrap();
        s.rows.remove(&Uuid::from_u128(21));
        s.rows
            .insert(Uuid::from_u128(n + 1), row(Uuid::from_u128(n + 1)));
        s.save().unwrap();
        drop(s);
        let next = h.local_page(Some(&key), Some(cursor)).unwrap();
        assert!(next
            .records
            .iter()
            .all(|v| Uuid::parse_str(v["id"].as_str().unwrap()).unwrap() > cursor));
        assert!(next
            .records
            .iter()
            .all(|v| v["id"] != Uuid::from_u128(21).to_string()));
        let other = AccountRef {
            uid: "synthetic-other".into(),
            ..key.clone()
        };
        assert!(h.local_page(Some(&other), None).unwrap().records.is_empty());
        eprintln!("MW112 records={n} before_list_us={} after_cold_list_us={} before_list_rows={n} after_list_rows=20 before_rewrite_bytes={} after_changed_pages_bytes={}",old_latency.as_micros(),latency.as_micros(),bytes.len(),changed_pages*4096);
        drop(h);
        std::fs::remove_dir_all(root).unwrap();
    }
}
use rusqlite::Connection;
#[test]
fn history_deleted_highest_sequence_cannot_hide_later_changes_after_restart() {
    let root = std::env::temp_dir().join(format!("writer-history-cursor-{}", Uuid::new_v4()));
    let h = History::open(root.clone());
    let path = root.join("cursor.json");
    let mut cache = Mutex::new(BTreeMap::new());
    let mut rows: Lease<'_, Rows> = Lease::open(&cache, path.clone()).unwrap();
    rows.insert(Uuid::from_u128(1), row(Uuid::from_u128(1)));
    rows.save().unwrap();
    let cursor = disk::changes::<Rows>(&path, "records", 0, 100).unwrap()[0].0;
    rows.clear();
    rows.save().unwrap();
    drop(rows);
    cache.get_mut().unwrap().clear();
    let mut rows: Lease<'_, Rows> = Lease::open(&cache, path.clone()).unwrap();
    rows.insert(Uuid::from_u128(2), row(Uuid::from_u128(2)));
    rows.save().unwrap();
    let changes = disk::changes::<Rows>(&path, "records", cursor, 100).unwrap();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].0 > cursor);
    assert_eq!(changes[0].1["id"], Uuid::from_u128(2).to_string());
    drop(rows);
    drop(h);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn interrupted_import_and_failed_transaction_keep_originals_readable() {
    let root = std::env::temp_dir().join(format!("writer-history-recovery-{}", Uuid::new_v4()));
    let h = History::open(root.clone());
    let key = AccountRef {
        project_id: "demo-writer".into(),
        uid: "synthetic-recovery".into(),
    };
    let path = h.path(&key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let id = Uuid::from_u128(1);
    std::fs::write(
        &path,
        json!({"joined":true,"rows":{id.to_string():row(id)}}).to_string(),
    )
    .unwrap();
    // Abort a migration-like SQLite transaction before a durable marker.
    let mut db = Connection::open(path.with_extension("sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE meta (id INTEGER PRIMARY KEY CHECK(id=1),value TEXT NOT NULL)")
        .unwrap();
    {
        let tx = db.transaction().unwrap();
        tx.execute("INSERT INTO meta VALUES(1,'{}')", []).unwrap();
    }
    drop(db);
    assert!(path.exists());
    assert_eq!(
        h.local_page(Some(&key), None).unwrap().records,
        vec![row(id)]
    );
    let db = Connection::open(path.with_extension("sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_edit BEFORE INSERT ON entries WHEN NEW.field='rows' BEGIN SELECT RAISE(ABORT,'synthetic storage failure'); END").unwrap();
    assert_eq!(
        h.edit(&key, id, "failed edit".into()),
        Err(RemoteError::StorageFailed)
    );
    assert_eq!(
        h.local_page(Some(&key), None).unwrap().records,
        vec![row(id)]
    );
    assert_eq!(h.status(&key).unwrap().pending, 0);
    drop(db);
    drop(h);
    let h = History::open(root.clone());
    assert_eq!(
        h.local_page(Some(&key), None).unwrap().records,
        vec![row(id)]
    );
    drop(h);
    std::fs::remove_dir_all(root).unwrap();
}
