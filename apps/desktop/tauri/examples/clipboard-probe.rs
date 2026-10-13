//! Explicit native acceptance with a separately launched synthetic target.
//! Never launched by Writer or ordinary tests. See smoke-clipboard.ps1.
#![allow(dead_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]
#[macro_use]
#[path = "../src/product.rs"]
mod product;
#[cfg(windows)]
#[path = "../src/shell/clipboard_preservation.rs"]
mod clipboard_preservation;
#[cfg(windows)]
#[path = "../src/shell/clipboard_rendering.rs"]
mod clipboard_rendering;
#[cfg(windows)]
#[path = "../src/shell/input_guard.rs"]
mod input_guard;
#[cfg(windows)]
#[path = "../src/shell/win_input.rs"]
mod win_input;
#[cfg(windows)]
#[path = "../src/shell/win_target.rs"]
mod win_target;
#[cfg(windows)]
#[path = "../src/shell/windows_delivery.rs"]
mod windows_delivery;
mod storage {
    pub type Result<T> = std::result::Result<T, String>;
}
mod delivery {
    #[derive(Clone, Copy)]
    pub struct Target {
        pub window: isize,
        pub process: u32,
        pub created: u64,
    }
    pub struct Outcome {
        pub copied: bool,
        pub pasted: bool,
        pub message: String,
    }
    pub struct Guards<'a> {
        pub wanted: &'a dyn Fn() -> bool,
        pub superseded: &'a dyn Fn() -> bool,
        pub recording_newer: &'a dyn Fn() -> bool,
        pub waiting: &'a dyn Fn(),
        pub target_valid: &'a dyn Fn() -> bool,
        pub mark: &'a dyn Fn(&'static str),
    }
}
#[cfg(windows)]
fn main() {
    use clipboard_preservation::*;
    use std::time::Duration;
    use windows::ApplicationModel::DataTransfer::Clipboard;
    #[link(name = "user32")]
    extern "system" {
        fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: isize,
            menu: isize,
            instance: isize,
            param: *const u8,
        ) -> isize;
        fn GetProcessWindowStation() -> isize;
        fn GetUserObjectInformationW(
            handle: isize,
            index: i32,
            info: *mut u32,
            size: u32,
            needed: *mut u32,
        ) -> i32;
        fn CountClipboardFormats() -> i32;
        fn OpenClipboard(owner: isize) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardSequenceNumber() -> u32;
        fn GetClipboardOwner() -> isize;
        fn GetClipboardData(format: u32) -> isize;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn PostMessageW(window: isize, message: u32, wp: usize, lp: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(memory: isize) -> *mut u8;
        fn GlobalUnlock(memory: isize) -> i32;
        fn GlobalSize(memory: isize) -> usize;
    }
    fn read(owner: isize) -> Option<(String, u32, isize)> {
        unsafe {
            if OpenClipboard(owner) == 0 {
                return None;
            }
            let memory = GetClipboardData(13);
            let ptr = GlobalLock(memory) as *const u16;
            let result = if ptr.is_null() {
                None
            } else {
                let slice = std::slice::from_raw_parts(ptr, GlobalSize(memory) / 2);
                Some(String::from_utf16_lossy(
                    &slice[..slice.iter().position(|v| *v == 0).unwrap_or(slice.len())],
                ))
            };
            if !ptr.is_null() {
                GlobalUnlock(memory);
            }
            let result =
                result.map(|text| (text, GetClipboardSequenceNumber(), GetClipboardOwner()));
            CloseClipboard();
            result
        }
    }
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--check-transport" {
        std::fs::write(&args[2], br#"{"transport":true}"#).unwrap();
        return;
    }
    if args.len() == 3 && args[1] == "--private-source" {
        // This mode refuses the interactive window station before any writes.
        unsafe {
            let mut flags = [0u32; 3];
            let mut needed = 0;
            assert_ne!(
                GetUserObjectInformationW(
                    GetProcessWindowStation(),
                    1,
                    flags.as_mut_ptr(),
                    12,
                    &mut needed
                ),
                0
            );
            assert_eq!(
                flags[2] & 1,
                0,
                "private-source must not touch the interactive clipboard"
            );
            let directory = std::path::PathBuf::from(&args[2]);
            let config: serde_json::Value =
                serde_json::from_slice(&std::fs::read(directory.join("config.json")).unwrap())
                    .unwrap();
            let case = config["case"].as_str().unwrap();
            let target = config["browser_pid"].as_u64().unwrap() as u32;
            let text = "Writer private paste שלום English";
            let previous = "previous private clipboard";
            let class: Vec<_> = "STATIC\0".encode_utf16().collect();
            let owner = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
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
            assert_ne!(owner, 0);
            // The launcher gives this process a dedicated noninteractive station.
            // Other fixture processes share it, never the user's clipboard.
            let _ = CountClipboardFormats();
            if !case.starts_with("browser-") {
                windows_delivery::clipboard(owner, previous).unwrap();
            }
            assert!(windows_delivery::acquire(owner));
            let saved = Snapshot::capture().unwrap();
            CloseClipboard();
            windows_delivery::clipboard(owner, text).unwrap();
            let mut source = clipboard_rendering::Source::new(text, false).unwrap();
            assert!(source
                .prepare(owner, GetClipboardSequenceNumber())
                .is_some());
            if case == "early-reader" {
                let _ = read(owner);
            }
            source.arm();
            if case == "other-reader" {
                let _ = read(owner);
            }
            std::fs::write(directory.join("ready.json"), b"{}").unwrap();
            let started = std::time::Instant::now();
            let mut observed = None;
            let mut eligible = false;
            loop {
                if source.eligible_read(target) {
                    let at = observed.get_or_insert_with(std::time::Instant::now);
                    if at.elapsed() >= Duration::from_millis(100) {
                        eligible = true;
                        break;
                    }
                } else if source.read().is_some() {
                    break;
                }
                if started.elapsed() >= Duration::from_secs(2) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if case == "new-copy" {
                windows_delivery::clipboard(owner, "new private copy").unwrap();
            }
            let sequence = source.materialize();
            let restored = eligible
                && sequence.is_some_and(|seq| saved.restore(owner, seq) == Restoration::Restored);
            let current = read(owner).map(|r| r.0);
            let formats_restored = if restored && windows_delivery::acquire(owner) {
                let same = Snapshot::capture().as_ref() == Some(&saved);
                CloseClipboard();
                same
            } else {
                false
            };
            let receipt = serde_json::json!({
                "case": case, "read": source.read().is_some(), "anonymous_reader": source.read() == Some(0),
                "eligible": eligible, "restored": restored, "formats_restored": formats_restored, "clipboard_is_previous": current.as_deref() == Some(previous),
                "clipboard_is_dictation": current.as_deref() == Some(text), "new_copy_kept": current.as_deref() == Some("new private copy"),
                "elapsed_ms": started.elapsed().as_millis(), "interactive_clipboard_touched": false,
            });
            std::fs::write(
                directory.join("result.json"),
                serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap();
            // Keep the ordinary owner alive until the browser's late-read case
            // finishes, so native clipboard ownership stays meaningful.
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while !directory.join("done").exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(source);
            if windows_delivery::acquire(owner) {
                if GetClipboardOwner() == owner {
                    extern "system" {
                        fn EmptyClipboard() -> i32;
                    }
                    EmptyClipboard();
                }
                CloseClipboard();
            }
            std::fs::write(directory.join("cleaned"), b"").unwrap();
        }
        return;
    }
    if args.len() == 3 && args[1] == "--inspect-field" {
        let window = unsafe { win_input::GetForegroundWindow() };
        let started = std::time::Instant::now();
        let observable = Insertion::capture(window, "Writer synthetic observation").is_some();
        std::fs::write(
            &args[2],
            serde_json::to_vec(&serde_json::json!({
                "observable": observable,
                "elapsed_ms": started.elapsed().as_millis(),
                "clipboard_touched": false,
                "input_sent": false,
            }))
            .unwrap(),
        )
        .unwrap();
        return;
    }
    assert_eq!(args.len(), 3, "clipboard-probe <target.json> <case>");
    let target: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
    let case = &args[2];
    let _apartment = Apartment::new().expect("COM");
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let owner = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            1,
            1,
            0,
            0,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(owner, 0);
    assert!(unsafe { windows_delivery::acquire(owner) });
    let original = unsafe { Snapshot::capture() };
    unsafe {
        CloseClipboard();
    }
    let original = original
        .expect("The current clipboard cannot be safely backed up; probe did not change it.");
    let prefix = format!("MW67-{}", uuid::Uuid::new_v4());
    let previous = format!("{prefix} previous");
    let newer_copy = format!("{prefix} newer copy");
    let transcript = format!("{prefix} Hebrew שלום English\nsecond line");
    windows_delivery::clipboard(owner, &previous).unwrap();
    let history = history_enabled();
    if history {
        assert!(retained(&previous), "history must retain fixture");
    }
    if matches!(case.as_str(), "unsupported" | "rich") {
        unsafe {
            assert!(windows_delivery::acquire(owner));
            let name: Vec<u16> = if case == "rich" {
                "Rich Text Format\0"
            } else {
                "MW67 unknown live format\0"
            }
            .encode_utf16()
            .collect();
            let format = RegisterClipboardFormatW(name.as_ptr());
            assert!(windows_delivery::put(
                format,
                b"{\\rtf1 synthetic rich text}\0"
            ));
            CloseClipboard();
        }
    }
    let fixture = unsafe {
        assert!(windows_delivery::acquire(owner));
        let snapshot = Snapshot::capture();
        CloseClipboard();
        snapshot
    };
    let directory = std::path::Path::new(&args[1]).parent().unwrap();
    std::fs::write(directory.join(format!("expected-{case}.txt")), &transcript).unwrap();
    let result = windows_delivery::windows_deliver(
        owner,
        Some(delivery::Target {
            window: target["window"].as_i64().unwrap() as isize,
            created: win_target::created(target["process"].as_u64().unwrap() as u32)
                .expect("live fixture process"),
            process: target["process"].as_u64().unwrap() as u32,
        }),
        &transcript,
        case != "off",
        false,
        &delivery::Guards {
            wanted: &|| true,
            superseded: &|| false,
            recording_newer: &|| false,
            waiting: &|| {},
            target_valid: &|| true,
            mark: &|_| {},
        },
    );
    let received_path = directory.join(format!("received-{case}.json"));
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    let exact_receipt = || {
        std::fs::read(&received_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|v| v["exact"] == true)
    };
    while !exact_receipt() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let received = exact_receipt();
    // A failed fixture must stop reading before its original clipboard is put
    // back. Never turn test cleanup into the early-restore race being tested.
    let target_alive = || unsafe {
        let window = target["window"].as_i64().unwrap() as isize;
        let mut process = 0u32;
        win_input::GetWindowThreadProcessId(window, &mut process);
        win_input::IsWindow(window) != 0 && process as u64 == target["process"].as_u64().unwrap()
    };
    if !received && target_alive() {
        unsafe {
            PostMessageW(target["window"].as_i64().unwrap() as isize, 0x10, 0, 0);
        }
        let close_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while target_alive() && std::time::Instant::now() < close_deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let safe_cleanup = received || !target_alive();
    let (current, observed_sequence, observed_owner) = match read(owner) {
        Some((text, sequence, clipboard_owner)) => (Some(text), sequence, clipboard_owner),
        None => (None, 0, 0),
    };
    let pasted = result.as_ref().is_ok_and(|r| r.pasted);
    let restored = current.as_deref() == Some(&previous);
    let rich_restored = if case == "rich" && restored {
        unsafe {
            assert!(windows_delivery::acquire(owner));
            let after = Snapshot::capture();
            CloseClipboard();
            after == fixture
        }
    } else {
        true
    };
    let fallback = current.as_deref() == Some(&transcript);
    let mut order = false;
    if history && restored && retained(&previous) {
        if let Ok(items) = Clipboard::GetHistoryItemsAsync()
            .and_then(|op| op.get())
            .and_then(|r| r.Items())
        {
            order = items.Size().unwrap_or(0) > 1
                && items
                    .GetAt(1)
                    .and_then(|i| i.Content())
                    .and_then(|c| c.GetTextAsync())
                    .and_then(|op| op.get())
                    .is_ok_and(|t| t == transcript);
        }
    }
    let expected = received
        && match case.as_str() {
            "normal" | "slow" | "rich" => {
                pasted && restored && rich_restored && (!history || order)
            }
            "browser" => pasted && (fallback || (restored && (!history || order))),
            "unsupported" => pasted && fallback,
            "off" | "timeout" => pasted && fallback,
            "concurrent" => pasted && current.as_deref() == Some(newer_copy.as_str()),
            _ => false,
        };
    // Do not restore over any personal copy made during the probe.
    let ours = current.as_ref().is_some_and(|s| s.starts_with(&prefix));
    let cleanup_restored = safe_cleanup
        && ours
        && original.restore(observed_owner, observed_sequence) == Restoration::Restored;
    let expected = expected && cleanup_restored;
    // Delete only our unique synthetic history entries, never clear history.
    if history {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(items) = Clipboard::GetHistoryItemsAsync()
            .and_then(|op| op.get())
            .and_then(|r| r.Items())
        {
            for i in 0..items.Size().unwrap_or(0) {
                if let Ok(item) = items.GetAt(i) {
                    if item
                        .Content()
                        .and_then(|c| c.GetTextAsync())
                        .and_then(|op| op.get())
                        .is_ok_and(|s| s.to_string().starts_with(&prefix))
                    {
                        let _ = Clipboard::DeleteItemFromHistory(&item);
                    }
                }
            }
        }
    }
    let receipt = serde_json::json!({"case":case,"passed":expected,"cleanup_restored":cleanup_restored,"received_exact":received,"pasted":pasted,"restored":restored,"history_enabled":history,"history_order":order,"message":result.map(|r|r.message).unwrap_or_else(|e|e)});
    std::fs::write(
        directory.join(format!("delivery-{case}.json")),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();

    if !expected {
        std::process::exit(1);
    }
}
#[cfg(not(windows))]
fn main() {
    panic!("Run on Windows with a synthetic target.");
}
