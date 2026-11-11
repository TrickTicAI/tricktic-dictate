//! MW-123: what the polish learns from the user's corrections. A History
//! correction proposes new terms and mishearing pairs; he accepts or declines
//! each. Accepted terms join the dictionary's words, accepted pairs are kept
//! here for the polish, and declined suggestions are remembered so they are
//! never proposed again. Kept beside the dictionary on this device; the synced
//! dictionary document is unchanged.
use super::{validate_document, VocabularyStore};
use crate::transcription::{ApiError, Envelope, ErrorCode, ErrorEnvelope, HintEntry};
use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

const MAX_PAIRS: usize = 200;
const MAX_DECLINED: usize = 2_000;
const MAX_VALUE_BYTES: usize = 200;
/// Longest mishearing, in words on either side.
const MAX_SPAN_WORDS: usize = 3;
const MAX_SUGGESTIONS: usize = 10;
/// Longer texts are not aligned word by word; a dictation is far shorter.
const MAX_ALIGN_WORDS: usize = 3_000;

/// Words recognition hears in place of one of the speaker's terms, such as
/// "marriage" for "merge". `when` optionally says in which context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mishearing {
    pub heard: String,
    pub meant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Learning {
    #[serde(default)]
    pub mishearings: Vec<Mishearing>,
    /// Keys of declined suggestions (`Suggestion::key`).
    #[serde(default)]
    pub declined: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Suggestion {
    /// A word or name to add to the dictionary's words.
    Term { text: String },
    /// Recognition heard `heard` where `meant` was said.
    Mishearing { heard: String, meant: String },
}

impl Suggestion {
    /// The same suggestion has the same key whatever its letter case.
    pub fn key(&self) -> String {
        match self {
            Self::Term { text } => format!("term:{}", fold(text)),
            Self::Mishearing { heard, meant } => {
                format!("mishearing:{}\u{2192}{}", fold(heard), fold(meant))
            }
        }
    }
}

fn fold(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// One word of a text: as written, and folded for comparison.
struct Word<'a> {
    text: &'a str,
    folded: String,
    sentence_start: bool,
}

fn words(text: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut sentence_start = true;
    for raw in text.split_whitespace() {
        let trimmed = raw.trim_matches(|c: char| !c.is_alphanumeric());
        if !trimmed.is_empty() {
            out.push(Word {
                text: trimmed,
                folded: trimmed.to_lowercase(),
                sentence_start,
            });
        }
        sentence_start =
            raw.ends_with(['.', '!', '?', ':']) || trimmed.is_empty() && sentence_start;
    }
    out
}

/// A name or identifier rather than an everyday word: inner capitals or
/// digits, an abbreviation, or a capital away from the start of a sentence.
fn term_like(word: &Word) -> bool {
    let mut chars = word.text.chars();
    let first_upper = chars.next().is_some_and(char::is_uppercase);
    let inner_upper = chars.any(char::is_uppercase);
    inner_upper
        || word.text.chars().any(|c| c.is_ascii_digit())
            && word.text.chars().any(char::is_alphabetic)
        || first_upper && !word.sentence_start
}

/// Pairs of word ranges that differ between `heard` and `corrected`, between
/// words both share (longest common subsequence of folded words).
fn differences<'a>(
    heard: &'a [Word<'a>],
    corrected: &'a [Word<'a>],
) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let (n, m) = (heard.len(), corrected.len());
    let mut table = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if heard[i].folded == corrected[j].folded {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut from_i, mut from_j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if i < n && j < m && heard[i].folded == corrected[j].folded {
            if (from_i, from_j) != (i, j) {
                out.push((from_i..i, from_j..j));
            }
            i += 1;
            j += 1;
            (from_i, from_j) = (i, j);
        } else if j < m && (i == n || table[at(i, j + 1)] >= table[at(i + 1, j)]) {
            j += 1;
        } else {
            i += 1;
        }
    }
    if (from_i, from_j) != (n, m) {
        out.push((from_i..n, from_j..m));
    }
    out
}

fn joined(words: &[Word]) -> String {
    words.iter().map(|w| w.text).collect::<Vec<_>>().join(" ")
}

