//! Fixed local storage choices. Confirmation holds an opaque snapshot, never
//! paths supplied by the WebView. A later recording is outside that snapshot.
use super::*;
use metrix_writer_api::recordings::RecordingStore;
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Serialize, Clone)]
pub struct Preview {
    id: Uuid,
    recordings: usize,
    jobs: usize,
    history: bool,
    before_ms: Option<u64>,
    #[serde(skip)]
    ids: Vec<Uuid>,
}
fn idle(state: &Shell) -> Result<()> {
    if state.capture.status()?.active
        || state.pending.lock().unwrap().is_some()
        || state.history_reload.load(Ordering::Acquire)
        || state.snapshot.lock().unwrap().as_ref().is_some_and(|s| {
            matches!(
                s["state"].as_str(),
                Some("requesting" | "capturing" | "saving" | "queued" | "transcribing")
            )
        })
    {
        return Err("Finish recording and transcription before clearing storage.".into());
    }
    Ok(())
}
#[tauri::command]
pub async fn storage_status(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
) -> Result<Value> {
    main_only(&window)?;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|e| e.message)?;
    let usage = pipeline
        .storage_usage(state.root.join("Models"), state.root.join("State"))
        .await
        .map_err(|e| e.message)?;
    serde_json::to_value(usage).map_err(|_| "Storage usage unavailable.".into())
}
#[tauri::command]
pub async fn storage_preview(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    before_ms: Option<u64>,
    history: bool,
) -> Result<Preview> {
    main_only(&window)?;
    if before_ms.is_some_and(|date| date == 0 || date > storage::now()) {
        return Err("Choose a date through yesterday.".into());
    }
    if history && before_ms.is_none() {
        return Err("Use Delete all audio and transcripts for all history.".into());
    }
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "Recording or history maintenance is in progress.")?;
    idle(&state)?;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|e| e.message)?;
    let capture = state.capture.clone();
    let native = tauri::async_runtime::spawn_blocking(move || capture.list())
        .await
        .map_err(|_| "Saved recordings unavailable.")??;
    let mut ids = BTreeSet::new();
    let eligible = |date| before_ms.is_none_or(|cutoff| date < cutoff);
    for recording in native {
        if eligible(recording.created_at_ms) {
            ids.insert(recording.id);
        }
    }
    for recording in pipeline.recordings.list().await.map_err(|e| e.message)? {
        if eligible(recording.metadata.created_at_ms) {
            ids.insert(recording.metadata.id);
        }
    }
    let all = pipeline.jobs.all().await.map_err(|e| e.message)?;
    if history {
        for job in &all {
            // Audio may already have been deleted. Its retained metadata still
            // gives the original date; never classify by a later retry's date.
            if let Ok(recording) = pipeline.recordings.get(job.request.recording_id).await {
                if eligible(recording.metadata.created_at_ms) {
                    ids.insert(recording.metadata.id);
                }
            }
        }
    }
    let preview = Preview {
        id: Uuid::new_v4(),
        recordings: ids.len(),
        jobs: all
            .iter()
            .filter(|j| ids.contains(&j.request.recording_id))
            .count(),
        history,
        before_ms,
        ids: ids.into_iter().collect(),
    };
    *state.storage_preview.lock().unwrap() = Some(preview.clone());
    Ok(preview)
}
#[tauri::command]
pub async fn storage_clear(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    portable: tauri::State<'_, std::sync::Arc<portable::Portable>>,
    preview_id: Uuid,
) -> Result<Value> {
    main_only(&window)?;
    let _maintenance = state
        .maintenance
        .try_lock()
        .map_err(|_| "Recording or history maintenance is in progress.")?;
    idle(&state)?;
    let preview = state
        .storage_preview
        .lock()
        .unwrap()
        .as_ref()
        .filter(|p| p.id == preview_id)
        .cloned()
        .ok_or("Review storage deletion again before confirming.")?;
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|e| e.message)?;
    pipeline
        .clear_selected_storage(&preview.ids, preview.history)
        .await
        .map_err(|e| e.message)?;
    let capture = state.capture.clone();
    let ids = preview.ids.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<()> {
        for id in ids {
            if capture.get(id)?.is_some() {
                capture.delete(id)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| "Local audio cleanup failed. Review and retry deletion.")??;
    if preview.history {
        portable
            .history
            .clear_local_before(preview.before_ms.ok_or("Deletion date unavailable.")?)
            .await
            .map_err(|_| {
                "Local transcript copies could not be cleared. Review and retry deletion."
            })?;
        for id in &preview.ids {
            state.context.forget(Some(id.to_string()))?;
        }
    }
    // Reload while capture is paused so erased audio cannot be imported back
    // from old UI state. For partial history, clear just these cache entries.
    state.history_reload.store(true, Ordering::Release);
    Ok(json!({"recording_ids": preview.ids, "history": preview.history}))
}
