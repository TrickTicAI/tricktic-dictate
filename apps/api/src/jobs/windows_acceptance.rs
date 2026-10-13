//! Explicit native acceptance against an isolated prepared profile and synthetic
//! speech or explicitly selected retained audio. Never run with the installed
//! user's Data/State directories.
use super::*;
use crate::{providers::*, recordings::RecordingStore, transcription::*};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

struct RetryOnce {
    real: Arc<dyn TranscriptionProvider>,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl TranscriptionProvider for RetryOnce {
    fn info(&self) -> ProviderInfo {
        self.real.info()
    }
    async fn transcribe(
        &self,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(crate::recordings::disk::error(
                ErrorCode::ProviderTimeout,
                "Injected acceptance timeout before inference",
                true,
            ));
        }
        self.real.transcribe(audio, options).await
    }
}

#[tokio::test]
#[ignore = "Requires explicit isolated Windows profile, verified GPU model/runtime and PCM16 speech"]
async fn gpu_saved_audio_recovers_while_two_text_slots_are_occupied() {
    let root = PathBuf::from(
        std::env::var_os("WRITER_RECOVERY_ACCEPTANCE_ROOT").expect("explicit isolated root"),
    );
    let sample = PathBuf::from(
        std::env::var_os("WRITER_RECOVERY_ACCEPTANCE_SAMPLE").expect("explicit speech WAV"),
    );
    let layout = crate::desktop_settings::Layout::from_env().unwrap();
    assert_eq!(layout.data_dir, root.join("Data"));
    assert_eq!(layout.state_dir, root.join("State"));
    assert_eq!(layout.models_dir, root.join("Models"));
    assert!(
        !root.join("Data/jobs").exists(),
        "use a fresh prepared acceptance profile"
    );
    let setup = crate::desktop_settings::global().unwrap();
    let mut options = setup.settings().transcription;
    assert!(matches!(options.selection.provider, ProviderId::Local));
    options.vocabulary = Vocabulary {
        hints: vec![],
        corrections: vec![],
        snippets: vec![],
    };
    options.cleanup = CleanupPolicy::Off {};
    let real = crate::local_stt::adapters_with(Some(&setup)).remove(0);
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            if let crate::local_stt::managed::WorkerState::Ready { backend, .. } =
                setup.runtime.worker()
            {
                assert_eq!(
                    backend, "vulkan",
                    "GPU acceptance must not silently use CPU"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    // Reproduce a history that the previous path-by-path scan considered full:
    // 9 MiB of retained audio has two names, against a 16 MiB data allowance.
    let native_history = root.join("Data/native-recordings/history");
    let api_history = root.join("Data/recordings/history");
    tokio::fs::create_dir_all(&native_history).await.unwrap();
    tokio::fs::create_dir_all(&api_history).await.unwrap();
    let retained = native_history.join("audio.wav");
    tokio::fs::write(&retained, vec![0; 9 * 1024 * 1024])
        .await
        .unwrap();
    tokio::fs::hard_link(&retained, api_history.join("audio"))
        .await
        .unwrap();
    let limits = crate::limits::Limits {
        storage_bytes: 16 * 1024 * 1024,
        device_storage_bytes: 16 * 1024 * 1024,
        ..Default::default()
    };
    let pipeline = Pipeline::open_shared(
        root.join("Data"),
        1_000_000_000,
        vec![Arc::new(RetryOnce {
            real,
            calls: calls.clone(),
        })],
        root.join("State/modes"),
        crate::limits::storage::StorageBudget::new(root.join("Data"), limits),
        None,
    )
    .await
    .unwrap();
    let bytes = tokio::fs::read(&sample).await.unwrap();
    let metadata = RecordingMetadata {
        id: Uuid::new_v4(),
        created_at_ms: super::disk::now(),
        mime_type: "audio/wav".into(),
        byte_length: bytes.len() as u64,
        duration_ms: 0,
        input_language: options.input_language.clone(),
        capture_state: CaptureState::Complete,
    };
    pipeline
        .recordings
        .import_native(SavedAudio {
            metadata: metadata.clone(),
            path: sample,
            prepared: None,
        })
        .await
        .unwrap();
    let primary = pipeline.work.admit().unwrap();
    let backup = pipeline.work.admit().unwrap();
    let job = pipeline
        .jobs
        .submit(TranscriptionRequest {
            recording_id: metadata.id,
            idempotency_key: Uuid::new_v4(),
            options,
            retry_of_job_id: None,
        })
        .await
        .unwrap();
    pipeline.run_pending().await.unwrap();
    assert!(matches!(
        pipeline.jobs.get(job.id).await.unwrap().status,
        JobStatus::Queued {}
    ));
    tokio::time::sleep(Duration::from_millis(2100)).await;
    pipeline.run_pending().await.unwrap();
    let completed = pipeline.jobs.get(job.id).await.unwrap();
    let JobStatus::Succeeded { result } = completed.status else {
        panic!("GPU recognition did not recover")
    };
    assert!(result.transcript.raw_text.split_whitespace().count() >= 5);
    assert_eq!(completed.recovery.unwrap().attempts_started, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    pipeline.run_pending().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        tokio::fs::read(pipeline.recordings.audio(metadata.id).await.unwrap().path)
            .await
            .unwrap(),
        bytes
    );
    drop((primary, backup));
    setup.runtime.select(None).unwrap();
    eprintln!("Native GPU recovery passed: native import above the old double-counted quota, two attempts, one saved result, retained audio, two occupied text slots.");
}
