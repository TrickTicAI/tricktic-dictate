//! Minimal Win32 declarations shared by modifier capture and text delivery.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Keyboard {
    pub vk: u16,
    pub scan: u16,
    pub flags: u32,
    pub time: u32,
    pub extra: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union Payload {
    pub keyboard: Keyboard,
    pub padding: [usize; 4],
}
#[repr(C)]
pub struct Input {
    pub kind: u32,
    pub payload: Payload,
}
pub fn key(vk: u16, up: bool) -> Input {
    Input {
        kind: 1,
        payload: Payload {
            keyboard: Keyboard {
                vk,
                scan: 0,
                flags: if up { 2 } else { 0 },
                time: 0,
                extra: 0,
            },
        },
    }
}
/// The complete automatic delivery chord. Transcript bytes never become keys.
pub fn paste_keys() -> [Input; 4] {
    [
        key(0x11, false),
        key(0x56, false),
        key(0x56, true),
        key(0x11, true),
    ]
}
#[link(name = "user32")]
extern "system" {
    pub fn ShowWindow(window: isize, command: i32) -> i32;
    pub fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
    pub fn GetAsyncKeyState(key: i32) -> i16;
    pub fn GetForegroundWindow() -> isize;
    pub fn SetForegroundWindow(window: isize) -> i32;
    pub fn BringWindowToTop(window: isize) -> i32;
    pub fn IsIconic(window: isize) -> i32;
    pub fn IsHungAppWindow(window: isize) -> i32;
    pub fn AttachThreadInput(attaching: u32, attached_to: u32, attach: i32) -> i32;
    pub fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
    pub fn IsWindow(window: isize) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    pub fn GetCurrentThreadId() -> u32;
}
pub fn send(keys: &[Input]) -> bool {
    unsafe {
        SendInput(
            keys.len() as u32,
            keys.as_ptr(),
            std::mem::size_of::<Input>() as i32,
        ) == keys.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_paste_never_sends_enter_or_text_as_keystrokes() {
        let keys = paste_keys();
        let events: Vec<_> = keys
            .iter()
            .map(|event| unsafe { (event.payload.keyboard.vk, event.payload.keyboard.flags) })
            .collect();
        assert_eq!(events, [(0x11, 0), (0x56, 0), (0x56, 2), (0x11, 2)]);
        assert!(events.iter().all(|(key, _)| *key != 0x0d));
    }
    #[test]
    fn x64_input_matches_win32_abi() {
        assert_eq!(std::mem::size_of::<Input>(), 40);
        assert_eq!(std::mem::align_of::<Input>(), 8);
        let up = key(0x56, true);
        assert_eq!(up.kind, 1);
        unsafe {
            assert_eq!(up.payload.keyboard.vk, 0x56);
            assert_eq!(up.payload.keyboard.flags, 2);
        }
    }
}
