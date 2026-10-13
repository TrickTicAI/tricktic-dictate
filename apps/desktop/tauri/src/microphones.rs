//! CPAL device selection, persisted beside capture journals. Never silently switch
//! a requested input to another microphone when it disappears.
use crate::storage::{Result, Store};
use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct Microphone {
    pub id: String,
    pub label: String,
}
#[derive(Default, Serialize)]
pub struct MicrophoneList {
    pub devices: Vec<Microphone>,
    pub selected_id: Option<String>,
    /// Whether System default avoids a Bluetooth headset's microphone. `None`
    /// where the platform offers no such choice.
    pub prefer_built_in: Option<bool>,
}
#[derive(Default, Deserialize, Serialize)]
struct Selection {
    device_id: Option<String>,
    /// With System default selected, record from the built-in microphone while
    /// the system input is a Bluetooth headset. Opening a headset's microphone
    /// switches it to its call profile, which sends about a second of silence
    /// first and lowers what it plays to call quality.
    #[serde(default)]
    prefer_built_in: bool,
}
fn selection(store: &Store) -> Result<Selection> {
    match std::fs::read(store.root.join("microphone.json")) {
        Ok(bytes) => serde_json::from_slice::<Selection>(&bytes)
            .map_err(|_| "Saved microphone preference is invalid. Select an input again.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Selection::default()),
        Err(_) => Err("Cannot read the saved microphone preference.".into()),
    }
}
pub fn list(store: &Store) -> Result<MicrophoneList> {
    let mut devices = Vec::new();
    for device in cpal::default_host()
        .input_devices()
        .map_err(|_| "Cannot list microphones. Check system input permissions.")?
    {
        if let Ok(name) = device.name() {
            if !devices.iter().any(|item: &Microphone| item.id == name) {
                devices.push(Microphone {
                    id: name.clone(),
                    label: name,
                });
            }
        }
    }
    let saved = selection(store)?;
    Ok(MicrophoneList {
        devices,
        selected_id: saved.device_id,
        prefer_built_in: cfg!(target_os = "macos").then_some(saved.prefer_built_in),
    })
}
fn resolve(id: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    let Some(id) = id else {
        return host.default_input_device().ok_or_else(|| "No default microphone is available. Select an input in Settings and check system microphone permissions.".into());
    };
    // CPAL 0.16 exposes names, not portable stable endpoint IDs. Refuse
    // ambiguous names rather than risk recording from the wrong microphone.
    let mut matches = host
        .input_devices()
        .map_err(|_| "Cannot list microphones.")?
        .filter(|device| device.name().is_ok_and(|name| name == id));
    let device = matches.next().ok_or("The selected microphone is disconnected or renamed. Reconnect it or choose an input in Settings.")?;
    if matches.next().is_some() {
        return Err("Multiple microphones have this name. Give them distinct names in system settings, or select System default.".into());
    }
    Ok(device)
}
/// The device a recording opens, and whether it is the built-in microphone
/// standing in for a Bluetooth headset. Only System default is ever replaced:
/// a microphone chosen by name is used as chosen.
pub fn input(store: &Store) -> Result<(cpal::Device, bool)> {
    let saved = selection(store)?;
    if saved.device_id.is_none() && saved.prefer_built_in {
        if let Some(device) = built_in_instead().and_then(|name| resolve(Some(&name)).ok()) {
            return Ok((device, true));
        }
    }
    resolve(saved.device_id.as_deref()).map(|device| (device, false))
}
#[cfg(target_os = "macos")]
fn built_in_instead() -> Option<String> {
    mac::default_input_is_bluetooth()
        .then(mac::built_in_input)
        .flatten()
}
#[cfg(not(target_os = "macos"))]
fn built_in_instead() -> Option<String> {
    None
}
pub fn select(store: &Store, device_id: Option<String>) -> Result<MicrophoneList> {
    if device_id.is_some() {
        resolve(device_id.as_deref())?;
    }
    save(
        store,
        Selection {
            device_id,
            ..selection(store).unwrap_or_default()
        },
    )
}
pub fn prefer_built_in(store: &Store, enabled: bool) -> Result<MicrophoneList> {
    if !cfg!(target_os = "macos") {
        return Err("This microphone option is available on macOS.".into());
    }
    save(
        store,
        Selection {
            prefer_built_in: enabled,
            ..selection(store)?
        },
    )
}
fn save(store: &Store, selection: Selection) -> Result<MicrophoneList> {
    let bytes = serde_json::to_vec(&selection).map_err(|_| "Cannot save microphone preference.")?;
    // Capture's owner thread serializes selection with Start. A synced new file
    // is replaced only after the entire preference was written successfully.
    use std::io::Write;
    let path = store.root.join("microphone.json");
    let pending = store.root.join("microphone.json.pending");
    let mut file =
        std::fs::File::create(&pending).map_err(|_| "Cannot save microphone preference.")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot save microphone preference.")?;
    drop(file);
    std::fs::rename(pending, path).map_err(|_| "Cannot save microphone preference.")?;
    list(store)
}

/// CoreAudio's own view of the input devices, for what CPAL does not expose:
/// how each one is connected. CPAL names devices by the same property read here.
#[cfg(target_os = "macos")]
mod mac {
    use core_foundation::{base::TCFType, string::CFString};
    use std::ffi::c_void;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }
    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> i32;
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }
    const fn code(name: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*name)
    }
    const SYSTEM: u32 = 1;
    const GLOBAL: u32 = code(b"glob");
    const INPUT: u32 = code(b"inpt");
    const DEFAULT_INPUT: u32 = code(b"dIn ");
    const DEVICES: u32 = code(b"dev#");
    const STREAMS: u32 = code(b"stm#");
    const TRANSPORT: u32 = code(b"tran");
    const NAME: u32 = code(b"lnam");
    const BUILT_IN: u32 = code(b"bltn");
    const BLUETOOTH: [u32; 2] = [code(b"blue"), code(b"blea")];

    fn address(selector: u32, scope: u32) -> Address {
        Address {
            selector,
            scope,
            element: 0,
        }
    }
    fn size(object: u32, selector: u32, scope: u32) -> Option<u32> {
        let mut size = 0;
        let status = unsafe {
            AudioObjectGetPropertyDataSize(
                object,
                &address(selector, scope),
                0,
                std::ptr::null(),
                &mut size,
            )
        };
        (status == 0).then_some(size)
    }
    fn value(object: u32, selector: u32) -> Option<u32> {
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address(selector, GLOBAL),
                0,
                std::ptr::null(),
                &mut size,
                (&mut value as *mut u32).cast(),
            )
        };
        (status == 0).then_some(value)
    }
    fn name(device: u32) -> Option<String> {
        let mut name: core_foundation::string::CFStringRef = std::ptr::null();
        let mut size = std::mem::size_of_val(&name) as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &address(NAME, GLOBAL),
                0,
                std::ptr::null(),
                &mut size,
                (&mut name as *mut core_foundation::string::CFStringRef).cast(),
            )
        };
        // The property hands over a retained string.
        (status == 0 && !name.is_null())
            .then(|| unsafe { CFString::wrap_under_create_rule(name) }.to_string())
    }
    fn devices() -> Vec<u32> {
        let Some(bytes) = size(SYSTEM, DEVICES, GLOBAL) else {
            return Vec::new();
        };
        let mut devices = vec![0u32; bytes as usize / std::mem::size_of::<u32>()];
        let mut size = bytes;
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM,
                &address(DEVICES, GLOBAL),
                0,
                std::ptr::null(),
                &mut size,
                devices.as_mut_ptr().cast(),
            )
        };
        if status != 0 {
            return Vec::new();
        }
        devices.truncate(size as usize / std::mem::size_of::<u32>());
        devices
    }
    pub fn default_input_is_bluetooth() -> bool {
        value(SYSTEM, DEFAULT_INPUT)
            .and_then(|device| value(device, TRANSPORT))
            .is_some_and(|transport| BLUETOOTH.contains(&transport))
    }
    pub fn built_in_input() -> Option<String> {
        devices()
            .into_iter()
            .filter(|device| value(*device, TRANSPORT) == Some(BUILT_IN))
            .filter(|device| size(*device, STREAMS, INPUT).is_some_and(|bytes| bytes > 0))
            .find_map(name)
    }
}
