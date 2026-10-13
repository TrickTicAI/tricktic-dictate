//! macOS delivery never types transcript characters: it publishes UTF-8 text
//! through NSPasteboard and sends Cmd+V only to the captured application.
use super::delivery::{Guards, Outcome, Target};
use super::macos_clipboard::{self, Published, Snapshot};
use crate::storage::Result;
use core_graphics::{
    event::{CGEvent, CGEventFlags},
    event_source::{CGEventSource, CGEventSourceStateID},
};
use objc2::rc::autoreleasepool;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSPasteboard, NSRunningApplication, NSWorkspace,
};
use objc2_foundation::NSString;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGEventSourceFlagsState(state: CGEventSourceStateID) -> u64;
}
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}
pub(super) const ACCESS: &str = concat!(
    "Copied to clipboard. Enable ",
    product_name!(),
    " in System Settings → Privacy & Security → Accessibility to paste automatically."
);
pub(super) fn secure_input() -> bool {
    unsafe { IsSecureEventInputEnabled() }
}
fn permission() -> bool {
    super::macos_hook::trusted(false) && unsafe { CGPreflightPostEventAccess() }
}
fn identity(app: &NSRunningApplication) -> Option<Target> {
    let process = app.processIdentifier();
    if process <= 0 || app.isTerminated() {
        return None;
    }
    // PID plus launch time rejects a quit/relaunch even if the PID is reused.
    // `window` is an opaque native identity on Mac, not an AX element pointer.
    let window = app.launchDate()?.timeIntervalSince1970().to_bits() as isize;
    Some(Target {
        window,
        process: process as u32,
    })
}
pub(super) fn foreground() -> Option<Target> {
    autoreleasepool(|_| {
        identity(
            NSWorkspace::sharedWorkspace()
                .frontmostApplication()?
                .as_ref(),
        )
    })
}
pub(super) fn alive(target: Target) -> bool {
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(target.process as i32)
            .is_some_and(|app| identity(&app) == Some(target))
    })
}
pub(super) fn application_name(target: Target) -> Option<String> {
    autoreleasepool(|_| {
        let app =
            NSRunningApplication::runningApplicationWithProcessIdentifier(target.process as i32)?;
        if identity(&app) != Some(target) {
            return None;
        }
        Some(format!(
            "{} ({})",
            app.localizedName()?,
            app.bundleIdentifier()?
        ))
    })
}
/// The application's own name, as the Dock shows it: never a window title,
/// which can name a document.
pub(super) fn display_name(target: Target) -> Option<String> {
    autoreleasepool(|_| {
        let app =
            NSRunningApplication::runningApplicationWithProcessIdentifier(target.process as i32)?;
        if identity(&app) != Some(target) {
            return None;
        }
        Some(app.localizedName()?.to_string())
    })
}
fn activate(target: Target) -> bool {
    autoreleasepool(|_| {
        let Some(app) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(target.process as i32)
        else {
            return false;
        };
        let Some(current) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return false;
        };
        // macOS 14+ may reject the legacy request from a background app.
        // Name the application relinquishing focus without activating Writer.
        // Activation is asynchronous; delivery still verifies actual focus.
        identity(&app) == Some(target)
            && app
                .activateFromApplication_options(&current, NSApplicationActivationOptions::empty())
    })
}
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(super) fn clipboard(text: &str) -> Result<isize> {
    // An explicit tray copy invalidates any pending paste just like an
    // external copy. It must not wait behind a held dictation shortcut.
    write(text)
}
fn write(text: &str) -> Result<isize> {
    write_with_privacy(text, false)
}
fn write_with_privacy(text: &str, private: bool) -> Result<isize> {
    macos_clipboard::write(text, private).ok_or_else(|| {
        "Clipboard copy failed or changed during copy. The transcript is saved in history.".into()
    })
}
pub(super) fn read_clipboard() -> Result<String> {
    autoreleasepool(|_| {
        let board = NSPasteboard::generalPasteboard();
        let before = board.changeCount();
        let value = board.stringForType(&NSString::from_str("public.utf8-plain-text"));
        if value.as_ref().is_some_and(|v| v.length() > 65_536) {
            return Err("Clipboard context is too large.".into());
        }
        let text = value.map(|s| s.to_string()).unwrap_or_default();
        if text.len() > 65_536 {
            return Err("Clipboard context is too large.".into());
        }
        if board.changeCount() != before {
            return Err("Clipboard changed during context capture. Try again.".into());
        }
        Ok(text)
    })
}
fn clipboard_current(written: isize) -> bool {
    autoreleasepool(|_| {
        super::input_guard::clipboard_unchanged(
            written,
            NSPasteboard::generalPasteboard().changeCount(),
        )
    })
}
const FN: u64 = 1 << 23;
const PASTE_MODIFIERS: u64 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | FN;
fn modifiers_released(session: u64, hardware: u64) -> bool {
    // The session tap consumes configured Fn edges. When focus changes while
    // Fn is held, the combined session flag can remain set after physical
    // release (MW-66). Hardware state observes both edges before that tap.
    // Keep session state for the other modifiers too: posted Command/Control/
    // Option/Shift must still prevent a paste, as must every physical modifier.
    ((session & !FN) | hardware) & PASTE_MODIFIERS == 0
}
fn keys_released() -> bool {
    unsafe {
        modifiers_released(
            CGEventSourceFlagsState(CGEventSourceStateID::CombinedSessionState),
            CGEventSourceFlagsState(CGEventSourceStateID::HIDSystemState),
        )
    }
}
fn paste_events() -> Option<(CGEvent, CGEvent)> {
    let source = CGEventSource::new(CGEventSourceStateID::Private).ok()?;
    let down = CGEvent::new_keyboard_event(source.clone(), 9, true).ok()?;
    let up = CGEvent::new_keyboard_event(source, 9, false).ok()?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);
    up.set_flags(CGEventFlags::CGEventFlagCommand);
    Some((down, up))
}
fn send_paste(target: Target) -> bool {
    let Some((down, up)) = paste_events() else {
        return false;
    };
    // PID-directed posting bounds the residual focus race: a late focus change
    // cannot redirect these key events to a different application.
    down.post_to_pid(target.process as i32);
    up.post_to_pid(target.process as i32);
    true // Event posting has no acknowledgement that an app inserted text.
}

