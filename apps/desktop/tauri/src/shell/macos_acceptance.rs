//! Explicit native acceptance harness. Only compiled with latency-probe and
//! entered through --probe with a fixture marker in an isolated data root.
use super::{context::ContextReader, macos_delivery, Shell};
use crate::storage::{self, Result};
use metrix_writer_api::cleanup::modes::{ContextPolicy, ModeSettings};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
use tauri::Manager;

pub(super) fn context(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let root = &app.state::<Shell>().root;
        let result = check_context(root);
        let passed = result
            .as_ref()
            .is_ok_and(|checks| checks.values().all(|v| *v));
        let receipt = match result {
            Ok(checks) => serde_json::json!({"checks": checks, "passed": passed}),
            Err(message) => serde_json::json!({"passed": false, "error": message}),
        };
        let _ = storage::atomic_json(&root.join("macos-context-probe.json"), &receipt);
        app.exit(if passed { 0 } else { 1 });
    });
}
fn action(root: &Path, id: u32, action: &str) -> Result<()> {
    storage::atomic_json(
        &root.join("command.json"),
        &serde_json::json!({"id":id,"action":action}),
    )?;
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        let ack: Option<serde_json::Value> = std::fs::read(root.join("ack.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if ack.is_some_and(|a| a["id"] == id) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("Native context fixture did not acknowledge its command.".into())
}
fn check_context(root: &Path) -> Result<BTreeMap<String, bool>> {
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("macos-context-fixture.json"))
            .map_err(|_| "Fixture marker unavailable.")?,
    )
    .map_err(|_| "Invalid fixture marker.")?;
    let fixture = Path::new(
        config["fixture_root"]
            .as_str()
            .ok_or("Fixture root unavailable.")?,
    );
    if !fixture.is_absolute() {
        return Err("Fixture root must be absolute.".into());
    }
    let target = macos_delivery::foreground().ok_or("No foreground fixture.")?;
    if !macos_delivery::application_name(target)
        .is_some_and(|n| n.ends_with("(com.metrix.writer.mw50-context-fixture)"))
    {
        return Err("Start the synthetic MW50 Context Fixture before this probe.".into());
    }
    let target = Some(("fixture".into(), target.window, target.process));
    let reader = ContextReader::new();
    let defaults = ModeSettings::default();
    let mut checks = BTreeMap::new();
    let empty = reader.capture("no-context".into(), defaults.profiles[0].clone(), None)?;
    checks.insert(
        "no_context_without_target".into(),
        empty.application.is_empty()
            && empty.selected_text.is_empty()
            && empty.clipboard.is_empty(),
    );
    let mut profile = defaults.profiles[1].clone();
    profile.context = ContextPolicy {
        application: true,
        selected_text: true,
        clipboard: true,
    };
    let captured = reader.capture("all-context".into(), profile, target.clone())?;
    checks.insert(
        "application_name_and_bundle".into(),
        captured.application.contains("MW50 Context Fixture")
            && captured
                .application
                .ends_with("(com.metrix.writer.mw50-context-fixture)"),
    );
    checks.insert(
        "selected_hebrew_and_english".into(),
        captured.selected_text == "שלום Writer",
    );
    checks.insert(
        "opt_in_clipboard".into(),
        captured.clipboard == "MW-50 clipboard fixture",
    );
    let command = defaults.profiles[2].clone();
    let selected = reader.capture("command".into(), command.clone(), target.clone())?;
    checks.insert(
        "unrequested_context_empty".into(),
        selected.application.is_empty() && selected.clipboard.is_empty(),
    );
    checks.insert("unchanged_selection_valid".into(), reader.valid("command"));
    checks.insert(
        "different_session_invalid".into(),
        !reader.valid("other-session"),
    );
    action(fixture, 1, "other-range")?;
    checks.insert(
        "same_text_other_range_invalid".into(),
        !reader.valid("command"),
    );
    action(fixture, 2, "replace")?;
    checks.insert("changed_text_invalid".into(), !reader.valid("command"));
    action(fixture, 3, "password")?;
    checks.insert(
        "secure_selection_refused".into(),
        reader.capture("secure".into(), command, target).is_err(),
    );
    let empty = reader.capture(
        "secure-no-context".into(),
        defaults.profiles[0].clone(),
        None,
    )?;
    checks.insert(
        "secure_no_context_works".into(),
        empty.application.is_empty()
            && empty.selected_text.is_empty()
            && empty.clipboard.is_empty(),
    );
    action(fixture, 4, "normal")?;
    Ok(checks)
}

