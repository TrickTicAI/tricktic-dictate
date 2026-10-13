//! The closed MW-OUATH-1 v1 wire contract. Manual backup stays limited to
//! settings/dictionary; transcript transfers require their own enrollment.
use super::json::{self, Checked, Invalid};
use crate::cleanup::modes::{Formatting, Layout, Rewrite};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

pub const SCHEMA_VERSION: u64 = 1;
/// Per-field dictionary limit, in UTF-8 bytes.
pub const MAX_DICTIONARY_BYTES: usize = 8192;
/// A manual export's per-kind entry limit and dictionary byte budget.
pub const MAX_ENTRIES_PER_KIND: usize = 200;
pub const MAX_EXPORT_DICTIONARY_BYTES: usize = 256 * 1024;
/// The whole settings/dictionary file, checked before parsing.
pub const MAX_BACKUP_BYTES: usize = 512 * 1024;
pub const BACKUP_FORMAT: &str = "metrix-writer-portable";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Settings,
    Dictionary,
    Transcripts,
}
impl Category {
    /// Categories in manual settings/dictionary backup. History is enrolled separately.
    pub const ALL: [Category; 2] = [Category::Settings, Category::Dictionary];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Dictionary => "dictionary",
            Self::Transcripts => "transcripts",
        }
    }
    fn parse(text: &str) -> Checked<Self> {
        match text {
            "settings" => Ok(Self::Settings),
            "dictionary" => Ok(Self::Dictionary),
            "transcripts" => Ok(Self::Transcripts),
            _ => Err(Invalid("unknown_category")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKey {
    UiLanguage,
    InputLanguage,
    OutputLanguage,
    TranscriptionModel,
    DictationMode,
    Cleanup,
    Delivery,
    Theme,
    PolishInstructions,
}
impl SettingKey {
    pub const ALL: [SettingKey; 9] = [
        Self::UiLanguage,
        Self::InputLanguage,
        Self::OutputLanguage,
        Self::TranscriptionModel,
        Self::DictationMode,
        Self::Cleanup,
        Self::Delivery,
        Self::Theme,
        Self::PolishInstructions,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UiLanguage => "ui_language",
            Self::InputLanguage => "input_language",
            Self::OutputLanguage => "output_language",
            Self::TranscriptionModel => "transcription_model",
            Self::DictationMode => "dictation_mode",
            Self::Cleanup => "cleanup",
            Self::Delivery => "delivery",
            Self::Theme => "theme",
            Self::PolishInstructions => "polish_instructions",
        }
    }
    fn index(self) -> u128 {
        Self::ALL.iter().position(|k| *k == self).unwrap() as u128 + 1
    }
    /// The contract's fixed record ID, `00000000-0000-4000-8000-00000000000N`.
    pub fn id(self) -> Uuid {
        Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | self.index())
    }
    pub fn from_id(id: &Uuid) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == *id)
    }
    fn parse(text: &str) -> Checked<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == text)
            .ok_or(Invalid("unknown_setting"))
    }
}

