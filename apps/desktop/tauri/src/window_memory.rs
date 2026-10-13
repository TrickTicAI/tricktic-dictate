//! Where the main window was, how big, and in what colour, kept between
//! launches.
//!
//! The frame is stored in physical pixels exactly as the window reported it and
//! checked against the displays present at the next launch: a window last seen
//! on a monitor that has since been unplugged opens at the default place rather
//! than somewhere nobody can reach it. The colour is the theme's page colour,
//! so a dark theme does not open as a white rectangle before the page paints.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    /// The last frame the window had while neither maximised nor minimised.
    pub frame: Option<Frame>,
    pub maximized: bool,
    /// The theme's page colour, as `#rrggbb`.
    pub background: Option<String>,
}

/// A window's outer position and inner size, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A display's work area in physical pixels, and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

/// How much of the window's top edge has to be on a display for the saved
/// place to be used: enough of the title bar to grab and drag it back.
const GRAB: (u32, u32) = (160, 32);

impl Memory {
    /// The saved memory, or none at all when there is nothing readable: a
    /// missing or damaged file only costs the window its last place.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

/// The saved frame on the display that holds most of its title bar, moved and
/// shrunk as little as needed to fit that display, with the display's scale.
/// `None` when no display holds enough of it to grab. `minimum` is the
/// window's minimum inner size in logical pixels.
pub fn restore(frame: Frame, displays: &[Display], minimum: (f64, f64)) -> Option<(Frame, f64)> {
    let display = displays
        .iter()
        .copied()
        .map(|display| (display, title_bar_on(frame, display)))
        .filter(|(_, (width, height))| {
            *width >= GRAB.0.min(frame.width) && *height >= GRAB.1.min(frame.height)
        })
        .max_by_key(|(_, (width, height))| u64::from(*width) * u64::from(*height))?
        .0;
    let least = |logical: f64, room: u32| ((logical * display.scale).round() as u32).min(room);
    let width = frame
        .width
        .min(display.width)
        .max(least(minimum.0, display.width));
    let height = frame
        .height
        .min(display.height)
        .max(least(minimum.1, display.height));
    let right = display.x + display.width as i32 - width as i32;
    let bottom = display.y + display.height as i32 - height as i32;
    Some((
        Frame {
            x: frame.x.clamp(display.x, right.max(display.x)),
            y: frame.y.clamp(display.y, bottom.max(display.y)),
            width,
            height,
        },
        display.scale,
    ))
}

/// The part of the window's top strip that lies on `display`, as width and
/// height in pixels.
fn title_bar_on(frame: Frame, display: Display) -> (u32, u32) {
    let overlap = |start: i32, length: u32, from: i32, size: u32| {
        let end = (i64::from(start) + i64::from(length)).min(i64::from(from) + i64::from(size));
        (end - i64::from(start.max(from))).max(0) as u32
    };
    (
        overlap(frame.x, frame.width, display.x, display.width),
        overlap(frame.y, GRAB.1.min(frame.height), display.y, display.height),
    )
}

/// `#rgb` or `#rrggbb`, with any alpha ignored: a window is never see-through.
pub fn colour(hex: &str) -> Option<(u8, u8, u8)> {
    let digits = hex.strip_prefix('#')?;
    let channel = |at: usize, width: usize| {
        let part = digits.get(at * width..at * width + width)?;
        let value = u8::from_str_radix(part, 16).ok()?;
        Some(if width == 1 { value * 17 } else { value })
    };
    match digits.len() {
        3 | 4 => Some((channel(0, 1)?, channel(1, 1)?, channel(2, 1)?)),
        6 | 8 => Some((channel(0, 2)?, channel(1, 2)?, channel(2, 2)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Display = Display {
        x: 0,
        y: 0,
        width: 2560,
        height: 1560,
        scale: 1.5,
    };
    const DESK: Display = Display {
        x: 2560,
        y: -200,
        width: 1920,
        height: 1040,
        scale: 1.0,
    };
    const MINIMUM: (f64, f64) = (760.0, 560.0);

    fn frame(x: i32, y: i32, width: u32, height: u32) -> Frame {
        Frame {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_window_that_fits_opens_exactly_where_it_was() {
        let saved = frame(300, 120, 1680, 1200);
        assert_eq!(restore(saved, &[LAPTOP, DESK], MINIMUM), Some((saved, 1.5)));
        let there = frame(2700, -100, 1400, 900);
        assert_eq!(restore(there, &[LAPTOP, DESK], MINIMUM), Some((there, 1.0)));
    }

    #[test]
    fn a_window_on_an_unplugged_display_opens_at_the_default_place() {
        assert_eq!(
            restore(frame(2700, -100, 1400, 900), &[LAPTOP], MINIMUM),
            None
        );
        // Most of it off screen, with too little of the title bar left to grab.
        assert_eq!(
            restore(frame(2480, 300, 1400, 900), &[LAPTOP], MINIMUM),
            None
        );
        assert_eq!(
            restore(frame(200, 1540, 1400, 900), &[LAPTOP], MINIMUM),
            None
        );
        assert_eq!(restore(frame(200, 300, 1400, 900), &[], MINIMUM), None);
    }

    #[test]
    fn a_window_partly_off_its_display_is_pulled_back_and_shrunk_to_fit() {
        // Hanging off the right edge, but with its title bar still grabbable.
        let (moved, scale) = restore(frame(2000, 100, 1400, 900), &[LAPTOP], MINIMUM).unwrap();
        assert_eq!(scale, 1.5);
        assert_eq!(moved, frame(1160, 100, 1400, 900));
        // Bigger than the smaller display it was left on.
        let (shrunk, _) = restore(frame(2600, -150, 2400, 1400), &[LAPTOP, DESK], MINIMUM).unwrap();
        assert_eq!(shrunk, frame(2560, -200, 1920, 1040));
        // Never below the window's own minimum, in that display's pixels.
        let (grown, _) = restore(frame(100, 100, 300, 200), &[LAPTOP], MINIMUM).unwrap();
        assert_eq!((grown.width, grown.height), (1140, 840));
    }

    #[test]
    fn the_title_bar_decides_which_display_a_window_belongs_to() {
        // Straddling both displays: most of its title bar is on the desk one.
        let (placed, scale) = restore(frame(2300, 0, 1600, 900), &[LAPTOP, DESK], MINIMUM).unwrap();
        assert_eq!(scale, 1.0);
        assert_eq!(placed.x, 2560);
    }

    #[test]
    fn memory_survives_a_round_trip_and_a_damaged_file_is_forgotten() {
        let root =
            std::env::temp_dir().join(format!("writer-window-memory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("window.json");
        let memory = Memory {
            frame: Some(frame(10, 20, 1200, 800)),
            maximized: true,
            background: Some("#282a36".into()),
        };
        crate::storage::atomic_json(&path, &memory).unwrap();
        assert_eq!(Memory::load(&path), memory);
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Memory::load(&path), Memory::default());
        assert_eq!(Memory::load(&root.join("missing.json")), Memory::default());
        // Fields added later default rather than discarding the rest.
        std::fs::write(&path, br#"{"maximized":true}"#).unwrap();
        assert!(Memory::load(&path).maximized);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn theme_colours_become_opaque_window_colours() {
        assert_eq!(colour("#282A36"), Some((0x28, 0x2a, 0x36)));
        assert_eq!(colour("#fff"), Some((255, 255, 255)));
        assert_eq!(colour("#28283680"), Some((0x28, 0x28, 0x36)));
        assert_eq!(colour("#abcd"), Some((0xaa, 0xbb, 0xcc)));
        for bad in ["", "282a36", "#28", "#gggggg", "#12345"] {
            assert_eq!(colour(bad), None, "{bad}");
        }
    }
}
