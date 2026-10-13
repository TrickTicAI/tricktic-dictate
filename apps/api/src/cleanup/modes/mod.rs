//! Versioned mode profiles and immutable processing attempts on durable transcripts.
//! Context and transcripts are private data, never included in diagnostics.
const PROMPT_VERSION: u32 = 4;
mod captured;
pub(super) mod diagnostics;
mod preview;
mod retention;
mod speculative;
mod style;
pub use captured::{CapturedMode, LiveProcessing, PreparedDelivery};
pub use diagnostics::Diagnostics;
pub use style::{Formatting, Layout, Rewrite};
mod historical;
mod processing;
mod routes;
mod store;
pub(crate) use historical::historical_identity;
#[cfg(all(test, windows))]
mod replay;
#[cfg(test)]
mod tests;
use crate::transcription::{ApiError, ModelSelection};
pub use routes::{routes, service};
use serde::{Deserialize, Serialize};
pub use store::ModeService;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModeKind {
    #[default]
    Regular,
    Super,
    Command,
}
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextPolicy {
    #[serde(default)]
    pub application: bool,
    #[serde(default)]
    pub selected_text: bool,
    #[serde(default)]
    pub clipboard: bool,
}
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextSnapshot {
    #[serde(default)]
    pub application: String,
    #[serde(default)]
    pub selected_text: String,
    #[serde(default)]
    pub clipboard: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModeProfile {
    pub id: String,
    pub revision: u64,
    pub name: String,
    pub kind: ModeKind,
    pub instructions: String,
    pub text_model: Option<ModelSelection>,
    pub context: ContextPolicy,
    #[serde(default)]
    pub shortcut: Option<String>,
    #[serde(default)]
    pub formatting: Formatting,
    #[serde(default)]
    pub process_while_speaking: bool,
    #[serde(default)]
    pub fast_processing: bool,
    /// Used when the chosen model fails outright, and when it is clearly slower
    /// than usual: twice its usual answer time, never sooner than
    /// `fallback_after_ms`. Both requests then run, and the first usable answer
    /// is the one that is pasted.
    /// Saved wordings of this mode's instructions, with the writing settings they
    /// were written for. They are compared against real dictations before one is
    /// adopted; only the mode's own fields decide what a dictation actually uses.
    #[serde(default)]
    pub prompt_versions: Vec<PromptVersion>,
    #[serde(default)]
    pub fallback_model: Option<ModelSelection>,
    #[serde(default)]
    pub fallback_after_ms: Option<u64>,
    #[serde(default)]
    pub polish_timeout_ms: Option<u64>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptVersion {
    pub id: String,
    pub name: String,
    pub instructions: String,
    pub formatting: Formatting,
    #[serde(default)]
    pub fast_processing: bool,
    pub created_at_ms: u64,
}
pub const VERSION_LIMIT: usize = 10;
/// The shortest wait before the backup joins a slow chosen model. The actual wait
/// is twice that model's usual answer time (`processing::fallback_wait`), so an
/// ordinary dictation never pays for a second request.
pub const DEFAULT_FALLBACK_MS: u64 = 1_500;
pub const SLOW_FACTOR: u64 = 2;
/// Taken as a model's usual answer time until it has answered a few times.
pub const COLD_USUAL_MS: u64 = 1_500;
pub const MIN_FALLBACK_MS: u64 = 250;
pub const MAX_FALLBACK_MS: u64 = 15_000;
pub const DEFAULT_POLISH_TIMEOUT_MS: u64 = 3_000;
pub const FALLBACK_RESPONSE_MS: u64 = 2_000;
impl ModeProfile {
    /// An explicitly saved long hedge is respected, with two seconds reserved
    /// for the backup. Old profiles need no migration or new cloud consent.
    pub fn polish_timeout_ms(&self) -> u64 {
        self.polish_timeout_ms
            .unwrap_or(DEFAULT_POLISH_TIMEOUT_MS)
            .max(if self.fallback_model.is_some() {
                self.fallback_after_ms.unwrap_or(DEFAULT_FALLBACK_MS) + FALLBACK_RESPONSE_MS
            } else {
                0
            })
    }
    pub(super) fn fallback(&self) -> Option<(&ModelSelection, std::time::Duration)> {
        let model = self.fallback_model.as_ref()?;
        Some((
            model,
            std::time::Duration::from_millis(self.fallback_after_ms.unwrap_or(DEFAULT_FALLBACK_MS)),
        ))
    }
}
/// Which of these modes can be chosen now: Regular always, any other mode only
/// once its text model is configured. The window's mode menu applies the same
/// rule to the same discovery, so the tray never offers a mode it refuses.
pub fn usable(profiles: &[ModeProfile]) -> Vec<bool> {
    let choices = super::provider::mode_discovery();
    profiles
        .iter()
        .map(|profile| {
            profile.kind == ModeKind::Regular
                || profile.text_model.as_ref().is_some_and(|model| {
                    choices.iter().any(|choice| {
                        choice.configured
                            && std::mem::discriminant(&choice.selection.provider)
                                == std::mem::discriminant(&model.provider)
                            && choice.selection.model == model.model
                    })
                })
        })
        .collect()
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModeSettings {
    pub revision: u64,
    pub active: String,
    pub profiles: Vec<ModeProfile>,
}
impl Default for ModeSettings {
    fn default() -> Self {
        Self { revision: 1, active: "regular".into(), profiles: [
            ("regular", "Regular", ModeKind::Regular, ""),
            ("super", "Super", ModeKind::Super, style::POLISHED_SUPER),
            ("command", "Command", ModeKind::Command, "Apply the spoken instruction to Selected Text. Return only the transformed text. Preserve its languages unless the spoken instruction explicitly requests translation."),
        ].into_iter().map(|(id, name, kind, instructions)| ModeProfile {
            id: id.into(), revision: 1, name: name.into(), kind, shortcut: None,
            formatting: Formatting::default(), process_while_speaking: kind == ModeKind::Super, fast_processing: true,
            instructions: instructions.into(), text_model: None, fallback_model: None, fallback_after_ms: None, polish_timeout_ms: None, prompt_versions: vec![],
            context: ContextPolicy { selected_text: kind == ModeKind::Command, ..Default::default() },
        }).collect() }
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessRequest {
    pub operation_id: Uuid,
    pub profile_id: String,
    pub profile_revision: u64,
    #[serde(default)]
    pub context: ContextSnapshot,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AttemptState {
    Pending,
    Succeeded { text: String },
    Failed { error: ApiError },
}
#[derive(Clone, Deserialize, Serialize)]
pub struct ProcessingAttempt {
    pub id: Uuid,
    pub job_id: Uuid,
    pub created_at_ms: u64,
    pub profile: ModeProfile,
    pub request: ProcessRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    pub raw_text: String,
    pub input_text: String,
    pub prompt_version: u32,
    pub state: AttemptState,
    #[serde(default, skip_serializing_if = "Diagnostics::is_empty")]
    pub diagnostics: Diagnostics,
}

#[cfg(test)]
mod local_qualification;