/// Sends only fixed synthetic phrases to the explicitly prepared Terminal cat
/// fixture, then checks stale/discarded native delivery without posting keys.
pub(super) fn delivery(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let root = &app.state::<Shell>().root;
        let result = check_delivery();
        let passed = result
            .as_ref()
            .is_ok_and(|checks| checks.values().all(|v| *v));
        let receipt = match result {
            Ok(checks) => serde_json::json!({"checks": checks, "passed": passed}),
            Err(message) => serde_json::json!({"passed": false, "error": message}),
        };
        let _ = storage::atomic_json(&root.join("macos-delivery-probe.json"), &receipt);
        app.exit(if passed { 0 } else { 1 });
    });
}
fn check_delivery() -> Result<BTreeMap<String, bool>> {
    use super::delivery::Guards;
    let fixture = Path::new("/tmp/mw50-terminal-delivery");
    if !fixture.join("ready").is_file() {
        return Err("Terminal cat fixture is not ready.".into());
    }
    let target = macos_delivery::foreground().ok_or("No foreground Terminal.")?;
    if !macos_delivery::application_name(target)
        .is_some_and(|n| n.ends_with("(com.apple.Terminal)"))
    {
        return Err("Start the synthetic Terminal cat fixture before this probe.".into());
    }
    let mut checks = BTreeMap::new();
    let wanted = std::cell::Cell::new(true);
    let cancel_after_copy = std::cell::Cell::new(false);
    let guards = Guards {
        wanted: &|| wanted.get(),
        superseded: &|| false,
        recording_newer: &|| false,
        waiting: &|| {},
        target_valid: &|| true,
        mark: &|stage| {
            if stage == "clipboard_written" && cancel_after_copy.get() {
                wanted.set(false);
            }
        },
    };
    let mut expected = String::new();
    for (name, text) in [
        ("english", "# MW-50 English\n"),
        ("hebrew", "# בדיקת עברית\n"),
        ("mixed", "# MW-50 בדיקה mixed\n"),
    ] {
        let outcome = macos_delivery::macos_deliver(0, Some(target), text, false, false, &guards)?;
        expected.push_str(text);
        let end = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(fixture.join("received.txt")).unwrap_or_default() != expected
            && Instant::now() < end
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        let correct = outcome.pasted
            && std::fs::read_to_string(fixture.join("received.txt")).unwrap_or_default()
                == expected;
        checks.insert(format!("terminal_{name}_exact"), correct);
        if !correct {
            return Err(format!(
                "Synthetic Terminal {name} check failed: {}",
                outcome.message
            ));
        }
    }
    wanted.set(false);
    let stale =
        macos_delivery::macos_deliver(0, Some(target), "stale fixture", false, false, &guards);
    checks.insert(
        "stale_before_copy_refused".into(),
        stale.is_err() && macos_delivery::read_clipboard()? == "# MW-50 בדיקה mixed\n",
    );
    wanted.set(true);
    cancel_after_copy.set(true);
    let canceled =
        macos_delivery::macos_deliver(0, Some(target), "discarded fixture", false, false, &guards)?;
    std::thread::sleep(Duration::from_millis(100));
    checks.insert(
        "discarded_after_copy_not_pasted".into(),
        canceled.copied
            && !canceled.pasted
            && canceled.message.contains("discarded")
            && std::fs::read_to_string(fixture.join("received.txt")).unwrap_or_default()
                == expected,
    );
    Ok(checks)
}
