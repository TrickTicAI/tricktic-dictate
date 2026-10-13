//! Delayed clipboard source for native clipboard preservation.
//! A render callback proves a read, not which application initiated that read.
use super::clipboard_preservation::Allocation;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

/// Clipboard bridges seen taking a promise before paste was armed, with their
/// process creation time so a recycled process id is not mistaken for one.
static MONITORS: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());
const MAX_MONITORS: usize = 16;
/// How long an eager bridge gets to read a new promise before paste is sent.
/// WSLg's msrdc was measured finishing its read within 8 ms of each update.
const SETTLE: Duration = Duration::from_millis(15);

#[repr(C)]
struct Class {
    style: u32,
    procedure: unsafe extern "system" fn(isize, u32, usize, isize) -> isize,
    class_extra: i32,
    window_extra: i32,
    instance: isize,
    icon: isize,
    cursor: isize,
    background: isize,
    menu: *const u16,
    name: *const u16,
}
#[repr(C)]
struct Message {
    window: isize,
    message: u32,
    wp: usize,
    lp: isize,
    time: u32,
    x: i32,
    y: i32,
    private: u32,
}
#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const Class) -> u16;
    fn UnregisterClassW(name: *const u16, instance: isize) -> i32;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: isize,
        menu: isize,
        instance: isize,
        parameter: *const u8,
    ) -> isize;
    fn SetWindowLongPtrW(window: isize, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(window: isize, index: i32) -> isize;
    fn DefWindowProcW(window: isize, message: u32, wp: usize, lp: isize) -> isize;
    fn DestroyWindow(window: isize) -> i32;
    fn PostQuitMessage(code: i32);
    fn GetMessageW(message: *mut Message, window: isize, min: u32, max: u32) -> i32;
    fn DispatchMessageW(message: *const Message) -> isize;
    fn PostMessageW(window: isize, message: u32, wp: usize, lp: isize) -> i32;
    fn GetThreadDesktop(thread: u32) -> isize;
    fn SetThreadDesktop(desktop: isize) -> i32;
    fn GetOpenClipboardWindow() -> isize;
    fn OpenClipboard(window: isize) -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn SetTimer(window: isize, id: usize, ms: u32, callback: usize) -> usize;
    fn KillTimer(window: isize, id: usize) -> i32;
    fn GetClipboardOwner() -> isize;
    fn GetClipboardSequenceNumber() -> u32;
    fn EmptyClipboard() -> i32;
    fn CloseClipboard() -> i32;
    fn SetClipboardData(format: u32, memory: isize) -> isize;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
}

