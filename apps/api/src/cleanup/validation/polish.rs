//! Super is a rewriting operation. Protect concrete values and polarity without
//! requiring every ordinary word to survive. This is not a semantic verifier.
use super::{tokens, Tokens, CUES};
use crate::cleanup::modes::{Formatting, PolishCheck};

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

fn hedge(word: &str) -> bool {
    matches!(
        word,
        "maybe" | "perhaps" | "possibly" | "probably" | "אולי" | "כנראה"
    )
}

/// Values a rewrite must keep exactly. At the balanced check, negations are
/// checked separately by count, order and scope (`negations_kept`), so their
/// wording may change, and hedges (`maybe`, `probably`) may be dropped: the maintainer
/// decided on 2026-10-28 that they are usually verbal habit in dictation.
/// The strict check keeps both exactly.
fn protected_word(word: &str, strict: bool) -> bool {
    strict && (negation(word) || hedge(word))
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

/// A sentence-initial "No" answering something, before a new clause ("No, why
/// would it?"), which a rewrite may drop. "No idea" and "no reason" stay.
fn interjection(tokens: &Tokens, i: usize) -> bool {
    let starts = i == 0 || tokens.gaps[i].contains(['.', '!', '?', '\n']);
    let after = tokens.gaps.get(i + 1).map_or("", String::as_str);
    tokens.values[i] == "w:no"
        && starts
        && (after.contains([',', '.', '!', '?'])
            || tokens.values.get(i + 1).is_some_and(|next| {
                matches!(
                    next.as_str(),
                    "w:i"
                        | "w:i'm"
                        | "w:we"
                        | "w:you"
                        | "w:they"
                        | "w:he"
                        | "w:she"
                        | "w:it"
                        | "w:it's"
                        | "w:that"
                        | "w:this"
                        | "w:why"
                        | "w:what"
                        | "w:how"
                        | "w:so"
                        | "w:but"
                        | "w:wait"
                        | "w:no"
                        | "w:okay"
                        | "w:let's"
                )
            }))
}
fn without_interjections(tokens: &Tokens) -> Vec<String> {
    (0..tokens.values.len())
        .filter(|&i| !interjection(tokens, i))
        .map(|i| tokens.values[i].clone())
        .collect()
}

fn source_values(source: &Tokens, preferences: &Formatting, strict: bool) -> Vec<String> {
    let mut values = expand_contractions(&if strict {
        source.values.clone()
    } else {
        without_interjections(source)
    });
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

fn anchors(values: &[String], preferences: &Formatting, strict: bool) -> Vec<String> {
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
            protected_word(word, strict)
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

/// Drops a run of tokens that immediately repeats the run before it ("I don't
/// know, I don't know"), since Super removes accidental repetitions.
fn collapse_repeats(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(values.len());
    let mut i = 0;
    'next: while i < values.len() {
        for len in (1..=8).rev() {
            if out.len() >= len
                && values.len() >= i + len
                && out[out.len() - len..] == values[i..i + len]
            {
                i += len;
                continue 'next;
            }
        }
        out.push(values[i].clone());
        i += 1;
    }
    out
}

/// A single English number word, so "five" and "5" are the same value. Each word
/// of a compound ("twenty five") stays a separate value and never equals "25".
fn number_word(word: &str) -> Option<u32> {
    const WORDS: [&str; 21] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    WORDS.iter().position(|w| *w == word).map(|n| n as u32)
}

/// "3,000" as "3000". Only well-formed thousands groups: "1,5" may be a decimal
/// comma and "1,50" a price, so both stay as written.
fn ungrouped(value: &str) -> Option<String> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let digits = whole.trim_start_matches(['+', '-']);
    let (first, rest) = digits.split_once(',')?;
    let plain = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let grouped = plain(first)
        && first.len() <= 3
        && rest.split(',').all(|part| part.len() == 3 && plain(part))
        && (fraction.is_empty() || plain(fraction));
    grouped.then(|| value.replace(',', ""))
}

/// The pronoun in "this one" or "the other one" names no quantity, so a rewrite
/// may drop it (maintainer decision, 2026-10-30). "only one", "one of" and a bare "one" stay
/// numbers.
fn without_pronoun_one(values: Vec<String>, strict: bool) -> Vec<String> {
    if strict {
        return values;
    }
    let pronoun = |i: usize| {
        values[i] == "w:one"
            && i > 0
            && matches!(
                values[i - 1].as_str(),
                "w:this"
                    | "w:that"
                    | "w:which"
                    | "w:the"
                    | "w:each"
                    | "w:every"
                    | "w:another"
                    | "w:other"
                    | "w:any"
                    | "w:first"
                    | "w:last"
                    | "w:next"
                    | "w:same"
                    | "w:new"
                    | "w:old"
                    | "w:right"
                    | "w:wrong"
            )
    };
    (0..values.len())
        .filter(|&i| !pronoun(i))
        .map(|i| values[i].clone())
        .collect()
}

/// Spellings of the same protected value: "10am" / "10 a.m.", "today's" /
/// "today", "five" / "5" and, unless strict, "3,000" / "3000".
fn normalize(mut values: Vec<String>, strict: bool) -> Vec<String> {
    let mut i = 0;
    while i < values.len() {
        if let Some(plain) = values[i]
            .strip_prefix("n:")
            .and_then(ungrouped)
            .filter(|_| !strict)
        {
            values[i] = format!("n:{plain}");
        }
        if let Some(stem) = values[i]
            .strip_prefix("w:")
            .and_then(|w| w.strip_suffix("'s"))
            .filter(|w| protected_word(w, true))
        {
            values[i] = format!("w:{stem}");
        }
        if let Some(digits) = values[i].strip_prefix("w:").and_then(number_word) {
            values[i] = format!("n:{digits}");
        }
        if values[i].starts_with("n:")
            && matches!(values.get(i + 1).map(String::as_str), Some("w:a" | "w:p"))
            && values.get(i + 2).map(String::as_str) == Some("w:m")
        {
            values[i + 1] = format!("{}m", values[i + 1]);
            values.remove(i + 2);
        }
        i += 1;
    }
    values
}

/// Numbers the model added as list markers ("1." at the start of a line) when
/// the mode allows lists.
fn without_list_markers(target: &Tokens, preferences: &Formatting, strict: bool) -> Vec<String> {
    target
        .values
        .iter()
        .enumerate()
        .filter(|(j, v)| {
            let marker = preferences.lists
                && v.starts_with("n:")
                && (*j == 0 || target.gaps[*j].contains('\n'))
                && target
                    .gaps
                    .get(j + 1)
                    .is_some_and(|g| g.starts_with(['.', ')']));
            !marker && (strict || !interjection(target, *j))
        })
        .map(|(_, v)| v.clone())
        .collect()
}

/// Every negation survives, in order, and still governs what it governed. Any
/// negation word may stand for another ("not wait" -> "no longer wait", "not
/// using" -> "without using"), and the governed words may be reworded, but one
/// of the original's first two governed words must be among the output's next
/// three words, allowing a corrected typo. So word order and wording may change
/// ("not do it" -> "not perform it") but the negation may not move to another
/// verb ("do not send and delete" -> "send and do not delete").
fn negations_kept(source: &[String], target: &[String]) -> bool {
    let governed = |values: &[String], take: usize| {
        let scopes: Vec<Vec<String>> = values
            .iter()
            .enumerate()
            .filter(|(_, v)| v.strip_prefix("w:").is_some_and(negation))
            .map(|(i, _)| {
                values[i + 1..]
                    .iter()
                    .filter(|v| !matches!(v.as_str(), "w:the" | "w:a" | "w:an" | "w:to"))
                    .take(take)
                    .map(|v| scope_word(v))
                    .collect()
            })
            .collect();
        scopes
    };
    let (source, target) = (governed(source, 2), governed(target, 3));
    source.len() == target.len()
        && source.iter().zip(&target).all(|(was, now)| {
            was.is_empty() && now.is_empty()
                || was.iter().any(|w| now.iter().any(|n| same_word(w, n)))
        })
}

/// The strict rule: every negation keeps its word and order, and the source's
/// first negated word must be among the next three output words.
fn negations_kept_strictly(source: &[String], target: &[String]) -> bool {
    let scopes = |values: &[String], take: usize| -> Vec<(String, Vec<String>)> {
        values
            .iter()
            .enumerate()
            .filter_map(|(i, value)| {
                let word = value.strip_prefix("w:")?;
                negation(word).then(|| {
                    let next = values[i + 1..]
                        .iter()
                        .filter(|v| !matches!(v.as_str(), "w:the" | "w:a" | "w:an" | "w:to"))
                        .take(take)
                        .map(|v| scope_word(v))
                        .collect();
                    (word.to_string(), next)
                })
            })
            .collect()
    };
    let (source, target) = (scopes(source, 1), scopes(target, 3));
    source.len() == target.len()
        && source
            .iter()
            .zip(&target)
            .all(|((word, scope), (now, next))| {
                word == now
                    && match scope.first() {
                        Some(scope) => next.contains(scope),
                        None => next.is_empty(),
                    }
            })
}

/// The same word, or a corrected misrecognition of it ("emuleto" -> "emulator").
fn same_word(a: &str, b: &str) -> bool {
    a == b
        || a.chars().count() >= 5
            && b.chars().count() >= 5
            && a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count() >= 4
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

fn compared_source(source: &Tokens, preferences: &Formatting, strict: bool) -> Vec<String> {
    normalize(
        collapse_repeats(without_pronoun_one(
            source_values(source, preferences, strict),
            strict,
        )),
        strict,
    )
}

fn compared_target(target: &Tokens, preferences: &Formatting, strict: bool) -> Vec<String> {
    normalize(
        collapse_repeats(without_pronoun_one(
            expand_contractions(&without_list_markers(target, preferences, strict)),
            strict,
        )),
        strict,
    )
}

/// The first rule `output` breaks, or None when it is an acceptable polish.
pub(super) fn rejection(
    original: &str,
    output: &str,
    preferences: &Formatting,
    check: PolishCheck,
) -> Option<&'static str> {
    if super::preserves(
        original,
        output,
        preferences.spoken_punctuation,
        preferences.remove_fillers,
    ) {
        return None;
    }
    if output.trim().is_empty() || output.contains('\0') {
        return Some("empty");
    }
    // Even with the check off, a dictation's languages must survive: output
    // language and translation are separate, explicit settings.
    if [false, true]
        .into_iter()
        .any(|h| has_script(original, h) != has_script(output, h))
    {
        return Some("script");
    }
    if check == PolishCheck::Off {
        return None;
    }
    let strict = check == PolishCheck::Strict;
    if output.len() > original.len().saturating_mul(3) + 128 {
        return Some("length");
    }
    let source = tokens(original);
    let target = tokens(output);
    let values = compared_source(&source, preferences, strict);
    let target_values = compared_target(&target, preferences, strict);
    if anchors(&values, preferences, strict) != anchors(&target_values, preferences, strict) {
        return Some("anchors");
    }
    let kept = if strict {
        negations_kept_strictly(&values, &target_values)
    } else {
        negations_kept(&values, &target_values)
    };
    if !kept {
        return Some("negation");
    }
    if !cues_preserved(&source, &target, preferences) {
        return Some("cues");
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
        return Some("grounding");
    }
    // Answering a question speaks as the assistant, so the speaker must stay the
    // same. Other pronouns change with ordinary grammar fixes ("he" -> "it",
    // "that we developed" -> "that was developed").
    let speaker = |values: &[String]| values.iter().any(|v| matches!(v.as_str(), "w:i" | "w:אני"));
    (speaker(&values) != speaker(&target_values)).then_some("speaker")
}

/// Explicit diagnostic over saved replay receipts: prints only the protected
/// values and negation scopes that differ, never the dictation itself.
#[cfg(test)]
#[test]
#[ignore = "requires a private replay receipt file"]
fn replay_rejection_detail() {
    let path = std::env::var_os("WRITER_REPLAY_RECEIPTS").expect("receipt path");
    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    // The maintainer's Super formatting as of 2026-10-28.
    let preferences = Formatting {
        lists: true,
        remove_fillers: true,
        self_corrections: true,
        spoken_punctuation: true,
        ..Default::default()
    };
    let check = match std::env::var("WRITER_POLISH_CHECK").as_deref() {
        Ok("strict") => PolishCheck::Strict,
        Ok("off") => PolishCheck::Off,
        _ => PolishCheck::Balanced,
    };
    let strict = check == PolishCheck::Strict;
    let (mut checked, mut accepted) = (0, 0);
    for receipt in receipts {
        let (Some(raw), Some(output)) = (
            receipt["raw_text"].as_str(),
            receipt["upstream_output"].as_str(),
        ) else {
            continue;
        };
        let Ok(spans) = serde_json::from_str::<serde_json::Value>(output) else {
            continue;
        };
        let output: Vec<&str> = spans["spans"]
            .as_array()
            .map(|s| s.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let output = output.join(" ");
        checked += 1;
        let Some(rule) = rejection(raw, &output, &preferences, check) else {
            accepted += 1;
            continue;
        };
        let source = compared_source(&tokens(raw), &preferences, strict);
        let target = compared_target(&tokens(&output), &preferences, strict);
        let (a, b) = (
            anchors(&source, &preferences, strict),
            anchors(&target, &preferences, strict),
        );
        let only = |x: &[String], y: &[String]| {
            let mut y = y.to_vec();
            x.iter()
                .filter(|v| match y.iter().position(|w| w == *v) {
                    Some(n) => {
                        y.remove(n);
                        false
                    }
                    None => true,
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        println!(
            "{} {} {rule} lost={:?} added={:?} reordered={}",
            &receipt["job_id"].as_str().unwrap_or("")[..8],
            receipt["role"].as_str().unwrap_or(""),
            only(&a, &b),
            only(&b, &a),
            only(&a, &b).is_empty() && only(&b, &a).is_empty() && a != b,
        );
    }
    println!("accepted {accepted} of {checked} model answers");
}
