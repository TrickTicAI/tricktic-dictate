use super::super::tests::AudioFixture;
use super::*;
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
    routing::any,
    Router,
};
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Default)]
struct Receiver {
    bytes: Vec<u8>,
    name: String,
    starts: usize,
    finalizes: usize,
    offsets: Vec<u64>,
    finalized: bool,
}
#[tokio::test]
async fn gemini_restart_queries_received_bytes_and_never_repeats_finalization() {
    for stage in ["start", "upload", "finalize"] {
        let state = Arc::new(Mutex::new(Receiver::default()));
        let failed = Arc::new(AtomicBool::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let host = origin.clone();
        let receiver = state.clone();
        let app=Router::new().fallback(any(move |headers:HeaderMap,uri:Uri,body:Bytes| {
            let state=receiver.clone(); let host=host.clone(); let failed=failed.clone();
            async move {
                let command=headers.get("x-goog-upload-command").and_then(|v|v.to_str().ok()).unwrap_or("get");
                let response={
                    let mut s=state.lock().unwrap();
                    match command {
                        "start"=> { s.starts+=1; s.name=serde_json::from_slice::<Value>(&body).unwrap()["file"]["name"].as_str().unwrap().to_owned(); s.bytes.clear(); s.finalized=false;
                            ([("X-Goog-Upload-URL",format!("{host}/upload/session?opaque=private-token"))],String::new()).into_response() },
                        "query"=> { ([("X-Goog-Upload-Status",if s.finalized {"final"} else {"active"}.to_owned()),("X-Goog-Upload-Size-Received",s.bytes.len().to_string())],String::new()).into_response() },
                        "upload"=> { let offset=headers["x-goog-upload-offset"].to_str().unwrap().parse::<u64>().unwrap(); assert_eq!(offset,s.bytes.len() as u64); s.offsets.push(offset); s.bytes.extend_from_slice(&body);
                            ([("X-Goog-Upload-Status","active")],String::new()).into_response() },
                        "finalize"=> { assert!(!s.finalized); s.finalized=true; s.finalizes+=1;
                            ([("X-Goog-Upload-Status","final")],json!({"file":{"name":s.name,"uri":format!("{host}/v1beta/{}",s.name),"state":"ACTIVE"}}).to_string()).into_response() },
                        "get" if uri.path()==format!("/v1beta/{}",s.name)=> json!({"name":s.name,"uri":format!("{host}/v1beta/{}",s.name),"state":"ACTIVE"}).to_string().into_response(),
                        _ => StatusCode::NOT_FOUND.into_response(),
                    }
                };
                if command==stage && !failed.swap(true,Ordering::SeqCst) { tokio::time::sleep(Duration::from_millis(300)).await; }
                response
            }
        })).layer(axum::extract::DefaultBodyLimit::disable());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut audio = AudioFixture::new();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&audio.0.path)
            .unwrap()
            .set_len(10 * 1024 * 1024 + 17)
            .unwrap();
        audio.0.metadata.byte_length = 10 * 1024 * 1024 + 17;
        let before = Sha256::digest(std::fs::read(&audio.0.path).unwrap());
        let make = || {
            CloudProvider::new(
                ProviderId::Gemini,
                Some("fixture".into()),
                format!("{origin}/v1beta"),
                Duration::from_millis(150),
            )
        };
        let adapter = make();
        let credential = adapter.credential().unwrap().0;
        let mut upload = Upload::open(
            &audio.0,
            AudioFormat::parse(&ProviderId::Gemini, "audio/wav").unwrap(),
            GEMINI_FILE_LIMIT,
        )
        .await
        .unwrap();
        let error = adapter
            .gemini_file(&mut upload, &credential)
            .await
            .unwrap_err();
        assert!(!error.message.contains("private-token"));
        drop(adapter);
        drop(upload);
        let adapter = make();
        let mut upload = Upload::open(
            &audio.0,
            AudioFormat::parse(&ProviderId::Gemini, "audio/wav").unwrap(),
            GEMINI_FILE_LIMIT,
        )
        .await
        .unwrap();
        adapter.gemini_file(&mut upload, &credential).await.unwrap();
        {
            let s = state.lock().unwrap();
            assert_eq!(s.finalizes, 1);
            assert_eq!(Sha256::digest(&s.bytes), before);
            if stage != "start" {
                assert_eq!(s.starts, 1);
            }
            if stage == "upload" {
                assert_eq!(s.offsets, vec![0, 8 * 1024 * 1024]);
            }
        }
        assert_eq!(
            Sha256::digest(std::fs::read(&audio.0.path).unwrap()),
            before
        );
        server.abort();
    }
}
