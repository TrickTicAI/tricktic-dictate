//! Include process creation time: a recycled HWND and PID are not the old app.
use super::{delivery::Target, win_input};
#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, id: u32) -> isize;
    fn GetProcessTimes(
        process: isize,
        created: *mut u64,
        exited: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn QueryFullProcessImageNameW(
        process: isize,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
}
#[link(name = "version")]
extern "system" {
    fn GetFileVersionInfoSizeW(file: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(file: *const u16, handle: u32, length: u32, data: *mut u8) -> i32;
    fn VerQueryValueW(
        block: *const u8,
        sub_block: *const u16,
        buffer: *mut *mut u8,
        length: *mut u32,
    ) -> i32;
}
pub fn created(process: u32) -> Option<u64> {
    unsafe {
        let handle = OpenProcess(0x1000, 0, process); // QUERY_LIMITED_INFORMATION
        if handle == 0 {
            return None;
        }
        let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
        let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        (ok && created != 0 && exited == 0).then_some(created)
    }
}
pub fn alive(target: Target) -> bool {
    unsafe {
        let mut process = 0;
        win_input::GetWindowThreadProcessId(target.window, &mut process);
        win_input::IsWindow(target.window) != 0
            && process == target.process
            && created(process) == Some(target.created)
    }
}
/// A bounded dedicated COM worker keeps UI Automation objects on one thread.
/// Unknown/password controls and focus changes fail closed to manual paste.
pub fn paste_safe(window: isize, process: u32) -> bool {
    use std::sync::{mpsc, OnceLock};
    type Request = (isize, u32, mpsc::SyncSender<bool>);
    static WORKER: OnceLock<mpsc::SyncSender<Request>> = OnceLock::new();
    let sender = WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Request>(8);
        std::thread::spawn(move || unsafe {
            use windows::{
                core::Interface,
                Win32::{System::Com::*, UI::Accessibility::*},
            };
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                return;
            }
            let automation: windows::core::Result<IUIAutomation> =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER);
            if let Ok(automation) = automation {
                if let Ok(timed) = automation.cast::<IUIAutomation2>() {
                    let _ = timed.SetConnectionTimeout(1000);
                    let _ = timed.SetTransactionTimeout(1000);
                }
                for (window, process, reply) in receiver {
                    let foreground = || {
                        let mut current = 0;
                        win_input::GetWindowThreadProcessId(window, &mut current);
                        current == process
                            && process != std::process::id()
                            && win_input::IsWindow(window) != 0
                            && win_input::GetForegroundWindow() == window
                    };
                    let safe = foreground()
                        && automation
                            .GetFocusedElement()
                            .and_then(|element| element.CurrentIsPassword())
                            .is_ok_and(|password| !password.as_bool())
                        && foreground();
                    let _ = reply.send(safe);
                }
            }
            CoUninitialize();
        });
        sender
    });
    let (reply, response) = mpsc::sync_channel(1);
    sender.try_send((window, process, reply)).is_ok()
        && response
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap_or(false)
}
/// The application's own name for itself — "Slack", "Microsoft Word" — from
/// its file description, or its file name when it gives none. Never its path,
/// and never a window title, which can name a document. `None` for a host
/// that only frames another app, whose name would be the wrong one.
pub fn application(process: u32) -> Option<String> {
    let path = unsafe {
        let handle = OpenProcess(0x1000, 0, process); // QUERY_LIMITED_INFORMATION
        if handle == 0 {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) != 0;
        CloseHandle(handle);
        if !ok || size == 0 {
            return None;
        }
        String::from_utf16_lossy(&buffer[..size as usize])
    };
    let file = std::path::Path::new(&path);
    if file
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("ApplicationFrameHost.exe"))
    {
        return None;
    }
    description(&path)
        .or_else(|| Some(file.file_stem()?.to_string_lossy().into_owned()))
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty() && name.chars().count() <= 80)
}
/// The `FileDescription` in an executable's version resource, in the first
/// language it declares.
fn description(path: &str) -> Option<String> {
    let file: Vec<u16> = path.encode_utf16().chain([0]).collect();
    unsafe {
        let mut ignored = 0;
        let length = GetFileVersionInfoSizeW(file.as_ptr(), &mut ignored);
        if length == 0 {
            return None;
        }
        // Whole u32s, so the UTF-16 text found inside stays aligned.
        let mut block = vec![0u32; (length as usize).div_ceil(4)];
        let data = block.as_mut_ptr().cast::<u8>();
        if GetFileVersionInfoW(file.as_ptr(), 0, length, data) == 0 {
            return None;
        }
        let query = |path: &str| -> Option<(*mut u8, u32)> {
            let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
            let (mut found, mut size) = (std::ptr::null_mut(), 0);
            (VerQueryValueW(data, path.as_ptr(), &mut found, &mut size) != 0 && !found.is_null())
                .then_some((found, size))
        };
        let (translation, size) = query("\\VarFileInfo\\Translation")?;
        if size < 4 {
            return None;
        }
        let pair = std::slice::from_raw_parts(translation.cast::<u16>(), 2);
        let (text, size) = query(&format!(
            "\\StringFileInfo\\{:04x}{:04x}\\FileDescription",
            pair[0], pair[1]
        ))?;
        let units = std::slice::from_raw_parts(text.cast::<u16>(), size as usize);
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        Some(String::from_utf16_lossy(&units[..end]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_application_is_named_by_its_description_or_its_file_never_its_path() {
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert_eq!(
            description(&format!(r"{system}\System32\cmd.exe")).as_deref(),
            Some("Windows Command Processor")
        );
        // This binary carries Writer's own version resource.
        assert_eq!(
            application(std::process::id()).as_deref(),
            Some(product_name!())
        );
        assert_eq!(application(0), None);
    }
    #[test]
    fn process_generation_disambiguates_a_reused_pid_and_unknown_processes() {
        let pid = std::process::id();
        let born = created(pid).unwrap();
        assert_eq!(created(pid), Some(born));
        assert_ne!(created(pid), Some(born + 1));
        assert_eq!(created(0), None);
        assert!(!alive(Target {
            window: 0,
            process: pid,
            created: born
        }));
    }
}
