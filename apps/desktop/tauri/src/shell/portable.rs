//! Main-window boundary for portable backup and optional account sync
//! (MW-OUATH-6). Export/import works in every build and without an account.
//! Sync exists only in builds with sign-in, runs only for categories this
//! device explicitly joined after a preview, and never on its own because a
//! `signed_in` event arrived. Tokens never cross to the WebView; errors cross
//! as fixed kinds. Capture, transcription and delivery never wait on any of it.
use super::*;
use crate::account::{self, Service as Account};
use metrix_writer_api::account_sync::{
    self as portable_api,
    local::{self, Choice, Disposition, Mode, PreviewError, Receipt},
    sync::{AccountRef, Engine, Remote, RemoteError, SyncError},
    wire::Category,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The WebView event: `{ kind: "status" | "applied" }`. It carries no data;
/// the page asks for status or reloads its settings.
pub const EVENT: &str = "writer-sync";
#[cfg(feature = "google-sign-in")]
const FIRESTORE: &str = "https://firestore.googleapis.com/v1";

pub struct Portable {
    engine: tokio::sync::OnceCell<Arc<Engine>>,
    state_dir: std::path::PathBuf,
    pub(super) nudge: tokio::sync::Notify,
    pub(super) history: Arc<portable_api::history::History>,
}
impl Portable {
    async fn engine(&self) -> Result<Arc<Engine>> {
        self.engine
            .get_or_try_init(|| portable_api::open_desktop(self.state_dir.clone()))
            .await
            .cloned()
            .map_err(|_| "unavailable".into())
    }
}

/// Registers the service. With sign-in compiled in, a background task keeps
/// joined categories in sync; without it nothing reaches the network.
pub fn setup(app: &tauri::AppHandle, root: &std::path::Path) {
    let portable = Arc::new(Portable {
        engine: tokio::sync::OnceCell::new(),
        state_dir: root.join("State"),
        nudge: tokio::sync::Notify::new(),
        history: portable_api::history::History::open_with_accounts(
            root.join("State/account-history"),
            root.join("Accounts"),
        ),
    });
    app.state::<Account>()
        .register(Arc::new(super::history_sync::Participant(
            portable.history.clone(),
        )));
    app.manage(portable.clone());
    // Opening finishes or undoes an import a crash interrupted, before anything
    // else reads the stores' portable state.
    #[cfg(not(feature = "google-sign-in"))]
    tauri::async_runtime::spawn(async move {
        let _ = portable.engine().await;
    });
    #[cfg(feature = "google-sign-in")]
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { background(app, portable).await });
    }
}

pub(super) fn emit(app: &tauri::AppHandle, kind: &str) {
    let _ = app.emit_to("main", EVENT, json!({ "kind": kind }));
}

pub(super) fn remote_kind(error: RemoteError) -> String {
    serde_json::to_value(error)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unavailable".into())
}
fn sync_kind(error: SyncError) -> String {
    match error {
        SyncError::Remote(e) => remote_kind(e),
        SyncError::Api(e) => e.message,
        SyncError::ConsentChanged => "consent_changed".into(),
        SyncError::NotPreviewed => "not_previewed".into(),
    }
}

/// The signed-in account and its session generation, or `None` for a guest.
pub(super) fn signed_in(account: &Account) -> Option<(AccountRef, u64)> {
    let snapshot = account.snapshot();
    let key = snapshot
        .account_key
        .filter(|_| snapshot.state == account::State::SignedIn)?;
    Some((
        AccountRef {
            project_id: key.project_id,
            uid: key.uid,
        },
        snapshot.generation,
    ))
}
/// True while the same account and session generation are signed in.
pub(super) fn still(
    account: &Account,
    key: &AccountRef,
    generation: u64,
) -> impl Fn() -> bool + Send + Sync {
    let account = account.clone();
    let key = key.clone();
    move || signed_in(&account).is_some_and(|(current, g)| current == key && g == generation)
}

