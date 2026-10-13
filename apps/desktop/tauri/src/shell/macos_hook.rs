//! Active, Accessibility-gated event tap. The callback only tracks physical
//! state and queues edges; capture, IPC and permissions never run in it.
use super::{
    hotkey_state::{ChordState, Edge},
    macos_keys, preferences, Shell,
};
use core_foundation::{
    base::TCFType,
    boolean::CFBoolean,
    dictionary::CFDictionary,
    runloop::{kCFRunLoopDefaultMode, CFRunLoop},
    string::{CFString, CFStringRef},
};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Mutex, OnceLock,
};
use tauri::{Emitter, Manager};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CAPTURING: AtomicBool = AtomicBool::new(false);
static DISABLED: AtomicBool = AtomicBool::new(false);
static STATE: OnceLock<Mutex<HookState>> = OnceLock::new();
struct HookState {
    states: [ChordState; 2],
    chords: [Vec<i32>; 2],
    send: mpsc::Sender<Message>,
}
enum Message {
    /// Cancel chord, pressed, and when the tap saw it.
    Edge(bool, bool, std::time::Instant),
    Capture(bool, u64),
}
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
        -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}
pub fn trusted(prompt: bool) -> bool {
    // The system constant is borrowed; retain it for the dictionary lifetime.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(prompt))]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}
pub fn capturing(value: bool) {
    CAPTURING.store(value, Ordering::Release);
    reset();
}
fn reset() {
    if let Some(state) = STATE.get() {
        let mut s = state.lock().unwrap();
        for index in 0..2 {
            if s.states[index].reset() {
                let _ = s
                    .send
                    .send(Message::Edge(index == 1, false, std::time::Instant::now()));
            }
        }
    }
}
pub fn configure(p: &preferences::Preferences) {
    if let Some(state) = STATE.get() {
        let mut s = state.lock().unwrap();
        let chords = [&p.recording, &p.cancel].map(|c| macos_keys::keys(c).unwrap_or_default());
        if s.chords != chords {
            s.chords = chords;
            s.states = Default::default();
        }
    }
}
fn modifier_down(code: u16, flags: u64) -> bool {
    let mask = match code {
        63 => 1 << 23,
        55 => 8,
        54 => 16,
        56 => 2,
        60 => 4,
        59 => 1,
        62 => 0x2000,
        58 => 0x20,
        61 => 0x40,
        _ => 0,
    };
    flags & mask != 0
}
fn chord_edge(
    state: &mut ChordState,
    key: i32,
    pressed: bool,
    modifiers: &[i32],
    chord: &[i32],
) -> Option<Edge> {
    // Fn supplies the F-key on media-key keyboards. Chords without explicit
    // Fn retain Carbon's behavior and do not treat it as an extra modifier.
    if key == macos_keys::FN && !chord.contains(&macos_keys::FN) {
        return None;
    }
    state.update_with_modifiers(key, pressed, modifiers, chord)
}
pub fn start(app: tauri::AppHandle, p: &preferences::Preferences) {
    let (send, receive) = mpsc::channel();
    if STATE
        .set(Mutex::new(HookState {
            states: Default::default(),
            chords: Default::default(),
            send,
        }))
        .is_err()
    {
        return;
    }
    configure(p);
    let delivery = app.clone();
    std::thread::spawn(move || {
        for message in receive {
            match message {
                Message::Edge(cancel, pressed, at) => {
                    super::shortcut_event_at(&delivery, cancel, pressed, at)
                }
                Message::Capture(pressed, flags) => {
                    if CAPTURING.load(Ordering::Acquire)
                        && delivery
                            .get_webview_window("main")
                            .is_some_and(|w| w.is_focused().unwrap_or(false))
                    {
                        let _ = delivery.emit_to("main", "writer-fn-capture", serde_json::json!({"pressed":pressed,"ctrl": flags & (1<<18) != 0,"alt": flags & (1<<19) != 0,"shift":flags & (1<<17) != 0,"meta":flags & (1<<20) != 0}));
                    }
                }
            }
        }
    });
    std::thread::spawn(move || {
        while !app.state::<Shell>().quitting.load(Ordering::Acquire) {
            if !trusted(false) {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![
                    CGEventType::FlagsChanged,
                    CGEventType::KeyDown,
                    CGEventType::KeyUp,
                ],
                |_, kind, event| {
                    if matches!(
                        kind,
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                    ) {
                        DISABLED.store(true, Ordering::Release);
                        return CallbackResult::Keep;
                    }
                    let code =
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                    let flags = event.get_flags().bits();
                    let pressed = if matches!(kind, CGEventType::FlagsChanged) {
                        modifier_down(code, flags)
                    } else {
                        matches!(kind, CGEventType::KeyDown)
                    };
                    let Some(state) = STATE.get() else {
                        return CallbackResult::Keep;
                    };
                    let Ok(mut s) = state.try_lock() else {
                        DISABLED.store(true, Ordering::Release);
                        return CallbackResult::Keep;
                    };
                    if CAPTURING.load(Ordering::Acquire) {
                        if code == 63 {
                            let _ = s.send.send(Message::Capture(pressed, flags));
                            return CallbackResult::Drop;
                        }
                        return CallbackResult::Keep;
                    }
                    let key = macos_keys::physical(code);
                    let modifiers: Vec<_> = [54, 55, 56, 60, 59, 62, 58, 61]
                        .into_iter()
                        .filter(|code| modifier_down(*code, flags))
                        .map(macos_keys::physical)
                        .collect();
                    let mut consume = false;
                    for index in 0..2 {
                        let chord = s.chords[index].clone();
                        // Consume fn edges only when it is part of a configured
                        // chord. Other applications' key events pass unchanged.
                        consume |= code == 63 && chord.contains(&macos_keys::FN);
                        if let Some(edge) =
                            chord_edge(&mut s.states[index], key, pressed, &modifiers, &chord)
                        {
                            let _ = s.send.send(Message::Edge(
                                index == 1,
                                edge == Edge::Press,
                                std::time::Instant::now(),
                            ));
                        }
                    }
                    if consume {
                        CallbackResult::Drop
                    } else {
                        CallbackResult::Keep
                    }
                },
            );
            if let Ok(tap) = tap {
                if let Ok(source) = tap.mach_port().create_runloop_source(0) {
                    let runloop = CFRunLoop::get_current();
                    runloop.add_source(&source, unsafe { kCFRunLoopDefaultMode });
                    reset();
                    if app.state::<Shell>().pressed.load(Ordering::Acquire) {
                        super::shortcut_event(&app, false, false);
                    }
                    ACTIVE.store(true, Ordering::Release);
                    tap.enable();
                    while trusted(false) && !app.state::<Shell>().quitting.load(Ordering::Acquire) {
                        CFRunLoop::run_in_mode(
                            unsafe { kCFRunLoopDefaultMode },
                            std::time::Duration::from_millis(100),
                            false,
                        );
                        if DISABLED.swap(false, Ordering::AcqRel) {
                            reset();
                            tap.enable();
                        }
                    }
                    ACTIVE.store(false, Ordering::Release);
                    reset();
                    runloop.remove_source(&source, unsafe { kCFRunLoopDefaultMode });
                }
            }
            // Denial or tap creation failure is recoverable, never a launch error.
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn implicit_fn_does_not_break_legacy_f_key_chords() {
        let mut state = ChordState::default();
        let chord = macos_keys::keys("Ctrl+Shift+F9").unwrap();
        assert_eq!(chord_edge(&mut state, 0xa2, true, &[0xa2], &chord), None);
        assert_eq!(
            chord_edge(&mut state, 0xa0, true, &[0xa2, 0xa0], &chord),
            None
        );
        assert_eq!(
            chord_edge(&mut state, macos_keys::FN, true, &[0xa2, 0xa0], &chord),
            None
        );
        assert_eq!(
            chord_edge(&mut state, 120, true, &[0xa2, 0xa0], &chord),
            Some(Edge::Press)
        );
        assert_eq!(
            chord_edge(&mut state, 120, false, &[0xa2, 0xa0], &chord),
            None
        );
        assert_eq!(chord_edge(&mut state, 0xa2, false, &[0xa0], &chord), None);
        assert_eq!(
            chord_edge(&mut state, 0xa0, false, &[], &chord),
            Some(Edge::Release)
        );
    }
    #[test]
    fn explicit_fn_chords_need_fn_and_release_every_key() {
        let mut state = ChordState::default();
        let chord = macos_keys::keys("Fn+R").unwrap();
        assert_eq!(
            chord_edge(&mut state, macos_keys::FN, true, &[], &chord),
            None
        );
        assert_eq!(
            chord_edge(&mut state, 82, true, &[], &chord),
            Some(Edge::Press)
        );
        assert_eq!(
            chord_edge(&mut state, macos_keys::FN, false, &[], &chord),
            None
        );
        assert_eq!(
            chord_edge(&mut state, 82, false, &[], &chord),
            Some(Edge::Release)
        );
    }
    #[test]
    fn fn_and_both_sides_have_independent_flags() {
        assert!(modifier_down(63, 1 << 23));
        assert!(!modifier_down(63, 1 << 17));
        assert!(modifier_down(60, 4));
        assert!(!modifier_down(56, 4));
        assert!(modifier_down(62, 0x2000));
    }
}
