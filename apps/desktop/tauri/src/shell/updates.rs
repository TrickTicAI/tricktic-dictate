use super::{main_only, system, Shell};
use crate::{storage::Result, update_security as security};
use metrix_writer_api::jobs::JobStore;
use serde::Serialize;
use std::sync::{atomic::Ordering, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Manager, WebviewWindow};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default, Serialize)]
pub struct View {
    pub installed: String,
    pub available: Option<String>,
    pub notes: Option<String>,
    pub stage: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub checked_at: Option<u64>,
    pub error: Option<String>,
}
#[derive(Default)]
struct Pending {
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
    release: Option<security::Release>,
}
#[derive(Default)]
pub struct Updates {
    pending: tokio::sync::Mutex<Pending>,
    view: Mutex<View>,
    cancel: Mutex<Option<CancellationToken>>,
}
fn platform() -> &'static str {
    if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-aarch64"
    } else {
        // Unsupported development architectures must never acquire another
        // architecture's otherwise valid signed update.
        "unsupported"
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn public_key(app: &tauri::AppHandle) -> Result<&str> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v["pubkey"].as_str())
        .ok_or_else(|| "Missing updater trust key.".into())
}
fn revalidate(app: &tauri::AppHandle, release: &security::Release) -> Result<()> {
    let key = public_key(app)?;
    let manifest = release.envelope.authenticate(key)?;
    let path = app.state::<Shell>().root.join("State/update-release.json");
    let previous = security::previous(&path, key)?;
    manifest.validate(
        &app.package_info().version.to_string(),
        platform(),
        now(),
        previous.as_ref(),
    )
}
fn view(app: &tauri::AppHandle) -> View {
    let mut v = app.state::<Updates>().view.lock().unwrap().clone();
    v.installed = app.package_info().version.to_string();
    if v.stage.is_empty() {
        v.stage = "unchecked".into();
    }
    v
}
fn change(app: &tauri::AppHandle, f: impl FnOnce(&mut View)) {
    f(&mut app.state::<Updates>().view.lock().unwrap());
}
#[tauri::command]
pub fn updates_status(window: WebviewWindow, app: tauri::AppHandle) -> Result<View> {
    main_only(&window)?;
    Ok(view(&app))
}
#[tauri::command]
pub async fn updates_check(window: WebviewWindow, app: tauri::AppHandle) -> Result<View> {
    main_only(&window)?;
    if app.state::<Shell>().probe {
        return Err("Update checks are disabled during probes.".into());
    }
    let state = app.state::<Updates>();
    let mut pending = state
        .pending
        .try_lock()
        .map_err(|_| "An update operation is already running.")?;
    if pending.bytes.is_some() {
        return Ok(view(&app));
    }
    change(&app, |v| {
        v.stage = "checking".into();
        v.error = None;
    });
    pending.update = None;
    pending.release = None;
    let result: Result<_> = async {
        let endpoint = app
            .config()
            .plugins
            .0
            .get("updater")
            .and_then(|v| v["endpoints"][0].as_str())
            .ok_or("Missing update endpoint.")?;
        let bytes = tokio::time::timeout(
            Duration::from_secs(30),
            security::fetch(
                &security::client()?,
                security::channel_url(endpoint, platform())?,
                security::MAX_FEED,
                &CancellationToken::new(),
                |_| {},
            ),
        )
        .await
        .map_err(|_| "Update check timed out.")??;
        let feed: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| "The update server returned invalid release data. Try again later.")?;
        let release = security::release_from_feed(&feed, platform(), public_key(&app)?)?;
        revalidate(&app, &release)?;
        let updater = app
            .updater_builder()
            .target(platform())
            // Keep the running shell intact if installer launch fails.
            .on_before_exit(|| {})
            .build()
            .map_err(|e| e.to_string())?;
        // Construct only from the bounded, authenticated response. Never ask the
        // upstream plugin to fetch unsigned metadata or buffer an unbounded body.
        let update = updater
            .update_from_response(feed)
            .map_err(|e| e.to_string())?;
        security::remember(
            &app.state::<Shell>().root.join("State/update-release.json"),
            &release.envelope,
        )?;
        Ok((update, release))
    }
    .await;
    match result {
        Ok((update, release)) => {
            change(&app, |v| {
                v.available = update.as_ref().map(|u| u.version.clone());
                v.notes = update.as_ref().and_then(|u| u.body.clone());
                v.stage = if update.is_some() {
                    "available"
                } else {
                    "current"
                }
                .into();
                v.checked_at = Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );
            });
            pending.update = update;
            pending.release = Some(release);
        }
        Err(error) => change(&app, |v| {
            v.stage = "error".into();
            v.available = None;
            v.notes = None;
            v.error = Some(error);
        }),
    }
    Ok(view(&app))
}
#[tauri::command]
pub async fn updates_download(window: WebviewWindow, app: tauri::AppHandle) -> Result<View> {
    main_only(&window)?;
    if app.state::<Shell>().probe {
        return Err("Updates are disabled during probes.".into());
    }
    let state = app.state::<Updates>();
    let mut pending = state
        .pending
        .try_lock()
        .map_err(|_| "An update operation is already running.")?;
    let update = pending
        .update
        .as_ref()
        .ok_or("Check for an available update first.")?
        .clone();
    let release = pending
        .release
        .as_ref()
        .ok_or("Check for an available update first.")?
        .clone();
    revalidate(&app, &release)?;
    // Nothing is written to an installer path until verification succeeds.
    pending.bytes = None;
    let cancel = CancellationToken::new();
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    change(&app, |v| {
        v.stage = "downloading".into();
        v.error = None;
        v.downloaded = 0;
        v.total = Some(release.manifest.size);
    });
    let result = async {
        let bytes = security::fetch(
            &security::client()?,
            release.url.clone(),
            release.manifest.size as usize,
            &cancel,
            |count| change(&app, |v| v.downloaded = count as u64),
        )
        .await?;
        revalidate(&app, &release)?;
        release.manifest.verify_bytes(&bytes)?;
        update
            .verify(&bytes)
            .map_err(|_| "The publisher signature could not be verified.")?;
        if cancel.is_cancelled() {
            return Err("Update download canceled.".into());
        }
        Ok::<_, String>(bytes)
    }
    .await;
    *state.cancel.lock().unwrap() = None;
    match result {
        Ok(bytes) => {
            // The plugin verifies the embedded publisher signature before returning.
            pending.bytes = Some(bytes);
            change(&app, |v| v.stage = "ready".into());
        }
        Err(error) => {
            pending.bytes = None;
            change(&app, |v| {
                v.stage = "available".into();
                v.error = Some(error);
            });
        }
    }
    Ok(view(&app))
}
#[tauri::command]
pub fn updates_cancel(window: WebviewWindow, app: tauri::AppHandle) -> Result<()> {
    main_only(&window)?;
    let state = app.state::<Updates>();
    if let Some(cancel) = state.cancel.lock().unwrap().as_ref() {
        cancel.cancel();
        return Ok(());
    }
    let mut pending = state
        .pending
        .try_lock()
        .map_err(|_| "An update operation is already running.")?;
    *pending = Pending::default();
    change(&app, |v| {
        v.stage = "unchecked".into();
        v.available = None;
        v.notes = None;
        v.downloaded = 0;
        v.total = None;
        v.error = None;
    });
    Ok(())
}
#[tauri::command]
pub async fn updates_install(window: WebviewWindow, app: tauri::AppHandle) -> Result<View> {
    main_only(&window)?;
    let shell = app.state::<Shell>();
    if shell.probe || !cfg!(any(windows, target_os = "macos")) {
        return Err("In-app installation is unavailable in this build or during probes.".into());
    }
    let _maintenance = shell
        .maintenance
        .try_lock()
        .map_err(|_| "Wait for recording or history maintenance to finish.")?;
    let state = app.state::<Updates>();
    let pending = state
        .pending
        .try_lock()
        .map_err(|_| "An update operation is already running.")?;
    let update = pending
        .update
        .as_ref()
        .ok_or("Download the update first.")?;
    let bytes = pending.bytes.as_ref().ok_or("Download the update first.")?;
    let release = pending
        .release
        .as_ref()
        .ok_or("Check for an available update first.")?;
    revalidate(&app, release)?;
    release.manifest.verify_bytes(bytes)?;
    update
        .verify(bytes)
        .map_err(|_| "The publisher signature could not be verified.")?;
    if update.version != release.manifest.version
        || update.current_version != app.package_info().version.to_string()
        || semver::Version::parse(&update.version).map_err(|_| "Invalid update version.")?
            <= app.package_info().version
    {
        return Err("The staged update version does not match this installation.".into());
    }
    if shell.capture.status()?.active
        || shell.pending.lock().unwrap().is_some()
        || shell.snapshot.lock().unwrap().as_ref().is_some_and(|s| {
            matches!(
                s["state"].as_str(),
                Some("requesting" | "capturing" | "saving" | "queued" | "transcribing")
            )
        })
    {
        return Err("Finish recording and transcription before updating.".into());
    }
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Cannot verify that saved jobs are idle.")?;
    if !pipeline
        .jobs
        .recoverable()
        .await
        .map_err(|_| "Cannot verify that saved jobs are idle.")?
        .is_empty()
    {
        return Err("Wait for saved transcription jobs to finish before updating.".into());
    }
    if shell.quitting.swap(true, Ordering::AcqRel) {
        return Err(concat!(product_name!(), " is already shutting down.").into());
    }
    change(&app, |v| {
        v.stage = "installing".into();
        v.error = None;
    });
    let result = async {
        // The maintenance lock and quitting flag exclude new capture. Stop drains
        // safely without permanently closing the recorder if installer launch fails.
        let capture = shell.capture.clone();
        let saved = tauri::async_runtime::spawn_blocking(move || capture.stop())
            .await
            .map_err(|_| "Could not flush saved audio.")??;
        if saved.error.is_some() && !saved.drain_confirmed {
            return Err("Saved audio could not be flushed. Update postponed.".into());
        }
        system::recording(&app, false, false);
        if let Ok(setup) = metrix_writer_api::desktop_settings::global() {
            setup
                .runtime
                .select(None)
                .map_err(|_| "Could not stop the model runtime.")?;
            let mut stopped = false;
            for _ in 0..100 {
                if matches!(
                    setup.runtime.worker(),
                    metrix_writer_api::local_stt::managed::WorkerState::Idle
                ) {
                    stopped = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if !stopped {
                return Err(
                    "The model runtime is still stopping. Try updating again shortly.".into(),
                );
            }
        }
        // Recheck immediately at handoff, including expiry during shutdown waits.
        revalidate(&app, release)?;
        release.manifest.verify_bytes(bytes)?;
        update
            .verify(bytes)
            .map_err(|_| "The publisher signature could not be verified.")?;
        // Windows launches NSIS and exits; macOS replaces the bundle and returns.
        update.install(bytes).map_err(|_| {
            "The installer could not start. Try again, or use the downloaded installer.".to_owned()
        })
    }
    .await;
    #[cfg(target_os = "macos")]
    if result.is_ok() {
        // macOS installation does not relaunch. Saved work and the owned model
        // runtime have drained before this point; restart only after success.
        app.restart();
    }
    if let Err(error) = result {
        shell.quitting.store(false, Ordering::Release);
        change(&app, |v| {
            v.stage = "ready".into();
            v.error = Some(error);
        });
    }
    Ok(view(&app))
}
