//! Session-scoped volume reduction. Endpoint/master volume is never changed.
use std::{collections::HashMap, path::PathBuf, sync::mpsc, time::Duration};
use windows::{
    core::Interface,
    Win32::{Media::Audio::*, System::Com::*},
};
pub struct Ducking {
    sender: mpsc::Sender<(bool, Option<mpsc::SyncSender<()>>)>,
}
impl Ducking {
    pub fn start(journal: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel::<(bool, Option<mpsc::SyncSender<()>>)>();
        std::thread::spawn(move || unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                return;
            }
            let mut active = false;
            let mut retained = HashMap::new();
            // Restore only the sessions and unchanged reduced values this app
            // journaled before a previous crash. Never change master volume.
            let mut recovery: HashMap<String, (f32, f32)> = std::fs::read(&journal)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            loop {
                let mut ack = None;
                match receiver.recv_timeout(Duration::from_millis(250)) {
                    Ok((next, reply)) => {
                        active = next;
                        ack = reply;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        if recovery.is_empty() {
                            restore(&mut retained, &journal);
                        }
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if !recovery.is_empty() {
                    if let Ok(sessions) = sessions() {
                        let mut remaining = HashMap::new();
                        for (id, volume) in sessions {
                            if let Some((original, reduced)) = recovery.get(&id) {
                                if volume
                                    .GetMasterVolume()
                                    .is_ok_and(|now| same_volume(now, *reduced))
                                    && volume.SetMasterVolume(*original, std::ptr::null()).is_err()
                                {
                                    remaining.insert(id, (*original, *reduced));
                                }
                            }
                        }
                        recovery = remaining;
                        if recovery.is_empty() {
                            let _ = std::fs::remove_file(&journal);
                        } else {
                            let _ = crate::storage::atomic_json(&journal, &recovery);
                        }
                    }
                }
                if active && recovery.is_empty() {
                    reduce(&mut retained, &journal);
                } else if recovery.is_empty() {
                    restore(&mut retained, &journal);
                }
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
            }
            CoUninitialize();
        });
        Self { sender }
    }
    pub fn set_active(&self, active: bool) {
        if active {
            let _ = self.sender.send((true, None));
        } else {
            let (send, receive) = mpsc::sync_channel(1);
            if self.sender.send((false, Some(send))).is_ok() {
                let _ = receive.recv_timeout(Duration::from_secs(2));
            }
        }
    }
}
type Volumes = HashMap<String, (ISimpleAudioVolume, f32, f32)>;
fn same_volume(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.0001
}
fn save(retained: &Volumes, journal: &std::path::Path) -> bool {
    let values: HashMap<_, _> = retained
        .iter()
        .map(|(id, (_, old, reduced))| (id, (*old, *reduced)))
        .collect();
    crate::storage::atomic_json(journal, &values).is_ok()
}
unsafe fn sessions() -> windows::core::Result<Vec<(String, ISimpleAudioVolume)>> {
    let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let outputs = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
    let mut found = Vec::new();
    for index in 0..outputs.GetCount()? {
        let output = outputs.Item(index)?;
        let manager: IAudioSessionManager2 = output.Activate(CLSCTX_ALL, None)?;
        let sessions = manager.GetSessionEnumerator()?;
        for index in 0..sessions.GetCount()? {
            let control = sessions.GetSession(index)?;
            let details: IAudioSessionControl2 = control.cast()?;
            if details.GetProcessId()? == std::process::id() {
                continue;
            }
            let ptr = details.GetSessionInstanceIdentifier()?;
            let id = ptr.to_string();
            CoTaskMemFree(Some(ptr.0.cast()));
            if let Ok(id) = id {
                found.push((id, control.cast()?));
            }
        }
    }
    Ok(found)
}
unsafe fn reduce(retained: &mut Volumes, journal: &std::path::Path) {
    let Ok(sessions) = sessions() else {
        return;
    };
    for (id, volume) in sessions {
        if retained.contains_key(&id) {
            continue;
        }
        let Ok(original) = volume.GetMasterVolume() else {
            continue;
        };
        let reduced = original * 0.2;
        retained.insert(id.clone(), (volume.clone(), original, reduced));
        // Write recovery data before changing another application's volume.
        if !save(retained, journal) {
            retained.remove(&id);
            continue;
        }
        if volume.SetMasterVolume(reduced, std::ptr::null()).is_err() {
            retained.remove(&id);
        }
    }
}
unsafe fn restore(retained: &mut Volumes, journal: &std::path::Path) {
    retained.retain(|_, (volume, original, reduced)| {
        // Respect a manual volume change during dictation. A terminated audio
        // session cannot be restored and has no live level left to change.
        if volume
            .GetMasterVolume()
            .is_ok_and(|now| same_volume(now, *reduced))
        {
            volume.SetMasterVolume(*original, std::ptr::null()).is_err()
        } else {
            false
        }
    });
    if retained.is_empty() {
        let _ = std::fs::remove_file(journal);
    } else {
        save(retained, journal);
    }
}
