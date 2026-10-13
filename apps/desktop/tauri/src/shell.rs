use crate::{
    capture::Capture,
    identity,
    storage::{self, Result, Store},
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, Response},
};
use metrix_writer_api::jobs::JobStore;
use serde_json::{json, Value};
use std::{
    borrow::Cow,
    fs::{File, OpenOptions},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tower::ServiceExt;
#[cfg(windows)]
mod clipboard_preservation;
#[cfg(windows)]
mod clipboard_rendering;
mod context;
mod credentials;
mod delivery;
#[cfg(windows)]
mod ducking;
mod history_sync;
mod hotkey_state;
mod input_guard;
#[cfg(all(target_os = "macos", feature = "latency-probe"))]
mod macos_acceptance;
#[cfg(target_os = "macos")]
mod macos_clipboard;
#[cfg(target_os = "macos")]
mod macos_delivery;
#[cfg(target_os = "macos")]
mod macos_hook;
#[cfg(target_os = "macos")]
mod macos_keys;
mod mode_shortcuts;
#[cfg(windows)]
mod modifier_hook;
mod onboarding;
#[cfg(windows)]
mod panel_presentation;
mod panel_recovery;
mod portable;
mod preferences;
mod privacy;
mod resources;
mod security;
#[cfg(feature = "security-probe")]
mod security_probe;
mod sign_in;
mod startup_log;
mod storage_controls;
mod system;
mod transcription;
mod tray;
mod updates;
#[cfg(windows)]
mod win_input;
#[cfg(windows)]
mod win_target;
mod window_place;
#[cfg(windows)]
mod windows_delivery;

struct Shell {
    capture: Capture,
    context: std::sync::Arc<context::ContextReader>,
    mode_shortcuts: Mutex<mode_shortcuts::Bindings>,
    capture_mode: Mutex<Option<(String, String)>>,
    replay_probe: bool,
    preview_sessions: Mutex<std::collections::HashSet<uuid::Uuid>>,
    /// Recognition options of the capture being previewed, so Stop can start the
    /// final pass before the WebView imports the audio and persists the job.
    final_options: Mutex<
        std::collections::HashMap<
            uuid::Uuid,
            metrix_writer_api::transcription::TranscriptionOptions,
        >,
    >,
    stopped_at: Mutex<StopTimes>,
    maintenance: tokio::sync::Mutex<()>,
    history_reload: AtomicBool,
    storage_preview: Mutex<Option<storage_controls::Preview>>,
    delivery: Mutex<delivery::Delivery>,
    shortcut_capture: AtomicBool,
    held_session: Mutex<Option<(String, String)>>,
    system: Mutex<system::Settings>,
    #[cfg(windows)]
    ducking: ducking::Ducking,
    snapshot: Mutex<Option<Value>>,
    pending: Mutex<Option<String>>,
    dismissed: Mutex<Option<Value>>,
    quitting: AtomicBool,
    preferences: Mutex<preferences::Preferences>,
    theme: Mutex<Option<preferences::OverlayTheme>>,
    panel_health: Mutex<crate::panel_health::Health>,
    /// Identifies the latest panel notice, so an expiring one never hides a newer.
    notices: AtomicU64,
    /// Until when a timed notice keeps the panel on screen.
    notice_until: Mutex<Option<std::time::Instant>>,
    /// Earlier dictations whose text is still to come while a newer one owns
    /// the panel; replayed to a panel that reloads.
    waiting: AtomicU64,
    pressed: AtomicBool,
    /// When the recording shortcut was last pressed, so the dictation it starts
    /// can time its stages from the key rather than from the microphone.
    pressed_at: Mutex<Option<std::time::Instant>>,
    root: PathBuf,
    probe: bool,
    _lease: File,
}
/// When each recent dictation's Stop was accepted, for its delivery receipt.
/// Several can be in flight: the next dictation may stop before the previous
/// one's text has been inserted.
#[derive(Default)]
struct StopTimes(std::collections::VecDeque<(uuid::Uuid, std::time::Instant)>);
impl StopTimes {
    /// Keeps the first Stop of each dictation; a repeated one changes nothing.
    fn record(&mut self, id: uuid::Uuid, at: std::time::Instant) {
        if self.0.iter().any(|(known, _)| *known == id) {
            return;
        }
        self.0.push_back((id, at));
        while self.0.len() > 8 {
            self.0.pop_front();
        }
    }
    #[cfg(any(windows, target_os = "macos"))]
    fn get(&self, id: uuid::Uuid) -> Option<std::time::Instant> {
        self.0
            .iter()
            .find(|(known, _)| *known == id)
            .map(|(_, at)| *at)
    }
}
fn main_only(window: &WebviewWindow) -> Result<()> {
    security::window_only(window, "main")
}
#[tauri::command]
async fn capture_start(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    id: uuid::Uuid,
    language: String,
) -> Result<crate::capture::Status> {
    main_only(&window)?;
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "History maintenance is in progress. Try recording again shortly.")?;
    if state.history_reload.load(Ordering::Acquire) {
        return Err(
            "History cleanup is finishing. Try recording again after the app reloads.".into(),
        );
    }
    if (state.probe && !state.replay_probe) || state.quitting.load(Ordering::Acquire) {
        return Err("Capture is disabled during a probe or shutdown.".into());
    }
    let modes = metrix_writer_api::cleanup::modes::service()
        .await
        .map_err(|e| e.message)?;
    let settings = modes.settings().await.map_err(|e| e.message)?;
    let override_mode = state
        .capture_mode
        .lock()
        .unwrap()
        .take()
        .filter(|(recording, _)| recording == &id.to_string())
        .map(|(_, mode)| mode);
    let profile = settings
        .profiles
        .into_iter()
        .find(|p| p.id == override_mode.as_deref().unwrap_or(&settings.active))
        .ok_or("Default mode unavailable.")?;
    metrix_writer_api::latency::mark(id, "capture_requested");
    let reader = state.context.clone();
    let target = state
        .delivery
        .lock()
        .unwrap()
        .target(&id.to_string())
        .map(|t| (id.to_string(), t.window, t.process));
    if state.replay_probe {
        if let Ok(bytes) = std::fs::read(state.root.join("fixture-target.json")) {
            if let Ok(target) = serde_json::from_slice::<(isize, u32)>(&bytes) {
                state.delivery.lock().unwrap().start(
                    &id.to_string(),
                    Some(delivery::Target {
                        window: target.0,
                        process: target.1,
                        #[cfg(windows)]
                        created: win_target::created(target.1).unwrap_or_default(),
                    }),
                );
            }
        }
    }
    let needs_context = profile.kind != metrix_writer_api::cleanup::modes::ModeKind::Regular
        && (profile.context.application
            || profile.context.selected_text
            || profile.context.clipboard
            || profile.kind == metrix_writer_api::cleanup::modes::ModeKind::Command);
    // Durably freeze the profile first. Context is optional input to processing,
    // never a prerequisite for reporting Listening or saving microphone audio.
    if needs_context {
        modes
            .begin_context_capture(id, profile.clone())
            .await
            .map_err(|e| e.message)?;
        let service = modes.clone();
        tauri::async_runtime::spawn(async move {
            let context_reader = reader.clone();
            let captured = tauri::async_runtime::spawn_blocking(move || {
                context_reader.capture(id.to_string(), profile, target)
            })
            .await
            .ok()
            .and_then(std::result::Result::ok);
            let captured_ok = captured.is_some();
            let accepted = service.complete_context_capture(id, captured).await;
            if !captured_ok || !matches!(accepted, Ok(true)) {
                // Forget is session-specific; a late read cannot clear a newer
                // Command's retained selection. Never block capture/delivery.
                reader.forget_later(id.to_string());
            }
        });
    } else {
        modes
            .bind_recording(id, profile, Default::default())
            .await
            .map_err(|e| e.message)?;
    }
    let capture = state.capture.clone();
    let recording = tauri::async_runtime::spawn_blocking(move || capture.start(id, language)).await;
    let result = recording.unwrap_or_else(|_| Err("Recorder unavailable.".to_owned()));
    if result.is_err() {
        // Serialized with the background completion: it cannot recreate this
        // binding after discard. A context failure never deletes saved audio.
        let _ = modes.discard_capture(id).await;
        state.context.forget_later(id.to_string());
    }
    result
}
/// Use the built-in microphone, rather than a Bluetooth headset's, while
/// System default is selected. macOS only.
#[tauri::command]
async fn microphone_prefer_built_in(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    enabled: bool,
) -> Result<crate::microphones::MicrophoneList> {
    main_only(&window)?;
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || capture.prefer_built_in(enabled))
        .await
        .map_err(|_| "Microphone settings unavailable.")?
}
#[tauri::command]
async fn list_microphones(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
) -> Result<crate::microphones::MicrophoneList> {
    main_only(&window)?;
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || capture.microphones())
        .await
        .map_err(|_| "Microphone list unavailable.")?
}
#[tauri::command]
async fn select_microphone(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    device_id: Option<String>,
) -> Result<crate::microphones::MicrophoneList> {
    main_only(&window)?;
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || capture.select_microphone(device_id))
        .await
        .map_err(|_| "Microphone selection unavailable.")?
}
#[tauri::command]
async fn history_delete(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    portable: tauri::State<'_, std::sync::Arc<portable::Portable>>,
) -> Result<Value> {
    main_only(&window)?;
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "Recording or history maintenance is in progress. Try again shortly.")?;
    if state.capture.status()?.active
        || state.pending.lock().unwrap().is_some()
        || state.snapshot.lock().unwrap().as_ref().is_some_and(|s| {
            matches!(
                s["state"].as_str(),
                Some("requesting" | "capturing" | "saving" | "queued" | "transcribing")
            )
        })
    {
        return Err(
            "Stop recording and wait for transcription to finish before deleting history.".into(),
        );
    }
    let capture = state.capture.clone();
    let native = tauri::async_runtime::spawn_blocking(move || capture.list())
        .await
        .map_err(|_| "Saved recordings unavailable.")??;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Saved history unavailable.")?;
    let server_ids: std::collections::HashSet<_> = pipeline
        .recordings
        .list()
        .await
        .map_err(|_| "Saved recordings unavailable.")?
        .into_iter()
        .map(|r| r.metadata.id)
        .collect();
    // Pipeline serializes this deletion with job submission and refuses active
    // jobs independently of any potentially stale document snapshot.
    let (mut recordings, jobs) = pipeline
        .delete_history()
        .await
        .map_err(|error| error.message)?;
    let capture = state.capture.clone();
    recordings += tauri::async_runtime::spawn_blocking(move || -> Result<usize> {
        let mut unique = 0;
        for recording in native {
            capture.delete(recording.id).map_err(|_| "Some history was deleted, but local audio could not be removed. Retry deletion.".to_owned())?;
            unique += usize::from(!server_ids.contains(&recording.id));
        }
        Ok(unique)
    }).await.map_err(|_| "Some history was deleted, but local audio cleanup failed. Retry deletion.")??;
    portable
        .history
        .clear_local_history()
        .await
        .map_err(|_| "Local transcript copies could not be cleared. Retry history deletion.")?;
    state.context.forget(None)?;
    *state.delivery.lock().unwrap() = Default::default();
    *state.held_session.lock().unwrap() = None;
    // Keep new capture disabled while the document clears its own attempt
    // cache. A successful reload releases this gate only after old state clears.
    state.history_reload.store(true, Ordering::Release);
    Ok(json!({"deleted_recordings":recordings,"deleted_jobs":jobs}))
}
#[tauri::command]
fn history_resume(window: WebviewWindow, state: tauri::State<'_, Shell>) -> Result<()> {
    main_only(&window)?;
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "History deletion is still running.")?;
    state.history_reload.store(false, Ordering::Release);
    Ok(())
}
/// `transcribe` is false when the WebView already knows this dictation is
/// canceled or discarded; older callers omit it and get the final pass ahead.
#[tauri::command]
async fn capture_stop(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    transcribe: Option<bool>,
) -> Result<crate::capture::Status> {
    main_only(&window)?;
    let stop_received = std::time::Instant::now();
    let capture = state.capture.clone();
    if let Some(id) = capture.status()?.id {
        if !transcribe.unwrap_or(true) {
            if let Ok(modes) = metrix_writer_api::cleanup::modes::service().await {
                modes.discard_draft(id).await;
            }
        }
        metrix_writer_api::latency::mark(id, "stop_accepted");
        state.stopped_at.lock().unwrap().record(id, stop_received);
        // Where the text goes by default: the window in front as it stops. The
        // replay probe's fixture field is its target whatever is in front.
        if !state.replay_probe {
            delivery::capture_stop_target(&state, &id.to_string());
        }
    }
    let status = tauri::async_runtime::spawn_blocking(move || {
        let result = capture.stop();
        if let Ok(status) = &result {
            if let Some(id) = status.id {
                metrix_writer_api::latency::mark(id, "audio_durable");
            }
        }
        result
    })
    .await
    .map_err(|_| "Recorder unavailable.".to_owned())??;
    if let Some(id) = status.id {
        if let Ok(modes) = metrix_writer_api::cleanup::modes::service().await {
            if transcribe.unwrap_or(true) {
                modes.release_draft(id).await;
            }
        }
        if !transcribe.unwrap_or(true) {
            // Quit also drains without transcription and keeps retry audio.
            // Only explicit discard (capture_delete) removes its mode snapshot.
            if let Ok(modes) = metrix_writer_api::cleanup::modes::service().await {
                modes.discard_draft(id).await;
            }
            if let Ok(modes) = metrix_writer_api::cleanup::modes::service().await {
                let _ = modes.complete_context_capture(id, None).await;
            }
            state.context.forget_later(id.to_string());
        }
        let options = state.final_options.lock().unwrap().remove(&id);
        if let Some(options) = options.filter(|_| transcribe.unwrap_or(true)) {
            transcription::final_ahead(state.capture.clone(), id, options);
        }
    }
    Ok(status)
}
#[tauri::command]
async fn capture_status(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
) -> Result<crate::capture::Status> {
    main_only(&window)?;
    state.capture.status()
}
fn local_metadata(
    metadata: &metrix_writer_api::transcription::RecordingMetadata,
) -> Result<storage::Recording> {
    let mut value = serde_json::to_value(metadata).map_err(|_| "Invalid retained metadata.")?;
    value["chunk_count"] = json!(1);
    serde_json::from_value(value).map_err(|_| "Invalid retained metadata.".into())
}
#[tauri::command]
async fn capture_get(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    id: uuid::Uuid,
) -> Result<Option<storage::Recording>> {
    main_only(&window)?;
    if let Some(m) = state.capture.get(id)? {
        return Ok(Some(m));
    }
    use metrix_writer_api::recordings::RecordingStore;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Saved audio unavailable.")?;
    match pipeline.recordings.get(id).await {
        Ok(r) => Ok(Some(local_metadata(&r.metadata)?)),
        Err(_) => Ok(None),
    }
}
#[tauri::command]
async fn capture_list(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
) -> Result<Vec<storage::Recording>> {
    main_only(&window)?;
    let capture = state.capture.clone();
    let mut recordings = tauri::async_runtime::spawn_blocking(move || capture.list())
        .await
        .map_err(|_| "Saved recordings unavailable.")??;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Saved audio unavailable.")?;
    for r in pipeline
        .recordings
        .list()
        .await
        .map_err(|_| "Could not restore saved audio.")?
    {
        if !recordings.iter().any(|m| m.id == r.metadata.id) {
            recordings.push(local_metadata(&r.metadata)?);
        }
    }
    recordings.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
    Ok(recordings)
}
#[tauri::command]
async fn capture_delete(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    id: uuid::Uuid,
) -> Result<()> {
    main_only(&window)?;
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "Recording or history maintenance is in progress. Try again shortly.")?;
    if state.capture.status()?.active && state.capture.status()?.id == Some(id) {
        return Err("Stop recording before deleting audio.".into());
    }
    if let Ok(modes) = metrix_writer_api::cleanup::modes::service().await {
        modes.discard_draft(id).await;
    }
    use metrix_writer_api::recordings::RecordingStore;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Saved audio unavailable.")?;
    if pipeline.recordings.get(id).await.is_ok() {
        pipeline
            .jobs
            .delete_recording(id)
            .await
            .map_err(|_| "Could not delete audio; wait for active jobs to finish.")?;
    }
    if state.capture.get(id)?.is_some() {
        state.capture.delete(id)?;
    }
    let jobs: Vec<_> = pipeline
        .jobs
        .all()
        .await
        .map_err(|e| e.message)?
        .into_iter()
        .filter(|job| job.request.recording_id == id)
        .map(|job| job.id)
        .collect();
    let canceled = state.delivery.lock().unwrap().is_canceled(&id.to_string());
    if canceled || jobs.is_empty() {
        pipeline
            .mode_service()
            .await
            .map_err(|e| e.message)?
            .discard_recording_context(id, &jobs)
            .await
            .map_err(|e| e.message)?;
        state.context.forget(Some(id.to_string()))?;
    }
    Ok(())
}
#[tauri::command]
async fn saved_attempts(window: WebviewWindow) -> Result<Vec<Value>> {
    main_only(&window)?;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Saved attempts unavailable.")?;
    let mut grouped =
        std::collections::BTreeMap::<uuid::Uuid, Vec<metrix_writer_api::transcription::Job>>::new();
    for job in pipeline
        .jobs
        .all()
        .await
        .map_err(|_| "Could not restore saved attempts.")?
    {
        grouped
            .entry(job.request.recording_id)
            .or_default()
            .push(job);
    }
    Ok(grouped
        .into_iter()
        .filter_map(|(id, mut jobs)| {
            jobs.pop()
                .map(|job| json!({"id":id,"request":job.request,"job":job,"history":jobs}))
        })
        .collect())
}
#[tauri::command]
fn session_event(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Shell>,
    message: Value,
) -> Result<()> {
    main_only(&window)?;
    if message.to_string().len() > 4096
        || message["version"] != 1
        || message["epoch"]
            .as_str()
            .and_then(|s| s.parse::<uuid::Uuid>().ok())
            .is_none()
    {
        return Err("Invalid session event.".into());
    }
    if message["kind"] == "snapshot" {
        let mut current = state.snapshot.lock().unwrap();
        if current.as_ref().is_some_and(|s| {
            s["epoch"] == message["epoch"] && s["revision"].as_u64() >= message["revision"].as_u64()
        }) {
            return Ok(());
        }
        if current.as_ref().is_none_or(|old| {
            old["session_id"] != message["session_id"]
                || old["epoch"] != message["epoch"]
                || (old["state"] != message["state"] && message["state"] == "requesting")
        }) {
            *state.dismissed.lock().unwrap() = None;
        }
        let was_capturing = current.as_ref().is_some_and(|s| s["state"] == "capturing");
        let entered = current.as_ref().is_none_or(|s| {
            s["state"] != message["state"] || s["session_id"] != message["session_id"]
        });
        let is_capturing = message["state"] == "capturing";
        if was_capturing != is_capturing {
            system::recording(&app, is_capturing, true);
        }
        *current = Some(message.clone());
        if let Some(session) = message["session_id"].as_str() {
            // The owner has taken up the start the shell sent: it is recording
            // it, or has already finished or failed it. Until then the start is
            // still pending, even while an earlier dictation reports progress --
            // that one may now be finishing in the background.
            if message["state"] != "requesting" {
                let mut pending = state.pending.lock().unwrap();
                if pending.as_deref() == Some(session) {
                    *pending = None;
                }
            }
            match message["state"].as_str() {
                Some("capturing") => state.delivery.lock().unwrap().captured(session),
                Some("canceled") => state.delivery.lock().unwrap().canceled(session),
                _ => {}
            }
        }
        let preferences = state.preferences.lock().unwrap().clone();
        let show = panel_shown(&state, &message);
        if let Some(overlay) = app.get_webview_window("overlay") {
            // Placed as it appears, so a dictation started on another screen
            // opens its panel there. A panel already up is placed again only
            // when it is on the wrong screen, so it never drifts around the
            // screen it is already on.
            if show
                && (!overlay.is_visible().unwrap_or(false)
                    || preferences::pointer_moved_monitor(&app))
            {
                preferences::position(&app, &preferences);
            }
            overlay_visibility(&overlay, show);
        }
        let stage = match message["state"].as_str() {
            Some("requesting") => Some("panel_opening"),
            Some("capturing") => Some("panel_listening"),
            _ => None,
        };
        if let (true, Some(stage), Some(recording)) = (
            entered,
            stage,
            message["session_id"]
                .as_str()
                .and_then(|id| uuid::Uuid::parse_str(id).ok()),
        ) {
            metrix_writer_api::latency::mark(recording, stage);
        }
        // Transcribing and failed show on the tray icon too.
        tray::sync(&app);
    }
    let _ = app.emit_to("overlay", "writer-session", message);
    Ok(())
}
/// Whether the panel belongs on screen for this snapshot: a dictation that has
/// not been dismissed, the always-visible idle panel, or a notice still being
/// read. "always" keeps an idle panel up to start the next dictation from; the
/// other modes hide between dictations.
fn panel_shown(state: &Shell, snapshot: &Value) -> bool {
    let visibility = state.preferences.lock().unwrap().visibility.clone();
    if state.probe || visibility == "off" {
        return false;
    }
    if state
        .notice_until
        .lock()
        .unwrap()
        .is_some_and(|until| until > std::time::Instant::now())
    {
        return true;
    }
    if matches!(
        snapshot["state"].as_str(),
        Some("idle" | "success" | "canceled")
    ) || (snapshot["state"] == "transcribing"
        && snapshot["session_id"]
            .as_str()
            .is_some_and(|session| state.delivery.lock().unwrap().was_pasted(session)))
    {
        visibility == "always"
    } else {
        state.dismissed.lock().unwrap().as_ref() != Some(&snapshot["session_id"])
    }
}
/// Says something on the panel that no dictation state covers: a shortcut press
/// Writer could not act on, a paste it skipped, insertion waiting for keys to
/// be released. A press of the shortcut should never go without an answer.
///
/// `session` scopes the notice to one dictation, and it is dropped once the
/// panel shows another. A `hold_ms` of zero keeps it until that dictation's next
/// change of state; otherwise it keeps the panel up for that long.
fn notice(app: &tauri::AppHandle, session: Option<&str>, text: &str, detail: &str, hold_ms: u64) {
    toned_notice(app, session, text, detail, hold_ms, None);
}
/// How long "Pasted" stays on the panel once the text is in, before the panel
/// goes the way it would have anyway.
const PASTED_MS: u64 = 350;
/// The text is in. Said with a tick for a moment, rather than by the panel
/// simply vanishing, which is also what a failure that hides it looks like.
fn pasted(app: &tauri::AppHandle, session: &str) {
    toned_notice(
        app,
        Some(session),
        "Pasted",
        "Your text is in.",
        PASTED_MS,
        Some("done"),
    );
}
/// A notice in a `tone` the panel draws: `done` puts a tick before the words.
fn toned_notice(
    app: &tauri::AppHandle,
    session: Option<&str>,
    text: &str,
    detail: &str,
    hold_ms: u64,
    tone: Option<&str>,
) {
    let state = app.state::<Shell>();
    let snapshot = state.snapshot.lock().unwrap().clone();
    if session.is_some_and(|session| {
        snapshot
            .as_ref()
            .is_none_or(|current| current["session_id"] != session)
    }) {
        return;
    }
    let preferences = state.preferences.lock().unwrap().clone();
    if state.probe || preferences.visibility == "off" {
        return;
    }
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    let id = state.notices.fetch_add(1, Ordering::AcqRel) + 1;
    if hold_ms > 0 {
        *state.notice_until.lock().unwrap() =
            Some(std::time::Instant::now() + Duration::from_millis(hold_ms));
    }
    let _ = app.emit_to(
        "overlay",
        "writer-notice",
        json!({"session_id": session, "text": text, "detail": detail, "ms": hold_ms, "tone": tone}),
    );
    if !overlay.is_visible().unwrap_or(false) {
        preferences::position(app, &preferences);
    }
    overlay_visibility(&overlay, true);
    if hold_ms == 0 {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(hold_ms));
        let state = app.state::<Shell>();
        // A newer notice owns the panel now, and hides it itself.
        if state.notices.load(Ordering::Acquire) != id {
            return;
        }
        *state.notice_until.lock().unwrap() = None;
        let snapshot = state.snapshot.lock().unwrap().clone();
        let show = snapshot.is_some_and(|snapshot| panel_shown(&state, &snapshot));
        if let Some(overlay) = app.get_webview_window("overlay") {
            overlay_visibility(&overlay, show);
        }
    });
}
/// The session owner could not take up a start the shell sent. The shell stops
/// waiting for it, and the panel says so instead of doing nothing.
#[tauri::command]
fn session_refused(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Shell>,
    session_id: String,
    code: Option<String>,
) -> Result<()> {
    main_only(&window)?;
    {
        let mut pending = state.pending.lock().unwrap();
        if pending.as_deref() == Some(session_id.as_str()) {
            *pending = None;
        }
    }
    {
        let mut held = state.held_session.lock().unwrap();
        if held
            .as_ref()
            .is_some_and(|(_, session)| *session == session_id)
        {
            *held = None;
        }
    }
    state.delivery.lock().unwrap().forget(&session_id);
    // A start let go of before it could begin needs no explanation.
    if code.as_deref() != Some("canceled") {
        notice(
            &app,
            None,
            "Busy",
            concat!(
                product_name!(),
                " could not start a new dictation yet. Press the shortcut again in a moment."
            ),
            2200,
        );
    }
    Ok(())
}
/// How many earlier dictations still have text to come while a newer one owns
/// the panel. The panel counts them, so an earlier dictation does not simply
/// vanish from sight until its text lands.
#[tauri::command]
fn session_waiting(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Shell>,
    count: u32,
) -> Result<()> {
    main_only(&window)?;
    state.waiting.store(u64::from(count), Ordering::Release);
    let _ = app.emit_to("overlay", "writer-waiting", json!({ "count": count }));
    Ok(())
}
/// Moves the panel to the screen the pointer moved to.
///
/// Only when the pointer crosses to another monitor, so a panel on the screen
/// you are working on never drifts around it. Crossing screens is followed even
/// mid-dictation: a panel left behind on the screen you just left is a panel you
/// cannot see, and the levels and progress it draws are what you are watching.
/// A hidden panel is moved too, so the next dictation opens where you already
/// are instead of waiting for its own placement.
fn follow_pointer(app: &tauri::AppHandle) {
    let preferences = app.state::<Shell>().preferences.lock().unwrap().clone();
    if preferences.visibility == "off" || !preferences::pointer_moved_monitor(app) {
        return;
    }
    preferences::position(app, &preferences);
}

