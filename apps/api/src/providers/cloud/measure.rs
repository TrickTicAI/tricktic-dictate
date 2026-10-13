//! Opt-in synthetic transport measurement. Run alone with --ignored --test-threads=1.
use super::{tests::*, *};
use axum::{http::Uri, Router};
use http_body_util::BodyExt;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use tokio::net::TcpListener;

struct MeasuredAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn allocated(size: usize) {
    let live = LIVE.fetch_add(size, SeqCst) + size;
    PEAK.fetch_max(live, SeqCst);
}
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = System.alloc(layout);
        if !result.is_null() {
            allocated(layout.size());
        }
        result
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), SeqCst);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = System.realloc(ptr, layout, size);
        if !result.is_null() {
            if size >= layout.size() {
                allocated(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, SeqCst);
            }
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

#[tokio::test]
#[ignore = "isolated peak heap measurement, not a speech/model test"]
async fn transport_memory_before_after() {
    use axum::routing::any;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let requests = count.clone();
    let app = Router::new().fallback(any(move |uri: Uri, mut body: axum::body::Body| {
        let requests = requests.clone();
        async move {
            requests.fetch_add(1, SeqCst);
            while let Some(frame) = body.frame().await {
                frame.unwrap();
            }
            if uri.path().ends_with(":generateContent") {
                success(&ProviderId::Gemini)
            } else {
                success(&ProviderId::Openai)
            }
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for (size, normalized) in [(32_000, false), (14_000_000, false), (14_000_044, true)] {
        let mut audio = AudioFixture::new();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&audio.0.path)
            .unwrap()
            .set_len(size)
            .unwrap();
        audio.0.metadata.byte_length = size;
        if normalized {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open(&audio.0.path)
                .unwrap();
            let mut header = crate::local_stt::silence::wav(&[]);
            header[4..8].copy_from_slice(&((size - 8) as u32).to_le_bytes());
            header[40..44].copy_from_slice(&((size - 44) as u32).to_le_bytes());
            use std::io::Write;
            file.write_all(&header).unwrap();
            let block: Vec<_> = (0..16_000).flat_map(|_| 1000i16.to_le_bytes()).collect();
            let mut remaining = size - 44;
            while remaining > 0 {
                let count = remaining.min(block.len() as u64) as usize;
                file.write_all(&block[..count]).unwrap();
                remaining -= count as u64;
            }
        }
        for id in [ProviderId::Openai, ProviderId::Gemini] {
            let adapter = CloudProvider::new(
                id.clone(),
                Some("fixture".into()),
                endpoint.clone(),
                Duration::from_secs(30),
            );
            let opts = options(id.clone(), InputLanguage::Mixed);
            // Warm connections/runtime allocation before measuring both variants.
            adapter.transcribe(&audio.0, &opts).await.unwrap();
            for legacy in [true, false] {
                let base = LIVE.load(SeqCst);
                PEAK.store(base, SeqCst);
                count.store(0, SeqCst);
                if legacy {
                    // Exact original non-decodable-file buffering/body construction from 4eb3eae.
                    let bytes = if normalized {
                        let pcm =
                            crate::local_stt::pcm::convert(&audio.0.path, 256 * 1024 * 1024, true)
                                .await
                                .unwrap()
                                .unwrap();
                        crate::local_stt::silence::wav(&crate::local_stt::silence::prepare(
                            audio.0.metadata.id,
                            &pcm,
                        ))
                    } else {
                        let file = tokio::fs::File::open(&audio.0.path).await.unwrap();
                        let mut bytes = Vec::new();
                        file.take(size + 1).read_to_end(&mut bytes).await.unwrap();
                        bytes
                    };
                    let response = if matches!(id, ProviderId::Openai) {
                        let form = multipart::Form::new()
                            .part(
                                "file",
                                multipart::Part::bytes(bytes)
                                    .file_name("recording.wav")
                                    .mime_str("audio/wav")
                                    .unwrap(),
                            )
                            .text("model", opts.selection.model.clone())
                            .text("response_format", "json")
                            .text("prompt", preservation_prompt(&opts));
                        adapter
                            .client
                            .as_ref()
                            .unwrap()
                            .post(format!("{endpoint}/audio/transcriptions"))
                            .multipart(form)
                            .send()
                            .await
                            .unwrap()
                    } else {
                        let body = serde_json::to_vec(&upload::gemini_body(&opts, json!({"inlineData":{"mimeType":"audio/wav","data":STANDARD.encode(bytes)}}))).unwrap();
                        adapter
                            .client
                            .as_ref()
                            .unwrap()
                            .post(format!(
                                "{endpoint}/models/{}:generateContent",
                                opts.selection.model
                            ))
                            .header("content-type", "application/json")
                            .body(body)
                            .send()
                            .await
                            .unwrap()
                    };
                    response_json(response).await.unwrap();
                } else {
                    adapter.transcribe(&audio.0, &opts).await.unwrap();
                }
                let peak = PEAK.load(SeqCst).saturating_sub(base);
                println!("transport_heap provider={id:?} bytes={size} normalized={normalized} variant={} additional_peak_bytes={peak} requests={}", if legacy {"baseline_4eb3eae"} else {"streamed"}, count.load(SeqCst));
                if !legacy {
                    assert!(
                        peak < 3_000_000,
                        "transport memory must not grow with saved audio"
                    );
                }
            }
        }
    }
    server.abort();
}
