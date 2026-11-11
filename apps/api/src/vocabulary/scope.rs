//! Context-scoped vocabulary (MW-122). The saved dictionary can hold more
//! hints than Whisper reads: whisper.cpp keeps only the last 223 tokens of a
//! prompt and silently drops its start. Each request instead sends the hints
//! that matter for this dictation, chosen in this order of evidence:
//!
//! 1. named in the target window's title or application name;
//! 2. corrected recently (a correction's replacement, a term the user restored
//!    in History);
//! 3. used in recent dictations to the same application;
//! 4. the dictionary's own order.
//!
//! Selection only decides membership; selected hints keep dictionary order,
//! so a dictionary that fits is sent exactly as today. The context lives in
//! memory for one recording and is never persisted or logged.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

/// whisper.cpp `max_prompt_ctx - 1`: half the 448-token text context, minus one.
pub const PROMPT_TOKENS: usize = 223;
/// Room left for `estimate_tokens` running low on an unusual word.
const MARGIN: usize = 12;
/// Most hint tokens sent at once. On the personal scorecard (MW-122) a
/// 128-token selection beat ~170 tokens, 96 and 64; longer prompts cost short
/// clips the most.
pub const HINT_TOKENS: usize = 128;

/// What is known about this dictation when it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HintContext {
    /// The target application's own name ("Slack").
    pub app: Option<String>,
    /// The target window's title. Memory only.
    pub title: Option<String>,
    /// Recently corrected terms, most recent first.
    pub recent: Vec<String>,
    /// Hints used in recent dictations to this application, most used first.
    pub used: Vec<String>,
}

/// A conservative count of Whisper (GPT-2 BPE) tokens: each letter run costs
/// a token per three letters (two when all capitals, which BPE splits more),
/// digits a token per two, every other symbol one, plus one per separator.
/// On the user's dictionary it reads about 15% above the real tokenizer.
pub fn estimate_tokens(text: &str) -> usize {
    let mut tokens = 0;
    let mut run = String::new();
    let flush = |run: &mut String, tokens: &mut usize| {
        if run.is_empty() {
            return;
        }
        let n = run.chars().count();
        // Digits and all-capital runs (abbreviations) split finer than words.
        *tokens += if run.chars().all(|c| !c.is_lowercase()) {
            n.div_ceil(2)
        } else {
            n.div_ceil(3)
        };
        run.clear();
    };
    let mut kind = None;
    for c in text.chars() {
        let next = if c.is_alphabetic() {
            Some(true)
        } else if c.is_ascii_digit() {
            Some(false)
        } else {
            None
        };
        if next != kind {
            flush(&mut run, &mut tokens);
        }
        kind = next;
        match next {
            Some(_) => run.push(c),
            None if !c.is_whitespace() => tokens += 1,
            None => {}
        }
    }
    flush(&mut run, &mut tokens);
    tokens
}