/// Keep the overlay nonactivating on every appearance, including later sessions.
fn overlay_visibility(window: &WebviewWindow, visible: bool) {
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        // Wry owns a separate child HWND as well as the WebView2 controller.
        // Pair both sides of visibility, including hiding: repeatedly setting
        // only the controller to true can retain a broken composition surface.
        let view: &tauri::Webview = window.as_ref();
        let _ = if visible { view.show() } else { view.hide() };
        let handle = hwnd.0 as isize;
        let _ = window.run_on_main_thread(move || unsafe {
            win_input::ShowWindow(handle, if visible { 4 } else { 0 });
        });
    }
    #[cfg(not(windows))]
    {
        let _ = if visible {
            window.show()
        } else {
            window.hide()
        };
    }
}
/// Window controls for the caption the web UI draws.
///
/// The application menu Tauri gives a Mac, with Settings… (⌘,) where every Mac
/// app has it. It opens the window on the section used last.
#[cfg(target_os = "macos")]
fn settings_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
    let menu = Menu::default(app)?;
    if let Some(MenuItemKind::Submenu(application)) = menu.items()?.into_iter().next() {
        let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
        // After About and its separator, as on every Mac.
        application.insert(&settings, 2)?;
        application.insert(&PredefinedMenuItem::separator(app)?, 3)?;
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id().as_ref() == "settings" {
            reveal(app);
            let _ = app.emit_to("main", "writer-open-settings", Option::<String>::None);
        }
    });
    Ok(())
}
/// Deliberately narrow commands rather than granting the page the core window
/// capabilities: `close` goes through the same request the real close button
/// does, so "closing the window keeps Writer running" stays one behaviour with
/// one implementation.
#[tauri::command]
fn window_command(window: WebviewWindow, command: String) -> Result<()> {
    main_only(&window)?;
    match command.as_str() {
        "minimize" => {
            let _ = window.minimize();
        }
        "toggle-maximize" => {
            if window.is_maximized().unwrap_or(false) {
                let _ = window.unmaximize();
            } else {
                let _ = window.maximize();
            }
        }
        "close" => {
            let _ = window.close();
        }
        _ => return Err("Invalid window command.".into()),
    }
    Ok(())
}

