//! Whisper can loop on a short window: 1.73 s of "first name, last name" came
//! back as the phrase three times. A repeated phrase alone is not proof, since
//! people repeat themselves ("blah blah blah blah"), so it only asks for a second
//! decode at full context. Text is removed only when the repeat survives that and
//! has more words than the audio could hold. Diagnostics hold counts, never text.

/// Words per second above which a window cannot be speech. Across 2,919 saved
/// Windows dictations (2026-10-30) the 99th percentile was 3.9; the three loops
/// above this bound were all recognition repeats.
const MAX_WORDS_PER_SECOND: f64 = 5.0;
const MAX_PHRASE_WORDS: usize = 8;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Repeat {
    /// Byte offsets: after the first copy, and after the last copy.
    first_end: usize,
    last_end: usize,
    pub copies: usize,
}

fn words(text: &str) -> Vec<(usize, usize, String)> {
    let mut found = vec![];
    let mut start = None;
    for (at, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        let part = c.is_alphanumeric() || matches!(c, '\'' | '’');
        match (start, part) {
            (None, true) => start = Some(at),
            (Some(from), false) => {
                found.push((from, at, text[from..at].to_lowercase()));
                start = None;
            }
            _ => {}
        }
    }
    found
}

/// The longest run of one phrase repeated back to back: four times for a single
/// word, three times for a phrase of two to eight words.
pub(super) fn find(text: &str) -> Option<Repeat> {
    let words = words(text);
    let same = |a: usize, b: usize, n: usize| (0..n).all(|k| words[a + k].2 == words[b + k].2);
    let mut best: Option<(usize, Repeat)> = None;
    for i in 0..words.len() {
        for n in 1..=MAX_PHRASE_WORDS {
            let mut copies = 1;
            while i + (copies + 1) * n <= words.len() && same(i, i + copies * n, n) {
                copies += 1;
            }
            let covered = copies * n;
            if copies >= if n == 1 { 4 } else { 3 }
                && best.as_ref().is_none_or(|(known, _)| covered > *known)
            {
                best = Some((
                    covered,
                    Repeat {
                        first_end: words[i + n - 1].1,
                        last_end: words[i + covered - 1].1,
                        copies,
                    },
                ));
            }
        }
    }
    best.map(|(_, repeat)| repeat)
}

/// More words than `seconds` of audio could carry.
pub(super) fn implausible(text: &str, seconds: f64) -> bool {
    seconds > 0.0 && words(text).len() as f64 / seconds > MAX_WORDS_PER_SECOND
}

/// The text with the repeat's later copies removed.
pub(super) fn collapse(text: &str, repeat: &Repeat) -> String {
    format!("{}{}", &text[..repeat.first_end], &text[repeat.last_end..])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_looped_phrase_is_found_and_collapsed_to_one_copy() {
        let text = " First name, last name First name, last name First name, last name";
        let repeat = find(text).unwrap();
        assert_eq!(repeat.copies, 3);
        assert_eq!(collapse(text, &repeat), " First name, last name");
        assert!(implausible(text, 1.73));
        assert!(!implausible(" First name, last name", 1.73));
    }
    #[test]
    fn ordinary_repeats_are_not_loops() {
        assert!(find("I said no, no, no.").is_none());
        assert!(find("very very very good").is_none());
        assert!(find("we need to test, we need to test it").is_none());
        // Found, but at a speaking rate a person can produce.
        let text = "it was blah blah blah blah and then we left";
        assert!(find(text).is_some());
        assert!(!implausible(text, 4.0));
    }
    #[test]
    fn the_text_around_a_loop_is_kept() {
        let text = "Then and and and and and three hundred.";
        let repeat = find(text).unwrap();
        assert_eq!(repeat.copies, 5);
        assert_eq!(collapse(text, &repeat), "Then and three hundred.");
    }
}