/// What a correction teaches: each short replaced span becomes a mishearing
/// pair, and each name or identifier the user wrote that recognition did not
/// (including a change of letter case alone) becomes a term. Suggestions they
/// already declined, terms already in `known_terms` and pairs already learned
/// are left out.
pub fn suggest(
    heard: &str,
    corrected: &str,
    known_terms: &[String],
    learning: &Learning,
) -> Vec<Suggestion> {
    let (heard, corrected) = (words(heard), words(corrected));
    if heard.len() > MAX_ALIGN_WORDS || corrected.len() > MAX_ALIGN_WORDS {
        return vec![];
    }
    let mut found = Vec::new();
    let term = |span: &[Word], found: &mut Vec<Suggestion>| {
        if span.iter().any(term_like) {
            found.push(Suggestion::Term { text: joined(span) });
        }
    };
    let mut changed = vec![false; corrected.len()];
    for (was, now) in differences(&heard, &corrected) {
        changed[now.clone()].iter_mut().for_each(|c| *c = true);
        let (was, now) = (&heard[was], &corrected[now]);
        if now.is_empty() || now.len() > MAX_SPAN_WORDS {
            continue;
        }
        // A spoken identifier ("two thirty six") can be longer than a pair,
        // but what was written is still a term.
        if !was.is_empty() && was.len() <= MAX_SPAN_WORDS {
            found.push(Suggestion::Mishearing {
                heard: joined(was),
                meant: joined(now),
            });
        }
        term(now, &mut found);
    }
    // Same word, new letter case: "claude" written as "Claude".
    for (word, changed) in corrected.iter().zip(changed) {
        if !changed
            && heard
                .iter()
                .any(|h| h.folded == word.folded && h.text != word.text)
        {
            term(std::slice::from_ref(word), &mut found);
        }
    }
    let known: std::collections::BTreeSet<String> = known_terms
        .iter()
        .map(|t| Suggestion::Term { text: t.clone() }.key())
        .chain(learning.mishearings.iter().map(|p| {
            Suggestion::Mishearing {
                heard: p.heard.clone(),
                meant: p.meant.clone(),
            }
            .key()
        }))
        .chain(learning.declined.iter().cloned())
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    found
        .into_iter()
        .filter(|s| {
            let key = s.key();
            !known.contains(&key) && seen.insert(key)
        })
        .take(MAX_SUGGESTIONS)
        .collect()
}

fn invalid(message: &str) -> ApiError {
    ApiError {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
        retryable: false,
        retry_after_ms: None,
    }
}
fn storage_error() -> ApiError {
    ApiError {
        code: ErrorCode::StorageFailed,
        message: "Learned vocabulary is unavailable; nothing saved was intentionally removed."
            .into(),
        retryable: true,
        retry_after_ms: None,
    }
}

fn valid_value(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_VALUE_BYTES
        && !value.chars().any(char::is_control)
}

pub fn validate(learning: &Learning) -> Result<(), ApiError> {
    let pairs_valid = learning.mishearings.iter().all(|p| {
        valid_value(&p.heard)
            && valid_value(&p.meant)
            && p.when.as_deref().is_none_or(valid_value)
            && fold(&p.heard) != fold(&p.meant)
    });
    if learning.mishearings.len() > MAX_PAIRS
        || learning.declined.len() > MAX_DECLINED
        || !pairs_valid
        || learning
            .declined
            .iter()
            .any(|k| k.len() > 2 * MAX_VALUE_BYTES + 16)
    {
        return Err(invalid(
            "Use at most 200 mishearings of short, different words.",
        ));
    }
    Ok(())
}

pub struct LearningStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl LearningStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }
    async fn read(&self) -> Result<Learning, ApiError> {
        match tokio::fs::read(&self.path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| storage_error()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Learning::default()),
            Err(_) => Err(storage_error()),
        }
    }
    pub async fn load(&self) -> Result<Learning, ApiError> {
        let _guard = self.lock.lock().await;
        self.read().await
    }
    /// The pairs a polish is given. A store that cannot be read gives none:
    /// polishing never waits on or fails because of what it learned.
    pub async fn mishearings(&self) -> Vec<Mishearing> {
        self.load().await.map(|l| l.mishearings).unwrap_or_default()
    }
    async fn update(
        &self,
        change: impl FnOnce(&mut Learning) -> Result<(), ApiError>,
    ) -> Result<Learning, ApiError> {
        let _guard = self.lock.lock().await;
        let mut learning = self.read().await?;
        change(&mut learning)?;
        validate(&learning)?;
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            crate::recordings::disk::private_dir(parent).await?;
        }
        crate::recordings::disk::write_json(&self.path, &learning).await?;
        Ok(learning)
    }
    /// Replaces the learned pairs (the dictionary's editor); declined
    /// suggestions stay declined.
    pub async fn set_mishearings(&self, pairs: Vec<Mishearing>) -> Result<Learning, ApiError> {
        self.update(|l| {
            l.mishearings = pairs;
            Ok(())
        })
        .await
    }
    /// Accepting a term adds it to the dictionary's words; accepting a pair
    /// keeps it for the polish. Either way, or declined, it is not proposed
    /// again.
    pub async fn decide(
        &self,
        suggestion: &Suggestion,
        accept: bool,
        dictionary: &VocabularyStore,
    ) -> Result<Learning, ApiError> {
        match (accept, suggestion) {
            (true, Suggestion::Term { text }) => {
                if !valid_value(text) {
                    return Err(invalid("A term must be a short word or name."));
                }
                add_term(dictionary, text).await?;
                self.load().await
            }
            (true, Suggestion::Mishearing { heard, meant }) => {
                self.update(|l| {
                    let key = suggestion.key();
                    l.declined.retain(|k| *k != key);
                    if !l.mishearings.iter().any(|p| {
                        Suggestion::Mishearing {
                            heard: p.heard.clone(),
                            meant: p.meant.clone(),
                        }
                        .key()
                            == key
                    }) {
                        l.mishearings.push(Mishearing {
                            heard: heard.clone(),
                            meant: meant.clone(),
                            when: None,
                        });
                    }
                    Ok(())
                })
                .await
            }
            (false, _) => {
                self.update(|l| {
                    let key = suggestion.key();
                    if !l.declined.contains(&key) {
                        if l.declined.len() == MAX_DECLINED {
                            l.declined.remove(0);
                        }
                        l.declined.push(key);
                    }
                    Ok(())
                })
                .await
            }
        }
    }
}

