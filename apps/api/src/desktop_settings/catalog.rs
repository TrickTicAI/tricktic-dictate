//! One curated artifact manifest shared by setup, language validation and the web UI.
use crate::{local_stt::catalog::ModelSpec, transcription::InputLanguage};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Deserialize, Serialize)]
pub struct CatalogModel {
    pub id: String,
    pub family: String,
    pub display_name: String,
    pub precision: String,
    pub file_name: String,
    pub source_name: String,
    pub revision: String,
    pub source_url: String,
    pub bytes: u64,
    pub sha256: String,
    pub license: String,
    /// Credit the license requires wherever the model is offered (CC BY 4.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    pub notes: String,
    pub input_languages: Vec<InputLanguage>,
    pub startup_language: String,
    pub runtime: String,
    pub backends: Vec<String>,
    pub qualification: String,
}
pub fn models() -> &'static [CatalogModel] {
    static CATALOG: OnceLock<Vec<CatalogModel>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("model-catalog.json"))
            .expect("validated curated artifact manifest")
    })
}
pub fn model(id: &str) -> Option<&'static CatalogModel> {
    models().iter().find(|m| m.id == id)
}
/// Adapt the shared manifest to the existing runtime without duplicating identities.
/// The strings are borrowed from a process-lifetime OnceLock, not per-request leaks.
pub fn specs() -> &'static [ModelSpec] {
    static SPECS: OnceLock<Vec<ModelSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        models()
            .iter()
            .map(|m| ModelSpec {
                id: &m.id,
                display_name: &m.display_name,
                file_name: &m.file_name,
                source_url: &m.source_url,
                source_name: &m.source_name,
                bytes: m.bytes,
                sha256: &m.sha256,
                license: &m.license,
                notes: &m.notes,
                startup_language: &m.startup_language,
            })
            .collect()
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_catalog_preserves_installed_identities_and_registers_every_variant() {
        let mut names = std::collections::BTreeSet::new();
        let mut files = std::collections::BTreeSet::new();
        for m in models() {
            assert!(names.insert(&m.id));
            assert!(files.insert(&m.file_name));
            assert!(
                m.id.len() <= 64
                    && m.id
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            );
            assert!(!m.file_name.contains(['/', '\\']));
            assert_eq!(m.revision.len(), 40);
            assert!(m.source_url.contains(&format!("/resolve/{}/", m.revision)));
            assert_eq!(m.sha256.len(), 64);
            assert!(m.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
            assert!(m.bytes > 0 && !m.input_languages.is_empty());
        }
        for previous in crate::local_stt::catalog::MODELS {
            let current = model(previous.id).unwrap();
            assert_eq!(current.file_name, previous.file_name);
            assert_eq!(current.sha256, previous.sha256);
            assert_eq!(current.bytes, previous.bytes);
        }
        assert!(model("whisper-small-en")
            .unwrap()
            .input_languages
            .iter()
            .all(|l| matches!(l, InputLanguage::En)));
        assert!(model("../escape").is_none());
        for m in models() {
            let proof = evidence(&m.id, &m.sha256).unwrap();
            assert!(proof.tests.iter().any(|t| t.backend == "cpu"));
            assert!(proof.tests.iter().any(|t| t.backend == "vulkan"));
            assert!(evidence(&m.id, "different-artifact").is_none());
        }
    }
    /// scripts/desktop/notices.mjs bundles a license text for each of these.
    #[test]
    fn every_model_license_has_a_bundled_notice_and_cc_by_names_its_creator() {
        for m in models() {
            assert!(
                ["mit", "apache-2.0", "CC-BY-4.0"].contains(&m.license.as_str()),
                "{}",
                m.id
            );
            let attribution = m.attribution.as_deref().unwrap_or_default();
            if m.license == "CC-BY-4.0" {
                assert!(attribution.contains("creativecommons.org/licenses/by/4.0"));
            } else {
                assert!(attribution.is_empty(), "{}", m.id);
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct ModelEvidence {
    pub model_sha256: String,
    pub host: String,
    pub date: String,
    pub scope: String,
    pub tests: Vec<SmokeTest>,
}
#[derive(Deserialize, Serialize)]
pub struct SmokeTest {
    pub sample: String,
    pub language: InputLanguage,
    pub backend: String,
    pub elapsed_ms: u64,
    pub word_errors: u64,
    pub reference_words: u64,
    pub receipt_sha256: String,
}
pub fn evidence(id: &str, sha256: &str) -> Option<&'static ModelEvidence> {
    static EVIDENCE: OnceLock<std::collections::BTreeMap<String, ModelEvidence>> = OnceLock::new();
    EVIDENCE
        .get_or_init(|| {
            serde_json::from_str(include_str!("model-smoke-tests.json"))
                .expect("validated model smoke evidence")
        })
        .get(id)
        .filter(|entry| entry.model_sha256 == sha256)
}
