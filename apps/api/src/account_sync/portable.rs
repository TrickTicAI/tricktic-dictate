//! Mapping between the desktop's existing stores and portable v1 records.
//!
//! Only the contract's allowlist crosses: languages, the transcription and
//! cleanup selections, the active mode's kind, the Super profile's polishing
//! instructions and writing preferences, the theme's light/dark kind and
//! dictionary entries with stable IDs. Microphones, runtime choices, paths,
//! shortcuts, panel placement, other profiles' prompts and keys never appear here.
//! Applying a value never downloads a model, calls a provider or inserts text:
//! a value this device cannot use yet is kept, unapplied, in the shadow store.
use super::wire::{Cleanup as PCleanup, Data, Entry, Model, Output, Polish, Setting, SettingKey};
use crate::cleanup::modes::Formatting;
use crate::transcription::{
    ApiError, CleanupPolicy, CorrectionEntry, HintEntry, ModelSelection, OutputLanguage, Settings,
    SnippetEntry, VocabularyDocument,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// The Super profile's portable face: its polishing instructions and the
/// writing preferences the Modes page edits (MW-SETTINGS-8). Prompt versions,
/// models and speeds stay on their own device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperPolish {
    pub instructions: String,
    pub formatting: Formatting,
}

/// The mode library's portable face: the active profile, every profile's kind,
/// and the Super profile's polishing instructions when one exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modes {
    pub active: String,
    pub profiles: Vec<(String, String)>,
    #[serde(default)]
    pub super_polish: Option<SuperPolish>,
}

/// The existing stores' portable-relevant state, read together.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Local {
    pub settings: Settings,
    pub vocabulary: VocabularyDocument,
    pub modes: Option<Modes>,
}

/// The WebView's named theme and whether it is light or dark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub id: String,
    pub kind: String,
}

#[async_trait::async_trait]
pub trait Stores: Send + Sync {
    async fn load(&self) -> Result<Local, ApiError>;
    /// Writes the stores whose portable state differs between `before` and
    /// `after`; each store's write is atomic on its own.
    async fn store(&self, before: &Local, after: &Local) -> Result<(), ApiError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    MissingKey,
    ModelNotInstalled,
    UnknownModel,
    Unsupported,
}

/// What this device can use right now. Checks only; nothing is started.
pub trait Capabilities: Send + Sync {
    fn transcription(&self, model: &ModelSelection) -> Readiness;
    fn cleanup(&self, model: &ModelSelection) -> Readiness;
    /// The language/model combination is one the device supports.
    fn languages(&self, settings: &Settings) -> bool;
}

/// Why a recognized portable value was kept without being applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// This desktop has no such option.
    Unsupported,
    MissingKey,
    ModelNotInstalled,
    UnknownModel,
    IncompatibleLanguage,
    /// The cleanup model was applied; its reasoning level has no desktop setting.
    ReasoningNotSupported,
    NoMatchingMode,
}

/// A recognized value kept in the portable shadow store: it round-trips in
/// export and sync but is not in force on this device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub value: Value,
    pub reason: Reason,
    /// The device's own value when the shadow was kept. When the user changes
    /// the field, the shadow is retired and the device's value is portable again.
    pub local: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", content = "reason", rename_all = "snake_case")]
pub enum Outcome {
    Applied,
    Unchanged,
    Preserved(Reason),
    Partial(Reason),
}

fn to_text<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()?
        .as_str()
        .map(str::to_owned)
}
fn from_text<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(Value::String(text.to_owned())).ok()
}
fn portable_model(selection: &ModelSelection) -> Option<Model> {
    let model = Model {
        provider: to_text(&selection.provider)?,
        model: selection.model.clone(),
    };
    super::wire::safe_model_id(&model.model).then_some(model)
}
fn local_model(model: &Model) -> Option<ModelSelection> {
    Some(ModelSelection {
        provider: from_text(&model.provider)?,
        model: model.model.clone(),
    })
}

