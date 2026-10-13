//! Mac physical positions mapped into the shared chord vocabulary. Never text.
use super::Result;
pub const FN: i32 = 0xff;
pub fn modifier(part: &str) -> Option<i32> {
    match part {
        "FN" | "GLOBE" => Some(FN),
        "CMD" | "COMMAND" | "⌘" | "WIN" | "SUPER" | "META" => Some(0x5b),
        "OPTION" | "OPT" | "ALT" | "⌥" => Some(0x12),
        "CTRL" | "CONTROL" | "⌃" => Some(0x11),
        "SHIFT" | "⇧" => Some(0x10),
        _ => None,
    }
}
pub fn keys(chord: &str) -> Result<Vec<i32>> {
    if chord.len() > 80 {
        return Err("Invalid shortcut.".into());
    }
    let upper = chord.to_uppercase();
    let parts: Vec<_> = upper.split('+').map(str::trim).collect();
    let mut keys = Vec::new();
    let mut main = false;
    for (index, part) in parts.iter().enumerate() {
        let key = if let Some(key) = modifier(part) {
            key
        } else {
            if index == 0 || index != parts.len() - 1 {
                return Err("Use fn, ⌘, ⌥, ⌃ or ⇧ with a key.".into());
            }
            main = true;
            match *part {
                "SPACE" => 0x20,
                "ESCAPE" => 0x1b,
                "BACKSPACE" => 8,
                _ if part.len() == 1 && part.as_bytes()[0].is_ascii_alphanumeric() => {
                    i32::from(part.as_bytes()[0])
                }
                _ => part
                    .strip_prefix('F')
                    .and_then(|s| s.parse::<i32>().ok())
                    .filter(|n| (1..=20).contains(n))
                    .map(|n| 0x6f + n)
                    .ok_or("Use a letter, digit, F1–F20, Space, Escape or Backspace.")?,
            }
        };
        if keys.contains(&key) {
            return Err("Duplicate modifier.".into());
        }
        keys.push(key);
    }
    if !main && keys.len() < 2 && keys != [FN] {
        return Err(
            "Use fn alone, or at least two modifiers. A bare ⌘ or ⌥ cannot be a shortcut.".into(),
        );
    }
    Ok(keys)
}
// Apple virtual key codes (ANSI positions, independent of the input source).
pub fn physical(code: u16) -> i32 {
    match code {
        63 => FN,
        55 => 0x5b,
        54 => 0x5c,
        56 => 0xa0,
        60 => 0xa1,
        59 => 0xa2,
        62 => 0xa3,
        58 => 0xa4,
        61 => 0xa5,
        0 => 65,
        11 => 66,
        8 => 67,
        2 => 68,
        14 => 69,
        3 => 70,
        5 => 71,
        4 => 72,
        34 => 73,
        38 => 74,
        40 => 75,
        37 => 76,
        46 => 77,
        45 => 78,
        31 => 79,
        35 => 80,
        12 => 81,
        15 => 82,
        1 => 83,
        17 => 84,
        32 => 85,
        9 => 86,
        13 => 87,
        7 => 88,
        16 => 89,
        6 => 90,
        29 => 48,
        18 => 49,
        19 => 50,
        20 => 51,
        21 => 52,
        23 => 53,
        22 => 54,
        26 => 55,
        28 => 56,
        25 => 57,
        122 => 112,
        120 => 113,
        99 => 114,
        118 => 115,
        96 => 116,
        97 => 117,
        98 => 118,
        100 => 119,
        101 => 120,
        109 => 121,
        103 => 122,
        111 => 123,
        105 => 124,
        107 => 125,
        113 => 126,
        106 => 127,
        64 => 128,
        79 => 129,
        80 => 130,
        90 => 131,
        49 => 0x20,
        53 => 0x1b,
        51 => 8,
        _ => 0x1000 + i32::from(code),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_chords_and_legacy_aliases() {
        assert_eq!(keys("fn").unwrap(), [FN]);
        assert_eq!(keys("⌘+⌥").unwrap(), keys("Win+Alt").unwrap());
        assert_eq!(keys("Ctrl+Shift+F9").unwrap(), [0x11, 0x10, 120]);
        assert_eq!(keys("fn+F12").unwrap(), [FN, 123]);
        for invalid in [
            "",
            "⌘",
            "Alt",
            "R",
            "Fn+Globe",
            "Cmd+Win",
            "Ctrl+R+Shift",
            "Ctrl+F21",
        ] {
            assert!(keys(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn mapping_matches_chord_parser() {
        for (code, chord) in [
            (63, "fn"),
            (14, "Ctrl+E"),
            (101, "Ctrl+F9"),
            (111, "Ctrl+F12"),
        ] {
            assert_eq!(
                super::super::hotkey_state::normalized(physical(code)),
                *keys(chord).unwrap().last().unwrap()
            );
        }
    }
}
