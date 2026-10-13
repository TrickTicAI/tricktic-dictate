//! Super is a rewriting operation. Protect concrete values and polarity without
//! requiring every ordinary word to survive. This is not a semantic verifier.
use super::{tokens, Tokens, CUES};
use crate::cleanup::modes::Formatting;

fn negation(word: &str) -> bool {
    matches!(
        word,
        "not"
            | "no"
            | "never"
            | "neither"
            | "nor"
            | "without"
            | "לא"
            | "אל"
            | "אין"
            | "אינו"
            | "אינה"
            | "אינם"
            | "אינן"
            | "בלי"
            | "ללא"
    )
}

fn protected_word(word: &str) -> bool {
    negation(word)
        || matches!(
            word,
            "zero"
                | "one"
                | "two"
                | "three"
                | "four"
                | "five"
                | "six"
                | "seven"
                | "eight"
                | "nine"
                | "ten"
                | "eleven"
                | "twelve"
                | "thirteen"
                | "fourteen"
                | "fifteen"
                | "sixteen"
                | "seventeen"
                | "eighteen"
                | "nineteen"
                | "twenty"
                | "thirty"
                | "forty"
                | "fifty"
                | "sixty"
                | "seventy"
                | "eighty"
                | "ninety"
                | "hundred"
                | "thousand"
                | "million"
                | "billion"
                | "אפס"
                | "אחד"
                | "אחת"
                | "שניים"
                | "שתיים"
                | "שני"
                | "שתי"
                | "שלוש"
                | "שלושה"
                | "ארבע"
                | "ארבעה"
                | "חמש"
                | "חמישה"
                | "שש"
                | "שישה"
                | "שבע"
                | "שבעה"
                | "שמונה"
                | "תשע"
                | "תשעה"
                | "עשר"
                | "עשרה"
                | "עשרים"
                | "שלושים"
                | "ארבעים"
                | "חמישים"
                | "שישים"
                | "שבעים"
                | "שמונים"
                | "תשעים"
                | "מאה"
                | "מאות"
                | "אלף"
                | "אלפים"
                | "מיליון"
                | "dollar"
                | "dollars"
                | "euro"
                | "euros"
                | "shekel"
                | "shekels"
                | "pound"
                | "pounds"
                | "percent"
                | "דולר"
                | "דולרים"
                | "אירו"
                | "שקל"
                | "שקלים"
                | "אחוז"
                | "אחוזים"
                | "today"
                | "tomorrow"
                | "yesterday"
                | "am"
                | "pm"
                | "היום"
                | "מחר"
                | "אתמול"
                | "monday"
                | "tuesday"
                | "wednesday"
                | "thursday"
                | "friday"
                | "saturday"
                | "sunday"
                | "maybe"
                | "perhaps"
                | "possibly"
                | "probably"
                | "אולי"
                | "כנראה"
                | "yes"
                | "כן"
        )
}

fn correction_marker(value: &str) -> bool {
    matches!(value, "w:actually" | "w:sorry" | "w:בעצם" | "w:סליחה")
}

fn correctable(value: &str) -> bool {
    value.starts_with("n:")
        || matches!(
            value,
            "w:monday"
                | "w:tuesday"
                | "w:wednesday"
                | "w:thursday"
                | "w:friday"
                | "w:saturday"
                | "w:sunday"
        )
}

fn expand_contractions(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|v| {
            let expanded = match v.as_str() {
                "w:i'm" => Some("i am"),
                "w:you're" => Some("you are"),
                "w:we're" => Some("we are"),
                "w:they're" => Some("they are"),
                "w:it's" => Some("it is"),
                "w:that's" => Some("that is"),
                "w:i've" => Some("i have"),
                "w:we've" => Some("we have"),
                "w:you've" => Some("you have"),
                "w:they've" => Some("they have"),
                "w:i'll" => Some("i will"),
                "w:we'll" => Some("we will"),
                "w:you'll" => Some("you will"),
                "w:they'll" => Some("they will"),
                _ => None,
            };
            expanded
                .map(|s| s.split_whitespace().map(|w| format!("w:{w}")).collect())
                .unwrap_or_else(|| vec![v.clone()])
        })
        .collect()
}

fn source_values(source: &Tokens, preferences: &Formatting) -> Vec<String> {
    let mut values = expand_contractions(&source.values);
    // Only explicit adjacent value corrections authorize dropping a protected
    // value: "5, actually 10". Never infer a correction from ordinary prose.
    if preferences.self_corrections {
        let mut i = 1;
        while i + 1 < values.len() {
            if correction_marker(&values[i])
                && correctable(&values[i - 1])
                && correctable(&values[i + 1])
                && values[i - 1].starts_with("n:") == values[i + 1].starts_with("n:")
            {
                values.drain(i - 1..=i);
                i = i.saturating_sub(1).max(1);
            } else {
                i += 1;
            }
        }
    }
    values
}

fn anchors(values: &[String], preferences: &Formatting) -> Vec<String> {
    values
        .iter()
        .enumerate()
        .filter(|(i, v)| {
            if matches!(v.as_str(), "w:am" | "w:pm") {
                return *i > 0 && values[*i - 1].starts_with("n:");
            }
            // "like" can be a filler, but removing it after a subject can
            // erase a stated preference ("I like apples" -> "I apples").
            if v.as_str() == "w:like"
                && *i > 0
                && matches!(
                    values[*i - 1].as_str(),
                    "w:i" | "w:we" | "w:you" | "w:they" | "w:would"
                )
            {
                return true;
            }
            let Some(word) = v.strip_prefix("w:") else {
                return true;
            };
            protected_word(word)
                || word.chars().any(char::is_numeric)
                || word.chars().any(|c| {
                    ('\u{0300}'..='\u{036f}').contains(&c) || ('\u{0591}'..='\u{05c7}').contains(&c)
                })
                || (!preferences.remove_fillers && matches!(word, "um" | "uh" | "אממ" | "like"))
        })
        .map(|(_, v)| v.clone())
        .collect()
}

