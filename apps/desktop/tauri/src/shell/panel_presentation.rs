//! Inspect only the panel's opaque fill. No screen image or pixel values are
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

/// Only an opaque CSS colour can be compared with a displayed pixel. Unknown
/// or translucent themes do not turn into a false report of a blank panel.
fn colour(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let mut rgb = [0; 3];
            for (i, channel) in rgb.iter_mut().enumerate() {
                *channel = u8::from_str_radix(hex.get(i..i + 1)?, 16).ok()? * 17;
            }
            Some(rgb)
        }
        6 | 8 if hex.len() == 6 || hex.ends_with("ff") || hex.ends_with("FF") => Some([
            u8::from_str_radix(hex.get(0..2)?, 16).ok()?,
            u8::from_str_radix(hex.get(2..4)?, 16).ok()?,
            u8::from_str_radix(hex.get(4..6)?, 16).ok()?,
        ]),
        _ => None,
    }
}

fn matches(pixel: u32, expected: [u8; 3]) -> bool {
    expected
        .into_iter()
        .enumerate()
        .all(|(channel, value)| ((pixel >> (channel * 8)) as u8).abs_diff(value) <= 12)
}

/// Run on the WebView's event thread, after the two frame callbacks. These
/// points lie in the pill's fill above/below the bars, dots and controls. Skip
/// pixels covered by another window (including the locked/secure desktop).
pub fn inspect(window: &WebviewWindow, view: tauri::webview::PlatformWebview) -> Presentation {
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
    let state = window.state::<Shell>();
    let own = state.preferences.lock().unwrap().panel_colors.clone();
    let theme = state.theme.lock().unwrap().clone();
    let background = own
        .as_ref()
        .map(|c| c.background.as_str())
        .or_else(|| theme.as_ref().map(|t| t.bg.as_str()))
        .unwrap_or("#08090b");
    let Some(expected) = colour(background) else {
        return Presentation::Unavailable;
    };
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
        let mut painted = false;
        for (across, down) in [(35, 18), (65, 18), (35, 82), (65, 82)] {
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
            let pixel = GetPixel(dc, point.x, point.y);
            if pixel == u32::MAX {
                continue;
            }
            sampled += 1;
            painted |= matches(pixel, expected);
        }
        ReleaseDC(0, dc);
        if painted {
            Presentation::Painted
        } else if sampled >= 2 {
            Presentation::Blank
        } else {
            Presentation::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_opaque_colours_can_confirm_presentation() {
        assert_eq!(colour("#aB3"), Some([170, 187, 51]));
        assert_eq!(colour("#123456"), Some([18, 52, 86]));
        assert_eq!(colour("#123456FF"), Some([18, 52, 86]));
        for value in ["#12345600", "#abc8", "#xyz", "#1", "red", "#💥"] {
            assert_eq!(colour(value), None);
        }
    }

    #[test]
    fn displayed_fill_allows_small_colour_variation_and_rejects_another_surface() {
        assert!(matches(0x563412, [18, 52, 86]));
        assert!(matches(0x62401e, [18, 52, 86]));
        assert!(!matches(0x303030, [0, 0, 0]));
    }
}
