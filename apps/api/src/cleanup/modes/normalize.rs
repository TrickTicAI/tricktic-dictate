//! Deterministic typography for model-written prose. Never applied to exact values.
pub(super) fn polished(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{2011}' => out.push('-'),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '–' | '—' => {
                // Numeric ranges retain their meaning with an ASCII hyphen.
                let left = out.chars().next_back();
                let right = chars.peek().copied();
                if left.is_some_and(|c| c.is_ascii_digit())
                    && right.is_some_and(|c| c.is_ascii_digit())
                {
                    out.push('-');
                    continue;
                }
                while out.ends_with([' ', '\t']) {
                    out.pop();
                }
                if out.is_empty() || out.ends_with('\n') {
                    out.push('-'); // A list marker, not a prose break.
                } else if !out.ends_with([',', ':', ';', '.', '!', '?']) {
                    out.push(',');
                }
                while chars.peek().is_some_and(|c| matches!(c, ' ' | '\t')) {
                    chars.next();
                }
                if chars.peek().is_some_and(|c| !c.is_whitespace()) {
                    out.push(' ');
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prose_dashes_apostrophes_and_identifiers() {
        assert_eq!(
            polished(
                "It’s ready — ship it–today. CRM-236 10-20 -5 state-of-the-art 10–20 non‑breaking"
            ),
            "It's ready, ship it, today. CRM-236 10-20 -5 state-of-the-art 10-20 non-breaking"
        );
        assert_eq!(polished("שלום — hello"), "שלום, hello");
        assert_eq!(polished("Tasks:\n— one\n– two"), "Tasks:\n- one\n- two");
        assert_eq!(polished("Ready: — yes"), "Ready: yes");
    }
    #[test]
    fn idempotent() {
        for value in ["word—word", "‘quote’", "a — b — c", "5–9", "\n— item"] {
            let once = polished(value);
            assert_eq!(polished(&once), once);
        }
    }
}

#[cfg(test)]
mod replay {
    #[test]
    #[ignore = "Requires private WRITER_DASH_REPLAY rows.json; prints counts only"]
    fn weekly_outputs() {
        let rows: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var_os("WRITER_DASH_REPLAY").expect("private rows path"))
                .unwrap(),
        )
        .unwrap();
        let mut before = 0;
        let mut after = 0;
        let mut count = 0;
        for row in rows.as_array().unwrap() {
            if row["original_used"] == true {
                continue;
            }
            let Some(text) = row["final"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            count += 1;
            before += usize::from(text.contains(['—', '–']));
            after += usize::from(super::polished(text).contains(['—', '–']));
        }
        println!("outputs={count} with_long_dashes_before={before} after={after}");
        assert!(count > 0);
        assert_eq!(after, 0);
    }
}