fn scope_word(value: &str) -> String {
    // A deliberately narrow equivalence for inflection immediately after a
    // negation; unrelated verbs may not replace one another.
    let word = value.strip_prefix("w:").unwrap_or(value);
    match word {
        "is" | "are" | "was" | "were" | "been" | "being" => "be".into(),
        "does" | "did" => "do".into(),
        "has" | "had" => "have".into(),
        _ => {
            for suffix in ["ing", "ed", "s"] {
                if let Some(stem) = word
                    .strip_suffix(suffix)
                    .filter(|s| s.len() >= 3 && s.is_ascii())
                {
                    return stem.into();
                }
            }
            word.into()
        }
    }
}

fn negation_scopes(values: &[String]) -> Vec<(String, Option<String>)> {
    values
        .iter()
        .enumerate()
        .filter_map(|(i, value)| {
            let word = value.strip_prefix("w:")?;
            if !negation(word) {
                return None;
            }
            let next = values[i + 1..]
                .iter()
                .find(|v| !matches!(v.as_str(), "w:the" | "w:a" | "w:an" | "w:to"));
            Some((word.into(), next.map(|v| scope_word(v))))
        })
        .collect()
}

fn has_script(text: &str, hebrew: bool) -> bool {
    text.chars().any(|c| {
        if hebrew {
            ('\u{05d0}'..='\u{05ea}').contains(&c)
        } else {
            c.is_ascii_alphabetic() || ('\u{00c0}'..='\u{024f}').contains(&c)
        }
    })
}

fn cues_preserved(source: &Tokens, target: &Tokens, preferences: &Formatting) -> bool {
    for (cue, marks) in CUES {
        let cue: Vec<String> = cue.split_whitespace().map(|v| format!("w:{v}")).collect();
        for (i, window) in source.values.windows(cue.len()).enumerate() {
            if window != cue {
                continue;
            }
            // Literal mentions can always remain as words.
            if target.values.windows(cue.len()).any(|w| w == cue) {
                continue;
            }
            let literal = source.values[i.saturating_sub(4)..i].iter().any(|v| {
                let word = v.strip_prefix("w:").unwrap_or(v);
                negation(word) || matches!(word, "word" | "literal" | "called" | "מילה" | "המילה")
            });
            if !preferences.spoken_punctuation || literal {
                return false;
            }
            // Rendering must stay beside the same preceding/following word.
            // Grammar elsewhere in the span does not invalidate the cue.
            let before = i.checked_sub(1).map(|n| &source.values[n]);
            let after = source.values.get(i + cue.len());
            let rendered = target.gaps.iter().enumerate().any(|(j, gap)| {
                marks.chars().any(|c| gap.contains(c))
                    && before.is_none_or(|v| {
                        j.checked_sub(1).and_then(|n| target.values.get(n)) == Some(v)
                    })
                    && after.is_none_or(|v| target.values.get(j) == Some(v))
            });
            if !rendered {
                return false;
            }
        }
    }
    true
}

pub(super) fn preserves(original: &str, output: &str, preferences: &Formatting) -> bool {
    if super::preserves(
        original,
        output,
        preferences.spoken_punctuation,
        preferences.remove_fillers,
    ) {
        return true;
    }
    if output.trim().is_empty()
        || output.contains('\0')
        || output.len() > original.len().saturating_mul(3) + 128
    {
        return false;
    }
    if [false, true]
        .into_iter()
        .any(|h| has_script(original, h) != has_script(output, h))
    {
        return false;
    }
    let source = tokens(original);
    let target = tokens(output);
    let values = source_values(&source, preferences);
    let target_values = expand_contractions(&target.values);
    if anchors(&values, preferences) != anchors(&target_values, preferences)
        || negation_scopes(&values) != negation_scopes(&target_values)
        || !cues_preserved(&source, &target, preferences)
    {
        return false;
    }
    // Polishing must still be grounded in the dictation, not an answer or a
    // replacement paragraph. Linear storage and lookup; no quadratic diff.
    let content = |v: &&String| {
        v.starts_with("w:")
            && !matches!(
                v.as_str(),
                "w:a"
                    | "w:an"
                    | "w:the"
                    | "w:is"
                    | "w:are"
                    | "w:was"
                    | "w:were"
                    | "w:to"
                    | "w:and"
                    | "w:of"
            )
    };
    let source_words: std::collections::BTreeSet<_> = values
        .iter()
        .filter(content)
        .map(|v| scope_word(v))
        .collect();
    let output_words: Vec<_> = target_values
        .iter()
        .filter(content)
        .map(|v| scope_word(v))
        .collect();
    if !source_words.is_empty()
        && (output_words.is_empty()
            || output_words
                .iter()
                .filter(|w| source_words.contains(*w))
                .count()
                * 2
                < output_words.len())
    {
        return false;
    }
    // Changing the speaker or answering a question is not dictation cleanup.
    let person = |v: &&String| {
        matches!(
            v.as_str(),
            "w:i"
                | "w:we"
                | "w:you"
                | "w:he"
                | "w:she"
                | "w:they"
                | "w:אני"
                | "w:אנחנו"
                | "w:אתה"
                | "w:את"
                | "w:הוא"
                | "w:היא"
                | "w:הם"
                | "w:הן"
        )
    };
    let source_people: std::collections::BTreeSet<_> = values.iter().filter(person).collect();
    let target_people: std::collections::BTreeSet<_> =
        target_values.iter().filter(person).collect();
    source_people == target_people
}