#[tauri::command]
fn window_state(window: WebviewWindow) -> Result<Value> {
    main_only(&window)?;
    Ok(json!({ "maximized": window.is_maximized().unwrap_or(false) }))
}

/// Quit from Settings, taking the same drain-and-save path as the tray menu.
#[tauri::command]
fn request_quit_command(window: WebviewWindow, app: tauri::AppHandle) -> Result<()> {
    main_only(&window)?;
    request_quit(&app);
    Ok(())
}

/// Opens the native key dialog. The key itself never crosses this boundary:
/// `credentials::prompt` reads it natively and writes it to the protected
/// store, and nothing is returned to the page but `"saved"`, `"canceled"`, or a
/// message.
#[tauri::command]
async fn credential_prompt(
    window: WebviewWindow,
    app: tauri::AppHandle,
    provider: String,
) -> Result<&'static str> {
    main_only(&window)?;
    let root = app.state::<Shell>().root.clone();
    tauri::async_runtime::spawn_blocking(move || credentials::prompt(&root, &provider))
        .await
        .map_err(|_| "The key dialog could not be opened.")?
}

#[tauri::command]
fn credential_remove(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    provider: String,
) -> Result<()> {
    main_only(&window)?;
    credentials::remove(&state.root, &provider)
}

#[tauri::command]
fn data_location(window: WebviewWindow, state: tauri::State<'_, Shell>) -> Result<Value> {
    main_only(&window)?;
    Ok(json!({ "root": state.root.to_string_lossy() }))
}

