pub mod account_sync;
pub mod cleanup;
pub mod desktop_host;
pub mod desktop_settings;
pub mod helper_security;
pub mod jobs;
pub mod language;
pub mod latency;
pub mod limits;
pub mod local_stt;
pub mod mobile;
pub mod providers;
pub mod recordings;
mod security_headers;
pub mod self_hosted;
#[cfg(all(test, unix))]
mod test_support;
pub mod transcription;
pub mod vocabulary;

use axum::{routing::get, Json, Router};
use std::path::PathBuf;
use tower_http::{services::ServeDir, trace::TraceLayer};

#[derive(Clone, Debug, Default)]
pub struct ProviderConfiguration {
    pub openai_configured: bool,
    pub gemini_configured: bool,
}

impl ProviderConfiguration {
    pub fn from_env() -> Self {
        let configured = |name: &str| {
            std::env::var(name)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        };
        Self {
            openai_configured: configured("OPENAI_API_KEY"),
            gemini_configured: configured("GEMINI_API_KEY"),
        }
    }
}

/// Configuration is read once by the retained runtime adapters, never sent to the browser.
pub fn app(_configuration: ProviderConfiguration, web_dist: PathBuf) -> Router {
    let providers = || async {
        use axum::response::IntoResponse;
        let pipeline = match jobs::runtime().await {
            Ok(pipeline) => pipeline,
            Err(error) => return jobs::response_error(error),
        };
        let setup = desktop_settings::global().ok();
        let providers: Vec<_> = pipeline.providers.iter().map(|provider| {
            let info = provider.info();
            let mut runtime = serde_json::Value::Null;
            let (name, models) = match info.id {
                transcription::ProviderId::Openai => ("OpenAI", vec!["gpt-transcribe".to_owned(), "gpt-4o-transcribe".into(), "gpt-4o-mini-transcribe".into(), "whisper-1".into()]),
                transcription::ProviderId::Gemini => ("Gemini", vec!["gemini-3.8-flash".to_owned()]),
                transcription::ProviderId::Grok => ("Grok", vec!["grok-voice-transcribe-2.0".to_owned()]),
                transcription::ProviderId::Openrouter => ("OpenRouter", vec![]),
                transcription::ProviderId::Local => {
                    // The loaded managed model, or the explicitly configured external alias.
                    let external = std::env::var("WRITER_LOCAL_STT_URL").is_ok();
                    let models = if external {
                        std::env::var("WRITER_LOCAL_STT_MODEL").ok().filter(|m| local_stt::catalog::spec(m).is_some()).into_iter().collect()
                    } else {
                        setup.as_ref().and_then(|s| s.runtime.loaded()).map(|l| l.model).into_iter().collect()
                    };
                    if let (false, Some(setup)) = (external, setup.as_ref()) {
                        runtime = serde_json::to_value(setup.runtime.worker()).unwrap_or_default();
                    }
                    ("Local model", models)
                }
            };
            let status = if !info.configured { "not_configured" } else if info.available { "available" } else if matches!(info.id, transcription::ProviderId::Local) { "not_ready" } else { "unverified" };
            serde_json::json!({ "id": info.id, "name": name, "models": models, "configured": info.configured, "available": info.available, "status": status, "execution_location": info.execution_location, "runtime": runtime })
        }).collect();
        Json(serde_json::json!({ "providers": providers })).into_response()
    };

    let admission = match limits::http::Admission::environment() {
        Ok(admission) => admission,
        Err(error) => {
            return Router::new().fallback(move || {
                let error = error.clone();
                async move { jobs::response_error(error) }
            })
        }
    };
    Router::new()
        .route("/api/v1/health", get(|| async {
            Json(serde_json::json!({ "status": "ok", "service": "metrix-writer-api", "version": env!("CARGO_PKG_VERSION") }))
        }))
        .route("/api/v1/providers", get(providers))
        .merge(recordings::routes())
        .merge(jobs::routes())
        .merge(local_stt::preview_routes())
        .merge(language::routes())
        .merge(vocabulary::routes())
        .merge(cleanup::routes())
        .merge(desktop_settings::routes())
        .fallback_service(ServeDir::new(web_dist))
        .layer(axum::middleware::from_fn_with_state(admission, limits::http::admit))
        .layer(TraceLayer::new_for_http())
        // Outermost: include static files, fallback, and extractor/method errors.
        .layer(axum::middleware::map_response(security_headers::apply))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_is_available_without_keys_or_web_build() {
        let app = app(
            ProviderConfiguration::default(),
            PathBuf::from("missing-dist"),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
