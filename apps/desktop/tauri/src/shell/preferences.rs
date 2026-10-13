use super::*;
use serde::{Deserialize, Serialize};
use tauri_plugin_global_shortcut::Shortcut;

/// The panel's base logical size, at scale 1.0.
pub const BASE_WIDTH: f64 = 112.0;
pub const BASE_HEIGHT: f64 = 44.0;
pub const MIN_SCALE: f64 = 0.75;
pub const MAX_SCALE: f64 = 2.5;
/// What the retired `compact: false` size worked out to.
const ROOMY_SCALE: f64 = 1.2;
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub recording: String,
    pub cancel: String,
    pub toggle: bool,
    /// Which window a finished dictation's text is pasted into: "stop", the
    /// window in front when it stopped, or "start", the one it was started in.
    /// Documents written before the choice existed get "stop".
    pub paste_into: String,
    /// Opt-in clipboard preservation after an observed paste (Windows and macOS).
    pub preserve_clipboard: bool,
    /// Opt-in history/sync exclusion for automatic delivery, not explicit Copy.
    pub private_clipboard: bool,
    /// "off", "recording" while a dictation runs, or "always" on screen.
    pub visibility: String,
    pub placement: String,
    /// Physical screen position of a dragged panel, only under `remembered`.
    /// Kept absolute; a monitor change reinterprets it as the same relative
    /// place inside the new work area.
    pub position: Option<[i32; 2]>,
    /// Multiplies the panel's base 112x44 logical size.
    pub scale: f64,
    pub reduced_motion: bool,
    pub completion_hold_ms: u64,
    pub auto_dismiss: bool,
    /// The panel's own colours, or `None` to follow the app theme. Every
    /// document written before this field existed means `None`.
    pub panel_colors: Option<PanelColors>,
}
/// A floating panel painted in colours of its own rather than the app theme's.
///
/// Both values are written straight into CSS custom properties in the panel's
/// document, so only a literal `#rrggbb` is accepted, as `OverlayTheme` already
/// requires of the theme's colours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelColors {
    /// The pill's fill.
    pub background: String,
    /// The voice animation -- level bars and processing dots -- and the glyphs
    /// the panel draws in the same colour.
    pub wave: String,
}
/// A literal six-digit hex colour and nothing else.
fn opaque_hex(colour: &str) -> bool {
    colour.len() == 7
        && colour.starts_with('#')
        && colour[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            recording: if cfg!(target_os = "macos") {
                "Fn"
            } else {
                "Ctrl+Shift+F9"
            }
            .into(),
            cancel: "Ctrl+Shift+F10".into(),
            toggle: false,
            paste_into: "stop".into(),
            preserve_clipboard: false,
            private_clipboard: false,
            visibility: "recording".into(),
            placement: "bottom".into(),
            position: None,
            scale: 1.0,
            reduced_motion: false,
            completion_hold_ms: 1200,
            auto_dismiss: true,
            panel_colors: None,
        }
    }
}
/// Rewrites a document written before the panel gained a size and a third
/// visibility choice. `Preferences` denies unknown fields, so the retired keys
/// have to be translated here rather than tolerated by the deserializer. A
/// document that already carries the new keys is left alone.
fn migrate(value: &mut Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    if let Some(compact) = map.remove("compact") {
        let scale = if compact.as_bool().unwrap_or(true) {
            1.0
        } else {
            ROOMY_SCALE
        };
        map.entry("scale").or_insert_with(|| json!(scale));
    }
    if let Some(enabled) = map.remove("overlay_enabled") {
        let visibility = if enabled.as_bool().unwrap_or(true) {
            "recording"
        } else {
            "off"
        };
        map.entry("visibility").or_insert_with(|| json!(visibility));
    }
}