/// This device's own value for a setting, before any shadow.
pub fn local_setting(local: &Local, theme: Option<&Theme>, key: SettingKey) -> Option<Setting> {
    let t = &local.settings.transcription;
    Some(match key {
        SettingKey::UiLanguage => Setting::UiLanguage(to_text(&local.settings.ui_language)?),
        SettingKey::InputLanguage => Setting::InputLanguage(to_text(&t.input_language)?),
        SettingKey::OutputLanguage => Setting::OutputLanguage(match &t.output_language {
            OutputLanguage::Preserve {} => Output::Preserve,
            OutputLanguage::Translate { language } => Output::Translate(to_text(language)?),
        }),
        SettingKey::TranscriptionModel => {
            Setting::TranscriptionModel(portable_model(&t.selection)?)
        }
        SettingKey::DictationMode => {
            let modes = local.modes.as_ref()?;
            let (_, kind) = modes.profiles.iter().find(|(id, _)| *id == modes.active)?;
            Setting::DictationMode(kind.clone())
        }
        SettingKey::Cleanup => Setting::Cleanup(match &t.cleanup {
            CleanupPolicy::Off {} => PCleanup::Off,
            CleanupPolicy::Enabled { selection } => PCleanup::Enabled {
                selection: portable_model(selection)?,
                reasoning: "provider_default".into(),
            },
        }),
        SettingKey::Delivery => return None,
        SettingKey::PolishInstructions => {
            let polish = local.modes.as_ref()?.super_polish.as_ref()?;
            Setting::PolishInstructions(Polish {
                instructions: polish.instructions.clone(),
                formatting: polish.formatting.clone(),
            })
        }
        SettingKey::Theme => {
            let kind = &theme?.kind;
            if kind != "light" && kind != "dark" {
                return None;
            }
            Setting::Theme(kind.clone())
        }
    })
}

/// The portable value of every setting and identified dictionary entry: the
/// shadow value where one is kept, otherwise the device's own.
pub fn project(
    local: &Local,
    theme: Option<&Theme>,
    shadow: &BTreeMap<SettingKey, Shadow>,
) -> BTreeMap<Uuid, Data> {
    let mut out = BTreeMap::new();
    for key in SettingKey::ALL {
        let value = match shadow.get(&key) {
            Some(kept) => Setting::decode(&kept.value).ok().map(Data::Setting),
            None => local_setting(local, theme, key).map(Data::Setting),
        };
        if let Some(value) = value {
            out.insert(key.id(), value);
        }
    }
    for (id, entry) in entries(&local.vocabulary) {
        if let Some(id) = id {
            out.insert(id, Data::Entry(entry));
        }
    }
    out
}

/// Dictionary entries as portable data, in the user's own order.
pub fn entries(doc: &VocabularyDocument) -> Vec<(Option<Uuid>, Entry)> {
    let hints = doc.hints.iter().map(|h| {
        (
            h.id,
            Entry::Hint {
                enabled: h.enabled,
                text: h.text.clone(),
            },
        )
    });
    let corrections = doc.corrections.iter().map(|c| {
        (
            c.id,
            Entry::Correction {
                enabled: c.enabled,
                from: c.from.clone(),
                to: c.to.clone(),
            },
        )
    });
    let snippets = doc.snippets.iter().map(|s| {
        (
            s.id,
            Entry::Snippet {
                enabled: s.enabled,
                phrase: s.phrase.clone(),
                text: s.text.clone(),
            },
        )
    });
    hints.chain(corrections).chain(snippets).collect()
}

/// Inserts or replaces one entry by ID. A new ID is appended to its list.
pub fn upsert_entry(doc: &mut VocabularyDocument, id: Uuid, entry: &Entry) {
    remove_entry(doc, id);
    match entry.clone() {
        Entry::Hint { enabled, text } => doc.hints.push(HintEntry {
            id: Some(id),
            text,
            enabled,
        }),
        Entry::Correction { enabled, from, to } => doc.corrections.push(CorrectionEntry {
            id: Some(id),
            from,
            to,
            enabled,
        }),
        Entry::Snippet {
            enabled,
            phrase,
            text,
        } => doc.snippets.push(SnippetEntry {
            id: Some(id),
            phrase,
            text,
            enabled,
        }),
    }
}
pub fn remove_entry(doc: &mut VocabularyDocument, id: Uuid) -> bool {
    let before = doc.hints.len() + doc.corrections.len() + doc.snippets.len();
    doc.hints.retain(|h| h.id != Some(id));
    doc.corrections.retain(|c| c.id != Some(id));
    doc.snippets.retain(|s| s.id != Some(id));
    before != doc.hints.len() + doc.corrections.len() + doc.snippets.len()
}
/// The contract's deterministic runtime order after an import or a synced
/// change: each list in UUID order, so every device matches rules alike.
pub fn sort_entries(doc: &mut VocabularyDocument) {
    doc.hints.sort_by_key(|h| h.id);
    doc.corrections.sort_by_key(|c| c.id);
    doc.snippets.sort_by_key(|s| s.id);
}