#[derive(Default)]
struct Checks {
    wanted: bool,
    permission: bool,
    secure: bool,
    alive: bool,
    clipboard: bool,
    keys: bool,
    selection: bool,
}
impl Checks {
    fn refusal(&self) -> Option<&'static str> {
        if !self.wanted {
            Some(concat!(
                "Copied to clipboard. The dictation was discarded or ",
                product_name!(),
                " is quitting, so it was not pasted."
            ))
        } else if !self.permission {
            Some(ACCESS)
        } else if self.secure {
            Some("Copied to clipboard. Secure input is active, so automatic paste was skipped. Paste manually where appropriate.")
        } else if !self.alive {
            Some("Copied to clipboard. The application you dictated into has closed, so automatic paste was skipped.")
        } else if !self.clipboard {
            Some("Clipboard changed after transcription. Automatic paste was skipped; your transcript is saved in history.")
        } else if !self.keys {
            Some("Copied to clipboard. Release shortcut keys and paste manually.")
        } else if !self.selection {
            Some("Copied to clipboard. The selected text or focus changed, so replacement was skipped.")
        } else {
            None
        }
    }
}
pub(super) fn macos_deliver(
    _owner: isize,
    target: Option<Target>,
    text: &str,
    preserve_clipboard: bool,
    private_clipboard: bool,
    guards: &Guards,
) -> Result<Outcome> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    deliver(
        &Native,
        target,
        text,
        preserve_clipboard,
        private_clipboard,
        guards,
    )
}

