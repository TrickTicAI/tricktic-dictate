//! Fixed document and command authority. Labels alone never confer access.
use super::*;

pub const MAIN_COMMANDS: &[&str] = &[
    "accessibility_status",
    "accessibility_open",
    "modes_save",
    "updates_status",
    "updates_check",
    "updates_download",
    "updates_install",
    "updates_cancel",
    "list_microphones",
    "select_microphone",
    "microphone_prefer_built_in",
    "history_delete",
    "history_resume",
    "storage_status",
    "storage_preview",
    "storage_clear",
    "system_settings_get",
    "system_settings_save",
    "onboarding_get",
    "onboarding_save",
    "deliver_transcript",
    "prepare_delivery",
    "session_refused",
    "session_waiting",
    "shortcut_capture",
    "capture_start",
    "capture_import",
    "capture_preview",
    "capture_preview_start",
    "dictation_probe_receipt",
    "dictation_probe_command",
    "capture_stop",
    "capture_status",
    "capture_get",
    "capture_list",
    "capture_delete",
    "saved_attempts",
    "session_event",
    "quit_ready",
    "preferences_get",
    "preferences_save",
    "preferences_theme",
    "import_model",
    "probe_result",
    "window_command",
    "window_state",
    "request_quit_command",
    "credential_prompt",
    "credential_remove",
    "data_location",
    "open_data_folder",
    "legal_open",
    "notices_index",
    "notice_text",
    "notices_folder",
    "account_snapshot",
    "account_sign_in",
    "account_reauthenticate",
    "account_cancel_sign_in",
    "account_sign_out",
    "account_switch",
    "account_local_copy",
    "account_tasks",
    "account_sensitive",
    "portable_status",
    "portable_export",
    "portable_import_preview",
    "portable_import_apply",
    "portable_import_discard",
    "portable_rollback",
    "portable_theme",
    "sync_preview",
    "sync_confirm",
    "sync_control",
    "sync_turn_off",
    "sync_resolve",
    "transcript_status",
    "transcript_preview",
    "transcript_confirm",
    "transcript_page",
    "transcript_edit",
    "transcript_delete",
    "transcript_turn_off",
];
pub const TRAY_COMMANDS: &[&str] = &[
    "tray_menu_ready",
    "tray_menu_measured",
    "tray_menu_command",
    "tray_menu_dismiss",
];

pub fn local_url(url: &tauri::Url) -> bool {
    matches!(
        (url.scheme(), url.host_str()),
        ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost"))
    ) && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}
pub fn document_url(label: &str, url: &tauri::Url) -> bool {
    local_url(url)
        && match label {
            "main" => matches!(url.path(), "" | "/" | "/index.html"),
            "overlay" => url.path() == "/desktop-overlay.html",
            "tray-menu" => url.path() == "/desktop-tray.html",
            _ => false,
        }
}
pub fn window_only(window: &WebviewWindow, label: &str) -> Result<()> {
    if window.label() == label
        && current_url(window.as_ref()).is_some_and(|u| document_url(label, &u))
    {
        Ok(())
    } else {
        Err("Untrusted document.".into())
    }
}
pub fn command_allowed(command: &str, label: &str, url: &tauri::Url) -> bool {
    document_url(label, url)
        && match label {
            "main" => MAIN_COMMANDS.contains(&command),
            "overlay" => command == "overlay_action",
            "tray-menu" => TRAY_COMMANDS.contains(&command),
            _ => false,
        }
}
pub fn allowed<R: tauri::Runtime>(command: &str, webview: &tauri::Webview<R>) -> bool {
    current_url(webview).is_some_and(|url| command_allowed(command, webview.label(), &url))
}

