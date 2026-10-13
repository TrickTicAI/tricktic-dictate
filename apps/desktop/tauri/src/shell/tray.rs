//! The tray icon and the menu behind its right click.
//!
//! The menu is rebuilt from live shell state moments before it is drawn, so it
//! never offers a microphone that has been unplugged, a language the selected
//! model cannot transcribe, a model whose weights are not installed, or a
//! transcript that was never produced. Rows that exist only to report state are
//! disabled, so nothing clickable on this menu can be a claim the rest of the
//! shell would not honour.
//!
//! Writer draws the menu itself, in a small borderless window carrying the
//! application's own theme, rather than handing a `muda` menu to the operating
//! system to draw. A system-drawn menu cannot be themed: it stays in the
//! platform's own colours and shape whatever the rest of the application looks
//! like, and on Windows that is a flat grey box in front of a themed
//! application.
//!
//! Only the drawing changed. `plan` still turns `Facts` into rows, `command`
//! still parses a row id back into an intent, and `chose` still performs it —
//! so what the menu offers, refuses and reports is decided in exactly the same
//! place it was before, and is still covered by the tests at the bottom of this
//! file.
use super::*;
use crate::tray_mark::{self, Look};
use metrix_writer_api::{
    cleanup::modes::AttemptState,
    desktop_settings::{self, credentials::CredentialStatus},
    language,
    local_stt::managed::WeightsState,
    transcription::{InputLanguage, JobStatus, ModelSelection, ProviderId, TranscriptionOptions},
};
use std::sync::Arc;
use tauri::{
    image::Image,
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

/// The label of the window the menu is drawn in.
pub const MENU_WINDOW: &str = "tray-menu";

const TOOLTIP: &str = product_name!();
const RECORDING_TOOLTIP: &str = concat!(product_name!(), " — recording");
const WORKING_TOOLTIP: &str = concat!(product_name!(), " — transcribing");
const ATTENTION_TOOLTIP: &str = concat!(product_name!(), " — the last dictation failed");
/// Longest transcript preview drawn under "Last dictation": long enough to
/// recognise the dictation, short enough that the menu keeps its shape.
const PREVIEW: usize = 52;
/// The menu is also refreshed when it is about to open, so this only bounds how
/// stale it can be if that press is ever missed.
const REFRESH: Duration = Duration::from_secs(2);
/// Copy entries use the same native clipboard path as automatic delivery.
const CLIPBOARD: bool = cfg!(any(windows, target_os = "macos"));

pub struct Tray {
    icon: TrayIcon,
    /// What the icon shows and at what pixel size, so a state change that
    /// leaves both alone redraws nothing.
    look: Mutex<Option<(Look, u32)>>,
    /// Signalled by anything that may have changed what the icon should say.
    /// One task reads the shell's state and redraws, so updates never arrive
    /// out of order and never wait on a lock the caller holds.
    dirty: Arc<tokio::sync::Notify>,
    /// When the microphone opened, in milliseconds since the epoch, so the
    /// menu's status row can count the recording up while it is open.
    since: Mutex<Option<u64>>,
    /// Signalled when the menu is about to open, so the rows are gathered from
    /// live state before the window is shown.
    wake: Arc<tokio::sync::Notify>,
    /// What the published menu was built from. `chose` reads the transcript
    /// back out of this, so a copy can only ever deliver text the menu the user
    /// was reading actually offered.
    shown: Mutex<Option<Facts>>,
    /// Where the menu window pins itself, in physical pixels, together with the
    /// work area it must stay inside. Set when the menu opens and read again
    /// every time the page reports a new content size, so a submenu that grows
    /// the panel still opens away from the screen edge.
    anchor: Mutex<Option<Anchor>>,
    /// Set by the right click that asks for the menu, and cleared by the
    /// refresh that opens it. The same signal also wakes the periodic refresh,
    /// which must warm the caches without popping a window open.
    pending_open: AtomicBool,
    #[cfg(target_os = "macos")]
    suppress_click: AtomicBool,
    /// The rows most recently published. Kept so a menu window that loaded a
    /// moment too late, or reloaded, can ask for them instead of showing an
    /// empty panel until the next right click.
    menu: Mutex<Option<Value>>,
    /// Whether a right click is currently asking for the menu. A page that
    /// reloads while hidden re-reads the last rows and measures them again;
    /// without this the measurement would pop a menu nobody asked for.
    opened: AtomicBool,
    /// One history scan recovers the last transcript from previous runs; after
    /// that the running session reports its own completions.
    scanned: AtomicBool,
}

/// The point the menu hangs from, and the screen it must stay on.
#[derive(Clone, Copy)]
struct Anchor {
    x: i32,
    y: i32,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    scale: f64,
}

/// Everything the menu draws, in the order it is drawn.
#[derive(Clone, Default, PartialEq)]
struct Facts {
    /// What the dictation is doing, as the status row says it.
    status: Status,
    /// The failed dictation's audio was saved, so it can be retried.
    audio_saved: bool,
    /// When the microphone opened, in milliseconds since the epoch.
    since_ms: Option<u64>,
    /// A recording is running, so the first entry stops it.
    dictating: bool,
    /// A session is in flight and can still be canceled.
    cancelable: bool,
    /// The Writer document has published a session. Without one the shell has
    /// nothing to address a start command to.
    ready: bool,
    recording_chord: String,
    cancel_chord: String,
    overlay: bool,
    sounds: bool,
    duck_audio: bool,
    launch_at_login: bool,
    microphones: Vec<Device>,
    microphone_problem: Option<String>,
    /// Selecting a microphone is the recorder's own refusal while it streams.
    microphones_locked: bool,
    languages: Vec<Choice>,
    language_note: String,
    models: Vec<Choice>,
    keys: Vec<Key>,
    modes: Vec<Mode>,
    last: Option<Last>,
    /// The interface language mirrors the layout, and the menu is part of the
    /// interface. Read from the same stored setting the window uses.
    rtl: bool,
}

/// What the dictation is doing. One answer, read in one place, for both the
/// menu's status row and the icon, so the two never disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Status {
    /// The Writer document has not published a session yet.
    #[default]
    Starting,
    Ready,
    Opening,
    Recording,
    Working,
    /// The last dictation failed, and its report on the panel was not
    /// dismissed.
    Failed,
}

impl Status {
    fn look(self) -> Look {
        match self {
            Self::Opening | Self::Recording => Look::Recording,
            Self::Working => Look::Working,
            Self::Failed => Look::Attention,
            Self::Starting | Self::Ready => Look::Idle,
        }
    }
}

/// A row that selects one value out of a set: microphone, language or model.
#[derive(Clone, PartialEq)]
struct Choice {
    /// Menu id suffix, and the value the action parses back out of it.
    id: String,
    label: String,
    /// The selected value as the root row and the status row name it, where
    /// the submenu's own label would repeat what the row already says.
    value: String,
    /// Why the row cannot be chosen, drawn in the shortcut column.
    detail: String,
    selected: bool,
    enabled: bool,
}
#[derive(Clone, PartialEq)]
struct Mode {
    id: String,
    name: String,
    /// Its own shortcut, drawn as keycaps beside it.
    shortcut: String,
    active: bool,
    /// A polishing mode needs a ready text model, and the window will not
    /// switch to one without it.
    usable: bool,
}
#[derive(Clone, PartialEq)]
struct Device {
    /// `None` is the operating system default input.
    id: Option<String>,
    label: String,
    selected: bool,
}
#[derive(Clone, PartialEq)]
struct Key {
    provider: String,
    label: String,
    present: bool,
}
#[derive(Clone, PartialEq)]
struct Last {
    job: String,
    session: String,
    /// The exact recognised text.
    raw: String,
    /// What the dictation pasted: the mode's polished text when that
    /// succeeded, otherwise the transcript with the dictionary applied.
    pasted: String,
    /// When it was dictated, in milliseconds since the epoch.
    at_ms: u64,
}

/// Builds the icon and its first menu, then keeps that menu current.
pub fn install(app: &tauri::AppHandle) -> Result<()> {
    let size = pixels(app);
    let (first, template) = frame(Look::Idle, size);
    let wake = Arc::new(tokio::sync::Notify::new());
    let signal = wake.clone();
    let handle = app.clone();
    let icon = TrayIconBuilder::new()
        .tooltip(TOOLTIP)
        .icon(first)
        .icon_as_template(template)
        // No `menu`: the operating system has nothing to draw here. Both
        // buttons are handled below.
        .show_menu_on_left_click(false)
        .on_tray_icon_event(move |_tray, event| {
            let TrayIconEvent::Click {
                button,
                button_state,
                position: _position,
                rect: _rect,
                ..
            } = event
            else {
                return;
            };
            #[cfg(target_os = "macos")]
            if matches!(button, MouseButton::Left | MouseButton::Right) {
                let state = handle.state::<Tray>();
                if button_state == MouseButtonState::Down {
                    // Closing on press also suppresses its release, even when
                    // the blur handler runs between the two events.
                    let was_open = state.opened.load(Ordering::Acquire);
                    state.suppress_click.store(was_open, Ordering::Release);
                    if was_open {
                        close(&handle);
                    }
                } else if !state.suppress_click.swap(false, Ordering::AcqRel) {
                    // The menu-bar item's rect, not the drifting pointer, is
                    // the anchor. Tauri reports physical screen pixels here.
                    let origin = _rect.position.to_physical::<f64>(1.);
                    let size = _rect.size.to_physical::<f64>(1.);
                    pin(&handle, origin.x, origin.y + size.height);
                    signal.notify_one();
                }
                return;
            }
            match (button, button_state) {
                // Pinned on press and opened on release, so the menu appears
                // where the click landed rather than where the pointer drifted
                // to while the rows were being gathered.
                #[cfg(windows)]
                (MouseButton::Right, MouseButtonState::Down) => {
                    pin(&handle, _position.x, _position.y);
                    signal.notify_one();
                }
                #[cfg(windows)]
                (MouseButton::Left, MouseButtonState::Up) => reveal(_tray.app_handle()),
                _ => {}
            }
        })
        .build(app)
        .map_err(|_| "Tray icon unavailable.")?;
    app.manage(Tray {
        icon,
        look: Mutex::new(Some((Look::Idle, size))),
        dirty: Arc::new(tokio::sync::Notify::new()),
        since: Mutex::new(None),
        wake,
        shown: Mutex::new(None),
        anchor: Mutex::new(None),
        pending_open: AtomicBool::new(false),
        #[cfg(target_os = "macos")]
        suppress_click: AtomicBool::new(false),
        menu: Mutex::new(None),
        opened: AtomicBool::new(false),
        scanned: AtomicBool::new(false),
    });
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let Some(wake) = handle.try_state::<Tray>().map(|tray| tray.wake.clone()) else {
                return;
            };
            let signalled = tokio::select! {
                _ = wake.notified() => true,
                _ = tokio::time::sleep(REFRESH) => false,
            };
            if handle.state::<Shell>().quitting.load(Ordering::Acquire) {
                return;
            }
            let opening = signalled
                && handle
                    .state::<Tray>()
                    .pending_open
                    .swap(false, Ordering::AcqRel);
            refresh(&handle, opening).await;
            // The same tick re-reads the icon's state, which also catches a
            // change of display scale.
            sync(&handle);
        }
    });
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let Some(dirty) = handle.try_state::<Tray>().map(|tray| tray.dirty.clone()) else {
                return;
            };
            dirty.notified().await;
            if handle.state::<Shell>().quitting.load(Ordering::Acquire) {
                return;
            }
            let look = status(&handle).0.look();
            let app = handle.clone();
            let _ = handle.run_on_main_thread(move || show(&app, look));
        }
    });
    // Warms the caches so the first right click has devices and models to draw
    // without waiting for them. It never opens the menu: only `pin` does that.
    app.state::<Tray>().wake.notify_one();
    Ok(())
}

