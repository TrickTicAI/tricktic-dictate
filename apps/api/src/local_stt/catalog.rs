//! Static registry of local whisper.cpp weights Writer can download, import and load.
//! Entries pin an exact file identity; nothing here proves the file is installed.
use crate::transcription::InputLanguage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// API alias, also the language-catalog model name and the registry key.
    pub id: &'static str,
    pub display_name: &'static str,
    /// Fixed file name inside the private models directory; imports must use it too.
    pub file_name: &'static str,
    pub source_url: &'static str,
    pub source_name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub license: &'static str,
    pub notes: &'static str,
    /// Language passed to the worker at startup; requests still send their own hint.
    pub startup_language: &'static str,
}

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "ivrit-turbo",
        display_name: "ivrit.ai Whisper large-v3-turbo (Hebrew only)",
        file_name: "ivrit-turbo.bin",
        source_url: "https://huggingface.co/ivrit-ai/whisper-large-v3-turbo-ggml/resolve/main/ggml-model.bin",
        source_name: "ivrit-ai/whisper-large-v3-turbo-ggml",
        bytes: 1_624_555_275,
        sha256: "c8090411113357097bfafc2b8e228ec1639fa7f5fe4ecb5d054ac0ccef8641b1",
        license: "Apache-2.0 (model card)",
        notes: "Hebrew fine-tune. English or mixed speech is rejected because the model translated English input into Hebrew during research.",
        startup_language: "he",
    },
    ModelSpec {
        id: "whisper-large-v3-turbo",
        display_name: "Whisper large-v3-turbo (multilingual)",
        file_name: "ggml-large-v3-turbo.bin",
        source_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        source_name: "ggerganov/whisper.cpp",
        bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        license: "MIT (whisper.cpp), Whisper weights MIT",
        notes: "Upstream multilingual weights for English, Hebrew and mixed presets. Accuracy needs evaluation; it is not a Hebrew quality guarantee.",
        startup_language: "auto",
    },
    ModelSpec {
        id: "whisper-tiny",
        display_name: "Whisper tiny (smoke test)",
        file_name: "ggml-tiny.bin",
        source_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        source_name: "ggerganov/whisper.cpp",
        bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        license: "MIT (whisper.cpp), Whisper weights MIT",
        notes: "Small multilingual smoke-test weights. Not a quality recommendation for Hebrew or accented English.",
        startup_language: "auto",
    },
];

pub fn spec(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|model| model.id == id)
}

/// Registered input languages from the shared language catalog; never inferred here.
pub fn input_languages(id: &str) -> Vec<InputLanguage> {
    crate::language::catalog()
        .models
        .into_iter()
        .find(|model| {
            matches!(
                model.selection.provider,
                crate::transcription::ProviderId::Local
            ) && model.selection.model == id
        })
        .map(|model| model.input_languages)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_local_model_has_language_capabilities_and_exact_identity() {
        for model in MODELS {
            assert!(!input_languages(model.id).is_empty(), "{}", model.id);
            assert_eq!(model.sha256.len(), 64);
            assert!(model.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(model.source_url.starts_with("https://"));
            assert!(model.bytes > 0);
            assert!(!model.file_name.contains(['/', '\\']));
        }
        let hebrew_only = input_languages("ivrit-turbo");
        assert_eq!(hebrew_only.len(), 1);
        assert!(matches!(hebrew_only[0], InputLanguage::He));
        assert_eq!(input_languages("whisper-large-v3-turbo").len(), 3);
        assert!(spec("../etc/passwd").is_none());
    }
}