impl Preferences {
    pub fn load(root: &std::path::Path) -> Result<Self> {
        let path = root.join("State/tauri-preferences.json");
        if path.exists() {
            let mut stored: Value = serde_json::from_slice(
                &std::fs::read(path).map_err(|_| "Cannot read desktop preferences.")?,
            )
            .map_err(|_| "Invalid desktop preferences.")?;
            migrate(&mut stored);
            let p: Self =
                serde_json::from_value(stored).map_err(|_| "Invalid desktop preferences.")?;
            p.validate()?;
            return Ok(p);
        }
        let mut p = Self::default();
        if let Ok(bytes) = std::fs::read(root.join("State/shortcuts.json")) {
            if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(s) = v["Recording"].as_str() {
                    p.recording = s.into();
                }
                if let Some(s) = v["Cancel"].as_str() {
                    p.cancel = s.into();
                }
                p.toggle = v["Mode"] == "Toggle";
            }
        }
        if let Ok(bytes) = std::fs::read(root.join("State/overlay.json")) {
            if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                p.visibility = if v["Enabled"].as_bool().unwrap_or(true) {
                    "recording"
                } else {
                    "off"
                }
                .into();
                p.scale = if v["Size"] == "Compact" {
                    1.0
                } else {
                    ROOMY_SCALE
                };
                p.reduced_motion = v["Motion"] == "Reduced";
                p.placement = if v["Placement"] == "TopCenter" {
                    "top"
                } else {
                    "bottom"
                }
                .into();
                p.completion_hold_ms = v["CompletionHoldMs"].as_u64().unwrap_or(1800);
                p.auto_dismiss = v["AutoDismissCompleted"].as_bool().unwrap_or(true);
            }
        }
        p.validate()?;
        Ok(p)
    }
    fn validate(&self) -> Result<()> {
        if !matches!(self.paste_into.as_str(), "stop" | "start") {
            return Err("Invalid desktop preferences.".into());
        }
        #[cfg(target_os = "macos")]
        if tap_only(&self.recording)
            && keys(&self.cancel)?
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                == keys("Ctrl+Shift+F9")?.into_iter().collect()
        {
            return Err(
                "Ctrl+Shift+F9 is reserved as the recording fallback while fn needs Accessibility."
                    .into(),
            );
        }
        let a = keys(&self.recording)?;
        let b = keys(&self.cancel)?;
        if {
            let mut a = a;
            let mut b = b;
            a.sort();
            b.sort();
            a == b
        } || !["top", "bottom", "remembered"].contains(&self.placement.as_str())
            || !["off", "recording", "always"].contains(&self.visibility.as_str())
            || !self.scale.is_finite()
            || !(MIN_SCALE..=MAX_SCALE).contains(&self.scale)
            || !(300..=60000).contains(&self.completion_hold_ms)
            || self
                .panel_colors
                .as_ref()
                .is_some_and(|c| !opaque_hex(&c.background) || !opaque_hex(&c.wave))
        {
            return Err("Invalid desktop preferences.".into());
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub fn keys(chord: &str) -> Result<Vec<i32>> {
    super::macos_keys::keys(chord)
}
#[cfg(not(target_os = "macos"))]
pub fn keys(chord: &str) -> Result<Vec<i32>> {
    if chord.len() > 80 {
        return Err("Invalid shortcut.".into());
    }
    let upper = chord.to_uppercase();
    let parts: Vec<_> = upper.split('+').map(str::trim).collect();
    if parts.len() < 2 {
        return Err("Use at least one modifier key.".into());
    }
    let mut keys = Vec::new();
    let modifier = |p: &str| match p {
        "CTRL" | "CONTROL" => Some(0x11),
        "ALT" => Some(0x12),
        "SHIFT" => Some(0x10),
        "WIN" | "SUPER" | "META" => Some(0x5b),
        _ => None,
    };
    let only_modifiers = parts.iter().all(|p| modifier(p).is_some());
    for p in &parts[..parts.len() - usize::from(!only_modifiers)] {
        let key = modifier(p).ok_or("Use Ctrl, Alt, Shift or Win modifiers.")?;
        if keys.contains(&key) {
            return Err("Duplicate modifier.".into());
        }
        keys.push(key);
    }
    if only_modifiers {
        if !keys.contains(&0x5b) {
            return Err("Modifier-only shortcuts must include Win and another modifier.".into());
        }
        #[cfg(not(windows))]
        return Err("Modifier-only shortcuts currently require Windows.".into());
        #[cfg(windows)]
        return Ok(keys);
    }
    let p = parts.last().unwrap();
    let key = match *p {
        "SPACE" => 0x20,
        "ESCAPE" => 0x1b,
        "BACKSPACE" => 8,
        _ if p.len() == 1 && p.as_bytes()[0].is_ascii_alphanumeric() => i32::from(p.as_bytes()[0]),
        _ => p
            .strip_prefix('F')
            .and_then(|n| n.parse::<i32>().ok())
            .filter(|n| (1..=24).contains(n) && *n != 12)
            .map(|n| 0x6f + n)
            .ok_or("Use a letter, digit, function key (except F12), Space, Escape or Backspace.")?,
    };
    keys.push(key);
    Ok(keys)
}
#[cfg(target_os = "macos")]
pub fn modifier_only(chord: &str) -> bool {
    chord
        .split('+')
        .all(|p| super::macos_keys::modifier(&p.trim().to_uppercase()).is_some())
}
#[cfg(not(target_os = "macos"))]
pub fn modifier_only(chord: &str) -> bool {
    chord.split('+').all(|p| {
        matches!(
            p.trim().to_uppercase().as_str(),
            "CTRL" | "CONTROL" | "ALT" | "SHIFT" | "WIN" | "SUPER" | "META"
        )
    })
}
pub fn tap_only(chord: &str) -> bool {
    #[cfg(target_os = "macos")]
    if keys(chord).is_ok_and(|keys| keys.contains(&super::macos_keys::FN)) {
        return true;
    }
    modifier_only(chord)
}
pub fn recording_registration(p: &Preferences) -> Result<Option<Shortcut>> {
    #[cfg(target_os = "macos")]
    if tap_only(&p.recording) {
        return shortcut("Ctrl+Shift+F9").map(Some);
    }
    registered(&p.recording)
}
pub fn registered(chord: &str) -> Result<Option<Shortcut>> {
    if tap_only(chord) {
        keys(chord)?;
        Ok(None)
    } else {
        shortcut(chord).map(Some)
    }
}
pub fn shortcut(chord: &str) -> Result<Shortcut> {
    keys(chord)?;
    #[cfg(target_os = "macos")]
    let chord = chord
        .split('+')
        .map(
            |p| match super::macos_keys::modifier(&p.trim().to_uppercase()) {
                Some(0x5b) => "Super".to_string(),
                Some(0x12) => "Alt".to_string(),
                Some(0x11) => "Ctrl".to_string(),
                Some(0x10) => "Shift".to_string(),
                _ => p.trim().to_string(),
            },
        )
        .collect::<Vec<_>>()
        .join("+");
    chord
        .replace("Win", "Super")
        .parse()
        .map_err(|_| "Invalid shortcut.".into())
}
#[tauri::command]
pub fn preferences_get(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
) -> Result<Preferences> {
    settings_only(&window)?;
    Ok(state.preferences.lock().unwrap().clone())
}
/// Settings are rendered in the main React document. The retired standalone
/// settings window must not regain its old privileges.
fn settings_only(window: &WebviewWindow) -> Result<()> {
    main_only(window)
}

/// Resolved colours for the floating panel, sent by the web layer whenever the
/// theme changes. The shell stores and forwards them; it never has to know what
/// a theme is.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayTheme {
    pub kind: String,
    pub bg: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub accent: String,
    pub accent_text: String,
    pub surface: String,
    pub danger: String,
    /// The page's own background, for the window behind it. Not used by the
    /// panel; optional so a page that predates it is still accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
}
impl OverlayTheme {
    /// Every value is written into a CSS custom property in the overlay
    /// document, so only literal hex colours are accepted.
    fn validate(&self) -> Result<()> {
        if !matches!(self.kind.as_str(), "dark" | "light") {
            return Err("Invalid theme.".into());
        }
        for colour in [
            &self.bg,
            &self.border,
            &self.text,
            &self.text_muted,
            &self.accent,
            &self.accent_text,
            &self.surface,
            &self.danger,
        ]
        .into_iter()
        .chain(&self.window)
        {
            let digits = colour.strip_prefix('#').unwrap_or("");
            if !matches!(digits.len(), 3 | 4 | 6 | 8)
                || !digits.chars().all(|c| c.is_ascii_hexdigit())
            {
                return Err("Invalid theme colour.".into());
            }
        }
        Ok(())
    }
}

#[tauri::command]
pub fn preferences_theme(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Shell>,
    theme: OverlayTheme,
) -> Result<()> {
    settings_only(&window)?;
    theme.validate()?;
    if let Some(window) = &theme.window {
        super::window_place::background(&app, window);
    }
    let _ = app.emit_to("overlay", "writer-theme", theme.clone());
    // Retained so a reloaded panel is repainted from its `ready` handshake
    // rather than flashing the previous theme.
    *state.theme.lock().unwrap() = Some(theme);
    Ok(())
}
#[tauri::command]
pub fn preferences_save(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Shell>,
    mut preferences: Preferences,
) -> Result<()> {
    settings_only(&window)?;
    preferences.validate()?;
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "Settings maintenance is busy.")?;
    mode_shortcuts::validate_main(&state.mode_shortcuts.lock().unwrap(), &preferences)?;
    let old = state.preferences.lock().unwrap().clone();
    // Only shortcut edits have to wait: they acquire and release an operating
    // system registration. Panel size, visibility and placement cost nothing and
    // are worth changing while watching the panel you are changing.
    let rebinding = preferences.recording != old.recording || preferences.cancel != old.cancel;
    if rebinding && (state.capture.status()?.active || state.pending.lock().unwrap().is_some()) {
        return Err("Finish recording before changing shortcuts.".into());
    }
    if preferences.position.is_none() {
        preferences.position = old.position.or_else(|| {
            app.get_webview_window("overlay")
                .and_then(|w| w.outer_position().ok())
                .map(|p| [p.x, p.y])
        });
    }
    let old_keys: Vec<_> = [recording_registration(&old)?, registered(&old.cancel)?]
        .into_iter()
        .flatten()
        .collect();
    let new_keys: Vec<_> = [
        recording_registration(&preferences)?,
        registered(&preferences.cancel)?,
    ]
    .into_iter()
    .flatten()
    .collect();
    if new_keys.iter().any(|key| {
        state
            .mode_shortcuts
            .lock()
            .unwrap()
            .iter()
            .any(|(mode, _)| mode == key)
    }) {
        return Err("This shortcut belongs to a dictation mode. Change it in Modes first.".into());
    }
    let mut acquired = Vec::new();
    for key in new_keys.iter().copied() {
        if !old_keys.contains(&key) {
            if app.global_shortcut().register(key).is_err() {
                for k in acquired {
                    let _ = app.global_shortcut().unregister(k);
                }
                return Err("Shortcut is already in use. Previous settings retained.".into());
            }
            acquired.push(key);
        }
    }
    if storage::atomic_json(
        &state.root.join("State/tauri-preferences.json"),
        &preferences,
    )
    .is_err()
    {
        for k in acquired {
            let _ = app.global_shortcut().unregister(k);
        }
        return Err("Could not save preferences. Previous settings retained.".into());
    }
    for key in old_keys {
        if !new_keys.contains(&key) {
            let _ = app.global_shortcut().unregister(key);
        }
    }
    #[cfg(windows)]
    if rebinding {
        modifier_hook::configure(&preferences);
    }
    #[cfg(target_os = "macos")]
    macos_hook::configure(&preferences);
    *state.preferences.lock().unwrap() = preferences.clone();
    position(&app, &preferences);
    let _ = app.emit_to("overlay", "writer-preferences", preferences);
    Ok(())
}
/// Applies one preference change that does not touch shortcut registration.
/// Shortcut edits keep going through `preferences_save`, which also has to
/// acquire and release the operating-system registration and therefore has to
/// refuse mid-recording; a panel or placement change has no such cost.
pub fn update(app: &tauri::AppHandle, change: impl FnOnce(&mut Preferences)) -> Result<()> {
    let state = app.state::<Shell>();
    let current = state.preferences.lock().unwrap().clone();
    let mut next = current.clone();
    change(&mut next);
    next.validate()?;
    if next.recording != current.recording || next.cancel != current.cancel {
        return Err("Change recording shortcuts in Settings.".into());
    }
    storage::atomic_json(&state.root.join("State/tauri-preferences.json"), &next)
        .map_err(|_| "Could not save preferences. Previous settings retained.")?;
    *state.preferences.lock().unwrap() = next.clone();
    position(app, &next);
    let _ = app.emit_to("overlay", "writer-preferences", next);
    Ok(())
}
/// The monitor the pointer is on, which is where the panel belongs.
///
/// Wispr-style: the dictation panel appears on the screen the user is working
/// on rather than the one it happened to be left on. Falls back to the panel's
/// own monitor when the pointer cannot be read.
fn target_monitor(app: &tauri::AppHandle, overlay: &WebviewWindow) -> Option<tauri::Monitor> {
    let pointer = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten());
    pointer.or_else(|| overlay.current_monitor().ok().flatten())
}

