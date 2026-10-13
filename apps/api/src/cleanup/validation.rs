//! Conservative dictation checks, not a proof of semantic equivalence.
//! See docs/testing/cleanup-meaning.md for allowed transformations and limits.
mod polish;
use super::failure;
use crate::transcription::{ApiError, ErrorCode};

const CUES: &[(&str, &str)] = &[
    ("open parentheses", "("),
    ("close parentheses", ")"),
    ("open parenthesis", "("),
    ("close parenthesis", ")"),
    ("open quote", "\"“"),
    ("close quote", "\"”"),
    ("double quote", "\"“”"),
    ("single quote", "'‘’"),
    ("new paragraph", "\n"),
    ("new line", "\n"),
    ("dot dot dot", "…"),
    ("question mark", "?"),
    ("exclamation mark", "!"),
    ("פתח מירכאות", "\"“"),
    ("סגור מירכאות", "\"”"),
    ("פתח מרכאות", "\"“"),
    ("סגור מרכאות", "\"”"),
    ("פתח סוגריים", "("),
    ("סגור סוגריים", ")"),
    ("פסקה חדשה", "\n"),
    ("שורה חדשה", "\n"),
    ("סימן שאלה", "?"),
    ("סימן קריאה", "!"),
    ("שלוש נקודות", "…"),
    ("נקודתיים", ":"),
    ("פסיק", ","),
    ("נקודה", "."),
    ("colon", ":"),
    ("comma", ","),
    ("ellipsis", "…"),
    ("apostrophe", "'’"),
];
#[derive(Default)]
struct Tokens {
    values: Vec<String>,
    // Punctuation/whitespace before each value, and after the last value.
    gaps: Vec<String>,
}
impl Tokens {
    fn push(&mut self, value: String) {
        self.values.push(value);
        self.gaps.push(String::new());
    }
}
fn digit(c: char) -> bool {
    c.is_ascii_digit()
}
fn letter(c: char) -> bool {
    c.is_alphabetic()
        || ('\u{0300}'..='\u{036f}').contains(&c)
        || ('\u{0591}'..='\u{05c7}').contains(&c)
}
fn formatting(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | '\''
                | '’'
                | '‘'
                | '"'
                | '“'
                | '”'
                | '„'
                | '«'
                | '»'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '…'
                | '-'
                | '–'
                | '—'
        )
}
fn numeric_start(chars: &[char], i: usize) -> bool {
    chars.get(i).is_some_and(|c| {
        digit(*c) || matches!(c, '.' | ',') && chars.get(i + 1).is_some_and(|c| digit(*c))
    })
}
fn tokens(text: &str) -> Tokens {
    let mut result = Tokens {
        values: vec![],
        gaps: vec![String::new()],
    };
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // URLs/addresses are case-sensitive exact non-whitespace values.
        if !c.is_whitespace() && (i == 0 || chars[i - 1].is_whitespace()) {
            let end = (i..chars.len())
                .find(|&n| chars[n].is_whitespace())
                .unwrap_or(chars.len());
            let chunk: String = chars[i..end].iter().collect();
            if chunk.contains("://") || chunk.starts_with("www.") || chunk.contains('@') {
                result.push(format!("x:{chunk}"));
                i = end;
                continue;
            }
        }
        // A spaced sign remains protected too; a numeric bullet is ambiguous.
        if matches!(c, '-' | '−' | '–' | '—')
            && !numeric_start(&chars, i + 1)
            && (i + 1..chars.len())
                .find(|&n| !chars[n].is_whitespace())
                .is_some_and(|n| numeric_start(&chars, n))
        {
            result.push("s:-".into());
            i += 1;
            continue;
        }
        if numeric_start(&chars, i)
            || matches!(c, '+' | '-' | '−' | '–' | '—') && numeric_start(&chars, i + 1)
        {
            let start = i;
            i += 1;
            while i < chars.len() {
                if digit(chars[i])
                    || matches!(chars[i], '.' | ',' | ':' | '/' | '-')
                        && chars.get(i + 1).is_some_and(|c| digit(*c))
                {
                    i += 1;
                } else {
                    break;
                }
            }
            let value: String = chars[start..i]
                .iter()
                .map(|c| {
                    if matches!(c, '−' | '–' | '—') {
                        '-'
                    } else {
                        *c
                    }
                })
                .collect();
            result.push(format!("n:{value}"));
        } else if letter(c) {
            let start = i;
            i += 1;
            while i < chars.len()
                && (letter(chars[i])
                    || chars[i].is_numeric()
                    || matches!(chars[i], '\'' | '’')
                        && chars.get(i + 1).is_some_and(|c| letter(*c)))
            {
                i += 1;
            }
            let value: String = chars[start..i]
                .iter()
                .collect::<String>()
                .replace('’', "'")
                .to_lowercase();
            let expanded = match value.as_str() {
                // A spelling split, not a newly introduced negative instruction.
                "nevermind" => "never mind",
                "don't" => "do not",
                "doesn't" => "does not",
                "didn't" => "did not",
                "can't" | "cannot" => "can not",
                "won't" => "will not",
                "isn't" => "is not",
                "aren't" => "are not",
                "wasn't" => "was not",
                "weren't" => "were not",
                "shouldn't" => "should not",
                "wouldn't" => "would not",
                "couldn't" => "could not",
                "mustn't" => "must not",
                "hasn't" => "has not",
                "haven't" => "have not",
                "hadn't" => "had not",
                _ => &value,
            };
            for word in expanded.split_whitespace() {
                result.push(format!("w:{word}"));
            }
        } else if !formatting(c) {
            result.push(format!("s:{c}"));
            i += 1;
        } else {
            result.gaps.last_mut().unwrap().push(c);
            i += 1;
        }
    }
    // Only unambiguous sign/decimal cues next to literal digits are normalized.
    let mut i = 0;
    while i < result.values.len() {
        if matches!(
            result.values[i].as_str(),
            "w:minus" | "w:מינוס" | "w:plus" | "w:פלוס"
        ) && result
            .values
            .get(i + 1)
            .is_some_and(|v| v.starts_with("n:"))
            && result.gaps[i + 1].trim().is_empty()
        {
            let sign = if matches!(result.values[i].as_str(), "w:minus" | "w:מינוס") {
                "-"
            } else {
                "+"
            };
            result.values[i] = format!("n:{sign}{}", &result.values[i + 1][2..]);
            result.values.remove(i + 1);
            result.gaps.remove(i + 1);
        }
        if result.values[i].starts_with("n:")
            && result
                .values
                .get(i + 1)
                .is_some_and(|v| matches!(v.as_str(), "w:point" | "w:נקודה"))
            && result
                .values
                .get(i + 2)
                .is_some_and(|v| v.starts_with("n:") && v[2..].chars().all(digit))
            && result.gaps[i + 1].trim().is_empty()
            && result.gaps[i + 2].trim().is_empty()
        {
            result.values[i] = format!("{}.{}", result.values[i], &result.values[i + 2][2..]);
            result.values.drain(i + 1..i + 3);
            result.gaps.drain(i + 1..i + 3);
        }
        i += 1;
    }
    result
}
/// Only a recognized cue rendered at the same content boundary may disappear.
/// Unknown rewording, self-corrections and ambiguous number formatting fail closed.
pub(super) fn preserves(original: &str, output: &str, spoken: bool, fillers: bool) -> bool {
    if output.trim().is_empty()
        || output.contains('\0')
        || output.len() > original.len().saturating_mul(8) + 8192
    {
        return false;
    }
    let source = tokens(original);
    let target = tokens(output);
    let (mut i, mut j) = (0, 0);
    while i < source.values.len() {
        if target.values.get(j) == Some(&source.values[i]) {
            i += 1;
            j += 1;
            continue;
        }
        if fillers && matches!(source.values[i].as_str(), "w:um" | "w:uh" | "w:אממ") {
            i += 1;
            continue;
        }
        // Literal/negated discussion of a cue is ambiguous: keep its words.
        let literal = source.values[i.saturating_sub(4)..i].iter().any(|w| {
            matches!(
                w.as_str(),
                "w:not"
                    | "w:no"
                    | "w:never"
                    | "w:לא"
                    | "w:אל"
                    | "w:בלי"
                    | "w:word"
                    | "w:literal"
                    | "w:called"
                    | "w:מילה"
                    | "w:המילה"
            )
        });
        let cue = (spoken && !literal)
            .then(|| {
                CUES.iter().find(|(cue, marks)| {
                    let cue: Vec<String> =
                        cue.split_whitespace().map(|w| format!("w:{w}")).collect();
                    source.values[i..].starts_with(&cue)
                        && marks.chars().any(|c| target.gaps[j].contains(c))
                })
            })
            .flatten();
        if let Some((cue, _)) = cue {
            i += cue.split_whitespace().count();
        } else {
            return false;
        }
    }
    j == target.values.len()
}
pub(super) fn validate(
    original: &str,
    output: &str,
    spoken: bool,
    fillers: bool,
) -> Result<(), ApiError> {
    if preserves(original, output, spoken, fillers) {
        Ok(())
    } else {
        Err(failure(ErrorCode::InvalidProviderResponse, "Cleanup changed protected content or could not be validated. Its output was rejected; the original text is available.", true))
    }
}

