//! Opt-in real-model qualification of production mode policies and output schema.
//! Uses synthetic text exclusively; full runtime lifecycle has separate API smoke tests.
use super::*;
use crate::transcription::ProviderId;
use serde_json::json;

struct LocalText {
    client: reqwest::Client,
    port: u16,
}
#[async_trait::async_trait]
impl processing::Processor for LocalText {
    async fn generate(
        &self,
        _: &ModelSelection,
        instructions: &str,
        input: &str,
    ) -> Result<String, ApiError> {
        let response = self
            .client
            .post(format!(
                "http://127.0.0.1:{}/v1/chat/completions",
                self.port
            ))
            // Deliberately public fixture token: never consult production credentials.
            .bearer_auth("writer-local-qualification")
            .json(&json!({"model": "writer-qualification", "temperature": 0,
                "max_tokens": 2048, "response_format": {"type": "json_object"},
                "json_schema": crate::cleanup::local_text::output_schema(input),
                "messages": [{"role": "system", "content": instructions},
                             {"role": "user", "content": input}]}))
            .send()
            .await
            .map_err(|_| processing::invalid("Local qualification transport failed"))?;
        if !response.status().is_success() {
            return Err(processing::invalid(
                "Local qualification HTTP request failed",
            ));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| processing::invalid("Local qualification JSON failed"))?;
        if body["choices"][0]["finish_reason"] != "stop" {
            return Err(processing::invalid(
                "Local qualification output was incomplete",
            ));
        }
        body["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| processing::invalid("Local qualification output missing"))
    }
}

#[tokio::test]
#[ignore = "Requires the pinned native Qwen/llama.cpp qualification server; see scripts/models/qualify-text.py"]
async fn real_local_super_and_command_policies() {
    let port = std::env::var("WRITER_TEXT_QUALIFICATION_PORT")
        .expect("Set the local qualification port")
        .parse::<u16>()
        .unwrap();
    assert_ne!(port, 0);
    let provider = LocalText {
        port,
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap(),
    };
    let samples = [
        (
            ModeKind::Super,
            "hello maya the meeting starts at 14:30 on Tuesday signature",
            "",
            vec!["Maya", "14:30", "Tuesday", "EXACT 729"],
        ),
        (
            ModeKind::Super,
            "שלום דנה הפגישה ביום שלישי בשעה 14:30",
            "",
            vec!["דנה", "שלישי", "14:30"],
        ),
        (
            ModeKind::Super,
            "שלום Maya הפגישה ביום Tuesday בשעה 14:30 signature",
            "",
            vec!["Maya", "Tuesday", "14:30", "EXACT 729"],
        ),
        (
            ModeKind::Command,
            "Replace the word Tuesday with Wednesday and keep everything else unchanged",
            "The meeting is on Tuesday at 14:30.",
            vec!["Wednesday", "14:30"],
        ),
    ];
    let mut failures = Vec::new();
    for (index, (kind, raw, target, required)) in samples.into_iter().enumerate() {
        let mut profile = ModeSettings::default()
            .profiles
            .into_iter()
            .find(|p| p.kind == kind)
            .unwrap();
        profile.text_model = Some(ModelSelection {
            provider: ProviderId::Local,
            model: "qwen3-4b-instruct-2507-q4".into(),
        });
        let id = Uuid::new_v4();
        let attempt = ProcessingAttempt {
            request_digest: None,
            id,
            job_id: Uuid::new_v4(),
            created_at_ms: 0,
            diagnostics: Diagnostics::default(),
            request: ProcessRequest {
                operation_id: id,
                profile_id: profile.id.clone(),
                profile_revision: profile.revision,
                context: ContextSnapshot {
                    selected_text: target.into(),
                    ..Default::default()
                },
            },
            profile,
            raw_text: raw.into(),
            input_text: raw.into(),
            prompt_version: 2,
            state: AttemptState::Pending,
        };
        let vocabulary = crate::transcription::Vocabulary {
            snippets: vec![crate::transcription::Snippet {
                phrase: "signature".into(),
                text: "EXACT 729".into(),
            }],
            ..crate::vocabulary::empty_vocabulary()
        };
        let result = processing::run(&attempt, &vocabulary, &provider)
            .await
            .unwrap();
        if let Some(directory) = std::env::var_os("WRITER_TEXT_QUALIFICATION_OUTPUT") {
            std::fs::write(
                std::path::PathBuf::from(directory).join(format!("sample-{index}.json")),
                serde_json::to_vec(&json!({"input": raw, "target": target, "output": result}))
                    .unwrap(),
            )
            .unwrap();
        }
        if !required
            .iter()
            .all(|word| result.to_lowercase().contains(&word.to_lowercase()))
        {
            failures.push(format!(
                "sample {index}: a required fact or protected snippet was changed"
            ));
        }
        if kind == ModeKind::Command && result != "The meeting is on Wednesday at 14:30." {
            failures.push(format!("sample {index}: Command changed unrelated content"));
        }
        assert!(attempt.raw_text == raw);
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}
