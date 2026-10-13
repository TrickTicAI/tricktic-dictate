use super::*;
use serde::Serialize;
/// What insertion asks about the world while it works. Each is re-read at the
/// points where it can change.
pub(super) struct Guards<'a> {
    /// False once the dictation has been discarded, or Writer is quitting.
    pub wanted: &'a dyn Fn() -> bool,
    /// True once a newer dictation has been started. That one may be in another
    /// window now, and focus is never pulled back out of it.
    pub superseded: &'a dyn Fn() -> bool,
    /// True while a newer dictation is being recorded, whose shortcut can be the
    /// keys that are held.
    pub recording_newer: &'a dyn Fn() -> bool,
    /// Tells the panel insertion is waiting for keys to be released.
    pub waiting: &'a dyn Fn(),
    /// MW-33: a Command-mode replacement must still be aimed at the selection
    /// it was captured from.
    pub target_valid: &'a dyn Fn() -> bool,
    /// Records a non-content latency stage for the delivery receipt.
    pub mark: &'a dyn Fn(&'static str),
}

use std::collections::{HashSet, VecDeque};

/// The window a dictation's text belongs in: whatever was in front when its
/// shortcut was pressed, and the process that owned it then. The process guards
/// against the handle being reused by another window before insertion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub window: isize,
    pub process: u32,
    #[cfg(windows)]
    pub created: u64,
}

/// One dictation recorded in this run of Writer.
struct Live {
    session: String,
    /// Starting order: a larger number is a newer dictation.
    sequence: u64,
    target: Option<Target>,
    /// The window in front when it stopped, unless that was Writer's own.
    stopped: Option<Target>,
    /// Audio was actually captured, as opposed to a start that was refused.
    captured: bool,
    /// Discarded by the user. Its text is never inserted.
    canceled: bool,
    /// Native input has been queued; clipboard housekeeping may still be running.
    pasted: bool,
}