/// Remembers where the menu was asked for, and on which screen, before the rows
/// are gathered. Physical pixels throughout; the page is told the scale.
fn pin(app: &tauri::AppHandle, x: f64, y: f64) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let monitor = app
        .monitor_from_point(x, y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let anchor = monitor.map(|monitor| {
        let area = monitor.work_area();
        Anchor {
            x: x.round() as i32,
            y: y.round() as i32,
            left: area.position.x,
            top: area.position.y,
            right: area.position.x + area.size.width as i32,
            bottom: area.position.y + area.size.height as i32,
            scale: monitor.scale_factor(),
        }
    });
    *tray.anchor.lock().unwrap() = anchor;
    tray.pending_open.store(true, Ordering::Release);
}

/// The microphone opened or closed. The icon says so straight away, and the
/// menu's status row counts the recording from this moment.
pub fn recording(app: &tauri::AppHandle, active: bool) {
    if let Some(tray) = app.try_state::<Tray>() {
        let mut since = tray.since.lock().unwrap();
        if !active {
            *since = None;
        } else if since.is_none() {
            *since = Some(now_ms());
        }
    }
    sync(app);
}

/// Something the icon reports may have changed. Takes no lock and never
/// waits, so it is safe from inside any state change, including one that
/// holds the session snapshot.
pub fn sync(app: &tauri::AppHandle) {
    if let Some(tray) = app.try_state::<Tray>() {
        tray.dirty.notify_one();
    }
}

/// What the dictation is doing, and whether a failed one kept its audio, read
/// from the shell's own state. Each lock is taken and released on its own, so
/// a caller must hold none of them.
fn status(app: &tauri::AppHandle) -> (Status, bool) {
    let Some(state) = app.try_state::<Shell>() else {
        return (Status::Starting, false);
    };
    let capturing = state.capture.status().is_ok_and(|status| status.active);
    let pending = state.pending.lock().unwrap().is_some();
    let Some(snapshot) = state.snapshot.lock().unwrap().clone() else {
        return (
            if capturing {
                Status::Recording
            } else {
                Status::Starting
            },
            false,
        );
    };
    let session = snapshot["session_id"].clone();
    let status = match snapshot["state"].as_str() {
        _ if capturing => Status::Recording,
        Some("capturing") => Status::Recording,
        Some("requesting") => Status::Opening,
        _ if pending => Status::Opening,
        Some("saving" | "queued") => Status::Working,
        // Pasted while it is still being polished is finished as far as anyone
        // at the keyboard can tell, which is also when the panel goes.
        Some("transcribing") => {
            let pasted = session
                .as_str()
                .is_some_and(|id| state.delivery.lock().unwrap().was_pasted(id));
            if pasted {
                Status::Ready
            } else {
                Status::Working
            }
        }
        Some("error") if state.dismissed.lock().unwrap().as_ref() != Some(&session) => {
            Status::Failed
        }
        _ => Status::Ready,
    };
    (status, snapshot["audio_saved"] == true)
}

/// Draws `look` at the size the notification area or the menu bar wants now,
/// and says it in the tooltip. Runs on the main thread, where the icon lives.
fn show(app: &tauri::AppHandle, look: Look) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let size = pixels(app);
    {
        let mut shown = tray.look.lock().unwrap();
        if *shown == Some((look, size)) {
            return;
        }
        *shown = Some((look, size));
    }
    let (image, template) = frame(look, size);
    let _ = tray.icon.set_icon_with_as_template(Some(image), template);
    let _ = tray.icon.set_tooltip(Some(tooltip(look)));
}

fn tooltip(look: Look) -> &'static str {
    match look {
        Look::Idle => TOOLTIP,
        Look::Recording => RECORDING_TOOLTIP,
        Look::Working => WORKING_TOOLTIP,
        Look::Attention => ATTENTION_TOOLTIP,
    }
}

/// The icon for `look` at `size` pixels, and whether macOS should tint it to
/// suit the menu bar. A badge has to keep its colour, so badged looks never are.
fn frame(look: Look, size: u32) -> (Image<'static>, bool) {
    let template = cfg!(target_os = "macos") && !tray_mark::coloured(look);
    (
        Image::new_owned(tray_mark::draw(size, look, template), size, size),
        template,
    )
}