/// Shows the storage folder in the system file manager. The path is the shell's
/// own root, never a value supplied by the page.
#[tauri::command]
fn open_data_folder(window: WebviewWindow, state: tauri::State<'_, Shell>) -> Result<()> {
    main_only(&window)?;
    show_data_folder(&state.root)
}
/// About's legal links. The page names one; the address is the shell's own.
#[tauri::command]
fn legal_open(window: WebviewWindow, kind: String) -> Result<()> {
    main_only(&window)?;
    let url = crate::notices::link(&kind).ok_or("Unknown link.")?;
    if sign_in::open_browser(&url) {
        Ok(())
    } else {
        Err(format!("The page could not be opened. It is at {url}"))
    }
}
#[tauri::command]
fn notices_index(window: WebviewWindow) -> Result<Vec<crate::notices::Notice>> {
    main_only(&window)?;
    crate::notices::index(&resources::dir()?)
}
#[tauri::command]
fn notice_text(window: WebviewWindow, path: String) -> Result<String> {
    main_only(&window)?;
    crate::notices::read(&resources::dir()?, &path)
}
/// Shows the installed notices folder, which also holds the FFmpeg source.
#[tauri::command]
fn notices_folder(window: WebviewWindow) -> Result<()> {
    main_only(&window)?;
    show_data_folder(&resources::dir()?.join("notices"))
}
fn show_data_folder(root: &std::path::Path) -> Result<()> {
    let opened = if cfg!(windows) {
        std::process::Command::new("explorer").arg(root).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(root).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(root).spawn()
    };
    opened.map(|_| ()).map_err(|_| {
        "The folder could not be opened. The path above can be pasted into a file manager.".into()
    })
}