/// Which dictations may still have their text inserted, and where.
///
/// Only a dictation recorded in this run qualifies, so text restored from
/// history is never pasted into another application. Several can be in flight
/// at once: the next dictation may start while the previous one is still being
/// transcribed or inserted, so this is kept per session rather than as the
/// latest target alone.
#[derive(Default)]
pub struct Delivery {
    sessions: VecDeque<Live>,
    next: u64,
    pub completed: HashSet<String>,
}
/// Far more dictations than can overlap; anything older is only history.
const RETAINED: usize = 32;
impl Delivery {
    fn find(&self, session: &str) -> Option<&Live> {
        self.sessions.iter().find(|live| live.session == session)
    }
    fn find_mut(&mut self, session: &str) -> Option<&mut Live> {
        self.sessions
            .iter_mut()
            .find(|live| live.session == session)
    }
    /// Records a dictation the shortcut, the panel or the tray has just started,
    /// with the window its text belongs in.
    pub fn start(&mut self, session: &str, target: Option<Target>) {
        if let Some(live) = self.find_mut(session) {
            live.target = target;
            return;
        }
        self.next += 1;
        self.sessions.push_back(Live {
            session: session.into(),
            sequence: self.next,
            target,
            stopped: None,
            captured: false,
            canceled: false,
            pasted: false,
        });
        while self.sessions.len() > RETAINED {
            self.sessions.pop_front();
        }
    }
    /// Marks a dictation as recording. One started from Writer's own window
    /// arrives here first, with no window to paste into.
    pub fn captured(&mut self, session: &str) {
        if self.find(session).is_none() {
            self.start(session, None);
        }
        if let Some(live) = self.find_mut(session) {
            live.captured = true;
        }
    }
    #[cfg(any(windows, target_os = "macos", test))]
    pub fn paste_sent(&mut self, session: &str) {
        if let Some(live) = self.find_mut(session) {
            live.pasted = true;
        }
    }
    pub fn was_pasted(&self, session: &str) -> bool {
        self.find(session).is_some_and(|live| live.pasted)
    }
    pub fn is_canceled(&self, session: &str) -> bool {
        self.find(session).is_some_and(|live| live.canceled)
    }
    pub fn canceled(&mut self, session: &str) {
        if let Some(live) = self.find_mut(session) {
            live.canceled = true;
        }
    }
    /// Drops a start the session owner refused, so it never counts as a newer
    /// dictation. A dictation that recorded anything is kept.
    pub fn forget(&mut self, session: &str) {
        self.sessions
            .retain(|live| live.session != session || live.captured);
    }
    pub fn target(&self, session: &str) -> Option<Target> {
        self.find(session).and_then(|live| live.target)
    }
    pub fn stopped(&mut self, session: &str, target: Target) {
        if let Some(live) = self.find_mut(session) {
            live.stopped = Some(target);
        }
    }
    /// Where this dictation's text is pasted: the window it stopped in when that
    /// is wanted and known, otherwise the window it started in.
    pub fn insertion_target(&self, session: &str, at_stop: bool) -> Option<Target> {
        self.find(session).and_then(|live| {
            if at_stop {
                live.stopped.or(live.target)
            } else {
                live.target
            }
        })
    }
    /// The dictation's place in the starting order, while its text may still be
    /// inserted.
    pub fn eligible(&self, session: &str) -> Option<u64> {
        self.find(session)
            .filter(|live| live.captured && !live.canceled)
            .map(|live| live.sequence)
    }
    #[cfg(any(windows, target_os = "macos", test))]
    pub fn wanted(&self, session: &str) -> bool {
        self.eligible(session).is_some()
    }
    /// Whether any dictation was started after the one at `sequence`.
    #[cfg(any(windows, target_os = "macos", test))]
    pub fn newer_than(&self, sequence: u64) -> bool {
        self.sessions.iter().any(|live| live.sequence > sequence)
    }
}
#[derive(Serialize)]
pub struct Outcome {
    pub(super) copied: bool,
    pub(super) pasted: bool,
    pub(super) message: String,
}
/// What the window is told about a delivery: the outcome, and the application
/// the text went to, by the name it gives itself, for "Pasted into Slack".
#[derive(Serialize)]
pub struct Delivered {
    #[serde(flatten)]
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    app: Option<String>,
}
/// The name an application gives itself. Read after the paste, so it never
/// delays one, and only ever for the window the text was meant for.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(unused_variables))]
fn application_name(target: Target) -> Option<String> {
    #[cfg(windows)]
    return super::win_target::application(target.process);
    #[cfg(target_os = "macos")]
    return super::macos_delivery::display_name(target);
    #[cfg(not(any(windows, target_os = "macos")))]
    None
}
/// The window in front right now, with its process. Nothing in the foreground
/// means no window to paste into later, and recording one would only produce a
/// misleading refusal.
fn foreground() -> Option<Target> {
    #[cfg(windows)]
    unsafe {
        let window = win_input::GetForegroundWindow();
        let mut process = 0;
        win_input::GetWindowThreadProcessId(window, &mut process);
        if window != 0 && process != 0 {
            return super::win_target::created(process).map(|created| Target {
                window,
                process,
                created,
            });
        }
    }
    #[cfg(target_os = "macos")]
    return super::macos_delivery::foreground();
    #[cfg(not(target_os = "macos"))]
    None
}
pub fn capture_target(state: &Shell, session: &str) {
    state.delivery.lock().unwrap().start(session, foreground());
}
/// Records the window in front as the dictation stops. Writer's own windows --
/// its main window and tray menu -- do not count: a stop made there leaves the
/// dictation with the window it started in.
pub fn capture_stop_target(state: &Shell, session: &str) {
    if let Some(target) = foreground().filter(|t| t.process != std::process::id()) {
        state.delivery.lock().unwrap().stopped(session, target);
    }
}
#[tauri::command]
pub async fn deliver_transcript(
    window: WebviewWindow,
    app: tauri::AppHandle,
    session_id: String,
    job_id: String,
    text: String,
) -> Result<Delivered> {
    main_only(&window)?;
    if text.trim().is_empty() || text.len() > 4 * 1024 * 1024 || text.contains('\0') {
        return Err("No usable transcript to copy.".into());
    }
    let state = app.state::<Shell>();
    let session = uuid::Uuid::parse_str(&session_id).map_err(|_| "Invalid completed session.")?;
    let job = uuid::Uuid::parse_str(&job_id).map_err(|_| "Invalid completed job.")?;
    let modes = metrix_writer_api::cleanup::modes::service()
        .await
        .map_err(|e| e.message)?;
    let processing = modes.live_processing(job).await.map_err(|e| e.message)?;
    let mut command = false;
    if processing.bound {
        let attempt = processing
            .attempt
            .ok_or("Mode processing has not completed. Original text is retained.")?;
        command = attempt.profile.kind == metrix_writer_api::cleanup::modes::ModeKind::Command;
        if !matches!(attempt.state, metrix_writer_api::cleanup::modes::AttemptState::Succeeded { text: ref processed } if processed == &text)
        {
            return Err("Mode processing did not produce this result. Copy the saved original or retry processing.".into());
        }
    }
    // The completed job itself says which dictation this text belongs to. The
    // dictation on screen is no longer the test: a newer one may have started
    // while this one was still being transcribed.
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Saved jobs unavailable. Copy the transcript from history.")?;
    let completed = pipeline
        .jobs
        .get(job)
        .await
        .map_err(|_| "The completed job is unavailable. Copy the transcript from history.")?;
    if completed.request.recording_id != session
        || !matches!(
            completed.status,
            metrix_writer_api::transcription::JobStatus::Succeeded { .. }
        )
    {
        return Err("This text does not belong to that dictation. Copy it from history.".into());
    }
    // Command mode replaces the selection it captured when it started, so its
    // text always goes back there; otherwise the preference decides.
    let at_stop = !command && state.preferences.lock().unwrap().paste_into == "stop";
    let (sequence, target) = {
        let mut delivery = state.delivery.lock().unwrap();
        let sequence = delivery.eligible(&session_id).ok_or(concat!(
            "Only a dictation recorded since ",
            product_name!(),
            " started is inserted automatically. Copy it from history."
        ))?;
        if !delivery.completed.insert(job_id.clone()) {
            return Ok(Delivered {
                outcome: Outcome {
                    copied: false,
                    pasted: false,
                    message: "Already delivered.".into(),
                },
                app: None,
            });
        }
        (sequence, delivery.insertion_target(&session_id, at_stop))
    };
    #[cfg(windows)]
    let owner = window.hwnd().map_err(|_| "Clipboard owner unavailable.")?.0 as isize;
    let notice_app = app.clone();
    let notice_session = session_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        #[cfg(any(windows, target_os = "macos"))]
        {
            #[cfg(windows)]
            use super::windows_delivery::windows_deliver as native_deliver;
            #[cfg(target_os = "macos")]
            use super::macos_delivery::macos_deliver as native_deliver;
            #[cfg(target_os = "macos")]
            let owner = 0;
            let shell = app.state::<Shell>();
            metrix_writer_api::latency::mark(session, "delivery_started");
            let (preserve_clipboard, private_clipboard) = {
                let preferences = shell.preferences.lock().unwrap();
                (preferences.preserve_clipboard, preferences.private_clipboard)
            };
            let result = native_deliver(
                owner,
                target,
                &text,
                preserve_clipboard,
                private_clipboard,
                &Guards {
                    wanted: &|| {
                        !shell.quitting.load(Ordering::Acquire)
                            && shell.delivery.lock().unwrap().wanted(&session_id)
                    },
                    superseded: &|| shell.delivery.lock().unwrap().newer_than(sequence),
                    recording_newer: &|| {
                        shell.snapshot.lock().unwrap().as_ref().is_some_and(|s| {
                            s["session_id"] != session_id.as_str()
                                && matches!(s["state"].as_str(), Some("requesting" | "capturing"))
                        })
                    },
                    waiting: &|| {
                        super::notice(
                            &app,
                            Some(&session_id),
                            "Release keys",
                            "Release the shortcut keys to insert the text.",
                            0,
                        )
                    },
                    target_valid: &|| !command || shell.context.valid(&session_id),
                    mark: &|stage| {
                        metrix_writer_api::latency::mark(session, stage);
                        if stage == "paste_sent" {
                            shell.delivery.lock().unwrap().paste_sent(&session_id);
                            super::pasted(&app, &session_id);
                            // Read the current session on the UI thread. An older
                            // delivery must never hide a newer recording's panel.
                            let handle = app.clone();
                            let _ = app.run_on_main_thread(move || {
                                let state = handle.state::<Shell>();
                                let snapshot = state.snapshot.lock().unwrap().clone();
                                if let (Some(snapshot), Some(overlay)) =
                                    (snapshot, handle.get_webview_window("overlay"))
                                {
                                    overlay_visibility(&overlay, panel_shown(&state, &snapshot));
                                }
                            });
                        }
                    },
                },
            );
            // Guarded insertion has finished; its selected target is no longer
            // needed. Never clear a newer dictation's retained selection.
            shell.context.forget_later(session_id.clone());
            // Keep only the latest non-content delivery outcome for native diagnosis.
            // No transcript, clipboard contents, window title or app path is recorded.
            let mut receipt = match &result {
                Ok(outcome) => serde_json::to_value(outcome).unwrap_or_default(),
                Err(message) => json!({"copied": false, "pasted": false, "message": message}),
            };
            let stopped = shell.stopped_at.lock().unwrap().get(session);
            if let Some(at) = stopped {
                metrix_writer_api::latency::mark(session, "delivery_finished");
                receipt["stop_to_delivery_ms"] = json!(at.elapsed().as_millis());
                receipt["stages"] = metrix_writer_api::latency::snapshot(session);
                metrix_writer_api::latency::measurements::flush(session);
            }
            let _ = storage::atomic_json(&shell.root.join("State/last-delivery.json"), &receipt);
            // A paste that did not happen is said on the panel as well. Writer's
            // own window, the only other place it shows, is usually hidden.
            if let Ok(outcome) = &result {
                if outcome.copied && !outcome.pasted {
                    super::notice(&app, Some(&session_id), "Copied", &outcome.message, 2600);
                }
            }
            result
        }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (target, text, command, sequence); Ok(Outcome {copied: false, pasted: false, message: "Automatic text delivery is currently available on Windows. Copy the transcript from history.".into()}) }

    }).await.map_err(|_| "Text delivery unavailable. Copy the saved transcript from history.".to_owned())?;
    let application = match target {
        Some(target) => tauri::async_runtime::spawn_blocking(move || application_name(target))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    if let Ok(outcome) = &result {
        if let Ok(service) = metrix_writer_api::cleanup::modes::service().await {
            let stop_to_delivery_ms = notice_app
                .state::<Shell>()
                .stopped_at
                .lock()
                .unwrap()
                .get(session)
                .map(|at| at.elapsed().as_millis() as u64);
            let _ = service
                .record_delivery(
                    job,
                    outcome.copied,
                    outcome.pasted,
                    stop_to_delivery_ms,
                    application.clone(),
                )
                .await;
            if (outcome.copied || outcome.pasted)
                && service
                    .prepare_delivery(job)
                    .await
                    .is_ok_and(|p| p.original_used)
            {
                super::notice(
                    &notice_app,
                    Some(&notice_session),
                    if outcome.pasted {
                        "Original inserted"
                    } else {
                        "Original copied"
                    },
                    "Polishing unavailable. Your original text and audio are saved.",
                    2600,
                );
            }
        }
    }
    result.map(|outcome| Delivered {
        outcome,
        app: application,
    })
}
#[cfg(windows)]
pub fn clipboard(owner: isize, text: &str) -> Result<()> {
    super::windows_delivery::clipboard(owner, text)
}

