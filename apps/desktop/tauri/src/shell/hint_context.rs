//! Which dictionary hints matter for this dictation (MW-122). When a dictation
//! starts, Writer tells local recognition the target application's name, its
//! window title, the terms in the user's recent History corrections and the
//! terms he used in that application before; `vocabulary::scope` picks the
//! hints that fit Whisper's prompt from those. The title stays in memory for
//! this recording and is never saved, logged or sent anywhere.
//!
//! `State/hint-usage.json` counts, per application name, how often each
//! dictionary hint appeared in delivered text. It holds dictionary entries and
//! application names only, both already in local history.
use super::*;
use metrix_writer_api::vocabulary::scope::{self, mentioned, HintContext, HintUsage};

const USAGE_FILE: &str = "State/hint-usage.json";
const RECENT_REFERENCES: usize = 20;

fn load(root: &std::path::Path) -> HintUsage {
    std::fs::read(root.join(USAGE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

async fn dictionary() -> Vec<String> {
    metrix_writer_api::vocabulary::global_store()
        .load()
        .await
        .map(|document| document.active().hints)
        .unwrap_or_default()
}

/// Terms the user restored recently: dictionary hints in their own History
/// corrections, newest first, and the replacements of their dictionary
/// corrections.
async fn recent(hints: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(pipeline) = metrix_writer_api::jobs::runtime().await {
        if let Ok(mut references) = pipeline.references.all().await {
            references
                .retain(|r| r.source == metrix_writer_api::jobs::references::ReferenceSource::User);
            references.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
            for reference in references.iter().take(RECENT_REFERENCES) {
                for term in mentioned(&reference.text, hints) {
                    if !out.contains(&term) {
                        out.push(term);
                    }
                }
            }
        }
    }
    if let Ok(document) = metrix_writer_api::vocabulary::global_store().load().await {
        for correction in document.active().corrections {
            for term in mentioned(&correction.to, hints) {
                if !out.contains(&term) {
                    out.push(term);
                }
            }
        }
    }
    out
}

#[cfg(windows)]
fn title(window: isize) -> Option<String> {
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowTextW(window: isize, text: *mut u16, maximum: i32) -> i32;
    }
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    let title = String::from_utf16_lossy(&buffer[..length.max(0) as usize]);
    Some(title).filter(|t| !t.trim().is_empty())
}

/// Registers this recording's hint context before its first decode. Never
/// delays or fails the capture: without it, recognition uses dictionary order.
pub(super) fn register(state: &Shell, id: uuid::Uuid) {
    let target = state.delivery.lock().unwrap().target(&id.to_string());
    let root = state.root.clone();
    tauri::async_runtime::spawn(async move {
        let hints = dictionary().await;
        if hints.is_empty() {
            return;
        }
        let named = match target {
            Some(target) => tauri::async_runtime::spawn_blocking(move || {
                #[cfg(windows)]
                let title = title(target.window);
                #[cfg(not(windows))]
                let title = None;
                (delivery::application_name(target), title)
            })
            .await
            .unwrap_or_default(),
            None => (None, None),
        };
        let (app, title) = named;
        let used = app
            .as_deref()
            .map(|app| load(&root).used_in(app))
            .unwrap_or_default();
        let recent = recent(&hints).await;
        scope::set_context(
            id,
            HintContext {
                app,
                title,
                recent,
                used,
            },
        );
    });
}

/// Counts the dictionary hints in text delivered to `app`.
pub(super) async fn record_use(root: std::path::PathBuf, app: String, text: String) {
    let hints = dictionary().await;
    let used = mentioned(&text, &hints);
    if used.is_empty() {
        return;
    }
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let mut usage = load(&root);
        usage.add(&app, &used);
        storage::atomic_json(&root.join(USAGE_FILE), &usage)
    })
    .await;
}