trait Desktop {
    fn write(&self, text: &str, private: bool) -> Result<isize>;
    fn permission(&self) -> bool;
    fn secure(&self) -> bool;
    fn keys_released(&self) -> bool;
    fn clipboard_current(&self, generation: isize) -> bool;
    fn alive(&self, target: Target) -> bool;
    fn foreground(&self) -> Option<Target>;
    fn activate(&self, target: Target) -> bool;
    fn send_paste(&self, target: Target) -> bool;
    fn field_secure(&self, target: Target) -> bool;
    fn snapshot(&self) -> Option<Snapshot> {
        None
    }
    fn publish(&self, _text: &str, _generation: isize, _private: bool) -> Option<Published> {
        None
    }
    fn materialize(&self, _text: &str, _generation: isize, _private: bool) -> bool {
        false
    }
    fn restore(&self, _previous: &Snapshot, _generation: isize) -> bool {
        false
    }
    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
struct Native;
impl Desktop for Native {
    fn field_secure(&self, target: Target) -> bool {
        super::context::secure_field(target)
    }
    fn write(&self, text: &str, private: bool) -> Result<isize> {
        write_with_privacy(text, private)
    }
    fn permission(&self) -> bool {
        permission()
    }
    fn secure(&self) -> bool {
        secure_input()
    }
    fn keys_released(&self) -> bool {
        keys_released()
    }
    fn clipboard_current(&self, generation: isize) -> bool {
        clipboard_current(generation)
    }
    fn alive(&self, target: Target) -> bool {
        alive(target)
    }
    fn foreground(&self) -> Option<Target> {
        foreground()
    }
    fn activate(&self, target: Target) -> bool {
        activate(target)
    }
    fn send_paste(&self, target: Target) -> bool {
        send_paste(target)
    }
    fn snapshot(&self) -> Option<Snapshot> {
        macos_clipboard::snapshot()
    }
    fn publish(&self, text: &str, generation: isize, private: bool) -> Option<Published> {
        macos_clipboard::publish(text, generation, private)
    }
    fn materialize(&self, text: &str, generation: isize, private: bool) -> bool {
        macos_clipboard::materialize(text, generation, private)
    }
    fn restore(&self, previous: &Snapshot, generation: isize) -> bool {
        macos_clipboard::restore(previous, generation)
    }
}
/// How a wait on Writer's lazily provided dictation ended.
#[derive(Debug, PartialEq)]
enum Waited {
    Read,
    /// Someone copied since: their copy stays, and Writer touches nothing more.
    Replaced,
    TimedOut,
    /// The dictation was discarded, Writer is quitting, or a newer one started.
    Interrupted,
}
fn wait_for_read(
    os: &impl Desktop,
    guards: &Guards,
    published: &Published,
    limit: Duration,
) -> Waited {
    // Counted steps rather than a deadline, so tests need no real waiting.
    for _ in 0..limit.as_millis() / 10 {
        if published.read.load(Ordering::Acquire) {
            return Waited::Read;
        }
        if !os.clipboard_current(published.generation) {
            return Waited::Replaced;
        }
        if !(guards.wanted)() || (guards.superseded)() {
            return Waited::Interrupted;
        }
        os.pause(Duration::from_millis(10));
    }
    if published.read.load(Ordering::Acquire) {
        Waited::Read
    } else {
        Waited::TimedOut
    }
}
/// After Cmd+V, restores the previous pasteboard once the paste has read the
/// dictation. Before restoring, the dictation is offered once more so a
/// clipboard history (CopyQ, Spotlight) can record it: the previous copy then
/// lands first in history and the dictation second.
fn preserve_after_paste(
    os: &impl Desktop,
    guards: &Guards,
    text: &str,
    previous: &Snapshot,
    published: Published,
    private_clipboard: bool,
) {
    match wait_for_read(os, guards, &published, READ_LIMIT) {
        Waited::Read => {}
        Waited::Replaced => return,
        // No observed paste: leave the dictation for a late or manual paste.
        Waited::TimedOut | Waited::Interrupted => {
            os.materialize(text, published.generation, private_clipboard);
            return;
        }
    }
    // A reader can hold its request briefly after the provider answered.
    os.pause(SETTLE);
    let mut generation = published.generation;
    if !private_clipboard && (guards.wanted)() && !(guards.superseded)() {
        let Some(offered) = os.publish(text, generation, private_clipboard) else {
            return;
        };
        match wait_for_read(os, guards, &offered, HISTORY_LIMIT) {
            Waited::Replaced => return,
            Waited::Read | Waited::TimedOut | Waited::Interrupted => {
                generation = offered.generation
            }
        }
    }
    os.restore(previous, generation);
}
const READ_LIMIT: Duration = Duration::from_secs(2);
const SETTLE: Duration = Duration::from_millis(100);
const HISTORY_LIMIT: Duration = Duration::from_millis(1500);
fn deliver(
    os: &impl Desktop,
    target: Option<Target>,
    text: &str,
    preserve_clipboard: bool,
    private_clipboard: bool,
    guards: &Guards,
) -> Result<Outcome> {
    if !(guards.wanted)() {
        return Err("The dictation was discarded before its text was inserted.".into());
    }
    // Captured under the delivery lock immediately before publishing, so a
    // copy made while recording or transcribing is the one preserved.
    let previous = preserve_clipboard.then(|| os.snapshot()).flatten();
    let generation = os.write(text, private_clipboard)?;
    (guards.mark)("clipboard_written");
    let fallback = |message: &str| {
        Ok(Outcome {
            copied: true,
            pasted: false,
            message: message.into(),
        })
    };
    if !os.permission() {
        return fallback(ACCESS);
    }
    if os.secure() {
        return fallback(
            "Copied to clipboard. Secure input is active, so automatic paste was skipped.",
        );
    }
    let Some(target) = target.filter(|t| t.process != std::process::id()) else {
        return fallback("Copied to clipboard. Start with the global shortcut in another app for automatic paste.");
    };
    let started = Instant::now();
    let mut hinted = false;
    while !os.keys_released() {
        let limit = if (guards.recording_newer)() { 900 } else { 6 };
        if !(guards.wanted)()
            || !os.permission()
            || os.secure()
            || !os.clipboard_current(generation)
            || started.elapsed() >= Duration::from_secs(limit)
        {
            break;
        }
        if !hinted && started.elapsed() >= Duration::from_millis(150) {
            (guards.waiting)();
            hinted = true;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    (guards.mark)("modifier_wait_finished");
    let checks = || Checks {
        wanted: (guards.wanted)(),
        permission: os.permission(),
        secure: os.secure(),
        alive: os.alive(target),
        clipboard: os.clipboard_current(generation),
        keys: os.keys_released(),
        selection: (guards.target_valid)(),
    };
    if let Some(reason) = checks().refusal() {
        return fallback(reason);
    }
    (guards.mark)("keys_released");
    if os.foreground() != Some(target) {
        if (guards.superseded)() {
            return fallback("Copied to clipboard. A newer dictation started in another app, so automatic paste was skipped.");
        }
        if !os.activate(target) {
            return fallback("Copied to clipboard. macOS would not return focus to the target application; paste manually.");
        }
        let started = Instant::now();
        while os.foreground() != Some(target) && started.elapsed() < Duration::from_millis(500) {
            if !(guards.wanted)() || (guards.superseded)() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    (guards.mark)("foreground_ready");
    if let Some(reason) = checks().refusal() {
        return fallback(reason);
    }
    if os.foreground() != Some(target) {
        return fallback(
            "Copied to clipboard. Application focus changed, so automatic paste was skipped.",
        );
    }
    if os.field_secure(target) {
        return fallback(
            "Copied to clipboard. The focused field is secure, so automatic paste was skipped.",
        );
    }
    // Context validation can perform IPC; recheck volatile guards after it.
    if !os.permission() {
        return fallback(ACCESS);
    }
    if !(guards.wanted)()
        || os.secure()
        || !os.alive(target)
        || !os.clipboard_current(generation)
        || !os.keys_released()
        || os.foreground() != Some(target)
    {
        return fallback(
            "Copied to clipboard. Input, clipboard or focus changed before paste; paste manually.",
        );
    }
    let published = previous
        .as_ref()
        .and_then(|_| os.publish(text, generation, private_clipboard));
    let pasted = os.send_paste(target);
    (guards.mark)("paste_sent");
    if let (Some(previous), Some(published)) = (&previous, published) {
        if pasted {
            preserve_after_paste(os, guards, text, previous, published, private_clipboard);
        } else {
            os.materialize(text, published.generation, private_clipboard);
        }
    }
    Ok(Outcome {
        copied: true,
        pasted,
        message: if pasted {
            "Copied; paste sent to the active application."
        } else {
            "Copied to clipboard. Automatic paste could not be sent; paste manually."
        }
        .into(),
    })
}

// Explicit activation-only acceptance probe: no clipboard reads or key events.
// The caller requires the probe feature, isolated root and fixture marker.
#[cfg(feature = "latency-probe")]
pub(super) fn probe_activation(app: &tauri::AppHandle) {
    use tauri::Manager;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        let mut results = Vec::new();
        for bundle in ["com.apple.Safari", "com.apple.TextEdit"] {
            let target = autoreleasepool(|_| {
                let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                    &NSString::from_str(bundle),
                );
                identity(apps.firstObject()?.as_ref())
            });
            let requested = target.is_some_and(activate);
            let started = Instant::now();
            while target.is_some()
                && foreground() != target
                && started.elapsed() < Duration::from_millis(500)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            results.push(serde_json::json!({"fixture": bundle,
                "requested": requested, "foreground_after": target.is_some() && foreground() == target}));
        }
        let root = &app.state::<super::Shell>().root;
        let _ = crate::storage::atomic_json(&root.join("macos-activation-probe.json"), &results);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::sync::{atomic::AtomicBool, Arc};
    const TARGET: Target = Target {
        window: 123,
        process: 987654,
    };
    struct Fake {
        text: RefCell<String>,
        generation: Cell<isize>,
        trusted: Cell<bool>,
        secure: Cell<bool>,
        alive: Cell<bool>,
        focused: Cell<bool>,
        session_flags: Cell<u64>,
        hardware_flags: Cell<u64>,
        sent: Cell<usize>,
        activated: Cell<usize>,
        change_during_wait: Cell<bool>,
        revoke_on_activation: Cell<bool>,
        revoke_on_field_check: Cell<bool>,
        field_secure: Cell<bool>,
        previous: RefCell<Option<Snapshot>>,
        /// Whether each lazy publication, in order, is read while Writer waits.
        reads: RefCell<std::collections::VecDeque<bool>>,
        published: RefCell<Vec<Arc<AtomicBool>>>,
        copy_on_pause: Cell<Option<usize>>,
        pauses: Cell<usize>,
        restored: RefCell<Option<Snapshot>>,
        materialized: Cell<usize>,
        policies: RefCell<Vec<bool>>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                text: RefCell::new(String::new()),
                generation: Cell::new(0),
                trusted: Cell::new(true),
                secure: Cell::new(false),
                alive: Cell::new(true),
                focused: Cell::new(true),
                session_flags: Cell::new(0),
                hardware_flags: Cell::new(0),
                sent: Cell::new(0),
                activated: Cell::new(0),
                change_during_wait: Cell::new(false),
                revoke_on_activation: Cell::new(false),
                revoke_on_field_check: Cell::new(false),
                field_secure: Cell::new(false),
                previous: RefCell::new(None),
                reads: RefCell::new(Default::default()),
                published: RefCell::new(Vec::new()),
                copy_on_pause: Cell::new(None),
                pauses: Cell::new(0),
                restored: RefCell::new(None),
                materialized: Cell::new(0),
                policies: RefCell::new(Vec::new()),
            }
        }
    }
    impl Desktop for Fake {
        fn field_secure(&self, _: Target) -> bool {
            if self.revoke_on_field_check.get() {
                self.trusted.set(false);
            }
            self.field_secure.get()
        }
        fn write(&self, text: &str, private: bool) -> Result<isize> {
            self.policies.borrow_mut().push(private);
            self.text.replace(text.into());
            self.generation.set(42);
            Ok(42)
        }
        fn permission(&self) -> bool {
            self.trusted.get()
        }
        fn secure(&self) -> bool {
            self.secure.get()
        }
        fn keys_released(&self) -> bool {
            if self.change_during_wait.replace(false) {
                self.generation.set(43);
                self.text.replace("foreign copy".into());
                return false;
            }
            modifiers_released(self.session_flags.get(), self.hardware_flags.get())
        }
        fn clipboard_current(&self, generation: isize) -> bool {
            generation == self.generation.get()
        }
        fn alive(&self, _: Target) -> bool {
            self.alive.get()
        }
        fn foreground(&self) -> Option<Target> {
            self.focused.get().then_some(TARGET)
        }
        fn activate(&self, _: Target) -> bool {
            self.activated.set(self.activated.get() + 1);
            self.focused.set(true);
            if self.revoke_on_activation.get() {
                self.trusted.set(false);
            }
            true
        }
        fn send_paste(&self, target: Target) -> bool {
            assert_eq!(target, TARGET);
            self.sent.set(self.sent.get() + 1);
            true
        }
        fn snapshot(&self) -> Option<Snapshot> {
            self.previous.borrow().clone()
        }
        fn publish(&self, text: &str, generation: isize, private: bool) -> Option<Published> {
            if !self.clipboard_current(generation) {
                return None;
            }
            let read = Arc::new(AtomicBool::new(
                self.reads.borrow_mut().pop_front().unwrap_or(false),
            ));
            self.published.borrow_mut().push(read.clone());
            self.policies.borrow_mut().push(private);
            self.text.replace(text.into());
            self.generation.set(generation + 1);
            Some(Published {
                generation: generation + 1,
                read,
                _provider: None,
            })
        }
        fn materialize(&self, _: &str, generation: isize, private: bool) -> bool {
            self.policies.borrow_mut().push(private);
            self.materialized.set(self.materialized.get() + 1);
            self.clipboard_current(generation)
        }
        fn restore(&self, previous: &Snapshot, generation: isize) -> bool {
            if !self.clipboard_current(generation) {
                return false;
            }
            self.restored.replace(Some(previous.clone()));
            self.generation.set(generation + 1);
            true
        }
        fn pause(&self, _: Duration) {
            self.pauses.set(self.pauses.get() + 1);
            if self.copy_on_pause.get() == Some(self.pauses.get()) {
                self.generation.set(900);
                self.text.replace("newer user copy".into());
            }
        }
    }
    fn previous() -> Snapshot {
        vec![vec![
            ("public.utf8-plain-text".into(), "copied before".into()),
            ("public.html".into(), b"<b>copied</b>".to_vec()),
        ]]
    }
    fn guards() -> Guards<'static> {
        Guards {
            wanted: &|| true,
            superseded: &|| false,
            recording_newer: &|| false,
            waiting: &|| {},
            target_valid: &|| true,
            mark: &|_| {},
        }
    }
    #[test]
    fn native_paste_events_only_contain_command_v_never_return_or_send() {
        use core_graphics::event::{CGEventType, EventField};
        let (down, up) = paste_events().unwrap();
        assert!(matches!(down.get_type(), CGEventType::KeyDown));
        assert!(matches!(up.get_type(), CGEventType::KeyUp));
        for event in [down, up] {
            assert_eq!(
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE),
                9
            );
            assert_eq!(event.get_flags(), CGEventFlags::CGEventFlagCommand);
        }
        // Events were constructed and inspected; none are posted by this test.
    }
    #[test]
    fn stale_session_fn_does_not_delay_paste_in_either_focus_sequence() {
        for session in [0, FN] {
            let os = Fake::default();
            os.session_flags.set(session);
            let mut g = guards();
            g.waiting = &|| panic!("released physical Fn must not display Release keys");
            let result = deliver(&os, Some(TARGET), "fixture", false, false, &g).unwrap();
            assert!(result.pasted);
            assert_eq!(os.sent.get(), 1);
            assert_eq!(os.activated.get(), 0);
        }
    }
    #[test]
    fn every_physical_modifier_and_other_session_modifiers_still_guard_paste() {
        for bit in [17, 18, 19, 20, 23] {
            assert!(!modifiers_released(0, 1 << bit));
            assert!(!modifiers_released(FN, 1 << bit));
            if bit != 23 {
                assert!(!modifiers_released(1 << bit, 0));
            }
        }
        // Caps Lock, numeric pad and unrelated flags are not held shortcuts.
        assert!(modifiers_released((1 << 16) | FN, (1 << 16) | (1 << 21)));
    }
    #[test]
    fn held_physical_fn_waits_for_release_even_with_a_newer_recording() {
        for newer in [false, true] {
            let os = Fake::default();
            // Consumed Fn-down can leave the session flag clear while held.
            os.hardware_flags.set(FN);
            let waited = Cell::new(false);
            let release = || {
                assert_eq!(os.sent.get(), 0);
                assert_eq!(os.activated.get(), 0);
                waited.set(true);
                os.hardware_flags.set(0);
                os.session_flags.set(FN); // stale after release
            };
            let mut g = guards();
            g.waiting = &release;
            let recording = || newer;
            g.recording_newer = &recording;
            g.superseded = &recording;
            let result = deliver(&os, Some(TARGET), "fixture", false, false, &g).unwrap();
            assert!(waited.get() && result.pasted);
            assert_eq!(os.sent.get(), 1);
        }
    }
    #[test]
    fn interrupted_modifier_wait_does_not_report_keys_released() {
        let os = Fake::default();
        os.hardware_flags.set(FN);
        let canceled = Cell::new(false);
        let cancel = || canceled.set(true);
        let wanted = || !canceled.get();
        let stages = RefCell::new(Vec::new());
        let mark = |stage| stages.borrow_mut().push(stage);
        let mut g = guards();
        g.waiting = &cancel;
        g.wanted = &wanted;
        g.mark = &mark;
        let result = deliver(&os, Some(TARGET), "fixture", false, false, &g).unwrap();
        assert!(result.copied && !result.pasted);
        assert!(stages.borrow().contains(&"modifier_wait_finished"));
        assert!(!stages.borrow().contains(&"keys_released"));
    }
    #[test]
    fn language_text_is_copied_unchanged_and_target_reactivated_once() {
        for text in ["Hello world", "שלום עולם", "בדיקת Writer באנגלית"] {
            let os = Fake::default();
            os.focused.set(false);
            let result = deliver(&os, Some(TARGET), text, false, false, &guards()).unwrap();
            assert!(result.copied && result.pasted);
            assert_eq!(*os.text.borrow(), text);
            assert_eq!(os.activated.get(), 1);
            assert_eq!(os.sent.get(), 1);
        }
    }
    #[test]
    fn foreign_copy_during_modifier_wait_is_preserved_without_paste() {
        let os = Fake::default();
        os.change_during_wait.set(true);
        let result = deliver(&os, Some(TARGET), "dictation", false, false, &guards()).unwrap();
        assert!(!result.pasted);
        assert!(result.message.contains("Clipboard changed"));
        assert_eq!(*os.text.borrow(), "foreign copy");
        assert_eq!(os.sent.get(), 0);
    }
    #[test]
    fn revocation_during_activation_refuses_with_settings_pane() {
        let os = Fake::default();
        os.focused.set(false);
        os.revoke_on_activation.set(true);
        let result = deliver(&os, Some(TARGET), "retained", false, false, &guards()).unwrap();
        assert!(result.copied && !result.pasted);
        assert_eq!(result.message, ACCESS);
        assert_eq!(*os.text.borrow(), "retained");
        assert_eq!(os.sent.get(), 0);
    }
    #[test]
    fn denied_secure_and_quit_targets_copy_without_activating_or_pasting() {
        for case in 0..3 {
            let os = Fake::default();
            os.focused.set(false);
            match case {
                0 => os.trusted.set(false),
                1 => os.secure.set(true),
                _ => os.alive.set(false),
            }
            let result = deliver(&os, Some(TARGET), "retained", false, false, &guards()).unwrap();
            assert!(result.copied && !result.pasted);
            assert_eq!(*os.text.borrow(), "retained");
            assert_eq!(os.sent.get(), 0);
            assert_eq!(os.activated.get(), 0);
        }
    }
    #[test]
    fn stale_sessions_never_write_and_newer_sessions_never_lose_focus() {
        let os = Fake::default();
        let mut g = guards();
        g.wanted = &|| false;
        assert!(deliver(&os, Some(TARGET), "stale", false, false, &g).is_err());
        assert!(os.text.borrow().is_empty());
        g.wanted = &|| true;
        g.superseded = &|| true;
        os.focused.set(false);
        let result = deliver(&os, Some(TARGET), "older", false, false, &g).unwrap();
        assert!(!result.pasted);
        assert_eq!(os.activated.get(), 0);
        assert_eq!(os.sent.get(), 0);
    }
    #[test]
    fn command_selection_changed_after_activation_never_pastes() {
        let os = Fake::default();
        os.focused.set(false);
        let valid = || os.activated.get() == 0;
        let mut g = guards();
        g.target_valid = &valid;
        let result = deliver(&os, Some(TARGET), "replacement", false, false, &g).unwrap();
        assert!(!result.pasted);
        assert!(result.message.contains("selected text"));
        assert_eq!(os.sent.get(), 0);
    }
    #[test]
    fn secure_field_and_last_moment_permission_revocation_refuse_paste() {
        for revoked in [false, true] {
            let os = Fake::default();
            os.field_secure.set(!revoked);
            os.revoke_on_field_check.set(revoked);
            let result = deliver(&os, Some(TARGET), "retained", false, false, &guards()).unwrap();
            assert!(result.copied && !result.pasted);
            assert_eq!(os.sent.get(), 0);
            if revoked {
                assert_eq!(result.message, ACCESS);
            } else {
                assert!(result.message.contains("field is secure"));
            }
        }
    }
    #[test]
    fn paste_read_offers_dictation_to_history_then_restores_previous() {
        let os = Fake::default();
        os.previous.replace(Some(previous()));
        os.reads.replace([true, true].into());
        let result = deliver(&os, Some(TARGET), "שלום dictation", true, false, &guards()).unwrap();
        assert!(result.pasted);
        // One lazy item for the paste, one for history, then the restore.
        assert_eq!(os.published.borrow().len(), 2);
        assert_eq!(*os.restored.borrow(), Some(previous()));
        assert_eq!(os.materialized.get(), 0);
    }
    #[test]
    fn restore_happens_without_a_history_reader() {
        let os = Fake::default();
        os.previous.replace(Some(previous()));
        os.reads.replace([true, false].into());
        deliver(&os, Some(TARGET), "dictation", true, false, &guards()).unwrap();
        assert_eq!(*os.restored.borrow(), Some(previous()));
    }
    #[test]
    fn unread_paste_keeps_dictation_as_ordinary_text() {
        let os = Fake::default();
        os.previous.replace(Some(previous()));
        let result = deliver(&os, Some(TARGET), "dictation", true, false, &guards()).unwrap();
        assert!(result.pasted);
        assert!(os.restored.borrow().is_none());
        assert_eq!(os.materialized.get(), 1);
        assert_eq!(*os.text.borrow(), "dictation");
    }
    #[test]
    fn newer_copy_while_waiting_is_never_overwritten() {
        for (reads, copy_at) in [(vec![false], 3), (vec![true, false], 15)] {
            let os = Fake::default();
            os.previous.replace(Some(previous()));
            os.reads.replace(reads.into());
            os.copy_on_pause.set(Some(copy_at));
            deliver(&os, Some(TARGET), "dictation", true, false, &guards()).unwrap();
            assert!(os.restored.borrow().is_none());
            assert_eq!(os.materialized.get(), 0);
            assert_eq!(*os.text.borrow(), "newer user copy");
        }
    }
    #[test]
    fn preference_off_or_nothing_to_preserve_publishes_nothing_lazily() {
        for (preserve, previous) in [(false, Some(previous())), (true, None)] {
            let os = Fake::default();
            os.previous.replace(previous);
            os.reads.replace([true, true].into());
            assert!(
                deliver(&os, Some(TARGET), "dictation", preserve, false, &guards())
                    .unwrap()
                    .pasted
            );
            assert!(os.published.borrow().is_empty());
            assert!(os.restored.borrow().is_none());
        }
    }
    #[test]
    fn refused_paste_keeps_dictation_without_lazy_publication() {
        let os = Fake::default();
        os.previous.replace(Some(previous()));
        os.secure.set(true);
        let result = deliver(&os, Some(TARGET), "dictation", true, false, &guards()).unwrap();
        assert!(!result.pasted);
        assert!(os.published.borrow().is_empty());
        assert!(os.restored.borrow().is_none());
        assert_eq!(*os.text.borrow(), "dictation");
    }
    #[test]
    fn newer_dictation_after_the_read_restores_without_waiting_for_history() {
        let os = Fake::default();
        os.previous.replace(Some(previous()));
        os.reads.replace([true].into());
        let mut g = guards();
        let sent = std::cell::Cell::new(false);
        let superseded = || sent.get();
        g.superseded = &superseded;
        let mark = |stage: &str| {
            if stage == "paste_sent" {
                sent.set(true)
            }
        };
        g.mark = &mark;
        deliver(&os, Some(TARGET), "dictation", true, false, &g).unwrap();
        assert_eq!(os.published.borrow().len(), 1);
        assert_eq!(*os.restored.borrow(), Some(previous()));
    }
    #[test]
    fn private_delivery_restores_without_offering_dictation_to_history() {
        let os = Fake::default();
        os.previous.replace(Some(previous()));
        os.reads.replace([true].into());
        assert!(
            deliver(&os, Some(TARGET), "private", true, true, &guards())
                .unwrap()
                .pasted
        );
        assert_eq!(os.published.borrow().len(), 1);
        assert_eq!(*os.policies.borrow(), [true, true]);
        assert_eq!(*os.restored.borrow(), Some(previous()));
    }
    #[test]
    fn private_delivery_preserves_policy_in_unread_and_permission_fallbacks() {
        for trusted in [true, false] {
            let os = Fake::default();
            os.previous.replace(Some(previous()));
            os.trusted.set(trusted);
            let result = deliver(&os, Some(TARGET), "private", true, true, &guards()).unwrap();
            assert_eq!(result.pasted, trusted);
            assert!(result.copied);
            assert_eq!(os.materialized.get(), usize::from(trusted));
            assert!(os.restored.borrow().is_none());
            assert!(os.policies.borrow().iter().all(|private| *private));
            assert_eq!(*os.text.borrow(), "private");
        }
    }
    #[test]
    fn private_delivery_leaves_concurrent_copies_untouched() {
        for (read, copy_at) in [(false, 3), (true, 1)] {
            let os = Fake::default();
            os.previous.replace(Some(previous()));
            os.reads.replace([read].into());
            os.copy_on_pause.set(Some(copy_at));
            deliver(&os, Some(TARGET), "private", true, true, &guards()).unwrap();
            assert!(os.restored.borrow().is_none());
            assert_eq!(os.materialized.get(), 0);
            assert_eq!(*os.text.borrow(), "newer user copy");
        }
    }
    #[test]
    #[ignore = "Writes synthetic text to the real Mac pasteboard; run explicitly for native acceptance"]
    fn native_pasteboard_roundtrips_all_three_language_cases() {
        for text in ["MW-50 English", "בדיקת עברית", "MW-50 בדיקה mixed"] {
            let generation = clipboard(text).unwrap();
            assert!(clipboard_current(generation));
            assert_eq!(read_clipboard().unwrap(), text);
        }
    }
}
