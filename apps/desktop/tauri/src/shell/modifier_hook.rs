use super::{
    hotkey_state::{ChordState, Edge},
    preferences, win_input, Result,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Mutex, OnceLock,
};
static CAPTURING: AtomicBool = AtomicBool::new(false);
pub fn capturing(active: bool) {
    CAPTURING.store(active, Ordering::Release);
}
struct HookState {
    recording: ChordState,
    cancel: ChordState,
    chords: [Vec<i32>; 2],
    send: mpsc::Sender<HookEvent>,
}
struct HookEvent {
    mask: bool,
    recording: Option<Edge>,
    cancel: Option<Edge>,
}
static STATE: OnceLock<Mutex<HookState>> = OnceLock::new();
#[cfg(feature = "latency-probe")]
mod probe {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use tauri::Manager;
    pub static FORWARDED: AtomicU64 = AtomicU64::new(0);
    pub static FINISHED: AtomicU64 = AtomicU64::new(0);
    pub static TAGGED: AtomicU64 = AtomicU64::new(0);
    pub static PHYSICAL: AtomicU64 = AtomicU64::new(0);
    pub static INJECTED: AtomicU64 = AtomicU64::new(0);
    pub static PRESSES: AtomicU64 = AtomicU64::new(0);
    pub static RECEIVED: AtomicU64 = AtomicU64::new(0);
    pub static MASK_STARTED: AtomicU64 = AtomicU64::new(0);
    pub static MASK_FINISHED: AtomicU64 = AtomicU64::new(0);
    pub static REMOVE: AtomicBool = AtomicBool::new(false);
    pub static REMOVED: AtomicU64 = AtomicU64::new(0);
    pub static RENEWED: AtomicU64 = AtomicU64::new(0);
    static TEST_INPUT: AtomicBool = AtomicBool::new(false);
    pub fn test_input(extra: usize) -> bool {
        extra == 0x5752_5042 && TEST_INPUT.load(Ordering::Acquire)
    }
    pub fn start(app: tauri::AppHandle) {
        let root = app.state::<super::super::Shell>().root.clone();
        if !root.join("shortcut-probe").is_file() {
            return;
        }
        TEST_INPUT.store(true, Ordering::Release);
        std::thread::spawn(move || loop {
            if std::fs::remove_file(root.join("remove-shortcut-hook")).is_ok() {
                REMOVE.store(true, Ordering::Release);
            }
            let shell = app.state::<super::super::Shell>();
            if shell.quitting.load(Ordering::Acquire) {
                break;
            }
            let snapshot = shell.snapshot.try_lock().ok().and_then(|s| {
                s.as_ref()
                    .and_then(|s| s["state"].as_str().map(str::to_owned))
            });
            let mut foreground_process = 0;
            unsafe {
                win_input::GetWindowThreadProcessId(
                    win_input::GetForegroundWindow(),
                    &mut foreground_process,
                );
            }
            // Explicit probe build + private-root marker only. Aggregate counts
            // and state, never key codes, typed text, transcript or target title.
            let _ = crate::storage::atomic_json(
                &root.join("State/shortcut-probe.json"),
                &serde_json::json!({
                    "removed": REMOVED.load(Ordering::Relaxed),
                    "renewed": RENEWED.load(Ordering::Relaxed),
                    "forwarded": FORWARDED.load(Ordering::Relaxed),
                    "finished": FINISHED.load(Ordering::Relaxed),
                    "tagged": TAGGED.load(Ordering::Relaxed),
                    "physical_events": PHYSICAL.load(Ordering::Relaxed),
                    "injected_events": INJECTED.load(Ordering::Relaxed),
                    "recording_presses": PRESSES.load(Ordering::Relaxed),
                    "worker_received": RECEIVED.load(Ordering::Relaxed),
                    "mask_started": MASK_STARTED.load(Ordering::Relaxed),
                    "mask_finished": MASK_FINISHED.load(Ordering::Relaxed),
                    "shortcut_editor": shell.shortcut_capture.load(Ordering::Acquire),
                    "own_foreground": foreground_process == std::process::id(),
                    "snapshot": snapshot,
                }),
            );
            std::thread::sleep(std::time::Duration::from_millis(250));
        });
    }
}
#[repr(C)]
struct KeyboardEvent {
    vk: u32,
    scan: u32,
    flags: u32,
    time: u32,
    extra: usize,
}
#[repr(C)]
struct Message {
    window: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    x: i32,
    y: i32,
    private: u32,
}
#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(
        kind: i32,
        callback: unsafe extern "system" fn(i32, usize, isize) -> isize,
        module: isize,
        thread: u32,
    ) -> isize;
    fn CallNextHookEx(hook: isize, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(message: *mut Message, window: isize, min: u32, max: u32) -> i32;
    fn UnhookWindowsHookEx(hook: isize) -> i32;
    fn SetTimer(window: isize, id: usize, ms: u32, callback: usize) -> usize;
    fn KillTimer(window: isize, id: usize) -> i32;
}
unsafe extern "system" fn callback(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let e = &*(lparam as *const KeyboardEvent);
        let physical = e.flags & 0x10 == 0;
        #[cfg(feature = "latency-probe")]
        if physical {
            probe::PHYSICAL.fetch_add(1, Ordering::Relaxed);
        } else {
            probe::INJECTED.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(feature = "latency-probe")]
        let physical = physical || {
            let tagged = probe::test_input(e.extra);
            if tagged {
                probe::TAGGED.fetch_add(1, Ordering::Relaxed);
            }
            tagged
        };
        if physical {
            if let Some(lock) = STATE.get() {
                if let Ok(mut s) = lock.lock() {
                    let down = matches!(wparam, 0x100 | 0x104);
                    let chords = s.chords.clone();
                    let recording = s.recording.update(e.vk as i32, down, &chords[0]);
                    #[cfg(feature = "latency-probe")]
                    if recording == Some(Edge::Press) {
                        probe::PRESSES.fetch_add(1, Ordering::Relaxed);
                    }
                    let cancel = s.cancel.update(e.vk as i32, down, &chords[1]);
                    let mask = recording == Some(Edge::Press)
                        || cancel == Some(Edge::Press)
                        || down && CAPTURING.load(Ordering::Acquire);
                    // Never inject input from inside the low-level hook. The
                    // injected event traverses the hook chain again, including
                    // hooks in other processes. Queue it with the edges
                    // and return so input delivery cannot stall the callback.
                    if mask || recording.is_some() || cancel.is_some() {
                        let _ = s.send.send(HookEvent {
                            mask,
                            recording,
                            cancel,
                        });
                    }
                }
            }
        }
    }
    #[cfg(feature = "latency-probe")]
    probe::FORWARDED.fetch_add(1, Ordering::Relaxed);
    let result = CallNextHookEx(0, code, wparam, lparam);
    #[cfg(feature = "latency-probe")]
    probe::FINISHED.fetch_add(1, Ordering::Relaxed);
    result
}
pub fn configure(p: &preferences::Preferences) {
    if let Some(s) = STATE.get() {
        if let Ok(mut s) = s.lock() {
            s.chords = [&p.recording, &p.cancel].map(|c| {
                if preferences::modifier_only(c) {
                    preferences::keys(c).unwrap_or_default()
                } else {
                    vec![]
                }
            });
        }
    }
}
pub fn start(app: tauri::AppHandle, p: &preferences::Preferences) -> Result<()> {
    let (send, receive) = mpsc::channel();
    STATE
        .set(Mutex::new(HookState {
            recording: Default::default(),
            cancel: Default::default(),
            chords: [vec![], vec![]],
            send,
        }))
        .map_err(|_| "Keyboard observer already started.")?;
    configure(p);
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::spawn(move || unsafe {
        let mut hook = SetWindowsHookExW(13, callback, 0, 0);
        let timer = SetTimer(0, 0, 5000, 0);
        let _ = ready.send(hook != 0 && timer != 0);
        if hook == 0 || timer == 0 {
            if hook != 0 {
                UnhookWindowsHookEx(hook);
            }
            if timer != 0 {
                KillTimer(0, timer);
            }
            return;
        }
        let mut message = std::mem::zeroed();
        while GetMessageW(&mut message, 0, 0, 0) > 0 {
            if message.message != 0x113 || message.wparam != timer {
                continue;
            }
            #[cfg(feature = "latency-probe")]
            if probe::REMOVE.swap(false, Ordering::AcqRel) {
                UnhookWindowsHookEx(hook);
                hook = 0;
                probe::REMOVED.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            // Windows can silently remove a timed-out low-level hook. Renew
            // while the keyboard is idle; do not inject heartbeat keystrokes or
            // disturb a held chord. Installing first keeps the old hook on failure.
            if (8..=255).any(|key| win_input::GetAsyncKeyState(key) < 0) {
                continue;
            }
            let replacement = SetWindowsHookExW(13, callback, 0, 0);
            if replacement == 0 {
                continue;
            }
            if hook != 0 {
                UnhookWindowsHookEx(hook);
            }
            hook = replacement;
            #[cfg(feature = "latency-probe")]
            probe::RENEWED.fetch_add(1, Ordering::Relaxed);
            if let Some(lock) = STATE.get() {
                if let Ok(mut state) = lock.lock() {
                    if (8..=255).any(|key| win_input::GetAsyncKeyState(key) < 0) {
                        continue;
                    }
                    // A release lost with the old hook must not leave recording
                    // or an unrelated physical key latched forever.
                    let recording = state.recording.reset().then_some(Edge::Release);
                    let cancel = state.cancel.reset().then_some(Edge::Release);
                    if recording.is_some() || cancel.is_some() {
                        let _ = state.send.send(HookEvent {
                            mask: false,
                            recording,
                            cancel,
                        });
                    }
                }
            }
        }
        KillTimer(0, timer);
        if hook != 0 {
            UnhookWindowsHookEx(hook);
        }
    });
    if !started.recv().unwrap_or(false) {
        return Err("Windows keyboard observer could not start.".into());
    }
    #[cfg(feature = "latency-probe")]
    probe::start(app.clone());
    std::thread::spawn(move || {
        for event in receive {
            #[cfg(feature = "latency-probe")]
            probe::RECEIVED.fetch_add(1, Ordering::Relaxed);
            // Mask bare Win/Alt release without changing Ctrl/Shift state.
            // This worker can wait for SendInput while the hook keeps pumping.
            if event.mask {
                #[cfg(feature = "latency-probe")]
                probe::MASK_STARTED.fetch_add(1, Ordering::Relaxed);
                win_input::send(&[win_input::key(0xe8, false), win_input::key(0xe8, true)]);
                #[cfg(feature = "latency-probe")]
                probe::MASK_FINISHED.fetch_add(1, Ordering::Relaxed);
            }
            if let Some(edge) = event.recording {
                super::shortcut_event(&app, false, edge == Edge::Press);
            }
            if let Some(edge) = event.cancel {
                super::shortcut_event(&app, true, edge == Edge::Press);
            }
        }
    });
    Ok(())
}
