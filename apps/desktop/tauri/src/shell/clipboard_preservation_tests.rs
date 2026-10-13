//! Real Win32 clipboard regressions in a noninteractive window station. These never
//! touch the user's interactive clipboard, foreground window or keyboard.
use super::*;
use crate::shell::{delivery::Guards, windows_delivery};

#[link(name = "user32")]
extern "system" {
    fn GetProcessWindowStation() -> isize;
    fn GetUserObjectInformationW(
        handle: isize,
        index: i32,
        info: *mut u32,
        size: u32,
        needed: *mut u32,
    ) -> i32;
    fn CountClipboardFormats() -> i32;
    fn OpenClipboard(window: isize) -> i32;
    fn GetOpenClipboardWindow() -> isize;
    fn CreateWindowStationW(
        name: *const u16,
        flags: u32,
        access: u32,
        security: *const u8,
    ) -> isize;
    fn SetProcessWindowStation(station: isize) -> i32;
    fn CloseWindowStation(station: isize) -> i32;
    fn GetThreadDesktop(thread: u32) -> isize;
    fn CreateDesktopW(
        name: *const u16,
        device: *const u16,
        mode: *const u8,
        flags: u32,
        access: u32,
        security: *const u8,
    ) -> isize;
    fn SetThreadDesktop(desktop: isize) -> i32;
    fn CloseDesktop(desktop: isize) -> i32;
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
    fn DestroyWindow(window: isize) -> i32;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
struct Station {
    previous: isize,
    previous_desktop: isize,
    station: isize,
    desktop: isize,
    window: isize,
}
impl Station {
    unsafe fn new() -> Self {
        let mut s = Self {
            previous: GetProcessWindowStation(),
            previous_desktop: GetThreadDesktop(crate::shell::win_input::GetCurrentThreadId()),
            station: 0,
            desktop: 0,
            window: 0,
        };
        s.station = CreateWindowStationW(
            std::ptr::null(), // Named stations require administrator rights.
            0,                // Windows chooses the noninteractive station for this logon.
            0x37F,
            std::ptr::null(),
        );
        assert_ne!(s.station, 0, "private station creation: {}", GetLastError());
        let mut flags = [0u32; 3];
        let mut needed = 0;
        assert_ne!(
            GetUserObjectInformationW(s.station, 1, flags.as_mut_ptr(), 12, &mut needed),
            0
        );
        assert_eq!(flags[2] & 1, 0, "must never use the interactive station");
        assert_ne!(SetProcessWindowStation(s.station), 0);
        s.desktop = CreateDesktopW(
            wide(&format!("WriterClipboardTest-{}", uuid::Uuid::new_v4())).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0xF01FF,
            std::ptr::null(),
        );
        assert_ne!(s.desktop, 0);
        assert_ne!(SetThreadDesktop(s.desktop), 0);
        s.window = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("private clipboard test").as_ptr(),
            0,
            0,
            0,
            1,
            1,
            0,
            0,
            0,
            std::ptr::null(),
        );
        assert_ne!(s.window, 0);
        assert_eq!(GetProcessWindowStation(), s.station);
        s
    }
}
impl Drop for Station {
    fn drop(&mut self) {
        unsafe {
            if self.window != 0 {
                if acquire(self.window) {
                    if GetClipboardOwner() == self.window {
                        EmptyClipboard();
                    }
                    CloseClipboard();
                }
                DestroyWindow(self.window);
            }
            SetProcessWindowStation(self.previous);
            SetThreadDesktop(self.previous_desktop);
            if self.desktop != 0 {
                CloseDesktop(self.desktop);
            }
            if self.station != 0 {
                CloseWindowStation(self.station);
            }
        }
    }
}
unsafe fn format(name: &str) -> u32 {
    RegisterClipboardFormatW(wide(name).as_ptr())
}
unsafe fn bytes(format: u32) -> Vec<u8> {
    let memory = GetClipboardData(format);
    assert_ne!(memory, 0);
    let pointer = GlobalLock(memory);
    assert!(!pointer.is_null());
    let result = std::slice::from_raw_parts(pointer, GlobalSize(memory)).to_vec();
    GlobalUnlock(memory);
    result
}
unsafe fn text(owner: isize) -> String {
    assert!(acquire(owner));
    let data = bytes(13);
    CloseClipboard();
    let units: Vec<_> = data
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16(&units).unwrap()
}