/// The signed-in account's cloud, bound to its identity and a fresh token.
pub(super) async fn remote(account: &Account) -> Result<(AccountRef, u64, Box<dyn Remote>)> {
    let (key, generation) = signed_in(account).ok_or("not_signed_in")?;
    #[cfg(feature = "google-sign-in")]
    {
        let wanted = account::AccountKey {
            project_id: key.project_id.clone(),
            uid: key.uid.clone(),
        };
        let token = account
            .id_token(&wanted, generation, false)
            .await
            .map_err(|kind| kind.as_str().to_owned())?;
        let firestore = portable_api::firestore::Firestore::new(
            FIRESTORE,
            &key.project_id,
            &key.uid,
            token,
            portable_api::firestore::client(),
        )
        .ok_or("unavailable")?
        .guarded(still(account, &key, generation));
        Ok((key, generation, Box::new(firestore)))
    }
    #[cfg(not(feature = "google-sign-in"))]
    {
        let _ = (key, generation);
        Err("unavailable".into())
    }
}

/// Runs a cycle whenever something may have changed: a local save, an import,
/// sign-in, an explicit "Sync now", or the slow poll for other devices' edits.
#[cfg(feature = "google-sign-in")]
async fn background(app: tauri::AppHandle, portable: Arc<Portable>) {
    use std::time::Duration;
    const POLL: Duration = Duration::from_secs(15 * 60);
    let account = app.state::<Account>().inner().clone();
    let Ok(engine) = portable.engine().await else {
        return;
    };
    let (_, mut events) = account.subscribe();
    let mut saved = portable_api::local_changes();
    let mut applied = engine.local().watch();
    let mut wait = Duration::from_secs(5);
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    if let account::EventKind::SessionInvalidated { account_key, .. } = &event.kind {
                        let key = AccountRef { project_id: account_key.project_id.clone(), uid: account_key.uid.clone() };
                        if matches!(event.kind, account::EventKind::SessionInvalidated { reason: account::Invalidation::SignedOut, .. }) {
                            let _ = engine.signed_out(&key).await;
                            let _ = portable.history.detach(&key);
                        }
                        emit(&app, "status");
                        continue;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
            _ = saved.changed() => tokio::time::sleep(Duration::from_millis(1500)).await,
            _ = applied.changed() => emit(&app, "applied"),
            _ = portable.nudge.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
        let before = *applied.borrow_and_update();
        let mut history_pending = false;
        let cycle = match remote(&account).await {
            Ok((key, generation, firestore)) => {
                let current = still(&account, &key, generation);
                let mut cycle = engine.run(&key, firestore.as_ref(), &current).await.ok();
                if portable.history.status(&key).is_ok_and(|s| s.joined) {
                    let observed = super::history_sync::observe(&portable.history)
                        .await
                        .is_ok();
                    if !observed
                        || portable
                            .history
                            .run(&key, firestore.as_ref(), &current)
                            .await
                            .is_err()
                    {
                        cycle = Some(portable_api::sync::Cycle::Deferred);
                    }
                    history_pending = portable
                        .history
                        .status(&key)
                        .is_ok_and(|s| s.joined && s.pending > 0);
                    emit(&app, "history");
                }
                cycle
            }
            Err(_) => None,
        };
        wait = match cycle {
            Some(portable_api::sync::Cycle::Deferred) => {
                (wait * 2).clamp(Duration::from_secs(30), POLL)
            }
            _ if history_pending => Duration::from_secs(5),
            _ => POLL,
        };
        if *applied.borrow_and_update() != before {
            emit(&app, "applied");
        }
        emit(&app, "status");
    }
}

