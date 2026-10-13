//! Static language semantics, separate from runtime availability and accuracy.
use crate::transcription::{
    ApiError, Envelope, ErrorCode, ExecutionLocation, InputLanguage, ModelSelection,
    OutputLanguage, TextLanguage, TranscriptionOptions,
};
use axum::{routing::get, Json, Router};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub id: InputLanguage,
    pub label: String,
    pub input_language: InputLanguage,
    pub output_language: OutputLanguage,
    pub task: String,
    pub language_hints: Vec<TextLanguage>,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub selection: ModelSelection,
    pub input_languages: Vec<InputLanguage>,
    pub translation_targets: Vec<TextLanguage>,
    pub execution_location: ExecutionLocation,
    pub language_control: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub presets: Vec<Preset>,
    pub models: Vec<ModelCapabilities>,
}

/// Also consumed by the standalone web component; never implies a configured model.
pub fn catalog() -> Catalog {
    let mut catalog: Catalog = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/language/catalog.json"
    ))
    .expect("validated language catalog");
    catalog.models.retain(|m| {
        !matches!(
            m.selection.provider,
            crate::transcription::ProviderId::Local
        )
    });
    catalog
        .models
        .extend(
            crate::desktop_settings::catalog::models()
                .iter()
                .map(|m| ModelCapabilities {
                    selection: ModelSelection {
                        provider: crate::transcription::ProviderId::Local,
                        model: m.id.clone(),
                    },
                    input_languages: m.input_languages.clone(),
                    translation_targets: vec![],
                    execution_location: ExecutionLocation::SelfHostedServer,
                    language_control: m.notes.clone(),
                }),
        );
    catalog
}

fn same<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// Capability-only checks allow a configured cloud adapter's first unverified trial.
pub fn validate_capabilities(options: &TranscriptionOptions) -> Result<(), ApiError> {
    let catalog = catalog();
    let model = catalog
        .models
        .iter()
        .find(|model| same(&model.selection, &options.selection));
    let Some(model) = model else {
        return Err(unsupported(
            "The selected model has no registered language capabilities.",
        ));
    };
    if !model
        .input_languages
        .iter()
        .any(|language| same(language, &options.input_language))
    {
        return Err(unsupported("The selected model does not support this input language. ivrit-turbo supports Hebrew only."));
    }
    if let OutputLanguage::Translate { language } = &options.output_language {
        if !model
            .translation_targets
            .iter()
            .any(|target| same(target, language))
        {
            return Err(unsupported("Translation is explicitly selected but is not supported by this adapter. Select preserved spoken language."));
        }
    }
    Ok(())
}

/// Use only when actual readiness is required. Cloud first trials may be unverified.
/// Adapter validation remains authoritative at execution time.
pub fn validate(options: &TranscriptionOptions, available: bool) -> Result<(), ApiError> {
    validate_capabilities(options)?;
    if !available {
        return Err(ApiError {
            code: ErrorCode::ProviderUnavailable,
            message: "The selected model is unavailable or unverified. Check credentials or the configured GPU server and loaded model.".into(),
            retryable: true,
            retry_after_ms: None,
        });
    }
    Ok(())
}

fn unsupported(message: &str) -> ApiError {
    ApiError {
        code: ErrorCode::UnsupportedCapability,
        message: message.into(),
        retryable: false,
        retry_after_ms: None,
    }
}

pub fn routes() -> Router {
    Router::new().route(
        "/api/v1/language-presets",
        get(|| async { Json(Envelope::new(catalog())) }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::{CleanupPolicy, ProviderId, Vocabulary};
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    fn options(model: &ModelCapabilities, language: InputLanguage) -> TranscriptionOptions {
        TranscriptionOptions {
            selection: model.selection.clone(),
            input_language: language,
            output_language: OutputLanguage::Preserve {},
            vocabulary: Vocabulary {
                hints: vec![],
                corrections: vec![],
                snippets: vec![],
            },
            cleanup: CleanupPolicy::Off {},
        }
    }

    #[test]
    fn presets_preserve_language_and_never_force_mixed_to_one_language() {
        let catalog = catalog();
        assert_eq!(catalog.presets.len(), 3);
        for preset in &catalog.presets {
            assert!(same(&preset.id, &preset.input_language));
            assert!(matches!(
                preset.output_language,
                OutputLanguage::Preserve {}
            ));
            assert_eq!(preset.task, "transcribe");
            assert_eq!(
                preset.language_hints.len(),
                if matches!(preset.id, InputLanguage::Mixed) {
                    2
                } else {
                    1
                }
            );
        }
    }

    #[test]
    fn every_model_enforces_input_translation_and_readiness_independently() {
        for model in catalog().models {
            for language in [InputLanguage::En, InputLanguage::He, InputLanguage::Mixed] {
                let mut options = options(&model, language);
                let supported = model
                    .input_languages
                    .iter()
                    .any(|input| same(input, &options.input_language));
                assert_eq!(validate(&options, true).is_ok(), supported);
                if supported {
                    assert!(matches!(
                        validate(&options, false).unwrap_err().code,
                        ErrorCode::ProviderUnavailable
                    ));
                }
                options.output_language = OutputLanguage::Translate {
                    language: TextLanguage::En,
                };
                assert!(matches!(
                    validate(&options, true).unwrap_err().code,
                    ErrorCode::UnsupportedCapability
                ));
            }
        }
    }

    #[test]
    fn unknown_model_and_cross_provider_alias_are_rejected() {
        let model = catalog().models.pop().unwrap();
        let mut options = options(&model, InputLanguage::He);
        options.selection.model = "unregistered-hebrew-model".into();
        assert!(validate(&options, true).is_err());
        options.selection = ModelSelection {
            provider: ProviderId::Openai,
            model: "ivrit-turbo".into(),
        };
        assert!(validate(&options, true).is_err());
    }

    #[tokio::test]
    async fn route_returns_versioned_catalog_without_claiming_availability() {
        let response = routes()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/language-presets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["api_version"], "1");
        assert_eq!(
            value["data"]["models"].as_array().unwrap().len(),
            catalog().models.len()
        );
        assert!(value["data"]["models"][0].get("available").is_none());
    }
}
