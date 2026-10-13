//! Keeps the main window's place, size and colour between launches. What is
//! kept, and how it is checked against the displays present, is in
//! `window_memory`; this is the part that talks to the window.
use super::*;
use crate::window_memory::{self, Display, Frame, Memory};
use std::path::Path;

/// Where the memory is saved, and what it holds now.
static MEMORY: Mutex<Option<(PathBuf, Memory)>> = Mutex::new(None);
/// Bumped by every change, so only the last of a burst is written.
static CHANGES: AtomicU64 = AtomicU64::new(0);
/// The window's minimum and first-launch inner sizes, in logical pixels.
const MINIMUM: (f64, f64) = (760.0, 560.0);
const FIRST: (f64, f64) = (1120.0, 800.0);
/// A drag reports hundreds of moves; the file is written once it stops.
const SETTLE: Duration = Duration::from_millis(600);

/// The main window's builder, placed, sized and coloured as it was last time.
pub fn prepare<'a, M: tauri::Manager<tauri::Wry>>(
    app: &tauri::AppHandle,
    root: &Path,
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    let path = root.join("State/window.json");
    let memory = Memory::load(&path);
    let displays: Vec<Display> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let area = monitor.work_area();
            Display {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
                scale: monitor.scale_factor(),
            }
        })
        .collect();
    let builder = builder.min_inner_size(MINIMUM.0, MINIMUM.1);
    // The builder takes logical pixels and converts them with the scale of the
    // display they land on, so the saved physical frame is converted with the
    // scale of the display it was restored to.
    let builder = match memory
        .frame
        .and_then(|frame| window_memory::restore(frame, &displays, MINIMUM))
    {
        Some((frame, scale)) => builder
            .position(f64::from(frame.x) / scale, f64::from(frame.y) / scale)
            .inner_size(
                f64::from(frame.width) / scale,
                f64::from(frame.height) / scale,
            ),
        None => builder.inner_size(FIRST.0, FIRST.1),
    };
    let builder = builder.maximized(memory.maximized);
    let builder = match memory.background.as_deref().and_then(window_memory::colour) {
        Some((r, g, b)) => builder.background_color(tauri::window::Color(r, g, b, 255)),
        None => builder,
    };
    *MEMORY.lock().unwrap() = Some((path, memory));
    builder
}

/// The window moved or changed size.
pub fn noticed(window: &tauri::Window) {
    // A minimised window reports a parking place far off screen, and a
    // maximised one the whole work area: neither is where it should reopen.
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    let maximized = window.is_maximized().unwrap_or(false);
    let frame = match (maximized, window.outer_position(), window.inner_size()) {
        (true, _, _) => None,
        (false, Ok(position), Ok(size)) => Some(Frame {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        }),
        _ => return,
    };
    {
        let mut slot = MEMORY.lock().unwrap();
        let Some((_, memory)) = slot.as_mut() else {
            return;
        };
        memory.maximized = maximized;
        if frame.is_some() {
            memory.frame = frame;
        }
    }
    save_soon();
}

/// The theme's page colour: painted behind the page now, and used for the
/// window before the page has painted at the next launch.
pub fn background(app: &tauri::AppHandle, hex: &str) {
    let Some((r, g, b)) = window_memory::colour(hex) else {
        return;
    };
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_background_color(Some(tauri::window::Color(r, g, b, 255)));
    }
    {
        let mut slot = MEMORY.lock().unwrap();
        let Some((_, memory)) = slot.as_mut() else {
            return;
        };
        if memory.background.as_deref() == Some(hex) {
            return;
        }
        memory.background = Some(hex.to_owned());
    }
    save_soon();
}

fn save_soon() {
    let change = CHANGES.fetch_add(1, Ordering::AcqRel) + 1;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SETTLE).await;
        if CHANGES.load(Ordering::Acquire) != change {
            return;
        }
        let kept = MEMORY.lock().unwrap().clone();
        if let Some((path, memory)) = kept {
            // Losing it costs the window its last place, nothing more.
            let _ = crate::storage::atomic_json(&path, &memory);
        }
    });
}
