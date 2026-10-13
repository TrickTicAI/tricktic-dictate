//! Main-window boundary for optional Google sign-in (MW-OUATH-3).
//!
//! Only fixed states, fixed error kinds, the account key and display metadata
//! cross to the WebView. Tokens, codes and the PKCE verifier stay inside
//! `crate::account`. Every command is main-window-only, like the credential
//! commands, and none of them runs during capture or delivery.
use super::*;
use crate::account::lifecycle::{LocalCopy, LocalCopyChoice, Workspace};
use crate::account::{self, ErrorKind, EventKind, Service};
use std::sync::Arc;

/// The WebView event carrying every ordered account lifecycle event.
pub const EVENT: &str = "writer-account";
/// The WebView event carrying sensitive operation progress (MW-OUATH-8).
#[cfg(feature = "google-sign-in")]
pub const TASK_EVENT: &str = "writer-account-task";

/// Registers the service and restores any protected session in the background.
/// A guest makes no network request; a failure here never delays capture.
pub fn setup(app: &tauri::AppHandle, root: &std::path::Path) {
    let handle = app.clone();
    let emit: account::Emit = Arc::new(move |event: &account::Event| {
        let _ = handle.emit_to("main", EVENT, event);
        // The browser leaves Writer behind; bring it back when the attempt ends.
        if matches!(event.kind, EventKind::SignInFinished { .. }) {
            if let Some(main) = handle.get_webview_window("main") {
                let _ = main.unminimize();
                let _ = main.show();
                let _ = main.set_focus();
            }
        }
    });
    let service = Service::new(account::vault::platform(root), emit, Arc::new(open_browser));
    service.set_workspace(Workspace::new(root));
    app.manage(service.clone());
    #[cfg(feature = "google-sign-in")]
    {
        let mut tasks = service.tasks();
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match tasks.recv().await {
                    Ok(task) => {
                        let _ = handle.emit_to("main", TASK_EVENT, &task);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
    tauri::async_runtime::spawn(async move { service.start().await });
}

/// Opens Google's page in the default browser, never in a WebView. The helper
/// processes are the ones the MW-OUATH-2 proof qualified on each platform.
/// About's legal links use it too.
pub(super) fn open_browser(url: &str) -> bool {
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    } else {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(url);
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn refuse(kind: ErrorKind) -> String {
    kind.as_str().into()
}

#[tauri::command]
pub async fn account_snapshot(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
) -> Result<account::Snapshot> {
    main_only(&window)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn account_sign_in(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
    attempt_id: uuid::Uuid,
) -> Result<()> {
    main_only(&window)?;
    state.sign_in(attempt_id, false).map_err(refuse)
}

/// Signs the same account in again after Firebase refused its credential.
#[tauri::command]
pub async fn account_reauthenticate(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
    attempt_id: uuid::Uuid,
) -> Result<()> {
    main_only(&window)?;
    state.sign_in(attempt_id, true).map_err(refuse)
}

#[tauri::command]
pub async fn account_cancel_sign_in(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
    attempt_id: uuid::Uuid,
) -> Result<()> {
    main_only(&window)?;
    state.cancel(attempt_id).map_err(refuse)
}

/// Signs out. `localCopy` decides what happens to this account's downloaded
/// copies; recordings and the guest data are never touched.
#[tauri::command]
pub async fn account_sign_out(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
    local_copy: Option<LocalCopyChoice>,
) -> Result<()> {
    main_only(&window)?;
    state
        .sign_out(local_copy.unwrap_or_default())
        .await
        .map_err(refuse)
}

/// Signs the current account out and starts a sign-in for another one.
#[tauri::command]
pub async fn account_switch(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
    attempt_id: uuid::Uuid,
    local_copy: Option<LocalCopyChoice>,
) -> Result<()> {
    main_only(&window)?;
    state
        .switch_account(attempt_id, local_copy.unwrap_or_default())
        .await
        .map_err(refuse)
}

/// How much this account keeps on this computer, so the page knows whether
/// to ask about local copies.
#[tauri::command]
pub async fn account_local_copy(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
) -> Result<LocalCopy> {
    main_only(&window)?;
    state.local_copy().map_err(refuse)
}

/// Whether this build can revoke sessions or delete cloud data, and any
/// cleanup of the current account that has not finished.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tasks {
    configured: bool,
    #[cfg(feature = "google-sign-in")]
    unfinished: Vec<account::cleanup::Task>,
    #[cfg(not(feature = "google-sign-in"))]
    unfinished: Vec<()>,
}

#[tauri::command]
pub async fn account_tasks(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
) -> Result<Tasks> {
    main_only(&window)?;
    #[cfg(feature = "google-sign-in")]
    return Ok(Tasks {
        configured: state.cleanup_configured(),
        unfinished: state.unfinished(),
    });
    #[cfg(not(feature = "google-sign-in"))]
    {
        let _ = state;
        Ok(Tasks {
            configured: false,
            unfinished: Vec::new(),
        })
    }
}

/// Starts a sensitive operation: revoke every session, delete cloud data,
/// delete the Writer account, or retry a stored cleanup. Reauthenticates the
/// same Google account first when its last sign-in is not recent. Returns the
/// operation ID; progress arrives as `writer-account-task`.
#[tauri::command]
pub async fn account_sensitive(
    window: WebviewWindow,
    state: tauri::State<'_, Service>,
    attempt_id: uuid::Uuid,
    action: serde_json::Value,
    local_copy: Option<LocalCopyChoice>,
) -> Result<uuid::Uuid> {
    main_only(&window)?;
    #[cfg(feature = "google-sign-in")]
    {
        let action = serde_json::from_value(action).map_err(|_| refuse(ErrorKind::Unavailable))?;
        state
            .sensitive(attempt_id, action, local_copy.unwrap_or_default())
            .map_err(refuse)
    }
    #[cfg(not(feature = "google-sign-in"))]
    {
        let _ = (state, attempt_id, action, local_copy);
        Err(refuse(ErrorKind::Unavailable))
    }
}
