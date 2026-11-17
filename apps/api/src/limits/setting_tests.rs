use super::*;
use crate::{
    jobs::Pipeline,
    limits::{storage::StorageBudget, Limits},
    providers::SavedAudio,
    transcription::{ContractCatalog, RecordingMetadata},
};
use uuid::Uuid;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("writer-limit-setting-{}", Uuid::new_v4())))
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
    m
}
/// Sparse logical size: a real quota without writing GiB of zeros.
fn sized_file(path: &Path, bytes: u64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::File::create(path).unwrap().set_len(bytes).unwrap();
}
async fn native(root: &Path) -> SavedAudio {
    let audio = root
        .join("native-recordings")
        .join(format!("{}.wav", Uuid::new_v4()));
    tokio::fs::write(&audio, vec![3; 32]).await.unwrap();
    SavedAudio {
        metadata: metadata(),
        path: audio,
        prepared: None,
    }
}

#[tokio::test]
async fn a_saved_limit_survives_reopening_and_bad_files_read_as_none() {
    let state = Temp::new();
    let setting = LimitSetting::new(&state.0, false);
    assert_eq!(setting.load().await, None);
    setting.save(12 * GIB).await.unwrap();
    assert_eq!(
        LimitSetting::new(&state.0, false).load().await,
        Some(12 * GIB)
    );
    for refused in [0, GIB / 2, GIB + 1, (MAX_GIB + 1) * GIB] {
        assert!(setting.save(refused).await.is_err(), "{refused}");
    }
    assert_eq!(setting.load().await, Some(12 * GIB));
    std::fs::write(&setting.path, b"{\"device_storage_bytes\":5}").unwrap();
    assert_eq!(setting.load().await, None);
    std::fs::write(&setting.path, b"not json").unwrap();
    assert_eq!(setting.load().await, None);
}

#[tokio::test]
async fn a_new_limit_applies_to_the_next_import_and_after_a_restart() {
    let state = Temp::new();
    let data = Temp::new();
    sized_file(&data.0.join("native-recordings/older.wav"), GIB + GIB / 2);
    let pipeline = Pipeline::open(data.0.clone(), 1024 * 1024, vec![])
        .await
        .unwrap();
    pipeline
        .use_storage_setting(LimitSetting::new(&state.0, false))
        .await;
    let limit = pipeline.storage_limit();
    assert_eq!(limit.limit_bytes, Limits::default().device_storage_bytes);
    assert!(!limit.fixed_by_environment);

    pipeline.set_storage_limit(1).await.unwrap();
    let error = pipeline
        .recordings
        .import_native(native(&data.0).await)
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::StorageFull));
    // Raising it lets the refused dictation in at once, without a restart.
    let raised = pipeline.set_storage_limit(2).await.unwrap();
    assert_eq!(raised.limit_bytes, 2 * GIB);
    pipeline
        .recordings
        .import_native(native(&data.0).await)
        .await
        .unwrap();
    assert_eq!(pipeline.storage_budget().unwrap().limit_bytes, 2 * GIB);
    // Lowering it below usage reads as full and deletes nothing.
    pipeline.set_storage_limit(1).await.unwrap();
    let error = pipeline
        .recordings
        .import_native(native(&data.0).await)
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::StorageFull));
    assert_eq!(
        std::fs::metadata(data.0.join("native-recordings/older.wav"))
            .unwrap()
            .len(),
        GIB + GIB / 2
    );
    drop(pipeline);

    // A restart applies the saved choice.
    let reopened = Pipeline::open(data.0.clone(), 1024 * 1024, vec![])
        .await
        .unwrap();
    reopened
        .use_storage_setting(LimitSetting::new(&state.0, false))
        .await;
    assert_eq!(reopened.storage_limit().limit_bytes, GIB);
}

#[tokio::test]
async fn the_environment_wins_and_the_choice_is_read_only() {
    let state = Temp::new();
    let data = Temp::new();
    LimitSetting::new(&state.0, false)
        .save(50 * GIB)
        .await
        .unwrap();
    let pipeline = Pipeline::open(data.0.clone(), 1024 * 1024, vec![])
        .await
        .unwrap();
    pipeline
        .use_storage_setting(LimitSetting::new(&state.0, true))
        .await;
    let limit = pipeline.storage_limit();
    assert!(limit.fixed_by_environment);
    assert_eq!(limit.limit_bytes, Limits::default().device_storage_bytes);
    assert!(pipeline.set_storage_limit(10).await.is_err());
    assert_eq!(
        LimitSetting::new(&state.0, false).load().await,
        Some(50 * GIB)
    );
}

#[tokio::test]
async fn a_server_pipeline_has_no_desktop_choice() {
    let data = Temp::new();
    let pipeline = Pipeline::open(data.0.clone(), 1024 * 1024, vec![])
        .await
        .unwrap();
    assert!(pipeline.storage_limit().fixed_by_environment);
    assert!(pipeline.set_storage_limit(10).await.is_err());
}

#[tokio::test]
async fn on_the_desktop_the_aggregate_cap_follows_a_larger_device_limit() {
    let root = Temp::new();
    sized_file(&root.0.join("older.bin"), GIB + GIB / 2);
    let budget = StorageBudget::new(
        root.0.clone(),
        Limits {
            storage_bytes: GIB,
            device_storage_bytes: GIB,
            ..Limits::default()
        },
    );
    // A server keeps its aggregate cap whatever the device limit.
    budget.set_device_limit(2 * GIB, false);
    assert_eq!(budget.storage_limit(), GIB);
    let error = budget
        .reserve(&root.0, root.0.join("server"), 1024)
        .await
        .err()
        .unwrap();
    assert!(matches!(error.code, ErrorCode::StorageFull));
    budget.set_device_limit(2 * GIB, true);
    assert_eq!(budget.storage_limit(), 2 * GIB);
    drop(
        budget
            .reserve(&root.0, root.0.join("desktop"), 1024)
            .await
            .unwrap(),
    );
}

#[test]
fn free_space_of_an_existing_directory_is_known() {
    assert!(free_bytes(&std::env::temp_dir()).is_some_and(|bytes| bytes > 0));
}