/// The size the panel occupies on a monitor at `scale`.
///
/// Computed rather than measured: until the panel arrives, it is still the size
/// the monitor it is leaving gave it, so measuring it would centre it for the
/// wrong scale on every move between screens that do not share one.
fn panel_size(p: &Preferences, scale: f64) -> tauri::PhysicalSize<u32> {
    tauri::PhysicalSize::new(
        (BASE_WIDTH * p.scale * scale).round() as u32,
        (BASE_HEIGHT * p.scale * scale).round() as u32,
    )
}

/// Where a dragged panel sat inside its own monitor, as a fraction of the room
/// it had. Applying the fraction to another monitor keeps a bottom-right panel
/// bottom-right instead of clamping it to the edge of a differently sized screen.
fn remembered_fraction(app: &tauri::AppHandle, p: &Preferences) -> Option<(f64, f64)> {
    let [x, y] = p.position?;
    // `work_area` borrows from the monitor, so the monitor has to outlive it.
    let monitor = app
        .monitor_from_point(f64::from(x), f64::from(y))
        .ok()
        .flatten()?;
    let size = panel_size(p, monitor.scale_factor());
    let area = monitor.work_area();
    let room = |extent: u32, window: u32| f64::from((extent as i32 - window as i32).max(1));
    Some((
        (f64::from(x - area.position.x) / room(area.size.width, size.width)).clamp(0., 1.),
        (f64::from(y - area.position.y) / room(area.size.height, size.height)).clamp(0., 1.),
    ))
}

