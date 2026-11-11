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
            if expected.is_whitespace() {
                // Recognition may write "My ID" as "myID" or "my, ID".
                while trigger.peek().is_some_and(|c| c.is_whitespace()) {
                    trigger.next();
                }
                while source
                    .peek()
                    .is_some_and(|(_, c)| c.is_whitespace() || matches!(c, ',' | '-'))
                {
                    source.next();
                }
                continue;
            }
            let (at, actual) = source.next()?;
            if actual.to_lowercase().eq(expected.to_lowercase()) {
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

/// Snippets with only spacing or punctuation between them. A comma between two
/// values of plain words goes: "first name, last name" is one name, while an
/// address or email keeps it. The raw transcript keeps what was returned.
fn snippet_run(values: &[String], gaps: &[String], out: &mut Vec<TextSegment>) {
    let words = |value: &str| value.chars().all(|c| c.is_alphabetic() || c == ' ');
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            let plain = gaps[i - 1].trim() == "," && words(&values[i - 1]) && words(value);
            out.push(TextSegment::Editable(if plain {
                " ".into()
            } else {
                gaps[i - 1].clone()
            }));
        }
        out.push(TextSegment::Exact(value.clone()));
    }
}
fn join_adjacent_snippets(segments: Vec<(TextSegment, bool)>) -> Vec<TextSegment> {
    let mut out = vec![];
    let (mut values, mut gaps): (Vec<String>, Vec<String>) = (vec![], vec![]);
    let mut gap: Option<String> = None;
    for (segment, snippet) in segments {
        match segment {
            TextSegment::Exact(value) if snippet => {
                gaps.extend(gap.take());
                values.push(value);
            }
            TextSegment::Editable(text)
                if !values.is_empty()
                    && gap.is_none()
                    && !text.contains('\n')
                    && !text.chars().any(char::is_alphanumeric) =>
            {
                gap = Some(text);
            }
            other => {
                snippet_run(&values, &gaps, &mut out);
                values.clear();
                gaps.clear();
                out.extend(gap.take().map(TextSegment::Editable));
                out.push(other);
            }
        }
    }
    snippet_run(&values, &gaps, &mut out);
    out.extend(gap.map(TextSegment::Editable));
    out
}

/// Snippets ignore capitalization and tolerate how recognition spaces their words;
/// corrections stay literal.
/// At each position: snippets before corrections, longest phrase, then list order.
/// Inserted output is never rescanned, so rules cannot cascade or rewrite snippets.
pub fn process(raw: &str, vocabulary: &Vocabulary) -> Result<ProcessedText, ApiError> {
    validate(vocabulary)?;
    process_rules(raw, vocabulary, true)
}

fn process_rules(
    raw: &str,
    vocabulary: &Vocabulary,
    snippets: bool,
) -> Result<ProcessedText, ApiError> {
    let mut rules: Vec<_> = vocabulary
        .snippets
        .iter()
        .filter(|_| snippets)
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
        if let Some((end, text, snippet)) = rules.iter().find_map(|(kind, phrase, text)| {
            match_end(raw, offset, phrase, *kind == 0).map(|end| (end, text, *kind == 0))
        }) {
            if start < offset {
                segments.push((TextSegment::Editable(raw[start..offset].into()), false));
            }
            segments.push((TextSegment::Exact((*text).into()), snippet));
            offset = end;
            start = offset;
        } else {
            offset += raw[offset..].chars().next().expect("in bounds").len_utf8();
        }
    }
    if start < raw.len() {
        segments.push((TextSegment::Editable(raw[start..].into()), false));
    }
    // Decide from the original unmatched speech, not replacement values: an
    // exact correction must not disguise an ordinary sentence as snippet-only.
    if snippets
        && segments.iter().any(|(s, snippet)| {
            !snippet
                && match s {
                    TextSegment::Editable(s) => s.chars().any(char::is_alphanumeric),
                    TextSegment::Exact(_) => true,
                }
        })
    {
        return process_rules(raw, vocabulary, false);
    }
    Ok(ProcessedText {
        raw_text: raw.into(),
        segments: join_adjacent_snippets(segments),
    })
}

#[cfg(test)]
mod spoken_snippet_tests {
    use super::*;
    use crate::transcription::Snippet;
    fn text(raw: &str) -> String {
        let snippet = |phrase: &str, text: &str| Snippet {
            phrase: phrase.into(),
            text: text.into(),
        };
        let vocabulary = Vocabulary {
            snippets: vec![
                snippet("First Name", "Ada"),
                snippet("Last Name", "Lovelace"),
                snippet("My ID", "12345"),
            ],
            hints: vec![],
            corrections: vec![],
        };
        process(raw, &vocabulary).unwrap().text()
    }
    #[test]
    fn recognition_spacing_does_not_hide_a_snippet() {
        for raw in [" myID", " My ID", " my, id", " my-id", " MY  ID."] {
            assert_eq!(text(raw), "12345", "{raw}");
        }
        assert_eq!(text("the myIDs here"), "the myIDs here");
        assert_eq!(text("army id"), "army id");
    }
    #[test]
    fn a_spoken_name_is_one_value() {
        assert_eq!(text(" First name, last name"), "Ada Lovelace");
        assert_eq!(text("first name first name first name"), "Ada Ada Ada");
    }
    #[test]
    fn deliberate_repeats_and_separated_values_stay_as_dictated() {
        assert_eq!(text("first name first name"), "Ada Ada");
        assert_eq!(text("first name. first name! first name?"), "Ada. Ada! Ada");
        assert_eq!(text("first name, my id"), "Ada, 12345");
        assert_eq!(
            text("first name and first name"),
            "first name and first name"
        );
        assert_eq!(text("first name\nlast name"), "Ada\nLovelace");
        assert_eq!(
            text("hi first name, last name, ok"),
            "hi first name, last name, ok"
        );
    }
}