/// Super permits grammatical rewrites; legacy cleanup keeps its stricter guard.
pub(super) fn validate_polish(
    original: &str,
    output: &str,
    preferences: &super::modes::Formatting,
) -> Result<(), ApiError> {
    if polish::preserves(original, output, preferences) {
        Ok(())
    } else {
        Err(failure(ErrorCode::InvalidProviderResponse, "Polishing changed protected values, negation or language, or returned unrelated text. The original text is available.", true))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_polishing_allows_requested_edits_and_keeps_protected_values() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/cleanup/desktop-polish.json"
        ))
        .unwrap();
        for case in cases["cases"].as_array().unwrap() {
            let preferences = crate::cleanup::modes::Formatting {
                spoken_punctuation: case["spoken"].as_bool().unwrap(),
                remove_fillers: case["fillers"].as_bool().unwrap(),
                self_corrections: case["self_corrections"].as_bool().unwrap(),
                ..Default::default()
            };
            assert_eq!(
                super::validate_polish(
                    case["raw"].as_str().unwrap(),
                    case["output"].as_str().unwrap(),
                    &preferences
                )
                .is_ok(),
                case["accepted"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
        }
        let preferences = crate::cleanup::modes::Formatting {
            self_corrections: false,
            ..Default::default()
        };
        assert!(super::validate_polish(
            "pay 5 actually 10 dollars",
            "Pay 10 dollars.",
            &preferences
        )
        .is_err());
    }

    #[test]
    fn shared_meaning_cases() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/cleanup/meaning.json"
        ))
        .unwrap();
        for case in cases["cases"].as_array().unwrap() {
            assert_eq!(
                super::preserves(
                    case["raw"].as_str().unwrap(),
                    case["output"].as_str().unwrap(),
                    case["spoken"].as_bool().unwrap(),
                    case["fillers"].as_bool().unwrap()
                ),
                case["accepted"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
        }
    }
}
