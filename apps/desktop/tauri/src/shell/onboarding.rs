//! Whether the first-run guide has been completed, stored beside the shell's
//! other state.
//!
//! Deliberately separate from `system::Settings`. That document is saved
//! through a Windows-only path because it owns launch at login, and refuses to
//! save at all elsewhere; whether the guide has been finished has to persist on
//! every platform this shell builds for. It is also not the API's `first_run`,
//! which only reports that no model choice has been saved yet: saving a model
//! part way through the guide must not end the guide.
use super::*;
use serde::{Deserialize, Serialize};

/// Raising this reopens the guide once for someone who finished an older one.
/// A stored value ahead of this build is left alone rather than downgraded.
pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The guide version actually finished. Zero means it never has been.
    pub completed_version: u32,
}

#[derive(Serialize)]
pub struct View {
    pub completed_version: u32,
    pub current_version: u32,
}

impl State {
    /// Unreadable or invalid state shows the guide again rather than failing to
    /// start. Nothing here can prevent the application from opening.
    fn load(root: &std::path::Path) -> Self {
        std::fs::read(root.join("State/onboarding.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn view(self) -> View {
        View {
            completed_version: self.completed_version,
            current_version: VERSION,
        }
    }
}

#[tauri::command]
pub fn onboarding_get(window: WebviewWindow, state: tauri::State<'_, Shell>) -> Result<View> {
    main_only(&window)?;
    Ok(State::load(&state.root).view())
}

/// `completed` false is what "run the guide again" stores, so replaying it is
/// the same single write as finishing it.
#[tauri::command]
pub fn onboarding_save(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    completed: bool,
) -> Result<View> {
    main_only(&window)?;
    let next = State {
        completed_version: if completed { VERSION } else { 0 },
    };
    // Everything else in State/ is written by components that create it first;
    // this one can be the earliest write of a fresh profile.
    let _ = std::fs::create_dir_all(state.root.join("State"));
    storage::atomic_json(&state.root.join("State/onboarding.json"), &next)
        .map_err(|_| "Could not record setup progress. The guide will open again next time.")?;
    Ok(next.view())
}

/// Separate capability view: no persisted permission claim and no microphone gate.
#[tauri::command]
pub fn accessibility_status(window: WebviewWindow) -> Result<Value> {
    main_only(&window)?;
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{ns_string, NSNumber, NSUserDefaults};
        let domain = NSUserDefaults::standardUserDefaults()
            .persistentDomainForName(ns_string!("com.apple.HIToolbox"));
        let action = domain
            .and_then(|d| d.objectForKey(ns_string!("AppleFnUsageType")))
            .and_then(|v| v.downcast::<NSNumber>().ok())
            .map(|n| n.intValue());
        Ok(
            json!({"supported":true, "trusted":macos_hook::trusted(false), "tap_active":macos_hook::active(), "globe_action":action}),
        )
    }
    #[cfg(not(target_os = "macos"))]
    Ok(json!({"supported":false, "trusted":false, "tap_active":false, "globe_action":null}))
}
#[tauri::command]
pub fn accessibility_open(window: WebviewWindow) -> Result<()> {
    main_only(&window)?;
    #[cfg(target_os = "macos")]
    {
        macos_hook::trusted(true);
        std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn()
            .map_err(|_| "Could not open Accessibility settings.")?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    Err("Accessibility setup is available on macOS.".into())
}