/// The icon's size in physical pixels: the notification area's 16 px, or the
/// menu bar's 18 pt, at the primary display's scale. Drawn for exactly that
/// size, so Windows never shrinks one bitmap into a blur.
fn pixels(app: &tauri::AppHandle) -> u32 {
    let scale = app
        .primary_monitor()
        .ok()
        .flatten()
        .map_or(1.0, |monitor| monitor.scale_factor());
    let points = if cfg!(target_os = "macos") {
        18.0
    } else {
        16.0
    };
    (points * scale).round().clamp(16.0, 64.0) as u32
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Reads current state, records what the menu is built from, and opens the menu
/// window when a right click asked for one. `opening` is also the only time the
/// audio devices are worth enumerating again.
async fn refresh(app: &tauri::AppHandle, opening: bool) {
    let previous = match app.try_state::<Tray>() {
        Some(tray) => tray.shown.lock().unwrap().clone(),
        None => return,
    };
    let facts = gather(app, previous.clone().unwrap_or_default(), opening).await;
    let changed = previous.as_ref() != Some(&facts);
    if !changed && !opening {
        return;
    }
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    if changed {
        *tray.shown.lock().unwrap() = Some(facts.clone());
    }
    if opening {
        open(app, &facts);
    }
}

/// Publishes the rows to the menu window and shows it at the pinned point.
///
/// The window is positioned once the page reports how big the rows actually
/// are, so the first paint happens off screen rather than as a flash at the
/// wrong size. Without a window there is nothing to draw the menu in, so the
/// click falls back to opening Writer rather than doing nothing at all.
fn open(app: &tauri::AppHandle, facts: &Facts) {
    let (Some(window), Some(tray)) = (app.get_webview_window(MENU_WINDOW), app.try_state::<Tray>())
    else {
        reveal(app);
        return;
    };
    let anchor = *tray.anchor.lock().unwrap();
    let scale = anchor.map_or(1.0, |anchor| anchor.scale);
    // The screen the menu opens on bounds it: a long model list scrolls inside
    // the panel rather than running off the display.
    let (max_width, max_height) = anchor.map_or((f64::from(u16::MAX), f64::from(u16::MAX)), |a| {
        (
            f64::from(a.right - a.left) / scale,
            f64::from(a.bottom - a.top) / scale,
        )
    });
    // The panel is painted from the application's own resolved theme, which is
    // the whole reason the menu is drawn here rather than by the platform. A
    // shell that has not been told a theme yet — the main window has not
    // finished its first paint — leaves this null and the page uses its own
    // neutral fallback rather than guessing at colours.
    let theme = app
        .try_state::<Shell>()
        .and_then(|state| state.theme.lock().unwrap().clone());
    // The app's one Animations setting covers this menu too.
    let reduced_motion = app
        .try_state::<Shell>()
        .is_some_and(|state| state.preferences.lock().unwrap().reduced_motion);
    let payload = json!({
        "rows": rows(&plan(facts)),
        "scale": scale,
        "max_width": max_width,
        "max_height": max_height,
        "theme": theme,
        "direction": if facts.rtl { "rtl" } else { "ltr" },
        // Shortcuts are drawn in the platform's own notation: ⌃⇧ on a Mac.
        "platform": if cfg!(target_os = "macos") { "macos" } else { "windows" },
        "reduced_motion": reduced_motion,
    });
    *tray.menu.lock().unwrap() = Some(payload.clone());
    tray.opened.store(true, Ordering::Release);
    if window.emit("writer-tray-menu", payload).is_err() {
        tray.opened.store(false, Ordering::Release);
        reveal(app);
    }
}

/// Places the menu window for a content size the page just measured, then shows
/// it. Called for the first paint and again whenever a submenu changes the
/// size, so the panel keeps its pinned corner and stays on one screen.
fn place(app: &tauri::AppHandle, width: f64, height: f64) -> Result<()> {
    let window = app
        .get_webview_window(MENU_WINDOW)
        .ok_or("Tray menu window unavailable.")?;
    let anchor = app
        .try_state::<Tray>()
        .and_then(|tray| *tray.anchor.lock().unwrap())
        .ok_or("The tray menu has no anchor.")?;

    let w = (width * anchor.scale).round().max(1.0) as i32;
    let h = (height * anchor.scale).round().max(1.0) as i32;
    // A tray sits at a screen edge, so the menu opens back towards the middle
    // whenever it would otherwise run off: down-right by default, up and left
    // near the corner the Windows taskbar normally occupies.
    let mut x = anchor.x;
    let mut y = anchor.y;
    if x + w > anchor.right {
        x = anchor.x - w;
    }
    if y + h > anchor.bottom {
        y = anchor.y - h;
    }
    x = x.clamp(anchor.left, (anchor.right - w).max(anchor.left));
    y = y.clamp(anchor.top, (anchor.bottom - h).max(anchor.top));

    window
        .set_size(tauri::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32))
        .map_err(|_| "The tray menu could not be sized.")?;
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|_| "The tray menu could not be placed.")?;
    // Only a right click opens the menu. A reload measures the rows it still
    // remembers, and that must not put a menu on screen by itself.
    if app
        .try_state::<Tray>()
        .is_some_and(|tray| tray.opened.load(Ordering::Acquire))
    {
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

/// Hides the menu. Safe to call when it is already hidden, which is what the
/// blur handler, the Escape key and every activated row all do.
pub fn close(app: &tauri::AppHandle) {
    if let Some(tray) = app.try_state::<Tray>() {
        tray.opened.store(false, Ordering::Release);
    }
    if let Some(window) = app.get_webview_window(MENU_WINDOW) {
        let _ = window.hide();
    }
}

async fn gather(app: &tauri::AppHandle, previous: Facts, opening: bool) -> Facts {
    let state = app.state::<Shell>();
    let preferences = state.preferences.lock().unwrap().clone();
    let system = {
        #[cfg_attr(windows, allow(unused_mut))]
        let mut settings = state.system.lock().unwrap();
        #[cfg(target_os = "macos")]
        settings.refresh();
        settings.clone()
    };
    let snapshot = state.snapshot.lock().unwrap().clone();
    let phase = snapshot
        .as_ref()
        .and_then(|s| s["state"].as_str())
        .unwrap_or("idle")
        .to_owned();
    let dictating = state.pending.lock().unwrap().is_some()
        || matches!(phase.as_str(), "requesting" | "capturing");
    let capturing = state.capture.status().is_ok_and(|status| status.active);

    // Devices are enumerated when someone is about to read the list, not on
    // every tick. While the recorder streams there is nothing to re-read: it
    // refuses a change then, and the previous list is what the menu shows.
    let stale = capturing
        || !(opening || previous.microphones.is_empty() && previous.microphone_problem.is_none());
    let (microphones, microphone_problem) = if stale {
        (previous.microphones, previous.microphone_problem)
    } else {
        let store = state.capture.store.clone();
        match tauri::async_runtime::spawn_blocking(move || crate::microphones::list(&store)).await {
            Ok(Ok(list)) => (devices(&list), None),
            Ok(Err(problem)) => (Vec::new(), Some(problem)),
            Err(_) => (Vec::new(), Some("Microphone list unavailable.".into())),
        }
    };

    // The model catalogue costs a credential read per provider, and like the
    // device list it is only ever read from the menu.
    let catalogue = opening || previous.models.is_empty();
    let modes = if catalogue {
        read_modes().await.unwrap_or(previous.modes)
    } else {
        previous.modes
    };
    let (languages, language_note, models, keys, rtl) = if catalogue {
        match desktop_settings::global() {
            Ok(setup) => tauri::async_runtime::spawn_blocking(move || {
                let rtl = name(&setup.settings().ui_language) == "he";
                let (languages, note, models, keys) = catalog(&setup);
                (languages, note, models, keys, rtl)
            })
            .await
            .unwrap_or_default(),
            Err(_) => Default::default(),
        }
    } else {
        (
            previous.languages,
            previous.language_note,
            previous.models,
            previous.keys,
            previous.rtl,
        )
    };

    let (status, audio_saved) = status(app);
    Facts {
        status,
        audio_saved,
        since_ms: app
            .try_state::<Tray>()
            .and_then(|tray| *tray.since.lock().unwrap()),
        dictating,
        cancelable: dictating || matches!(phase.as_str(), "saving" | "queued" | "transcribing"),
        ready: snapshot.is_some(),
        recording_chord: preferences.recording,
        cancel_chord: preferences.cancel,
        overlay: preferences.visibility != "off",
        sounds: system.sounds,
        duck_audio: system.duck_audio,
        launch_at_login: system.launch_at_login,
        microphones,
        microphone_problem,
        microphones_locked: capturing,
        languages,
        language_note,
        models,
        keys,
        modes,
        last: last(app, previous.last, snapshot.as_ref()).await,
        rtl,
    }
}

/// The modes, with the active one marked and each one's usability judged by
/// the rule the window's mode menu uses.
async fn read_modes() -> Option<Vec<Mode>> {
    let settings = metrix_writer_api::cleanup::modes::service()
        .await
        .ok()?
        .settings()
        .await
        .ok()?;
    let profiles = settings.profiles.clone();
    // Discovery reads credentials, so it stays off the async workers.
    let usable = tauri::async_runtime::spawn_blocking(move || {
        metrix_writer_api::cleanup::modes::usable(&profiles)
    })
    .await
    .ok()?;
    Some(
        settings
            .profiles
            .iter()
            .zip(usable)
            .map(|(profile, usable)| Mode {
                id: profile.id.clone(),
                name: profile.name.clone(),
                shortcut: profile.shortcut.clone().unwrap_or_default(),
                active: profile.id == settings.active,
                usable,
            })
            .collect(),
    )
}

fn devices(list: &crate::microphones::MicrophoneList) -> Vec<Device> {
    let mut devices = vec![Device {
        id: None,
        label: "System default".into(),
        selected: list.selected_id.is_none(),
    }];
    devices.extend(list.devices.iter().map(|device| Device {
        id: Some(device.id.clone()),
        label: device.label.clone(),
        selected: list.selected_id.as_deref() == Some(device.id.as_str()),
    }));
    // A saved microphone that is not present is still the selection. Saying so
    // is the difference between "unplugged" and "silently switched".
    if let Some(missing) = list
        .selected_id
        .as_ref()
        .filter(|id| !list.devices.iter().any(|device| device.id == **id))
    {
        devices.push(Device {
            id: Some(missing.clone()),
            label: format!("{missing} (unavailable)"),
            selected: true,
        });
    }
    devices
}

/// Languages, models and credentials, read from the same setup the
/// transcription pipeline itself uses.
fn catalog(setup: &desktop_settings::Setup) -> (Vec<Choice>, String, Vec<Choice>, Vec<Key>) {
    let view = setup.view(&|_| false);
    let options = &view.settings.transcription;
    let catalog = language::catalog();
    let spoken_by = |selection: &ModelSelection| {
        catalog
            .models
            .iter()
            .find(|model| same(&model.selection, selection))
            .map(|model| model.input_languages.clone())
            .unwrap_or_default()
    };
    let capable = |selection: &ModelSelection| {
        language::validate_capabilities(&TranscriptionOptions {
            selection: selection.clone(),
            ..options.clone()
        })
        .is_ok()
    };

    let languages = catalog
        .presets
        .iter()
        .map(|preset| Choice {
            id: name(&preset.id),
            label: preset.label.clone(),
            value: preset.label.clone(),
            detail: String::new(),
            selected: same(&preset.input_language, &options.input_language)
                && same(&preset.output_language, &options.output_language),
            enabled: language::validate_capabilities(&TranscriptionOptions {
                input_language: preset.input_language.clone(),
                output_language: preset.output_language.clone(),
                ..options.clone()
            })
            .is_ok(),
        })
        .collect();
    let language_note = match spoken_by(&options.selection).as_slice() {
        [] => String::new(),
        spoken => format!(
            "{} transcribes {}.",
            options.selection.model,
            sentence(spoken)
        ),
    };

    let mut models: Vec<Choice> = view
        .models
        .iter()
        .map(|model| {
            let selection = ModelSelection {
                provider: ProviderId::Local,
                model: model.id.into(),
            };
            let installed = matches!(model.weights, WeightsState::Installed { .. });
            Choice {
                detail: if !installed {
                    weights(&model.weights).into()
                } else if !capable(&selection) {
                    format!("{} only", sentence(&spoken_by(&selection)))
                } else {
                    String::new()
                },
                enabled: installed && capable(&selection),
                label: model.display_name.into(),
                value: model.display_name.into(),
                selected: model.selected,
                id: format!("local:{}", model.id),
            }
        })
        .collect();
    models.extend(view.cloud.iter().flat_map(|cloud| {
        let configured = !matches!(
            cloud.credential,
            CredentialStatus::Absent | CredentialStatus::Unreadable
        );
        let provider = name(&cloud.provider);
        cloud.models.iter().map(move |model| {
            let selection = ModelSelection {
                provider: cloud.provider.clone(),
                model: (*model).into(),
            };
            Choice {
                detail: if !configured {
                    "Needs a key".into()
                } else if !capable(&selection) {
                    format!("{} only", sentence(&spoken_by(&selection)))
                } else {
                    String::new()
                },
                enabled: configured && capable(&selection),
                selected: cloud.selected && options.selection.model == *model,
                label: format!("{} · {model}", cloud_name(&provider)),
                value: (*model).into(),
                id: format!("{provider}:{model}"),
            }
        })
    }));

    let keys = view
        .cloud
        .iter()
        .map(|cloud| Key {
            label: cloud_name(&name(&cloud.provider)).into(),
            present: matches!(
                cloud.credential,
                CredentialStatus::ProtectedStore | CredentialStatus::Environment
            ),
            provider: name(&cloud.provider),
        })
        .collect();
    (languages, language_note, models, keys)
}

/// The last transcript this shell can honestly offer to copy: the running
/// session's own success, or, once per launch, the newest saved one.
async fn last(
    app: &tauri::AppHandle,
    previous: Option<Last>,
    snapshot: Option<&Value>,
) -> Option<Last> {
    let finished = snapshot.filter(|s| s["state"] == "success").and_then(|s| {
        Some((
            s["job_id"].as_str()?.to_owned(),
            s["session_id"].as_str()?.to_owned(),
        ))
    });
    if let Some((job, session)) = finished {
        if previous.as_ref().is_some_and(|last| last.job == job) {
            return previous;
        }
        let Ok(id) = job.parse::<uuid::Uuid>() else {
            return previous;
        };
        let Ok(pipeline) = metrix_writer_api::jobs::runtime().await else {
            return previous;
        };
        return match pipeline.jobs.get(id).await {
            // A success that produced no text leaves the previous transcript as
            // the last one there is anything to copy from.
            Ok(found) => match transcript(&found) {
                Some((raw, corrected)) => Some(Last {
                    pasted: polished(id).await.unwrap_or(corrected),
                    at_ms: found.created_at_ms,
                    job,
                    session,
                    raw,
                }),
                None => previous,
            },
            Err(_) => previous,
        };
    }
    if previous.is_some() {
        return previous;
    }
    if app
        .try_state::<Tray>()
        .is_none_or(|tray| tray.scanned.load(Ordering::Acquire))
    {
        return None;
    }
    // Marked only after the history actually read, so a storage failure is
    // retried on the next refresh instead of silently disabling the entry.
    let mut jobs = metrix_writer_api::jobs::runtime()
        .await
        .ok()?
        .jobs
        .all()
        .await
        .ok()?;
    if let Some(tray) = app.try_state::<Tray>() {
        tray.scanned.store(true, Ordering::Release);
    }
    jobs.sort_by_key(|job| job.created_at_ms);
    let (job, (raw, corrected)) = jobs
        .iter()
        .rev()
        .find_map(|job| Some((job, transcript(job)?)))?;
    Some(Last {
        job: job.id.to_string(),
        session: job.request.recording_id.to_string(),
        pasted: polished(job.id).await.unwrap_or(corrected),
        at_ms: job.created_at_ms,
        raw,
    })
}

/// The exact recognised text, and the same text with the dictionary's
/// corrections and snippets applied — what a dictation without a polishing
/// mode pastes. `None` when the job has nothing to copy.
fn transcript(job: &metrix_writer_api::transcription::Job) -> Option<(String, String)> {
    let JobStatus::Succeeded { result } = &job.status else {
        return None;
    };
    let raw = result.transcript.raw_text.trim();
    if raw.is_empty() {
        return None;
    }
    let corrected = metrix_writer_api::vocabulary::process(
        &result.transcript.raw_text,
        &job.request.options.vocabulary,
    )
    .map(|text| text.text().trim().to_owned())
    .ok()
    .filter(|text| !text.is_empty())
    .unwrap_or_else(|| raw.to_owned());
    Some((raw.to_owned(), corrected))
}

/// The newest text a mode polished for this job, if any did. Read-only: the
/// menu never starts processing.
async fn polished(job: uuid::Uuid) -> Option<String> {
    let attempts = metrix_writer_api::cleanup::modes::service()
        .await
        .ok()?
        .attempts(job)
        .await
        .ok()?;
    attempts
        .into_iter()
        .filter_map(|attempt| match attempt.state {
            AttemptState::Succeeded { text } if !text.trim().is_empty() => {
                Some((attempt.created_at_ms, text.trim().to_owned()))
            }
            _ => None,
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, text)| text)
}

/// One row of the menu, decided before any native object exists so the whole
/// menu can be checked without a running shell.
#[derive(Clone, PartialEq)]
struct Entry {
    /// Empty on rows that only report state.
    id: String,
    label: String,
    /// Drawn in the right-hand column: a shortcut, a current value, or why a
    /// row is unavailable.
    detail: String,
    /// The detail is a shortcut, drawn as keycaps rather than as words.
    keys: bool,
    enabled: bool,
    /// Name of the glyph the menu window draws in the leading column. Empty
    /// leaves the column blank, which is what a row inside a submenu wants: a
    /// list of microphones is a set of choices, not a set of commands.
    icon: &'static str,
    kind: Kind,
}
#[derive(Clone, PartialEq)]
enum Kind {
    Action,
    /// One of a set: a microphone, a language, a model, a mode.
    Check(bool),
    /// A setting that is on or off, drawn as a switch.
    Switch(bool),
    Submenu(Vec<Entry>),
    Divider,
    /// The first row: what the dictation is doing, in a `tone` the page
    /// colours, counting up from `since` while it records.
    Status {
        tone: &'static str,
        since: Option<u64>,
    },
    /// A small title over the rows below it, with the time of what they are
    /// about when there is one. The page formats the time in local time.
    Heading(Option<u64>),
}

impl Entry {
    fn act(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: String::new(),
            keys: false,
            enabled: true,
            icon: "",
            kind: Kind::Action,
        }
    }
    /// A row that only reports state: no id and never enabled, so it cannot be
    /// clicked into meaning something.
    fn note(label: &str) -> Self {
        Self {
            enabled: false,
            ..Self::act("", label)
        }
    }
    fn check(id: &str, label: &str, on: bool) -> Self {
        Self {
            kind: Kind::Check(on),
            ..Self::act(id, label)
        }
    }
    fn switch(id: &str, label: &str, on: bool) -> Self {
        Self {
            kind: Kind::Switch(on),
            ..Self::act(id, label)
        }
    }
    fn heading(label: &str, at: Option<u64>) -> Self {
        Self {
            kind: Kind::Heading(at),
            ..Self::note(label)
        }
    }
    fn sub(label: &str, rows: Vec<Entry>) -> Self {
        Self {
            kind: Kind::Submenu(rows),
            ..Self::act("", label)
        }
    }
    fn line() -> Self {
        Self {
            kind: Kind::Divider,
            ..Self::note("")
        }
    }
    fn detail(self, detail: &str) -> Self {
        Self {
            detail: detail.into(),
            keys: false,
            ..self
        }
    }
    fn keys(self, chord: &str) -> Self {
        Self {
            detail: chord.into(),
            keys: !chord.is_empty(),
            ..self
        }
    }
    fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }
    fn icon(self, icon: &'static str) -> Self {
        Self { icon, ..self }
    }
}