/// Everything a set of portable settings changes, with what could not apply.
pub struct Applied {
    pub local: Local,
    pub theme: Option<Theme>,
    pub shadow: BTreeMap<SettingKey, Shadow>,
    pub outcomes: BTreeMap<SettingKey, Outcome>,
}

/// Applies portable settings onto a copy of the device state. Nothing is
/// written; the caller persists the result through its journal.
pub fn apply_settings(
    local: &Local,
    theme: Option<&Theme>,
    shadow: &BTreeMap<SettingKey, Shadow>,
    settings: &[Setting],
    caps: &dyn Capabilities,
) -> Applied {
    let mut out = Applied {
        local: local.clone(),
        theme: theme.cloned(),
        shadow: shadow.clone(),
        outcomes: BTreeMap::new(),
    };
    let by_key: BTreeMap<SettingKey, &Setting> = settings.iter().map(|s| (s.key(), s)).collect();
    // The language and model fields are checked together, then one by one, so
    // an imported Hebrew-only model and its Hebrew input can arrive together.
    let joint = [
        SettingKey::TranscriptionModel,
        SettingKey::InputLanguage,
        SettingKey::OutputLanguage,
    ];
    let mut trial = out.local.settings.clone();
    let mut pending = Vec::new();
    for key in joint {
        let Some(setting) = by_key.get(&key) else {
            continue;
        };
        match stage(&mut trial, setting, caps) {
            Ok(()) => pending.push(*setting),
            Err(reason) => keep(&mut out, setting, Outcome::Preserved(reason)),
        }
    }
    if caps.languages(&trial) {
        out.local.settings = trial;
        for setting in pending {
            settle(&mut out, setting, Outcome::Applied);
        }
    } else {
        for setting in pending {
            let mut single = out.local.settings.clone();
            let _ = stage(&mut single, setting, caps);
            if caps.languages(&single) {
                out.local.settings = single;
                settle(&mut out, setting, Outcome::Applied);
            } else {
                keep(
                    &mut out,
                    setting,
                    Outcome::Preserved(Reason::IncompatibleLanguage),
                );
            }
        }
    }
    for (key, setting) in &by_key {
        if joint.contains(key) {
            continue;
        }
        let outcome = match setting {
            Setting::UiLanguage(v) => match from_text(v) {
                Some(language) => {
                    out.local.settings.ui_language = language;
                    Outcome::Applied
                }
                None => Outcome::Preserved(Reason::Unsupported),
            },
            Setting::DictationMode(kind) => match out.local.modes.as_mut() {
                Some(modes) => {
                    let current = modes
                        .profiles
                        .iter()
                        .find(|(id, _)| *id == modes.active)
                        .map(|(_, k)| k.clone());
                    if current.as_deref() == Some(kind.as_str()) {
                        Outcome::Applied
                    } else if let Some((id, _)) = modes
                        .profiles
                        .iter()
                        .find(|(id, k)| k == kind && id == kind)
                        .or_else(|| modes.profiles.iter().find(|(_, k)| k == kind))
                    {
                        modes.active = id.clone();
                        Outcome::Applied
                    } else {
                        Outcome::Preserved(Reason::NoMatchingMode)
                    }
                }
                None => Outcome::Preserved(Reason::Unsupported),
            },
            Setting::Cleanup(PCleanup::Off) => {
                out.local.settings.transcription.cleanup = CleanupPolicy::Off {};
                Outcome::Applied
            }
            Setting::Cleanup(PCleanup::Enabled {
                selection,
                reasoning,
            }) => match local_model(selection).map(|m| (caps.cleanup(&m), m)) {
                Some((Readiness::Ready, model)) => {
                    out.local.settings.transcription.cleanup =
                        CleanupPolicy::Enabled { selection: model };
                    if reasoning == "provider_default" {
                        Outcome::Applied
                    } else {
                        Outcome::Partial(Reason::ReasoningNotSupported)
                    }
                }
                Some((readiness, _)) => Outcome::Preserved(reason_for(readiness)),
                None => Outcome::Preserved(Reason::Unsupported),
            },
            Setting::PolishInstructions(polish) => match out.local.modes.as_mut() {
                Some(modes) if modes.super_polish.is_some() => {
                    modes.super_polish = Some(SuperPolish {
                        instructions: polish.instructions.clone(),
                        formatting: polish.formatting.clone(),
                    });
                    Outcome::Applied
                }
                Some(_) => Outcome::Preserved(Reason::NoMatchingMode),
                None => Outcome::Preserved(Reason::Unsupported),
            },
            Setting::Theme(kind) if kind == "light" || kind == "dark" => {
                if out.theme.as_ref().map(|t| &t.kind) != Some(kind) {
                    out.theme = Some(Theme {
                        id: kind.clone(),
                        kind: kind.clone(),
                    });
                }
                Outcome::Applied
            }
            Setting::Theme(_) | Setting::Delivery(_) => Outcome::Preserved(Reason::Unsupported),
            _ => unreachable!("joint settings are applied above"),
        };
        match outcome {
            Outcome::Applied => settle(&mut out, setting, outcome),
            other => keep(&mut out, setting, other),
        }
    }
    out
}

