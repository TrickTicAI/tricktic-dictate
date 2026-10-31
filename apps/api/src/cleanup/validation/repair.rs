//! A refused Super answer is usually one changed value in an otherwise good
//! rewrite. Keep the polish of the sentences that pass and the dictated words of
//! those that do not, instead of discarding the whole answer. The assembled text
//! must still pass the whole-span check, so this never delivers what that refuses.
use super::{polish::rejection, tokens};
use crate::cleanup::modes::{Formatting, PolishCheck};
use std::{collections::BTreeSet, ops::Range};

/// Alignment is quadratic in sentences; a longer span keeps its original.
const MAX_SENTENCES: usize = 256;
/// A rewrite may split or join sentences. Each pair is (dictated, polished).
const GROUPS: [(usize, usize); 6] = [(1, 1), (1, 2), (2, 1), (2, 2), (1, 3), (3, 1)];

pub(in crate::cleanup) struct Repaired {
    pub text: String,
    /// How many dictated sentences stayed as spoken.
    pub kept: u32,
}

/// Each range ends after its terminator and whitespace. A piece with no letter
/// or digit joins the sentence before it.
fn sentences(text: &str) -> Vec<Range<usize>> {
    let mut found: Vec<Range<usize>> = vec![];
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        let ended = c == '\n'
            || matches!(c, '.' | '!' | '?' | '…')
                && chars.peek().is_none_or(|(_, next)| next.is_whitespace());
        if !ended {
            continue;
        }
        while chars.peek().is_some_and(|(_, next)| next.is_whitespace()) {
            chars.next();
        }
        let end = chars.peek().map_or(text.len(), |(at, _)| *at);
        push(&mut found, text, start..end);
        start = end;
    }
    if start < text.len() {
        push(&mut found, text, start..text.len());
    }
    found
}
fn push(found: &mut Vec<Range<usize>>, text: &str, range: Range<usize>) {
    match found.last_mut() {
        Some(last) if !text[range.clone()].chars().any(char::is_alphanumeric) => {
            last.end = range.end
        }
        _ => found.push(range),
    }
}

type Words = BTreeSet<String>;
fn words(text: &str, ranges: &[Range<usize>]) -> Vec<Words> {
    ranges
        .iter()
        .map(|range| tokens(&text[range.clone()]).values.into_iter().collect())
        .collect()
}
fn score(a: &[Words], b: &[Words]) -> i64 {
    let a: BTreeSet<_> = a.iter().flatten().collect();
    let b: BTreeSet<_> = b.iter().flatten().collect();
    let shared = a.intersection(&b).count() as i64;
    2 * shared - (a.len() as i64 - shared) - (b.len() as i64 - shared)
}

/// The in-order grouping of sentences whose words agree best.
fn align(a: &[Words], b: &[Words]) -> Option<Vec<(usize, usize, usize, usize)>> {
    let mut best = vec![vec![None::<(i64, usize, usize)>; b.len() + 1]; a.len() + 1];
    best[0][0] = Some((0, 0, 0));
    for i in 0..=a.len() {
        for j in 0..=b.len() {
            let Some((total, _, _)) = best[i][j] else {
                continue;
            };
            for (di, dj) in GROUPS {
                if i + di > a.len() || j + dj > b.len() {
                    continue;
                }
                // On a tie, the finest grouping: one refusal costs the fewest sentences.
                let merged = (di + dj - 2) as i64;
                let next = total + score(&a[i..i + di], &b[j..j + dj]) - merged;
                let cell = &mut best[i + di][j + dj];
                if cell.is_none_or(|(known, _, _)| next > known) {
                    *cell = Some((next, di, dj));
                }
            }
        }
    }
    let (mut i, mut j) = (a.len(), b.len());
    best[i][j]?;
    let mut groups = vec![];
    while i > 0 || j > 0 {
        let (_, di, dj) = best[i][j]?;
        i -= di;
        j -= dj;
        groups.push((i, di, j, dj));
    }
    groups.reverse();
    Some(groups)
}

pub(in crate::cleanup) fn repair(
    original: &str,
    output: &str,
    preferences: &Formatting,
    check: PolishCheck,
) -> Option<Repaired> {
    let (from, to) = (sentences(original), sentences(output));
    if from.len().max(to.len()) > MAX_SENTENCES {
        return None;
    }
    let (mut text, mut kept, mut polished) = (String::new(), 0, false);
    for (i, di, j, dj) in align(&words(original, &from), &words(output, &to))? {
        let dictated = &original[from[i].start..from[i + di - 1].end];
        let answer = &output[to[j].start..to[j + dj - 1].end];
        if rejection(dictated, answer, preferences, check).is_none() {
            text.push_str(answer);
            polished = true;
        } else {
            // The answer's own spacing, so its paragraphs stay as formatted.
            text.push_str(dictated.trim());
            text.push_str(&answer[answer.trim_end().len()..]);
            kept += di as u32;
        }
    }
    (polished && kept > 0 && rejection(original, &text, preferences, check).is_none())
        .then_some(Repaired { text, kept })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixed(raw: &str, output: &str) -> Option<(String, u32)> {
        repair(raw, output, &Formatting::default(), PolishCheck::Balanced).map(|r| (r.text, r.kept))
    }
    #[test]
    fn only_the_changed_sentence_keeps_its_dictated_words() {
        assert_eq!(
            fixed(
                "so we shipped it. pay 5 dollars. tell them it works.",
                "We shipped it.\n\nPay 6 dollars.\n\nTell them it works.",
            ),
            Some((
                "We shipped it.\n\npay 5 dollars.\n\nTell them it works.".into(),
                1
            ))
        );
    }
    #[test]
    fn a_split_sentence_stays_with_the_sentence_it_came_from() {
        assert_eq!(
            fixed(
                "we shipped it and they liked it. pay 5 dollars.",
                "We shipped it. They liked it. Pay 6 dollars.",
            ),
            Some(("We shipped it. They liked it. pay 5 dollars.".into(), 1))
        );
    }
    #[test]
    fn nothing_polished_or_nothing_alignable_is_not_a_repair() {
        assert!(fixed("pay 5 dollars.", "Pay 6 dollars.").is_none());
        assert!(fixed(
            "pay 5 dollars. send 3 files.",
            "Pay 6 dollars. Send 4 files."
        )
        .is_none());
        assert!(fixed("one two three", "A. B. C. D. E.").is_none());
    }
    #[test]
    fn a_value_moved_between_sentences_is_never_delivered() {
        // Moving "not" to another verb must not pass by being checked per sentence.
        assert_eq!(
            fixed(
                "do not send the report. delete the old file. keep the notes.",
                "Send the report. Do not delete the old file. Keep the notes.",
            ),
            Some((
                "do not send the report. delete the old file. Keep the notes.".into(),
                2
            ))
        );
    }
}
