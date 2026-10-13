use super::delivery::{Guards, Outcome, Target};
use super::{input_guard, win_input};
use crate::storage::Result;
use std::time::{Duration, Instant};
#[link(name = "user32")]
extern "system" {
    fn GetClipboardSequenceNumber() -> u32;
    fn GetClipboardOwner() -> isize;
    fn OpenClipboard(owner: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
}
pub(super) unsafe fn put(format: u32, bytes: &[u8]) -> bool {
    super::clipboard_preservation::Allocation::copy(bytes)
        .is_some_and(|mut prepared| prepared.publish(format))
}
/// Any application can hold the clipboard; wait briefly rather than failing on
/// the first attempt.
pub(super) unsafe fn acquire(owner: isize) -> bool {
    for _ in 0..20 {
        if OpenClipboard(owner) != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}
/// Replaces an open clipboard with `text`, letting Windows clipboard history
/// keep it. The caller opens and closes the clipboard around this.
pub(super) unsafe fn write(text: &str) -> bool {
    write_with_privacy(text, false)
}
/// Publish exclusion metadata before text while the clipboard is exclusively
/// open. Privacy failures never fall back to an unmarked publication.
pub(super) unsafe fn write_with_privacy(text: &str, private: bool) -> bool {
    let units: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
    let bytes: Vec<_> = units.iter().flat_map(|v| v.to_le_bytes()).collect();
    let Some(mut prepared) = super::clipboard_preservation::Allocation::copy(&bytes) else {
        return false;
    };
    if EmptyClipboard() == 0 {
        return false;
    }
    if private {
        for (name, value) in [
            ("ExcludeClipboardContentFromMonitorProcessing", 0u32),
            ("CanIncludeInClipboardHistory", 0),
            ("CanUploadToCloudClipboard", 0),
        ] {
            let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
            let format = RegisterClipboardFormatW(name.as_ptr());
            if format == 0 || !put(format, &value.to_le_bytes()) {
                return false;
            }
        }
    }
    let copied = prepared.publish(13);
    if copied && !private {
        let name: Vec<_> = "CanIncludeInClipboardHistory\0".encode_utf16().collect();
        let format = RegisterClipboardFormatW(name.as_ptr());
        if format != 0 {
            put(format, &1u32.to_le_bytes());
        }
    }
    copied
}
/// True while no Ctrl/Shift/Alt/Win key is physically down. A shortcut still held
/// when transcription finishes would turn the synthesized Ctrl+V into a
/// different chord in the target application.
unsafe fn modifiers_released() -> bool {
    [0x10, 0x11, 0x12, 0x5b, 0x5c]
        .iter()
        .all(|k| win_input::GetAsyncKeyState(*k) >= 0)
}
/// How long insertion waits for held modifier keys before it copies only.
///
/// The keys are usually the shortcut of the next dictation, pressed again as
/// soon as the last one was released. That is waited out -- it ends when that
/// dictation does -- instead of giving up on this one's paste after a moment.
/// Keys held for any other reason get a few seconds.
const KEYS_IDLE: Duration = Duration::from_secs(6);
const KEYS_RECORDING: Duration = Duration::from_secs(15 * 60);
/// A wait longer than this is shown on the panel.
const KEYS_HINT: Duration = Duration::from_millis(150);
pub(super) fn keys_wait_over(waited: Duration, recording_newer: bool) -> bool {
    waited
        >= if recording_newer {
            KEYS_RECORDING
        } else {
            KEYS_IDLE
        }
}
/// Brings the window the dictation was started from back to the front, so Ctrl+V
/// reaches the field that was being typed in.
///
/// A dictation ends a second or more after it starts, and a notification, a
/// background application or the user's own click can hold the foreground by
/// then. Windows only lets the process owning the foreground reassign it, so the
/// current foreground thread's input queue is attached for the length of the
/// call. Returns whether the window actually came forward; the caller types
/// nothing if it did not, so a refusal can never paste into a bystander.
unsafe fn restore_foreground(window: isize) -> bool {
    if win_input::GetForegroundWindow() == window {
        return true;
    }
    if win_input::IsIconic(window) != 0 {
        win_input::ShowWindow(window, 9);
    }
    let foreground = win_input::GetForegroundWindow();
    let ours = win_input::GetCurrentThreadId();
    // Attaching to a thread that has stopped pumping messages can block with no
    // timeout, so a foreground application Windows already considers hung is
    // left alone; the plain request below either works or is refused.
    let theirs = match foreground {
        0 => 0,
        held if win_input::IsHungAppWindow(held) != 0 => 0,
        held => win_input::GetWindowThreadProcessId(held, std::ptr::null_mut()),
    };
    let attached =
        theirs != 0 && theirs != ours && win_input::AttachThreadInput(ours, theirs, 1) != 0;
    win_input::SetForegroundWindow(window);
    win_input::BringWindowToTop(window);
    if attached {
        win_input::AttachThreadInput(ours, theirs, 0);
    }
    // Activation is asynchronous, and a webview or terminal still has to put the
    // caret back in the control that held it, which it does after the window
    // itself is already in front.
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(10));
        if win_input::GetForegroundWindow() == window {
            std::thread::sleep(Duration::from_millis(50));
            return win_input::GetForegroundWindow() == window;
        }
    }
    false
}
/// Copies saved text with no automatic paste. The tray uses this: once its menu
/// has opened, the window that had focus before is no longer knowable, so there
/// is no target to press Ctrl+V into.
pub fn clipboard(owner: isize, text: &str) -> Result<()> {
    // Guard the publication baseline shared with automatic delivery, but do not
    // make a user Copy wait for another dictation's held-key or paste wait.
    let _publication = PUBLICATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        if !acquire(owner) {
            return Err("Clipboard is busy. Try again in a moment.".into());
        }
        let copied = write(text);
        CloseClipboard();
        if copied {
            Ok(())
        } else {
            Err("Clipboard copy failed. The transcript is saved in history.".into())
        }
    }
}
/// One insertion at a time. Two dictations finishing together must not
/// interleave their clipboard writes and key presses; the earlier one, which
/// asked first, goes first.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
// Same-owner Writer copies must not interleave CloseClipboard and the sequence
// sample. External copies have a different owner and are caught by both guards.
pub(super) static PUBLICATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) struct Preservation {
    pub(super) restoration: super::clipboard_preservation::Restoration,
    sequence: u32,
    pub(super) history_retained: bool,
}

