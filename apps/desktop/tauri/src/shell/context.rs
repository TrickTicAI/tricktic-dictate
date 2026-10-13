//! Opt-in current context. No clipboard-history reads or synthetic Copy shortcut.
//! UI Automation objects stay on their owning COM thread and never enter the UI.
use crate::storage::Result;
use metrix_writer_api::cleanup::modes::{ContextSnapshot, ModeKind, ModeProfile};
#[cfg(any(windows, target_os = "macos"))]
use std::{sync::mpsc, time::Duration};

pub struct ContextReader {
    #[cfg(any(windows, target_os = "macos"))]
    sender: mpsc::SyncSender<Request>,
}
#[cfg(any(windows, target_os = "macos"))]
enum Request {
    Capture(
        String,
        Box<ModeProfile>,
        Option<(String, isize, u32)>,
        mpsc::SyncSender<Result<ContextSnapshot>>,
    ),
    Validate(String, mpsc::SyncSender<bool>),
    Forget(Option<String>, mpsc::SyncSender<()>),
}
impl ContextReader {
    pub fn new() -> Self {
        #[cfg(any(windows, target_os = "macos"))]
        {
            let (sender, receiver) = mpsc::sync_channel(8);
            #[cfg(windows)]
            std::thread::spawn(move || windows::worker(receiver));
            #[cfg(target_os = "macos")]
            std::thread::spawn(move || macos::worker(receiver));
            Self { sender }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            Self {}
        }
    }
    pub fn capture(
        &self,
        session: String,
        profile: ModeProfile,
        target: Option<(String, isize, u32)>,
    ) -> Result<ContextSnapshot> {
        if profile.kind == ModeKind::Regular
            || (!profile.context.application
                && !profile.context.selected_text
                && !profile.context.clipboard
                && profile.kind != ModeKind::Command)
        {
            return Ok(ContextSnapshot::default());
        }
        #[cfg(any(windows, target_os = "macos"))]
        {
            let (send, receive) = mpsc::sync_channel(1);
            self.sender
                .try_send(Request::Capture(session, Box::new(profile), target, send))
                .map_err(|_| "Context capture is busy.")?;
            receive.recv_timeout(Duration::from_secs(5)).map_err(|_| {
                "Application context timed out. Try again or use Regular mode.".to_owned()
            })?
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (session, profile, target);
            Err("Native context is currently available on Windows.".into())
        }
    }
    pub fn forget(&self, session: Option<String>) -> Result<()> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            let (send, receive) = mpsc::sync_channel(1);
            self.sender
                .try_send(Request::Forget(session, send))
                .map_err(|_| "Context cleanup is busy. Try again.")?;
            receive
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| "Context cleanup timed out. Try again.".to_owned())
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = session;
            Ok(())
        }
    }
    /// Queue session-specific housekeeping without waiting on OS context work.
    /// The bounded FIFO orders this with captures and Command validation.
    pub fn forget_later(&self, session: String) {
        #[cfg(any(windows, target_os = "macos"))]
        {
            let (send, _) = mpsc::sync_channel(1);
            let _ = self.sender.try_send(Request::Forget(Some(session), send));
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        let _ = session;
    }
    #[cfg(any(windows, target_os = "macos"))]
    pub fn valid(&self, session: &str) -> bool {
        let (send, receive) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::Validate(session.into(), send))
            .is_ok()
            && receive
                .recv_timeout(Duration::from_secs(3))
                .unwrap_or(false)
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::shell::win_input;
    use ::windows::{
        core::Interface,
        Win32::{System::Com::*, UI::Accessibility::*},
    };
    struct Target {
        window: isize,
        process: u32,
        element: IUIAutomationElement,
        range: IUIAutomationTextRange,
        text: String,
    }
    pub(super) fn worker(receiver: mpsc::Receiver<Request>) {
        unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                return;
            }
            let automation: ::windows::core::Result<IUIAutomation> =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER);
            if let Ok(automation) = automation {
                if let Ok(timed) = automation.cast::<IUIAutomation2>() {
                    let _ = timed.SetConnectionTimeout(1000);
                    let _ = timed.SetTransactionTimeout(1000);
                }
                let mut retained: Option<(String, Target)> = None;
                for request in receiver {
                    match request {
                        Request::Capture(session, profile, target, reply) => {
                            retained = None;
                            let result =
                                capture(&automation, &profile, target).map(|(context, target)| {
                                    retained = target.map(|t| (session, t));
                                    context
                                });
                            if reply.send(result).is_err() {
                                retained = None;
                            }
                        }
                        Request::Forget(session, reply) => {
                            if session.as_ref().is_none_or(|session| {
                                retained.as_ref().is_some_and(|(id, _)| id == session)
                            }) {
                                retained = None;
                            }
                            let _ = reply.send(());
                        }
                        Request::Validate(session, reply) => {
                            let valid = retained.as_ref().is_some_and(|(id, target)| {
                                id == &session && validate(&automation, target).unwrap_or(false)
                            });
                            let _ = reply.send(valid);
                        }
                    }
                }
            }
            CoUninitialize();
        }
    }
    unsafe fn foreground(window: isize, process: u32) -> bool {
        let mut current = 0;
        win_input::GetWindowThreadProcessId(window, &mut current);
        current == process
            && process != std::process::id()
            && win_input::IsWindow(window) != 0
            && win_input::GetForegroundWindow() == window
    }
    unsafe fn selection(
        automation: &IUIAutomation,
    ) -> ::windows::core::Result<(IUIAutomationElement, IUIAutomationTextRange, String)> {
        let element = automation.GetFocusedElement()?;
        if element.CurrentIsPassword()?.as_bool() {
            return Err(::windows::core::Error::from_hresult(
                ::windows::core::HRESULT(0x80070005u32 as i32),
            ));
        }
        let pattern: IUIAutomationTextPattern = element.GetCurrentPatternAs(UIA_TextPatternId)?;
        let ranges = pattern.GetSelection()?;
        if ranges.Length()? != 1 {
            return Err(::windows::core::Error::from_hresult(
                ::windows::core::HRESULT(0x80070057u32 as i32),
            ));
        }
        let range = ranges.GetElement(0)?;
        let text = range.GetText(65_537)?.to_string();
        Ok((element, range, text))
    }
    unsafe fn capture(
        automation: &IUIAutomation,
        profile: &ModeProfile,
        target: Option<(String, isize, u32)>,
    ) -> Result<(ContextSnapshot, Option<Target>)> {
        let mut context = ContextSnapshot::default();
        let mut retained = None;
        if profile.context.clipboard {
            context.clipboard = clipboard()?;
        }
        if profile.context.selected_text
            || profile.context.application
            || profile.kind == ModeKind::Command
        {
            let (_, window, process) = target.ok_or("Start this context-enabled mode with its global shortcut in the target application.")?;
            if !foreground(window, process) {
                return Err("Application focus changed before recording. Start again in the target application.".into());
            }
            if profile.context.application {
                #[link(name = "user32")]
                extern "system" {
                    fn GetWindowTextW(window: isize, text: *mut u16, maximum: i32) -> i32;
                }
                let mut title = [0u16; 1024];
                let length =
                    GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
                context.application = String::from_utf16_lossy(&title[..length]);
            }
            if profile.context.selected_text {
                if let Ok((element, range, text)) = selection(automation) {
                    if text.len() > 65_536 {
                        return Err("Select a smaller passage (maximum 64 KiB).".into());
                    }
                    context.selected_text = text.clone();
                    retained = Some(Target {
                        window,
                        process,
                        element,
                        range,
                        text,
                    });
                }
            }
            if !foreground(window, process) {
                return Err("Application focus changed during context capture.".into());
            }
        }
        if profile.kind == ModeKind::Command && context.selected_text.trim().is_empty() {
            return Err("Command mode needs accessible selected text in the target app. Select text and start with the global shortcut.".into());
        }
        Ok((context, retained))
    }
    unsafe fn validate(
        automation: &IUIAutomation,
        target: &Target,
    ) -> ::windows::core::Result<bool> {
        if !foreground(target.window, target.process) {
            return Ok(false);
        }
        let (element, range, text) = selection(automation)?;
        Ok(automation
            .CompareElements(&element, &target.element)?
            .as_bool()
            && range.Compare(&target.range)?.as_bool()
            && text == target.text
            && foreground(target.window, target.process))
    }
    unsafe fn clipboard() -> Result<String> {
        #[link(name = "user32")]
        extern "system" {
            fn OpenClipboard(window: isize) -> i32;
            fn CloseClipboard() -> i32;
            fn GetClipboardData(format: u32) -> isize;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalLock(memory: isize) -> *mut u8;
            fn GlobalUnlock(memory: isize) -> i32;
            fn GlobalSize(memory: isize) -> usize;
        }
        if OpenClipboard(0) == 0 {
            return Err("Clipboard is busy. Disable clipboard context or try again.".into());
        }
        let memory = GetClipboardData(13);
        if memory == 0 {
            CloseClipboard();
            return Ok(String::new());
        }
        let size = GlobalSize(memory);
        if size > 131_074 {
            CloseClipboard();
            return Err("Clipboard context is too large.".into());
        }
        let pointer = GlobalLock(memory);
        let output = if pointer.is_null() {
            String::new()
        } else {
            let units = std::slice::from_raw_parts(pointer.cast::<u16>(), size / 2);
            let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
            let output = String::from_utf16_lossy(&units[..end]);
            GlobalUnlock(memory);
            output
        };
        CloseClipboard();
        Ok(output)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires the isolated WPF context fixture on an interactive Windows desktop"]
    fn native_selection_rejects_moved_ranges_edits_and_password_fields() {
        let root = std::path::PathBuf::from(
            std::env::var_os("WRITER_CONTEXT_FIXTURE_DIR").expect("context fixture directory"),
        );
        let ready: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("ready.json")).unwrap()).unwrap();
        let target = (
            "fixture-session".into(),
            ready["window"].as_i64().unwrap() as isize,
            ready["process"].as_u64().unwrap() as u32,
        );
        let identity = crate::shell::delivery::Target {
            window: target.1,
            process: target.2,
            created: crate::shell::win_target::created(target.2).unwrap(),
        };
        assert!(crate::shell::win_target::alive(identity));
        assert!(
            !crate::shell::win_target::alive(crate::shell::delivery::Target {
                created: identity.created + 1,
                ..identity
            }),
            "Reused PID/HWND with a different creation time must be refused"
        );
        let action = |id: u32, action: &str| {
            std::fs::write(
                root.join("command.json"),
                serde_json::to_vec(&serde_json::json!({"id":id,"action":action})).unwrap(),
            )
            .unwrap();
            let end = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let ack: Option<serde_json::Value> = std::fs::read(root.join("ack.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok());
                if ack.is_some_and(|a| a["id"] == id) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < end,
                    "Fixture did not acknowledge command"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let reader = ContextReader::new();
        // Activate after launching the driver, which can otherwise steal focus.
        action(1, "focus");
        unsafe {
            let current = crate::shell::win_input::GetForegroundWindow();
            let mut current_process = 0;
            let mut target_process = 0;
            crate::shell::win_input::GetWindowThreadProcessId(current, &mut current_process);
            crate::shell::win_input::GetWindowThreadProcessId(target.1, &mut target_process);
            std::fs::write(root.join("focus.json"), serde_json::to_vec(&serde_json::json!({"foreground":current,"foreground_process":current_process,"target":target.1,"target_process":target_process,"expected_process":target.2,"test_process":std::process::id()})).unwrap()).unwrap();
        }
        let profile = metrix_writer_api::cleanup::modes::ModeSettings::default()
            .profiles
            .pop()
            .unwrap();
        let captured = reader
            .capture(
                "fixture-session".into(),
                profile.clone(),
                Some(target.clone()),
            )
            .unwrap();
        assert!(
            captured.selected_text == "Tuesday"
                && captured.clipboard.is_empty()
                && captured.application.is_empty()
        );
        assert!(reader.valid("fixture-session"));
        assert!(crate::shell::win_target::paste_safe(target.1, target.2));
        assert!(!reader.valid("another-session"));
        action(2, "other-range");
        assert!(
            !reader.valid("fixture-session"),
            "Identical text at a different range must not be replaced"
        );
        action(3, "replace");
        assert!(!reader.valid("fixture-session"));
        assert!(!crate::shell::win_target::paste_safe(
            target.1,
            target.2.wrapping_add(1)
        ));
        action(4, "password");
        assert!(!crate::shell::win_target::paste_safe(target.1, target.2));
        assert!(reader
            .capture("password".into(), profile, Some(target))
            .is_err());
    }
}

#[cfg(target_os = "macos")]
pub(super) fn secure_field(target: super::delivery::Target) -> bool {
    macos::secure_field(target)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::shell::{delivery::Target, macos_delivery, macos_hook};
    use core_foundation::{
        base::{CFType, CFTypeRef, TCFType},
        string::{CFString, CFStringRef},
    };
    use std::ffi::c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
        fn AXUIElementGetTypeID() -> usize;
        fn AXUIElementCopyAttributeValue(
            element: CFTypeRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
        fn AXValueGetTypeID() -> usize;
        fn AXValueGetValue(value: CFTypeRef, kind: i32, result: *mut c_void) -> bool;
    }
    fn attribute(element: &CFType, name: &str) -> Result<CFType> {
        let name = CFString::new(name);
        let mut value = std::ptr::null();
        let status = unsafe {
            AXUIElementCopyAttributeValue(
                element.as_CFTypeRef(),
                name.as_concrete_TypeRef(),
                &mut value,
            )
        };
        if status != 0 || value.is_null() {
            return Err(
                "The application does not expose this context through Accessibility.".into(),
            );
        }
        Ok(unsafe { CFType::wrap_under_create_rule(value) })
    }
    fn string(value: CFType) -> Result<String> {
        let value = value
            .downcast::<CFString>()
            .ok_or("The application returned invalid text context.")?;
        if value.char_len() > 65_536 {
            return Err("Select a smaller passage (maximum 64 KiB).".into());
        }
        let text = value.to_string();
        if text.len() > 65_536 {
            return Err("Select a smaller passage (maximum 64 KiB).".into());
        }
        Ok(text)
    }
    fn focused_element(target: Target) -> Result<CFType> {
        if !macos_hook::trusted(false) {
            return Err(concat!(
                "Enable ",
                product_name!(),
                " in System Settings → Privacy & Security → Accessibility to read selected text."
            )
            .into());
        }
        if macos_delivery::secure_input() {
            return Err("Selected text is unavailable while secure input is active.".into());
        }
        let app = unsafe { AXUIElementCreateApplication(target.process as i32) };
        if app.is_null() {
            return Err("Application context is unavailable.".into());
        }
        let app = unsafe { CFType::wrap_under_create_rule(app) };
        unsafe {
            AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), 0.5);
        }
        let element = attribute(&app, "AXFocusedUIElement")?;
        if element.type_of() != unsafe { AXUIElementGetTypeID() } {
            return Err("Focused element is unavailable.".into());
        }
        unsafe {
            AXUIElementSetMessagingTimeout(element.as_CFTypeRef(), 0.5);
        }
        Ok(element)
    }
    pub(super) fn secure_field(target: Target) -> bool {
        focused_element(target).ok().is_some_and(|element| {
            attribute(&element, "AXSubrole")
                .and_then(string)
                .is_ok_and(|s| s == "AXSecureTextField")
        })
    }
    fn focused(target: Target) -> Result<CFType> {
        let element = focused_element(target)?;
        // Password controls may expose their secure role without enabling the
        // process-global Secure Event Input flag. Refuse before reading text.
        if attribute(&element, "AXSubrole")
            .and_then(string)
            .is_ok_and(|s| s == "AXSecureTextField")
        {
            return Err("Selected text is unavailable in a secure text field.".into());
        }
        Ok(element)
    }
    #[derive(Clone, Copy, Default, PartialEq)]
    #[repr(C)]
    struct Range {
        location: isize,
        length: isize,
    }
    fn range(element: &CFType) -> Result<Range> {
        let value = attribute(element, "AXSelectedTextRange")?;
        let mut range = Range::default();
        if value.type_of() != unsafe { AXValueGetTypeID() }
            || !unsafe {
                AXValueGetValue(value.as_CFTypeRef(), 4, (&mut range as *mut Range).cast())
            }
            || range.location < 0
            || range.length < 0
        {
            return Err("The application does not expose a verifiable text selection.".into());
        }
        Ok(range)
    }
    struct Selection {
        target: Target,
        element: CFType,
        range: Range,
        text: String,
    }
    fn foreground(target: Target) -> bool {
        target.process != std::process::id()
            && macos_delivery::foreground() == Some(target)
            && macos_delivery::alive(target)
    }
    fn capture(
        profile: &ModeProfile,
        target: Option<(String, isize, u32)>,
    ) -> Result<(ContextSnapshot, Option<Selection>)> {
        let mut context = ContextSnapshot::default();
        let mut retained = None;
        if profile.context.clipboard {
            context.clipboard = macos_delivery::read_clipboard()?;
        }
        if profile.context.application
            || profile.context.selected_text
            || profile.kind == ModeKind::Command
        {
            let (_, window, process) = target.ok_or("Start this context-enabled mode with its global shortcut in the target application.")?;
            let target = Target { window, process };
            if !foreground(target) {
                return Err("Application focus changed before recording. Start again in the target application.".into());
            }
            if profile.context.application {
                context.application = macos_delivery::application_name(target)
                    .ok_or("Application context is unavailable.")?;
            }
            if profile.context.selected_text {
                let element = focused(target)?;
                // Missing selected-text support is normal for some apps, but
                // malformed/oversized text is never silently accepted.
                if let Ok(value) = attribute(&element, "AXSelectedText") {
                    context.selected_text = string(value)?;
                    if profile.kind == ModeKind::Command {
                        retained = Some(Selection {
                            target,
                            range: range(&element)?,
                            element,
                            text: context.selected_text.clone(),
                        });
                    }
                }
            }
            if !foreground(target)
                || (profile.context.selected_text
                    && (!macos_hook::trusted(false) || macos_delivery::secure_input()))
            {
                return Err(
                    "Application focus or permission changed during context capture.".into(),
                );
            }
        }
        if profile.kind == ModeKind::Command && context.selected_text.trim().is_empty() {
            return Err("Command mode needs accessible selected text in the target app. Select text and start with the global shortcut.".into());
        }
        Ok((context, retained))
    }
    fn validate(saved: &Selection) -> bool {
        if !foreground(saved.target) {
            return false;
        }
        let Ok(element) = focused(saved.target) else {
            return false;
        };
        element == saved.element
            && range(&element).is_ok_and(|r| r == saved.range)
            && attribute(&element, "AXSelectedText")
                .and_then(string)
                .is_ok_and(|s| s == saved.text)
            && foreground(saved.target)
            && macos_hook::trusted(false)
            && !macos_delivery::secure_input()
    }
    pub(super) fn worker(receiver: mpsc::Receiver<Request>) {
        // AX references are created, retained and released on this worker only.
        // A wedged app cannot block the UI: callers have the existing 5 s
        // deadline and bounded busy queue; per-AX-message timeout is 500 ms.
        let mut retained: Option<(String, Selection)> = None;
        for request in receiver {
            match request {
                Request::Capture(session, profile, target, reply) => {
                    retained = None;
                    let result = capture(&profile, target).map(|(context, selection)| {
                        retained = selection.map(|s| (session, s));
                        context
                    });
                    if reply.send(result).is_err() {
                        retained = None;
                    }
                }
                Request::Forget(session, reply) => {
                    if session.as_ref().is_none_or(|session| {
                        retained.as_ref().is_some_and(|(id, _)| id == session)
                    }) {
                        retained = None;
                    }
                    let _ = reply.send(());
                }
                Request::Validate(session, reply) => {
                    let _ = reply.send(
                        retained
                            .as_ref()
                            .is_some_and(|(id, s)| id == &session && validate(s)),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn regular_mode_without_context_never_needs_a_target_or_os_access() {
        let reader = ContextReader::new();
        let profile = metrix_writer_api::cleanup::modes::ModeSettings::default()
            .profiles
            .remove(0);
        let context = reader.capture("no-context".into(), profile, None).unwrap();
        assert!(
            context.application.is_empty()
                && context.selected_text.is_empty()
                && context.clipboard.is_empty()
        );
    }
}