/// The panel's own view of a dictation-start state: when the snapshot reached
/// its page and when a frame showing it had been produced, as Unix
/// milliseconds, and whether it came as the event itself or from the resync.
#[derive(serde::Deserialize)]
struct PanelShown {
    session_id: String,
    state: String,
    received_ms: f64,
    painted_ms: f64,
    via: String,
}
#[tauri::command]
fn overlay_action(
    window: WebviewWindow,
    app: tauri::AppHandle,
    action: String,
    epoch: Option<String>,
    revision: Option<u64>,
    nonce: Option<u64>,
    shown: Option<PanelShown>,
) -> Result<()> {
    security::window_only(&window, "overlay")?;
    match action.as_str() {
        // `session_event` marks panel_opening/panel_listening when it forwards
        // the snapshot; these mark when the panel had it and had painted it.
        "shown" => {
            let shown = shown.ok_or("Invalid action.")?;
            let recording =
                uuid::Uuid::parse_str(&shown.session_id).map_err(|_| "Invalid action.")?;
            let (received, painted, via) = match shown.state.as_str() {
                "requesting" => (
                    "panel_opening_received",
                    "panel_opening_painted",
                    "panel_opening_via",
                ),
                "capturing" => (
                    "panel_listening_received",
                    "panel_listening_painted",
                    "panel_listening_via",
                ),
                _ => return Err("Invalid action.".into()),
            };
            metrix_writer_api::latency::mark_at_wall(recording, received, shown.received_ms);
            metrix_writer_api::latency::mark_at_wall(recording, painted, shown.painted_ms);
            if let Some(value) = ["event", "sync"].into_iter().find(|v| *v == shown.via) {
                metrix_writer_api::latency::detail(recording, via, value);
            }
        }
        // The panel is nonactivating, so starting from it leaves the foreground
        // window -- the one the transcript is inserted into -- untouched.
        "start" | "stop" | "cancel" | "retry" => dispatch(&app, &action),
        "dismiss" => {
            let state = app.state::<Shell>();
            let snapshot = state.snapshot.lock().unwrap();
            if !snapshot.as_ref().is_some_and(|s| {
                s["epoch"].as_str() == epoch.as_deref() && s["revision"].as_u64() == revision
            }) {
                return Ok(());
            }
            let dismissed = snapshot.as_ref().map(|s| s["session_id"].clone());
            *state.dismissed.lock().unwrap() = dismissed;
            overlay_visibility(&window, false);
            // A dismissed failure no longer needs the icon's attention.
            tray::sync(&app);
        }
        "drag" => {
            let _ = window.start_dragging();
        }
        "alive" => {
            if let Some(nonce) = nonce {
                panel_recovery::acknowledge(&window, nonce);
            }
        }
        "ready" => {
            let _ = app.emit_to(
                "overlay",
                "writer-preferences",
                app.state::<Shell>().preferences.lock().unwrap().clone(),
            );
            if let Some(theme) = app.state::<Shell>().theme.lock().unwrap().clone() {
                let _ = app.emit_to("overlay", "writer-theme", theme);
            }
            if let Some(snapshot) = app.state::<Shell>().snapshot.lock().unwrap().clone() {
                let _ = app.emit_to("overlay", "writer-session", snapshot);
            }
            let waiting = app.state::<Shell>().waiting.load(Ordering::Acquire);
            let _ = app.emit_to("overlay", "writer-waiting", json!({ "count": waiting }));
        }
        "open" => reveal(&app),
        _ => return Err("Invalid action.".into()),
    };
    Ok(())
}
fn dispatch(app: &tauri::AppHandle, action: &str) {
    dispatch_expected(app, action, None);
}
fn dispatch_expected(app: &tauri::AppHandle, action: &str, expected: Option<(String, String)>) {
    dispatch_full(app, action, expected, None);
}
fn dispatch_full(
    app: &tauri::AppHandle,
    action: &str,
    expected: Option<(String, String)>,
    mode: Option<String>,
) {
    let state = app.state::<Shell>();
    if state.quitting.load(Ordering::Acquire) {
        return;
    }
    let snapshot = state.snapshot.lock().unwrap().clone();
    // A dictation still being saved, transcribed or inserted no longer holds the
    // next one back: the session owner finishes it in the background. Only one
    // still recording does, and a press while it records is the key repeating
    // (hold) or has already become Stop (toggle).
    if action == "start"
        && (state.pending.lock().unwrap().is_some()
            || snapshot
                .as_ref()
                .is_some_and(|s| matches!(s["state"].as_str(), Some("requesting" | "capturing"))))
    {
        return;
    }
    let _maintenance = if action == "start" {
        match state.maintenance.try_lock() {
            Ok(guard) => Some(guard),
            Err(_) => {
                notice(
                    app,
                    None,
                    "Busy",
                    concat!(product_name!(), " is finishing a settings or history change. Press the shortcut again in a moment."),
                    2200,
                );
                return;
            }
        }
    } else {
        None
    };
    if action == "start" && state.history_reload.load(Ordering::Acquire) {
        notice(
            app,
            None,
            "Busy",
            concat!(
                product_name!(),
                " is reloading after clearing history. Press the shortcut again in a moment."
            ),
            2200,
        );
        return;
    }
    if snapshot.is_none() && action == "start" {
        notice(
            app,
            None,
            "Starting",
            concat!(
                product_name!(),
                " is still starting. Press the shortcut again in a moment."
            ),
            2200,
        );
    }
    if let Some(snapshot) = snapshot {
        let (epoch, id) = if action == "start" {
            let Some(epoch) = snapshot["epoch"].as_str() else {
                return;
            };
            let recording = uuid::Uuid::new_v4();
            let id = recording.to_string();
            // A press this start did not come from -- one refused earlier, or a
            // start from the panel -- is not this dictation's first stage.
            match state
                .pressed_at
                .lock()
                .unwrap()
                .take()
                .filter(|at| at.elapsed() < Duration::from_secs(2))
            {
                Some(at) => metrix_writer_api::latency::begin(recording, "shortcut_pressed", at),
                None => metrix_writer_api::latency::mark(recording, "start_requested"),
            }
            *state.capture_mode.lock().unwrap() = mode.map(|mode| (id.clone(), mode));
            delivery::capture_target(&state, &id);
            *state.held_session.lock().unwrap() = Some((epoch.to_owned(), id.clone()));
            *state.pending.lock().unwrap() = Some(id.clone());
            (epoch.to_owned(), id)
        } else {
            let current_session = state
                .pending
                .lock()
                .unwrap()
                .clone()
                .or_else(|| snapshot["session_id"].as_str().map(str::to_owned));
            let Some(target) = input_guard::command_target(
                expected.as_ref().map(|(e, s)| (e.as_str(), s.as_str())),
                snapshot["epoch"].as_str(),
                current_session.as_deref(),
            ) else {
                return;
            };
            (target.epoch, target.session)
        };
        if action == "stop" {
            if let Ok(recording) = uuid::Uuid::parse_str(&id) {
                metrix_writer_api::latency::mark(recording, "stop_accepted");
                state
                    .stopped_at
                    .lock()
                    .unwrap()
                    .record(recording, std::time::Instant::now());
            }
        }
        if action == "start" {
            if let Ok(recording) = uuid::Uuid::parse_str(&id) {
                metrix_writer_api::latency::mark(recording, "start_dispatched");
            }
        }
        let _ = app.emit_to("main", "writer-command", json!({"version":1,"kind":"command","epoch":epoch,"command_id":uuid::Uuid::new_v4(),"action":action,"session_id":id}));
        if action != "start" {
            let mut pending = state.pending.lock().unwrap();
            if pending.as_deref() == Some(id.as_str()) {
                *pending = None;
            }
        }
    }
}
#[tauri::command]
fn shortcut_capture(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    active: bool,
) -> Result<()> {
    main_only(&window)?;
    state.shortcut_capture.store(active, Ordering::Release);
    state.pressed.store(false, Ordering::Release);
    #[cfg(windows)]
    modifier_hook::capturing(active);
    #[cfg(target_os = "macos")]
    macos_hook::capturing(active);
    Ok(())
}
/// Tells the window that one of Writer's own two chords changed physical state,
/// so the first-run guide can draw the combination as it is actually held.
///
/// Only which of the two chords it was travels here. No key code, no other
/// application's input and nothing typed: this fires exactly where the shell has
/// already decided the event belongs to Writer, on the same debounced edges the
/// recorder itself acts on.
fn hotkey_edge(app: &tauri::AppHandle, chord: &str, pressed: bool) {
    let _ = app.emit_to(
        "main",
        "writer-hotkey",
        json!({"chord": chord, "pressed": pressed}),
    );
}
fn shortcut_event(app: &tauri::AppHandle, cancel: bool, pressed: bool) {
    shortcut_event_at(app, cancel, pressed, std::time::Instant::now());
}
/// `at` is when the key event reached Writer, which a hook thread records
/// before handing the edge on.
fn shortcut_event_at(app: &tauri::AppHandle, cancel: bool, pressed: bool, at: std::time::Instant) {
    let state = app.state::<Shell>();
    if state.shortcut_capture.load(Ordering::Acquire) {
        return;
    }
    if cancel {
        if pressed {
            hotkey_edge(app, "cancel", true);
            dispatch(app, "cancel");
        }
        return;
    }
    let prefs = state.preferences.lock().unwrap().clone();
    if pressed && state.pressed.swap(true, Ordering::AcqRel) {
        return;
    }
    if pressed {
        *state.pressed_at.lock().unwrap() = Some(at);
    }
    if !pressed {
        state.pressed.store(false, Ordering::Release);
    }
    hotkey_edge(app, "recording", pressed);
    if prefs.toggle {
        if pressed {
            let active = state.pending.lock().unwrap().is_some()
                || state.snapshot.lock().unwrap().as_ref().is_some_and(|s| {
                    matches!(s["state"].as_str(), Some("requesting" | "capturing"))
                });
            dispatch(app, if active { "stop" } else { "start" });
        }
    } else if pressed {
        dispatch(app, "start");
    } else {
        let Some((epoch, session)) = state.held_session.lock().unwrap().take() else {
            return;
        };
        let app = app.clone();
        std::thread::spawn(move || {
            #[cfg(windows)]
            while unsafe {
                preferences::keys(&prefs.recording)
                    .unwrap_or_default()
                    .iter()
                    .any(|key| {
                        win_input::GetAsyncKeyState(*key) < 0
                            || *key == 0x5b && win_input::GetAsyncKeyState(0x5c) < 0
                    })
            } {
                std::thread::sleep(Duration::from_millis(20));
            }
            dispatch_expected(&app, "stop", Some((epoch, session)));
        });
    }
}
fn reveal(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}
fn request_quit(app: &tauri::AppHandle) {
    let state = app.state::<Shell>();
    if state.quitting.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = app.emit_to("main", "writer-quit", ());
    // If the document is absent/crashed, native capture remains independently
    // drainable. Never force exit while a successful final save is unconfirmed.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if app.state::<Shell>().quitting.load(Ordering::Acquire) {
            shutdown(app).await;
        }
    });
}
#[tauri::command]
async fn quit_ready(window: WebviewWindow, app: tauri::AppHandle) -> Result<()> {
    main_only(&window)?;
    if app.state::<Shell>().quitting.load(Ordering::Acquire) {
        shutdown(app).await;
    }
    Ok(())
}
async fn shutdown(app: tauri::AppHandle) {
    let state = app.state::<Shell>();
    system::recording(&app, false, false);
    let capture = state.capture.clone();
    let saved = tauri::async_runtime::spawn_blocking(move || capture.quiesce()).await;
    if !matches!(saved, Ok(Ok(ref status)) if status.error.is_none() || status.drain_confirmed) {
        state.quitting.store(false, Ordering::Release);
        reveal(&app);
        return;
    }
    if let Ok(setup) = metrix_writer_api::desktop_settings::global() {
        let _ = setup.runtime.select(None);
        for _ in 0..100 {
            if matches!(
                setup.runtime.worker(),
                metrix_writer_api::local_stt::managed::WorkerState::Idle
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    app.exit(0);
}
fn relay(
    capture: Capture,
    router: axum::Router,
    request: Request<Vec<u8>>,
    response: &mut Response<Cow<'static, [u8]>>,
) {
    let path = request.uri().path().to_owned();
    // This hook belongs only to the main WebView. Never turn a foreign URL or
    // Origin into an authenticated in-process API request by stripping its host.
    if (path.starts_with("/api/") || path.starts_with("/native/"))
        && (!tauri::Url::parse(&request.uri().to_string())
            .is_ok_and(|url| security::local_url(&url))
            || request.headers().get("origin").is_some_and(|origin| {
                !origin
                    .to_str()
                    .ok()
                    .and_then(|value| tauri::Url::parse(value).ok())
                    .is_some_and(|url| security::local_url(&url))
            }))
    {
        *response = Response::builder()
            .status(403)
            .body(Cow::Borrowed(b"Untrusted document." as &[u8]))
            .unwrap();
        return;
    }
    if path.starts_with("/api/v1/") {
        let (mut parts, body) = request.into_parts();
        // The router is invoked in-process. There is no exposed port or bearer
        // token in a browser, command line, environment or discovery file.
        parts.uri = parts
            .uri
            .path_and_query()
            .unwrap()
            .as_str()
            .parse()
            .unwrap();
        let result = std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                let result = router
                    .oneshot(Request::from_parts(parts, Body::from(body)))
                    .await
                    .unwrap();
                let (parts, body) = result.into_parts();
                to_bytes(body, 1024 * 1024 * 1024 + 65536)
                    .await
                    .map(|bytes| Response::from_parts(parts, Cow::Owned(bytes.to_vec())))
            })
        })
        .join();
        *response = match result {
            Ok(Ok(r)) => r,
            _ => Response::builder()
                .status(503)
                .body(Cow::Borrowed(b"Backend unavailable." as &[u8]))
                .unwrap(),
        };
    } else if let Some(id) = path
        .strip_prefix("/native/preview-audio/")
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
    {
        let audio = if capture.status().is_ok_and(|s| s.active && s.id == Some(id)) {
            capture.store.snapshot_audio(id)
        } else {
            Err("No active preview recording.".into())
        };
        *response = match audio {
            Ok(bytes) => Response::builder()
                .header("Content-Type", "audio/wav")
                .header("Cache-Control", "no-store")
                .body(Cow::Owned(bytes))
                .unwrap(),
            Err(_) => Response::builder()
                .status(409)
                .header("Cache-Control", "no-store")
                .body(Cow::Borrowed(b"Preview audio unavailable." as &[u8]))
                .unwrap(),
        };
    } else if let Some(id) = path
        .strip_prefix("/native/audio/")
        .and_then(|s| s.parse().ok())
    {
        *response = match capture.store.audio(id) {
            Ok(bytes) => Response::builder()
                .header("Content-Type", "audio/wav")
                .header("Cache-Control", "no-store")
                .body(Cow::Owned(bytes))
                .unwrap(),
            Err(_) => {
                let router = router.clone();
                std::thread::spawn(move || {
                    tauri::async_runtime::block_on(async move {
                        let r = router
                            .oneshot(
                                Request::builder()
                                    .uri(format!("/api/v1/recordings/{id}/audio"))
                                    .body(Body::empty())
                                    .unwrap(),
                            )
                            .await
                            .unwrap();
                        let (parts, body) = r.into_parts();
                        let bytes = to_bytes(body, 1024 * 1024 * 1024 + 65536)
                            .await
                            .unwrap_or_default();
                        Response::from_parts(parts, Cow::Owned(bytes.to_vec()))
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Response::builder()
                        .status(503)
                        .body(Cow::Borrowed(b"Audio unavailable." as &[u8]))
                        .unwrap()
                })
            }
        };
    }
}
pub fn run() {
    if let Err(error) = run_inner() {
        let args: Vec<_> = std::env::args().skip(1).collect();
        // The launch that failed is the one a later report is about, and a message
        // box the user dismissed leaves nothing behind. Record it where it can be read.
        if let Some(root) = args
            .iter()
            .position(|a| a == "--user-root")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
            .or_else(|| default_root().ok())
        {
            startup_log::mark(&root, &format!("failed: {error}"));
        }
        if args
            .first()
            .is_some_and(|s| s == "--probe" || s == "--dictation-probe")
        {
            if let Some(root) = args.get(1) {
                let _ = storage::atomic_json(
                    &PathBuf::from(root).join("probe.json"),
                    &json!({"startup": false}),
                );
            }
            std::process::exit(2);
        }
        // Error strings are bounded application messages, never keys or transcripts.
        #[cfg(windows)]
        unsafe {
            #[link(name = "user32")]
            extern "system" {
                fn MessageBoxW(w: isize, text: *const u16, title: *const u16, flags: u32) -> i32;
            }
            let message: Vec<_> = error.encode_utf16().chain(Some(0)).collect();
            let title: Vec<_> = product_name!().encode_utf16().chain(Some(0)).collect();
            MessageBoxW(0, message.as_ptr(), title.as_ptr(), 0x10);
        }
        #[cfg(target_os = "macos")]
        startup_alert(&error);
    }
}
#[cfg(target_os = "macos")]
fn startup_alert(error: &str) {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAlert, NSAlertStyle};
    use objc2_foundation::NSString;
    let mtm = MainThreadMarker::new().expect("startup runs on the main thread");
    let alert = NSAlert::init(NSAlert::alloc(mtm));
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str(concat!(
        product_name!(),
        " could not start"
    )));
    alert.setInformativeText(&NSString::from_str(error));
    alert.runModal();
}