/// Called only after an eligible paste read. Windows history is best effort;
/// its failure must not veto restoration of the user's clipboard.
pub(super) fn restore_after_read(
    previous: &super::clipboard_preservation::Snapshot,
    owner: isize,
    mut sequence: u32,
    text: &str,
    history: bool,
    retained: &dyn Fn(&str) -> bool,
    mark: &dyn Fn(&'static str),
) -> Preservation {
    let mut history_retained = history && retained(text);
    if history && !history_retained {
        // A quick ordinary -> excluded delayed -> ordinary publication can be
        // missed by the history monitor. Give it one more ordinary publication,
        // checking the current generation under the exclusive clipboard lock.
        let publication = PUBLICATION.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            if acquire(owner) {
                if GetClipboardOwner() == owner && GetClipboardSequenceNumber() == sequence {
                    let copied = write(text);
                    CloseClipboard();
                    sequence = GetClipboardSequenceNumber();
                    drop(publication);
                    if copied {
                        mark("clipboard_history_retry");
                        history_retained = retained(text);
                    }
                } else {
                    CloseClipboard();
                    drop(publication);
                }
            } else {
                drop(publication);
            }
        }
    }
    if history {
        mark(if history_retained {
            "clipboard_history_retained"
        } else {
            "clipboard_history_unavailable"
        });
    }
    let restoration = previous.restore(owner, sequence);
    use super::clipboard_preservation::Restoration;
    mark(match restoration {
        Restoration::Restored => "clipboard_restored",
        Restoration::Changed => "clipboard_newer_copy_kept",
        Restoration::Unavailable => "clipboard_restore_unavailable",
        Restoration::Failed => "clipboard_restore_failed",
    });
    Preservation {
        restoration,
        sequence,
        history_retained,
    }
}