/// The whole menu, from state alone.
fn plan(facts: &Facts) -> Vec<Entry> {
    let mut rows = vec![
        status_row(facts),
        Entry::act(
            "dictate",
            if facts.dictating {
                "Stop and paste"
            } else {
                "Start dictation"
            },
        )
        .keys(&facts.recording_chord)
        .icon(if facts.dictating { "stop" } else { "mic" })
        .enabled(facts.ready),
    ];
    // Discarding only means something while a dictation is in flight.
    if facts.cancelable {
        rows.push(
            Entry::act("cancel", "Discard dictation")
                .keys(&facts.cancel_chord)
                .icon("trash"),
        );
    }
    rows.push(Entry::line());

    rows.push(Entry::heading(
        "Last dictation",
        facts.last.as_ref().map(|last| last.at_ms),
    ));
    match &facts.last {
        Some(last) => {
            rows.push(Entry::note(&preview(&last.pasted)));
            if CLIPBOARD {
                rows.push(Entry::act("copy-pasted", "Copy it again").icon("copy"));
                // The exact recognised text stays reachable on its own
                // whenever what was pasted differs from it.
                if last.pasted != last.raw {
                    rows.push(Entry::act("copy-raw", "Copy as spoken").icon("quote"));
                }
            }
            rows.push(Entry::act("show-last", concat!("Open in ", product_name!())).icon("text"));
        }
        None => rows.push(Entry::note("Nothing dictated yet")),
    }
    rows.push(Entry::line());

    // Each submenu names its current value, so the menu answers "what am I
    // dictating with" without opening anything.
    rows.push(
        Entry::sub("Microphone", microphones(facts))
            .icon("mic")
            .detail(if facts.microphones_locked {
                "In use"
            } else {
                microphone(facts)
            }),
    );
    rows.push(
        Entry::sub("Language", languages(facts))
            .icon("globe")
            .detail(chosen(&facts.languages)),
    );
    rows.push(
        Entry::sub("Speech model", models(facts))
            .icon("engine")
            .detail(chosen(&facts.models)),
    );
    if !facts.modes.is_empty() {
        rows.push(
            Entry::sub("Mode", modes(facts))
                .icon("wand")
                .detail(mode(facts)),
        );
    }
    rows.push(Entry::line());
    rows.push(
        Entry::act("restore-panel", "Restore floating panel")
            .icon("window")
            .enabled(facts.overlay),
    );
    rows.push(Entry::sub("Preferences", preferences(facts)).icon("preferences"));
    rows.push(Entry::sub("Settings", sections()).icon("settings"));
    rows.push(Entry::line());
    rows.push(Entry::act("quit", concat!("Quit ", product_name!())).icon("power"));
    rows
}

