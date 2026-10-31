//! Recover the presentation window without reloading the recording owner.
use super::*;
use crate::panel_health::Check;

pub fn create(app: &tauri::AppHandle) -> tauri::Result<WebviewWindow> {
    #[cfg(windows)]
    {
        *app.state::<Shell>().panel_surface.lock().unwrap() = None;
    }
    let builder = WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App("desktop-overlay.html".into()),
    )
    .data_directory(app.state::<Shell>().root.join("PanelWebView2"))
    .title(concat!(product_name!(), " recording"))
    .inner_size(112., 44.)
    .decorations(false)
    .resizable(false)
    .transparent(!cfg!(windows))
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .focusable(false)
    .visible(false)
    .on_navigation(|url| security::document_url("overlay", url))
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    #[cfg(windows)]
    let builder = builder
        .background_color(super::panel_surface::background(&app.state::<Shell>()))
        .initialization_script("window.writerPanelOpaque = true;");
    startup_log::mark(&app.state::<Shell>().root, "panel build starting");
    let window = builder.build()?;
    startup_log::mark(&app.state::<Shell>().root, "panel build returned");
    #[cfg(windows)]
    super::panel_surface::refresh(&window);
    startup_log::mark(&app.state::<Shell>().root, "panel surface refresh returned");
    Ok(window)
}

pub(super) fn record(app: &tauri::AppHandle, reason: &str, accepted: bool) {
    // Retain a bounded sequence: manual Restore used to overwrite the evidence
    // of what happened immediately before it. No screen pixels or user content.
    let state = app.state::<Shell>();
    let _guard = state.panel_diagnostics.lock().unwrap();
    let receipt = json!({"at_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(), "reason": reason, "accepted": accepted});
    let _ = storage::atomic_json(&state.root.join("State/panel-recovery.json"), &receipt);
    let path = state.root.join("State/panel-diagnostics.json");
    let mut history: Vec<Value> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if history.len() >= 64 {
        history.drain(..history.len() - 63);
    }
    history.push(receipt);
    let _ = storage::atomic_json(&path, &history);
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
            let result = inspect(&panel, view, nonce);
            let state = panel.state::<Shell>();
            let mut health = state.panel_health.lock().unwrap();
            let was_blank = health.recovering();
            match result {
                Presentation::Painted => health.acknowledge_if(nonce, true),
                Presentation::Blank => health.acknowledge_if(nonce, false),
                Presentation::Unavailable => health.defer(nonce),
            }
            let is_blank = health.recovering();
            drop(health);
            // WithWebview holds Wry's view-ID lock on this thread. Emitting an
            // event evaluates script and needs the same lock; enqueue it from
            // a worker after returning from the native inspection callback.
            let app = panel.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = app.emit_to("overlay", "writer-panel-checked", json!({"nonce": nonce}));
            });
            if was_blank != is_blank {
                record(
                    panel.app_handle(),
                    if is_blank {
                        "presentation_challenge_missing"
                    } else {
                        "presentation_challenge_recovered"
                    },
                    !is_blank,
                );
            }
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

/// Wake on appearances instead of relying on a two-second poll to notice
/// a short dictation. This touches only health state, never the recording owner.
pub fn visibility(window: &WebviewWindow, visible: bool) {
    let state = window.state::<Shell>();
    let changed = {
        let mut health = state.panel_health.lock().unwrap();
        health.visibility(state.panel_health_started.elapsed(), visible)
    };
    if changed {
        state.panel_health_wake.notify_one();
    }
}

pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        record(&app, "watchdog_started", true);
        loop {
            let state = app.state::<Shell>();
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {},
                _ = state.panel_health_wake.notified() => {},
            }
            if state.quitting.load(Ordering::Acquire) {
                break;
            }
            let snapshot = state.snapshot.lock().unwrap().clone();
            let show = snapshot.as_ref().is_some_and(|s| panel_shown(&state, s));
            let (check, blank) = {
                let mut health = state.panel_health.lock().unwrap();
                let check = health.check(state.panel_health_started.elapsed(), show);
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
            // A topmost panel can be left behind an ordinary window while shown;
            // the presentation check treats that as covered and only defers.
            #[cfg(windows)]
            if super::panel_surface::buried(&window) {
                super::panel_surface::raise(&window);
                record(&app, "topmost_order_restored", true);
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
                            "nonce": nonce, "presentation": cfg!(windows), "snapshot": snapshot, "preferences": preferences,
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
