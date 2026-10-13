use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub launch_at_login: bool,
    pub sounds: bool,
    pub duck_audio: bool,
}
impl Settings {
    /// External Login Items / Startup apps changes are authoritative.
    pub fn refresh(&mut self) {
        self.launch_at_login = startup_enabled();
        #[cfg(target_os = "macos")]
        {
            self.duck_audio = false;
        }
    }
    /// `carried_over` is true on the launch that moved a Metrix Writer
    /// installation to this identity (docs/release/identity.md).
    pub fn load(root: &std::path::Path, carried_over: bool) -> Result<Self> {
        let path = root.join("State/system.json");
        let mut settings = if path.exists() {
            serde_json::from_slice(
                &std::fs::read(path).map_err(|_| "Cannot read system settings.")?,
            )
            .map_err(|_| "Invalid system settings.")?
        } else {
            Self::default()
        };
        // macOS keeps Login Items per bundle identifier, so the Metrix Writer
        // registration does not carry over. The saved intent is what the user
        // last chose in this app; re-register once, on the carry-over launch.
        #[cfg(target_os = "macos")]
        if carried_over
            && settings.launch_at_login
            && super::default_root().ok().as_deref() == Some(root)
            && !startup_enabled()
        {
            let _ = startup(true, root);
        }
        #[cfg(windows)]
        let _ = carried_over;
        // Windows owns launch at login. A reinstall, an edit in Task Scheduler
        // or a leftover Run entry all change registration without touching this
        // file, so Windows is the truth and this file only remembers an intent.
        // Reading it back is what stops an unrelated save of sounds or ducking
        // from silently restoring an entry the user removed outside Writer.
        settings.refresh();
        // An installation that predates the logon task still holds the throttled
        // Run entry, and would keep starting minutes late until the user happened
        // to toggle a setting they had already turned on. Convert it here, once.
        // A Metrix Writer task or Run value is converted the same way, which also
        // points it at this installation's executable.
        //
        // Only the launch that owns the ordinary root may do this. `refresh()`
        // reads one per-user registration, so a portable copy or an isolated
        // `--user-root` launch also sees launch-at-login as on; converting from
        // there would repoint the installed app's autostart at that copy and its
        // private data. Registration stays an explicit act everywhere else.
        #[cfg(windows)]
        if settings.launch_at_login
            && super::default_root().ok().as_deref() == Some(root)
            && !task_registered(TASK_NAME)
        {
            let _ = startup(true, root);
            settings.refresh();
        }
        Ok(settings)
    }
}
#[tauri::command]
pub fn system_settings_get(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
) -> Result<Settings> {
    main_only(&window)?;
    // Re-read rather than report a cached intent: Windows' own Startup apps
    // list can have turned this off since launch, and a settings panel that
    // disagreed with it would put the entry back on the next unrelated save.
    let mut settings = state.system.lock().unwrap();
    settings.refresh();
    Ok(settings.clone())
}
#[tauri::command]
pub fn system_settings_save(
    window: WebviewWindow,
    app: tauri::AppHandle,
    settings: Settings,
    changed: Option<String>,
) -> Result<()> {
    main_only(&window)?;
    if changed
        .as_deref()
        .is_some_and(|field| !matches!(field, "launch_at_login" | "sounds" | "duck_audio"))
    {
        return Err("Unknown system setting.".into());
    }
    let startup_changed = changed.as_deref().map_or_else(
        || settings.launch_at_login != app.state::<Shell>().system.lock().unwrap().launch_at_login,
        |field| field == "launch_at_login",
    );
    save(&app, settings, startup_changed)
}
/// Applies one system setting from the tray, through the same save the
/// interface uses so launch-at-login and audio ducking stay consistent.
pub fn update(app: &tauri::AppHandle, change: impl FnOnce(&mut Settings)) -> Result<()> {
    let mut settings = app.state::<Shell>().system.lock().unwrap().clone();
    #[cfg(target_os = "macos")]
    {
        settings.refresh();
        *app.state::<Shell>().system.lock().unwrap() = settings.clone();
    }
    let startup_before = settings.launch_at_login;
    change(&mut settings);
    let startup_changed = startup_before != settings.launch_at_login;
    save(app, settings, startup_changed)
}
fn save(app: &tauri::AppHandle, settings: Settings, _startup_changed: bool) -> Result<()> {
    #[cfg(not(windows))]
    {
        let state = app.state::<Shell>();
        if settings.duck_audio {
            return Err("Audio ducking is unavailable on macOS: other apps do not expose restorable per-app session volumes.".into());
        }
        let mut old = state.system.lock().unwrap().clone();
        // Only an explicit login edit may change registration. A stale whole
        // settings document saved for sounds must not undo an external change.
        let startup_changed = _startup_changed;
        old.refresh();
        let mut next = settings;
        if startup_changed {
            startup(next.launch_at_login, &state.root)?;
        }
        next.refresh();
        if storage::atomic_json(&state.root.join("State/system.json"), &next).is_err() {
            if startup_changed {
                let _ = startup(old.launch_at_login, &state.root);
            }
            return Err("Could not save system settings. Previous settings retained.".into());
        }
        *state.system.lock().unwrap() = next;
        Ok(())
    }
    #[cfg(windows)]
    {
        let state = app.state::<Shell>();
        let old = state.system.lock().unwrap().clone();
        startup(settings.launch_at_login, &state.root)?;
        if storage::atomic_json(&state.root.join("State/system.json"), &settings).is_err() {
            let _ = startup(old.launch_at_login, &state.root);
            return Err("Could not save system settings. Previous settings retained.".into());
        }
        *state.system.lock().unwrap() = settings;
        let active = state.capture.status()?.active;
        recording(app, active, false);
        Ok(())
    }
}
/// The per-user autostart entry Windows reads at sign-in. Writer no longer writes
/// it: Windows staggers this key behind every other startup app, which on a
/// machine with a dozen of them delayed Writer by minutes after sign-in. It is
/// still read, and still deleted, so an installation that predates the scheduled
/// task neither reports the wrong state nor launches Writer twice.
#[cfg(windows)]
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "TrickTicDictate";
/// The logon task that replaced it. Task Scheduler honours a logon trigger
/// immediately, so Writer is usable seconds after sign-in instead of minutes.
/// The cost is that Task Manager's Startup tab does not list tasks: this
/// setting is owned by Writer's own settings and tray from here on.
#[cfg(windows)]
const TASK_NAME: &str = "TrickTicDictate";
/// The Run value and task name of the Metrix Writer identity (0.39 and earlier).
#[cfg(windows)]
const LEGACY_NAME: &str = "MetrixWriter";
/// Windows console programs flash a window unless explicitly detached.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn schtasks(arguments: &[&str]) -> Option<std::process::Output> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("schtasks.exe")
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
}
#[cfg(windows)]
fn task_registered(name: &str) -> bool {
    schtasks(&["/Query", "/TN", name]).is_some_and(|out| out.status.success())
}
/// Task Scheduler reads a definition file as UTF-16, whatever the declaration
/// says, so the document is encoded rather than written as bytes.
#[cfg(windows)]
fn write_definition(path: &std::path::Path, command: &str, arguments: &str) -> Result<()> {
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    // No delay, no battery or idle condition and no execution limit: this task
    // exists to start one foreground application the moment its user signs in.
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts {product} at sign-in.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
      <Delay>PT0S</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>{}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        escape(command),
        escape(arguments),
        product = product_name!(),
        user = escape(&user)
    );
    let bytes: Vec<_> = std::iter::once(0xfeffu16)
        .chain(document.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    std::fs::write(path, bytes).map_err(|_| "Windows could not update launch at login.".into())
}
/// Whether Windows currently holds an autostart entry for Writer, whoever wrote
/// it and in whichever form. Only presence is read; the stored command line is
/// never returned. The legacy Run value still counts, so an installation that
/// has not yet saved this setting again keeps reporting that it is on.
#[cfg(windows)]
fn startup_enabled() -> bool {
    [TASK_NAME, LEGACY_NAME]
        .into_iter()
        .any(|name| task_registered(name) || run_value_present(name))
}
#[cfg(windows)]
fn run_value_present(value: &str) -> bool {
    use windows::{core::PCWSTR, Win32::System::Registry::*};
    let key_name: Vec<_> = RUN_KEY.encode_utf16().chain(Some(0)).collect();
    let name: Vec<_> = value.encode_utf16().chain(Some(0)).collect();
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_ANY,
            None,
            None,
            Some(&mut size),
        )
        .ok()
        .is_ok()
    }
}
/// Deletes the legacy Run values. Absent is success: this runs on every change so
/// an installation that predates the scheduled task stops launching Writer twice.
#[cfg(windows)]
fn clear_run_value() {
    for value in [RUN_VALUE, LEGACY_NAME] {
        clear_run_value_named(value);
    }
}
#[cfg(windows)]
fn clear_run_value_named(value: &str) {
    use windows::{core::PCWSTR, Win32::System::Registry::*};
    let key_name: Vec<_> = RUN_KEY.encode_utf16().chain(Some(0)).collect();
    let name: Vec<_> = value.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
        .is_ok()
        {
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
            let _ = RegCloseKey(key);
        }
    }
}
#[cfg(windows)]
fn startup(enabled: bool, root: &std::path::Path) -> Result<()> {
    if !enabled {
        // Absent is the requested state, so a task that was never registered is
        // not a failure. The legacy value goes with it.
        let removed = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]);
        let _ = schtasks(&["/Delete", "/TN", LEGACY_NAME, "/F"]);
        clear_run_value();
        return match removed {
            Some(out)
                if (out.status.success() || !task_registered(TASK_NAME))
                    && !task_registered(LEGACY_NAME) =>
            {
                Ok(())
            }
            _ => Err("Windows could not update launch at login.".into()),
        };
    }
    let exe = std::env::current_exe().map_err(|_| "Application path unavailable.")?;
    // Only a launch with no path argument joins the single-instance group.
    // Naming the ordinary root explicitly would leave the copy started at
    // sign-in outside it, so a later click on the shortcut would fail on the
    // data lease instead of revealing the window that is already running. An
    // unusual root still has to be passed.
    let arguments = if super::default_root().ok().as_deref() == Some(root) {
        "--autostart".to_string()
    } else {
        format!("--autostart --user-root \"{}\"", root.display())
    };
    // The definition lives in the private state directory rather than a shared
    // temporary one, and does not outlive the call that registers it.
    let definition = root.join("State/launch-at-login.xml");
    std::fs::create_dir_all(root.join("State"))
        .map_err(|_| "Windows could not update launch at login.")?;
    write_definition(&definition, &exe.display().to_string(), &arguments)?;
    let registered = schtasks(&[
        "/Create",
        "/TN",
        TASK_NAME,
        "/XML",
        &definition.display().to_string(),
        "/F",
    ]);
    let _ = std::fs::remove_file(&definition);
    if !registered.is_some_and(|out| out.status.success()) {
        return Err("Windows could not update launch at login.".into());
    }
    // The Metrix Writer task points at an executable that is no longer installed.
    let _ = schtasks(&["/Delete", "/TN", LEGACY_NAME, "/F"]);
    clear_run_value();
    Ok(())
}
pub fn recording(app: &tauri::AppHandle, active: bool, play_sound: bool) {
    let state = app.state::<Shell>();
    let settings = state.system.lock().unwrap().clone();
    #[cfg(windows)]
    {
        state.ducking.set_active(active && settings.duck_audio);
    }
    if settings.sounds && play_sound {
        #[cfg(target_os = "macos")]
        let _ = app.run_on_main_thread(move || {
            use objc2_app_kit::NSSound;
            use objc2_foundation::NSString;
            if let Some(sound) =
                NSSound::soundNamed(&NSString::from_str(if active { "Tink" } else { "Pop" }))
            {
                sound.play();
            }
        });
        #[cfg(windows)]
        std::thread::spawn(move || unsafe {
            #[link(name = "kernel32")]
            extern "system" {
                fn Beep(frequency: u32, duration: u32) -> i32;
            }
            Beep(if active { 880 } else { 660 }, 45);
        });
    }
    super::tray::recording(app, active);
}