/// The first row: whether you can dictate now, and with what. Choosing it
/// opens the window, which is what a click on the icon does on Windows.
fn status_row(facts: &Facts) -> Entry {
    let join = |parts: &[&str]| {
        parts
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let setup = join(&[chosen(&facts.models), chosen(&facts.languages), mode(facts)]);
    let listening = join(&[
        match facts.microphones.iter().find(|device| device.selected) {
            Some(Device { id: None, .. }) => "Default microphone",
            _ => microphone(facts),
        },
        mode(facts),
    ]);
    let (title, detail, tone) = match facts.status {
        Status::Starting => (
            "Getting ready",
            concat!(product_name!(), " is starting").to_owned(),
            "starting",
        ),
        Status::Ready => ("Ready to dictate", setup, "ready"),
        Status::Opening => ("Opening the microphone", listening, "recording"),
        Status::Recording => ("Recording", listening, "recording"),
        Status::Working => ("Transcribing", setup, "working"),
        Status::Failed => (
            "The last dictation failed",
            if facts.audio_saved {
                "Your audio is saved. Open to retry."
            } else {
                "Open for details."
            }
            .to_owned(),
            "failed",
        ),
    };
    Entry {
        kind: Kind::Status {
            tone,
            since: facts.since_ms.filter(|_| facts.status == Status::Recording),
        },
        ..Entry::act("open", title).detail(&detail)
    }
}

/// The selected value of a set, as the root row names it.
fn chosen(choices: &[Choice]) -> &str {
    choices
        .iter()
        .find(|choice| choice.selected)
        .map_or("", |choice| choice.value.as_str())
}

fn microphone(facts: &Facts) -> &str {
    facts
        .microphones
        .iter()
        .find(|device| device.selected)
        .map_or("", |device| device.label.as_str())
}

fn mode(facts: &Facts) -> &str {
    facts
        .modes
        .iter()
        .find(|mode| mode.active)
        .map_or("", |mode| mode.name.as_str())
}

fn microphones(facts: &Facts) -> Vec<Entry> {
    let mut rows = Vec::new();
    if let Some(problem) = &facts.microphone_problem {
        rows.push(Entry::note(problem));
    } else if facts.microphones.is_empty() {
        rows.push(Entry::note("Reading microphones…"));
    }
    for (index, device) in facts.microphones.iter().enumerate() {
        // The default input is a different kind of answer from a named device.
        if index == 1 {
            rows.push(Entry::line());
        }
        rows.push(
            Entry::check(
                &format!("mic:{}", device.id.clone().unwrap_or_default()),
                &device.label,
                device.selected,
            )
            .enabled(!facts.microphones_locked),
        );
    }
    if facts.microphones_locked {
        rows.push(Entry::line());
        rows.push(Entry::note("Stop recording to change the microphone"));
    }
    rows.push(Entry::line());
    rows.push(Entry::act("settings:microphone", "Microphone settings…").icon("settings"));
    rows
}

fn languages(facts: &Facts) -> Vec<Entry> {
    let mut rows = Vec::new();
    if facts.languages.is_empty() {
        rows.push(Entry::note("Language settings unavailable"));
    }
    for language in &facts.languages {
        rows.push(
            Entry::check(
                &format!("lang:{}", language.id),
                &language.label,
                language.selected,
            )
            .enabled(language.enabled),
        );
    }
    if !facts.language_note.is_empty() {
        rows.push(Entry::line());
        rows.push(Entry::note(&facts.language_note));
    }
    rows
}

fn models(facts: &Facts) -> Vec<Entry> {
    let mut rows = Vec::new();
    if facts.models.is_empty() {
        rows.push(Entry::note("Model setup unavailable"));
    }
    // Where a model runs is not a detail, so the two groups never mix.
    let (local, cloud): (Vec<_>, Vec<_>) = facts
        .models
        .iter()
        .partition(|model| model.id.starts_with("local:"));
    for (heading, group) in [("On this computer", local), ("Cloud", cloud)] {
        if group.is_empty() {
            continue;
        }
        rows.push(Entry::heading(heading, None));
        for model in group {
            rows.push(
                Entry::check(&format!("model:{}", model.id), &model.label, model.selected)
                    .detail(&model.detail)
                    .enabled(model.enabled),
            );
        }
    }
    let mut keys: Vec<Entry> = facts
        .keys
        .iter()
        .map(|key| {
            Entry::act(
                &format!("key-set:{}", key.provider),
                &format!("Set {} key…", key.label),
            )
        })
        .collect();
    keys.extend(facts.keys.iter().filter(|key| key.present).map(|key| {
        Entry::act(
            &format!("key-clear:{}", key.provider),
            &format!("Remove {} key", key.label),
        )
    }));
    rows.push(Entry::line());
    if !keys.is_empty() {
        rows.push(Entry::sub("API keys", keys).icon("key"));
    }
    rows.push(Entry::act("settings:engine", "Speech engine settings…").icon("settings"));
    rows
}

/// The window's own rule: a mode it would refuse is listed, disabled, with the
/// reason, unless it is already the active one.
fn modes(facts: &Facts) -> Vec<Entry> {
    let mut rows: Vec<Entry> = facts
        .modes
        .iter()
        .map(|mode| {
            let row = Entry::check(&format!("mode:{}", mode.id), &mode.name, mode.active)
                .enabled(mode.usable || mode.active);
            if mode.usable {
                row.keys(&mode.shortcut)
            } else {
                row.detail("Needs a text model")
            }
        })
        .collect();
    rows.push(Entry::line());
    rows.push(Entry::act("settings:modes", "Mode settings…").icon("settings"));
    rows
}

/// The switches, which the shell implements per platform.
fn preferences(facts: &Facts) -> Vec<Entry> {
    let mut rows = vec![
        Entry::switch("toggle:overlay", "Floating panel", facts.overlay),
        Entry::switch("toggle:sounds", "Start and stop sounds", facts.sounds),
    ];
    // macOS has no restorable per-app volumes to quiet.
    if cfg!(windows) {
        rows.push(Entry::switch(
            "toggle:duck",
            "Quiet other audio while dictating",
            facts.duck_audio,
        ));
    }
    rows.push(Entry::switch(
        "toggle:startup",
        if cfg!(target_os = "macos") {
            "Launch at login"
        } else {
            "Start with Windows"
        },
        facts.launch_at_login,
    ));
    rows
}

fn sections() -> Vec<Entry> {
    // The window's own sections, in its order and with its names and icons, so
    // both surfaces lead to the same places. A section the window no longer
    // knows falls back to General there rather than opening an empty pane.
    let mut rows: Vec<Entry> = [
        ("engine", "Speech engine", "engine"),
        ("shortcuts", "Shortcuts & pasting", "keyboard"),
        ("microphone", "Microphone & sound", "mic"),
        ("panel", "Floating panel", "panel"),
        ("modes", "Modes", "wand"),
        ("text", "Text models", "sparkles"),
        ("keys", "API keys", "key"),
        ("general", "General", "general"),
        ("appearance", "Appearance", "palette"),
        ("account", "Account & sync", "user"),
        ("data", "Privacy & data", "shield"),
        ("about", "About & updates", "info"),
    ]
    .into_iter()
    .map(|(id, label, icon)| Entry::act(&format!("settings:{id}"), label).icon(icon))
    .collect();
    rows.push(Entry::line());
    rows.push(Entry::act("data-folder", "Open data folder").icon("folder"));
    rows
}

/// The rows as the menu window draws them.
///
/// One shape for every kind, so the page never has to infer what a row is from
/// which fields happen to be present. `note` is the disabled, id-less row the
/// plan uses to report state; it is emitted as its own kind rather than as a
/// disabled action, because the page draws it differently and must never make
/// it focusable.
fn rows(entries: &[Entry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| match &entry.kind {
            Kind::Divider => json!({ "kind": "divider" }),
            Kind::Heading(at) => json!({
                "kind": "heading",
                "label": entry.label,
                "at_ms": at,
            }),
            Kind::Status { tone, since } => json!({
                "kind": "status",
                "id": entry.id,
                "label": entry.label,
                "detail": entry.detail,
                "tone": tone,
                "since_ms": since,
                "enabled": entry.enabled,
            }),
            Kind::Submenu(children) => json!({
                "kind": "submenu",
                "label": entry.label,
                "detail": entry.detail,
                "icon": entry.icon,
                "enabled": entry.enabled,
                "rows": rows(children),
            }),
            Kind::Check(on) | Kind::Switch(on) => json!({
                "kind": if matches!(entry.kind, Kind::Switch(_)) { "switch" } else { "check" },
                "id": entry.id,
                "label": entry.label,
                "detail": entry.detail,
                "keys": entry.keys,
                "icon": entry.icon,
                "enabled": entry.enabled,
                "checked": on,
            }),
            Kind::Action if entry.id.is_empty() => json!({
                "kind": "note",
                "label": entry.label,
                "detail": entry.detail,
            }),
            Kind::Action => json!({
                "kind": "action",
                "id": entry.id,
                "label": entry.label,
                "detail": entry.detail,
                "keys": entry.keys,
                "icon": entry.icon,
                "enabled": entry.enabled,
            }),
        })
        .collect()
}

/// Only the menu window may drive the menu. Everything these commands can do is
/// already reachable from the interface, but a document that is not the menu
/// has no business closing it or claiming a row was clicked.
fn menu_only(window: &WebviewWindow) -> Result<()> {
    super::security::window_only(window, MENU_WINDOW)
}

/// The rows waiting to be drawn, if any. The page asks on load, so a window
/// that finished loading after the menu was published still draws it.
#[tauri::command]
pub fn tray_menu_ready(window: WebviewWindow, app: tauri::AppHandle) -> Result<Value> {
    menu_only(&window)?;
    Ok(app
        .try_state::<Tray>()
        .and_then(|tray| tray.menu.lock().unwrap().clone())
        .unwrap_or(Value::Null))
}

/// The page finished laying the rows out and is reporting how much room they
/// need, in CSS pixels.
#[tauri::command]
pub fn tray_menu_measured(
    window: WebviewWindow,
    app: tauri::AppHandle,
    width: f64,
    height: f64,
) -> Result<()> {
    menu_only(&window)?;
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err("Invalid menu size.".into());
    }
    place(&app, width, height)
}

/// A row was activated. The menu closes first: several of these open a dialog
/// or reveal a window, and an always-on-top menu still standing in front of one
/// would be the shell arguing with itself.
#[tauri::command]
pub fn tray_menu_command(window: WebviewWindow, app: tauri::AppHandle, id: String) -> Result<()> {
    menu_only(&window)?;
    close(&app);
    chose(&app, &id);
    Ok(())
}

#[tauri::command]
pub fn tray_menu_dismiss(window: WebviewWindow, app: tauri::AppHandle) -> Result<()> {
    menu_only(&window)?;
    close(&app);
    Ok(())
}

fn preview(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(PREVIEW) {
        Some((cut, _)) => format!("“{}…”", text[..cut].trim_end()),
        None => format!("“{text}”"),
    }
}

fn weights(state: &WeightsState) -> &'static str {
    match state {
        WeightsState::NotInstalled { .. } => "Not downloaded",
        WeightsState::Downloading { .. } => "Downloading",
        WeightsState::Verifying { .. } => "Verifying",
        WeightsState::Installed { .. } => "",
        WeightsState::Failed { .. } => "Download failed",
    }
}

fn cloud_name(provider: &str) -> &'static str {
    match provider {
        "openai" => "OpenAI",
        "gemini" => "Gemini",
        "grok" => "Grok",
        "openrouter" => "OpenRouter",
        _ => "Cloud",
    }
}

fn spoken(language: &InputLanguage) -> &'static str {
    match name(language).as_str() {
        "he" => "Hebrew",
        "mixed" => "mixed speech",
        _ => "English",
    }
}

