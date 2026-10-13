//! Persisted vocabulary and deterministic, raw-text-preserving postprocessing.
mod processing;
mod storage;
pub use processing::*;
pub use storage::{global_store, routes, routes_with_store, VocabularyStore};

use crate::transcription::{
    ApiError, Correction, CorrectionEntry, ErrorCode, HintEntry, Snippet, SnippetEntry, Vocabulary,
    VocabularyDocument,
};

pub fn empty_vocabulary() -> Vocabulary {
    Vocabulary {
        hints: vec![],
        corrections: vec![],
        snippets: vec![],
    }
}

/// The saved dictionary before anything has been added to it.
pub fn empty_document() -> VocabularyDocument {
    VocabularyDocument::default()
}

impl VocabularyDocument {
    /// What transcription is given: the entries that are switched on, in the
    /// order they are listed. Precedence depends on that order, so a disabled
    /// entry is skipped where it stands rather than moved out of the list.
    pub fn active(&self) -> Vocabulary {
        Vocabulary {
            hints: self
                .hints
                .iter()
                .filter(|h| h.enabled)
                .map(|h| h.text.clone())
                .collect(),
            corrections: self
                .corrections
                .iter()
                .filter(|c| c.enabled)
                .map(|c| Correction {
                    from: c.from.clone(),
                    to: c.to.clone(),
                })
                .collect(),
            snippets: self
                .snippets
                .iter()
                .filter(|s| s.enabled)
                .map(|s| Snippet {
                    phrase: s.phrase.clone(),
                    text: s.text.clone(),
                })
                .collect(),
        }
    }

    /// Reads a dictionary saved before entries could be switched off. Every
    /// entry in one of those was in force, so every entry comes back enabled.
    pub fn from_vocabulary(value: Vocabulary) -> Self {
        Self {
            hints: value
                .hints
                .into_iter()
                .map(|text| HintEntry {
                    id: None,
                    text,
                    enabled: true,
                })
                .collect(),
            corrections: value
                .corrections
                .into_iter()
                .map(|c| CorrectionEntry {
                    id: None,
                    from: c.from,
                    to: c.to,
                    enabled: true,
                })
                .collect(),
            snippets: value
                .snippets
                .into_iter()
                .map(|s| SnippetEntry {
                    id: None,
                    phrase: s.phrase,
                    text: s.text,
                    enabled: true,
                })
                .collect(),
        }
    }
}

fn invalid_entries() -> ApiError {
    ApiError { code: ErrorCode::InvalidRequest, message: "Use nonblank entries without NUL characters, at most 200 per category, 8192 UTF-8 bytes per value and 256 KiB total.".into(), retryable: false, retry_after_ms: None }
}

/// Validate without trimming or normalizing exact replacement values.
pub fn validate(value: &Vocabulary) -> Result<(), ApiError> {
    let valid = |s: &str| !s.trim().is_empty() && s.len() <= 8192 && !s.contains('\0');
    if value.hints.len() > 200
        || value.corrections.len() > 200
        || value.snippets.len() > 200
        || !value.hints.iter().all(|s| valid(s))
        || !value
            .corrections
            .iter()
            .all(|r| valid(&r.from) && valid(&r.to))
        || !value
            .snippets
            .iter()
            .all(|r| valid(&r.phrase) && valid(&r.text))
        || serde_json::to_vec(value).map_or(true, |v| v.len() > storage::MAX_BYTES)
    {
        return Err(invalid_entries());
    }
    Ok(())
}

/// Validates the saved dictionary, switched-off entries included: a disabled
/// entry is still stored, so it is held to the same limits as an active one.
pub fn validate_document(value: &VocabularyDocument) -> Result<(), ApiError> {
    validate(
        &VocabularyDocument {
            hints: value
                .hints
                .iter()
                .map(|h| HintEntry {
                    id: None,
                    text: h.text.clone(),
                    enabled: true,
                })
                .collect(),
            corrections: value
                .corrections
                .iter()
                .map(|c| CorrectionEntry {
                    id: None,
                    from: c.from.clone(),
                    to: c.to.clone(),
                    enabled: true,
                })
                .collect(),
            snippets: value
                .snippets
                .iter()
                .map(|s| SnippetEntry {
                    id: None,
                    phrase: s.phrase.clone(),
                    text: s.text.clone(),
                    enabled: true,
                })
                .collect(),
        }
        .active(),
    )?;
    // The stored document carries the switches as well as the entries, so its
    // own serialized size is what has to fit.
    if serde_json::to_vec(value).map_or(true, |v| v.len() > storage::MAX_BYTES) {
        return Err(invalid_entries());
    }
    // A portable identity names one entry. The fixed setting identities of the
    // sync contract are never dictionary entries.
    let mut seen = std::collections::BTreeSet::new();
    for id in value.ids().into_iter().flatten() {
        if !seen.insert(id) || is_reserved_id(&id) {
            return Err(ApiError {
                code: ErrorCode::InvalidRequest,
                message: "Each dictionary entry needs its own identity.".into(),
                retryable: false,
                retry_after_ms: None,
            });
        }
    }
    Ok(())
}

/// The sync contract's fixed setting identities (`00000000-0000-4000-8000-…`).
pub fn is_reserved_id(id: &uuid::Uuid) -> bool {
    id.as_bytes()[..10] == [0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0]
}

impl VocabularyDocument {
    /// Every entry's portable identity, in document order: hints, corrections,
    /// then snippets.
    pub fn ids(&self) -> Vec<Option<uuid::Uuid>> {
        self.hints
            .iter()
            .map(|h| h.id)
            .chain(self.corrections.iter().map(|c| c.id))
            .chain(self.snippets.iter().map(|s| s.id))
            .collect()
    }
    /// Gives each entry without one a fresh random identity, once. Returns
    /// whether anything was assigned, so the caller persists it before use.
    pub fn assign_ids(&mut self) -> bool {
        let mut assigned = false;
        let mut fresh = |id: &mut Option<uuid::Uuid>| {
            if id.is_none() {
                *id = Some(uuid::Uuid::new_v4());
                assigned = true;
            }
        };
        self.hints.iter_mut().for_each(|h| fresh(&mut h.id));
        self.corrections.iter_mut().for_each(|c| fresh(&mut c.id));
        self.snippets.iter_mut().for_each(|s| fresh(&mut s.id));
        assigned
    }
}

#[cfg(test)]
mod tests;