#[cfg(target_os = "macos")]
fn startup_enabled() -> bool {
    use objc2_service_management::{SMAppService, SMAppServiceStatus};
    unsafe { SMAppService::mainAppService().status() == SMAppServiceStatus::Enabled }
}

#[cfg(target_os = "macos")]
fn startup(enabled: bool, root: &std::path::Path) -> Result<()> {
    use objc2_service_management::{SMAppService, SMAppServiceStatus};
    // SMAppService registers the bundle's ordinary launch, never --user-root.
    if super::default_root()?.as_path() != root {
        return Err(concat!(
            "Launch at login is unavailable with a custom data folder. Open the installed ",
            product_name!(),
            " app normally."
        )
        .into());
    }
    let exe = std::env::current_exe().map_err(|_| "Application path unavailable.")?;
    if !exe.parent().is_some_and(|p| p.ends_with("Contents/MacOS")) {
        return Err(concat!(
            "Launch at login requires the installed ",
            product_name!(),
            " app bundle."
        )
        .into());
    }
    unsafe {
        let service = SMAppService::mainAppService();
        let status = service.status();
        if enabled {
            if status != SMAppServiceStatus::Enabled
                && status != SMAppServiceStatus::RequiresApproval
            {
                service.registerAndReturnError().map_err(|_| {
                    concat!(
                        "macOS could not register ",
                        product_name!(),
                        ". Keep the app in Applications and try again."
                    )
                })?;
            }
            if service.status() == SMAppServiceStatus::RequiresApproval {
                SMAppService::openSystemSettingsLoginItems();
                return Err(concat!(
                    "Approve ",
                    product_name!(),
                    " in System Settings → General → Login Items & Extensions, then return here."
                )
                .into());
            }
        } else if status != SMAppServiceStatus::NotRegistered {
            service.unregisterAndReturnError().map_err(|_| {
                concat!(
                    "macOS could not remove ",
                    product_name!(),
                    " from Login Items."
                )
            })?;
        }
    }
    if startup_enabled() != enabled {
        return Err("macOS did not apply launch at login. Check System Settings → General → Login Items & Extensions.".into());
    }
    Ok(())
}