/// Stages a language/model value on `settings`, or says why it cannot be used.
fn stage(
    settings: &mut Settings,
    setting: &Setting,
    caps: &dyn Capabilities,
) -> Result<(), Reason> {
    let t = &mut settings.transcription;
    match setting {
        Setting::InputLanguage(v) => t.input_language = from_text(v).ok_or(Reason::Unsupported)?,
        Setting::OutputLanguage(Output::Preserve) => {
            t.output_language = OutputLanguage::Preserve {}
        }
        Setting::OutputLanguage(Output::Translate(language)) => {
            t.output_language = OutputLanguage::Translate {
                language: from_text(language).ok_or(Reason::Unsupported)?,
            }
        }
        Setting::TranscriptionModel(model) => {
            let selection = local_model(model).ok_or(Reason::Unsupported)?;
            match caps.transcription(&selection) {
                Readiness::Ready => t.selection = selection,
                other => return Err(reason_for(other)),
            }
        }
        _ => return Err(Reason::Unsupported),
    }
    Ok(())
}

fn reason_for(readiness: Readiness) -> Reason {
    match readiness {
        Readiness::Ready | Readiness::Unsupported => Reason::Unsupported,
        Readiness::MissingKey => Reason::MissingKey,
        Readiness::ModelNotInstalled => Reason::ModelNotInstalled,
        Readiness::UnknownModel => Reason::UnknownModel,
    }
}

/// The value is in force: any shadow for its key retires.
fn settle(out: &mut Applied, setting: &Setting, outcome: Outcome) {
    out.shadow.remove(&setting.key());
    out.outcomes.insert(setting.key(), outcome);
}

/// The value is kept unapplied (or only partly applied) in the shadow store.
fn keep(out: &mut Applied, setting: &Setting, outcome: Outcome) {
    let reason = match outcome {
        Outcome::Preserved(r) | Outcome::Partial(r) => r,
        _ => return settle(out, setting, outcome),
    };
    let own = local_setting(&out.local, out.theme.as_ref(), setting.key()).map(|s| s.encode());
    if own.as_ref() == Some(&setting.encode()) {
        // Already this device's value: nothing to keep aside.
        out.shadow.remove(&setting.key());
        out.outcomes.insert(setting.key(), Outcome::Unchanged);
        return;
    }
    out.shadow.insert(
        setting.key(),
        Shadow {
            value: setting.encode(),
            reason,
            local: own,
        },
    );
    out.outcomes.insert(setting.key(), outcome);
}

/// Retires shadows whose field the user has since changed on this device.
pub fn retire_changed(
    shadow: &mut BTreeMap<SettingKey, Shadow>,
    local: &Local,
    theme: Option<&Theme>,
) -> bool {
    let before = shadow.len();
    shadow.retain(|key, kept| local_setting(local, theme, *key).map(|s| s.encode()) == kept.local);
    before != shadow.len()
}
