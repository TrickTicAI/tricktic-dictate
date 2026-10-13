//! Recover the presentation window without reloading the recording owner.
use super::*;
use crate::panel_health::Check;

pub fn create(app: &tauri::AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App("desktop-overlay.html".into()),
    )
    .data_directory(app.state::<Shell>().root.join("PanelWebView2"))
    .title(concat!(product_name!(), " recording"))
    .inner_size(112., 44.)
    .decorations(false)
    .resizable(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .focusable(false)
    .visible(false)
    .on_navigation(|url| security::document_url("overlay", url))
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .build()
}

fn record(app: &tauri::AppHandle, reason: &str, accepted: bool) {
    // One bounded receipt, no audio, transcript, session IDs or process data.
    let _ = storage::atomic_json(
        &app.state::<Shell>().root.join("State/panel-recovery.json"),
        &json!({"at_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(), "reason": reason, "accepted": accepted}),
    );
}

/// Also available when a compositor accepts frames but still paints nothing.
pub fn restore(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        record(app, "manual_restore", window.destroy().is_ok());
    }
    // The next expected appearance recreates the window, even when the user
    // invokes this between dictations. Capture and owner state are untouched.
}

/// Frame callbacks are necessary, but cannot confirm that Windows displayed
/// the surface. A blank surface keeps the existing challenge deadline running,
/// so Reload and then panel-only recreation remain the bounded recovery path.
pub fn acknowledge(window: &WebviewWindow, nonce: u64) {
    #[cfg(windows)]
    {
        let panel = window.clone();
        let _ = window.with_webview(move |view| {
            use super::panel_presentation::{inspect, Presentation};
            let result = inspect(&panel, view);
            let state = panel.state::<Shell>();
            state
                .panel_health
                .lock()
                .unwrap()
                .acknowledge_if(nonce, result != Presentation::Blank);
        });
    }
    #[cfg(not(windows))]
    window
        .state::<Shell>()
        .panel_health
        .lock()
        .unwrap()
        .acknowledge_if(nonce, true);
}

pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let state = app.state::<Shell>();
            if state.quitting.load(Ordering::Acquire) {
                break;
            }
            let snapshot = state.snapshot.lock().unwrap().clone();
            let show = snapshot.as_ref().is_some_and(|s| panel_shown(&state, s));
            let (check, blank) = {
                let mut health = state.panel_health.lock().unwrap();
                let check = health.check(started.elapsed(), show);
                (check, health.presentation_failed())
            };
            if !show {
                continue;
            }
            let Some(window) = app.get_webview_window("overlay") else {
                // Building a WebView on the native event thread avoids Windows
                // WebView2 creation deadlocks. Recheck after queued destruction.
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if handle.state::<Shell>().quitting.load(Ordering::Acquire)
                        || handle.get_webview_window("overlay").is_some()
                    {
                        return;
                    }
                    let result = create(&handle);
                    record(&handle, "missing_window_recreate", result.is_ok());
                });
                continue;
            };
            if !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(false) {
                let preferences = state.preferences.lock().unwrap().clone();
                preferences::position(&app, &preferences);
                let _ = window.set_always_on_top(true);
                overlay_visibility(&window, true);
                record(&app, "restore_visibility", true);
            }
            match check {
                Check::Idle => {}
                Check::Ping(nonce) => {
                    let preferences = state.preferences.lock().unwrap().clone();
                    let theme = state.theme.lock().unwrap().clone();
                    let _ = app.emit_to(
                        "overlay",
                        "writer-panel-sync",
                        json!({
                            "nonce": nonce, "snapshot": snapshot, "preferences": preferences,
                            "theme": theme, "waiting": state.waiting.load(Ordering::Acquire),
                        }),
                    );
                }
                Check::Reload => {
                    record(
                        &app,
                        if blank {
                            "blank_surface_reload"
                        } else {
                            "unresponsive_reload"
                        },
                        window.reload().is_ok(),
                    );
                }
                Check::Recreate => {
                    // A dead browser controller may accept Reload yet never
                    // load. Destroy only the panel; the next tick rebuilds it.
                    record(
                        &app,
                        if blank {
                            "blank_surface_destroy"
                        } else {
                            "unresponsive_destroy"
                        },
                        window.destroy().is_ok(),
                    );
                }
            }
        }
    });
}
