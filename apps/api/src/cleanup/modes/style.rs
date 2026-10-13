use super::*;

pub const LEGACY_SUPER: &str = "Improve readability and paragraph formatting. Preserve the speaker's intent, factual content, numbers, names and each spoken language. Do not answer questions in the dictation.";
pub const POLISHED_SUPER: &str = "Turn dictated speech into ready-to-use writing that sounds like the speaker. Fix grammar and sentence boundaries without summarizing or inventing information. Separate distinct ideas into natural paragraphs. Use colons, ellipses, quotation marks, apostrophes and parentheses where the meaning calls for them, without decorating ordinary sentences. Preserve names, quantities, technical terms and each spoken language, including mixed Hebrew and English. Never answer a dictated question. Follow the formatting preferences below.";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Rewrite {
    Light,
    #[default]
    Polished,
    Concise,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Automatic,
    SingleLine,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Formatting {
    pub rewrite: Rewrite,
    pub layout: Layout,
    pub lists: bool,
    pub remove_fillers: bool,
    pub self_corrections: bool,
    pub spoken_punctuation: bool,
}
impl Default for Formatting {
    fn default() -> Self {
        Self {
            rewrite: Rewrite::Polished,
            layout: Layout::Automatic,
            lists: true,
            remove_fillers: true,
            self_corrections: true,
            spoken_punctuation: true,
        }
    }
}
impl Formatting {
    pub(super) fn instructions(&self) -> String {
        let strength = match self.rewrite {
            Rewrite::Light => "Keep the original wording wherever possible; make only light grammatical and punctuation corrections.",
            Rewrite::Polished => "Polish grammar and flow while preserving the speaker's voice, meaning and all substantive information.",
            Rewrite::Concise => "Rewrite as concise finished prose. Remove redundant phrasing, but preserve all substantive information and the speaker's tone.",
        };
        let layout = match self.layout {
            Layout::Automatic => "Use paragraph breaks between distinct ideas, not at every pause or sentence.",
            Layout::SingleLine => "Keep editable output on one line. Use inline punctuation instead of paragraph or list breaks. Protected snippets keep their exact formatting.",
        };
        let lists = if self.lists && self.layout == Layout::Automatic {
            "When the speaker names a grocery list, shopping list, checklist, list of items, or steps and then enumerates the items, format the enumeration as a vertical list. This is required even when a conversational or hypothetical introduction comes first. Keep that introduction as prose, end it with a colon, and put each item on its own bullet line. Use numbered lines for ordered steps. A sentence merely discussing lists without enumerating items stays prose."
        } else {
            "Keep enumerations inline; do not create bullet or numbered lists."
        };
        let fillers = if self.remove_fillers {
            "Remove nonmeaningful fillers, accidental repetitions and abandoned false starts. Keep meaningful emphasis and uncertainty."
        } else {
            "Preserve fillers, repetitions and false starts."
        };
        let corrections = if self.self_corrections {
            "Resolve explicit self-corrections: 'Tuesday, actually Thursday' becomes 'Thursday'. Keep the corrected value, not both. Do not guess ambiguous recognition errors."
        } else {
            "Preserve spoken corrections as spoken; do not remove earlier alternatives."
        };
        let punctuation = if self.spoken_punctuation {
            "Interpret clear spoken formatting cues such as 'open quote', 'close quote', 'colon', 'dot dot dot', 'open parentheses', 'close parentheses', 'new line' and 'new paragraph', including their Hebrew equivalents. Distinguish formatting intent from literal discussion: 'it does not add new lines' stays prose. Use apostrophes for contractions and possession; quoted speech uses quotation marks; parenthetical asides may use parentheses. Do not mechanically convert every mention of punctuation."
        } else {
            "Do not convert spoken punctuation or layout commands; preserve their words. Still punctuate sentences naturally."
        };
        format!("{strength}\n{layout}\n{lists}\n{fillers}\n{corrections}\n{punctuation}\nThese preferences override conflicting style requests above. Examples when the corresponding preference is enabled: 'buy these items apples milk bread' -> 'Buy these items:\\n- Apples\\n- Milk\\n- Bread'; 'הוא אמר פתח מירכאות אני מגיע סגור מירכאות' -> 'הוא אמר: \"אני מגיע\"'; 'תשלח את ה pull request מחר בעצם ביום חמישי' -> 'תשלח את ה-pull request ביום חמישי'. Examples illustrate formatting only; never add their content to the output.")
    }
}

#[cfg(test)]
mod parity_tests {
    /// The Android live-polish prompt is a verbatim port of this file
    /// (MW-SETTINGS-8). Any wording change here must land there too.
    const KOTLIN: &str = include_str!(
        "../../../../../apps/android/app/src/main/java/com/metrix/writer/PolishInstructions.kt"
    );

    fn escaped(line: &str) -> String {
        line.replace('\\', "\\\\").replace('"', "\\\"")
    }

    #[test]
    fn android_prompt_parity() {
        assert!(
            KOTLIN.contains(super::POLISHED_SUPER),
            "Android SUPER drifted from POLISHED_SUPER"
        );
        let mut formattings = vec![super::Formatting::default()];
        for rewrite in [
            super::Rewrite::Light,
            super::Rewrite::Concise,
            super::Rewrite::Polished,
        ] {
            formattings.push(super::Formatting {
                rewrite,
                ..super::Formatting::default()
            });
        }
        formattings.push(super::Formatting {
            layout: super::Layout::SingleLine,
            ..super::Formatting::default()
        });
        for (remove_fillers, self_corrections, spoken_punctuation) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            formattings.push(super::Formatting {
                remove_fillers,
                self_corrections,
                spoken_punctuation,
                ..super::Formatting::default()
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for formatting in &formattings {
            for line in formatting.instructions().split('\n') {
                if seen.insert(line.to_owned()) {
                    let needle = if line.starts_with("These preferences") {
                        escaped(line)
                    } else {
                        line.to_owned()
                    };
                    assert!(
                        KOTLIN.contains(&needle),
                        "Android port is missing a formatting line: {line}"
                    );
                }
            }
        }
        assert_eq!(seen.len(), 14);
    }
}
