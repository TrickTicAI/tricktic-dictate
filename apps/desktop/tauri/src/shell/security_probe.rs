//! Compiled only for an explicitly requested private security fixture.
use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Receipts(Mutex<BTreeMap<String, BTreeMap<String, bool>>>);

#[tauri::command]
fn security_probe_receipt(
    window: WebviewWindow,
    app: tauri::AppHandle,
    result: BTreeMap<String, bool>,
    done: Option<bool>,
) -> Result<()> {
    let state = app.state::<Shell>();
    if !state.probe
        || !std::env::args().any(|a| a == "--security-probe")
        || result.is_empty()
        || result.len() > 128
    {
        return Err("Security fixture disabled.".into());
    }
    let receipts = app.state::<Receipts>();
    let mut receipts = receipts.0.lock().unwrap();
    if !["main", "overlay", "tray-menu", "settings"].contains(&window.label()) {
        return Err("Unknown fixture window.".into());
    }
    if done == Some(false) {
        return storage::atomic_json(
            &state
                .root
                .join(format!("security-progress-{}.json", window.label())),
            &result,
        );
    }
    storage::atomic_json(
        &state
            .root
            .join(format!("security-result-{}.json", window.label())),
        &result,
    )?;
    receipts.insert(window.label().into(), result);
    if receipts.len() == 4 {
        let passed = receipts
            .values()
            .all(|checks| checks.values().all(|value| *value));
        storage::atomic_json(
            &state.root.join("security-probe.json"),
            &json!({
                "platform": std::env::consts::OS, "passed": passed, "windows": *receipts,
                "popup_count": app.webview_windows().len().saturating_sub(4),
            }),
        )?;
        app.exit(if passed && app.webview_windows().len() == 4 {
            0
        } else {
            1
        });
    }
    Ok(())
}

/// Synthetic saved content only; this runs before the API loads the private root.
pub fn prepare(root: &std::path::Path) -> Result<()> {
    // A fixture must not discover inherited paid-provider credentials.
    for name in [
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "XAI_API_KEY",
        "OPENROUTER_API_KEY",
    ] {
        std::env::remove_var(name);
    }
    let payload = "<img id=security-payload src=x onerror=window.securityExecuted=true><script>window.securityExecuted=true</script> שלום";
    std::fs::create_dir_all(root.join("State")).map_err(|_| "Fixture state unavailable.")?;
    storage::atomic_json(
        &root.join("State/onboarding.json"),
        &json!({"completed_version": 1}),
    )?;
    let store = Store::open(root.join("Data/native-recordings"))?;
    let id = uuid::Uuid::new_v4();
    let mut journal = store.create(id, "mixed".into(), 16000, 1)?;
    journal.append(&[0; 1600])?;
    journal.finish(false)?;
    let options = json!({"selection":{"provider":"openai","model":"gpt-transcribe"},"input_language":"mixed","output_language":{"mode":"preserve"},"vocabulary":{"hints":[],"corrections":[],"snippets":[]},"cleanup":{"mode":"off"}});
    let job = uuid::Uuid::new_v4();
    std::fs::create_dir_all(root.join("Data/jobs")).map_err(|_| "Fixture jobs unavailable.")?;
    storage::atomic_json(
        &root.join(format!("Data/jobs/{job}.json")),
        &json!({
            "id":job,"request":{"recording_id":id,"idempotency_key":uuid::Uuid::new_v4(),"options":options},
            "created_at_ms":1700000000001_u64,"updated_at_ms":1700000000002_u64,
            "status":{"state":"succeeded","result":{"transcript":{"raw_text":payload,"selection":options["selection"],"execution_location":"cloud","input_language":"mixed","output_language":{"mode":"preserve"}},"cleanup":{"state":"off"}}}
        }),
    )?;
    storage::atomic_json(
        &root.join("Data/vocabulary.json"),
        &json!({"hints":[],"corrections":[],"snippets":[{"phrase":"fixture phrase","text":payload,"enabled":true}]}),
    )?;
    Ok(())
}

pub fn receive(invoke: tauri::ipc::Invoke) -> bool {
    let handler: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![security_probe_receipt];
    handler(invoke)
}

pub fn start(app: &tauri::AppHandle) -> tauri::Result<()> {
    app.manage(Receipts::default());
    // WKWebView throttles timers in hidden windows. The main fixture exercises
    // real UI navigation, so give it a visible window for bounded DOM waits.
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.set_focus()?;
    }
    for (label, page) in [
        ("tray-menu", "desktop-tray.html"),
        ("settings", "index.html"),
    ] {
        WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
            .visible(false)
            .focused(false)
            .data_directory(app.state::<Shell>().root.join(format!("Security-{label}")))
            .on_navigation(move |url| {
                security::document_url(if label == "settings" { "main" } else { label }, url)
            })
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
            .build()?;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let commands: Vec<_> = security::MAIN_COMMANDS
            .iter()
            .chain(security::TRAY_COMMANDS)
            .copied()
            .chain(["overlay_action"])
            .collect();
        for (label, window) in app.webview_windows() {
            let setup = json!({"label": label, "commands": commands});
            let evaluated = window
                .eval(format!(
                    "window.writerSecurityFixture={setup};{}",
                    include_str!("security-probe.js")
                ))
                .is_ok();
            let _ = storage::atomic_json(
                &app.state::<Shell>()
                    .root
                    .join(format!("security-eval-{label}.json")),
                &json!({"evaluated": evaluated}),
            );
        }
        std::thread::sleep(Duration::from_secs(45));
        app.exit(2);
    });
    Ok(())
}
