//! The desktop's existing stores and capability checks behind the portable
//! mapping: transcription settings, the dictionary and the mode library.
use super::portable::{Capabilities, Local, Modes, Readiness, Stores, SuperPolish};
use crate::{
    cleanup::modes::{ModeKind, ModeService, ModeSettings},
    desktop_settings::{credentials::CredentialStatus, Setup},
    local_stt::managed::WeightsState,
    transcription::{ApiError, ErrorCode, ModelSelection, ProviderId, Settings},
    vocabulary::VocabularyStore,
};
use std::sync::Arc;

pub struct DesktopStores {
    pub setup: Arc<Setup>,
    pub vocabulary: Arc<VocabularyStore>,
    pub modes: Option<Arc<ModeService>>,
}

fn kind_name(kind: &ModeKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The Super profile whose polishing instructions sync: the builtin `super`
/// profile, else the first Super-kind profile. Load and store use the same
/// choice so a synced value lands where the next export reads it.
fn super_profile(settings: &ModeSettings) -> Option<usize> {
    settings
        .profiles
        .iter()
        .position(|p| p.id == "super" && p.kind == ModeKind::Super)
        .or_else(|| {
            settings
                .profiles
                .iter()
                .position(|p| p.kind == ModeKind::Super)
        })
}

#[async_trait::async_trait]
impl Stores for DesktopStores {
    async fn load(&self) -> Result<Local, ApiError> {
        let modes = match &self.modes {
            Some(service) => {
                let settings = service.settings().await?;
                let super_polish = super_profile(&settings).map(|index| SuperPolish {
                    instructions: settings.profiles[index].instructions.clone(),
                    formatting: settings.profiles[index].formatting.clone(),
                });
                Some(Modes {
                    active: settings.active.clone(),
                    profiles: settings
                        .profiles
                        .iter()
                        .map(|p| (p.id.clone(), kind_name(&p.kind)))
                        .collect(),
                    super_polish,
                })
            }
            None => None,
        };
        Ok(Local {
            settings: self.setup.settings(),
            vocabulary: self.vocabulary.load().await?,
            modes,
        })
    }
    async fn store(&self, before: &Local, after: &Local) -> Result<(), ApiError> {
        let json = |v: &Settings| serde_json::to_value(v).unwrap_or_default();
        if json(&before.settings) != json(&after.settings) {
            self.setup.save(after.settings.clone()).await?;
        }
        if serde_json::to_value(&before.vocabulary).ok()
            != serde_json::to_value(&after.vocabulary).ok()
        {
            self.vocabulary.save(&after.vocabulary).await?;
        }
        if let (Some(service), Some(next)) = (&self.modes, &after.modes) {
            let active_changed = before.modes.as_ref().map(|m| &m.active) != Some(&next.active);
            let polish_changed = before.modes.as_ref().and_then(|m| m.super_polish.as_ref())
                != next.super_polish.as_ref();
            if active_changed || polish_changed {
                let mut settings = service.settings().await?;
                if active_changed
                    && settings.active != next.active
                    && settings.profiles.iter().any(|p| p.id == next.active)
                {
                    settings.active = next.active.clone();
                }
                if polish_changed {
                    match (super_profile(&settings), &next.super_polish) {
                        (Some(index), Some(polish)) => {
                            settings.profiles[index].instructions = polish.instructions.clone();
                            settings.profiles[index].formatting = polish.formatting.clone();
                        }
                        // The profile this import targeted is gone; the caller
                        // rolls back and the shadow keeps the incoming value.
                        _ => {
                            return Err(ApiError {
                                code: ErrorCode::IdempotencyConflict,
                                message: "This mode changed. Reload before saving.".into(),
                                retryable: false,
                                retry_after_ms: None,
                            });
                        }
                    }
                }
                service.save(settings).await?;
            }
        }
        Ok(())
    }
}

/// The cloud speech models this desktop offers, as the setup view lists them.
fn cloud_speech_models(provider: &ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Openai => &[
            "gpt-transcribe",
            "gpt-4o-transcribe",
            "gpt-4o-mini-transcribe",
            "whisper-1",
        ],
        ProviderId::Gemini => &["gemini-3.8-flash"],
        ProviderId::Grok => &["grok-voice-transcribe-2.0"],
        ProviderId::Openrouter | ProviderId::Local => &[],
    }
}

pub struct DesktopCapabilities {
    pub setup: Arc<Setup>,
}
impl DesktopCapabilities {
    fn key(&self, provider: &ProviderId) -> bool {
        matches!(
            self.setup.credentials.status(provider),
            CredentialStatus::Environment | CredentialStatus::ProtectedStore
        )
    }
}
impl Capabilities for DesktopCapabilities {
    fn transcription(&self, model: &ModelSelection) -> Readiness {
        match model.provider {
            ProviderId::Local => {
                let runtime = &self.setup.runtime;
                if !runtime.catalog().iter().any(|spec| spec.id == model.model) {
                    Readiness::UnknownModel
                } else if !crate::local_stt::managed::engine::supported(&model.model) {
                    Readiness::Unsupported
                } else if !matches!(
                    runtime.weights(&model.model),
                    WeightsState::Installed { .. }
                ) {
                    Readiness::ModelNotInstalled
                } else {
                    Readiness::Ready
                }
            }
            ProviderId::Openrouter => Readiness::Unsupported,
            ref provider => {
                if !cloud_speech_models(provider).contains(&model.model.as_str()) {
                    Readiness::UnknownModel
                } else if !self.key(provider) {
                    Readiness::MissingKey
                } else {
                    Readiness::Ready
                }
            }
        }
    }
    fn cleanup(&self, model: &ModelSelection) -> Readiness {
        match model.provider {
            // Local text models are set up per device; a restored choice waits for that.
            ProviderId::Local => Readiness::ModelNotInstalled,
            ref provider if self.key(provider) => Readiness::Ready,
            _ => Readiness::MissingKey,
        }
    }
    fn languages(&self, settings: &Settings) -> bool {
        crate::language::validate_capabilities(&settings.transcription).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::super_profile;
    use crate::cleanup::modes::{ModeKind, ModeSettings};

    #[test]
    fn sync_targets_the_builtin_super_profile_first() {
        let mut settings = ModeSettings::default();
        let id = |settings: &ModeSettings| {
            super_profile(settings).map(|index| settings.profiles[index].id.clone())
        };
        assert_eq!(id(&settings).as_deref(), Some("super"));
        // An earlier Super-kind profile does not win over the builtin one.
        settings.profiles[0].kind = ModeKind::Super;
        assert_eq!(id(&settings).as_deref(), Some("super"));
        // Without the builtin profile the first Super-kind profile syncs.
        settings.profiles[0].kind = ModeKind::Regular;
        settings.profiles[1].id = "mine".into();
        assert_eq!(id(&settings).as_deref(), Some("mine"));
        settings.profiles.retain(|p| p.kind != ModeKind::Super);
        assert_eq!(id(&settings), None);
    }
}