#[test]
#[ignore = "Run alone: temporarily binds this test process to a noninteractive Windows window station"]
fn real_windows_clipboard_formats_and_delivery_fallback() {
    unsafe {
        let station = Station::new();
        let owner = station.window;
        assert_eq!(
            CountClipboardFormats(),
            0,
            "do not disturb existing noninteractive clipboard data"
        );
        let data_object = format("DataObject");
        let ole_private = format("Ole Private Data");
        let history_id = format("ClipboardHistoryItemId");
        let excluded = format("ExcludeClipboardContentFromMonitorProcessing");
        let rtf = format("Rich Text Format");
        let drop_description = format("DropDescription");
        let mut description = vec![0u8; 4 + 260 * 2 * 2];
        description[0..4].copy_from_slice(&1u32.to_le_bytes());
        description[4..12].copy_from_slice(&[b'C', 0, b'o', 0, b'p', 0, b'y', 0]);

        let opened = OpenClipboard(owner);
        let error = GetLastError();
        assert_ne!(
            opened,
            0,
            "private clipboard setup: error {error}, holder {}",
            GetOpenClipboardWindow()
        );
        CloseClipboard();

        // Ordinary OLE text has descriptors which must not be treated as data
        // or replayed with stale references after the clipboard changes.
        windows_delivery::clipboard(owner, "old text שלום").unwrap();
        assert!(acquire(owner));
        for id in [data_object, ole_private, history_id] {
            assert!(windows_delivery::put(id, b"synthetic descriptor\0"));
        }
        assert!(windows_delivery::put(rtf, b"{\\rtf1 old text}\0"));
        assert!(windows_delivery::put(drop_description, &description));
        let saved = Snapshot::capture().expect("OLE bookkeeping must not reject text");
        CloseClipboard();
        windows_delivery::clipboard(owner, "dictation").unwrap();
        assert!(saved.restore(owner, GetClipboardSequenceNumber()) == Restoration::Restored);
        assert_eq!(text(owner), "old text שלום");
        assert!(acquire(owner));
        assert!(bytes(rtf).starts_with(b"{\\rtf1 old text}\0"));
        assert!(bytes(drop_description).starts_with(&description));
        assert_eq!(IsClipboardFormatAvailable(data_object), 0);
        CloseClipboard();

        // Windows actually synthesizes CF_BITMAP from these four pixels, just
        // as it does for screenshots. Preserve the DIB bytes, not a GDI handle.
        let mut dib = vec![0u8; 56];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[20..24].copy_from_slice(&16u32.to_le_bytes());
        dib[40..56].copy_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
        assert!(acquire(owner));
        assert_ne!(EmptyClipboard(), 0);
        assert!(windows_delivery::put(8, &dib));
        for id in [data_object, ole_private, history_id] {
            assert!(windows_delivery::put(id, b"synthetic descriptor\0"));
        }
        // Presence is the privacy policy; delayed/empty marker data is valid.
        SetClipboardData(excluded, 0);
        CloseClipboard();
        assert_ne!(IsClipboardFormatAvailable(2), 0);
        assert!(acquire(owner));
        let image =
            Snapshot::capture().expect("screenshot with OLE/history wrappers must be retained");
        CloseClipboard();
        windows_delivery::clipboard(owner, "dictation").unwrap();
        assert!(image.restore(owner, GetClipboardSequenceNumber()) == Restoration::Restored);
        assert!(acquire(owner));
        assert!(bytes(8).starts_with(&dib));
        assert_ne!(GetClipboardData(2), 0, "bitmap must be available again");
        assert_ne!(
            IsClipboardFormatAvailable(excluded),
            0,
            "privacy exclusion retained"
        );
        assert_eq!(
            IsClipboardFormatAvailable(history_id),
            0,
            "stale history identity not replayed"
        );
        CloseClipboard();

        // History can fail after a real paste read even when the image snapshot
        // was saved correctly. Its failure must not leave dictation on top.
        windows_delivery::clipboard(owner, "after image dictation").unwrap();
        let mut marks = Vec::new();
        let stages = std::cell::RefCell::new(&mut marks);
        let attempts = std::cell::Cell::new(0);
        let result = windows_delivery::restore_after_read(
            &image,
            owner,
            GetClipboardSequenceNumber(),
            "after image dictation",
            true,
            &|_| {
                attempts.set(attempts.get() + 1);
                false
            },
            &|stage| stages.borrow_mut().push(stage),
        );
        assert_eq!(result.restoration, Restoration::Restored);
        assert!(!result.history_retained);
        assert_eq!(attempts.get(), 2, "one bounded history publication retry");
        assert!(marks.contains(&"clipboard_history_retry"));
        assert!(marks.contains(&"clipboard_history_unavailable"));
        assert!(acquire(owner));
        assert!(
            bytes(8).starts_with(&dib),
            "image restored despite history failure"
        );
        CloseClipboard();

        // A missed first publication can recover on the retry. Report that
        // separately from restoration, so the receipt never invents retention.
        windows_delivery::clipboard(owner, "retry dictation").unwrap();
        let attempts = std::cell::Cell::new(0);
        let result = windows_delivery::restore_after_read(
            &image,
            owner,
            GetClipboardSequenceNumber(),
            "retry dictation",
            true,
            &|_| {
                attempts.set(attempts.get() + 1);
                attempts.get() == 2
            },
            &|_| {},
        );
        assert_eq!(result.restoration, Restoration::Restored);
        assert!(result.history_retained);

        // A newer user copy during the history wait defeats both retry and
        // restoration, including a copy made through the same Writer owner.
        windows_delivery::clipboard(owner, "before newer copy").unwrap();
        let result = windows_delivery::restore_after_read(
            &image,
            owner,
            GetClipboardSequenceNumber(),
            "before newer copy",
            true,
            &|_| {
                windows_delivery::clipboard(owner, "newer copy during history wait").unwrap();
                false
            },
            &|stage| assert_ne!(stage, "clipboard_history_retry"),
        );
        assert_eq!(result.restoration, Restoration::Changed);
        assert_eq!(text(owner), "newer copy during history wait");

        // A new copy invalidates restoration even with the same owner HWND.
        let old_sequence = GetClipboardSequenceNumber();
        windows_delivery::clipboard(owner, "new copy").unwrap();
        assert!(image.restore(owner, old_sequence) == Restoration::Changed);
        assert_eq!(text(owner), "new copy");

        // Chromium includes source attribution even for plain textarea/API
        // copies. Keep these HGLOBAL payloads alongside text/HTML, byte for byte.
        let source_url = format("Chromium internal source URL");
        let source_frame = format("Chromium internal source RFH token");
        let html = format("HTML Format");
        windows_delivery::clipboard(owner, "browser text שלום").unwrap();
        assert!(acquire(owner));
        assert!(windows_delivery::put(
            source_url,
            b"https://example.invalid/"
        ));
        assert!(windows_delivery::put(
            source_frame,
            &[4, 0, 0, 0, 1, 2, 3, 4]
        ));
        assert!(windows_delivery::put(html, b"<b>browser text</b>\0"));
        let browser = Snapshot::capture().expect("ordinary Chromium copies must be preserved");
        let original_url = bytes(source_url);
        let original_frame = bytes(source_frame);
        let original_html = bytes(html);
        CloseClipboard();
        windows_delivery::clipboard(owner, "dictation").unwrap();
        assert!(browser.restore(owner, GetClipboardSequenceNumber()) == Restoration::Restored);
        assert_eq!(text(owner), "browser text שלום");
        assert!(acquire(owner));
        assert_eq!(bytes(source_url), original_url);
        assert_eq!(bytes(source_frame), original_frame);
        assert_eq!(bytes(html), original_html);
        CloseClipboard();

        // Unknown application formats may prevent preservation, never delivery.
        assert!(acquire(owner));
        assert!(windows_delivery::put(
            format("MW67 unknown payload"),
            b"opaque"
        ));
        assert!(Snapshot::capture().is_none());
        CloseClipboard();
        let outcome = windows_delivery::windows_deliver(
            owner,
            None,
            "dictation שלום",
            true,
            false,
            &Guards {
                wanted: &|| true,
                superseded: &|| false,
                recording_newer: &|| false,
                waiting: &|| {},
                target_valid: &|| true,
                mark: &|_| {},
            },
        )
        .unwrap();
        assert!(outcome.copied);
        assert!(!outcome.pasted, "no target was requested");
        assert!(outcome.message.contains("could not be preserved"));
        assert_eq!(text(owner), "dictation שלום");
        // A terminal-style consumer need not expose any field text. Delayed
        // rendering supplies exact Unicode bytes on its clipboard read.
        let mut source =
            crate::shell::clipboard_rendering::Source::new("delayed שלום", false).unwrap();
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        assert!(source.read().is_none());
        assert!(acquire(0)); // Current Chromium worker reads are anonymous.
        let data = bytes(13);
        CloseClipboard();
        assert!(data.starts_with(
            &"delayed שלום\0"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        ));
        assert_eq!(source.read(), Some(0));
        assert!(
            !source.eligible_read(42),
            "a read before paste was armed is not a receipt"
        );
        windows_delivery::clipboard(owner, "new copy wins").unwrap();
        drop(source);
        assert_eq!(text(owner), "new copy wins");
        let mut source =
            crate::shell::clipboard_rendering::Source::new("armed anonymous read", false).unwrap();
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        source.arm();
        assert!(acquire(0));
        let _ = bytes(13);
        CloseClipboard();
        assert!(source.eligible_read(42));
        assert!(source.materialize().is_some());
        assert_eq!(GetClipboardOwner(), owner);
        assert_eq!(text(owner), "armed anonymous read");
        assert_eq!(
            IsClipboardFormatAvailable(excluded),
            0,
            "ordinary history publication removes temporary exclusion"
        );
        drop(source);
        let mut source =
            crate::shell::clipboard_rendering::Source::new("unread fallback", false).unwrap();
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        drop(source);
        assert_eq!(text(owner), "unread fallback");
        assert_eq!(IsClipboardFormatAvailable(excluded), 0);

        // Privacy is applied before the first publication, including copy-only
        // fallback. Delayed cleanup must never re-enable history or cloud sync.
        let history = format("CanIncludeInClipboardHistory");
        let cloud = format("CanUploadToCloudClipboard");
        let check_private = || {
            assert_ne!(IsClipboardFormatAvailable(excluded), 0);
            assert!(acquire(owner));
            assert_eq!(&bytes(history)[..4], &0u32.to_le_bytes());
            assert_eq!(&bytes(cloud)[..4], &0u32.to_le_bytes());
            CloseClipboard();
        };
        for preserve in [false, true] {
            windows_delivery::clipboard(owner, "previous synthetic copy").unwrap();
            let outcome = windows_delivery::windows_deliver(
                owner,
                None,
                "private dictation שלום",
                preserve,
                true,
                &Guards {
                    wanted: &|| true,
                    superseded: &|| false,
                    recording_newer: &|| false,
                    waiting: &|| {},
                    target_valid: &|| true,
                    mark: &|_| {},
                },
            )
            .unwrap();
            assert!(outcome.copied && !outcome.pasted);
            check_private();
            assert_eq!(text(owner), "private dictation שלום");
            let mut source =
                crate::shell::clipboard_rendering::Source::new("private dictation שלום", true)
                    .unwrap();
            assert!(source
                .prepare(owner, GetClipboardSequenceNumber())
                .is_some());
            assert_ne!(IsClipboardFormatAvailable(excluded), 0);
            source.arm();
            assert_eq!(text(owner), "private dictation שלום");
            assert!(source.materialize().is_some());
            check_private();
            drop(source);
            check_private();
        }
        // Unread/drop fallback retains privacy; a concurrent explicit Copy wins
        // and keeps its ordinary history policy.
        let mut source =
            crate::shell::clipboard_rendering::Source::new("private unread", true).unwrap();
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        drop(source);
        check_private();
        assert_eq!(text(owner), "private unread");
        let mut source =
            crate::shell::clipboard_rendering::Source::new("private replaced", true).unwrap();
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        windows_delivery::clipboard(owner, "new explicit copy").unwrap();
        assert!(source.materialize().is_none());
        drop(source);
        assert_eq!(text(owner), "new explicit copy");
        assert_eq!(IsClipboardFormatAvailable(excluded), 0);
        assert!(acquire(owner));
        assert_eq!(&bytes(history)[..4], &1u32.to_le_bytes());
        CloseClipboard();

        // An eager clipboard bridge (WSLg's msrdc) reads every update through
        // its own window. Its read before paste is served; the promise is then
        // offered again and its repeat refused, leaving the render for the paste.
        let bridge = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("private clipboard bridge").as_ptr(),
            0,
            0,
            0,
            1,
            1,
            0,
            0,
            0,
            std::ptr::null(),
        );
        assert_ne!(bridge, 0);
        let mut source =
            crate::shell::clipboard_rendering::Source::new("bridged שלום", false).unwrap();
        source.target(42);
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        assert!(acquire(bridge));
        let _ = bytes(13);
        CloseClipboard();
        assert_eq!(source.read(), Some(std::process::id()));
        let (sequence, reoffered) = source.settle().unwrap();
        assert!(reoffered);
        assert_eq!(sequence, GetClipboardSequenceNumber());
        assert!(source.read().is_none());
        assert_ne!(IsClipboardFormatAvailable(excluded), 0);
        assert!(acquire(bridge));
        assert_eq!(GetClipboardData(13), 0, "a served bridge is refused");
        CloseClipboard();
        assert!(source.refused() && source.read().is_none());
        source.arm();
        assert!(acquire(0));
        let _ = bytes(13);
        CloseClipboard();
        assert!(source.eligible_read(42));
        assert!(source.materialize().is_some());
        assert_eq!(text(owner), "bridged שלום");
        drop(source);
        // The bridge is remembered: a later promise survives its first read,
        // and unread dictation still becomes ordinary text.
        let mut source =
            crate::shell::clipboard_rendering::Source::new("bridged again", false).unwrap();
        source.target(42);
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        assert!(acquire(bridge));
        assert_eq!(GetClipboardData(13), 0);
        CloseClipboard();
        let (_, reoffered) = source.settle().unwrap();
        assert!(!reoffered && source.read().is_none());
        drop(source);
        assert_eq!(text(owner), "bridged again");
        // The paste target is never refused, even after acting as a bridge.
        let mut source =
            crate::shell::clipboard_rendering::Source::new("bridge is target", false).unwrap();
        source.target(std::process::id());
        assert!(source
            .prepare(owner, GetClipboardSequenceNumber())
            .is_some());
        source.arm();
        assert!(acquire(bridge));
        let _ = bytes(13);
        CloseClipboard();
        assert!(source.eligible_read(std::process::id()));
        assert!(source.materialize().is_some());
        drop(source);
        assert_eq!(text(owner), "bridge is target");
        DestroyWindow(bridge);

        assert_eq!(GetProcessWindowStation(), station.station);
        assert!(acquire(owner));
        assert_ne!(EmptyClipboard(), 0);
        CloseClipboard();
    }
}

#[test]
fn history_text_matching_skips_non_text_items() {
    use windows::ApplicationModel::DataTransfer::DataPackage;
    let _apartment = Apartment::new().expect("COM");
    let non_text = DataPackage::new().unwrap();
    non_text
        .SetHtmlFormat(&"<b>no text provider</b>".into())
        .unwrap();
    assert!(!contains_text(&non_text.GetView().unwrap(), "dictation").unwrap());
    let text = DataPackage::new().unwrap();
    text.SetText(&"dictation שלום".into()).unwrap();
    assert!(contains_text(&text.GetView().unwrap(), "dictation שלום").unwrap());
    assert!(!contains_text(&text.GetView().unwrap(), "another dictation").unwrap());
}
