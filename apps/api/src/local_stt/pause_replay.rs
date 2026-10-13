//! Explicit same-build saved public speech replay; no microphone, clipboard or cloud.
use super::*;
use crate::{
    jobs::{JobStore, Pipeline},
    providers::PreparedAudio,
    transcription::*,
};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Manifest {
    output: PathBuf,
    baseline: bool,
    repeats: usize,
    samples: Vec<Sample>,
}
#[derive(Deserialize)]
struct Sample {
    id: String,
    wav: PathBuf,
    language: InputLanguage,
}

#[tokio::test]
#[ignore = "Requires an isolated Windows GPU profile and explicit public benchmark corpus manifest"]
async fn gpu_pause_reuse() {
    assert_eq!(
        std::env::var("WRITER_GPU_PROBE_CONFIRMED").as_deref(),
        Ok("1")
    );
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(std::env::var_os("WRITER_PAUSE_REPLAY_MANIFEST").unwrap()).unwrap(),
    )
    .unwrap();
    assert!(
        !manifest.output.exists(),
        "retain previous runs; choose a fresh output"
    );
    assert!((1..=6).contains(&manifest.repeats) && !manifest.samples.is_empty());
    let setup = crate::desktop_settings::global().unwrap();
    assert!(!setup.first_run());
    assert!(
        !setup.layout.data_dir.join("jobs").exists(),
        "use a fresh private profile"
    );
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            if let managed::WorkerState::Ready { backend, .. } = setup.runtime.worker() {
                assert_eq!(
                    backend, "vulkan",
                    "CPU inference is forbidden for this replay"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("verified GPU readiness");
    let mut provider = LocalProvider::managed(setup.runtime.clone());
    provider.preview_pause_reuse = !manifest.baseline;
    let provider = Arc::new(provider);
    assert_eq!(provider.readiness().await, Readiness::Ready);
    std::fs::create_dir_all(&manifest.output).unwrap();
    let pipeline = Pipeline::open(setup.layout.data_dir.clone(), 1_000_000_000, vec![])
        .await
        .unwrap();
    let mut receipts = Vec::new();
    for repeat in 0..manifest.repeats {
        for sample in &manifest.samples {
            let original = std::fs::read(&sample.wav).unwrap();
            let decoded = writer_dictation_audio::wav_pcm16(&original).expect("public PCM16 WAV");
            assert_eq!((decoded.rate, decoded.channels), (16_000, 1));
            let pcm = decoded.data.to_vec();
            assert!(
                pcm.len() < prepared::LIVE_WINDOW,
                "this gate exercises one open window"
            );
            let id = uuid::Uuid::new_v4();
            let root = manifest.output.join(id.to_string());
            std::fs::create_dir(&root).unwrap();
            let mut pcm = pcm;
            // Explicit 256ms digital pause before the first observation, then
            // 128ms more. These are constructed saved-audio derivatives, never
            // described as physical microphone samples or natural room tone.
            pcm.extend(vec![0; 8_192]);
            let prepared_path = root.join("prepared.pcm");
            std::fs::write(&prepared_path, &pcm).unwrap();
            let audio_path = root.join("audio.wav");
            let wave = mixed::wav(&pcm);
            std::fs::write(&audio_path, &wave).unwrap();
            let mut options = setup.settings().transcription;
            options.input_language = sample.language.clone();
            options.output_language = OutputLanguage::Preserve {};
            options.cleanup = CleanupPolicy::Off {};
            options.vocabulary = crate::vocabulary::empty_vocabulary();
            let mut audio = SavedAudio {
                path: audio_path,
                prepared: Some(PreparedAudio {
                    path: prepared_path.clone(),
                    byte_length: pcm.len() as u64,
                }),
                metadata: RecordingMetadata {
                    id,
                    created_at_ms: managed::now_ms(),
                    duration_ms: pcm.len() as u64 / 32,
                    byte_length: wave.len() as u64,
                    mime_type: "audio/wav".into(),
                    input_language: sample.language.clone(),
                    capture_state: CaptureState::Complete,
                },
            };
            let initial = Instant::now();
            let first = provider.preview(&audio, &options).await.unwrap();
            let first_preview_ms = initial.elapsed().as_millis() as u64;
            pcm.extend(vec![0; 4_096]);
            std::fs::write(&prepared_path, &pcm).unwrap();
            audio.prepared.as_mut().unwrap().byte_length = pcm.len() as u64;
            let wave = mixed::wav(&pcm);
            std::fs::write(&audio.path, &wave).unwrap();
            audio.metadata.byte_length = wave.len() as u64;
            let next = {
                let p = provider.clone();
                let a = audio.clone();
                let o = options.clone();
                tokio::spawn(async move {
                    let at = Instant::now();
                    let text = p.preview(&a, &o).await.unwrap();
                    (at.elapsed().as_millis() as u64, text)
                })
            };
            // Simulate Stop one native scheduler tick after the quiet extension.
            // Stop timing excludes the first completed preview and model startup.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let stop = Instant::now();
            crate::latency::mark(id, "stop_accepted");
            let transcript = provider.transcribe(&audio, &options).await.unwrap();
            let (extension_preview_ms, extension_text) = next.await.unwrap();
            // Job registration intentionally trims pre-job model diagnostics.
            // Preserve recognition proof before registering the durable job.
            let recognition_trace = crate::latency::snapshot(id);
            pipeline
                .recordings
                .import_native(audio.clone())
                .await
                .unwrap();
            let job = pipeline
                .jobs
                .submit(TranscriptionRequest {
                    recording_id: id,
                    idempotency_key: uuid::Uuid::new_v4(),
                    retry_of_job_id: None,
                    options,
                })
                .await
                .unwrap();
            pipeline
                .jobs
                .set_status(job.id, JobStatus::Transcribing {})
                .await
                .unwrap();
            pipeline
                .jobs
                .set_status(
                    job.id,
                    JobStatus::Succeeded {
                        result: TranscriptionResult {
                            transcript: transcript.clone(),
                            cleanup: CleanupResult::Off {},
                        },
                    },
                )
                .await
                .unwrap();
            crate::latency::mark(id, "raw_result_durable");
            let stop_to_raw_ms = stop.elapsed().as_millis() as u64;
            let trace = crate::latency::snapshot(id);
            let receipt = serde_json::json!({"id":sample.id,"repeat":repeat,"baseline":manifest.baseline,"warm":!receipts.is_empty(),"source_sha256":format!("{:x}",Sha256::digest(&original)),"derived_pcm_sha256":format!("{:x}",Sha256::digest(&pcm)),"first_preview_ms":first_preview_ms,"extension_preview_ms":extension_preview_ms,"stop_to_raw_ms":stop_to_raw_ms,"first_text":first,"extension_text":extension_text,"raw_text":transcript.raw_text,"stages":recognition_trace,"durable_stages":trace});
            receipts.push(receipt);
            std::fs::write(
                manifest.output.join("results.json"),
                serde_json::to_vec_pretty(&receipts).unwrap(),
            )
            .unwrap();
            println!(
                "Completed saved-speech pause replay {}/{}",
                receipts.len(),
                manifest.samples.len() * manifest.repeats
            );
        }
    }
    setup.runtime.select(None).unwrap();
}