pub fn position(app: &tauri::AppHandle, p: &Preferences) {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    if p.visibility == "off" {
        super::overlay_visibility(&overlay, false);
    }
    let _ = overlay.set_size(tauri::LogicalSize::new(
        BASE_WIDTH * p.scale,
        BASE_HEIGHT * p.scale,
    ));
    let Some(monitor) = target_monitor(app, &overlay) else {
        return;
    };
    let area = monitor.work_area();
    let size = panel_size(p, monitor.scale_factor());
    let room = |extent: u32, window: u32| (extent as i32 - window as i32).max(0);
    let (across, down) = (
        room(area.size.width, size.width),
        room(area.size.height, size.height),
    );
    let (x, y) = match p.placement.as_str() {
        "remembered" => match remembered_fraction(app, p) {
            Some((fx, fy)) => (
                area.position.x + (fx * f64::from(across)).round() as i32,
                area.position.y + (fy * f64::from(down)).round() as i32,
            ),
            // Nothing has been dragged yet, so the default placement applies.
            None => (area.position.x + across / 2, area.position.y + down - 24),
        },
        "top" => (area.position.x + across / 2, area.position.y + 24),
        _ => (area.position.x + across / 2, area.position.y + down - 24),
    };
    let _ = overlay.set_position(tauri::PhysicalPosition::new(
        x.clamp(area.position.x, area.position.x + across),
        y.clamp(area.position.y, area.position.y + down),
    ));
}

