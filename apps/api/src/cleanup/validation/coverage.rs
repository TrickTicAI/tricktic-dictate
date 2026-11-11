//! Whether a polished answer left out much of what was said. A model sometimes
//! ends a complete-looking answer after the first sentences, and the same input
//! polished again is usually whole. This runs at every Polish check level,
//! Off included, and compares words only: numbers stay with the Polish check.
use super::{tokens, CUES};
use std::collections::BTreeSet;

/// Distinct content words a dictation needs before a share of them means much.
const MIN_WORDS: usize = 12;
/// On three weeks of the maintainer's dictations (2026-10-20 to 11-10), cut
/// answers kept at most 58% of the distinct content words; whole ones at least 67%.
const MIN_KEPT: usize = 60;
/// A sentence of this many content words with nearly all of them gone was
/// dropped, even when the rest of a long dictation survived.
const SENTENCE_WORDS: usize = 8;
const SENTENCE_KEPT: usize = 25;

/// Words a faithful polish may well remove or change: function words, fillers,
/// hedges and the words of spoken formatting cues.
const ORDINARY: &str =
    "an the and or but if so of to in on at by for with from as is are was were be \
    been being am me my we us our you your he him his she her it its they them their \
    this that these those there here what which who whom how why when where do does \
    did done have has had having will would can could should shall may might must \
    not no just like um uh uhm hmm yeah okay ok oh well also then than too very \
    really actually basically kind sort know mean thing things stuff gonna wanna \
    gotta get got one some any all about into out up down over again more most much \
    many i'm it's that's there's let's i've i'll i'd you're we're they're";
fn ordinary(word: &str) -> bool {
    ORDINARY.split_whitespace().any(|w| w == word)
        || CUES
            .iter()
            .any(|(cue, _)| cue.split_whitespace().any(|w| w == word))
}

/// The content words of each sentence, in order.
fn sentences(text: &str) -> Vec<BTreeSet<String>> {
    let tokens = tokens(text);
    let mut result = vec![BTreeSet::new()];
    for (value, gap) in tokens.values.iter().zip(&tokens.gaps) {
        if gap.contains(['.', '?', '!', '\n']) && !result.last().is_some_and(BTreeSet::is_empty) {
            result.push(BTreeSet::new());
        }
        if let Some(word) = value.strip_prefix("w:") {
            if word.chars().count() > 1 && !ordinary(word) {
                result
                    .last_mut()
                    .expect("never empty")
                    .insert(word.to_owned());
            }
        }
    }
    result
}

fn share_below(words: &BTreeSet<&String>, kept: &BTreeSet<String>, percent: usize) -> bool {
    words.iter().filter(|w| kept.contains(**w)).count() * 100 < words.len() * percent
}

/// True when `output` keeps too few of the dictation's content words overall,
/// or loses nearly all of one sentence.
pub(super) fn dropped(original: &str, output: &str) -> bool {
    let source = sentences(original);
    let kept: BTreeSet<String> = sentences(output).into_iter().flatten().collect();
    let all: BTreeSet<&String> = source.iter().flatten().collect();
    if all.len() >= MIN_WORDS && share_below(&all, &kept, MIN_KEPT) {
        return true;
    }
    source.iter().any(|sentence| {
        let words: BTreeSet<&String> = sentence.iter().collect();
        words.len() >= SENTENCE_WORDS && share_below(&words, &kept, SENTENCE_KEPT)
    })
}

#[cfg(test)]
mod tests {
    use super::dropped;

    const SPOKEN: &str = "Also she asked about payments and a payment system, how complicated would it be for us to integrate a payment system? The receipts and stuff Probably one that has receipts integrated Like would it be complicated to add an API for it? Or like decide on a single system and then just use it, let the owner configure it and then he provides the API key and once the connection is there, all the integration would work. Also our conversation is tomorrow so I need to focus on some stuff and then some stuff can probably be deferred by importance.";

    #[test]
    fn an_answer_that_stops_after_the_first_sentences_is_dropped() {
        let cut = "Also, she asked about payments and a payment system: how complicated would it be for us to integrate a payment system? The receipts and stuff… probably one that has receipts integrated.";
        assert!(dropped(SPOKEN, cut));
    }

    #[test]
    fn a_whole_polish_is_kept() {
        let whole = "Also, she asked about payments and a payment system: how complicated would it be for us to integrate a payment system? The receipts and stuff… probably one that has receipts integrated. Would it be complicated to add an API for it? Or decide on a single system and use it: let the owner configure it, and then he provides the API key. Once the connection is there, all the integration would work.\n\nAlso, our conversation is tomorrow, so I need to focus on some things, and others can be deferred by importance.";
        assert!(!dropped(SPOKEN, whole));
    }

    #[test]
    fn fillers_cues_and_short_dictations_are_not_counted() {
        assert!(!dropped(
            "um so like basically I think we should uh merge the branch question mark",
            "I think we should merge the branch?"
        ));
        assert!(!dropped("send it to the team tomorrow", "Send it."));
    }

    #[test]
    fn one_lost_sentence_in_a_long_dictation_is_dropped() {
        let spoken = "Merge the finished branches today. Then rebuild the Windows installer with the signing certificate and upload the release notes to the shared release folder. Tell me when it is done.";
        let polished = "Merge the finished branches today. Tell me when it is done.";
        assert!(dropped(spoken, polished));
    }
}
