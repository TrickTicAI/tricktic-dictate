use super::*;
use metrix_writer_api::cleanup::modes::ModeSettings;
use tauri_plugin_global_shortcut::{Modifiers, Shortcut};
pub type Bindings = Vec<(Shortcut, String)>;
static HELD: Mutex<Vec<Shortcut>> = Mutex::new(Vec::new());
fn modifier_prefix(chord: &str) -> Result<Option<Modifiers>> {
    if !preferences::modifier_only(chord) {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    if preferences::keys(chord)?.contains(&super::macos_keys::FN) {
        return Ok(None);
    }
    let mut flags = Modifiers::empty();
    for key in preferences::keys(chord)? {
        flags |= match key {
            0x10 => Modifiers::SHIFT,
            0x11 => Modifiers::CONTROL,
            0x12 => Modifiers::ALT,
            0x5b => Modifiers::SUPER,
            _ => Modifiers::empty(),
        };
    }
    Ok(Some(flags))
}
pub fn validate_main(bindings: &Bindings, preferences: &preferences::Preferences) -> Result<()> {
    for chord in [&preferences.recording, &preferences.cancel] {
        if let Some(prefix) = modifier_prefix(chord)? {
            if bindings.iter().any(|(key, _)| key.mods.contains(prefix)) {
                return Err("A mode shortcut cannot start with the modifier-only recording/cancel shortcut. Choose different modifiers.".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifier_only_prefixes_are_validated_for_the_platform() {
        let mut preferences = preferences::Preferences {
            recording: "Win+Shift".into(),
            ..Default::default()
        };
        for chord in ["Win+Shift+K", "Win+Shift+Alt+K"] {
            let key = preferences::registered(chord).unwrap().unwrap();
            assert!(validate_main(&vec![(key, "super".into())], &preferences).is_err());
        }
        let key = preferences::registered("Ctrl+Alt+K").unwrap().unwrap();
        let result = validate_main(&vec![(key, "super".into())], &preferences);
        #[cfg(any(windows, target_os = "macos"))]
        assert!(result.is_ok());
        #[cfg(not(any(windows, target_os = "macos")))]
        assert_eq!(
            result.unwrap_err(),
            "Modifier-only shortcuts currently require Windows."
        );
        preferences.recording = "Ctrl+Shift+F9".into();
        let key = preferences::registered("Ctrl+Shift+K").unwrap().unwrap();
        assert!(validate_main(&vec![(key, "super".into())], &preferences).is_ok());
    }
}

fn desired(settings: &ModeSettings, preferences: &preferences::Preferences) -> Result<Bindings> {
    let reserved = [
        preferences::recording_registration(preferences)?,
        preferences::registered(&preferences.cancel)?,
    ];
    let mut bindings: Bindings = Vec::new();
    for profile in &settings.profiles {
        if let Some(chord) = &profile.shortcut {
            let key = preferences::registered(chord)?
                .ok_or("Mode shortcuts need modifiers and a letter, number or function key.")?;
            if reserved.contains(&Some(key)) || bindings.iter().any(|(other, _)| *other == key) {
                return Err("Each mode shortcut must be different from other modes and the main recording/cancel shortcuts.".into());
            }
            bindings.push((key, profile.id.clone()));
        }
    }
    validate_main(&bindings, preferences)?;
    Ok(bindings)
}
fn acquire(app: &tauri::AppHandle, old: &Bindings, next: &Bindings) -> Result<Vec<Shortcut>> {
    let mut added = Vec::new();
    for (key, _) in next {
        if !old.iter().any(|(k, _)| k == key) {
            if app.global_shortcut().register(*key).is_err() {
                for key in added {
                    let _ = app.global_shortcut().unregister(key);
                }
                return Err("A mode shortcut is already used by another app. Previous settings are retained.".into());
            }
            added.push(*key);
        }
    }
    Ok(added)
}
pub fn initialize(app: &tauri::AppHandle, settings: &ModeSettings) -> Result<()> {
    let state = app.state::<Shell>();
    let next = desired(settings, &state.preferences.lock().unwrap())?;
    acquire(app, &Vec::new(), &next)?;
    *state.mode_shortcuts.lock().unwrap() = next;
    Ok(())
}
#[tauri::command]
pub async fn modes_save(
    window: WebviewWindow,
    app: tauri::AppHandle,
    settings: ModeSettings,
) -> Result<ModeSettings> {
    main_only(&window)?;
    let state = app.state::<Shell>();
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "Settings maintenance is busy.")?;
    let next = desired(&settings, &state.preferences.lock().unwrap())?;
    let old = state.mode_shortcuts.lock().unwrap().clone();
    if (next.len() != old.len() || next.iter().any(|binding| !old.contains(binding)))
        && (state.capture.status()?.active || state.pending.lock().unwrap().is_some())
    {
        return Err("Finish recording before changing mode shortcuts.".into());
    }
    let service = metrix_writer_api::cleanup::modes::service()
        .await
        .map_err(|e| e.message)?;
    let added = acquire(&app, &old, &next)?;
    match service.save(settings).await {
        Ok(saved) => {
            for (key, _) in old {
                if !next.iter().any(|(k, _)| *k == key) {
                    let _ = app.global_shortcut().unregister(key);
                }
            }
            *state.mode_shortcuts.lock().unwrap() = next;
            Ok(saved)
        }
        Err(error) => {
            for key in added {
                let _ = app.global_shortcut().unregister(key);
            }
            Err(error.message)
        }
    }
}
pub fn handle(app: &tauri::AppHandle, key: &Shortcut, pressed: bool) -> bool {
    let state = app.state::<Shell>();
    let mode = state
        .mode_shortcuts
        .lock()
        .unwrap()
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, m)| m.clone());
    let Some(mode) = mode else {
        return false;
    };
    if !pressed {
        HELD.lock().unwrap().retain(|held| held != key);
        return true;
    }
    if state.shortcut_capture.load(Ordering::Acquire) {
        return true;
    }
    {
        let mut held = HELD.lock().unwrap();
        if held.contains(key) {
            return true;
        }
        held.push(*key);
    }
    if state.capture.status().is_ok_and(|s| s.active) || state.pending.lock().unwrap().is_some() {
        dispatch(app, "stop");
    } else {
        dispatch_full(app, "start", None, Some(mode));
    }
    true
}