fn sentence(languages: &[InputLanguage]) -> String {
    let names: Vec<_> = languages.iter().map(spoken).collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// The shared contract types are serialisable but not comparable; the language
/// module identifies them the same way.
fn same<T: serde::Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

fn name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// What a clicked row asks for. Parsing ids separately from acting on them is
/// what lets a test prove every enabled row leads somewhere.
enum Command {
    Dictate,
    Cancel,
    /// The text that was pasted, or the exact recognised text.
    Copy {
        pasted: bool,
    },
    ShowLast,
    RestorePanel,
    Open,
    DataFolder,
    Quit,
    /// `None` is the operating system default input.
    Microphone(Option<String>),
    Language(String),
    Model(ProviderId, String),
    SetKey(String),
    ClearKey(String),
    Settings(String),
    Toggle(&'static str),
    Mode(String),
}

fn command(id: &str) -> Option<Command> {
    Some(match id.split_once(':') {
        Some(("mic", device)) => {
            Command::Microphone((!device.is_empty()).then(|| device.to_owned()))
        }
        Some(("lang", preset)) => Command::Language(preset.to_owned()),
        Some(("model", selection)) => {
            let (provider, model) = selection.split_once(':')?;
            Command::Model(
                match provider {
                    "openai" => ProviderId::Openai,
                    "gemini" => ProviderId::Gemini,
                    "grok" => ProviderId::Grok,
                    "openrouter" => ProviderId::Openrouter,
                    "local" => ProviderId::Local,
                    _ => return None,
                },
                model.to_owned(),
            )
        }
        Some(("key-set", provider)) => Command::SetKey(provider.to_owned()),
        Some(("key-clear", provider)) => Command::ClearKey(provider.to_owned()),
        Some(("settings", section)) => Command::Settings(section.to_owned()),
        Some(("mode", mode)) if !mode.is_empty() => Command::Mode(mode.to_owned()),
        Some(("toggle", "overlay")) => Command::Toggle("overlay"),
        Some(("toggle", "sounds")) => Command::Toggle("sounds"),
        Some(("toggle", "duck")) => Command::Toggle("duck"),
        Some(("toggle", "startup")) => Command::Toggle("startup"),
        None => match id {
            "dictate" => Command::Dictate,
            "cancel" => Command::Cancel,
            "copy-pasted" => Command::Copy { pasted: true },
            "copy-raw" => Command::Copy { pasted: false },
            "show-last" => Command::ShowLast,
            "restore-panel" => Command::RestorePanel,
            "open" => Command::Open,
            "data-folder" => Command::DataFolder,
            "quit" => Command::Quit,
            _ => return None,
        },
        Some(_) => return None,
    })
}

fn chose(app: &tauri::AppHandle, id: &str) {
    let Some(command) = command(id) else {
        return;
    };
    let state = app.state::<Shell>();
    match command {
        Command::Dictate => {
            // Same reading of "already running" the toggle shortcut uses.
            let running = state.pending.lock().unwrap().is_some()
                || state.snapshot.lock().unwrap().as_ref().is_some_and(|s| {
                    matches!(s["state"].as_str(), Some("requesting" | "capturing"))
                });
            dispatch(app, if running { "stop" } else { "start" });
        }
        Command::Cancel => dispatch(app, "cancel"),
        Command::RestorePanel => panel_recovery::restore(app),
        Command::Copy { pasted } => match remembered(app, |last| {
            Some(if pasted {
                last.pasted.clone()
            } else {
                last.raw.clone()
            })
        }) {
            Some(text) => copy(app, text),
            None => complain(
                app,
                "Transcript",
                "That transcript is no longer current. Copy it from history.".into(),
            ),
        },
        Command::ShowLast => {
            let session = remembered(app, |last| Some(last.session.clone()));
            reveal(app);
            if let Some(session) = session {
                let _ = app.emit_to("main", "writer-open-session", session);
            }
        }
        Command::Open => reveal(app),
        Command::DataFolder => {
            if let Err(problem) = super::show_data_folder(&state.root) {
                complain(app, "Data folder", problem);
            }
        }
        Command::Quit => request_quit(app),
        Command::Microphone(device) => {
            let capture = state.capture.clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let selected =
                    tauri::async_runtime::spawn_blocking(move || capture.select_microphone(device))
                        .await;
                if let Ok(Err(problem)) = selected {
                    complain(&app, "Microphone", problem);
                }
            });
        }
        Command::Language(preset) => settings(app, move |options| {
            let catalog = language::catalog();
            let chosen = catalog
                .presets
                .iter()
                .find(|candidate| name(&candidate.id) == preset)
                .ok_or("Unknown language preset.")?;
            options.input_language = chosen.input_language.clone();
            options.output_language = chosen.output_language.clone();
            Ok(())
        }),
        Command::Model(provider, model) => settings(app, move |options| {
            options.selection = ModelSelection {
                provider: provider.clone(),
                model: model.clone(),
            };
            Ok(())
        }),
        Command::SetKey(provider) => {
            let root = state.root.clone();
            let app = app.clone();
            std::thread::spawn(move || {
                if let Err(problem) = credentials::prompt(&root, &provider) {
                    complain(&app, "API key", problem);
                }
            });
        }
        Command::ClearKey(provider) => {
            if let Err(problem) = credentials::remove(&state.root, &provider) {
                complain(app, "API key", problem);
            }
        }
        // Revealing the window is the whole fallback. The standalone settings
        // window this used to open instead was a raw HTML form that duplicated
        // a stale subset of these sections, so failing to reach the document is
        // now reported rather than answered with a worse copy of it.
        Command::Settings(section) => {
            reveal(app);
            if app
                .emit_to("main", "writer-open-settings", section)
                .is_err()
            {
                complain(
                    app,
                    "Settings",
                    concat!(product_name!(), " could not open that settings section. The window is open; choose the section from its sidebar.").into(),
                );
            }
        }
        Command::Toggle(setting) => switch(app, setting),
        Command::Mode(mode) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                match choose_mode(&mode).await {
                    // The window's dock names the active mode; it reads it
                    // again now rather than at the next dictation.
                    Ok(()) => {
                        let _ = app.emit_to("main", "writer-modes-changed", ());
                    }
                    Err(problem) => complain(&app, "Mode", problem),
                }
            });
        }
    }
}

/// Makes `mode` the active mode through the same validated save the window
/// uses, which also refuses a mode that no longer exists. Read fresh first, so
/// a change made in the window a moment ago is kept rather than overwritten.
async fn choose_mode(mode: &str) -> Result<()> {
    let service = metrix_writer_api::cleanup::modes::service()
        .await
        .map_err(|error| error.message)?;
    let mut settings = service.settings().await.map_err(|error| error.message)?;
    if settings.active == mode {
        return Ok(());
    }
    settings.active = mode.to_owned();
    service
        .save(settings)
        .await
        .map(|_| ())
        .map_err(|error| error.message)
}

/// Reads the transcript the installed menu was actually showing, so a copy
/// never delivers text the user never saw offered.
fn remembered(app: &tauri::AppHandle, pick: impl Fn(&Last) -> Option<String>) -> Option<String> {
    let tray = app.try_state::<Tray>()?;
    let shown = tray.shown.lock().unwrap();
    pick(shown.as_ref()?.last.as_ref()?)
}

fn switch(app: &tauri::AppHandle, setting: &str) {
    let outcome = match setting {
        // The tray toggle is on/off only; "always" is chosen in Settings and
        // is preserved when the panel is switched back on from here.
        "overlay" => preferences::update(app, |preferences| {
            preferences.visibility = if preferences.visibility == "off" {
                "recording"
            } else {
                "off"
            }
            .into()
        }),
        "sounds" => system::update(app, |system| system.sounds = !system.sounds),
        "duck" => system::update(app, |system| system.duck_audio = !system.duck_audio),
        "startup" => system::update(app, |system| {
            system.launch_at_login = !system.launch_at_login
        }),
        _ => Ok(()),
    };
    if let Err(problem) = outcome {
        complain(app, "Settings", problem);
    }
}

/// Applies one transcription settings change through the same validated save
/// the interface uses, so an unsupported combination is refused here too.
fn settings(
    app: &tauri::AppHandle,
    change: impl FnOnce(&mut TranscriptionOptions) -> Result<()> + Send + 'static,
) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(setup) = desktop_settings::global() else {
            complain(&app, "Settings", "Model setup unavailable.".into());
            return;
        };
        let mut settings = setup.settings();
        if let Err(problem) = change(&mut settings.transcription) {
            complain(&app, "Settings", problem);
            return;
        }
        if let Err(problem) = setup.save(settings).await {
            complain(&app, "Settings", problem.message);
        }
    });
}

fn complain(app: &tauri::AppHandle, title: &str, message: String) {
    use tauri_plugin_dialog::DialogExt;
    app.dialog().message(message).title(title).show(|_| {});
}

/// Puts a saved transcript on the clipboard. The tray cannot know which window
/// had focus before the menu opened, so this never pastes for you.
#[cfg(any(windows, target_os = "macos"))]
fn copy(app: &tauri::AppHandle, text: String) {
    #[cfg(target_os = "macos")]
    let owner = 0;
    #[cfg(windows)]
    let owner = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .map_or(0, |handle| handle.0 as isize);
    let app = app.clone();
    std::thread::spawn(move || match delivery::clipboard(owner, &text) {
        Ok(()) => flash(
            &app,
            concat!(product_name!(), " — transcript copied").into(),
        ),
        Err(problem) => complain(&app, "Transcript", problem),
    });
}

#[cfg(not(any(windows, target_os = "macos")))]
fn copy(app: &tauri::AppHandle, _: String) {
    complain(
        app,
        "Transcript",
        concat!(
            "Copying from the tray is currently available on Windows. Open ",
            product_name!(),
            " and copy the saved transcript from history."
        )
        .into(),
    );
}