/// The per-user folder that holds the data root.
fn data_base() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(
            std::env::var_os("LOCALAPPDATA").ok_or("User data directory unavailable.")?,
        ))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(
            PathBuf::from(std::env::var_os("HOME").ok_or("User data directory unavailable.")?)
                .join("Library/Application Support"),
        )
    }
}
/// The per-user data root a launch with no path argument resolves to. Kept as
/// one function because launch at login has to reproduce exactly this launch.
/// It is the Metrix Writer folder only while that folder's carry-over is pending.
fn default_root() -> Result<PathBuf> {
    Ok(identity::resolve(&data_base()?))
}
/// Carries a Metrix Writer installation over to this identity before anything
/// opens its data, then returns the ordinary root (docs/release/identity.md).
fn carry_over() -> Result<(PathBuf, identity::Outcome)> {
    let base = data_base()?;
    let outcome = identity::carry_over_root(&base, Duration::from_secs(5));
    let root = identity::resolve(&base);
    #[cfg(target_os = "macos")]
    if root == base.join(identity::DATA_FOLDER) {
        // WebKit keeps the window's local storage per bundle identifier.
        if let Some(library) = base.parent() {
            for parent in ["WebKit", "HTTPStorages"] {
                let _ = identity::carry_over_folder(&library.join(parent));
            }
        }
        if identity::secrets_pending(&root)
            && identity::carry_over_secrets(
                &credentials::Keychain,
                &root,
                &base.join(identity::LEGACY_DATA_FOLDER),
            )
        {
            identity::secrets_settled(&root);
        }
    }
    Ok((root, outcome))
}
fn run_inner() -> Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let hidden = args.first().is_some_and(|s| s == "--hidden");
    if hidden {
        args.remove(0);
    }
    // Sign-in launches show the window without taking the foreground. `--hidden`
    // left only a tray icon, which reads as "Writer did not start"; stealing focus
    // at sign-in would type into Writer instead of whatever was already open.
    let autostart = args.first().is_some_and(|s| s == "--autostart");
    if autostart {
        args.remove(0);
    }
    let replay_probe =
        cfg!(feature = "latency-probe") && args.first().is_some_and(|s| s == "--dictation-probe");
    let security_probe =
        cfg!(feature = "security-probe") && args.first().is_some_and(|s| s == "--security-probe");
    let probe = replay_probe || security_probe || args.first().is_some_and(|s| s == "--probe");
    let mut carried = None;
    let root = match args.as_slice() {
        [] => {
            let (root, outcome) = carry_over()?;
            carried = Some(outcome);
            root
        }
        [flag, path, _wav] if replay_probe && flag == "--dictation-probe" => {
            let p = PathBuf::from(path);
            if !p.is_absolute() {
                return Err("Probe root must be absolute.".into());
            }
            p
        }
        [flag, path]
            if flag == "--user-root"
                || flag == "--probe"
                || (security_probe && flag == "--security-probe") =>
        {
            let p = PathBuf::from(path);
            if !p.is_absolute() {
                return Err("User root must be absolute.".into());
            }
            p
        }
        _ => {
            return Err(concat!(
                "Usage: ",
                product_name!(),
                " [--user-root <absolute directory>]"
            )
            .into())
        }
    };
    if security_probe && root.exists() {
        return Err("Security probe requires a new private root.".into());
    }
    if replay_probe && root.join("Data").exists() {
        return Err("Replay requires a fresh isolated root with no recording history.".into());
    }
    let replay_duration = if replay_probe {
        let input = std::fs::read(&args[2]).map_err(|_| "Replay fixture unavailable.")?;
        let wav =
            writer_dictation_audio::wav_pcm16(&input).map_err(|_| "Replay requires PCM16 WAV.")?;
        wav.data.len() as u64 / (u64::from(wav.channels) * 2) * 1000 / u64::from(wav.rate)
    } else {
        0
    };
    privacy::protect(&root)?;
    startup_log::mark(
        &root,
        &format!("launch hidden={hidden} autostart={autostart} probe={probe}"),
    );
    if let Some(outcome) = &carried {
        startup_log::mark(&root, &format!("identity carry-over: {outcome:?}"));
    }
    let data = root.join("Data");
    #[cfg(target_os = "macos")]
    {
        privacy::protect(&data)?;
        privacy::protect(&root.join("State"))?;
    }
    std::fs::create_dir_all(&data).map_err(|_| "Cannot open private data directory.")?;
    // Match the previous shell's exclusive data lease, including coexistence.
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    // Acquired in setup, after single-instance dispatch has revealed an existing app.
    std::env::set_var("WRITER_DATA_DIR", &data);
    std::env::set_var("WRITER_STATE_DIR", root.join("State"));
    std::env::set_var("WRITER_MODELS_DIR", root.join("Models"));
    std::env::set_var("WRITER_VOCABULARY_PATH", data.join("vocabulary.json"));
    std::env::set_var("WRITER_DESKTOP_IN_PROCESS", "1");
    let resources = resources::dir()?;
    resources::verify(&resources)?;
    startup_log::mark(&root, "packaged runtime verified");
    std::env::set_var("WRITER_RUNTIME_DIR", resources.join("runtime/whisper"));
    std::env::set_var(
        "WRITER_LOCAL_STT_FFMPEG",
        resources.join(if cfg!(windows) {
            "runtime/ffmpeg/ffmpeg.exe"
        } else {
            "runtime/ffmpeg/ffmpeg"
        }),
    );
    #[cfg(feature = "security-probe")]
    if security_probe {
        security_probe::prepare(&root)?;
    }
    let capture_store = Store::open(data.join("native-recordings"))?;
    #[cfg(feature = "latency-probe")]
    let capture = if replay_probe {
        Capture::replay(capture_store, PathBuf::from(&args[2]))
    } else {
        Capture::new(capture_store)
    };
    #[cfg(not(feature = "latency-probe"))]
    let capture = Capture::new(capture_store);
    let router =
        metrix_writer_api::app(Default::default(), PathBuf::from("unused-desktop-web-root"));
    let capture_setup = capture.clone();
    let builder = tauri::Builder::default();
    // Explicit private roots and probes have their own data-directory lease.
    // They must never activate or alter the user's ordinary installed instance.
    let builder = if probe || args.first().is_some_and(|s| s == "--user-root") {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| reveal(app)))
    };
    let app = builder
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(|app, shortcut, event| {
            let state = app.state::<Shell>();
            if mode_shortcuts::handle(app, shortcut, event.state == ShortcutState::Pressed) { return; }
            let cancel = preferences::shortcut(&state.preferences.lock().unwrap().cancel).is_ok_and(|s| s == *shortcut);
            #[cfg(target_os = "macos")]
            if macos_hook::active() { return; }
            shortcut_event(app, cancel, event.state == ShortcutState::Pressed);
        }).build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::Updates::default())
        .invoke_handler(move |invoke| {
            #[cfg(feature = "security-probe")]
            if invoke.message.command() == "security_probe_receipt" { return security_probe::receive(invoke); }
            if !security::allowed(invoke.message.command(), invoke.message.webview_ref()) {
                invoke.resolver.reject("Untrusted document.");
                return true;
            }
            let handler: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![onboarding::accessibility_status, onboarding::accessibility_open, mode_shortcuts::modes_save, updates::updates_status, updates::updates_check, updates::updates_download, updates::updates_install, updates::updates_cancel, list_microphones, select_microphone, microphone_prefer_built_in, history_delete, history_resume, storage_controls::storage_status, storage_controls::storage_preview, storage_controls::storage_clear, system::system_settings_get, system::system_settings_save, onboarding::onboarding_get, onboarding::onboarding_save, delivery::deliver_transcript, delivery::prepare_delivery, session_refused, session_waiting, shortcut_capture, capture_start, transcription::capture_import, transcription::capture_preview, transcription::capture_preview_start, dictation_probe_receipt, dictation_probe_command, capture_stop, capture_status, capture_get, capture_list, capture_delete, saved_attempts, session_event, overlay_action, quit_ready, preferences::preferences_get, preferences::preferences_save, preferences::preferences_theme, preferences::import_model, probe_result, window_command, window_state, request_quit_command, credential_prompt, credential_remove, data_location, open_data_folder, legal_open, notices_index, notice_text, notices_folder, sign_in::account_snapshot, sign_in::account_sign_in, sign_in::account_reauthenticate, sign_in::account_cancel_sign_in, sign_in::account_sign_out, sign_in::account_switch, sign_in::account_local_copy, sign_in::account_tasks, sign_in::account_sensitive, portable::portable_status, portable::portable_export, portable::portable_import_preview, portable::portable_import_apply, portable::portable_import_discard, portable::portable_rollback, portable::portable_theme, portable::sync_preview, portable::sync_confirm, portable::sync_control, portable::sync_turn_off, portable::sync_resolve, history_sync::transcript_status, history_sync::transcript_preview, history_sync::transcript_confirm, history_sync::transcript_page, history_sync::transcript_edit, history_sync::transcript_delete, history_sync::transcript_turn_off, tray::tray_menu_ready, tray::tray_menu_measured, tray::tray_menu_command, tray::tray_menu_dismiss];
            handler(invoke)
        })
        .setup(move |app| {
            let initialized = (|| -> std::result::Result<(), Box<dyn std::error::Error>> {
            #[cfg(target_os = "macos")]
            let hidden = hidden || system::launched_at_login();
            let lease = options.open(data.join("desktop.lock")).map_err(|_| concat!(product_name!(), " is already using this data. Quit the previous shell before launching Tauri."))?;
            fs2::FileExt::try_lock_exclusive(&lease).map_err(|_| concat!(product_name!(), " data is already in use."))?;
            startup_log::mark(&root, "data lease acquired");
            let preferences = preferences::Preferences::load(&root)?;
            app.manage(Shell { replay_probe, preview_sessions: Mutex::new(Default::default()), final_options: Mutex::new(Default::default()), stopped_at: Mutex::new(Default::default()), mode_shortcuts: Mutex::new(Vec::new()), capture_mode: Mutex::new(None), context: std::sync::Arc::new(context::ContextReader::new()), history_reload: AtomicBool::new(false), storage_preview: Mutex::new(None), maintenance: tokio::sync::Mutex::new(()), held_session: Mutex::new(None), system: Mutex::new(system::Settings::load(&root, carried == Some(identity::Outcome::Moved))?), #[cfg(windows)] ducking: ducking::Ducking::start(root.join("State/ducking-recovery.json")), delivery: Mutex::new(Default::default()), shortcut_capture: AtomicBool::new(false), capture: capture_setup.clone(), snapshot: Mutex::new(None), pending: Mutex::new(None), dismissed: Mutex::new(None), quitting: AtomicBool::new(false), preferences: Mutex::new(preferences.clone()), theme: Mutex::new(None), panel_health: Mutex::new(Default::default()), notices: AtomicU64::new(0), notice_until: Mutex::new(None), waiting: AtomicU64::new(0), pressed: AtomicBool::new(false), pressed_at: Mutex::new(None), root: root.clone(), probe, _lease: lease });
            sign_in::setup(app.handle(), &root);
            portable::setup(app.handle(), &root);
            tauri::async_runtime::block_on(async {
                let pipeline = metrix_writer_api::jobs::runtime().await.map_err(|e| e.message)?;
                capture_setup.list()?; // repairs interrupted captures
                let mut retained: std::collections::BTreeSet<_> = capture_setup.store.ids()?.into_iter().collect();
                for recording in pipeline.recordings.list().await.map_err(|e| e.message)? { retained.insert(recording.metadata.id); }
                for job in pipeline.jobs.all().await.map_err(|e| e.message)? { retained.insert(job.request.recording_id); }
                pipeline.mode_service().await.map_err(|e| e.message)?.reconcile_captures(&retained).await.map_err(|e| e.message)
            }).map_err(|_| "Cannot reconcile saved context.")?;
            startup_log::mark(&root, "saved jobs ready");
            let main = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title(product_name!()).focused(!probe && !hidden && !autostart).visible(!probe && !hidden)
                .resizable(true)
                .data_directory(root.join("TauriWebView2"))
                .on_navigation(|url| security::document_url("main", url))
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .on_web_resource_request(move |request, response| relay(capture_setup.clone(), router.clone(), request, response));
            // Where it was last time, in the theme's colour.
            let main = window_place::prepare(app.handle(), &root, main);
            // Windows draws its caption in the page. A Mac keeps its own
            // traffic lights, centred in the page's 48 px bar, which leaves
            // room for them.
            #[cfg(not(target_os = "macos"))]
            let main = main.decorations(false);
            #[cfg(target_os = "macos")]
            let main = main
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .hidden_title(true)
                .traffic_light_position(tauri::LogicalPosition::new(18.0, 26.0));
            let main = main.build()?;
            #[cfg(target_os = "macos")]
            settings_menu(app.handle())?;
            let overlay = panel_recovery::create(app.handle())?;
            startup_log::mark(&root, "main window and panel created");
            // A sign-in launch can run before the session has composed a monitor.
            // Losing the panel's opening position is recoverable; aborting the
            // whole launch over it is not, and `preferences::position` below
            // places the panel again from the saved placement.
            if let (Ok(Some(monitor)), Ok(size)) = (main.current_monitor(), overlay.outer_size()) {
                let area = monitor.work_area();
                let _ = overlay.set_position(tauri::PhysicalPosition::new(area.position.x + (area.size.width as i32 - size.width as i32)/2, area.position.y + area.size.height as i32 - size.height as i32 - 24));
            }
            #[cfg(feature = "security-probe")]
            if security_probe { security_probe::start(app.handle())?; return Ok(()); }
            if probe {
                #[cfg(all(target_os = "macos", feature = "latency-probe"))]
                if !replay_probe && root.join("macos-context-fixture.json").is_file() { macos_acceptance::context(app.handle()); return Ok(()); }
                #[cfg(all(target_os = "macos", feature = "latency-probe"))]
                if !replay_probe && root.join("macos-delivery-fixture").is_file() { macos_acceptance::delivery(app.handle()); return Ok(()); }
                #[cfg(target_os = "macos")]
                system::probe_overlay(app.handle());
                #[cfg(all(target_os = "macos", feature = "latency-probe"))]
                if !replay_probe && root.join("macos-activation-fixture").is_file() { macos_delivery::probe_activation(app.handle()); }
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(2));
                    if let Some(w) = handle.get_webview_window("main") {
                        let script = if replay_probe { include_str!("dictation-probe.js") } else { include_str!("probe.js") };
                        let _ = w.eval(format!("window.writerReplayDurationMs={replay_duration};{script}"));
                    }
                    std::thread::sleep(Duration::from_secs(if replay_probe { 120 } else { 40 }));
                    handle.exit(2);
                });
                if !replay_probe { return Ok(()); }
            }
            let events = app.handle().clone();
            let mut changes = tauri::async_runtime::block_on(metrix_writer_api::jobs::runtime()).map_err(|_| "Cannot subscribe to saved jobs.")?.jobs.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    match changes.recv().await {
                        Ok(job) => { let _ = events.emit_to("main", "writer-job-status", job); }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
            panel_recovery::start(app.handle());
            let observer = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = 0u8;
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if observer.state::<Shell>().quitting.load(Ordering::Acquire) { break; }
                    if let Ok(status) = observer.state::<Shell>().capture.status() {
                        let _ = observer.emit_to("main", "writer-capture-status", status);
                    }
                    let job_id = observer.state::<Shell>().snapshot.lock().unwrap().as_ref().and_then(|s| {
                        if matches!(s["state"].as_str(), Some("queued" | "transcribing")) { s["job_id"].as_str().and_then(|id| id.parse::<uuid::Uuid>().ok()) } else { None }
                    });
                    if let Some(id) = job_id {
                        if let Ok(pipeline) = metrix_writer_api::jobs::runtime().await {
                            if let Ok(job) = pipeline.jobs.get(id).await { let _ = observer.emit_to("main", "writer-job-status", job); }
                        }
                    }
                    follow_pointer(&observer);
                    tick = (tick + 1) % 5;
                    if tick != 0 { continue; }
                    if let Ok(setup) = metrix_writer_api::desktop_settings::global() {
                        let model = setup.settings().transcription.selection.model;
                        let _ = observer.emit_to("overlay", "writer-model", json!({"worker":setup.runtime.worker(),"weights":setup.runtime.weights(&model)}));
                    }
                }
            });
            if replay_probe { return Ok(()); }
            // Writer draws its own tray menu. The window is borderless,
            // transparent and off the taskbar, sized to the rows each time it
            // opens; `tray::place` positions it once the page has measured
            // them. It takes focus so Escape and the arrow keys work, and
            // `Focused(false)` below is what dismisses it.
            WebviewWindowBuilder::new(app, tray::MENU_WINDOW, WebviewUrl::App("desktop-tray.html".into()))
                .title(concat!(product_name!(), " menu")).inner_size(260., 320.).decorations(false).resizable(false)
                .transparent(true).shadow(false).always_on_top(true).skip_taskbar(true)
                .focused(cfg!(windows)).visible(false).on_navigation(|url| security::document_url("tray-menu", url))
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny).build()?;
            preferences::position(app.handle(), &preferences);
            tray::install(app.handle())?;
            startup_log::mark(&root, "tray installed");
            if !probe {
            let mode_settings = tauri::async_runtime::block_on(async { metrix_writer_api::cleanup::modes::service().await?.settings().await });
            if let Ok(settings) = mode_settings { if let Err(error) = mode_shortcuts::initialize(app.handle(), &settings) {
                use tauri_plugin_dialog::DialogExt; app.dialog().message(error).title("Mode shortcuts").show(|_| {});
            } }

            #[cfg(windows)] modifier_hook::start(app.handle().clone(), &preferences)?;
            #[cfg(target_os = "macos")] macos_hook::start(app.handle().clone(), &preferences);
            for key in [preferences::recording_registration(&preferences)?, preferences::registered(&preferences.cancel)?].into_iter().flatten() {
                    if app.global_shortcut().register(key).is_err() {
                        use tauri_plugin_dialog::DialogExt; app.dialog().message("A recording shortcut is already in use. Change it in Settings; Record and Stop remain available.").title("Recording shortcuts").show(|_| {});
                    }
            }
            }
            startup_log::mark(&root, "setup complete");
            Ok(())
            })();
            #[cfg(windows)]
            { initialized }
            #[cfg(target_os = "macos")]
            {
                if initialized.is_err() {
                    // Tauri otherwise panics on setup errors after build().
                    // Keep these errors visible even without a terminal.
                    if probe {
                        let _ = storage::atomic_json(&root.join("probe.json"), &json!({"startup": false}));
                    } else {
                        startup_alert(concat!("Could not initialize ", product_name!(), ". Quit any other copy using this data folder, then reopen the installed app."));
                    }
                    app.handle().exit(2);
                }
                Ok(())
            }
        })
        .on_page_load(|window, event| {
            if window.label() == "main" && matches!(event.event(), tauri::webview::PageLoadEvent::Started) {
                if let Some(state) = window.app_handle().try_state::<Shell>() {
                    system::recording(window.app_handle(), false, false);
                    state.shortcut_capture.store(false, Ordering::Release);
                    #[cfg(windows)] modifier_hook::capturing(false);
                    #[cfg(target_os = "macos")] macos_hook::capturing(false);
                    let _ = state.capture.stop();
                    *state.snapshot.lock().unwrap() = None;
                    *state.pending.lock().unwrap() = None;
                    // A reloaded document starts a new owner with nothing in flight.
                    state.waiting.store(0, Ordering::Release);
                    let _ = window.app_handle().emit_to("overlay", "writer-waiting", json!({ "count": 0 }));
                    state.history_reload.store(false, Ordering::Release);
                }
            }
        })
        .on_window_event(|window, event| {
            // Losing focus is what closes a menu. A tray menu the user has
            // clicked away from must not stay in front of whatever they
            // clicked on instead.
            if window.label() == tray::MENU_WINDOW {
                if let tauri::WindowEvent::Focused(false) = event {
                    tray::close(window.app_handle());
                }
            }
            if window.label() == "overlay" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); }
                if let tauri::WindowEvent::Moved(position) = event {
                    if let Some(state) = window.app_handle().try_state::<Shell>() {
                        let mut p = state.preferences.lock().unwrap();
                        if p.placement == "remembered" { p.position = Some([position.x, position.y]); let _ = storage::atomic_json(&state.root.join("State/tauri-preferences.json"), &*p); }
                    }
                }
            }
            // Minimize is a normal taskbar operation. Only Close hides to the tray.
            if window.label() == "main" {
                match event {
                    tauri::WindowEvent::CloseRequested { api, .. } => { api.prevent_close(); let _ = window.hide(); }
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => window_place::noticed(window),
                    _ => {}
                }
            }
        })
        .build(tauri::generate_context!()).map_err(|_| "Could not initialize the desktop shell.".to_owned())?;
    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if matches!(event, tauri::RunEvent::Reopen { .. })
            && app.try_state::<Shell>().is_some_and(|state| !state.probe)
        {
            reveal(app);
        }
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
            request_quit(app);
        }
    });
    Ok(())
}
#[tauri::command]
fn probe_result(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Shell>,
    result: std::collections::BTreeMap<String, bool>,
) -> Result<()> {
    main_only(&window)?;
    if !state.probe || result.len() > 16 {
        return Err("Probe disabled.".into());
    }
    storage::atomic_json(&state.root.join("probe.json"), &result)?;
    app.exit(if result.values().all(|v| *v) { 0 } else { 1 });
    Ok(())
}

