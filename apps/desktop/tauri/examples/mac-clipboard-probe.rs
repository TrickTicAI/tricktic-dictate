//! Explicit macOS acceptance for clipboard preservation on the real pasteboard.
//! Never launched by Writer or ordinary tests. A separate process reads the
//! pasteboard in place of an application handling Cmd+V; the provider answers
//! on this process's main run loop, as it does inside Writer.
//!
//! The user's pasteboard is snapshotted first and restored last. With
//! `--copyq PATH`, the probe also checks CopyQ's history order and removes only
//! the rows holding its own synthetic markers.
#![allow(dead_code)]
#[cfg(target_os = "macos")]
#[path = "../src/shell/macos_clipboard.rs"]
mod macos_clipboard;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    let copyq = std::env::args().skip_while(|a| a != "--copyq").nth(1);
    std::thread::spawn(move || {
        let code = match probe(copyq.as_deref()) {
            Ok(()) => 0,
            Err(message) => {
                eprintln!("FAIL: {message}");
                1
            }
        };
        std::process::exit(code);
    });
    core_foundation::runloop::CFRunLoop::run_current();
}

#[cfg(target_os = "macos")]
fn probe(copyq: Option<&str>) -> Result<(), String> {
    use macos_clipboard::*;
    use std::process::Command;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    const PREVIOUS: &str = "MW-71 previous בדיקה";
    const DICTATION: &str = "MW-71 dictation שלום mixed";
    let wait_read = |published: &Published, ms: u64| {
        for _ in 0..ms / 10 {
            if published.read.load(Ordering::Acquire) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        published.read.load(Ordering::Acquire)
    };
    let paste = || -> Result<String, String> {
        let out = Command::new("/usr/bin/pbpaste")
            .output()
            .map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let count = || -> isize {
        objc2::rc::autoreleasepool(|_| {
            objc2_app_kit::NSPasteboard::generalPasteboard().changeCount()
        })
    };
    let user = snapshot();
    let result = (|| -> Result<(), String> {
        // Previous clipboard: text, HTML and a private type, all restored byte for byte.
        let previous: Snapshot = vec![vec![
            (
                "public.utf8-plain-text".into(),
                PREVIOUS.as_bytes().to_vec(),
            ),
            ("public.html".into(), b"<b>MW-71 previous</b>".to_vec()),
            ("com.example.mw71-private".into(), vec![0, 1, 2, 255]),
        ]];
        restore(&previous, count())
            .then_some(())
            .ok_or("seed previous")?;
        std::thread::sleep(Duration::from_millis(1200)); // let a history reader record it
        let taken = snapshot().ok_or("snapshot unavailable")?;
        for entry in &previous[0] {
            if !taken[0].contains(entry) {
                return Err(format!("snapshot missed {}", entry.0));
            }
        }

        // 1. Observed paste, history offer, restore.
        let published = publish(DICTATION, count(), false).ok_or("publish")?;
        let pasted = paste()?;
        if pasted != DICTATION || !wait_read(&published, 2000) {
            return Err(format!(
                "paste read {pasted:?}, observed {}",
                published.read.load(Ordering::Acquire)
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
        let offered = publish(DICTATION, published.generation, false).ok_or("history offer")?;
        let history_read = wait_read(&offered, 1500);
        if !restore(&taken, offered.generation) {
            return Err("restore refused".into());
        }
        let after = snapshot().ok_or("snapshot after restore")?;
        for entry in &previous[0] {
            if !after[0].contains(entry) {
                return Err(format!("restore lost {}", entry.0));
            }
        }
        println!("PASS observed paste; history reader observed: {history_read}; previous restored byte for byte");

        if let Some(copyq) = copyq {
            std::thread::sleep(Duration::from_millis(1500));
            let row = |n: &str| {
                Command::new(copyq)
                    .args(["read", n])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            };
            let (first, second) = (
                row("0").map_err(|e| e.to_string())?,
                row("1").map_err(|e| e.to_string())?,
            );
            println!("CopyQ row 0: {first:?}; row 1: {second:?}");
            let ordered = first == PREVIOUS && second == DICTATION;
            // Remove only this probe's synthetic rows, highest row first.
            for n in ["1", "0"] {
                let text = row(n).unwrap_or_default();
                if text == PREVIOUS || text == DICTATION {
                    let _ = Command::new(copyq).args(["remove", n]).status();
                }
            }
            if !ordered {
                return Err("CopyQ did not list previous first and dictation second".into());
            }
            println!("PASS CopyQ lists previous copy first and dictation second");
        }

        // 2. A newer copy after publication always wins.
        let published = publish(DICTATION, count(), false).ok_or("publish 2")?;
        Command::new("/bin/sh")
            .args(["-c", "printf 'MW-71 newer copy' | pbcopy"])
            .status()
            .map_err(|e| e.to_string())?;
        if restore(&taken, published.generation)
            || materialize(DICTATION, published.generation, false)
            || paste()? != "MW-71 newer copy"
        {
            return Err("newer copy was overwritten".into());
        }
        println!("PASS newer copy wins");

        // 3. Unread publication becomes ordinary text for a late paste.
        let published = publish(DICTATION, count(), false).ok_or("publish 3")?;
        if !materialize(DICTATION, published.generation, false) {
            return Err("materialize refused".into());
        }
        drop(published);
        if paste()? != DICTATION {
            return Err("late paste after materialize".into());
        }
        println!("PASS unread dictation stays pasteable after the provider is released");

        // 4. Private automatic publications carry cooperative markers on the
        // same item as text, including lazy read and manual-paste fallback.
        let check_markers = |transient: bool| -> Result<(), String> {
            objc2::rc::autoreleasepool(|_| {
                let board = objc2_app_kit::NSPasteboard::generalPasteboard();
                let items = board.pasteboardItems().ok_or("private item missing")?;
                let item = items.firstObject().ok_or("private item missing")?;
                let types: Vec<_> = item.types().iter().map(|t| t.to_string()).collect();
                for marker in [
                    "org.nspasteboard.ConcealedType",
                    "org.nspasteboard.AutoGeneratedType",
                ] {
                    if !types.iter().any(|t| t == marker) {
                        return Err(format!("private marker missing: {marker}"));
                    }
                }
                if types.iter().any(|t| t == "org.nspasteboard.TransientType") != transient {
                    return Err("incorrect transient marker".into());
                }
                Ok(())
            })
        };
        let generation = write(DICTATION, true).ok_or("private write")?;
        check_markers(false)?;
        let published = publish(DICTATION, generation, true).ok_or("private lazy write")?;
        check_markers(true)?;
        if paste()? != DICTATION || !wait_read(&published, 1000) {
            return Err("private lazy paste was not observed".into());
        }
        if !materialize(DICTATION, published.generation, true) {
            return Err("private fallback refused".into());
        }
        check_markers(false)?;
        drop(published);
        if paste()? != DICTATION {
            return Err("private fallback text unavailable".into());
        }
        let published = publish(DICTATION, count(), true).ok_or("private concurrent copy")?;
        Command::new("/bin/sh")
            .args(["-c", "printf 'MW-71 newer copy' | pbcopy"])
            .status()
            .map_err(|e| e.to_string())?;
        if materialize(DICTATION, published.generation, true)
            || restore(&taken, published.generation)
            || paste()? != "MW-71 newer copy"
        {
            return Err("private delivery replaced a newer copy".into());
        }
        println!("PASS private markers, lazy read, fallback and concurrent copy");
        Ok(())
    })();
    match user {
        Some(user) => {
            restore(&user, count());
        }
        None => {
            objc2::rc::autoreleasepool(|_| {
                objc2_app_kit::NSPasteboard::generalPasteboard().clearContents()
            });
        }
    }
    if let Some(copyq) = copyq {
        std::thread::sleep(Duration::from_millis(1200));
        // Synthetic rows from steps 2 and 3, if CopyQ recorded them.
        for _ in 0..4 {
            for n in ["0", "1", "2", "3"] {
                let text = Command::new(copyq)
                    .args(["read", n])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default();
                if text.starts_with("MW-71 ") {
                    let _ = Command::new(copyq).args(["remove", n]).status();
                    break;
                }
            }
        }
    }
    result
}