fn one_of(text: &str, allowed: &[&str]) -> Checked<String> {
    allowed
        .contains(&text)
        .then(|| text.to_owned())
        .ok_or(Invalid("unexpected_value"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub provider: String,
    pub model: String,
}
pub const PROVIDERS: [&str; 5] = ["openai", "gemini", "grok", "openrouter", "local"];
impl Model {
    pub(super) fn decode(value: &Value) -> Checked<Self> {
        let map = json::object(value, &["provider", "model"])?;
        let provider = one_of(json::string(map, "provider")?, &PROVIDERS)?;
        let model = json::string(map, "model")?;
        if !safe_model_id(model) {
            return Err(Invalid("unsafe_model_id"));
        }
        Ok(Self {
            provider,
            model: model.to_owned(),
        })
    }
    pub(super) fn encode(&self) -> Value {
        json!({ "provider": self.provider, "model": self.model })
    }
}
/// A catalog name, never a URL or path: no `..`, no empty slash segment.
pub fn safe_model_id(model: &str) -> bool {
    let bytes = model.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 256
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/~-".contains(b))
        && !model.contains("..")
        && !model.split('/').any(str::is_empty)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    Preserve,
    Translate(String),
}
impl Output {
    pub(super) fn decode(value: &Value) -> Checked<Self> {
        match value.get("mode").and_then(Value::as_str) {
            Some("preserve") => {
                json::object(value, &["mode"])?;
                Ok(Self::Preserve)
            }
            Some("translate") => {
                let map = json::object(value, &["mode", "language"])?;
                Ok(Self::Translate(one_of(
                    json::string(map, "language")?,
                    &["en", "he"],
                )?))
            }
            _ => Err(Invalid("unexpected_value")),
        }
    }
    pub(super) fn encode(&self) -> Value {
        match self {
            Self::Preserve => json!({ "mode": "preserve" }),
            Self::Translate(language) => json!({ "mode": "translate", "language": language }),
        }
    }
}

pub const REASONING: [&str; 6] = [
    "provider_default",
    "off",
    "minimal",
    "low",
    "medium",
    "high",
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cleanup {
    Off,
    Enabled { selection: Model, reasoning: String },
}
impl Cleanup {
    fn decode(value: &Value) -> Checked<Self> {
        match value.get("mode").and_then(Value::as_str) {
            Some("off") => {
                json::object(value, &["mode"])?;
                Ok(Self::Off)
            }
            Some("enabled") => {
                let map = json::object(value, &["mode", "selection", "reasoning"])?;
                Ok(Self::Enabled {
                    selection: Model::decode(&map["selection"])?,
                    reasoning: one_of(json::string(map, "reasoning")?, &REASONING)?,
                })
            }
            _ => Err(Invalid("unexpected_value")),
        }
    }
    fn encode(&self) -> Value {
        match self {
            Self::Off => json!({ "mode": "off" }),
            Self::Enabled {
                selection,
                reasoning,
            } => {
                json!({ "mode": "enabled", "selection": selection.encode(), "reasoning": reasoning })
            }
        }
    }
}

/// Super polishing instructions plus the Modes writing preferences
/// (MW-SETTINGS-8). The text limit and non-blank rule match mode-profile
/// validation exactly, so any value decoded here applies on the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Polish {
    pub instructions: String,
    pub formatting: Formatting,
}
pub const MAX_POLISH_BYTES: usize = 16384;
impl Polish {
    fn decode(value: &Value) -> Checked<Self> {
        let map = json::object(value, &["instructions", "formatting"])?;
        let instructions = json::string(map, "instructions")?;
        if instructions.is_empty()
            || instructions.len() > MAX_POLISH_BYTES
            || instructions.contains('\0')
            || instructions.trim().is_empty()
        {
            return Err(Invalid("invalid_polish_instructions"));
        }
        let f = json::object(
            &map["formatting"],
            &[
                "rewrite",
                "layout",
                "lists",
                "remove_fillers",
                "self_corrections",
                "spoken_punctuation",
            ],
        )?;
        let rewrite = match json::string(f, "rewrite")? {
            "light" => Rewrite::Light,
            "polished" => Rewrite::Polished,
            "concise" => Rewrite::Concise,
            _ => return Err(Invalid("unexpected_value")),
        };
        let layout = match json::string(f, "layout")? {
            "automatic" => Layout::Automatic,
            "single_line" => Layout::SingleLine,
            _ => return Err(Invalid("unexpected_value")),
        };
        Ok(Self {
            instructions: instructions.to_owned(),
            formatting: Formatting {
                rewrite,
                layout,
                lists: json::boolean(f, "lists")?,
                remove_fillers: json::boolean(f, "remove_fillers")?,
                self_corrections: json::boolean(f, "self_corrections")?,
                spoken_punctuation: json::boolean(f, "spoken_punctuation")?,
            },
        })
    }
    fn encode(&self) -> Value {
        let rewrite = match self.formatting.rewrite {
            Rewrite::Light => "light",
            Rewrite::Polished => "polished",
            Rewrite::Concise => "concise",
        };
        let layout = match self.formatting.layout {
            Layout::Automatic => "automatic",
            Layout::SingleLine => "single_line",
        };
        json!({
            "instructions": self.instructions,
            "formatting": {
                "rewrite": rewrite,
                "layout": layout,
                "lists": self.formatting.lists,
                "remove_fillers": self.formatting.remove_fillers,
                "self_corrections": self.formatting.self_corrections,
                "spoken_punctuation": self.formatting.spoken_punctuation,
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setting {
    UiLanguage(String),
    InputLanguage(String),
    OutputLanguage(Output),
    TranscriptionModel(Model),
    DictationMode(String),
    Cleanup(Cleanup),
    Delivery(String),
    Theme(String),
    PolishInstructions(Polish),
}
impl Setting {
    pub fn key(&self) -> SettingKey {
        match self {
            Self::UiLanguage(_) => SettingKey::UiLanguage,
            Self::InputLanguage(_) => SettingKey::InputLanguage,
            Self::OutputLanguage(_) => SettingKey::OutputLanguage,
            Self::TranscriptionModel(_) => SettingKey::TranscriptionModel,
            Self::DictationMode(_) => SettingKey::DictationMode,
            Self::Cleanup(_) => SettingKey::Cleanup,
            Self::Delivery(_) => SettingKey::Delivery,
            Self::Theme(_) => SettingKey::Theme,
            Self::PolishInstructions(_) => SettingKey::PolishInstructions,
        }
    }
    pub fn decode(value: &Value) -> Checked<Self> {
        let map = json::object(value, &["kind", "key", "value"])?;
        json::literal(map, "kind", "setting")?;
        let v = &map["value"];
        let text = || v.as_str().ok_or(Invalid("not_a_string"));
        Ok(match SettingKey::parse(json::string(map, "key")?)? {
            SettingKey::UiLanguage => Self::UiLanguage(one_of(text()?, &["en", "he"])?),
            SettingKey::InputLanguage => {
                Self::InputLanguage(one_of(text()?, &["en", "he", "mixed"])?)
            }
            SettingKey::OutputLanguage => Self::OutputLanguage(Output::decode(v)?),
            SettingKey::TranscriptionModel => Self::TranscriptionModel(Model::decode(v)?),
            SettingKey::DictationMode => {
                Self::DictationMode(one_of(text()?, &["regular", "super", "command"])?)
            }
            SettingKey::Cleanup => Self::Cleanup(Cleanup::decode(v)?),
            SettingKey::Delivery => {
                Self::Delivery(one_of(text()?, &["raw", "dictionary", "cleaned"])?)
            }
            SettingKey::Theme => Self::Theme(one_of(text()?, &["system", "light", "dark"])?),
            SettingKey::PolishInstructions => Self::PolishInstructions(Polish::decode(v)?),
        })
    }
    pub fn value(&self) -> Value {
        match self {
            Self::UiLanguage(s)
            | Self::InputLanguage(s)
            | Self::DictationMode(s)
            | Self::Delivery(s)
            | Self::Theme(s) => json!(s),
            Self::OutputLanguage(o) => o.encode(),
            Self::TranscriptionModel(m) => m.encode(),
            Self::Cleanup(c) => c.encode(),
            Self::PolishInstructions(p) => p.encode(),
        }
    }
    pub fn encode(&self) -> Value {
        json!({ "kind": "setting", "key": self.key().as_str(), "value": self.value() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Hint {
        enabled: bool,
        text: String,
    },
    Correction {
        enabled: bool,
        from: String,
        to: String,
    },
    Snippet {
        enabled: bool,
        phrase: String,
        text: String,
    },
}
/// Nonblank under JavaScript `trim()`, NUL-free and within the byte limit.
pub fn dictionary_text(text: &str) -> bool {
    let js_space = |c: char| c.is_whitespace() || c == '\u{feff}';
    !text.chars().all(js_space) && !text.contains('\0') && text.len() <= MAX_DICTIONARY_BYTES
}
impl Entry {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hint { .. } => "hint",
            Self::Correction { .. } => "correction",
            Self::Snippet { .. } => "snippet",
        }
    }
    fn decode(value: &Value) -> Checked<Self> {
        let kind = value.get("kind").and_then(Value::as_str);
        let entry = match kind {
            Some("hint") => {
                let m = json::object(value, &["kind", "enabled", "text"])?;
                Self::Hint {
                    enabled: json::boolean(m, "enabled")?,
                    text: json::string(m, "text")?.to_owned(),
                }
            }
            Some("correction") => {
                let m = json::object(value, &["kind", "enabled", "from", "to"])?;
                Self::Correction {
                    enabled: json::boolean(m, "enabled")?,
                    from: json::string(m, "from")?.to_owned(),
                    to: json::string(m, "to")?.to_owned(),
                }
            }
            Some("snippet") => {
                let m = json::object(value, &["kind", "enabled", "phrase", "text"])?;
                Self::Snippet {
                    enabled: json::boolean(m, "enabled")?,
                    phrase: json::string(m, "phrase")?.to_owned(),
                    text: json::string(m, "text")?.to_owned(),
                }
            }
            _ => return Err(Invalid("unknown_entry_kind")),
        };
        if !entry.texts().iter().all(|t| dictionary_text(t)) {
            return Err(Invalid("invalid_dictionary_text"));
        }
        Ok(entry)
    }
    pub fn texts(&self) -> Vec<&str> {
        match self {
            Self::Hint { text, .. } => vec![text],
            Self::Correction { from, to, .. } => vec![from, to],
            Self::Snippet { phrase, text, .. } => vec![phrase, text],
        }
    }
    pub fn encode(&self) -> Value {
        match self {
            Self::Hint { enabled, text } => {
                json!({ "kind": "hint", "enabled": enabled, "text": text })
            }
            Self::Correction { enabled, from, to } => {
                json!({ "kind": "correction", "enabled": enabled, "from": from, "to": to })
            }
            Self::Snippet {
                enabled,
                phrase,
                text,
            } => json!({ "kind": "snippet", "enabled": enabled, "phrase": phrase, "text": text }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    Setting(Setting),
    Entry(Entry),
    Transcript(super::history_wire::Transcript),
}
impl Data {
    pub fn category(&self) -> Category {
        match self {
            Self::Setting(_) => Category::Settings,
            Self::Entry(_) => Category::Dictionary,
            Self::Transcript(_) => Category::Transcripts,
        }
    }
    fn decode(category: Category, id: &Uuid, value: &Value) -> Checked<Self> {
        let data = match category {
            Category::Settings => Self::Setting(Setting::decode(value)?),
            Category::Dictionary => Self::Entry(Entry::decode(value)?),
            Category::Transcripts => {
                Self::Transcript(super::history_wire::Transcript::decode(value)?)
            }
        };
        data.check_id(id)?;
        Ok(data)
    }
    /// Fixed setting IDs belong to their keys; no other record may use one.
    pub fn check_id(&self, id: &Uuid) -> Checked<()> {
        match self {
            Self::Setting(s) if s.key().id() == *id => Ok(()),
            Self::Entry(_) | Self::Transcript(_) if SettingKey::from_id(id).is_none() => Ok(()),
            _ => Err(Invalid("id_mismatch")),
        }
    }
    pub fn encode(&self) -> Value {
        match self {
            Self::Setting(s) => s.encode(),
            Self::Entry(e) => e.encode(),
            Self::Transcript(t) => t.encode(),
        }
    }
}

/// `None` is a content-free tombstone.
pub type Content = Option<Data>;
fn decode_content(category: Category, id: &Uuid, value: &Value) -> Checked<Content> {
    match value.get("state").and_then(Value::as_str) {
        Some("deleted") => {
            json::object(value, &["state"])?;
            Ok(None)
        }
        Some("live") => {
            let map = json::object(value, &["state", "data"])?;
            Ok(Some(Data::decode(category, id, &map["data"])?))
        }
        _ => Err(Invalid("unknown_content_state")),
    }
}
pub fn encode_content(content: &Content) -> Value {
    match content {
        None => json!({ "state": "deleted" }),
        Some(data) => json!({ "state": "live", "data": data.encode() }),
    }
}
fn version(map: &Map<String, Value>) -> Checked<()> {
    match json::uint(map, "schema_version") {
        Ok(SCHEMA_VERSION) => Ok(()),
        _ => Err(Invalid("unsupported_version")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: Uuid,
    pub origin: Uuid,
    pub category: Category,
    pub revision: u64,
    pub content: Content,
}
const RECORD_KEYS: [&str; 6] = [
    "schema_version",
    "id",
    "origin_device_id",
    "category",
    "revision",
    "content",
];
impl Record {
    pub fn decode(value: &Value) -> Checked<Self> {
        if serde_json::to_vec(value).map_or(true, |b| b.len() > 768 * 1024) {
            return Err(Invalid("record_too_large"));
        }
        let map = json::object(value, &RECORD_KEYS)?;
        version(map)?;
        let id = json::uuid(map, "id")?;
        let category = Category::parse(json::string(map, "category")?)?;
        Ok(Self {
            id,
            origin: json::uuid(map, "origin_device_id")?,
            category,
            revision: json::positive(map, "revision")?,
            content: decode_content(category, &id, &map["content"])?,
        })
    }
    pub fn encode(&self) -> Value {
        json!({
            "schema_version": SCHEMA_VERSION, "id": self.id, "origin_device_id": self.origin,
            "category": self.category.as_str(), "revision": self.revision,
            "content": encode_content(&self.content),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub uid: String,
    pub account_epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub binding: Binding,
    pub consent_generation: u64,
    pub history_enrollment_id: Option<Uuid>,
    pub category: Category,
    pub operation_id: Uuid,
    pub device_id: Uuid,
    pub record_id: Uuid,
    pub origin: Uuid,
    pub base_revision: u64,
    pub content: Content,
}
impl Operation {
    pub fn decode(value: &Value) -> Checked<Self> {
        if serde_json::to_vec(value).map_or(true, |b| b.len() > 768 * 1024) {
            return Err(Invalid("record_too_large"));
        }
        let mut keys = vec![
            "schema_version",
            "uid",
            "account_epoch",
            "consent_generation",
            "category",
            "operation_id",
            "device_id",
            "record_id",
            "origin_device_id",
            "base_revision",
            "content",
        ];
        if value["category"] == "transcripts" {
            keys.push("history_enrollment_id");
        }
        let map = json::object(value, &keys)?;
        version(map)?;
        let record_id = json::uuid(map, "record_id")?;
        let category = Category::parse(json::string(map, "category")?)?;
        let uid = json::string(map, "uid")?;
        if uid.is_empty() || uid.chars().count() > 128 || uid.contains('\0') {
            return Err(Invalid("invalid_uid"));
        }
        let base_revision = json::uint(map, "base_revision")?;
        if base_revision == json::MAX_SAFE {
            return Err(Invalid("revision_exhausted"));
        }
        Ok(Self {
            binding: Binding {
                uid: uid.to_owned(),
                account_epoch: json::positive(map, "account_epoch")?,
            },
            consent_generation: json::positive(map, "consent_generation")?,
            history_enrollment_id: if category == Category::Transcripts {
                Some(json::uuid(map, "history_enrollment_id")?)
            } else {
                None
            },
            category,
            operation_id: json::uuid(map, "operation_id")?,
            device_id: json::uuid(map, "device_id")?,
            record_id,
            origin: json::uuid(map, "origin_device_id")?,
            base_revision,
            content: decode_content(category, &record_id, &map["content"])?,
        })
    }
    pub fn encode(&self) -> Value {
        let mut value = json!({
            "schema_version": SCHEMA_VERSION, "uid": self.binding.uid,
            "account_epoch": self.binding.account_epoch,
            "consent_generation": self.consent_generation, "category": self.category.as_str(),
            "operation_id": self.operation_id, "device_id": self.device_id,
            "record_id": self.record_id, "origin_device_id": self.origin,
            "base_revision": self.base_revision, "content": encode_content(&self.content),
        });
        if let Some(id) = self.history_enrollment_id {
            value["history_enrollment_id"] = json!(id);
        }
        value
    }
    /// The record this operation writes when it is accepted.
    pub fn record(&self, revision: u64) -> Record {
        Record {
            id: self.record_id,
            origin: self.origin,
            category: self.category,
            revision,
            content: self.content.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toggle {
    pub enabled: bool,
    pub generation: u64,
}
fn decode_toggle(value: &Value) -> Checked<Toggle> {
    let map = json::object(value, &["enabled", "generation"])?;
    Ok(Toggle {
        enabled: json::boolean(map, "enabled")?,
        generation: json::positive(map, "generation")?,
    })
}

/// Authoritative per-account consent. Transcript consent is kept exactly as
/// read (MW-OUATH-9 owns its meaning) so a settings change never rewrites it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Consent {
    pub binding: Binding,
    pub revision: u64,
    pub settings: Toggle,
    pub dictionary: Toggle,
    pub transcripts: Value,
}
impl Consent {
    pub fn decode(value: &Value) -> Checked<Self> {
        let map = json::object(
            value,
            &[
                "schema_version",
                "uid",
                "account_epoch",
                "revision",
                "settings",
                "dictionary",
                "transcripts",
            ],
        )?;
        version(map)?;
        let transcripts = &map["transcripts"];
        let t = transcripts.as_object().ok_or(Invalid("not_an_object"))?;
        let history_ok = match (t.get("enabled"), t.get("history")) {
            (Some(Value::Bool(false)), Some(h)) => *h == json!({ "mode": "off" }),
            (Some(Value::Bool(true)), Some(h)) => {
                let m = json::object(h, &["mode", "enrollment_id"])?;
                json::uuid(m, "enrollment_id")?;
                ["future_only", "include_existing"].contains(&json::string(m, "mode")?)
            }
            _ => false,
        };
        let tm = json::object(transcripts, &["enabled", "generation", "history"])?;
        json::positive(tm, "generation")?;
        if !history_ok {
            return Err(Invalid("invalid_history"));
        }
        Ok(Self {
            binding: Binding {
                uid: json::string(map, "uid")?.to_owned(),
                account_epoch: json::positive(map, "account_epoch")?,
            },
            revision: json::positive(map, "revision")?,
            settings: decode_toggle(&map["settings"])?,
            dictionary: decode_toggle(&map["dictionary"])?,
            transcripts: transcripts.clone(),
        })
    }
    pub fn encode(&self) -> Value {
        json!({
            "schema_version": SCHEMA_VERSION, "uid": self.binding.uid,
            "account_epoch": self.binding.account_epoch, "revision": self.revision,
            "settings": self.settings, "dictionary": self.dictionary,
            "transcripts": self.transcripts,
        })
    }
    pub fn toggle(&self, category: Category) -> Toggle {
        match category {
            Category::Settings => self.settings,
            Category::Dictionary => self.dictionary,
            Category::Transcripts => Toggle {
                enabled: self.transcripts["enabled"] == true,
                generation: self.transcripts_generation(),
            },
        }
    }
    pub fn transcripts_generation(&self) -> u64 {
        self.transcripts["generation"].as_u64().unwrap_or(1)
    }
    /// The only account a client may create: every category off, generation 1.
    pub fn initial(uid: &str) -> Self {
        let off = Toggle {
            enabled: false,
            generation: 1,
        };
        Self {
            binding: Binding {
                uid: uid.to_owned(),
                account_epoch: 1,
            },
            revision: 1,
            settings: off,
            dictionary: off,
            transcripts: json!({ "enabled": false, "generation": 1, "history": { "mode": "off" } }),
        }
    }
    /// `nextConsent`: a changed toggle advances its generation by exactly one.
    pub fn next(&self, category: Category, enabled: bool) -> Self {
        let mut next = self.clone();
        let before = self.toggle(category);
        let after = Toggle {
            enabled,
            generation: before.generation + u64::from(before.enabled != enabled),
        };
        match category {
            Category::Settings => next.settings = after,
            Category::Dictionary => next.dictionary = after,
            Category::Transcripts => {
                next.transcripts = json!({"enabled": enabled, "generation": after.generation, "history": if enabled { self.transcripts["history"].clone() } else { json!({"mode":"off"}) }});
            }
        }
        next.revision += 1;
        next
    }
}

/// The account document's client-visible state.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub transcript_reset: Option<u64>,
    pub epoch: u64,
    pub active: bool,
    pub consent: Consent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    WrongAccount,
    AccountUnavailable,
    StaleEpoch,
    ConsentRequired,
    StaleGeneration,
    HistoryNotEnrolled,
    IdempotencyConflict,
    Deleted,
    Immutable,
    InvalidRecord,
    InvalidBase,
    QuotaExceeded,
    UnsupportedVersion,
}

/// The decision for one outbox operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Acknowledged(Record),
    Conflict(Record),
    Rejected(Rejection),
}

/// A manual export record: live data with its stable identity, no revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRecord {
    pub id: Uuid,
    pub origin: Uuid,
    pub data: Data,
}
impl ExportRecord {
    pub fn decode(value: &Value, category: Category) -> Checked<Self> {
        let map = json::object(
            value,
            &[
                "schema_version",
                "id",
                "origin_device_id",
                "category",
                "data",
            ],
        )?;
        version(map)?;
        if Category::parse(json::string(map, "category")?)? != category {
            return Err(Invalid("wrong_category"));
        }
        let id = json::uuid(map, "id")?;
        Ok(Self {
            id,
            origin: json::uuid(map, "origin_device_id")?,
            data: Data::decode(category, &id, &map["data"])?,
        })
    }
    pub fn encode(&self) -> Value {
        json!({
            "schema_version": SCHEMA_VERSION, "id": self.id, "origin_device_id": self.origin,
            "category": self.data.category().as_str(), "data": self.data.encode(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    pub export_id: Uuid,
    pub created_at_ms: u64,
    pub settings: Vec<ExportRecord>,
    pub dictionary: Vec<ExportRecord>,
}
impl Backup {
    /// Parses and fully validates a portable file before anything changes.
    /// A newer format is reported as such rather than stripped to fit v1.
    pub fn decode(bytes: &[u8]) -> Checked<Self> {
        if bytes.len() > MAX_BACKUP_BYTES {
            return Err(Invalid("too_large"));
        }
        let value = json::parse(bytes)?;
        let root = value.as_object().ok_or(Invalid("not_an_object"))?;
        if root.get("format").and_then(Value::as_str) != Some(BACKUP_FORMAT) {
            return Err(Invalid("not_a_portable_file"));
        }
        if root.get("format_version").and_then(Value::as_u64) != Some(1) {
            return Err(Invalid("unsupported_version"));
        }
        let map = json::object(
            &value,
            &[
                "format",
                "format_version",
                "export_id",
                "created_at_ms",
                "settings",
                "dictionary",
            ],
        )?;
        let list = |key: &str, category: Category, max: usize| -> Checked<Vec<ExportRecord>> {
            let items = map[key].as_array().ok_or(Invalid("not_an_array"))?;
            if items.len() > max {
                return Err(Invalid("too_many_records"));
            }
            items
                .iter()
                .map(|item| ExportRecord::decode(item, category))
                .collect()
        };
        let backup = Self {
            export_id: json::uuid(map, "export_id")?,
            created_at_ms: json::uint(map, "created_at_ms")?,
            settings: list("settings", Category::Settings, SettingKey::ALL.len())?,
            dictionary: list("dictionary", Category::Dictionary, 3 * MAX_ENTRIES_PER_KIND)?,
        };
        backup.check()?;
        Ok(backup)
    }
    /// The contract's whole-file rules: unique IDs and dictionary capacity.
    pub fn check(&self) -> Checked<()> {
        let mut ids = std::collections::BTreeSet::new();
        if !self
            .settings
            .iter()
            .chain(&self.dictionary)
            .all(|r| ids.insert(r.id))
        {
            return Err(Invalid("duplicate_id"));
        }
        for kind in ["hint", "correction", "snippet"] {
            let count = self
                .dictionary
                .iter()
                .filter(|r| matches!(&r.data, Data::Entry(e) if e.kind() == kind))
                .count();
            if count > MAX_ENTRIES_PER_KIND {
                return Err(Invalid("too_many_entries"));
            }
        }
        let encoded: Vec<Value> = self.dictionary.iter().map(ExportRecord::encode).collect();
        if serde_json::to_vec(&encoded).map_or(true, |b| b.len() > MAX_EXPORT_DICTIONARY_BYTES) {
            return Err(Invalid("dictionary_too_large"));
        }
        Ok(())
    }
    pub fn encode(&self) -> Value {
        json!({
            "format": BACKUP_FORMAT, "format_version": 1, "export_id": self.export_id,
            "created_at_ms": self.created_at_ms,
            "settings": self.settings.iter().map(ExportRecord::encode).collect::<Vec<_>>(),
            "dictionary": self.dictionary.iter().map(ExportRecord::encode).collect::<Vec<_>>(),
        })
    }
}