#[cfg(target_os = "macos")]
pub fn clipboard(_owner: isize, text: &str) -> Result<()> {
    super::macos_delivery::clipboard(text).map(|_| ())
}

#[tauri::command]
pub(super) async fn prepare_delivery(
    window: WebviewWindow,
    job_id: uuid::Uuid,
) -> Result<metrix_writer_api::cleanup::modes::PreparedDelivery> {
    main_only(&window)?;
    let service = metrix_writer_api::cleanup::modes::service()
        .await
        .map_err(|e| e.message)?;
    tokio::time::timeout(Duration::from_secs(930), service.prepare_delivery(job_id))
        .await
        .map_err(|_| {
            "Mode processing did not finish. The original transcript is saved.".to_owned()
        })?
        .map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_dictation_recorded_in_this_run_and_not_discarded_is_inserted() {
        let mut delivery = Delivery::default();
        // Started from the shortcut: its window is known before it records.
        let shortcut = Target {
            window: 7,
            process: 9,
            #[cfg(windows)]
            created: 1,
        };
        delivery.start("a", Some(shortcut));
        assert_eq!(delivery.eligible("a"), None);
        delivery.captured("a");
        let a = delivery.eligible("a").unwrap();
        assert_eq!(delivery.target("a"), Some(shortcut));
        // Started from Writer's own window: recording, nowhere to paste into.
        delivery.captured("b");
        let b = delivery.eligible("b").unwrap();
        assert!(b > a);
        assert_eq!(delivery.target("b"), None);
        assert!(delivery.newer_than(a));
        assert!(!delivery.newer_than(b));
        // A discarded dictation keeps its place but is never inserted.
        delivery.canceled("a");
        assert!(!delivery.wanted("a"));
        assert!(delivery.wanted("b"));
        // Text restored from history was never recorded in this run.
        assert_eq!(delivery.eligible("from-history"), None);
    }

    #[test]
    fn the_text_goes_where_the_dictation_stopped_unless_the_start_window_is_chosen() {
        let mut delivery = Delivery::default();
        let started = Target {
            window: 7,
            process: 9,
            #[cfg(windows)]
            created: 1,
        };
        let stopped = Target {
            window: 11,
            process: 13,
            #[cfg(windows)]
            created: 2,
        };
        delivery.start("a", Some(started));
        delivery.captured("a");
        // Until a stop is recorded, both choices mean the start window.
        assert_eq!(delivery.insertion_target("a", true), Some(started));
        delivery.stopped("a", stopped);
        assert_eq!(delivery.insertion_target("a", true), Some(stopped));
        assert_eq!(delivery.insertion_target("a", false), Some(started));
        // The selection context is still read from where it started.
        assert_eq!(delivery.target("a"), Some(started));
        // Started from Writer's own window: no start target, but a stop target.
        delivery.captured("b");
        delivery.stopped("b", stopped);
        assert_eq!(delivery.insertion_target("b", true), Some(stopped));
        assert_eq!(delivery.insertion_target("b", false), None);
    }

    #[test]
    fn a_refused_start_is_forgotten_and_never_counts_as_newer() {
        let mut delivery = Delivery::default();
        delivery.captured("spoken");
        let spoken = delivery.eligible("spoken").unwrap();
        delivery.start("refused", None);
        assert!(delivery.newer_than(spoken));
        delivery.forget("refused");
        assert!(!delivery.newer_than(spoken));
        // A dictation that recorded anything is not dropped by a refusal.
        delivery.forget("spoken");
        assert!(delivery.wanted("spoken"));
    }

    #[test]
    fn paste_completion_is_scoped_to_its_own_live_session() {
        let mut delivery = Delivery::default();
        delivery.captured("earlier");
        delivery.captured("newer");
        delivery.paste_sent("earlier");
        assert!(delivery.was_pasted("earlier"));
        assert!(!delivery.was_pasted("newer"));
        assert!(
            delivery.wanted("earlier"),
            "clipboard cleanup still has its generation guards"
        );
        delivery.paste_sent("unknown");
        assert!(!delivery.was_pasted("unknown"));
    }

    #[test]
    fn retention_is_bounded_and_drops_the_oldest() {
        let mut delivery = Delivery::default();
        for index in 0..RETAINED + 8 {
            delivery.captured(&index.to_string());
        }
        assert_eq!(delivery.sessions.len(), RETAINED);
        assert!(!delivery.wanted("0"));
        assert!(delivery.wanted(&(RETAINED + 7).to_string()));
    }
}