/// Read during Tauri setup (applicationDidFinishLaunching), while macOS's
/// launch Apple event is current. Login launches follow the --hidden path.
#[cfg(target_os = "macos")]
pub fn launched_at_login() -> bool {
    use objc2_foundation::NSAppleEventManager;
    {
        NSAppleEventManager::sharedAppleEventManager()
            .currentAppleEvent()
            .is_some_and(|event| {
                event.eventID() == u32::from_be_bytes(*b"oapp")
                    && event
                        .paramDescriptorForKeyword(u32::from_be_bytes(*b"prdt"))
                        .is_some_and(|property| {
                            property.enumCodeValue() == u32::from_be_bytes(*b"lgit")
                        })
            })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mac_tests {
    use super::*;

    #[test]
    fn load_uses_login_items_and_never_advertises_ducking() {
        let root = std::env::temp_dir().join(format!("writer-system-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("State")).unwrap();
        storage::atomic_json(
            &root.join("State/system.json"),
            &Settings {
                launch_at_login: !startup_enabled(),
                sounds: true,
                duck_audio: true,
            },
        )
        .unwrap();
        let loaded = Settings::load(&root, false).unwrap();
        assert_eq!(loaded.launch_at_login, startup_enabled());
        assert!(loaded.sounds);
        assert!(!loaded.duck_audio);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_root_cannot_register_a_different_login_launch() {
        assert!(
            startup(true, std::path::Path::new("/tmp/writer-custom-root"))
                .unwrap_err()
                .contains("custom data folder")
        );
    }
}

/// Native --probe evidence only; never runs in an ordinary launch. A hidden
/// isolated shell shows its real overlay repeatedly while another app is active.
#[cfg(target_os = "macos")]
pub fn probe_overlay(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        let (tx, rx) = std::sync::mpsc::channel();
        let mut passes = Vec::new();
        for _ in 0..3 {
            let handle = app.clone();
            let send = tx.clone();
            let _ = app.run_on_main_thread(move || {
                let front = objc2_app_kit::NSWorkspace::sharedWorkspace()
                    .frontmostApplication()
                    .map(|app| app.processIdentifier());
                if let Some(window) = handle.get_webview_window("overlay") {
                    super::overlay_visibility(&window, true);
                }
                let _ = send.send(front);
            });
            let Ok(before) = rx.recv_timeout(Duration::from_secs(2)) else {
                return;
            };
            std::thread::sleep(Duration::from_millis(150));
            let handle = app.clone();
            let send = tx.clone();
            let _ = app.run_on_main_thread(move || {
                let front = objc2_app_kit::NSWorkspace::sharedWorkspace()
                    .frontmostApplication()
                    .map(|app| app.processIdentifier());
                let window = handle.get_webview_window("overlay").unwrap();
                let no_key = !window.is_focused().unwrap_or(true);
                let visible = window.is_visible().unwrap_or(false);
                let passed = before.is_some_and(|pid| pid != std::process::id() as i32)
                    && before == front
                    && no_key
                    && visible;
                super::overlay_visibility(&window, false);
                let _ = send.send(Some(if passed { 1 } else { 0 }));
            });
            let Ok(passed) = rx.recv_timeout(Duration::from_secs(2)) else {
                return;
            };
            passes.push(passed == Some(1));
            std::thread::sleep(Duration::from_millis(100));
        }
        let state = app.state::<Shell>();
        let _ = storage::atomic_json(
            &state.root.join("macos-overlay-probe.json"),
            &json!({ "visible_without_key_or_app_activation": passes }),
        );
    });
}