/// True when the panel is no longer on the monitor the pointer is using.
pub fn pointer_moved_monitor(app: &tauri::AppHandle) -> bool {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return false;
    };
    let Ok(cursor) = app.cursor_position() else {
        return false;
    };
    let Some(pointer) = app.monitor_from_point(cursor.x, cursor.y).ok().flatten() else {
        return false;
    };
    let Some(current) = overlay.current_monitor().ok().flatten() else {
        return true;
    };
    if current.position() == pointer.position() {
        return false;
    }
    // A panel under the pointer is being dragged across the boundary, or reached
    // for; either way it belongs where the pointer put it rather than back at the
    // placement the new screen would give it.
    !holding(&overlay, cursor)
}

/// Whether the pointer is on the panel itself.
fn holding(overlay: &WebviewWindow, cursor: tauri::PhysicalPosition<f64>) -> bool {
    let (Ok(origin), Ok(size)) = (overlay.outer_position(), overlay.outer_size()) else {
        return false;
    };
    let within = |point: f64, start: i32, extent: u32| {
        point >= f64::from(start) && point < f64::from(start) + f64::from(extent)
    };
    within(cursor.x, origin.x, size.width) && within(cursor.y, origin.y, size.height)
}

#[tauri::command]
pub async fn import_model(
    window: WebviewWindow,
    app: tauri::AppHandle,
    model: String,
) -> Result<()> {
    settings_only(&window)?;
    if app.state::<Shell>().capture.status()?.active {
        return Err("Finish recording before importing model files.".into());
    }
    let setup =
        metrix_writer_api::desktop_settings::global().map_err(|_| "Model setup unavailable.")?;
    let runtime = if setup.runtime.spec(&model).is_some() {
        setup.runtime.clone()
    } else {
        metrix_writer_api::cleanup::local_text::store()
            .map_err(|e| e.message)?
            .assets
            .clone()
    };
    runtime.spec(&model).ok_or("Unknown model.")?;
    use tauri_plugin_dialog::DialogExt;
    let selected = app
        .dialog()
        .file()
        .add_filter("Model weights", &["bin", "gguf"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(());
    };
    let source = selected
        .into_path()
        .map_err(|_| "Select a local weights file.")?;
    if app.state::<Shell>().capture.status()?.active {
        return Err("Finish recording before importing model files.".into());
    }
    runtime
        .import_file(&model, source)
        .await
        .map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_preservation_is_opt_in_and_round_trips() {
        let root = scratch();
        std::fs::write(
            root.join("State/tauri-preferences.json"),
            br#"{"paste_into":"start"}"#,
        )
        .unwrap();
        let mut p = Preferences::load(&root).unwrap();
        assert!(!p.preserve_clipboard);
        assert!(!p.private_clipboard);
        p.preserve_clipboard = true;
        p.private_clipboard = true;
        std::fs::write(
            root.join("State/tauri-preferences.json"),
            serde_json::to_vec(&p).unwrap(),
        )
        .unwrap();
        let loaded = Preferences::load(&root).unwrap();
        assert!(loaded.preserve_clipboard);
        assert!(loaded.private_clipboard);
        assert_eq!(loaded.paste_into, "start");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unicode_layout_independent_shortcuts_reject_ambiguous_or_reserved_keys() {
        #[cfg(not(target_os = "macos"))]
        assert!(keys("Ctrl+F12").is_err());
        #[cfg(target_os = "macos")]
        assert!(keys("Ctrl+F12").is_ok());
        assert_eq!(keys("Ctrl+Shift+F9").unwrap(), vec![0x11, 0x10, 0x78]);
        assert_eq!(keys("Alt+R").unwrap(), vec![0x12, 0x52]);
        for invalid in ["F9", "Ctrl+Ctrl+R", "Ctrl+א", "Ctrl+"] {
            assert!(keys(invalid).is_err(), "{invalid}");
        }
        let p = Preferences {
            cancel: "Alt+F9".into(),
            ..Default::default()
        };
        assert!(p.validate().is_ok());
    }
    #[test]
    fn text_goes_where_the_dictation_stops_unless_the_start_window_is_chosen() {
        assert_eq!(Preferences::default().paste_into, "stop");
        let root = scratch();
        // A document written before the choice existed takes the new default.
        std::fs::write(
            root.join("State/tauri-preferences.json"),
            br#"{"recording":"Ctrl+Shift+D","toggle":false,"scale":0.8}"#,
        )
        .unwrap();
        let p = Preferences::load(&root).unwrap();
        assert_eq!(p.paste_into, "stop");
        assert_eq!(p.recording, "Ctrl+Shift+D");
        for value in ["stop", "start"] {
            assert!(Preferences {
                paste_into: value.into(),
                ..Default::default()
            }
            .validate()
            .is_ok());
        }
        for value in ["", "release", "focused", "STOP"] {
            assert!(
                Preferences {
                    paste_into: value.into(),
                    ..Default::default()
                }
                .validate()
                .is_err(),
                "{value}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn windows_modifier_only_chords_use_hook_and_full_chords_use_registration() {
        assert_eq!(keys("Win+Alt").unwrap(), vec![0x5b, 0x12]);
        assert_eq!(keys("Alt+Win").unwrap(), vec![0x12, 0x5b]);
        assert!(registered("Win+Alt").unwrap().is_none());
        assert!(registered("Win+Ctrl+R").unwrap().is_some());
        for invalid in ["Win", "Win+Win", "Ctrl+Alt", "Win+F12"] {
            assert!(keys(invalid).is_err(), "{invalid}");
        }
        assert!(Preferences {
            recording: "Win+Alt".into(),
            cancel: "Alt+Win".into(),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn fresh_mac_defaults_and_fn_round_trip_preserve_legacy_chords() {
        let root = scratch();
        let mut p = Preferences::load(&root).unwrap();
        assert_eq!(p.recording, "Fn");
        assert_eq!(p.cancel, "Ctrl+Shift+F10");
        assert!(registered("Fn").unwrap().is_none());
        assert_eq!(
            recording_registration(&p).unwrap(),
            registered("Ctrl+Shift+F9").unwrap()
        );
        for chord in ["Fn", "fn+⌥", "⌘+⌥", "Ctrl+Shift+D", "Win+Alt+R"] {
            p.recording = chord.into();
            p.validate().unwrap();
            storage::atomic_json(&root.join("State/tauri-preferences.json"), &p).unwrap();
            assert_eq!(Preferences::load(&root).unwrap().recording, chord);
        }
        p.recording = "Fn".into();
        p.cancel = "Ctrl+Shift+F9".into();
        assert!(p.validate().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn scratch() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("State")).unwrap();
        root
    }

    #[test]
    fn a_stored_panel_size_and_visibility_migrate_from_the_retired_fields() {
        let root = scratch();
        // Exactly the shape written by the shipped 0.4.1 app.
        std::fs::write(root.join("State/tauri-preferences.json"), br#"{"recording":"Ctrl+Shift+D","cancel":"Ctrl+Shift+F10","toggle":false,"overlay_enabled":true,"placement":"bottom","position":[1033,1196],"compact":false,"reduced_motion":false,"completion_hold_ms":1800,"auto_dismiss":true}"#).unwrap();
        let p = Preferences::load(&root).unwrap();
        assert_eq!(p.visibility, "recording");
        assert_eq!(p.scale, ROOMY_SCALE);
        // Unrelated settings, including a dragged position, are preserved.
        assert_eq!(p.position, Some([1033, 1196]));
        assert_eq!(p.recording, "Ctrl+Shift+D");
        assert_eq!(p.completion_hold_ms, 1800);

        std::fs::write(
            root.join("State/tauri-preferences.json"),
            br#"{"overlay_enabled":false,"compact":true}"#,
        )
        .unwrap();
        let p = Preferences::load(&root).unwrap();
        assert_eq!(p.visibility, "off");
        assert_eq!(p.scale, 1.0);

        // A document already in the new shape is left exactly as written.
        std::fs::write(
            root.join("State/tauri-preferences.json"),
            br#"{"visibility":"always","scale":2.0}"#,
        )
        .unwrap();
        let p = Preferences::load(&root).unwrap();
        assert_eq!(p.visibility, "always");
        assert_eq!(p.scale, 2.0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_out_of_range_panel_size_or_unknown_visibility_is_refused() {
        for (scale, visibility) in [
            (0.5, "recording"),
            (3.0, "recording"),
            (f64::NAN, "recording"),
            (1.0, "sometimes"),
            (1.0, ""),
        ] {
            let p = Preferences {
                scale,
                visibility: visibility.into(),
                ..Default::default()
            };
            assert!(p.validate().is_err(), "{scale} {visibility}");
        }
        for scale in [MIN_SCALE, 1.0, MAX_SCALE] {
            for visibility in ["off", "recording", "always"] {
                assert!(Preferences {
                    scale,
                    visibility: visibility.into(),
                    ..Default::default()
                }
                .validate()
                .is_ok());
            }
        }
        // A stored document outside the range is rejected rather than clamped
        // silently, so a hand-edited file cannot produce an unusable panel.
        let root = scratch();
        std::fs::write(
            root.join("State/tauri-preferences.json"),
            br#"{"scale":9.0}"#,
        )
        .unwrap();
        assert!(Preferences::load(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_document_written_before_panel_colours_follows_the_theme() {
        let root = scratch();
        // Exactly the shape the shipped 0.9.0 app writes.
        std::fs::write(root.join("State/tauri-preferences.json"), br#"{"recording":"Ctrl+Shift+D","cancel":"Ctrl+Shift+F10","toggle":false,"visibility":"recording","placement":"bottom","position":[1033,1196],"scale":0.8,"reduced_motion":false,"completion_hold_ms":1800,"auto_dismiss":true}"#).unwrap();
        let p = Preferences::load(&root).unwrap();
        assert_eq!(p.panel_colors, None);
        // Nothing else in the document moves.
        assert_eq!(p.scale, 0.8);
        assert_eq!(p.position, Some([1033, 1196]));
        assert_eq!(p.recording, "Ctrl+Shift+D");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn panel_colours_round_trip_and_anything_but_a_literal_hex_is_refused() {
        let colours = |background: &str, wave: &str| Preferences {
            panel_colors: Some(PanelColors {
                background: background.into(),
                wave: wave.into(),
            }),
            ..Default::default()
        };
        let root = scratch();
        let saved = colours("#282a36", "#BD93F9");
        assert!(saved.validate().is_ok());
        std::fs::write(
            root.join("State/tauri-preferences.json"),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        assert_eq!(
            Preferences::load(&root).unwrap().panel_colors,
            saved.panel_colors
        );
        // These are written into the panel's CSS, so a named colour, a short
        // or alpha form, or anything that could close the declaration is out.
        for invalid in [
            "red",
            "#fff",
            "#12345g",
            "url(x)",
            "#1234567",
            "#12345678",
            "",
            "282a36",
            "#28 a36",
            "#28a36;",
        ] {
            assert!(colours(invalid, "#ffffff").validate().is_err(), "{invalid}");
            assert!(colours("#000000", invalid).validate().is_err(), "{invalid}");
        }
        // A hand-edited file is refused rather than painted, and a key the
        // colour object does not have is as unknown as one anywhere else.
        for stored in [
            &br##"{"panel_colors":{"background":"red","wave":"#ffffff"}}"##[..],
            br##"{"panel_colors":{"background":"#000000","wave":"#ffffff","border":"#ffffff"}}"##,
            br##"{"panel_colors":{"background":"#000000"}}"##,
        ] {
            std::fs::write(root.join("State/tauri-preferences.json"), stored).unwrap();
            assert!(Preferences::load(&root).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_preferences_are_read_without_overwriting_the_old_shell_files() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("State")).unwrap();
        let source = br#"{"Recording":"Ctrl+Alt+R","Cancel":"Ctrl+Shift+F10","Mode":"Toggle"}"#;
        std::fs::write(root.join("State/shortcuts.json"), source).unwrap();
        let p = Preferences::load(&root).unwrap();
        assert!(p.toggle);
        assert_eq!(p.recording, "Ctrl+Alt+R");
        assert_eq!(
            std::fs::read(root.join("State/shortcuts.json")).unwrap(),
            source
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_panel_is_sized_for_the_screen_it_is_moving_to() {
        let p = Preferences {
            scale: 1.2,
            ..Default::default()
        };
        // A 125% screen beside a 100% one. The window ends up at these sizes
        // once it arrives, so centring it on anything else leaves it off by the
        // difference -- 17px across a 1920-wide work area.
        assert_eq!(panel_size(&p, 1.25), tauri::PhysicalSize::new(168, 66));
        assert_eq!(panel_size(&p, 1.0), tauri::PhysicalSize::new(134, 53));
        let centred = |work: i32, panel: u32| (work - panel as i32) / 2;
        assert_eq!(centred(1920, panel_size(&p, 1.0).width), 893);
        assert_eq!(centred(1920, panel_size(&p, 1.25).width), 876);
    }
}