#[tauri::command]
pub async fn portable_status(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<Portable>>,
    account: tauri::State<'_, Account>,
) -> Result<Value> {
    main_only(&window)?;
    let engine = state.engine().await?;
    let projection = engine.local().projection().await.map_err(|e| e.message)?;
    let last_import = engine.local().last_import().await.map_err(|e| e.message)?;
    let sync = match signed_in(&account) {
        Some((key, _)) => Some(engine.status(&key).await.map_err(|e| e.message)?),
        None => None,
    };
    Ok(json!({
        "sync_available": cfg!(feature = "google-sign-in"),
        "last_import": last_import,
        "kept": local::kept(&projection.shadow),
        "theme": engine.local().theme().await.map_err(|e| e.message)?,
        "sync": sync,
    }))
}

/// Saves a portable file where the user chooses. Returns `false` if cancelled.
#[tauri::command]
pub async fn portable_export(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
) -> Result<bool> {
    main_only(&window)?;
    let bytes = state
        .engine()
        .await?
        .local()
        .export()
        .await
        .map_err(|e| e.message)?;
    use tauri_plugin_dialog::DialogExt;
    let name = format!(
        "writer-settings-{}.json",
        chrono_date(metrix_writer_api::account_sync::local::now_ms())
    );
    let Some(target) = app
        .dialog()
        .file()
        .set_file_name(&name)
        .add_filter("Writer settings", &["json"])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = target.into_path().map_err(|_| "Choose a local file.")?;
    std::fs::write(&path, &bytes).map_err(|_| "The file could not be saved.")?;
    Ok(true)
}

/// Asks for a portable file and previews it. `null` if the user cancelled.
#[tauri::command]
pub async fn portable_import_preview(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
    categories: Vec<Category>,
    mode: Mode,
) -> Result<Option<local::Preview>> {
    main_only(&window)?;
    use tauri_plugin_dialog::DialogExt;
    let Some(selected) = app
        .dialog()
        .file()
        .add_filter("Writer settings", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|_| "Choose a local file.")?;
    let size = std::fs::metadata(&path).map_err(|_| "invalid_file")?.len();
    if size > portable_api::wire::MAX_BACKUP_BYTES as u64 {
        return Err("invalid_file".into());
    }
    let bytes = std::fs::read(&path).map_err(|_| "invalid_file")?;
    match state
        .engine()
        .await?
        .local()
        .preview_file(&bytes, categories, mode)
        .await
    {
        Ok(preview) => Ok(Some(preview)),
        Err(PreviewError::UnsupportedVersion) => Err("unsupported_version".into()),
        Err(PreviewError::Invalid(_)) => Err("invalid_file".into()),
        Err(PreviewError::Api(e)) => Err(e.message),
    }
}

#[tauri::command]
pub async fn portable_import_apply(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
    import_id: uuid::Uuid,
    choices: BTreeMap<uuid::Uuid, Choice>,
) -> Result<Receipt> {
    main_only(&window)?;
    let engine = state.engine().await?;
    if let Some(local::Source::Cloud { .. }) = engine
        .local()
        .pending_source(import_id)
        .await
        .map_err(|e| e.message)?
    {
        return Err("not_previewed".into());
    }
    let (receipt, _) = engine
        .local()
        .apply(import_id, &choices)
        .await
        .map_err(|e| e.message)?;
    emit(&app, "applied");
    state.nudge.notify_one();
    Ok(receipt)
}

#[tauri::command]
pub async fn portable_import_discard(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<Portable>>,
    import_id: uuid::Uuid,
) -> Result<()> {
    main_only(&window)?;
    state
        .engine()
        .await?
        .local()
        .discard_preview(import_id)
        .await
        .map_err(|e| e.message)
}

#[tauri::command]
pub async fn portable_rollback(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
    import_id: uuid::Uuid,
    rollback_snapshot_id: uuid::Uuid,
    expected_local_revision: u64,
) -> Result<Receipt> {
    main_only(&window)?;
    let receipt = state
        .engine()
        .await?
        .local()
        .rollback(import_id, rollback_snapshot_id, expected_local_revision)
        .await
        .map_err(|e| e.message)?;
    emit(&app, "applied");
    state.nudge.notify_one();
    Ok(receipt)
}