#[tauri::command]
fn dictation_probe_receipt(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    elapsed_ms: Option<f64>,
    visible_ms: Option<f64>,
) -> Result<Value> {
    main_only(&window)?;
    if !state.replay_probe {
        return Err("Replay probe disabled.".into());
    }
    if let Some(ms) = elapsed_ms.filter(|ms| ms.is_finite() && *ms >= 0.) {
        storage::atomic_json(
            &state.root.join("State/webview-latency.json"),
            &json!({"stop_command_to_receipt_ms":ms,"stop_to_visible_transcript_ms":visible_ms.filter(|v|v.is_finite() && *v>=0.),"scope":"real WebView, replay capture, managed model, durable mode and guarded delivery; transcript visibility measured with MutationObserver, receipt observation adds up to one poll interval"}),
        )?;
    }
    let bytes = std::fs::read(state.root.join("State/last-delivery.json"))
        .map_err(|_| "Delivery pending.")?;
    serde_json::from_slice(&bytes).map_err(|_| "Invalid receipt.".into())
}

#[tauri::command]
fn dictation_probe_command(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Shell>,
    action: String,
) -> Result<()> {
    main_only(&window)?;
    if !state.replay_probe || !matches!(action.as_str(), "start" | "stop") {
        return Err("Replay probe disabled.".into());
    }
    dispatch(&app, &action);
    Ok(())
}
