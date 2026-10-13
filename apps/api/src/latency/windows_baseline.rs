//! Explicit public saved-speech replay on a prepared isolated Windows GPU profile.
use crate::{
    jobs::{JobStore, Pipeline},
    providers::SavedAudio,
    transcription::*,
};
use std::{path::PathBuf, time::Duration};

#[tokio::test]
#[ignore = "Requires an isolated prepared Windows profile, public PCM16 WAV and verified GPU runtime"]
async fn gpu_stage_baseline() {
    assert_eq!(
        std::env::var("WRITER_GPU_PROBE_CONFIRMED").as_deref(),
        Ok("1")
    );
    let root = PathBuf::from(
        std::env::var_os("WRITER_STAGE_BASELINE_ROOT").expect("explicit isolated root"),
    );
    let sample = PathBuf::from(
        std::env::var_os("WRITER_STAGE_BASELINE_SAMPLE").expect("public test speech WAV"),
    );
    let setup = crate::desktop_settings::global().unwrap();
    assert!(
        !setup.first_run(),
        "Baseline settings must be a valid version-one envelope"
    );
    assert_eq!(setup.layout.data_dir, root.join("Data"));
    assert_eq!(setup.layout.state_dir, root.join("State"));
    assert_eq!(setup.layout.models_dir, root.join("Models"));
    assert!(
        !root.join("Data/jobs").exists(),
        "use a fresh isolated profile"
    );
    let mut options = setup.settings().transcription;
    assert!(matches!(options.selection.provider, ProviderId::Local));
    options.vocabulary = Vocabulary {
        hints: vec![],
        corrections: vec![],
        snippets: vec![],
    };
    options.cleanup = CleanupPolicy::Off {};
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            if let crate::local_stt::managed::WorkerState::Ready { backend, .. } =
                setup.runtime.worker()
            {
                assert_eq!(
                    backend, "vulkan",
                    "baseline must not silently fall back to CPU"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("GPU readiness failed: {:?}", setup.runtime.worker()));
    let providers = crate::local_stt::adapters_with(Some(&setup));
    let pipeline = Pipeline::open(root.join("Data"), 1_000_000_000, providers)
        .await
        .unwrap();
    let bytes = tokio::fs::read(&sample).await.unwrap();
    for index in 0..6 {
        let id = uuid::Uuid::new_v4();
        let metadata = RecordingMetadata {
            id,
            created_at_ms: crate::local_stt::managed::now_ms(),
            mime_type: "audio/wav".into(),
            byte_length: bytes.len() as u64,
            duration_ms: 0,
            input_language: options.input_language.clone(),
            capture_state: CaptureState::Complete,
        };
        pipeline
            .recordings
            .import_native(SavedAudio {
                metadata,
                path: sample.clone(),
                prepared: None,
            })
            .await
            .unwrap();
        // Replay starts at the simulated Stop; microphone/readiness and upload
        // are absent. Cold means first inference on this newly started worker,
        // with model loading completed before Stop; OS disk cache is uncontrolled.
        super::mark(id, "stop_accepted");
        let job = pipeline
            .jobs
            .submit(TranscriptionRequest {
                recording_id: id,
                idempotency_key: uuid::Uuid::new_v4(),
                options: options.clone(),
                retry_of_job_id: None,
            })
            .await
            .unwrap();
        pipeline.run_pending().await.unwrap();
        assert!(matches!(
            pipeline.jobs.get(job.id).await.unwrap().status,
            JobStatus::Succeeded { .. }
        ));
        super::measurements::set_warm(
            id,
            if index == 0 {
                super::measurements::Warm::Cold
            } else {
                super::measurements::Warm::Warm
            },
        );
        super::measurements::set_measurement(id, super::measurements::Measurement::PhysicalDevice);
        super::measurements::flush(id);
    }
    assert_eq!(
        super::measurements::read(&pipeline.jobs.latency_path())
            .records
            .len(),
        6
    );
    setup.runtime.select(None).unwrap();
}