// Wry 0.55 unwraps WKWebView.URL before a document is committed. Query the
// nullable native property instead, rejecting commands until it exists.
#[cfg(all(target_os = "macos", not(test)))]
fn current_url<R: tauri::Runtime>(webview: &tauri::Webview<R>) -> Option<tauri::Url> {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    webview
        .with_webview(move |native| {
            use objc2::{msg_send, rc::Retained, runtime::AnyObject};
            use objc2_foundation::NSString;
            let value = unsafe {
                let view = &*(native.inner() as *const AnyObject);
                let url: Option<Retained<AnyObject>> = msg_send![view, URL];
                url.and_then(|url| {
                    let text: Option<Retained<NSString>> = msg_send![&*url, absoluteString];
                    text.and_then(|text| tauri::Url::parse(&text.to_string()).ok())
                })
            };
            let _ = send.send(value);
        })
        .ok()?;
    receive.recv_timeout(Duration::from_secs(2)).ok().flatten()
}
#[cfg(any(not(target_os = "macos"), test))]
fn current_url<R: tauri::Runtime>(webview: &tauri::Webview<R>) -> Option<tauri::Url> {
    webview.url().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framework_rejects_remote_request_origin_even_when_window_is_main() {
        use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let called = reached.clone();
        let app = mock_builder()
            .invoke_handler(move |invoke| {
                called.fetch_add(1, Ordering::SeqCst);
                if allowed(invoke.message.command(), invoke.message.webview_ref()) {
                    invoke.resolver.resolve(true);
                } else {
                    invoke.resolver.reject("Untrusted document.");
                }
                true
            })
            .build(mock_context(noop_assets()))
            .unwrap();
        let view = WebviewWindowBuilder::new(&app, "main", WebviewUrl::App("index.html".into()))
            .build()
            .unwrap();
        let local = if cfg!(windows) {
            "http://tauri.localhost/index.html"
        } else {
            "tauri://localhost/index.html"
        };
        // MockContext has no bundled assets and defaults to a developer URL.
        // Give its synthetic WebView the packaged document this guard requires.
        view.navigate(local.parse().unwrap()).unwrap();
        let request = |command: &str, url: &str| tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: url.parse().unwrap(),
            body: Default::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        };
        for command in MAIN_COMMANDS
            .iter()
            .chain(TRAY_COMMANDS)
            .copied()
            .chain(["overlay_action"])
        {
            for origin in [
                "https://evil.invalid/",
                "http://127.0.0.1:8791/",
                "https://tauri.localhost.evil.invalid/",
                "data:text/html,hostile",
                "about:blank",
            ] {
                assert!(
                    get_ipc_response(&view, request(command, origin)).is_err(),
                    "{command} {origin}"
                );
            }
        }
        assert_eq!(
            reached.load(Ordering::SeqCst),
            0,
            "Remote requests must not reach custom dispatch"
        );
        let result = get_ipc_response(&view, request("data_location", local));
        assert!(
            result.is_ok(),
            "local request failed: {:?}; document: {:?}",
            result.err(),
            view.url()
        );
        assert_eq!(reached.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn authority_and_encoded_navigation_matrix() {
        assert!(document_url("main", &"tauri://localhost".parse().unwrap()));
        for root in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            for (label, path) in [
                ("main", "/index.html"),
                ("overlay", "/desktop-overlay.html"),
                ("tray-menu", "/desktop-tray.html"),
            ] {
                let good = tauri::Url::parse(&format!("{root}{path}")).unwrap();
                assert!(document_url(label, &good));
                for command in MAIN_COMMANDS {
                    assert_eq!(
                        command_allowed(command, label, &good),
                        label == "main",
                        "{command} {label}"
                    );
                }
                for path in [
                    "/api/v1/health",
                    "/native/audio/id",
                    "/%69ndex.html",
                    "/index.html%2f",
                    "/%2findex.html",
                    "/desktop-settings.html",
                ] {
                    assert!(!document_url(
                        label,
                        &tauri::Url::parse(&format!("{root}{path}")).unwrap()
                    ));
                }
            }
        }
        for url in [
            "https://evil.invalid/index.html",
            "https://tauri.localhost.evil.invalid/index.html",
            "https://tauri.localhost@evil.invalid/index.html",
            "http://user@tauri.localhost/index.html",
            "http://tauri.localhost:8791/index.html",
            "file:///index.html",
            "data:text/html,hello",
            "about:blank",
            "javascript:alert(1)",
        ] {
            let url = tauri::Url::parse(url).unwrap();
            assert!(!document_url("main", &url), "{url}");
        }
        let url = tauri::Url::parse("tauri://localhost/index.html").unwrap();
        for label in ["settings", "popup", "", "main-child"] {
            assert!(!command_allowed("data_location", label, &url));
        }
        assert!(!command_allowed("not_registered", "main", &url));
    }
}
