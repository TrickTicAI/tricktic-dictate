//! Windows clipboard restoration requires observed insertion, never elapsed time.
//! Clipboard bytes and target text stay in memory and are never logged/persisted.
use super::windows_delivery::acquire;
use std::time::{Duration, Instant};
use windows::{
    core::Interface,
    ApplicationModel::DataTransfer::{
        Clipboard, ClipboardHistoryItemsResultStatus, DataPackageView,
    },
    Win32::{System::Com::*, UI::Accessibility::*},
};

#[link(name = "user32")]
extern "system" {
    fn EnumClipboardFormats(format: u32) -> u32;
    fn GetClipboardFormatNameW(format: u32, name: *mut u16, count: i32) -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn GetClipboardSequenceNumber() -> u32;
    fn GetClipboardOwner() -> isize;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, memory: isize) -> isize;
    fn EnumChildWindows(
        window: isize,
        callback: unsafe extern "system" fn(isize, isize) -> i32,
        data: isize,
    ) -> i32;
    fn IsWindowVisible(window: isize) -> i32;
    fn GetForegroundWindow() -> isize;
    fn GetClassNameW(window: isize, name: *mut u16, count: i32) -> i32;
    fn SendMessageTimeoutW(
        window: isize,
        message: u32,
        wp: usize,
        lp: isize,
        flags: u32,
        timeout: u32,
        result: *mut usize,
    ) -> isize;
}
#[link(name = "kernel32")]
extern "system" {
    fn GlobalSize(memory: isize) -> usize;
    fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
    fn GlobalFree(memory: isize) -> isize;
    fn GlobalLock(memory: isize) -> *mut u8;
    fn GlobalUnlock(memory: isize) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
}
const MAX_CLIPBOARD: usize = 16 * 1024 * 1024;
const MAX_TEXT: i32 = 65_536;