struct Data {
    allocation: Mutex<Option<Allocation>>,
    rendered: AtomicBool,
    reader_process: AtomicU32,
    armed: AtomicBool,
    read_after_arm: AtomicBool,
    sequence: AtomicU32,
    fallback_owner: std::sync::atomic::AtomicIsize,
    target_process: AtomicU32,
    monitors: Mutex<Vec<u32>>,
    refused: AtomicBool,
    text: String,
    private: bool,
}
unsafe extern "system" fn procedure(window: isize, message: u32, wp: usize, lp: isize) -> isize {
    if message == 0x81 {
        // CREATESTRUCTW begins with lpCreateParams.
        SetWindowLongPtrW(window, -21, *(lp as *const isize));
    }
    let pointer = GetWindowLongPtrW(window, -21) as *const Data;
    if message == 0x305 && wp == 13 && !pointer.is_null() {
        let data = &*pointer;
        let armed = data.armed.load(Ordering::Acquire);
        let mut reader = 0;
        let reading = GetOpenClipboardWindow();
        super::win_input::GetWindowThreadProcessId(reading, &mut reader);
        // An eager clipboard bridge already took this text. Leaving its request
        // unanswered keeps the promise for the read that follows the paste.
        if reading != window
            && reading != data.fallback_owner.load(Ordering::Acquire)
            && reader != data.target_process.load(Ordering::Acquire)
            && data
                .monitors
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(&reader)
        {
            data.refused.store(true, Ordering::Release);
            return 0;
        }
        let mut allocation = data.allocation.lock().unwrap_or_else(|p| p.into_inner());
        if allocation.as_mut().is_some_and(|memory| memory.publish(13)) {
            *allocation = None;
            data.reader_process.store(reader, Ordering::Release);
            data.read_after_arm.store(armed, Ordering::Release);
            data.rendered.store(true, Ordering::Release);
        }
        return 0;
    }
    if message == 0x306 && !pointer.is_null() {
        // Graceful process shutdown must fulfill any remaining format promise.
        if OpenClipboard(window) != 0 {
            if GetClipboardOwner() == window {
                GetClipboardData(13);
            }
            CloseClipboard();
        }
        return 0;
    }
    if matches!(message, 0x10 | 0x113) && !pointer.is_null() {
        let data = &*pointer;
        // Keep pumping render requests while another app holds the clipboard.
        // A busy clipboard must not destroy its still-unrendered owner.
        if OpenClipboard(data.fallback_owner.load(Ordering::Acquire)) == 0 {
            SetTimer(window, 1, 100, 0);
            return 0;
        }
        let mut ready = true;
        if GetClipboardOwner() == window {
            if GetClipboardSequenceNumber() == data.sequence.load(Ordering::Acquire) {
                ready = super::windows_delivery::write_with_privacy(&data.text, data.private);
            } else {
                // Preserve an intervening update. Only fulfill a promise that
                // is still present; a replaced format returns its current data.
                GetClipboardData(13);
            }
        }
        CloseClipboard();
        if !ready {
            SetTimer(window, 1, 100, 0);
            return 0;
        }
        KillTimer(window, 1);
        DestroyWindow(window);
        return 0;
    }
    if message == 2 {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(window, message, wp, lp)
}

/// Tracks the first clipboard read. Known unrelated readers and pre-paste
/// reads never authorize restoration. Anonymous Windows readers require the
/// same foreground target plus a short settling interval; this is best-effort
/// consumption detection, not an assertion about the editor's final contents.
pub(super) struct Source {
    pub window: isize,
    data: Arc<Data>,
    sequence: u32,
    stopped: std::sync::mpsc::Receiver<()>,
}
impl Source {
    pub fn new(text: &str, private: bool) -> Option<Self> {
        let monitors = {
            let mut known = MONITORS.lock().unwrap_or_else(|p| p.into_inner());
            known.retain(|(process, created)| {
                super::win_target::created(*process) == Some(*created)
            });
            known.iter().map(|(process, _)| *process).collect()
        };
        let data = Arc::new(Data {
            allocation: Mutex::new(Some(unsafe { Allocation::copy(&encoded(text))? })),
            rendered: AtomicBool::new(false),
            reader_process: AtomicU32::new(0),
            armed: AtomicBool::new(false),
            read_after_arm: AtomicBool::new(false),
            sequence: AtomicU32::new(0),
            fallback_owner: std::sync::atomic::AtomicIsize::new(0),
            target_process: AtomicU32::new(0),
            monitors: Mutex::new(monitors),
            refused: AtomicBool::new(false),
            text: text.into(),
            private,
        });
        let thread_data = data.clone();
        let desktop = unsafe { GetThreadDesktop(super::win_input::GetCurrentThreadId()) };
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let (stopped_send, stopped) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || unsafe {
            // Also preserves noninteractive test isolation on a fresh thread.
            if SetThreadDesktop(desktop) == 0 {
                return;
            }
            let name: Vec<_> = format!("WriterClipboard-{}\0", uuid::Uuid::new_v4())
                .encode_utf16()
                .collect();
            let class = Class {
                style: 0,
                procedure,
                class_extra: 0,
                window_extra: 0,
                instance: 0,
                icon: 0,
                cursor: 0,
                background: 0,
                menu: std::ptr::null(),
                name: name.as_ptr(),
            };
            if RegisterClassW(&class) == 0 {
                return;
            }
            let window = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                0,
                0,
                0,
                1,
                1,
                0,
                0,
                0,
                Arc::as_ptr(&thread_data).cast(),
            );
            if window != 0 {
                if send.send(window).is_ok() {
                    let mut message: Message = std::mem::zeroed();
                    while GetMessageW(&mut message, 0, 0, 0) > 0 {
                        DispatchMessageW(&message);
                    }
                } else {
                    DestroyWindow(window);
                }
            }
            UnregisterClassW(name.as_ptr(), 0);
            let _ = stopped_send.send(());
        });
        let window = receive.recv_timeout(Duration::from_secs(1)).ok()?;
        Some(Self {
            window,
            data,
            sequence: 0,
            stopped,
        })
    }
    /// The process whose read counts as the paste. It is never refused, even
    /// when it was earlier seen acting as a clipboard bridge.
    pub fn target(&self, process: u32) {
        self.data.target_process.store(process, Ordering::Release);
    }
    /// Re-arm the already-published dictation only if its generation is still
    /// current. Preparation and the original user's snapshot do not race a copy.
    pub fn prepare(&mut self, owner: isize, sequence: u32) -> Option<u32> {
        self.data.fallback_owner.store(owner, Ordering::Release);
        self.promise(owner, sequence)
    }
    fn promise(&mut self, owner: isize, sequence: u32) -> Option<u32> {
        let _publication = super::windows_delivery::PUBLICATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe {
            let name: Vec<_> = "ExcludeClipboardContentFromMonitorProcessing\0"
                .encode_utf16()
                .collect();
            let excluded = RegisterClipboardFormatW(name.as_ptr());
            let mut marker = Allocation::copy(&[0])?;
            if excluded == 0 || !super::windows_delivery::acquire(self.window) {
                return None;
            }
            if GetClipboardOwner() != owner || GetClipboardSequenceNumber() != sequence {
                CloseClipboard();
                return None;
            }
            let ok = EmptyClipboard() != 0 && marker.publish(excluded);
            SetLastError(0);
            if ok {
                SetClipboardData(13, 0);
            }
            let ok = ok && GetLastError() == 0;
            // On a publication error, leave usable ordinary text, never an
            // unreadable delayed promise. The caller still has the transcript.
            if !ok {
                super::windows_delivery::write_with_privacy(&self.data.text, self.data.private);
            }
            CloseClipboard();
            self.sequence = GetClipboardSequenceNumber();
            self.data.sequence.store(self.sequence, Ordering::Release);
            ok.then_some(self.sequence)
        }
    }
    /// Eager clipboard bridges (WSLg, remote desktop clients) read every update
    /// within milliseconds, consuming the single render a paste is recognised
    /// by. A reader with a window in another process that arrives before paste
    /// is armed cannot be the paste: it keeps the text it took, is refused from
    /// then on, and the promise is offered again. Returns the current sequence
    /// and whether the promise was offered again.
    pub fn settle(&mut self) -> Option<(u32, bool)> {
        let target = self.data.target_process.load(Ordering::Acquire);
        let mut reoffered = false;
        for round in 0.. {
            let deadline = Instant::now() + SETTLE;
            while self.read().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            match self.read() {
                Some(reader) if reader != 0 && reader != target && round < 3 => {
                    self.reoffer(reader)?;
                    reoffered = true;
                }
                _ => break,
            }
        }
        Some((self.sequence, reoffered))
    }
    fn reoffer(&mut self, reader: u32) -> Option<u32> {
        let allocation = unsafe { Allocation::copy(&encoded(&self.data.text))? };
        if let Some(created) = super::win_target::created(reader) {
            let mut known = MONITORS.lock().unwrap_or_else(|p| p.into_inner());
            if known.len() == MAX_MONITORS {
                known.remove(0);
            }
            known.push((reader, created));
        }
        self.data
            .monitors
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(reader);
        {
            let mut slot = self
                .data
                .allocation
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            *slot = Some(allocation);
            self.data.rendered.store(false, Ordering::Release);
        }
        self.promise(self.window, self.sequence)
    }
    /// Whether a known clipboard bridge asked for the promise and was refused.
    pub fn refused(&self) -> bool {
        self.data.refused.load(Ordering::Acquire)
    }
    pub fn arm(&self) {
        self.data.armed.store(true, Ordering::Release);
    }
    pub fn eligible_read(&self, target_process: u32) -> bool {
        self.read().is_some_and(|reader| {
            self.data.read_after_arm.load(Ordering::Acquire)
                && (reader == 0 || reader == target_process)
        })
    }
    /// Transfer dictation back to Writer's ordinary owner with the selected
    /// history policy. Acquiring the clipboard waits for the reader to release it.
    /// The ownership/generation check remains inside the exclusive lock.
    pub fn materialize(&self) -> Option<u32> {
        let _publication = super::windows_delivery::PUBLICATION
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe {
            let owner = self.data.fallback_owner.load(Ordering::Acquire);
            if !super::windows_delivery::acquire(owner) {
                return None;
            }
            if GetClipboardOwner() != self.window || GetClipboardSequenceNumber() != self.sequence {
                CloseClipboard();
                return None;
            }
            let copied =
                super::windows_delivery::write_with_privacy(&self.data.text, self.data.private);
            CloseClipboard();
            copied.then(|| GetClipboardSequenceNumber())
        }
    }
    pub fn read(&self) -> Option<u32> {
        self.data
            .rendered
            .load(Ordering::Acquire)
            .then(|| self.data.reader_process.load(Ordering::Acquire))
    }
}
fn encoded(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}
impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            // Cleanup runs on the owning message thread and can retry without
            // blocking another application's synchronous render request.
            PostMessageW(self.window, 0x10, 0, 0);
            let _ = self.stopped.recv_timeout(Duration::from_secs(1));
        }
    }
}
