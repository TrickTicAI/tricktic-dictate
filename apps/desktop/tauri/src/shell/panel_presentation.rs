//! Inspect only the panel's transient presentation witnesses. No screen image or pixel values are
//! retained. A live renderer can answer frames while its native surface is blank.
use super::*;
use windows::{
    core::BOOL,
    Win32::Foundation::{HWND, RECT},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Presentation {
    Painted,
    Blank,
    Unavailable,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}
#[link(name = "user32")]
extern "system" {
    fn GetWindowRect(window: isize, rect: *mut RECT) -> i32;
    fn GetDC(window: isize) -> isize;
    fn ReleaseDC(window: isize, dc: isize) -> i32;
    fn WindowFromPoint(point: Point) -> isize;
    fn GetAncestor(window: isize, flags: u32) -> isize;
    fn GetWindow(window: isize, command: u32) -> isize;
    fn IsChild(parent: isize, child: isize) -> i32;
    fn IsWindowVisible(window: isize) -> i32;
}
#[link(name = "gdi32")]
extern "system" {
    fn GetPixel(dc: isize, x: i32, y: i32) -> u32;
}

/// Mirrored by desktop-overlay.js. A fresh nonce paints a high-contrast witness,
/// so a black underlying terminal cannot satisfy the near-black panel's check.
fn challenge_colour(nonce: u64) -> [u8; 3] {
    [
        128 + (nonce & 7) as u8 * 16,
        32 + ((nonce >> 3) & 7) as u8 * 16,
        128 + ((nonce >> 6) & 3) as u8 * 32,
    ]
}

fn matches(pixel: u32, expected: [u8; 3]) -> bool {
    expected
        .into_iter()
        .enumerate()
        .all(|(channel, value)| ((pixel >> (channel * 8)) as u8).abs_diff(value) <= 6)
}

/// Run on the WebView's event thread, after the two frame callbacks. These
/// witnesses lie above the activity and away from controls. Skip
/// pixels covered by another window (including the locked/secure desktop).
pub fn inspect(
    window: &WebviewWindow,
    view: tauri::webview::PlatformWebview,
    nonce: u64,
) -> Presentation {
    let Ok(hwnd) = window.hwnd() else {
        return Presentation::Unavailable;
    };
    let hwnd = hwnd.0 as isize;
    let controller = view.controller();
    let mut visible = BOOL::default();
    let mut parent = HWND::default();
    let mut bounds = RECT::default();
    unsafe {
        if controller.IsVisible(&mut visible).is_err()
            || controller.ParentWindow(&mut parent).is_err()
            || controller.Bounds(&mut bounds).is_err()
        {
            return Presentation::Blank;
        }
        if !visible.as_bool()
            || IsWindowVisible(parent.0 as isize) == 0
            || bounds.right <= bounds.left
            || bounds.bottom <= bounds.top
        {
            return Presentation::Blank;
        }
    }
    let expected = challenge_colour(nonce);
    let mut rect = RECT::default();
    unsafe {
        if IsWindowVisible(hwnd) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
            return Presentation::Unavailable;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width < 16 || height < 16 {
            return Presentation::Blank;
        }
        let dc = GetDC(0);
        if dc == 0 {
            return Presentation::Unavailable;
        }
        let mut sampled = 0;
        let mut missing = false;
        for (across, down) in [(35, 18), (65, 18)] {
            let point = Point {
                x: rect.left + width * across / 100,
                y: rect.top + height * down / 100,
            };
            let at = WindowFromPoint(point);
            if at != hwnd && IsChild(hwnd, at) == 0 {
                // A wholly transparent browser can pass hit tests through to
                // the app behind it. Distinguish that from an actual occluder:
                // the panel precedes the hit window in the desktop Z order.
                let mut above = GetAncestor(at, 2); // GA_ROOT
                let mut behind_panel = false;
                for _ in 0..256 {
                    above = GetWindow(above, 3); // GW_HWNDPREV
                    if above == 0 {
                        break;
                    }
                    if above == hwnd {
                        behind_panel = true;
                        break;
                    }
                }
                if !behind_panel {
                    continue;
                }
            }
            // CSS percentages and native pixels round differently under DPI and
            // panel scaling. Search only the witness's tiny neighbourhood.
            let radius = (window.scale_factor().unwrap_or(1.0) * 2.0).ceil() as i32;
            let mut available = false;
            let mut found = false;
            for y in -1..=radius {
                for x in -1..=radius {
                    let pixel = GetPixel(dc, point.x + x, point.y + y);
                    if pixel != u32::MAX {
                        available = true;
                        found |= matches(pixel, expected);
                    }
                }
            }
            if available {
                sampled += 1;
                missing |= !found;
            }
        }
        ReleaseDC(0, dc);
        if missing {
            Presentation::Blank
        } else if sampled > 0 {
            Presentation::Painted
        } else {
            Presentation::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_black_surface_cannot_answer_a_presentation_challenge() {
        for nonce in 0..256 {
            let expected = challenge_colour(nonce);
            assert!(!matches(0, expected));
            assert!(!matches(0x0b0a0a, expected));
            let pixel = u32::from(expected[0])
                | (u32::from(expected[1]) << 8)
                | (u32::from(expected[2]) << 16);
            assert!(matches(pixel, expected));
            assert!(!matches(pixel, challenge_colour(nonce + 1)));
        }
        assert_eq!(challenge_colour(42), [160, 112, 128]);
    }
}