/// The WebView reports the theme it applied, so export and sync see it.
#[tauri::command]
pub async fn portable_theme(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<Portable>>,
    id: String,
    kind: String,
) -> Result<()> {
    main_only(&window)?;
    state
        .engine()
        .await?
        .local()
        .set_theme(local_theme(id, kind))
        .await
        .map_err(|e| e.message)
}
fn local_theme(id: String, kind: String) -> metrix_writer_api::account_sync::portable::Theme {
    metrix_writer_api::account_sync::portable::Theme { id, kind }
}

/// Compares this device with the account's cloud copy. Reads only.
#[tauri::command]
pub async fn sync_preview(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<Portable>>,
    account: tauri::State<'_, Account>,
    categories: Vec<Category>,
    disposition: Disposition,
) -> Result<portable_api::sync::SyncPreview> {
    main_only(&window)?;
    let engine = state.engine().await?;
    let (key, _, remote) = remote(&account).await?;
    engine
        .preview(&key, remote.as_ref(), categories, disposition)
        .await
        .map_err(sync_kind)
}

#[tauri::command]
pub async fn sync_confirm(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
    account: tauri::State<'_, Account>,
    import_id: uuid::Uuid,
    choices: BTreeMap<uuid::Uuid, Choice>,
) -> Result<Receipt> {
    main_only(&window)?;
    let engine = state.engine().await?;
    let (key, generation, remote) = remote(&account).await?;
    let current = still(&account, &key, generation);
    let receipt = engine
        .confirm(&key, remote.as_ref(), import_id, &choices, &current)
        .await
        .map_err(sync_kind)?;
    emit(&app, "applied");
    state.nudge.notify_one();
    Ok(receipt)
}

/// `now`, `pause`, `resume` or `dismiss` (clear notices) for this device.
#[tauri::command]
pub async fn sync_control(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
    account: tauri::State<'_, Account>,
    action: String,
) -> Result<()> {
    main_only(&window)?;
    let engine = state.engine().await?;
    let (key, _) = signed_in(&account).ok_or("not_signed_in")?;
    match action.as_str() {
        "now" => {}
        "pause" => engine.set_paused(&key, true).await.map_err(|e| e.message)?,
        "resume" => engine
            .set_paused(&key, false)
            .await
            .map_err(|e| e.message)?,
        "dismiss" => engine.dismiss_notices(&key).await.map_err(|e| e.message)?,
        _ => return Err("Unknown sync action.".into()),
    }
    state.nudge.notify_one();
    emit(&app, "status");
    Ok(())
}

/// Turns a category's sync off for the whole account.
#[tauri::command]
pub async fn sync_turn_off(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
    account: tauri::State<'_, Account>,
    category: Category,
) -> Result<()> {
    main_only(&window)?;
    let engine = state.engine().await?;
    let (key, _, remote) = remote(&account).await?;
    engine
        .turn_off(&key, remote.as_ref(), category)
        .await
        .map_err(sync_kind)?;
    emit(&app, "status");
    Ok(())
}

#[tauri::command]
pub async fn sync_resolve(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Portable>>,
    account: tauri::State<'_, Account>,
    id: uuid::Uuid,
    choice: Choice,
) -> Result<()> {
    main_only(&window)?;
    let engine = state.engine().await?;
    let (key, _) = signed_in(&account).ok_or("not_signed_in")?;
    engine
        .resolve(&key, id, choice)
        .await
        .map_err(|e| e.message)?;
    state.nudge.notify_one();
    emit(&app, "status");
    Ok(())
}

/// `YYYY-MM-DD` in UTC, for a default file name only.
fn chrono_date(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    // Civil-from-days (Howard Hinnant), valid for the Unix era.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_file_names_use_the_utc_date() {
        assert_eq!(super::chrono_date(0), "1970-01-01");
        assert_eq!(super::chrono_date(1_789_948_800_000), "2026-09-21");
    }
}
