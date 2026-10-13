//! Explicit Windows acceptance probe: retained public PCM -> real journal ->
//! durable job -> production guarded paste -> readback from a dedicated fixture.
//! Never targets a terminal and never sends Enter. Run against an owned worker.
#![allow(dead_code)]
#[macro_use]
#[path = "../src/product.rs"]
mod product;
#[cfg(windows)]
#[path = "../src/shell/clipboard_preservation.rs"]
mod clipboard_preservation;
#[cfg(windows)]
#[path = "../src/shell/clipboard_rendering.rs"]
mod clipboard_rendering;
#[cfg(windows)]
#[path = "../src/shell/input_guard.rs"]
mod input_guard;
#[path = "../src/storage.rs"]
mod storage;
#[cfg(windows)]
#[path = "../src/shell/win_input.rs"]
mod win_input;
#[cfg(windows)]
#[path = "../src/shell/win_target.rs"]
mod win_target;
#[cfg(windows)]
#[path = "../src/shell/windows_delivery.rs"]
mod windows_delivery;
mod delivery {
    pub struct Outcome {
        pub copied: bool,
        pub pasted: bool,
        pub message: String,
    }
    #[derive(Clone, Copy)]
    pub struct Target {
        pub window: isize,
        pub process: u32,
        pub created: u64,
    }
    /// Mirrors `shell::delivery::Guards`. MW-50 moved this out of
    /// `windows_delivery.rs`, so the probe's stub module has to carry it too.
    pub struct Guards<'a> {
        pub wanted: &'a dyn Fn() -> bool,
        pub superseded: &'a dyn Fn() -> bool,
        pub recording_newer: &'a dyn Fn() -> bool,
        pub waiting: &'a dyn Fn(),
        pub target_valid: &'a dyn Fn() -> bool,
        pub mark: &'a dyn Fn(&'static str),
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Run this acceptance probe on Windows.");
}
#[cfg(windows)]
#[tokio::main]
async fn main() {
    use metrix_writer_api::{
        jobs::{JobStore, Pipeline},
        local_stt::{LocalConfig, LocalProvider, Readiness},
        providers::{PreparedAudio, SavedAudio, TranscriptionProvider},
        transcription::*,
    };
    use std::{
        path::PathBuf,
        sync::Arc,
        time::{Duration, Instant},
    };
    #[link(name = "user32")]
    extern "system" {
        fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: isize,
            menu: isize,
            instance: isize,
            param: *const u8,
        ) -> isize;
        fn DestroyWindow(hwnd: isize) -> i32;
        fn SendMessageW(hwnd: isize, message: u32, w: usize, l: isize) -> isize;
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert!(
        args.len() == 5 || args.len() == 8,
        "wav fresh-root inference-url model-path ffmpeg-path [target-hwnd edit-hwnd target-pid]"
    );
    let root = PathBuf::from(&args[1]);
    assert!(!root.exists(), "Use a fresh isolated output directory");
    let provider = Arc::new(
        LocalProvider::new(Some(LocalConfig {
            worker_auth: None,
            inference_url: args[2].clone(),
            model: "ivrit-turbo".into(),
            model_path: args[3].clone().into(),
            ffmpeg: args[4].clone().into(),
            timeout: Duration::from_secs(60),
            audio_limit: 64 * 1024 * 1024,
        }))
        .unwrap(),
    );
    assert_eq!(provider.readiness().await, Readiness::Ready);
    let pipeline = Pipeline::open(
        root.join("Data"),
        1024 * 1024 * 1024,
        vec![provider.clone()],
    )
    .await
    .unwrap();
    let store = storage::Store::open(root.join("native")).unwrap();
    let input = std::fs::read(&args[0]).unwrap();
    let wav = writer_dictation_audio::wav_pcm16(&input).unwrap();
    let id = uuid::Uuid::new_v4();
    let mut journal = store
        .create(id, "he".into(), wav.rate, wav.channels)
        .unwrap();
    let catalog: ContractCatalog =
        serde_json::from_str(include_str!("../../../../packages/contracts/examples.json")).unwrap();
    let mut request = catalog.transcription_request.data;
    request.recording_id = id;
    request.idempotency_key = uuid::Uuid::new_v4();
    request.retry_of_job_id = None;
    request.options.selection.provider = ProviderId::Local;
    request.options.selection.model = "ivrit-turbo".into();
    request.options.input_language = InputLanguage::He;
    request.options.output_language = OutputLanguage::Preserve {};
    request.options.cleanup = CleanupPolicy::Off {};
    // Contract examples contain a vocabulary hint; public corpus benchmarking
    // must use the same empty prompt as the worker-only comparison.
    request.options.vocabulary = Vocabulary {
        hints: vec![],
        corrections: vec![],
        snippets: vec![],
    };
    metrix_writer_api::latency::mark(id, "capture_started");
    let mut preview_task: Option<tokio::task::JoinHandle<()>> = None;
    let completed_previews = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut preview_requests = 0;
    let frames = (wav.rate as usize / 50) * usize::from(wav.channels);
    let playback_started = Instant::now();
    let mut fed_samples = 0;
    for (index, block) in wav.data.chunks(frames * 2).enumerate() {
        let samples: Vec<_> = block
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        journal.append(&samples).unwrap();
        if (index + 1) % 50 == 0 {
            journal.commit().unwrap();
        }
        if (index + 1) % 5 == 0
            && (index + 1) >= 25
            && preview_task.as_ref().is_none_or(|task| task.is_finished())
        {
            if let Some(task) = preview_task.take() {
                task.await.unwrap();
            }
            let (live, path, byte_length) = journal.preview().unwrap();
            let audio = SavedAudio {
                metadata: RecordingMetadata {
                    id,
                    created_at_ms: journal.metadata.created_at_ms,
                    input_language: InputLanguage::He,
                    duration_ms: live.duration_ms,
                    byte_length: live.byte_length,
                    mime_type: "audio/wav".into(),
                    capture_state: CaptureState::Capturing,
                },
                path: store.path(id).join("audio.wav"),
                prepared: Some(PreparedAudio { path, byte_length }),
            };
            let provider = provider.clone();
            let options = request.options.clone();
            let completed = completed_previews.clone();
            preview_requests += 1;
            preview_task = Some(tokio::spawn(async move {
                if provider
                    .preview(&audio, &options)
                    .await
                    .is_ok_and(|text| !text.is_empty())
                {
                    completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }));
        }
        // Account for append/checkpoint time instead of adding 20ms after it.
        fed_samples += block.len() / 2;
        let due = playback_started
            + Duration::from_secs_f64(
                fed_samples as f64 / f64::from(wav.rate) / f64::from(wav.channels),
            );
        tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
    }
    let stop = Instant::now();
    metrix_writer_api::latency::mark(id, "stop_accepted");
    journal.finish(false).unwrap();
    metrix_writer_api::latency::mark(id, "audio_durable");
    let metadata = RecordingMetadata {
        id,
        created_at_ms: journal.metadata.created_at_ms,
        input_language: InputLanguage::He,
        duration_ms: journal.metadata.duration_ms,
        byte_length: journal.metadata.byte_length,
        mime_type: "audio/wav".into(),
        capture_state: CaptureState::Complete,
    };
    drop(journal);
    let native_metadata = store.get(id, false).unwrap().unwrap();
    let (path, byte_length) = store.prepared(id, &native_metadata).unwrap();
    pipeline
        .recordings
        .import_native(SavedAudio {
            metadata,
            path: store.path(id).join("audio.wav"),
            prepared: Some(PreparedAudio { path, byte_length }),
        })
        .await
        .unwrap();
    metrix_writer_api::latency::mark(id, "audio_imported");
    let imported_ms = stop.elapsed().as_secs_f64() * 1000.;
    let job = pipeline.jobs.submit(request).await.unwrap();
    pipeline.run_pending().await.unwrap();
    let job = pipeline.jobs.get(job.id).await.unwrap();
    let JobStatus::Succeeded { result } = job.status else {
        panic!("Inference did not succeed")
    };
    let text = result.transcript.raw_text;
    assert!(!text.trim().is_empty());
    let transcript_ms = stop.elapsed().as_secs_f64() * 1000.;
    let retained = std::fs::read(store.path(id).join("audio.wav")).unwrap();
    let retained_pcm = writer_dictation_audio::wav_pcm16(&retained).unwrap();
    assert_eq!(
        retained_pcm.data, wav.data,
        "The complete original PCM must survive inference"
    );
    if let Some(task) = preview_task {
        task.await.unwrap();
    }
    let completed_previews = completed_previews.load(std::sync::atomic::Ordering::Relaxed);
    if args.len() == 5 {
        let receipt = serde_json::json!({"import_ms": imported_ms, "stop_to_transcript_ms": transcript_ms, "stages": metrix_writer_api::latency::snapshot(id),
            "duration_ms": native_metadata.duration_ms, "preview_requests": preview_requests,
            "previews_with_text": completed_previews, "insertion_tested": false, "original_pcm_preserved": true, "source_paced": true,
            "scope": "native journal, live previews, registration and durable final job; no physical microphone or WebView"});
        storage::atomic_json(&root.join("timing.json"), &receipt).unwrap();
        println!("{}", receipt);
        return;
    }
    let target: isize = args[5].parse().unwrap();
    let edit: isize = args[6].parse().unwrap();
    let process: u32 = args[7].parse().unwrap();
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let owner = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            1,
            1,
            0,
            0,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(owner, 0);
    let outcome = windows_delivery::windows_deliver(
        owner,
        Some(delivery::Target {
            window: target,
            created: win_target::created(process).expect("live fixture process"),
            process,
        }),
        &text,
        false,
        false,
        &delivery::Guards {
            wanted: &|| true,
            superseded: &|| false,
            recording_newer: &|| false,
            waiting: &|| {},
            target_valid: &|| true,
            mark: &|stage| metrix_writer_api::latency::mark(id, stage),
        },
    )
    .unwrap();
    assert!(outcome.pasted, "{}", outcome.message);
    let mut actual = vec![0u16; text.encode_utf16().count() + 32];
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut readback = String::new();
    let inserted = loop {
        let count =
            unsafe { SendMessageW(edit, 0x000D, actual.len(), actual.as_mut_ptr() as isize) }
                as usize;
        readback.clear();
        readback.push_str(&String::from_utf16_lossy(&actual[..count]));
        if readback == text {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    std::fs::write(root.join("field.txt"), &readback).unwrap();
    let receipt = serde_json::json!({"import_ms":imported_ms,"stages": metrix_writer_api::latency::snapshot(id),"stop_to_transcript_ms":transcript_ms,"stop_to_insertion_ms":stop.elapsed().as_secs_f64()*1000.,"inserted_exact_text":inserted,"newline_normalized_match":readback.replace("\r\n","\n")==text.replace("\r\n","\n"),"trimmed_match":readback.trim()==text.trim(),"copied":outcome.copied,"paste_sent":outcome.pasted,"duration_ms":native_metadata.duration_ms,"model":"ivrit-turbo","preview_requests":preview_requests,"previews_with_text":completed_previews,"scope":"journal, registration, durable job, worker, guarded paste, native field readback; excludes WebView command/event scheduling and physical microphone"});
    storage::atomic_json(&root.join("timing.json"), &receipt).unwrap();
    println!("{}", receipt);
    unsafe {
        DestroyWindow(owner);
    }
    assert!(
        inserted,
        "Dedicated native fixture did not receive the exact transcript"
    );
}
