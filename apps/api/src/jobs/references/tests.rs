use super::*;
use crate::{
    providers::{ProviderInfo, SavedAudio, TranscriptionProvider},
    recordings::RecordingStore,
    transcription::*,
};
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> ContractCatalog {
    serde_json::from_str(include_str!(
        "../../../../../packages/contracts/examples.json"
    ))
    .unwrap()
}
struct Heard;
#[async_trait]
impl TranscriptionProvider for Heard {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::Openai,
            execution_location: ExecutionLocation::Cloud,
            configured: true,
            available: true,
        }
    }
    async fn transcribe(
        &self,
        _: &SavedAudio,
        _: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        let JobStatus::Succeeded { result } = fixture().job.data.status else {
            panic!()
        };
        Ok(result.transcript)
    }
}
async fn open(root: &std::path::Path) -> Arc<Pipeline> {
    Pipeline::open(root.to_path_buf(), 1_000_000, vec![Arc::new(Heard)])
        .await
        .unwrap()
}
/// A saved recording with one finished transcription.
async fn transcribed(pipeline: &Pipeline) -> crate::transcription::Job {
    let f = fixture();
    let mut metadata = f.recording.data.metadata;
    metadata.byte_length = 1_000;
    pipeline
        .recordings
        .save(metadata, Box::new(std::io::Cursor::new(vec![7; 1_000])))
        .await
        .unwrap();
    let job = pipeline
        .jobs
        .submit(f.transcription_request.data)
        .await
        .unwrap();
    pipeline.run_pending().await.unwrap();
    pipeline.jobs.get(job.id).await.unwrap()
}
fn raw(job: &crate::transcription::Job) -> String {
    let JobStatus::Succeeded { result } = &job.status else {
        panic!("not transcribed")
    };
    result.transcript.raw_text.clone()
}
async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn correction_is_saved_beside_the_raw_transcript_and_survives_restart() {
    let temp = Temp(std::env::temp_dir().join(format!("writer-reference-{}", Uuid::new_v4())));
    let pipeline = open(&temp.0).await;
    let job = transcribed(&pipeline).await;
    let recording = job.request.recording_id;
    let heard = raw(&job);
    let app = super::super::routes_with_pipeline(Some(pipeline.clone()));
    let uri = format!("/api/v1/recordings/{recording}/reference");

    let (status, body) = call(&app, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_null());

    let corrected = "merge all finished agent work in this repo";
    let (status, body) = call(
        &app,
        "PUT",
        &uri,
        Some(serde_json::json!({
            "api_version": "1",
            "data": {"job_id": job.id, "text": format!("  {corrected}\n"), "gold": true}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["text"], corrected);
    assert_eq!(body["data"]["source"], "user");
    assert_eq!(body["data"]["gold"], true);
    assert_eq!(body["data"]["needs_check"], false);

    // The raw transcript is never replaced.
    assert_eq!(raw(&pipeline.jobs.get(job.id).await.unwrap()), heard);
    drop(app);
    drop(pipeline);

    let pipeline = open(&temp.0).await;
    let app = super::super::routes_with_pipeline(Some(pipeline.clone()));
    let (_, body) = call(&app, "GET", &uri, None).await;
    assert_eq!(body["data"]["text"], corrected);
    assert_eq!(body["data"]["gold"], true);
    assert_eq!(raw(&pipeline.jobs.get(job.id).await.unwrap()), heard);
    let (_, body) = call(&app, "GET", "/api/v1/references", None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    let (status, _) = call(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = call(&app, "GET", &uri, None).await;
    assert!(body["data"].is_null());
    assert_eq!(raw(&pipeline.jobs.get(job.id).await.unwrap()), heard);
}

#[tokio::test]
async fn a_correction_must_name_a_finished_transcript_of_the_same_recording() {
    let temp = Temp(std::env::temp_dir().join(format!("writer-reference-{}", Uuid::new_v4())));
    let pipeline = open(&temp.0).await;
    let job = transcribed(&pipeline).await;
    let other = Uuid::new_v4();
    let error = pipeline
        .correct(
            other,
            ReferenceInput {
                job_id: job.id,
                text: "x".into(),
                gold: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    let missing = pipeline
        .correct(
            job.request.recording_id,
            ReferenceInput {
                job_id: Uuid::new_v4(),
                text: "x".into(),
                gold: false,
            },
        )
        .await;
    assert!(missing.is_err());
    let app = super::super::routes_with_pipeline(Some(pipeline.clone()));
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/recordings/not-a-uuid/reference",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(pipeline.references.all().await.unwrap().is_empty());
}

#[tokio::test]
async fn drafted_references_wait_for_a_check_and_history_deletion_removes_them() {
    let temp = Temp(std::env::temp_dir().join(format!("writer-reference-{}", Uuid::new_v4())));
    let pipeline = open(&temp.0).await;
    let job = transcribed(&pipeline).await;
    let recording = job.request.recording_id;
    // The drafting tool writes files directly; a broken file hides nothing else.
    let root = temp.0.join("references");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(format!("{}.json", Uuid::new_v4())), b"{broken").unwrap();
    let draft = Reference {
        recording_id: recording,
        job_id: job.id,
        text: "first draft".into(),
        gold: true,
        source: ReferenceSource::Draft,
        needs_check: true,
        alternative: Some("second draft".into()),
        updated_at_ms: 1,
    };
    std::fs::write(
        root.join(format!("{recording}.json")),
        serde_json::to_vec(&draft).unwrap(),
    )
    .unwrap();
    assert_eq!(pipeline.references.all().await.unwrap(), vec![draft]);

    let checked = pipeline
        .correct(
            recording,
            ReferenceInput {
                job_id: job.id,
                text: "checked".into(),
                gold: true,
            },
        )
        .await
        .unwrap();
    assert!(!checked.needs_check && checked.alternative.is_none());
    assert_eq!(checked.source, ReferenceSource::User);

    pipeline.delete_history().await.unwrap();
    assert!(pipeline.references.get(recording).await.unwrap().is_none());
    assert!(!root.exists());
}
