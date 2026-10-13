use super::validate;
use crate::transcription::{ApiError, ErrorCode, Vocabulary};

/// Adapter declares actual model support; this module never guesses from provider name.
#[derive(Debug, PartialEq, Eq)]
pub enum HintPlan {
    Empty,
    Supported(Vec<String>),
    Unsupported {
        requested: usize,
        message: &'static str,
    },
}
pub fn prepare_hints(vocabulary: &Vocabulary, supports_hints: bool) -> Result<HintPlan, ApiError> {
    validate(vocabulary)?;
    Ok(if vocabulary.hints.is_empty() {
        HintPlan::Empty
    } else if supports_hints {
        HintPlan::Supported(vocabulary.hints.clone())
    } else {
        HintPlan::Unsupported { requested: vocabulary.hints.len(), message: "This model does not support vocabulary hints. Exact corrections and snippets still apply; recognition is not guaranteed." }
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextSegment {
    Editable(String),
    Exact(String),
}
#[derive(Clone, Debug)]
pub struct ProcessedText {
    pub raw_text: String,
    segments: Vec<TextSegment>,
}
impl ProcessedText {
    pub fn segments(&self) -> &[TextSegment] {
        &self.segments
    }
    pub fn text(&self) -> String {
        self.join(
            self.segments
                .iter()
                .map(|s| match s {
                    TextSegment::Editable(s) | TextSegment::Exact(s) => s.as_str(),
                })
                .collect(),
        )
    }
    /// Dictating nothing but snippet triggers asks for those stored values alone.
    /// Recognition's own leading space and sentence mark around a trigger are not
    /// spoken content, and such a value is usually typed into a field. One spoken
    /// word outside a snippet makes this an ordinary dictation again.
    fn snippet_only(&self) -> bool {
        self.segments
            .iter()
            .any(|s| matches!(s, TextSegment::Exact(_)))
            && !self.segments.iter().any(
                |s| matches!(s, TextSegment::Editable(s) if s.chars().any(char::is_alphanumeric)),
            )
    }
    /// One value per segment, in order. Exact values keep every character they
    /// were given, including their own surrounding spaces and punctuation.
    fn join(&self, values: Vec<&str>) -> String {
        let mut span = 0..values.len();
        if self.snippet_only() {
            if matches!(self.segments.first(), Some(TextSegment::Editable(_))) {
                span.start += 1;
            }
            if matches!(self.segments.last(), Some(TextSegment::Editable(_))) {
                span.end -= 1;
            }
        }
        values[span].concat()
    }
    /// Send only these spans to cleanup, separately and in order. Exact values
    /// never enter the rewriting model. Keep this object's raw text unchanged.
    pub fn cleanup_inputs(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                TextSegment::Editable(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }
    /// Caller must supply one successful result per editable span, in order.
    /// On any cleanup failure retain `text()` and the independent raw transcript.
    pub fn restore_cleaned(&self, cleaned: &[String]) -> Result<String, ApiError> {
        if cleaned.len() != self.cleanup_inputs().len() {
            return Err(ApiError {
                code: ErrorCode::InvalidProviderResponse,
                message:
                    "Cleanup did not return every editable span; keep the original processed text."
                        .into(),
                retryable: false,
                retry_after_ms: None,
            });
        }
        let mut replacements = cleaned.iter();
        Ok(self.join(
            self.segments
                .iter()
                .map(|s| match s {
                    TextSegment::Exact(s) => s.as_str(),
                    TextSegment::Editable(_) => {
                        replacements.next().expect("count checked").as_str()
                    }
                })
                .collect(),
        ))
    }
}

// Marks are word continuations, including Hebrew niqqud/cantillation. Conservative
// coverage of combining blocks prevents matches inside accented/pointed words.
fn word(c: char) -> bool {
    c.is_alphanumeric()
        || c == '_'
        || matches!(c, '\u{0300}'..='\u{036f}' | '\u{0591}'..='\u{05bd}' | '\u{05bf}' | '\u{05c1}'..='\u{05c2}' | '\u{05c4}'..='\u{05c5}' | '\u{05c7}' | '\u{0610}'..='\u{061a}' | '\u{064b}'..='\u{065f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{200c}'..='\u{200d}' | '\u{203f}'..='\u{2040}' | '\u{20d0}'..='\u{20ff}' | '\u{fe20}'..='\u{fe2f}')
}
fn match_end(raw: &str, offset: usize, phrase: &str, snippet: bool) -> Option<usize> {
    if raw[..offset].chars().next_back().is_some_and(word) {
        return None;
    }
    let end = if snippet {
        // Speech has no letter case. Consume original UTF-8 characters so case
        // conversion never changes the offsets used to retain untouched text.
        let mut source = raw[offset..].char_indices().peekable();
        let mut trigger = phrase.chars().peekable();
        let mut end = offset;
        while let Some(expected) = trigger.next() {
            let (at, actual) = source.next()?;
            if expected.is_whitespace() && actual.is_whitespace() {
                end = offset + at + actual.len_utf8();
                while trigger.peek().is_some_and(|c| c.is_whitespace()) {
                    trigger.next();
                }
                while source.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                    let (at, c) = source.next()?;
                    end = offset + at + c.len_utf8();
                }
            } else if actual.to_lowercase().eq(expected.to_lowercase()) {
                end = offset + at + actual.len_utf8();
            } else {
                return None;
            }
        }
        end
    } else if raw[offset..].starts_with(phrase) {
        offset + phrase.len()
    } else {
        return None;
    };
    (!raw[end..].chars().next().is_some_and(word)).then_some(end)
}

/// Snippets ignore capitalization and runs of whitespace; corrections stay literal.
/// At each position: snippets before corrections, longest phrase, then list order.
/// Inserted output is never rescanned, so rules cannot cascade or rewrite snippets.
pub fn process(raw: &str, vocabulary: &Vocabulary) -> Result<ProcessedText, ApiError> {
    validate(vocabulary)?;
    let mut rules: Vec<_> = vocabulary
        .snippets
        .iter()
        .map(|r| (0, r.phrase.as_str(), r.text.as_str()))
        .chain(
            vocabulary
                .corrections
                .iter()
                .map(|r| (1, r.from.as_str(), r.to.as_str())),
        )
        .collect();
    rules.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.len().cmp(&a.1.len())));
    let mut segments = vec![];
    let mut offset = 0;
    let mut start = 0;
    while offset < raw.len() {
        if let Some((end, text)) = rules.iter().find_map(|(kind, phrase, text)| {
            match_end(raw, offset, phrase, *kind == 0).map(|end| (end, text))
        }) {
            if start < offset {
                segments.push(TextSegment::Editable(raw[start..offset].into()));
            }
            segments.push(TextSegment::Exact((*text).into()));
            offset = end;
            start = offset;
        } else {
            offset += raw[offset..].chars().next().expect("in bounds").len_utf8();
        }
    }
    if start < raw.len() {
        segments.push(TextSegment::Editable(raw[start..].into()));
    }
    Ok(ProcessedText {
        raw_text: raw.into(),
        segments,
    })
}