pub(super) fn windows_deliver(
    owner: isize,
    target: Option<Target>,
    text: &str,
    preserve_clipboard: bool,
    private_clipboard: bool,
    guards: &Guards,
) -> Result<Outcome> {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _apartment = preserve_clipboard
        .then(super::clipboard_preservation::Apartment::new)
        .flatten();
    let publication = PUBLICATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        if !acquire(owner) {
            return Err("Clipboard is busy. Copy the saved transcript from history.".into());
        }
        if !(guards.wanted)() {
            CloseClipboard();
            return Err("The dictation was discarded before its text was inserted.".into());
        }
        let previous = if preserve_clipboard && _apartment.is_some() {
            super::clipboard_preservation::Snapshot::capture()
        } else {
            None
        };
        let preservation_unavailable = preserve_clipboard && previous.is_none();
        if preservation_unavailable {
            (guards.mark)("clipboard_preservation_unavailable");
        }
        let copied = write_with_privacy(text, private_clipboard);
        CloseClipboard();
        // Closing publishes synthesized clipboard formats. Baseline the public
        // clipboard, then verify ownership too so an intervening external copy
        // cannot become the accepted baseline.
        let mut clipboard_sequence = GetClipboardSequenceNumber();
        if !copied {
            if let Some(previous) = &previous {
                previous.restore(owner, clipboard_sequence);
            }
            return Err("Clipboard copy failed. The transcript is saved in history.".into());
        }
        drop(publication);
        (guards.mark)("clipboard_written");
        let with_preservation_status = |message: &str| {
            if preservation_unavailable {
                format!("{message} Previous clipboard contents could not be preserved.")
            } else {
                message.to_owned()
            }
        };
        let fallback = |message: &str| {
            Ok(Outcome {
                copied: true,
                pasted: false,
                message: with_preservation_status(message),
            })
        };
        let Some(
            target @ Target {
                window, process, ..
            },
        ) = target
        else {
            return fallback("Copied to clipboard. Paste it where you need it.");
        };
        if process == std::process::id() {
            return fallback("Copied to clipboard. Start with the global shortcut in another app for automatic paste.");
        }
        let waiting = Instant::now();
        let mut hinted = false;
        while !modifiers_released() {
            let waited = waiting.elapsed();
            if !(guards.wanted)() || keys_wait_over(waited, (guards.recording_newer)()) {
                break;
            }
            if !hinted && waited >= KEYS_HINT {
                (guards.waiting)();
                hinted = true;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        (guards.mark)("keys_released");
        if !(guards.wanted)() {
            return fallback(
                "Copied to clipboard. The dictation was discarded, so it was not pasted.",
            );
        }
        // The captured process identity protects against HWND reuse: this has to
        // still be the same live window the dictation was started from.
        if !super::win_target::alive(target) {
            return fallback("Copied to clipboard. The application you dictated into has closed, so automatic paste was skipped.");
        }
        if !modifiers_released() {
            return fallback("Copied to clipboard. Release shortcut keys and paste manually.");
        }
        if GetClipboardOwner() != owner
            || !input_guard::clipboard_unchanged(clipboard_sequence, GetClipboardSequenceNumber())
        {
            return fallback("Clipboard changed after transcription. Automatic paste was skipped; your transcript is saved in history.");
        }
        // Checked before anything moves the foreground.
        if !(guards.target_valid)() {
            return fallback("Copied to clipboard. The selected text or focus changed, so replacement was skipped.");
        }
        if win_input::GetForegroundWindow() != window {
            // Focus is taken back from a notification or a stray click, but not
            // from a newer dictation: it may be running in another window, and
            // pulling the user out of it is worse than a manual paste.
            if (guards.superseded)() {
                return fallback("Copied to clipboard. A newer dictation started in another window, so this one was not pasted into its own.");
            }
            if !restore_foreground(window) {
                return fallback("Copied to clipboard. Windows would not return focus to the application you dictated into; paste it where you need it.");
            }
        }
        (guards.mark)("foreground_ready");
        let mut delayed = previous
            .as_ref()
            .and_then(|_| super::clipboard_rendering::Source::new(text, private_clipboard));
        if let Some(source) = &delayed {
            source.target(process);
        }
        let insertion = previous
            .as_ref()
            .filter(|_| delayed.is_none())
            .and_then(|_| super::clipboard_preservation::Insertion::capture(window, text));
        if previous.is_some() {
            (guards.mark)(if delayed.is_some() {
                "clipboard_read_tracking"
            } else if insertion.is_some() {
                "clipboard_target_observable"
            } else {
                "clipboard_target_unobservable"
            });
        }
        // Re-read everything after the foreground moved: waiting for activation
        // is the one point where another application can still intervene.
        if !(guards.wanted)() {
            return fallback(
                "Copied to clipboard. The dictation was discarded, so it was not pasted.",
            );
        }
        if GetClipboardOwner() != owner || GetClipboardSequenceNumber() != clipboard_sequence {
            return fallback("Clipboard changed after transcription. Automatic paste was skipped; your transcript is saved in history.");
        }
        if !modifiers_released() {
            return fallback("Copied to clipboard. Release shortcut keys and paste manually.");
        }
        if !super::win_target::alive(target)
            || !super::win_target::paste_safe(window, process)
            || !(guards.target_valid)()
            || win_input::GetForegroundWindow() != window
        {
            return fallback("Copied to clipboard. The selected text or focus changed, so replacement was skipped.");
        }
        if let Some(source) = &mut delayed {
            let Some((sequence, reoffered)) = source
                .prepare(owner, clipboard_sequence)
                .and_then(|_| source.settle())
            else {
                return fallback("Clipboard changed or could not be prepared. Automatic paste was skipped; your transcript is saved in history.");
            };
            clipboard_sequence = sequence;
            if reoffered {
                (guards.mark)("clipboard_bridge_reoffered");
            }
            // Preparation can wait for a reader. Recheck target and keys after it.
            if !(guards.wanted)()
                || !super::win_target::paste_safe(window, process)
                || !super::win_target::alive(target)
                || !(guards.target_valid)()
                || !modifiers_released()
                || win_input::GetForegroundWindow() != window
                || GetClipboardOwner() != source.window
                || GetClipboardSequenceNumber() != clipboard_sequence
            {
                return fallback("Input, clipboard or focus changed before paste. Your transcript is saved in history.");
            }
            source.arm();
        }
        if !super::win_target::alive(target)
            || !(guards.wanted)()
            || !modifiers_released()
            || win_input::GetForegroundWindow() != window
            || GetClipboardSequenceNumber() != clipboard_sequence
        {
            return fallback(
                "Input or focus changed before paste. The transcript is saved in history.",
            );
        }
        let pasted = win_input::send(&win_input::paste_keys());
        (guards.mark)(if pasted {
            "paste_sent"
        } else {
            "paste_blocked"
        });
        let mut message = with_preservation_status(if pasted {
            "Copied; paste sent to the active application."
        } else {
            "Copied to clipboard. This application blocked automatic paste; paste manually."
        });
        if let Some(previous) = previous.filter(|_| pasted) {
            let deadline = Instant::now() + Duration::from_secs(2);
            let read_owner = delayed.as_ref().map_or(owner, |source| source.window);
            let mut final_owner = read_owner;
            let mut read_at = None;
            let mut confirmed = loop {
                if GetClipboardOwner() != read_owner
                    || GetClipboardSequenceNumber() != clipboard_sequence
                    || win_input::GetForegroundWindow() != window
                    || !(guards.wanted)()
                {
                    break false;
                }
                if let Some(source) = &delayed {
                    if source.eligible_read(process) {
                        let at = read_at.get_or_insert_with(Instant::now);
                        if at.elapsed() >= Duration::from_millis(100) {
                            (guards.mark)(if source.read() == Some(0) {
                                "clipboard_read_anonymous"
                            } else {
                                "clipboard_read_target"
                            });
                            break true;
                        }
                    } else if source.read().is_some() {
                        (guards.mark)("clipboard_read_unrelated_or_early");
                        break false;
                    }
                } else if insertion
                    .as_ref()
                    .is_some_and(|insertion| insertion.received())
                {
                    break true;
                } else if insertion.is_none() {
                    break false;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            if delayed.as_ref().is_some_and(|source| source.refused()) {
                (guards.mark)("clipboard_bridge_refused");
            }
            // Even an unread/uncertain paste keeps ordinary text available and
            // governed by the selected privacy policy. Only a confirmed paste restores.
            if let Some(source) = &delayed {
                if let Some(sequence) = source.materialize() {
                    clipboard_sequence = sequence;
                    final_owner = owner;
                } else {
                    confirmed = false;
                }
            }
            let history = !private_clipboard && super::clipboard_preservation::history_enabled();
            use super::clipboard_preservation::Restoration;
            let preservation = confirmed.then(|| {
                restore_after_read(
                    &previous,
                    owner,
                    clipboard_sequence,
                    text,
                    history,
                    &super::clipboard_preservation::retained,
                    guards.mark,
                )
            });
            let restoration = preservation.as_ref().map(|result| result.restoration);
            if let Some(result) = &preservation {
                clipboard_sequence = result.sequence;
            }
            if restoration == Some(Restoration::Restored) {
                message = if previous.empty() {
                    "Previous clipboard restored after paste; it was previously empty."
                } else if private_clipboard {
                    concat!("Previous clipboard restored after paste. Dictation was marked to exclude Windows clipboard history and sync; it remains in ", product_name!(), " history.")
                } else if preservation.as_ref().is_some_and(|result| result.history_retained) {
                    "Previous clipboard restored after paste. Dictation is also in Windows clipboard history."
                } else if history {
                    concat!("Previous clipboard restored after paste. Windows did not confirm saving dictation in clipboard history; it remains in ", product_name!(), " history.")
                } else {
                    concat!("Previous clipboard restored after paste. Windows clipboard history is unavailable; dictation remains in ", product_name!(), " history.")
                }.into();
            } else if restoration == Some(Restoration::Failed) {
                message = concat!(
                    "Clipboard restoration failed after paste. The transcript remains in ",
                    product_name!(),
                    " history."
                )
                .into();
            } else if GetClipboardSequenceNumber() != clipboard_sequence
                || GetClipboardOwner() != final_owner
            {
                message.push_str(" A newer clipboard copy was kept.");
            } else {
                message.push_str(" Clipboard restoration could not be verified; dictation remains on the clipboard.");
            }
        }
        Ok(Outcome {
            copied: true,
            pasted,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_keys_are_waited_out_while_a_newer_dictation_records_and_not_forever_otherwise() {
        assert!(!keys_wait_over(Duration::from_secs(5), false));
        assert!(keys_wait_over(KEYS_IDLE, false));
        // The next dictation's shortcut is held for as long as it records.
        assert!(!keys_wait_over(Duration::from_secs(120), true));
        assert!(keys_wait_over(KEYS_RECORDING, true));
        assert!(KEYS_HINT < KEYS_IDLE);
    }
}
