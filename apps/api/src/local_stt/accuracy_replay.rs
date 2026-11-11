//! Opt-in private replay through the real provider. Receipts never go to stdout.
use super::*;
use crate::providers::PreparedAudio;
use crate::transcription::{CaptureState, ContractCatalog, RecordingMetadata};

#[derive(Deserialize)]
struct Manifest {
    output: PathBuf,
    inference_url: String,
    model_path: PathBuf,
    ffmpeg: PathBuf,
    model: String,
    mode: String,
    samples: Vec<Sample>,
}
#[derive(Deserialize)]
struct Sample {
    id: String,
    pcm: PathBuf,
    hints: Vec<String>,
    language: InputLanguage,
}

#[tokio::test]
#[ignore = "Requires private saved-audio manifest and an isolated verified Windows GPU worker"]
async fn dictation_accuracy_replay() {
    assert_eq!(
        std::env::var("WRITER_GPU_PROBE_CONFIRMED").as_deref(),
        Ok("1")
    );
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(std::env::var_os("WRITER_ACCURACY_MANIFEST").unwrap()).unwrap(),
    )
    .unwrap();
    assert!(matches!(manifest.mode.as_str(), "live" | "batch" | "final"));
    assert!(!manifest.output.exists(), "use a fresh output directory");
    std::fs::create_dir_all(&manifest.output).unwrap();
    let config = LocalConfig {
        inference_url: manifest.inference_url,
        worker_auth: None, // owned loopback proxy authenticates its private worker
        model: manifest.model.clone(),
        model_path: manifest.model_path,
        ffmpeg: manifest.ffmpeg,
        timeout: Duration::from_secs(180),
        audio_limit: 512 * 1024 * 1024,
    };
    let endpoint = Url::parse(&config.inference_url).unwrap();
    assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
    assert_eq!(endpoint.scheme(), "http");
    let mut provider = LocalProvider::new(None).unwrap();
    provider.source = Source::External(config.clone());
    let provider = Arc::new(provider);
    assert_eq!(provider.readiness().await, Readiness::Ready);
    let fixture: ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let mut receipts = Vec::new();
    for sample in manifest.samples {
        let pcm = std::fs::read(&sample.pcm).unwrap();
        assert!(!pcm.is_empty() && pcm.len().is_multiple_of(2));
        let mut options = fixture.transcription_request.data.options.clone();
        options.selection.provider = ProviderId::Local;
        options.selection.model = manifest.model.clone();
        options.input_language = sample.language.clone();
        options.output_language = OutputLanguage::Preserve {};
        options.vocabulary = crate::vocabulary::empty_vocabulary();
        options.vocabulary.hints = sample.hints;
        let id = uuid::Uuid::new_v4();
        let audio = SavedAudio {
            path: sample.pcm.clone(),
            prepared: Some(PreparedAudio {
                path: sample.pcm,
                byte_length: pcm.len() as u64,
            }),
            metadata: RecordingMetadata {
                id,
                created_at_ms: 0,
                duration_ms: pcm.len() as u64 / 32,
                byte_length: pcm.len() as u64,
                mime_type: "audio/wav".into(),
                input_language: sample.language,
                capture_state: CaptureState::Complete,
            },
        };
        let mut preview: Option<tokio::task::JoinHandle<()>> = None;
        if manifest.mode == "live" {
            let started = Instant::now();
            // Real-time 50ms journal ticks; one speculative call at a time.
            for covered in (1600..pcm.len()).step_by(1600) {
                tokio::time::sleep_until(tokio::time::Instant::from_std(
                    started + Duration::from_secs_f64(covered as f64 / 32000.0),
                ))
                .await;
                if covered >= 16000 && preview.as_ref().is_none_or(|p| p.is_finished()) {
                    if let Some(done) = preview.take() {
                        done.await.unwrap();
                    }
                    let mut snapshot = audio.clone();
                    snapshot.prepared.as_mut().unwrap().byte_length = covered as u64;
                    let p = provider.clone();
                    let o = options.clone();
                    preview = Some(tokio::spawn(async move {
                        let _ = p.preview(&snapshot, &o).await;
                    }));
                }
            }
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                started + Duration::from_secs_f64(pcm.len() as f64 / 32000.0),
            ))
            .await;
        }
        let stop = Instant::now();
        let text = if manifest.mode != "batch" {
            provider
                .transcribe(&audio, &options)
                .await
                .unwrap()
                .raw_text
        } else {
            provider
                .request(
                    &config,
                    mixed::wav(&pcm),
                    &options,
                    false,
                    whisper_profile(&config.model, pcm.len() + 44),
                )
                .await
                .unwrap()
        };
        let stop_to_raw_ms = stop.elapsed().as_millis() as u64;
        if let Some(done) = preview {
            done.await.unwrap();
        }
        receipts.push(serde_json::json!({
            "id":sample.id, "raw_text":text, "stop_to_raw_ms":stop_to_raw_ms,
            "speechless":speech::speechless(&pcm), "repetition":repetition::find(&text).is_some(),
            "stages":crate::latency::snapshot(id),
        }));
        std::fs::write(
            manifest.output.join("results.json"),
            serde_json::to_vec(&receipts).unwrap(),
        )
        .unwrap();
        println!("Completed private replay sample {}", receipts.len());
    }
}