/// Adds an enabled word to the dictionary unless it is already listed.
async fn add_term(dictionary: &VocabularyStore, text: &str) -> Result<(), ApiError> {
    for _ in 0..3 {
        let (mut document, tag) = dictionary.load_tagged().await?;
        if document.hints.iter().any(|h| fold(&h.text) == fold(text)) {
            return Ok(());
        }
        document.hints.push(HintEntry {
            id: None,
            text: text.trim().to_owned(),
            enabled: true,
        });
        validate_document(&document)?;
        if dictionary.save_if(&document, Some(&tag)).await?.is_ok() {
            return Ok(());
        }
    }
    Err(ApiError {
        code: ErrorCode::IdempotencyConflict,
        message: "The dictionary kept changing; the term was not added. Try again.".into(),
        retryable: true,
        retry_after_ms: None,
    })
}

/// The process's store, beside the configured dictionary file.
pub fn global() -> Arc<LearningStore> {
    static STORE: std::sync::OnceLock<Arc<LearningStore>> = std::sync::OnceLock::new();
    STORE
        .get_or_init(|| {
            Arc::new(LearningStore::new(
                super::storage::configured_path().with_file_name("vocabulary-learning.json"),
            ))
        })
        .clone()
}

#[derive(Clone)]
struct Stores {
    learning: Arc<LearningStore>,
    dictionary: Arc<VocabularyStore>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SuggestRequest {
    heard: String,
    corrected: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    suggestion: Suggestion,
    accept: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pairs {
    mishearings: Vec<Mishearing>,
}

type Failure = (StatusCode, Json<ErrorEnvelope>);
fn failure(error: ApiError) -> Failure {
    let status = match error.code {
        ErrorCode::InvalidRequest => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(ErrorEnvelope {
            api_version: crate::transcription::ApiVersion::V1,
            error,
        }),
    )
}
fn body<T>(payload: Result<Json<Envelope<T>>, JsonRejection>) -> Result<T, Failure> {
    payload
        .map(|Json(e)| e.data)
        .map_err(|_| failure(invalid("Expected a version-one request.")))
}

async fn read(State(stores): State<Stores>) -> Result<Json<Envelope<Learning>>, Failure> {
    stores
        .learning
        .load()
        .await
        .map(|l| Json(Envelope::new(l)))
        .map_err(failure)
}
async fn replace(
    State(stores): State<Stores>,
    payload: Result<Json<Envelope<Pairs>>, JsonRejection>,
) -> Result<Json<Envelope<Learning>>, Failure> {
    let pairs = body(payload)?;
    stores
        .learning
        .set_mishearings(pairs.mishearings)
        .await
        .map(|l| Json(Envelope::new(l)))
        .map_err(failure)
}
async fn suggestions(
    State(stores): State<Stores>,
    payload: Result<Json<Envelope<SuggestRequest>>, JsonRejection>,
) -> Result<Json<Envelope<Vec<Suggestion>>>, Failure> {
    let request = body(payload)?;
    if request.heard.len() > 200_000 || request.corrected.len() > 200_000 {
        return Err(failure(invalid("The texts are too long to compare.")));
    }
    let learning = stores.learning.load().await.map_err(failure)?;
    let terms: Vec<String> = stores
        .dictionary
        .load()
        .await
        .map_err(failure)?
        .hints
        .into_iter()
        .map(|h| h.text)
        .collect();
    Ok(Json(Envelope::new(suggest(
        &request.heard,
        &request.corrected,
        &terms,
        &learning,
    ))))
}
async fn decide(
    State(stores): State<Stores>,
    payload: Result<Json<Envelope<Decision>>, JsonRejection>,
) -> Result<Json<Envelope<Learning>>, Failure> {
    let decision = body(payload)?;
    stores
        .learning
        .decide(&decision.suggestion, decision.accept, &stores.dictionary)
        .await
        .map(|l| Json(Envelope::new(l)))
        .map_err(failure)
}

pub fn routes_with(learning: Arc<LearningStore>, dictionary: Arc<VocabularyStore>) -> Router {
    Router::new()
        .route("/api/v1/vocabulary/learning", get(read).put(replace))
        .route("/api/v1/vocabulary/suggestions", post(suggestions))
        .route("/api/v1/vocabulary/suggestions/decision", post(decide))
        .layer(axum::extract::DefaultBodyLimit::max(512 * 1024))
        .with_state(Stores {
            learning,
            dictionary,
        })
}

pub fn routes() -> Router {
    routes_with(global(), super::global_store())
}

#[cfg(test)]
mod tests;
