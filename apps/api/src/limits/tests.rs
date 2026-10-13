use super::*;
use crate::{
    jobs::{JobStore, Pipeline},
    providers::SavedAudio,
    recordings::{FileRecordingStore, RecordingStore},
    transcription::*,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer-limits-{}", Uuid::new_v4())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn metadata() -> RecordingMetadata {
    let fixture: ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let mut m = fixture.recording.data.metadata;
    m.id = Uuid::new_v4();
    m.byte_length = 32;
    m.duration_ms = 12 * 60 * 60 * 1000;
    m
}
async fn store(root: &Temp, limits: Limits) -> Arc<FileRecordingStore> {
    let budget = storage::StorageBudget::new(root.0.clone(), limits);
    Arc::new(
        FileRecordingStore::open_with_budget(root.0.clone(), 1024 * 1024, budget)
            .await
            .unwrap(),
    )
}
#[tokio::test]
async fn stalled_upload_does_not_block_second_upload_and_idle_releases_everything() {
    let root = Temp::new();
    let store = store(
        &root,
        Limits {
            idle: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .await;
    let stalled = metadata();
    let id = stalled.id;
    let (reader, mut writer) = tokio::io::duplex(64);
    let copy = store.clone();
    let task = tokio::spawn(async move { copy.save(stalled, Box::new(reader)).await });
    writer.write_all(b"partial").await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let good = metadata();
    let saved = tokio::time::timeout(
        Duration::from_secs(2),
        store.save(good.clone(), Box::new(std::io::Cursor::new(vec![7; 32]))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(saved.audio, AudioState::Saved {}));
    let error = tokio::time::timeout(Duration::from_secs(6), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.retryable);
    assert!(!root.0.join(format!("{id}.uploading")).exists());
    assert!(store.audio(good.id).await.is_ok());
    store
        .save(
            {
                let mut m = metadata();
                m.id = id;
                m
            },
            Box::new(std::io::Cursor::new(vec![7; 32])),
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn reservations_bound_parallel_uploads_and_cancel_frees_space() {
    let root = Temp::new();
    let store = store(
        &root,
        Limits {
            storage_bytes: 25000,
            device_storage_bytes: 25000,
            ..Default::default()
        },
    )
    .await;
    let m = metadata();
    let id = m.id;
    let (reader, _writer) = tokio::io::duplex(64);
    let copy = store.clone();
    let task = tokio::spawn(async move { copy.save(m, Box::new(reader)).await });
    for _ in 0..1000 {
        if root.0.join(format!("{id}.uploading/audio")).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let error = store
        .save(metadata(), Box::new(std::io::Cursor::new(vec![0; 32])))
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::RateLimited));
    task.abort();
    let _ = task.await;
    assert!(!root.0.join(format!("{id}.uploading")).exists());
    store
        .save(metadata(), Box::new(std::io::Cursor::new(vec![0; 32])))
        .await
        .unwrap();
}
#[tokio::test]
async fn oversized_declared_bytes_crash_leftovers_and_long_progressing_uploads() {
    let root = Temp::new();
    let store = store(
        &root,
        Limits {
            idle: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .await;
    let m = metadata();
    let id = m.id;
    assert!(store
        .save(m, Box::new(std::io::Cursor::new(vec![0; 33])))
        .await
        .is_err());
    assert!(!root.0.join(format!("{id}.uploading")).exists());
    let abandoned = root.0.join(format!("{}.uploading", Uuid::new_v4()));
    tokio::fs::create_dir(&abandoned).await.unwrap();
    tokio::fs::write(abandoned.join("audio"), b"crashed")
        .await
        .unwrap();
    drop(store);
    let reopened = FileRecordingStore::open(root.0.clone(), 1024 * 1024)
        .await
        .unwrap();
    assert!(!abandoned.exists());
    let (reader, mut writer) = tokio::io::duplex(64);
    let task = tokio::spawn(async move {
        for _ in 0..8 {
            writer.write_all(&[1; 4]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });
    reopened.save(metadata(), Box::new(reader)).await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn shared_storage_and_device_storage_both_apply_after_reopen() {
    let root = Temp::new();
    let limits = Limits {
        storage_bytes: 20000,
        device_storage_bytes: 18000,
        ..Default::default()
    };
    let budget = storage::StorageBudget::new(root.0.clone(), limits.clone());
    let a = FileRecordingStore::open_with_budget(root.0.join("a"), 10000, budget.clone())
        .await
        .unwrap();
    let b = FileRecordingStore::open_with_budget(root.0.join("b"), 10000, budget)
        .await
        .unwrap();
    let mut m = metadata();
    m.byte_length = 1000;
    a.save(m.clone(), Box::new(std::io::Cursor::new(vec![1; 1000])))
        .await
        .unwrap();
    // External/older retained files count too; no overwrite or eviction to fit.
    tokio::fs::write(root.0.join("retained"), vec![2; 4000])
        .await
        .unwrap();
    assert!(b
        .save(metadata(), Box::new(std::io::Cursor::new(vec![1; 32])))
        .await
        .is_err());
    assert!(a.audio(m.id).await.is_ok());
    drop(a);
    drop(b);
    let b = FileRecordingStore::open_with_budget(
        root.0.join("b"),
        10000,
        storage::StorageBudget::new(root.0.clone(), limits),
    )
    .await
    .unwrap();
    assert!(b
        .save(metadata(), Box::new(std::io::Cursor::new(vec![1; 32])))
        .await
        .is_err());
}
#[test]
fn processing_capacity_is_shared_and_released_without_daily_usage() {
    let limits = Limits {
        work_concurrency: 2,
        device_work_concurrency: 1,
        ..Default::default()
    };
    let capacity = work::WorkCapacity::new(limits);
    let a = capacity.account();
    let b = capacity.account();
    let c = capacity.account();
    let first = a.admit().unwrap();
    assert!(a.admit().is_err(), "one owner's text slot is occupied");
    let second = b.admit().unwrap();
    assert!(c.admit().is_err(), "both shared slots are occupied");
    drop(first);
    drop(second);
    // Exceed both former call caps; completed work never exhausts admission.
    for _ in 0..2_001 {
        drop(a.admit().unwrap());
        drop(b.try_admit_transcription().unwrap());
    }
    assert!(c.admit().is_ok());
}
#[tokio::test]
async fn queue_saturation_preserves_idempotency_and_audio_and_recovers() {
    let root = Temp::new();
    let limits = Limits {
        pending: 2,
        ..Default::default()
    };
    let shared = storage::StorageBudget::new(root.0.clone(), limits.clone());
    let budget = work::WorkCapacity::new(limits);
    let mut pipelines = vec![];
    for owner in ["a", "b", "c"] {
        let p = Pipeline::open_shared(
            root.0.join(owner),
            1000,
            vec![],
            root.0.join(owner).join("modes"),
            shared.clone(),
            Some(budget.account()),
        )
        .await
        .unwrap();
        pipelines.push(p);
    }
    let fixture: ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let mut requests = vec![];
    for p in &pipelines {
        let m = metadata();
        p.recordings
            .save(m.clone(), Box::new(std::io::Cursor::new(vec![0; 32])))
            .await
            .unwrap();
        let mut r = fixture.transcription_request.data.clone();
        r.recording_id = m.id;
        r.idempotency_key = Uuid::new_v4();
        requests.push(r);
    }
    let first = pipelines[0].jobs.submit(requests[0].clone()).await.unwrap();
    pipelines[1].jobs.submit(requests[1].clone()).await.unwrap();
    assert_eq!(
        pipelines[0]
            .jobs
            .submit(requests[0].clone())
            .await
            .unwrap()
            .id,
        first.id
    );
    assert!(
        pipelines[2]
            .jobs
            .submit(requests[2].clone())
            .await
            .unwrap_err()
            .retryable
    );
    pipelines[0].run_pending().await.unwrap(); // no provider: explicit retryable source retained
    assert!(pipelines[0]
        .recordings
        .audio(first.request.recording_id)
        .await
        .is_ok());
    pipelines[2].jobs.submit(requests[2].clone()).await.unwrap();
}
#[tokio::test]
async fn multipart_idle_before_metadata_and_declared_size_rejection_finish_promptly() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let root = Temp::new();
    let limits = Limits {
        idle: Duration::from_millis(50),
        ..Default::default()
    };
    let budget = storage::StorageBudget::new(root.0.clone(), limits);
    let p = Pipeline::open_shared(
        root.0.clone(),
        1000,
        vec![],
        root.0.join("modes"),
        budget,
        None,
    )
    .await
    .unwrap();
    let app = crate::recordings::routes_with_pipeline(Some(p.clone()));
    let (reader, _writer) = tokio::io::duplex(64);
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        app.clone().oneshot(
            Request::post("/api/v1/recordings")
                .header("content-type", "multipart/form-data; boundary=X")
                .body(Body::from_stream(tokio_util::io::ReaderStream::new(reader)))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let m = metadata();
    let prefix = format!("--X\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{}\r\n--X\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"a\"\r\nContent-Type: audio/wav\r\n\r\n", serde_json::to_string(&Envelope::new(m.clone())).unwrap());
    let (reader, mut writer) = tokio::io::duplex(4096);
    writer.write_all(prefix.as_bytes()).await.unwrap();
    writer.write_all(&[7; 200]).await.unwrap(); // malicious uploader never closes the field
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        app.oneshot(
            Request::post("/api/v1/recordings")
                .header("content-type", "multipart/form-data; boundary=X")
                .body(Body::from_stream(tokio_util::io::ReaderStream::new(reader)))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!response.status().is_success());
    assert!(p.recordings.get(m.id).await.is_err());
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn enospc_is_retryable_and_does_not_remove_committed_audio() {
    let root = Temp::new();
    let store = store(&root, Limits::default()).await;
    let m = metadata();
    store
        .save(m.clone(), Box::new(std::io::Cursor::new(vec![1; 32])))
        .await
        .unwrap();
    let full = crate::recordings::disk::private_file(std::path::Path::new("/dev/full"))
        .await
        .unwrap();
    let error = crate::recordings::ingest::receive(
        Box::new(std::io::Cursor::new(vec![1; 32])),
        full,
        32,
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert!(error.retryable);
    assert!(matches!(error.code, ErrorCode::StorageFailed));
    assert_eq!(
        tokio::fs::read(store.audio(m.id).await.unwrap().path)
            .await
            .unwrap(),
        vec![1; 32]
    );
}

#[tokio::test]
async fn same_id_is_exclusive_and_early_quota_denial_cancels_a_silent_body() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let root = Temp::new();
    let budget = storage::StorageBudget::new(root.0.clone(), Limits::default());
    let p = Pipeline::open_shared(
        root.0.clone(),
        1000,
        vec![],
        root.0.join("modes"),
        budget,
        None,
    )
    .await
    .unwrap();
    let m = metadata();
    let id = m.id;
    let (reader, mut writer) = tokio::io::duplex(128);
    let copy = p.clone();
    let copy_m = m.clone();
    let saving = tokio::spawn(async move { copy.recordings.save(copy_m, Box::new(reader)).await });
    writer.write_all(&[1; 4]).await.unwrap();
    for _ in 0..1000 {
        if root
            .0
            .join(format!("recordings/{id}.uploading/audio"))
            .exists()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        p.recordings
            .save(m.clone(), Box::new(std::io::Cursor::new(vec![2; 32])))
            .await
            .unwrap_err()
            .retryable
    );
    assert!(root
        .0
        .join(format!("recordings/{id}.uploading/audio"))
        .exists());
    writer.write_all(&[1; 28]).await.unwrap();
    drop(writer);
    saving.await.unwrap().unwrap();
    // Existing ID rejects admission without waiting for the audio field or its EOF.
    let prefix = format!(
        "--X\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{}\r\n--X\r\n",
        serde_json::to_string(&Envelope::new(m)).unwrap()
    );
    let (reader, mut writer) = tokio::io::duplex(4096);
    writer.write_all(prefix.as_bytes()).await.unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        crate::recordings::routes_with_pipeline(Some(p.clone())).oneshot(
            Request::post("/api/v1/recordings")
                .header("content-type", "multipart/form-data; boundary=X")
                .body(Body::from_stream(tokio_util::io::ReaderStream::new(reader)))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        tokio::fs::read(p.recordings.audio(id).await.unwrap().path)
            .await
            .unwrap(),
        vec![1; 32]
    );
}

#[tokio::test]
async fn reopening_over_quota_keeps_history_readable_and_pending_work_counted() {
    let root = Temp::new();
    let p = Pipeline::open(root.0.clone(), 1000, vec![]).await.unwrap();
    let m = metadata();
    p.recordings
        .save(m.clone(), Box::new(std::io::Cursor::new(vec![3; 32])))
        .await
        .unwrap();
    let fixture: ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let mut request = fixture.transcription_request.data;
    request.recording_id = m.id;
    let job = p.jobs.submit(request.clone()).await.unwrap();
    p.jobs
        .set_status(job.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    drop(p);
    let limits = Limits {
        storage_bytes: 1,
        device_storage_bytes: 1,
        pending: 1,
        ..Default::default()
    };
    let budget = storage::StorageBudget::new(root.0.clone(), limits);
    let p = Pipeline::open_shared(
        root.0.clone(),
        1000,
        vec![],
        root.0.join("modes"),
        budget,
        None,
    )
    .await
    .unwrap();
    assert!(p.recordings.audio(m.id).await.is_ok());
    assert_eq!(p.jobs.submit(request).await.unwrap().id, job.id);
    assert_eq!(p.jobs.recoverable().await.unwrap().len(), 1);
    assert!(p.work.queue(false).is_err());
    assert!(p.run_pending().await.is_err());
    assert!(p.recordings.audio(m.id).await.is_ok());
}
#[tokio::test]
async fn measure_counts_a_nested_path_without_its_own_directory() {
    let root = Temp::new();
    tokio::fs::create_dir_all(root.0.join("sub")).await.unwrap();
    tokio::fs::write(root.0.join("a"), [1; 5]).await.unwrap();
    tokio::fs::write(root.0.join("sub/b"), [1; 7])
        .await
        .unwrap();
    let measured = storage::measure(vec![
        root.0.clone(),
        root.0.join("sub"),
        root.0.join("missing"),
    ])
    .await
    .unwrap();
    assert_eq!(measured[&root.0], (12, 3));
    assert_eq!(measured[&root.0.join("sub")], (7, 1));
    assert_eq!(measured[&root.0.join("missing")], (0, 0));
}
const MIB: u64 = 1024 * 1024;
#[tokio::test]
async fn hardlinked_audio_counts_once_per_quota_root_but_every_name_is_an_entry() {
    let root = Temp::new();
    let native = root.0.join("native-recordings");
    let imported = root.0.join("recordings");
    tokio::fs::create_dir_all(&native).await.unwrap();
    tokio::fs::create_dir_all(&imported).await.unwrap();
    let original = native.join("audio.wav");
    tokio::fs::write(&original, vec![7; 1024]).await.unwrap();
    tokio::fs::hard_link(&original, imported.join("audio"))
        .await
        .unwrap();
    // Equal bytes in a separate file still use separate storage.
    tokio::fs::copy(&original, imported.join("copy"))
        .await
        .unwrap();
    let measured = storage::measure(vec![root.0.clone(), native.clone(), imported.clone()])
        .await
        .unwrap();
    assert_eq!(measured[&root.0], (2048, 5));
    assert_eq!(measured[&native], (1024, 1));
    assert_eq!(measured[&imported], (2048, 2));
    // Removing one name does not free the bytes retained by the other name.
    tokio::fs::remove_file(&original).await.unwrap();
    assert_eq!(
        storage::measure(vec![root.0.clone()]).await.unwrap()[&root.0],
        (2048, 4)
    );
}

#[tokio::test]
async fn native_import_survives_reopening_history_over_the_old_double_counted_limit() {
    use crate::providers::SavedAudio;
    let root = Temp::new();
    let native = root.0.join("native-recordings");
    let imported = root.0.join("recordings");
    tokio::fs::create_dir_all(&native).await.unwrap();
    let old = native.join("old.wav");
    tokio::fs::write(&old, vec![7; 2 * MIB as usize])
        .await
        .unwrap();
    tokio::fs::create_dir_all(&imported).await.unwrap();
    tokio::fs::hard_link(&old, imported.join("old-audio"))
        .await
        .unwrap();
    let limits = Limits {
        storage_bytes: 3 * MIB,
        device_storage_bytes: 3 * MIB,
        ..Limits::default()
    };
    // A fresh budget has to scan persisted history; no old cache can mask the bug.
    let budget = storage::StorageBudget::new(root.0.clone(), limits.clone());
    let store = FileRecordingStore::open_scoped(imported.clone(), MIB, budget, root.0.clone())
        .await
        .unwrap();
    let audio = native.join("new.wav");
    tokio::fs::write(&audio, vec![3; 32]).await.unwrap();
    let source = SavedAudio {
        metadata: metadata(),
        path: audio.clone(),
        prepared: None,
    };
    store.import_native(source.clone()).await.unwrap();
    assert_eq!(
        tokio::fs::read(&store.audio(source.metadata.id).await.unwrap().path)
            .await
            .unwrap(),
        vec![3; 32]
    );
    drop(store);
    let store = FileRecordingStore::open_scoped(
        imported,
        MIB,
        storage::StorageBudget::new(root.0.clone(), limits),
        root.0.clone(),
    )
    .await
    .unwrap();
    store.import_native(source.clone()).await.unwrap();
    // An actual separate copy exceeding the quota must still stop new writes.
    tokio::fs::copy(&old, native.join("separate-copy.wav"))
        .await
        .unwrap();
    let mut next = source;
    next.metadata.id = Uuid::new_v4();
    let error = store.import_native(next).await.unwrap_err();
    assert!(error.message.contains("Storage budget reached"));
    assert_eq!(tokio::fs::read(audio).await.unwrap(), vec![3; 32]);
}

#[tokio::test]
#[ignore = "Explicit read-only audit of an existing private Windows history"]
async fn audit_existing_history_storage_without_modifying_it() {
    let root = PathBuf::from(
        std::env::var_os("WRITER_STORAGE_AUDIT_ROOT").expect("explicit history root"),
    );
    let measured = storage::measure(vec![root.clone()]).await.unwrap();
    let (bytes, entries) = measured[&root];
    eprintln!("Read-only storage audit: {bytes} unique file bytes, {entries} names");
    if let Ok(expected) = std::env::var("WRITER_STORAGE_AUDIT_EXPECTED_BYTES") {
        assert_eq!(bytes, expected.parse::<u64>().unwrap());
    }
}

fn gib_budget(root: &Temp) -> Arc<storage::StorageBudget> {
    storage::StorageBudget::new(
        root.0.clone(),
        Limits {
            storage_bytes: 1024 * MIB,
            device_storage_bytes: 1024 * MIB,
            ..Default::default()
        },
    )
}
#[tokio::test]
async fn far_below_budget_admits_without_walking_and_near_it_walks_exactly() {
    let root = Temp::new();
    tokio::fs::create_dir_all(&root.0).await.unwrap();
    let budget = gib_budget(&root);
    drop(budget.reserve(&root.0, root.0.join("a"), 10).await.unwrap());
    assert_eq!(budget.walks(), 1, "the first request has nothing cached");
    drop(budget.reserve(&root.0, root.0.join("b"), 10).await.unwrap());
    assert_eq!(budget.walks(), 1, "far below every budget reuses the walk");
    // Within the margin: the exact walk decides, and still admits what fits.
    drop(
        budget
            .reserve(&root.0, root.0.join("c"), 900 * MIB)
            .await
            .unwrap(),
    );
    assert_eq!(budget.walks(), 2);
    // An owner that was never measured is walked, never guessed.
    let other = root.0.join("other");
    drop(budget.reserve(&other, other.join("d"), 10).await.unwrap());
    assert_eq!(budget.walks(), 3);
}
#[tokio::test]
async fn released_reservations_count_until_a_newer_walk_covers_them() {
    let root = Temp::new();
    tokio::fs::create_dir_all(&root.0).await.unwrap();
    let budget = gib_budget(&root);
    // Its bytes may land after the walk that measured the directory.
    drop(
        budget
            .reserve(&root.0, root.0.join("a"), 300 * MIB)
            .await
            .unwrap(),
    );
    assert_eq!(budget.walks(), 1);
    // 300 released + 600 requested + a quarter of the remaining 724 MiB
    // exceeds 1 GiB: the released write cannot disappear from accounting.
    drop(
        budget
            .reserve(&root.0, root.0.join("b"), 600 * MIB)
            .await
            .unwrap(),
    );
    assert_eq!(budget.walks(), 2);
    // A cached admission schedules a background walk that re-bases the ledger.
    drop(budget.reserve(&root.0, root.0.join("c"), 10).await.unwrap());
    for _ in 0..500 {
        if budget.walks() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(budget.walks() >= 3, "no background walk after admission");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let walks = budget.walks();
    drop(
        budget
            .reserve(&root.0, root.0.join("d"), 500 * MIB)
            .await
            .unwrap(),
    );
    assert_eq!(
        budget.walks(),
        walks,
        "the refreshed walk covers earlier releases"
    );
}
#[tokio::test]
async fn cached_admission_still_refuses_a_duplicate_upload() {
    let root = Temp::new();
    tokio::fs::create_dir_all(&root.0).await.unwrap();
    let budget = gib_budget(&root);
    drop(budget.reserve(&root.0, root.0.join("a"), 10).await.unwrap());
    let held = budget.reserve(&root.0, root.0.join("b"), 10).await.unwrap();
    let error = budget
        .reserve(&root.0, root.0.join("b"), 10)
        .await
        .err()
        .unwrap();
    assert!(matches!(error.code, ErrorCode::RateLimited));
    drop(held);
}

fn sized_file(path: &std::path::Path, bytes: u64) {
    // Sparse logical size: exercise the real quota without writing GiB of zeros.
    std::fs::File::create(path).unwrap().set_len(bytes).unwrap();
}

#[tokio::test]
async fn large_history_import_and_job_saves_reuse_one_storage_walk() {
    let root = Temp::new();
    let history = root.0.join("native-recordings");
    std::fs::create_dir_all(&history).unwrap();
    sized_file(&history.join("retained.pcm"), 3900 * MIB);
    for i in 0..13_000 {
        std::fs::write(history.join(format!("entry-{i}")), b"saved").unwrap();
    }
    let budget = storage::StorageBudget::new(root.0.clone(), Limits::default());
    let recordings = Arc::new(
        FileRecordingStore::open_scoped(
            root.0.join("recordings"),
            1024 * MIB,
            budget.clone(),
            root.0.clone(),
        )
        .await
        .unwrap(),
    );
    let jobs = crate::jobs::FileJobStore::open_budgeted(
        root.0.join("jobs"),
        recordings.clone(),
        None,
        Some(budget.clone()),
    )
    .await
    .unwrap();
    let m = metadata();
    let native = history.join("new.wav");
    std::fs::write(&native, [7; 32]).unwrap();
    // The capture/startup check has measured the existing history. Stop must
    // not repeat that walk at import, queue, transcribing, and result saves.
    let started = std::time::Instant::now();
    drop(
        budget
            .reserve(&root.0, root.0.join("capture"), 16_384)
            .await
            .unwrap(),
    );
    let scan_ms = started.elapsed().as_millis();
    assert_eq!(budget.admission_walks(), 1);
    let started = std::time::Instant::now();
    recordings
        .import_native(SavedAudio {
            metadata: m.clone(),
            path: native.clone(),
            prepared: None,
        })
        .await
        .unwrap();
    let fixture: ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let mut request = fixture.transcription_request.data;
    request.recording_id = m.id;
    let job = jobs.submit(request).await.unwrap();
    jobs.set_status(job.id, JobStatus::Transcribing {})
        .await
        .unwrap();
    let completed = jobs
        .set_status(job.id, fixture.job.data.status)
        .await
        .unwrap();
    assert!(matches!(completed.status, JobStatus::Succeeded { .. }));
    assert_eq!(
        budget.admission_walks(),
        1,
        "a large history must not rescan for every save"
    );
    eprintln!("Large history: 3900 MiB, 13000 entries; first scan {scan_ms} ms, import/job saves {} ms, one foreground walk", started.elapsed().as_millis());
    assert_eq!(std::fs::read(native).unwrap(), [7; 32]);
    assert_eq!(
        std::fs::read(recordings.audio(m.id).await.unwrap().path).unwrap(),
        [7; 32]
    );
    assert_eq!(
        std::fs::metadata(history.join("retained.pcm"))
            .unwrap()
            .len(),
        3900 * MIB
    );
}

#[tokio::test]
async fn recent_near_quota_accounting_keeps_active_and_released_writes() {
    let root = Temp::new();
    std::fs::create_dir_all(&root.0).unwrap();
    sized_file(&root.0.join("retained"), 900 * MIB);
    let budget = gib_budget(&root);
    drop(
        budget
            .reserve(&root.0, root.0.join("warm"), 10)
            .await
            .unwrap(),
    );
    let held = budget
        .reserve(&root.0, root.0.join("held"), 32 * MIB)
        .await
        .unwrap();
    assert_eq!(budget.walks(), 1, "recent usage above 75% can be reused");
    let error = budget
        .reserve(&root.0, root.0.join("too-large"), 100 * MIB)
        .await
        .err()
        .unwrap();
    assert!(matches!(error.code, ErrorCode::RateLimited));
    assert_eq!(
        budget.walks(),
        2,
        "an exact scan must still enforce active reservations"
    );
    sized_file(&root.0.join("held"), 32 * MIB);
    drop(held);
    let error = budget
        .reserve(&root.0, root.0.join("too-large"), 100 * MIB)
        .await
        .err()
        .unwrap();
    assert!(matches!(error.code, ErrorCode::RateLimited));
    assert_eq!(
        budget.walks(),
        3,
        "released bytes still count before and after remeasurement"
    );
    assert_eq!(
        std::fs::metadata(root.0.join("retained")).unwrap().len(),
        900 * MIB
    );
}

#[tokio::test]
async fn old_near_quota_samples_rescan_and_detect_external_growth() {
    let root = Temp::new();
    std::fs::create_dir_all(&root.0).unwrap();
    sized_file(&root.0.join("retained"), 900 * MIB);
    let budget = gib_budget(&root);
    drop(budget.reserve(&root.0, root.0.join("a"), 10).await.unwrap());
    budget.age_samples();
    drop(budget.reserve(&root.0, root.0.join("b"), 10).await.unwrap());
    assert_eq!(
        budget.walks(),
        2,
        "near-quota relaxation expires after 30 seconds"
    );
    drop(budget.reserve(&root.0, root.0.join("c"), 10).await.unwrap());
    assert_eq!(
        budget.walks(),
        2,
        "following saves reuse the refreshed total"
    );
    sized_file(&root.0.join("external"), 124 * MIB);
    budget.age_samples();
    assert!(budget.reserve(&root.0, root.0.join("d"), 10).await.is_err());
    assert_eq!(budget.walks(), 3);
}

#[tokio::test]
async fn large_entry_count_reuses_a_recent_scan_but_full_directory_is_refused() {
    let root = Temp::new();
    std::fs::create_dir_all(&root.0).unwrap();
    for i in 0..6300 {
        std::fs::write(root.0.join(format!("entry-{i}")), b"").unwrap();
    }
    let budget = storage::StorageBudget::new(
        root.0.clone(),
        Limits {
            storage_entries: 8192,
            ..Default::default()
        },
    );
    drop(budget.reserve(&root.0, root.0.join("a"), 10).await.unwrap());
    drop(budget.reserve(&root.0, root.0.join("b"), 10).await.unwrap());
    assert_eq!(
        budget.walks(),
        1,
        "entry quotas must not retain the 75% cliff"
    );
    for i in 6300..8192 {
        std::fs::write(root.0.join(format!("entry-{i}")), b"").unwrap();
    }
    budget.age_samples();
    assert!(budget.reserve(&root.0, root.0.join("c"), 10).await.is_err());
    assert_eq!(budget.walks(), 2);
}
#[tokio::test]
async fn storage_readout_counts_linked_audio_once_and_separates_models() {
    let root = std::env::temp_dir().join(format!("writer-usage-{}", uuid::Uuid::new_v4()));
    let data = root.join("Data");
    let models = root.join("Models");
    let state = root.join("State");
    let id = uuid::Uuid::new_v4().to_string();
    let native = data.join("native-recordings").join(&id);
    let imported = data.join("recordings").join(&id);
    for dir in [&native, &imported, &models, &state] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(native.join("audio.wav"), vec![0; 1000]).unwrap();
    std::fs::hard_link(native.join("audio.wav"), imported.join("audio")).unwrap();
    std::fs::write(imported.join("recording.json"), b"metadata").unwrap();
    std::fs::write(models.join("model.bin"), vec![0; 2000]).unwrap();
    std::fs::write(state.join("settings.json"), b"keep").unwrap();
    assert_eq!(
        super::storage::usage(data, models, state).await.unwrap(),
        (1008, 1000, 2000, 4)
    );
    std::fs::remove_dir_all(root).unwrap();
}