fn folded(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether every word of `hint` appears, in order and adjacent, in `text`.
fn mentions(text: &[String], hint: &str) -> bool {
    let hint = folded(hint);
    !hint.is_empty() && text.windows(hint.len()).any(|w| w == hint.as_slice())
}

/// The hints that `text` mentions, as whole words, in dictionary order.
pub fn mentioned(text: &str, hints: &[String]) -> Vec<String> {
    let words = folded(text);
    hints
        .iter()
        .filter(|hint| mentions(&words, hint))
        .cloned()
        .collect()
}

/// Per application name, how often each dictionary hint appeared in text
/// delivered there. Bounded to 32 applications of 64 terms each.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct HintUsage(std::collections::BTreeMap<String, std::collections::BTreeMap<String, u32>>);

impl HintUsage {
    const APPS: usize = 32;
    const TERMS: usize = 64;

    pub fn add(&mut self, app: &str, used: &[String]) {
        if used.is_empty() {
            return;
        }
        let terms = self.0.entry(app.to_owned()).or_default();
        for term in used {
            *terms.entry(term.clone()).or_default() += 1;
        }
        if terms.len() > Self::TERMS {
            let keep: HashSet<String> = Self::ranked(terms).into_iter().take(Self::TERMS).collect();
            terms.retain(|t, _| keep.contains(t));
        }
        while self.0.len() > Self::APPS {
            let quietest = self
                .0
                .iter()
                .filter(|(name, _)| name.as_str() != app)
                .min_by_key(|(_, terms)| terms.values().sum::<u32>())
                .map(|(name, _)| name.clone());
            match quietest {
                Some(name) => self.0.remove(&name),
                None => break,
            };
        }
    }

    /// The application's terms, most used first.
    pub fn used_in(&self, app: &str) -> Vec<String> {
        self.0.get(app).map(Self::ranked).unwrap_or_default()
    }

    fn ranked(terms: &std::collections::BTreeMap<String, u32>) -> Vec<String> {
        let mut ranked: Vec<_> = terms.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        ranked.into_iter().map(|(t, _)| t.clone()).collect()
    }
}

/// The hints to send, within `budget` estimated tokens, in dictionary order.
pub fn select(hints: &[String], context: &HintContext, budget: usize) -> Vec<String> {
    let cost = |h: &String| estimate_tokens(h) + 1;
    if hints.iter().map(cost).sum::<usize>() <= budget {
        return hints.to_vec();
    }
    let named: Vec<String> = [&context.app, &context.title]
        .into_iter()
        .flatten()
        .flat_map(|t| folded(t))
        .collect();
    let position = |list: &[String], hint: &str| {
        let key = folded(hint);
        list.iter().position(|t| folded(t) == key)
    };
    let rank = |index: usize| {
        let hint = &hints[index];
        if mentions(&named, hint) {
            (0, 0)
        } else if let Some(p) = position(&context.recent, hint) {
            (1, p)
        } else if let Some(p) = position(&context.used, hint) {
            (2, p)
        } else {
            (3, 0)
        }
    };
    let mut order: Vec<usize> = (0..hints.len()).collect();
    order.sort_by_key(|&i| (rank(i), i));
    let mut chosen = HashSet::new();
    let mut spent = 0;
    for i in order {
        let c = cost(&hints[i]);
        if spent + c <= budget {
            spent += c;
            chosen.insert(i);
        }
    }
    (0..hints.len())
        .filter(|i| chosen.contains(i))
        .map(|i| hints[i].clone())
        .collect()
}

/// The hint budget beside a preceding-text suffix that is also sent: at most
/// `HINT_TOKENS`, less when the suffix leaves less of Whisper's prompt.
pub fn budget_beside(suffix: &str) -> usize {
    PROMPT_TOKENS
        .saturating_sub(MARGIN + estimate_tokens(suffix) + 1)
        .min(HINT_TOKENS)
}

/// Contexts by recording, with recordings in insertion order.
type Contexts = (HashMap<uuid::Uuid, HintContext>, VecDeque<uuid::Uuid>);

/// Per-recording context, set when a dictation starts. Bounded: an entry for
/// an old recording is dropped once 32 newer ones exist.
static CONTEXTS: Mutex<Option<Contexts>> = Mutex::new(None);

pub fn set_context(recording: uuid::Uuid, context: HintContext) {
    let mut guard = CONTEXTS.lock().unwrap();
    let (map, order) = guard.get_or_insert_with(Default::default);
    if map.insert(recording, context).is_none() {
        order.push_back(recording);
        while order.len() > 32 {
            if let Some(old) = order.pop_front() {
                map.remove(&old);
            }
        }
    }
}

pub fn context(recording: uuid::Uuid) -> HintContext {
    CONTEXTS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|(map, _)| map.get(&recording).cloned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_dictionary_that_fits_is_sent_unchanged() {
        let hints = list(&["OAuth", "card", "Claude Code", "worktrees"]);
        let context = HintContext {
            title: Some("Slack".into()),
            ..Default::default()
        };
        assert_eq!(select(&hints, &context, 200), hints);
    }

    #[test]
    fn the_estimate_stays_at_or_above_whisper_tokens_for_the_dictionary() {
        // Real counts from Whisper's multilingual tokenizer for ", "-joined lists.
        let today = list(&[
            "OAuth",
            "card",
            "Metrix",
            "PR",
            "ADE",
            "Metrix-Writer",
            "Remote-Sessions",
            "N8N",
            "TrickTic",
            "merged",
            "merge",
            "dasheserver2",
            "worktrees",
            "CI",
            "Claude",
            "Claude Code",
            "Codex",
            "worktree",
            "agent work",
            "waiting to merge",
            "orchestration board",
            "repo",
            "PRs",
        ]);
        let estimated: usize = today.iter().map(|h| estimate_tokens(h) + 1).sum();
        assert!(estimated >= 78, "{estimated}");
        assert!(estimated <= 78 * 13 / 10, "{estimated}");
    }

    #[test]
    fn over_budget_the_window_title_and_corrections_win_over_order() {
        let mut hints: Vec<String> = (0..60).map(|i| format!("filler{i}")).collect();
        hints.extend(list(&["Seedance", "worktrees", "HeyGen", "Codex"]));
        let context = HintContext {
            app: Some("Brave".into()),
            title: Some("HeyGen video studio - Brave".into()),
            recent: list(&["worktrees"]),
            used: list(&["Codex"]),
        };
        let budget = 20;
        let chosen = select(&hints, &context, budget);
        assert!(chosen.iter().map(|h| estimate_tokens(h) + 1).sum::<usize>() <= budget);
        assert_eq!(
            &chosen[chosen.len() - 3..],
            ["worktrees", "HeyGen", "Codex"]
        );
        assert!(!chosen.contains(&"Seedance".to_string()));
        // The rest is filled in dictionary order, earliest first.
        assert_eq!(chosen[0], "filler0");
    }

    #[test]
    fn multiword_hints_match_whole_words_in_the_title_only() {
        let named = folded("Claude Code — pull request #12");
        assert!(mentions(&named, "Claude Code"));
        assert!(mentions(&named, "claude"));
        assert!(!mentions(&named, "Code review"));
        assert!(!mentions(&folded("Cardiff weather"), "card"));
    }

    #[test]
    fn without_context_an_oversized_dictionary_keeps_its_first_entries() {
        let hints: Vec<String> = (0..100).map(|i| format!("term{i}")).collect();
        let chosen = select(&hints, &HintContext::default(), budget_beside(""));
        assert!(chosen.len() < hints.len());
        assert_eq!(chosen, hints[..chosen.len()]);
    }

    #[test]
    fn the_preceding_text_reduces_the_budget() {
        let suffix = "and then we merge the worktrees once the card is finished ".repeat(8);
        assert_eq!(budget_beside(""), HINT_TOKENS);
        assert_eq!(budget_beside("short"), HINT_TOKENS);
        assert!(budget_beside(&suffix) < HINT_TOKENS);
    }

    #[test]
    fn mentions_are_whole_words_and_multiword_hints_need_every_word() {
        let hints = list(&["card", "Claude Code", "PRs", "merge"]);
        assert_eq!(
            mentioned(
                "Merge the PRs, then ask Claude about the Cardiff card",
                &hints
            ),
            list(&["card", "PRs", "merge"])
        );
    }

    #[test]
    fn usage_ranks_by_count_and_stays_bounded() {
        let mut usage = HintUsage::default();
        usage.add("Brave", &list(&["merge", "card"]));
        usage.add("Brave", &list(&["merge"]));
        assert_eq!(usage.used_in("Brave"), list(&["merge", "card"]));
        assert!(usage.used_in("Slack").is_empty());

        let many: Vec<String> = (0..100).map(|i| format!("t{i}")).collect();
        usage.add("Brave", &many);
        assert_eq!(usage.0["Brave"].len(), HintUsage::TERMS);
        assert_eq!(usage.0["Brave"]["merge"], 2);
        for i in 0..40 {
            usage.add(&format!("App {i}"), &list(&["card"]));
        }
        assert_eq!(usage.0.len(), HintUsage::APPS);
        assert!(usage.0.contains_key("Brave"));
        let saved = serde_json::to_string(&usage).unwrap();
        assert_eq!(serde_json::from_str::<HintUsage>(&saved).unwrap(), usage);
    }

    #[test]
    fn contexts_are_bounded_and_default_when_unknown() {
        let first = uuid::Uuid::new_v4();
        set_context(
            first,
            HintContext {
                app: Some("Slack".into()),
                ..Default::default()
            },
        );
        assert_eq!(context(first).app.as_deref(), Some("Slack"));
        for _ in 0..40 {
            set_context(uuid::Uuid::new_v4(), HintContext::default());
        }
        assert_eq!(context(first), HintContext::default());
    }

    /// Scorecard replay (MW-122): `WRITER_SCOPE_INPUT` names a private JSON
    /// file `{"hints": [...], "clips": {id: {"app", "title", "recent",
    /// "used"}}}`; the hints each clip's request would carry are written to
    /// `WRITER_SCOPE_OUTPUT` as `{id: [...]}`. Prints counts only.
    #[test]
    #[ignore]
    fn scope_replay() {
        let input: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("WRITER_SCOPE_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let strings = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let hints = strings(&input["hints"]);
        let mut out = serde_json::Map::new();
        let budget = std::env::var("WRITER_SCOPE_BUDGET")
            .ok()
            .and_then(|b| b.parse().ok())
            .unwrap_or_else(|| budget_beside(""));
        for (id, clip) in input["clips"].as_object().unwrap() {
            let context = HintContext {
                app: clip["app"].as_str().map(str::to_owned),
                title: clip["title"].as_str().map(str::to_owned),
                recent: strings(&clip["recent"]),
                used: strings(&clip["used"]),
            };
            out.insert(id.clone(), select(&hints, &context, budget).into());
        }
        println!("{} clips scoped from {} hints", out.len(), hints.len());
        std::fs::write(
            std::env::var("WRITER_SCOPE_OUTPUT").unwrap(),
            serde_json::to_vec(&out).unwrap(),
        )
        .unwrap();
    }
}