/// Brief tray tooltip confirmation, restored to whatever the current recording
/// state should say.
#[cfg(any(windows, target_os = "macos"))]
fn flash(app: &tauri::AppHandle, message: String) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.try_state::<Tray>() {
            let _ = tray.icon.set_tooltip(Some(message));
        }
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let look = status(&app).0.look();
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(tray) = handle.try_state::<Tray>() {
                let _ = tray.icon.set_tooltip(Some(tooltip(look)));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrix_writer_api::transcription::{
        CleanupResult, Correction, ExecutionLocation, Transcript, TranscriptionRequest,
        TranscriptionResult,
    };

    fn job(raw: &str, cleanup: CleanupResult) -> metrix_writer_api::transcription::Job {
        let selection = ModelSelection {
            provider: ProviderId::Local,
            model: "whisper-tiny".into(),
        };
        metrix_writer_api::transcription::Job {
            audio_preparation: None,
            recovery: None,
            id: uuid::Uuid::nil(),
            created_at_ms: 0,
            updated_at_ms: 0,
            request: TranscriptionRequest {
                recording_id: uuid::Uuid::nil(),
                idempotency_key: uuid::Uuid::nil(),
                retry_of_job_id: None,
                options: metrix_writer_api::desktop_settings::default_settings().transcription,
            },
            status: JobStatus::Succeeded {
                result: TranscriptionResult {
                    transcript: Transcript {
                        raw_text: raw.into(),
                        selection,
                        execution_location: ExecutionLocation::SelfHostedServer,
                        input_language: InputLanguage::He,
                        output_language:
                            metrix_writer_api::transcription::OutputLanguage::Preserve {},
                    },
                    cleanup,
                },
            },
        }
    }

    /// Every row of the menu, submenus included, in drawing order.
    fn flatten(entries: &[Entry]) -> Vec<Entry> {
        entries
            .iter()
            .flat_map(|entry| match &entry.kind {
                Kind::Submenu(children) => {
                    let mut rows = vec![entry.clone()];
                    rows.extend(flatten(children));
                    rows
                }
                _ => vec![entry.clone()],
            })
            .collect()
    }
    fn row<'a>(entries: &'a [Entry], id: &str) -> Option<&'a Entry> {
        entries.iter().find(|entry| entry.id == id)
    }
    fn submenu<'a>(entries: &'a [Entry], label: &str) -> &'a Entry {
        entries
            .iter()
            .find(|entry| entry.label == label && matches!(entry.kind, Kind::Submenu(_)))
            .unwrap_or_else(|| panic!("no {label} submenu"))
    }
    fn choice(
        id: &str,
        label: &str,
        value: &str,
        detail: &str,
        selected: bool,
        enabled: bool,
    ) -> Choice {
        Choice {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            detail: detail.into(),
            selected,
            enabled,
        }
    }
    fn dictated() -> Facts {
        Facts {
            status: Status::Ready,
            ready: true,
            recording_chord: "Ctrl+Shift+F9".into(),
            cancel_chord: "Ctrl+Shift+F10".into(),
            microphones: vec![
                Device {
                    id: None,
                    label: "System default".into(),
                    selected: false,
                },
                Device {
                    id: Some("Headset".into()),
                    label: "Headset".into(),
                    selected: true,
                },
            ],
            languages: vec![
                choice("en", "English", "English", "", false, false),
                choice("he", "Hebrew", "Hebrew", "", true, true),
            ],
            language_note: "ivrit-turbo transcribes Hebrew.".into(),
            models: vec![
                choice(
                    "local:ivrit-turbo",
                    "ivrit-turbo",
                    "ivrit-turbo",
                    "",
                    true,
                    true,
                ),
                choice(
                    "local:whisper-tiny",
                    "Whisper Tiny",
                    "Whisper Tiny",
                    "Not downloaded",
                    false,
                    false,
                ),
                choice(
                    "openai:gpt-transcribe",
                    "OpenAI · gpt-transcribe",
                    "gpt-transcribe",
                    "Needs a key",
                    false,
                    false,
                ),
            ],
            keys: vec![Key {
                provider: "openai".into(),
                label: "OpenAI".into(),
                present: false,
            }],
            modes: vec![
                Mode {
                    id: "regular".into(),
                    name: "Regular".into(),
                    shortcut: String::new(),
                    active: false,
                    usable: true,
                },
                Mode {
                    id: "super".into(),
                    name: "Super".into(),
                    shortcut: "Ctrl+Alt+S".into(),
                    active: true,
                    usable: true,
                },
                Mode {
                    id: "command".into(),
                    name: "Command".into(),
                    shortcut: String::new(),
                    active: false,
                    usable: false,
                },
            ],
            last: Some(Last {
                job: "j".into(),
                session: "s".into(),
                raw: "שלום".into(),
                pasted: "שלום".into(),
                at_ms: 1_790_000_000_000,
            }),
            ..Default::default()
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_system_rows_offer_only_implemented_actions() {
        let entries = flatten(&plan(&Facts::default()));
        assert!(entries.iter().any(|entry| entry.id == "toggle:sounds"));
        assert_eq!(
            row(&entries, "toggle:startup").unwrap().label,
            "Launch at login"
        );
        // macOS has no restorable per-app volumes, so there is nothing to quiet.
        assert!(!entries.iter().any(|entry| entry.id == "toggle:duck"));
    }

    #[test]
    fn every_row_that_can_be_clicked_reaches_a_command() {
        for facts in [Facts::default(), dictated()] {
            let rows = flatten(&plan(&facts));
            assert!(rows.len() > 12, "{}", rows.len());
            for entry in &rows {
                if entry.id.is_empty() {
                    // Notes, headings, dividers and submenu parents are not
                    // clickable.
                    assert!(
                        !entry.enabled || matches!(entry.kind, Kind::Submenu(_)),
                        "{}",
                        entry.label
                    );
                    continue;
                }
                assert!(command(&entry.id).is_some(), "{}", entry.id);
            }
        }
        // Ids the menu never emits are refused rather than falling through to
        // some neighbouring action.
        for unknown in [
            "",
            "toggle:launch",
            "model:aws:whisper",
            "model:local",
            "mic",
            "mode:",
            "copy-cleaned",
        ] {
            assert!(command(unknown).is_none(), "{unknown}");
        }
        assert!(matches!(command("mic:"), Some(Command::Microphone(None))));
        assert!(matches!(command("mode:super"), Some(Command::Mode(mode)) if mode == "super"));
    }

    #[test]
    fn the_first_entries_say_whether_you_can_dictate_and_what_the_shortcut_does() {
        let idle = plan(&dictated());
        assert!(matches!(
            idle[0].kind,
            Kind::Status {
                tone: "ready",
                since: None
            }
        ));
        assert_eq!(idle[0].id, "open");
        assert_eq!(idle[0].label, "Ready to dictate");
        assert_eq!(idle[0].detail, "ivrit-turbo · Hebrew · Super");

        let start = row(&idle, "dictate").unwrap();
        assert_eq!(start.label, "Start dictation");
        assert_eq!(start.detail, "Ctrl+Shift+F9");
        assert!(start.keys);
        assert!(start.enabled);
        // Discard is offered only while there is something to discard.
        assert!(row(&idle, "cancel").is_none());

        let running = plan(&Facts {
            status: Status::Recording,
            since_ms: Some(42),
            dictating: true,
            cancelable: true,
            microphones_locked: true,
            ..dictated()
        });
        assert!(matches!(
            running[0].kind,
            Kind::Status {
                tone: "recording",
                since: Some(42)
            }
        ));
        assert_eq!(running[0].label, "Recording");
        assert_eq!(running[0].detail, "Headset · Super");
        assert_eq!(row(&running, "dictate").unwrap().label, "Stop and paste");
        let cancel = row(&running, "cancel").unwrap();
        assert!(cancel.enabled && cancel.keys);
        assert_eq!(cancel.detail, "Ctrl+Shift+F10");
        assert_eq!(submenu(&running, "Microphone").detail, "In use");

        // Before the Writer document reports a session there is nothing to
        // address a start command to.
        let waiting = plan(&Facts::default());
        assert!(!row(&waiting, "dictate").unwrap().enabled);
        assert!(matches!(
            waiting[0].kind,
            Kind::Status {
                tone: "starting",
                ..
            }
        ));
    }

    #[test]
    fn the_status_row_names_every_state_and_the_icon_agrees() {
        let says = |status: Status, audio_saved: bool| {
            let rows = plan(&Facts {
                status,
                audio_saved,
                ..dictated()
            });
            (rows[0].label.clone(), rows[0].detail.clone())
        };
        assert_eq!(says(Status::Opening, false).0, "Opening the microphone");
        assert_eq!(says(Status::Working, false).0, "Transcribing");
        assert_eq!(
            says(Status::Failed, true),
            (
                "The last dictation failed".into(),
                "Your audio is saved. Open to retry.".into()
            )
        );
        assert_eq!(says(Status::Failed, false).1, "Open for details.");
        // The default microphone is named as such, not as "System default".
        let rows = plan(&Facts {
            status: Status::Recording,
            microphones: vec![Device {
                id: None,
                label: "System default".into(),
                selected: true,
            }],
            ..dictated()
        });
        assert_eq!(rows[0].detail, "Default microphone · Super");

        assert_eq!(Status::Starting.look(), Look::Idle);
        assert_eq!(Status::Ready.look(), Look::Idle);
        assert_eq!(Status::Opening.look(), Look::Recording);
        assert_eq!(Status::Recording.look(), Look::Recording);
        assert_eq!(Status::Working.look(), Look::Working);
        assert_eq!(Status::Failed.look(), Look::Attention);
        for look in [Look::Idle, Look::Recording, Look::Working, Look::Attention] {
            assert!(tooltip(look).starts_with(TOOLTIP));
        }
    }

    #[test]
    fn quick_switches_show_their_current_value() {
        let root = plan(&dictated());
        assert_eq!(submenu(&root, "Microphone").detail, "Headset");
        assert_eq!(submenu(&root, "Language").detail, "Hebrew");
        assert_eq!(submenu(&root, "Speech model").detail, "ivrit-turbo");
        assert_eq!(submenu(&root, "Mode").detail, "Super");
        // A cloud model is named by its model alone; the submenu says where it runs.
        let cloud = plan(&Facts {
            models: vec![choice(
                "openai:gpt-transcribe",
                "OpenAI · gpt-transcribe",
                "gpt-transcribe",
                "",
                true,
                true,
            )],
            ..dictated()
        });
        assert_eq!(submenu(&cloud, "Speech model").detail, "gpt-transcribe");
        // Without modes there is no Mode row to show an empty value.
        let none = plan(&Facts {
            modes: vec![],
            ..dictated()
        });
        assert!(!none.iter().any(|entry| entry.label == "Mode"));
    }

    #[test]
    fn a_mode_the_window_would_refuse_is_listed_disabled_with_the_reason() {
        let rows = flatten(&plan(&dictated()));
        let active = row(&rows, "mode:super").unwrap();
        assert!(matches!(active.kind, Kind::Check(true)));
        assert!(active.enabled && active.keys);
        assert_eq!(active.detail, "Ctrl+Alt+S");
        let regular = row(&rows, "mode:regular").unwrap();
        assert!(regular.enabled && !regular.keys && regular.detail.is_empty());
        let command = row(&rows, "mode:command").unwrap();
        assert!(!command.enabled && !command.keys);
        assert_eq!(command.detail, "Needs a text model");
        assert!(row(&rows, "settings:modes").is_some());

        // The active mode stays choosable even when its model went away, as
        // the window's own menu leaves it.
        let stranded = flatten(&plan(&Facts {
            modes: vec![Mode {
                id: "super".into(),
                name: "Super".into(),
                shortcut: String::new(),
                active: true,
                usable: false,
            }],
            ..dictated()
        }));
        assert!(row(&stranded, "mode:super").unwrap().enabled);
    }

    #[test]
    fn preferences_are_switches_and_settings_lead_to_the_windows_sections() {
        let root = plan(&dictated());
        let Kind::Submenu(preferences) = &submenu(&root, "Preferences").kind else {
            unreachable!()
        };
        let switches: Vec<&str> = preferences
            .iter()
            .filter(|entry| matches!(entry.kind, Kind::Switch(_)))
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(switches[..2], ["toggle:overlay", "toggle:sounds"]);
        assert!(switches.contains(&"toggle:startup"));
        assert_eq!(switches.contains(&"toggle:duck"), cfg!(windows));
        assert!(row(preferences, "restore-panel").is_none());
        assert!(row(&root, "restore-panel").is_some());
        // None of these stay on the root, which is what makes it shorter.
        assert!(!root.iter().any(|entry| entry.id.starts_with("toggle:")));

        // Every section the window has, by the name the window gives it.
        let Kind::Submenu(sections) = &submenu(&root, "Settings").kind else {
            unreachable!()
        };
        let search = include_str!("../../../../web/src/features/settings/search.ts");
        let labels = search
            .split_once("export const SECTION_LABELS")
            .and_then(|(_, rest)| rest.split_once("};"))
            .map(|(block, _)| block)
            .unwrap();
        let listed: Vec<&Entry> = sections
            .iter()
            .filter(|entry| entry.id.starts_with("settings:"))
            .collect();
        assert_eq!(listed.len(), labels.matches(": '").count());
        for entry in listed {
            let id = entry.id.trim_start_matches("settings:");
            assert!(
                labels.contains(&format!("  {id}: '{}',", entry.label.replace('\'', "\\'"))),
                "{id} is not named \"{}\" in Settings",
                entry.label
            );
        }
        assert!(row(sections, "data-folder").is_some());
        // The submenu links land on the sections that hold those settings.
        let rows = flatten(&root);
        assert!(row(&rows, "settings:microphone").is_some());
        assert!(row(&rows, "settings:engine").is_some());
    }

    #[test]
    fn unavailable_choices_are_shown_with_their_reason_rather_than_hidden() {
        let rows = flatten(&plan(&dictated()));
        let english = row(&rows, "lang:en").unwrap();
        assert_eq!(english.label, "English");
        assert!(!english.enabled);
        assert!(rows
            .iter()
            .any(|entry| entry.label == "ivrit-turbo transcribes Hebrew."));

        let tiny = row(&rows, "model:local:whisper-tiny").unwrap();
        assert!(!tiny.enabled);
        assert_eq!(tiny.detail, "Not downloaded");
        assert!(!tiny.keys);
        assert_eq!(
            row(&rows, "model:openai:gpt-transcribe").unwrap().detail,
            "Needs a key"
        );
        // Where a model runs heads its group.
        assert!(rows
            .iter()
            .any(|entry| entry.label == "On this computer"
                && matches!(entry.kind, Kind::Heading(None))));
        // A key that is not configured can still be set; only removal is hidden.
        assert!(row(&rows, "key-set:openai").is_some());
        assert!(row(&rows, "key-clear:openai").is_none());
    }

    #[test]
    fn the_microphone_list_refuses_a_change_while_the_recorder_streams() {
        let rows = flatten(&plan(&dictated()));
        assert!(row(&rows, "mic:Headset").unwrap().enabled);
        assert!(matches!(
            row(&rows, "mic:Headset").unwrap().kind,
            Kind::Check(true)
        ));
        assert!(matches!(
            row(&rows, "mic:").unwrap().kind,
            Kind::Check(false)
        ));

        let capturing = flatten(&plan(&Facts {
            microphones_locked: true,
            ..dictated()
        }));
        assert!(!row(&capturing, "mic:Headset").unwrap().enabled);
        assert!(capturing
            .iter()
            .any(|entry| entry.label == "Stop recording to change the microphone"));
    }

    #[test]
    fn copy_entries_appear_only_for_a_transcript_that_exists() {
        let empty = plan(&Facts::default());
        assert!(row(&empty, "copy-pasted").is_none());
        assert!(row(&empty, "copy-raw").is_none());
        assert!(row(&empty, "show-last").is_none());
        assert!(empty
            .iter()
            .any(|entry| entry.label == "Nothing dictated yet"));
        assert!(empty
            .iter()
            .any(|entry| matches!(entry.kind, Kind::Heading(None))
                && entry.label == "Last dictation"));

        let same = plan(&dictated());
        assert!(same
            .iter()
            .any(|entry| matches!(entry.kind, Kind::Heading(Some(1_790_000_000_000)))));
        assert!(row(&same, "show-last").unwrap().enabled);
        assert_eq!(row(&same, "copy-pasted").is_some(), CLIPBOARD);
        // What was pasted is what was said, so there is one copy entry.
        assert!(row(&same, "copy-raw").is_none());

        let polished = plan(&Facts {
            last: Some(Last {
                job: "j".into(),
                session: "s".into(),
                raw: "hello".into(),
                pasted: "Hello.".into(),
                at_ms: 0,
            }),
            ..Facts::default()
        });
        // The preview quotes what was pasted; the exact words stay a click away.
        assert!(polished.iter().any(|entry| entry.label == "“Hello.”"));
        assert_eq!(row(&polished, "copy-raw").is_some(), CLIPBOARD);
    }

    #[test]
    fn the_pasted_text_applies_the_dictionary_and_the_raw_text_stays_exact() {
        let (raw, corrected) = transcript(&job(" hello  there ", CleanupResult::Off {})).unwrap();
        assert_eq!(raw, "hello  there");
        assert_eq!(corrected, "hello  there");

        let mut fixed = job("meet at the mettrix office", CleanupResult::Off {});
        fixed
            .request
            .options
            .vocabulary
            .corrections
            .push(Correction {
                from: "mettrix".into(),
                to: "Metrix".into(),
            });
        let (raw, corrected) = transcript(&fixed).unwrap();
        assert_eq!(raw, "meet at the mettrix office");
        assert_eq!(corrected, "meet at the Metrix office");

        // A job with nothing recognised, or not finished, has nothing to copy.
        assert!(transcript(&job("   ", CleanupResult::Off {})).is_none());
        let mut queued = job("hello", CleanupResult::Off {});
        queued.status = JobStatus::Queued {};
        assert!(transcript(&queued).is_none());
    }

    #[test]
    fn a_saved_microphone_that_is_gone_stays_the_selection_and_says_so() {
        let list = crate::microphones::MicrophoneList {
            devices: vec![crate::microphones::Microphone {
                id: "Headset".into(),
                label: "Headset".into(),
            }],
            selected_id: Some("Desk mic".into()),
            ..Default::default()
        };
        let listed = devices(&list);
        assert_eq!(listed[0].label, "System default");
        assert!(!listed[0].selected);
        assert!(!listed[1].selected);
        assert_eq!(listed[2].label, "Desk mic (unavailable)");
        assert!(listed[2].selected);
        assert_eq!(listed[2].id.as_deref(), Some("Desk mic"));

        let present = devices(&crate::microphones::MicrophoneList {
            devices: vec![crate::microphones::Microphone {
                id: "Headset".into(),
                label: "Headset".into(),
            }],
            selected_id: Some("Headset".into()),
            ..Default::default()
        });
        assert_eq!(present.len(), 2);
        assert!(present[1].selected);
        assert!(
            devices(&crate::microphones::MicrophoneList {
                devices: vec![],
                selected_id: None,
                ..Default::default()
            })[0]
                .selected
        );
    }

    /// Every id the page can send back, including the ones nested in submenus.
    fn published(rows: &[Value]) -> Vec<Value> {
        rows.iter()
            .flat_map(|row| match row["rows"].as_array() {
                Some(children) => {
                    let mut all = vec![row.clone()];
                    all.extend(published(children));
                    all
                }
                None => vec![row.clone()],
            })
            .collect()
    }

    #[test]
    fn the_published_menu_carries_every_state_the_page_draws_and_no_id_it_invented() {
        let rows = published(&rows(&plan(&Facts {
            status: Status::Recording,
            since_ms: Some(1_790_000_000_000),
            ..dictated()
        })));

        let status = &rows[0];
        assert_eq!(status["kind"], "status");
        assert_eq!(status["id"], "open");
        assert_eq!(status["tone"], "recording");
        assert_eq!(status["since_ms"], 1_790_000_000_000_u64);

        let start = rows.iter().find(|row| row["id"] == "dictate").unwrap();
        assert_eq!(start["kind"], "action");
        assert_eq!(start["label"], "Start dictation");
        assert_eq!(start["enabled"], true);
        assert_eq!(start["icon"], "mic");
        // The chord is drawn verbatim, as keycaps. A modifier-only chord is
        // exactly the case a parsed platform accelerator used to drop silently.
        assert_eq!(start["detail"], "Ctrl+Shift+F9");
        assert_eq!(start["keys"], true);

        // A note is its own kind, so the page draws it as the text it is rather
        // than as a control the user could try and fail to click.
        let note = rows.iter().find(|row| {
            row["kind"] == "note" && row["label"] == preview("\u{5e9}\u{5dc}\u{5d5}\u{5dd}")
        });
        assert!(note.is_some(), "the transcript preview is a note");
        let heading = rows
            .iter()
            .find(|row| row["kind"] == "heading" && row["label"] == "Last dictation")
            .unwrap();
        assert_eq!(heading["at_ms"], 1_790_000_000_000_u64);
        assert!(rows.iter().any(|row| row["kind"] == "divider"));

        let tiny = rows
            .iter()
            .find(|row| row["id"] == "model:local:whisper-tiny")
            .expect("the unavailable model is still listed");
        assert_eq!(tiny["kind"], "check");
        assert_eq!(tiny["checked"], false);
        assert_eq!(tiny["enabled"], false);
        assert_eq!(tiny["detail"], "Not downloaded");
        assert_eq!(tiny["keys"], false);

        let overlay = rows
            .iter()
            .find(|row| row["id"] == "toggle:overlay")
            .unwrap();
        assert_eq!(overlay["kind"], "switch");
        assert_eq!(overlay["checked"], false);

        // Whatever the page sends back has to be an id this menu issued and
        // `command` already understands.
        for row in &rows {
            if let Some(id) = row["id"].as_str() {
                assert!(command(id).is_some(), "{id}");
            }
            assert_ne!(row["kind"], Value::Null, "every row names its kind");
        }
    }

    /// The page learns its rows from the `writer-tray-menu` event, and a window
    /// may only listen for events if a capability names it. Without that the
    /// right click publishes rows nobody receives, the page never measures
    /// them, and the menu never appears.
    #[test]
    fn the_menu_window_may_listen_for_the_rows_it_is_sent() {
        let capability: Value =
            serde_json::from_str(include_str!("../../capabilities/desktop.json")).unwrap();
        let windows = capability["windows"].as_array().unwrap();
        assert!(
            windows.iter().any(|window| window == MENU_WINDOW),
            "{windows:?}"
        );
        assert!(capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|permission| permission == "core:event:allow-listen"));
    }

    #[test]
    fn a_modifier_only_chord_survives_publication() {
        let rows = rows(&plan(&Facts {
            recording_chord: "Win+Alt".into(),
            ..dictated()
        }));
        let start = rows.iter().find(|row| row["id"] == "dictate").unwrap();
        assert_eq!(start["detail"], "Win+Alt");
    }

    #[test]
    fn preview_collapses_whitespace_and_marks_where_it_was_cut() {
        assert_eq!(preview("one\n two\t three"), "“one two three”");
        let long = preview(&"word ".repeat(40));
        assert!(long.ends_with("…”"), "{long}");
        assert!(long.chars().count() < PREVIEW + 8, "{long}");
    }

    #[test]
    fn language_lists_read_as_a_sentence() {
        assert_eq!(sentence(&[InputLanguage::He]), "Hebrew");
        assert_eq!(
            sentence(&[InputLanguage::En, InputLanguage::He, InputLanguage::Mixed]),
            "English, Hebrew and mixed speech"
        );
        assert_eq!(sentence(&[]), "");
    }
}
