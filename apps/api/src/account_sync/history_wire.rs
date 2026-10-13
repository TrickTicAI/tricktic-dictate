//! Closed v1 transcript records. No local paths, audio, context or job requests.
use super::{
    json::{self, Checked, Invalid},
    wire::{Model, Output},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transcript {
    Original {
        attempt_id: Uuid,
        recorded_at_ms: u64,
        input_language: String,
        output_language: Output,
        selection: Model,
        text: String,
    },
    Derived {
        kind: String,
        original_record_id: Uuid,
        parent_record_id: Uuid,
        created_at_ms: u64,
        text: String,
    },
}
impl Transcript {
    pub fn decode(v: &Value) -> Checked<Self> {
        let kind = v["kind"].as_str().ok_or(Invalid("invalid_transcript"))?;
        let text = v["text"].as_str().ok_or(Invalid("invalid_text"))?;
        if serde_json::to_vec(v).map_or(true, |b| b.len() > 768 * 1024 - 2048) {
            return Err(Invalid("transcript_too_large"));
        }
        if text.contains('\0') || text.len() > 512 * 1024 || text.chars().count() > 131072 {
            return Err(Invalid("transcript_too_large"));
        }
        Ok(if kind == "original" {
            let m = json::object(
                v,
                &[
                    "kind",
                    "attempt_id",
                    "recorded_at_ms",
                    "input_language",
                    "output_language",
                    "selection",
                    "text",
                ],
            )?;
            let lang = json::string(m, "input_language")?;
            if !["en", "he", "mixed"].contains(&lang) {
                return Err(Invalid("invalid_language"));
            }
            Self::Original {
                attempt_id: json::uuid(m, "attempt_id")?,
                recorded_at_ms: json::uint(m, "recorded_at_ms")?,
                input_language: lang.into(),
                output_language: Output::decode(&m["output_language"])?,
                selection: Model::decode(&m["selection"])?,
                text: text.into(),
            }
        } else {
            if !["corrected", "cleaned", "edited"].contains(&kind) {
                return Err(Invalid("invalid_kind"));
            }
            let m = json::object(
                v,
                &[
                    "kind",
                    "original_record_id",
                    "parent_record_id",
                    "created_at_ms",
                    "text",
                ],
            )?;
            Self::Derived {
                kind: kind.into(),
                original_record_id: json::uuid(m, "original_record_id")?,
                parent_record_id: json::uuid(m, "parent_record_id")?,
                created_at_ms: json::uint(m, "created_at_ms")?,
                text: text.into(),
            }
        })
    }
    pub fn encode(&self) -> Value {
        match self {
            Self::Original {
                attempt_id,
                recorded_at_ms,
                input_language,
                output_language,
                selection,
                text,
            } => {
                json!({"kind":"original", "attempt_id":attempt_id,"recorded_at_ms":recorded_at_ms,"input_language":input_language,"output_language":output_language.encode(),"selection":selection.encode(),"text":text})
            }
            Self::Derived {
                kind,
                original_record_id,
                parent_record_id,
                created_at_ms,
                text,
            } => {
                json!({"kind":kind,"original_record_id":original_record_id,"parent_record_id":parent_record_id,"created_at_ms":created_at_ms,"text":text})
            }
        }
    }
    pub fn original(&self, id: Uuid) -> Uuid {
        match self {
            Self::Original { .. } => id,
            Self::Derived {
                original_record_id, ..
            } => *original_record_id,
        }
    }
    pub fn text(&self) -> &str {
        match self {
            Self::Original { text, .. } | Self::Derived { text, .. } => text,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMode {
    FutureOnly,
    IncludeExisting,
}