// Windows Clipboard history is a single-threaded WinRT class. Delivery runs
// on a worker, and balances initialization before returning that worker.
pub(super) struct Apartment;
impl Apartment {
    fn automation() -> Option<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        }
        Some(Self)
    }
    pub fn new() -> Option<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().ok()?;
        }
        Some(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

/// Copy payload formats, but never replay OLE pointers or an old history ID.
/// Windows recreates bitmap handles from the retained DIB representation.
enum FormatKind {
    Payload,
    Bitmap,
    Transport,
    Excluded,
    Unsupported,
}
unsafe fn format_kind(format: u32) -> FormatKind {
    if format == 2 {
        return FormatKind::Bitmap;
    }
    if matches!(format, 1 | 7 | 8 | 13 | 15 | 16 | 17) {
        return FormatKind::Payload;
    }
    let mut name = [0u16; 256];
    let count = GetClipboardFormatNameW(format, name.as_mut_ptr(), name.len() as i32);
    match String::from_utf16_lossy(&name[..count.max(0) as usize]).as_str() {
        "DataObject" | "Ole Private Data" | "ClipboardHistoryItemId" => FormatKind::Transport,
        // This policy is defined by presence, regardless of its data value.
        "ExcludeClipboardContentFromMonitorProcessing" => FormatKind::Excluded,
        // Chromium uses HGLOBAL bytes for the source URL and serialized frame
        // token. Keep attribution with its payload, including privacy context.
        "Chromium internal source URL"
        | "Chromium internal source RFH token"
        | "HTML Format"
        | "Rich Text Format"
        | "PNG"
        | "Preferred DropEffect"
        // Win+V-selected copies can carry a DROPDESCRIPTION: a value and two
        // fixed UTF-16 arrays, with no pointers or native handles.
        | "DropDescription"
        | "CanIncludeInClipboardHistory"
        | "CanUploadToCloudClipboard" => FormatKind::Payload,
        _ => FormatKind::Unsupported,
    }
}

pub(super) struct Allocation(isize);
impl Allocation {
    pub(super) unsafe fn copy(bytes: &[u8]) -> Option<Self> {
        let memory = Self(GlobalAlloc(0x42, bytes.len()));
        if memory.0 == 0 {
            return None;
        }
        let ptr = GlobalLock(memory.0);
        if ptr.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        GlobalUnlock(memory.0);
        Some(memory)
    }
    pub(super) unsafe fn publish(&mut self, format: u32) -> bool {
        if SetClipboardData(format, self.0) == 0 {
            return false;
        }
        self.0 = 0; // ownership transferred to Windows
        true
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                GlobalFree(self.0);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Restoration {
    Restored,
    Changed,
    Unavailable,
    Failed,
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    formats: Vec<(u32, Vec<u8>)>,
}
impl Snapshot {
    /// Caller holds OpenClipboard across snapshot AND dictation publication.
    /// Thus a copy made during dictation is the one preserved, not an old copy
    /// remembered when recording started.
    pub unsafe fn capture() -> Option<Self> {
        let mut formats = Vec::new();
        let mut format = 0;
        let mut total = 0usize;
        let mut bitmap = false;
        let mut any = false;
        loop {
            SetLastError(0);
            format = EnumClipboardFormats(format);
            if format == 0 {
                if GetLastError() != 0 {
                    return None;
                }
                break;
            }
            any = true;
            match format_kind(format) {
                FormatKind::Unsupported => return None,
                FormatKind::Transport => continue,
                FormatKind::Bitmap => {
                    bitmap = true;
                    continue;
                }
                FormatKind::Excluded => {
                    formats.push((format, vec![0]));
                    continue;
                }
                FormatKind::Payload => {}
            }
            let memory = GetClipboardData(format);
            if memory == 0 {
                return None;
            }
            let size = GlobalSize(memory);
            total = total.checked_add(size)?;
            if size == 0 || total > MAX_CLIPBOARD {
                return None;
            }
            let ptr = GlobalLock(memory);
            if ptr.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
            GlobalUnlock(memory);
            formats.push((format, bytes));
        }
        // A handle is replaceable only when its pixel data was actually saved.
        if (bitmap && !formats.iter().any(|(id, _)| matches!(id, 8 | 17)))
            || (any
                && !formats
                    .iter()
                    .any(|(id, _)| !matches!(format_kind(*id), FormatKind::Excluded)))
        {
            return None;
        }
        Some(Self { formats })
    }

    pub fn empty(&self) -> bool {
        self.formats.is_empty()
    }

    /// Compare and replace while exclusively holding the clipboard: a newer
    /// external copy can never slip between the generation check and restore.
    pub fn restore(&self, owner: isize, sequence: u32) -> Restoration {
        unsafe {
            // Allocate every replacement block before changing the clipboard.
            let Some(mut prepared) = self
                .formats
                .iter()
                .map(|(format, bytes)| Allocation::copy(bytes).map(|memory| (*format, memory)))
                .collect::<Option<Vec<_>>>()
            else {
                return Restoration::Unavailable;
            };
            if !acquire(owner) {
                return Restoration::Unavailable;
            }
            let unchanged = sequence != 0
                && GetClipboardSequenceNumber() == sequence
                && GetClipboardOwner() == owner;
            let restored = if !unchanged {
                Restoration::Changed
            } else if EmptyClipboard() == 0 {
                Restoration::Unavailable
            } else if prepared
                .iter_mut()
                .all(|(format, memory)| memory.publish(*format))
            {
                Restoration::Restored
            } else {
                Restoration::Failed
            };
            CloseClipboard();
            restored
        }
    }
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn consistent_document(
    before: &str,
    prefix: &str,
    selected: &str,
    suffix: &str,
    value: Option<&str>,
) -> bool {
    let before = normalized(before);
    before == normalized(&format!("{prefix}{selected}{suffix}"))
        && value.is_none_or(|value| before == normalized(value))
}

/// A read-only UI Automation observation of the exact selected range and its
/// surrounding document. An inaccessible, secure or very large field opts out.
/// The expected document must differ from the original: finding text that was
/// already there cannot acknowledge a paste.
pub(super) struct Insertion {
    id: u64,
    _live: std::sync::Arc<()>,
    sender: std::sync::mpsc::SyncSender<ObservationRequest>,
}
enum ObservationRequest {
    Capture {
        id: u64,
        window: isize,
        text: String,
        live: std::sync::Weak<()>,
        reply: std::sync::mpsc::SyncSender<bool>,
    },
    Read {
        id: u64,
        reply: std::sync::mpsc::SyncSender<bool>,
    },
}
impl Insertion {
    pub fn capture(window: isize, text: &str) -> Option<Self> {
        use std::sync::{
            atomic::{AtomicU64, Ordering},
            mpsc, Arc, OnceLock,
        };
        static NEXT: AtomicU64 = AtomicU64::new(1);
        static WORKER: OnceLock<mpsc::SyncSender<ObservationRequest>> = OnceLock::new();
        let sender = WORKER.get_or_init(|| {
            let (send, receive) = mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let Some(_apartment) = Apartment::automation() else {
                    return;
                };
                let mut current: Option<(u64, std::sync::Weak<()>, FieldInsertion)> = None;
                loop {
                    if current
                        .as_ref()
                        .is_some_and(|(_, live, _)| live.upgrade().is_none())
                    {
                        current = None;
                    }
                    let request = match receive.recv_timeout(Duration::from_millis(250)) {
                        Ok(request) => request,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    match request {
                        ObservationRequest::Capture {
                            id,
                            window,
                            text,
                            live,
                            reply,
                        } => {
                            current = None;
                            if live.upgrade().is_none() {
                                continue;
                            }
                            let observation = FieldInsertion::capture(window, &text);
                            if reply.send(observation.is_some()).is_ok() && live.upgrade().is_some()
                            {
                                current = observation.map(|observation| (id, live, observation));
                            }
                        }
                        ObservationRequest::Read { id, reply } => {
                            let received =
                                current.as_ref().is_some_and(|(held, _, observation)| {
                                    *held == id && observation.received()
                                });
                            let _ = reply.send(received);
                        }
                    }
                }
            });
            send
        });
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let live = Arc::new(());
        let (send, receive) = mpsc::sync_channel(1);
        sender
            .try_send(ObservationRequest::Capture {
                id,
                window,
                text: text.into(),
                live: Arc::downgrade(&live),
                reply: send,
            })
            .ok()?;
        // Some accessibility providers ignore COM's requested transaction timeout.
        // A fixed worker and bounded queue isolate those calls without spawning
        // more threads or allowing an expired observation to restore anything.
        if !receive
            .recv_timeout(Duration::from_millis(900))
            .unwrap_or(false)
        {
            return None;
        }
        Some(Self {
            id,
            _live: live,
            sender: sender.clone(),
        })
    }
    pub fn received(&self) -> bool {
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        self.sender
            .try_send(ObservationRequest::Read {
                id: self.id,
                reply: send,
            })
            .is_ok()
            && receive
                .recv_timeout(Duration::from_millis(100))
                .unwrap_or(false)
    }
}

struct FieldInsertion {
    automation: IUIAutomation,
    element: IUIAutomationElement,
    pattern: Field,
    expected: String,
}
enum Field {
    Text(IUIAutomationTextPattern),
    Edit(IUIAutomationValuePattern),
}
impl Field {
    unsafe fn text(&self) -> windows::core::Result<String> {
        match self {
            Self::Text(pattern) => Ok(pattern.DocumentRange()?.GetText(MAX_TEXT + 1)?.to_string()),
            Self::Edit(pattern) => Ok(pattern.CurrentValue()?.to_string()),
        }
    }
}
impl FieldInsertion {
    pub fn capture(window: isize, text: &str) -> Option<Self> {
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).ok()?;
            let timed: IUIAutomation2 = automation.cast().ok()?;
            timed.SetConnectionTimeout(300).ok()?;
            timed.SetTransactionTimeout(300).ok()?;
            if let Ok(insertion) = Self::try_capture(&automation, text) {
                return Some(insertion);
            }
            // Chromium can expose only its outer View until its renderer gets
            // an accessibility request. Request the visible renderer's provider,
            // without changing browser flags, preferences, focus or field text.
            unsafe extern "system" fn renderer(child: isize, data: isize) -> i32 {
                let mut name = [0u16; 128];
                let count = GetClassNameW(child, name.as_mut_ptr(), name.len() as i32);
                if IsWindowVisible(child) != 0
                    && String::from_utf16_lossy(&name[..count.max(0) as usize])
                        == "Chrome_RenderWidgetHostHWND"
                {
                    *(data as *mut isize) = child;
                    return 0;
                }
                1
            }
            let mut child = 0isize;
            EnumChildWindows(window, renderer, &mut child as *mut isize as isize);
            if child == 0 || GetForegroundWindow() != window {
                return None;
            }
            let mut result = 0usize;
            if SendMessageTimeoutW(child, 0x3d, 0, 1, 2, 300, &mut result) == 0 {
                return None;
            }
            let _provider = automation
                .ElementFromHandle(windows::Win32::Foundation::HWND(child as *mut _))
                .ok()?;
            let deadline = Instant::now() + Duration::from_millis(350);
            loop {
                if GetForegroundWindow() != window {
                    return None;
                }
                if let Ok(insertion) = Self::try_capture(&automation, text) {
                    return Some(insertion);
                }
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    unsafe fn try_capture(automation: &IUIAutomation, text: &str) -> windows::core::Result<Self> {
        let element = automation.GetFocusedElement()?;
        let refuse =
            || windows::core::Error::from_hresult(windows::core::HRESULT(0x80070057u32 as i32));
        if element.CurrentIsPassword()?.as_bool() {
            return Err(refuse());
        }
        let (pattern, before, expected) = if let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            let document = pattern.DocumentRange()?;
            let before = document.GetText(MAX_TEXT + 1)?.to_string();
            let selections = pattern.GetSelection()?;
            if selections.Length()? != 1 {
                return Err(refuse());
            }
            let selection = selections.GetElement(0)?;
            let prefix = document.Clone()?;
            prefix.MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selection,
                TextPatternRangeEndpoint_Start,
            )?;
            let suffix = document.Clone()?;
            suffix.MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &selection,
                TextPatternRangeEndpoint_End,
            )?;
            let prefix = prefix.GetText(MAX_TEXT + 1)?.to_string();
            let selected = selection.GetText(MAX_TEXT + 1)?.to_string();
            let suffix = suffix.GetText(MAX_TEXT + 1)?.to_string();
            // Some Chromium accessibility proxies return the accessible label
            // as DocumentRange, rather than the edit value. Such a document is
            // not a trustworthy receipt (xterm also clears its helper field).
            let value = element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok();
            let edit_value = value
                .as_ref()
                .map(|value| value.CurrentValue())
                .transpose()?;
            if !consistent_document(
                &before,
                &prefix,
                &selected,
                &suffix,
                edit_value
                    .as_ref()
                    .map(|value| value.to_string())
                    .as_deref(),
            ) {
                return Err(refuse());
            }
            let expected = normalized(&format!("{prefix}{text}{suffix}"));
            let field = value.map(Field::Edit).unwrap_or(Field::Text(pattern));
            (field, before, expected)
        } else {
            // Classic Win32 Edit exposes ValuePattern, but not TextPattern.
            // EM_GETSEL supplies its exact UTF-16 selection; arbitrary value-only
            // fields remain unsupported rather than guessing where the caret is.
            let window = element.CurrentNativeWindowHandle()?.0 as isize;
            let mut class = [0u16; 128];
            let count = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
            let class = String::from_utf16_lossy(&class[..count.max(0) as usize]);
            if class != "Edit" && !class.starts_with("WindowsForms10.EDIT.") {
                return Err(refuse());
            }
            let pattern: IUIAutomationValuePattern =
                element.GetCurrentPatternAs(UIA_ValuePatternId)?;
            let before = pattern.CurrentValue()?.to_string();
            let units: Vec<_> = before.encode_utf16().collect();
            if units.len() > MAX_TEXT as usize {
                return Err(refuse());
            }
            let (mut start, mut end, mut result) = (0u32, 0u32, 0usize);
            if SendMessageTimeoutW(
                window,
                0xb0,
                &mut start as *mut u32 as usize,
                &mut end as *mut u32 as isize,
                2,
                300,
                &mut result,
            ) == 0
                || start > end
                || end as usize > units.len()
            {
                return Err(refuse());
            }
            let expected = normalized(&format!(
                "{}{}{}",
                String::from_utf16_lossy(&units[..start as usize]),
                text,
                String::from_utf16_lossy(&units[end as usize..])
            ));
            (Field::Edit(pattern), before, expected)
        };
        if before.encode_utf16().count() > MAX_TEXT as usize {
            return Err(refuse());
        }
        if expected == normalized(&before) || expected.encode_utf16().count() > MAX_TEXT as usize {
            return Err(refuse());
        }
        Ok(Self {
            automation: automation.clone(),
            element,
            pattern,
            expected,
        })
    }
    pub fn received(&self) -> bool {
        unsafe {
            (|| -> windows::core::Result<bool> {
                let focused = self.automation.GetFocusedElement()?;
                if focused.CurrentIsPassword()?.as_bool() {
                    return Ok(false);
                }
                if !self
                    .automation
                    .CompareElements(&focused, &self.element)?
                    .as_bool()
                {
                    return Ok(false);
                }
                let actual = normalized(&self.pattern.text()?);
                Ok(actual == self.expected)
            })()
            .unwrap_or(false)
        }
    }
}

fn contains_text(content: &DataPackageView, text: &str) -> windows::core::Result<bool> {
    // Image-only history entries have no text provider. Do not ask them to
    // render text, and do not assume the newest entry is the dictation: Windows
    // may deduplicate an existing text entry below a newer image.
    if !content.Contains(&"Text".into())? {
        return Ok(false);
    }
    Ok(content.GetTextAsync()?.get()? == text)
}

/// Wait only for Windows to retain *this* text before promoting the old copy.
/// A worker owns WinRT calls so a stuck history service cannot block delivery.
/// One bounded queue prevents piling up workers if that service stops answering.
pub(super) fn retained(text: &str) -> bool {
    use std::sync::{mpsc, OnceLock};
    type Request = (String, mpsc::SyncSender<bool>);
    static WORKER: OnceLock<mpsc::SyncSender<Request>> = OnceLock::new();
    let sender = WORKER.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel::<Request>(1);
        std::thread::spawn(move || {
            let Some(_apartment) = Apartment::new() else {
                return;
            };
            for (text, reply) in receive {
                let retained = (|| -> windows::core::Result<bool> {
                    let history = Clipboard::GetHistoryItemsAsync()?.get()?;
                    if history.Status()? != ClipboardHistoryItemsResultStatus::Success {
                        return Ok(false);
                    }
                    let items = history.Items()?;
                    for index in 0..items.Size()? {
                        if items
                            .GetAt(index)
                            .and_then(|item| item.Content())
                            .and_then(|content| contains_text(&content, &text))
                            .unwrap_or(false)
                        {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                })()
                .unwrap_or(false);
                let _ = reply.send(retained);
            }
        });
        send
    });
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1) {
        let (send, receive) = mpsc::sync_channel(1);
        if sender.try_send((text.into(), send)).is_err() {
            return false;
        }
        if receive
            .recv_timeout(Duration::from_millis(300))
            .unwrap_or(false)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

pub(super) fn history_enabled() -> bool {
    Clipboard::IsHistoryEnabled().unwrap_or(false)
}

#[cfg(test)]
#[path = "clipboard_preservation_tests.rs"]
mod tests;

#[cfg(test)]
mod document_tests {
    use super::consistent_document;
    #[test]
    fn selection_must_partition_the_actual_edit_value() {
        assert!(consistent_document(
            "abc שלום\r\n",
            "abc ",
            "שלום",
            "\n",
            Some("abc שלום\n")
        ));
        assert!(!consistent_document(
            "Terminal input",
            "",
            "",
            "\n",
            Some("")
        ));
        assert!(!consistent_document("abc", "a", "", "c", Some("abc")));
        assert!(!consistent_document(
            "label",
            "",
            "",
            "label",
            Some("actual value")
        ));
    }
}
