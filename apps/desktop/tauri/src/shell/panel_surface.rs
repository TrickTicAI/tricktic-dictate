//! Windows owns the pill's shape, rather than an alpha-composited WebView.
//! An opaque HWND/WebView avoids Tao's BlurBehind + transparent softbuffer path.
use super::*;
use windows::Win32::Foundation::RECT;

#[link(name = "user32")]
extern "system" {
    fn GetClientRect(window: isize, rect: *mut RECT) -> i32;
    fn GetDpiForWindow(window: isize) -> u32;
    fn SetWindowRgn(window: isize, region: isize, redraw: i32) -> i32;
    fn GetWindowRect(window: isize, rect: *mut RECT) -> i32;
    fn GetWindow(window: isize, command: u32) -> isize;
    fn GetWindowLongPtrW(window: isize, index: i32) -> isize;
    fn IsWindowVisible(window: isize) -> i32;
    fn IsIconic(window: isize) -> i32;
    fn SetWindowPos(
        window: isize,
        after: isize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
}
#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(window: isize, attribute: u32, value: *mut u32, size: u32) -> i32;
}
#[link(name = "gdi32")]
extern "system" {
    fn CreateRoundRectRgn(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        width: i32,
        height: i32,
    ) -> isize;
    fn CombineRgn(destination: isize, first: isize, second: isize, mode: i32) -> i32;
    fn DeleteObject(object: isize) -> i32;
}

#[derive(Clone, PartialEq)]
pub struct Surface {
    hwnd: isize,
    width: i32,
    height: i32,
    unit: f64,
    badge: bool,
    rgb: (u8, u8, u8),
}

pub fn background(state: &Shell) -> tauri::window::Color {
    let preferences = state.preferences.lock().unwrap().clone();
    let theme = state.theme.lock().unwrap().clone();
    let hex = preferences
        .panel_colors
        .as_ref()
        .map(|c| c.background.as_str())
        .or_else(|| theme.as_ref().map(|t| t.bg.as_str()))
        .unwrap_or("#08090b");
    let (r, g, b) = crate::window_memory::colour(hex).unwrap_or((8, 9, 11));
    tauri::window::Color(r, g, b, 255)
}

/// Run on the native event thread, after the size/DPI change. Do not use
/// with_webview: its dispatch holds the WebView ID lock, and changing its
/// background from that callback would reenter the same lock. Cache before
/// SetWindowRgn: that API itself sends window-position messages synchronously.
pub fn refresh(window: &WebviewWindow) {
    let panel = window.clone();
    tauri::async_runtime::spawn(async move {
        let dispatcher = panel.clone();
        let _ = dispatcher.run_on_main_thread(move || {
            let Ok(hwnd) = panel.hwnd() else {
                return;
            };
            let hwnd = hwnd.0 as isize;
            let mut rect = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
                return;
            }
            let state = panel.state::<Shell>();
            let scale = state.preferences.lock().unwrap().scale;
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
            let colour = background(&state);
            let surface = Surface {
                hwnd,
                width: rect.right,
                height: rect.bottom,
                unit: scale * f64::from(dpi) / 96.0,
                badge: state.waiting.load(Ordering::Acquire) > 0,
                rgb: (colour.0, colour.1, colour.2),
            };
            {
                let mut last = state.panel_surface.lock().unwrap();
                if last.as_ref() == Some(&surface) {
                    return;
                }
                *last = Some(surface.clone());
            }
            let coloured = panel.set_background_color(Some(colour)).is_ok();
            let shaped = unsafe { shape(&surface) };
            if !coloured || !shaped {
                *state.panel_surface.lock().unwrap() = None;
                super::panel_recovery::record(
                    panel.app_handle(),
                    "opaque_surface_configuration_failed",
                    false,
                );
            }
        });
    });
}

/// Put the panel back at the top of the topmost band and report whether an
/// ordinary window had been stacked above it. Windows can leave a WS_EX_TOPMOST
/// window below normal windows while keeping the style; Tao then treats
/// set_always_on_top(true) as unchanged and never calls SetWindowPos, so the
/// panel kept appearing behind a maximized window until Restore recreated it.
/// Asynchronous: safe from any thread, never activates or moves the panel.
pub fn raise(window: &WebviewWindow) -> bool {
    let Ok(hwnd) = window.hwnd() else {
        return false;
    };
    let buried = buried(window);
    // HWND_TOPMOST; SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS
    let flags = 0x0001 | 0x0002 | 0x0010 | 0x0200 | 0x4000;
    unsafe { SetWindowPos(hwnd.0 as isize, -1, 0, 0, 0, 0, flags) };
    buried
}

/// Whether a visible, non-topmost window that overlaps the panel precedes it
/// in the Z order. Topmost windows above it are legitimate and not reported.
pub fn buried(window: &WebviewWindow) -> bool {
    let Ok(hwnd) = window.hwnd() else {
        return false;
    };
    let hwnd = hwnd.0 as isize;
    unsafe {
        let mut panel = RECT::default();
        if IsWindowVisible(hwnd) == 0 || GetWindowRect(hwnd, &mut panel) == 0 {
            return false;
        }
        let mut above = hwnd;
        for _ in 0..1024 {
            above = GetWindow(above, 3); // GW_HWNDPREV
            if above == 0 {
                return false;
            }
            let mut cloaked = 0u32;
            let mut rect = RECT::default();
            if IsWindowVisible(above) == 0
                || IsIconic(above) != 0
                || GetWindowLongPtrW(above, -20) & 0x8 != 0 // GWL_EXSTYLE, WS_EX_TOPMOST
                || (DwmGetWindowAttribute(above, 14, &mut cloaked, 4) == 0 && cloaked != 0) // DWMWA_CLOAKED
                || GetWindowRect(above, &mut rect) == 0
            {
                continue;
            }
            if rect.left < panel.right
                && panel.left < rect.right
                && rect.top < panel.bottom
                && panel.top < rect.bottom
            {
                return true;
            }
        }
        false
    }
}

unsafe fn shape(s: &Surface) -> bool {
    if s.width <= 0 || s.height <= 0 {
        return false;
    }
    // Same 3 logical-pixel margin and 16-pixel badge as desktop-overlay.css.
    let px = |value: f64| (value * s.unit).round() as i32;
    let inset = px(3.0);
    let diameter = (s.height - 2 * inset).max(1);
    let region = CreateRoundRectRgn(
        inset,
        inset,
        s.width - inset + 1,
        s.height - inset + 1,
        diameter,
        diameter,
    );
    if region == 0 {
        return false;
    }
    if s.badge {
        // Include the badge's 2-pixel outer ring; Windows clips at the HWND edge.
        let badge = CreateRoundRectRgn(
            s.width - px(20.0),
            -px(2.0),
            s.width + 1,
            px(18.0) + 1,
            px(20.0),
            px(20.0),
        );
        let combined = badge != 0 && CombineRgn(region, region, badge, 2 /* RGN_OR */) != 0;
        if badge != 0 {
            DeleteObject(badge);
        }
        if !combined {
            DeleteObject(region);
            return false;
        }
    }
    // Ownership transfers only on success. Never delete an installed region.
    if SetWindowRgn(s.hwnd, region, 1) == 0 {
        DeleteObject(region);
        return false;
    }
    true
}
