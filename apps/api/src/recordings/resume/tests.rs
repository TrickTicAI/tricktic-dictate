use super::*;
use crate::jobs::JobStore;
use crate::recordings::RecordingStore;
fn identity() -> Identity {
    let catalog: ContractCatalog = serde_json::from_str(include_str!(
        "../../../../../packages/contracts/examples.json"
    ))
    .unwrap();
    let mut metadata = catalog.recording.data.metadata;
    metadata.id = Uuid::new_v4();
    metadata.byte_length = 150000;
    Identity {
        metadata,
        sha256: format!("{:x}", Sha256::digest(vec![42; 150000])),
    }
}
#[tokio::test]
async fn confirmed_offsets_survive_disconnects_and_restart_with_one_finalize_and_job() {
    let root = std::env::temp_dir().join(format!("writer-resume-{}", Uuid::new_v4()));
    let source = identity();
    let id = source.metadata.id;
    let store = FileRecordingStore::open(root.clone(), 200000)
        .await
        .unwrap();
    let start = store.begin_upload(source.clone()).await.unwrap();
    // Lost start acknowledgement: exact same session, zero accepted bytes.
    assert_eq!(
        store.begin_upload(source.clone()).await.unwrap().session_id,
        start.session_id
    );
    let mid = store
        .upload_chunk(id, start.session_id, 0, &vec![42; 65536])
        .await
        .unwrap();
    // Lost middle acknowledgement: a duplicate offset cannot append again.
    assert!(store
        .upload_chunk(id, start.session_id, 0, &[42; 100])
        .await
        .is_err());
    assert_eq!(
        store.begin_upload(source.clone()).await.unwrap().offset,
        65536
    );
    // Simulate process death between audio sync and journal sync.
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(store.partial(id).join("audio"))
        .await
        .unwrap();
    file.write_all(&[99; 123]).await.unwrap();
    file.sync_all().await.unwrap();
    drop(file);
    drop(store);
    let store = FileRecordingStore::open(root.clone(), 200000)
        .await
        .unwrap();
    assert_eq!(
        store.begin_upload(source.clone()).await.unwrap().offset,
        mid.offset
    );
    store
        .upload_chunk(id, start.session_id, 65536, &vec![42; 150000 - 65536])
        .await
        .unwrap();
    assert!(
        store
            .finish_upload(id, start.session_id)
            .await
            .unwrap()
            .saved
    );
    // Lost final acknowledgement and process restart return the same committed receipt.
    drop(store);
    let store = FileRecordingStore::open(root.clone(), 200000)
        .await
        .unwrap();
    assert!(
        store
            .finish_upload(id, start.session_id)
            .await
            .unwrap()
            .saved
    );
    let audio = store.audio(id).await.unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(audio.path).await.unwrap())),
        source.sha256
    );
    assert_eq!(store.list().await.unwrap().len(), 1);
    drop(store);
    // Existing Writer submission idempotency remains independent of ingest.
    let pipeline = crate::jobs::Pipeline::open(root.clone(), 200000, vec![])
        .await
        .unwrap();
    let catalog: ContractCatalog = serde_json::from_str(include_str!(
        "../../../../../packages/contracts/examples.json"
    ))
    .unwrap();
    // Pipeline stores recordings under recordings/, import fixture bytes there through the same protocol.
    let cp = pipeline
        .recordings
        .begin_upload(source.clone())
        .await
        .unwrap();
    pipeline
        .recordings
        .upload_chunk(id, cp.session_id, 0, &vec![42; 150000])
        .await
        .unwrap();
    pipeline
        .recordings
        .finish_upload(id, cp.session_id)
        .await
        .unwrap();
    let mut request = catalog.transcription_request.data;
    request.recording_id = id;
    let first = pipeline.jobs.submit(request.clone()).await.unwrap();
    assert_eq!(pipeline.jobs.submit(request).await.unwrap().id, first.id);
    assert_eq!(pipeline.jobs.recoverable().await.unwrap().len(), 1);
    drop(pipeline);
    fs::remove_dir_all(root).await.unwrap();
}
#[tokio::test]
async fn expired_changed_foreign_cancelled_and_corrupt_checkpoints_fail_safely() {
    let root = std::env::temp_dir().join(format!("writer-resume-{}", Uuid::new_v4()));
    let store = FileRecordingStore::open(root.clone(), 200000)
        .await
        .unwrap();
    let mut source = identity();
    let id = source.metadata.id;
    let start = store.begin_upload(source.clone()).await.unwrap();
    store
        .upload_chunk(id, start.session_id, 0, &[42; 100])
        .await
        .unwrap();
    assert!(store
        .upload_chunk(id, Uuid::new_v4(), 100, &[42])
        .await
        .is_err());
    let other = FileRecordingStore::open(root.join("other-account"), 200000)
        .await
        .unwrap();
    assert!(other
        .upload_chunk(id, start.session_id, 100, &[42])
        .await
        .is_err());
    source.sha256 = format!("{:x}", Sha256::digest(vec![43; 150000]));
    let changed = store.begin_upload(source.clone()).await.unwrap();
    assert_eq!(changed.offset, 0);
    assert_ne!(changed.session_id, start.session_id);
    let path = store.partial(id).join("upload.json");
    let mut journal: Journal = read_json(&path).await.unwrap();
    journal.expires_at_ms = 0;
    write_json(&path, &journal).await.unwrap();
    assert!(store
        .upload_chunk(id, changed.session_id, 0, &[43])
        .await
        .is_err());
    let fresh = store.begin_upload(source.clone()).await.unwrap();
    assert_ne!(fresh.session_id, changed.session_id);
    store
        .upload_chunk(id, fresh.session_id, 0, &vec![42; 150000])
        .await
        .unwrap();
    assert!(store.finish_upload(id, fresh.session_id).await.is_err());
    assert!(store.audio(id).await.is_err());
    // Explicit cancellation/deletion makes the session unusable, never claims success.
    store.delete(id).await.unwrap();
    assert!(store.finish_upload(id, fresh.session_id).await.is_err());
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn http_disconnect_never_acknowledges_partial_chunk_and_finish_requires_full_hash() {
    use axum::{
        body::{Body, Bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let root = std::env::temp_dir().join(format!("writer-resume-http-{}", Uuid::new_v4()));
    let pipeline = crate::jobs::Pipeline::open(root.clone(), 200000, vec![])
        .await
        .unwrap();
    let router = crate::recordings::routes_with_pipeline(Some(pipeline.clone()));
    let source = identity();
    let path = format!("/api/v1/recordings/{}/upload", source.metadata.id);
    let start = router
        .clone()
        .oneshot(
            Request::post(&path)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&Envelope::new(source.clone())).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    let cp: Envelope<Checkpoint> =
        serde_json::from_slice(&to_bytes(start.into_body(), 10000).await.unwrap()).unwrap();
    let chunks = vec![
        Ok(Bytes::from(vec![42; 99])),
        Err(std::io::Error::other("injected disconnect")),
    ];
    let partial = router
        .clone()
        .oneshot(
            Request::patch(&path)
                .header("upload-session", cp.data.session_id.to_string())
                .header("upload-offset", "0")
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(partial.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        pipeline
            .recordings
            .begin_upload(source.clone())
            .await
            .unwrap()
            .offset,
        0
    );
    let chunk = router
        .clone()
        .oneshot(
            Request::patch(&path)
                .header("upload-session", cp.data.session_id.to_string())
                .header("upload-offset", "0")
                .body(Body::from(vec![42; 150000]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chunk.status(), StatusCode::OK);
    // Old acknowledged session cannot be used under a foreign token.
    let invalid = router
        .clone()
        .oneshot(
            Request::post(format!("{path}/finish"))
                .header("upload-session", Uuid::new_v4().to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::CONFLICT);
    for _ in 0..2 {
        let finished = router
            .clone()
            .oneshot(
                Request::post(format!("{path}/finish"))
                    .header("upload-session", cp.data.session_id.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(finished.status(), StatusCode::OK);
    }
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(
                fs::read(
                    pipeline
                        .recordings
                        .audio(source.metadata.id)
                        .await
                        .unwrap()
                        .path
                )
                .await
                .unwrap()
            )
        ),
        source.sha256
    );
    drop(router);
    drop(pipeline);
    fs::remove_dir_all(root).await.unwrap();
}
