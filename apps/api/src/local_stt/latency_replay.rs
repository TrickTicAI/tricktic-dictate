//! Opt-in replay on a separately owned Windows GPU worker; no microphone or clipboard.
use super::*;
use crate::{
    cleanup::modes::{ModeService, ModeSettings},
    jobs::{JobStore, Pipeline},
};
use std::io::Write;
#[derive(serde::Deserialize)]
struct Manifest {
    output: PathBuf,
    inference_url: String,
    model_path: PathBuf,
    ffmpeg: PathBuf,
    samples: Vec<Sample>,
    #[serde(default)]
    early_only: bool,
}
#[derive(serde::Deserialize)]
struct Sample {
    name: String,
    pcm: PathBuf,
    options: TranscriptionOptions,
}
#[tokio::test]
#[ignore = "Explicit saved-audio replay on an owned Windows Vulkan worker with Gemini key; see super-dictation.md"]
async fn saved_audio_super_latency() {
    assert_eq!(
        std::env::var("WRITER_GPU_PROBE_CONFIRMED").as_deref(),
        Ok("1")
    );
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(std::env::var_os("WRITER_LATENCY_MANIFEST").unwrap()).unwrap(),
    )
    .unwrap();
    assert!(!manifest.output.exists(), "Use a fresh output directory");
    let endpoint = Url::parse(&manifest.inference_url).unwrap();
    assert_eq!(endpoint.scheme(), "http");
    assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
    assert_eq!(endpoint.path(), "/inference");
    std::fs::create_dir_all(&manifest.output).unwrap();
    let mut receipts = vec![];
    for sample in &manifest.samples {
        assert!(sample
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
        let pcm = std::fs::read(&sample.pcm).unwrap();
        assert!(pcm.len().is_multiple_of(2) && !pcm.is_empty());
        for early in if manifest.early_only {
            vec![true]
        } else {
            vec![false, true]
        } {
            let root = manifest.output.join(format!("{}-{early}", sample.name));
            std::fs::create_dir_all(&root).unwrap();
            // Test-only trusted worker configuration. Production managed runtime
            // verifies actual GPU/model identity before serving real recordings.
            let mut provider = LocalProvider::new(None).unwrap();
            provider.source = Source::External(LocalConfig {
                worker_auth: None,
                inference_url: manifest.inference_url.clone(),
                model: sample.options.selection.model.clone(),
                model_path: manifest.model_path.clone(),
                ffmpeg: manifest.ffmpeg.clone(),
                timeout: Duration::from_secs(90),
                audio_limit: 128 * 1024 * 1024,
            });
            let provider = Arc::new(provider);
            assert_eq!(provider.readiness().await, Readiness::Ready);
            let pipeline = Pipeline::open(root.join("Data"), 128 * 1024 * 1024, vec![])
                .await
                .unwrap();
            let modes = ModeService::open(root.join("modes"), pipeline.jobs.clone())
                .await
                .unwrap();
            let mut profile = ModeSettings::default().profiles[1].clone();
            profile.process_while_speaking = early;
            profile.text_model = Some(crate::transcription::ModelSelection {
                provider: ProviderId::Gemini,
                model: "gemini-3.8-flash".into(),
            });
            let id = uuid::Uuid::new_v4();
            modes
                .bind_recording(id, profile, Default::default())
                .await
                .unwrap();
            let wave = mixed::wav(&pcm);
            let wave_path = root.join("audio.wav");
            std::fs::write(&wave_path, &wave).unwrap();
            let mut audio = SavedAudio {
                path: wave_path,
                prepared: None,
                metadata: crate::transcription::RecordingMetadata {
                    id,
                    created_at_ms: crate::local_stt::managed::now_ms(),
                    duration_ms: pcm.len() as u64 / 32,
                    byte_length: wave.len() as u64,
                    mime_type: "audio/wav".into(),
                    input_language: sample.options.input_language.clone(),
                    capture_state: crate::transcription::CaptureState::Complete,
                },
            };
            pipeline
                .recordings
                .import_native(audio.clone())
                .await
                .unwrap();
            let prepared_path = root.join("prepared.pcm");
            let mut file = std::fs::File::create(&prepared_path).unwrap();
            let mut covered = 0;
            let snapshots = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
            let mut preview: Option<tokio::task::JoinHandle<()>> = None;
            let started = Instant::now();
            crate::latency::mark(id, "capture_started");
            for bytes in pcm.chunks(1600) {
                file.write_all(bytes).unwrap();
                file.flush().unwrap();
                covered += bytes.len();
                if covered >= 16000 && preview.as_ref().is_none_or(|p| p.is_finished()) {
                    if let Some(previous) = preview.take() {
                        previous.await.unwrap();
                    }
                    let mut snapshot = audio.clone();
                    snapshot.prepared = Some(crate::providers::PreparedAudio {
                        path: prepared_path.clone(),
                        byte_length: covered as u64,
                    });
                    let snapshots = snapshots.clone();
                    let provider = provider.clone();
                    let modes = modes.clone();
                    let options = sample.options.clone();
                    preview = Some(tokio::spawn(async move {
                        if let Ok(text) = provider.preview(&snapshot, &options).await {
                            snapshots.lock().unwrap().push(serde_json::json!({"covered": snapshot.prepared.as_ref().unwrap().byte_length, "text":text}));
                            modes.prepare_draft(id, text, options.vocabulary).await;
                        }
                    }));
                }
                tokio::time::sleep_until(tokio::time::Instant::from_std(
                    started + Duration::from_secs_f64(covered as f64 / 32000.0),
                ))
                .await;
            }
            let stop = Instant::now();
            crate::latency::mark(id, "stop_accepted");
            drop(file);
            audio.prepared = Some(crate::providers::PreparedAudio {
                path: prepared_path,
                byte_length: pcm.len() as u64,
            });
            modes.release_draft(id).await;
            let raw = provider.transcribe(&audio, &sample.options).await.unwrap();
            let raw_ms = stop.elapsed().as_millis() as u64;
            let job = pipeline
                .jobs
                .submit(crate::transcription::TranscriptionRequest {
                    recording_id: id,
                    idempotency_key: uuid::Uuid::new_v4(),
                    retry_of_job_id: None,
                    options: sample.options.clone(),
                })
                .await
                .unwrap();
            pipeline
                .jobs
                .set_status(job.id, crate::transcription::JobStatus::Transcribing {})
                .await
                .unwrap();
            pipeline
                .jobs
                .set_status(
                    job.id,
                    crate::transcription::JobStatus::Succeeded {
                        result: crate::transcription::TranscriptionResult {
                            transcript: raw.clone(),
                            cleanup: crate::transcription::CleanupResult::Off {},
                        },
                    },
                )
                .await
                .unwrap();
            crate::latency::mark(id, "raw_result_durable");
            let text = modes.delivery_text(job.id).await.unwrap();
            let total_ms = stop.elapsed().as_millis() as u64;
            if let Some(preview) = preview {
                preview.await.unwrap();
            }
            let attempt = modes
                .live_processing(job.id)
                .await
                .unwrap()
                .attempt
                .unwrap();
            std::fs::write(
                root.join("previews.json"),
                serde_json::to_vec_pretty(&*snapshots.lock().unwrap()).unwrap(),
            )
            .unwrap();
            let receipt = serde_json::json!({"sample":sample.name,"early":early,"stop_to_raw_ms":raw_ms,"stop_to_text_ms":total_ms,"raw_text":raw.raw_text,"text":text,"diagnostics":attempt.diagnostics,"stages":crate::latency::snapshot(id)});
            println!(
                "{}",
                serde_json::json!({"sample":sample.name,"early":early,"raw_ms":raw_ms,"total_ms":total_ms,"reused":receipt["diagnostics"]["reused_draft"],"miss":receipt["stages"]["details"]["mode_draft_miss"]})
            );
            receipts.push(receipt);
            std::fs::write(
                manifest.output.join("results.json"),
                serde_json::to_vec_pretty(&receipts).unwrap(),
            )
            .unwrap();
        }
    }
}
