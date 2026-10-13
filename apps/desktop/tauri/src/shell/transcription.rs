//! UUID-only native audio handoff. Original capture never crosses the WebView.
use super::*;
use metrix_writer_api::{
    providers::{PreparedAudio, SavedAudio},
    recordings::RecordingStore,
    transcription::TranscriptionOptions,
};

pub(super) fn saved(capture: &Capture, id: uuid::Uuid, preview: bool) -> Result<SavedAudio> {
    let (metadata, live) = if preview {
        let (metadata, path, byte_length) = capture.preview(id)?;
        (metadata, Some(PreparedAudio { path, byte_length }))
    } else {
        (capture.get(id)?.ok_or("Recording not found.")?, None)
    };
    if !preview && metadata.capture_state == "capturing" {
        return Err("Stop recording before transcription.".into());
    }
    let prepared = live.or_else(|| {
        capture
            .store
            .prepared(id, &metadata)
            .map(|(path, byte_length)| PreparedAudio { path, byte_length })
    });
    if preview && prepared.is_none() {
        return Err("Prepared preview is not available yet.".into());
    }
    let value = serde_json::to_value(&metadata).map_err(|_| "Invalid recording metadata.")?;
    // Native-only fields aren't part of the shared upload contract.
    let mut value = value;
    for name in ["chunk_count", "sample_rate", "channels"] {
        value.as_object_mut().unwrap().remove(name);
    }
    Ok(SavedAudio {
        metadata: serde_json::from_value(value).map_err(|_| "Invalid recording metadata.")?,
        path: capture.store.path(id).join("audio.wav"),
        prepared,
    })
}

#[tauri::command]
pub(super) async fn capture_import(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    id: uuid::Uuid,
) -> Result<()> {
    main_only(&window)?;
    metrix_writer_api::latency::mark(id, "audio_import_requested");
    let _maintenance = state.maintenance.lock().await;
    metrix_writer_api::latency::mark(id, "audio_import_lock_acquired");
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "Saved audio unavailable.")?;
    // History/retry may contain server-origin audio with no native journal.
    if state.capture.get(id)?.is_none() {
        pipeline
            .recordings
            .audio(id)
            .await
            .map_err(|_| "Saved audio unavailable.")?;
        return Ok(());
    }
    let source = saved(&state.capture, id, false)?;
    pipeline
        .recordings
        .import_native(source)
        .await
        .map_err(|e| e.message)?;
    metrix_writer_api::latency::mark(id, "audio_imported");
    Ok(())
}

#[tauri::command]
pub(super) async fn capture_preview(
    window: WebviewWindow,
    state: tauri::State<'_, Shell>,
    id: uuid::Uuid,
    options: TranscriptionOptions,
) -> Result<String> {
    main_only(&window)?;
    // Acquire the trusted paths under maintenance, then let the API preview slot
    // coordinate inference/deletion. Never hold up final registration on compute.
    let audio = {
        let _maintenance = state.maintenance.lock().await;
        let status = state.capture.status()?;
        if !status.active || status.id != Some(id) {
            return Err("Capture is no longer active.".into());
        }
        saved(&state.capture, id, true)?
    };
    metrix_writer_api::local_stt::native_preview(audio, options)
        .await
        .map_err(|e| e.message)
}

/// One native loop per capture; neither input publication nor inference scheduling
/// depends on background WebView timers. All paths originate from the recorder.
#[tauri::command]
pub(super) async fn capture_preview_start(
    window: WebviewWindow,
    app: tauri::AppHandle,
    id: uuid::Uuid,
    mut options: TranscriptionOptions,
) -> Result<()> {
    main_only(&window)?;
    if !matches!(
        options.selection.provider,
        metrix_writer_api::transcription::ProviderId::Local
    ) {
        return Err("Preview requires a local model.".into());
    }
    let state = app.state::<Shell>();
    let capture = state.capture.clone();
    let status = capture.status()?;
    if !status.active || status.id != Some(id) {
        return Err("Capture is no longer active.".into());
    }
    if !state.preview_sessions.lock().unwrap().insert(id) {
        return Ok(());
    }
    options.cleanup = metrix_writer_api::transcription::CleanupPolicy::Off {};
    {
        let mut pending = state.final_options.lock().unwrap();
        pending.retain(|session, _| *session == id);
        pending.insert(id, options.clone());
    }
    tauri::async_runtime::spawn(async move {
        let mut previous = 0;
        loop {
            // Coalesces new frames while inference runs, without an extra 500ms wait.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let audio = {
                let state = app.state::<Shell>();
                let _maintenance = state.maintenance.lock().await;
                let capture = capture.clone();
                tauri::async_runtime::spawn_blocking(move || saved(&capture, id, true)).await
            };
            let audio = match audio {
                Ok(Ok(audio)) => audio,
                _ => {
                    // Some microphone drivers take longer than one tick to
                    // publish their first buffer. Keep waiting while this exact
                    // capture is active; Stop/discard always ends the loop.
                    let capture = capture.clone();
                    let active =
                        tauri::async_runtime::spawn_blocking(move || capture.status()).await;
                    if matches!(active, Ok(Ok(status)) if status.active && status.id == Some(id)) {
                        continue;
                    }
                    break;
                }
            };
            let bytes = audio.prepared.as_ref().map_or(0, |p| p.byte_length);
            if bytes < 16_000 || bytes <= previous {
                continue;
            }
            previous = bytes;
            if let Ok(text) =
                metrix_writer_api::local_stt::native_preview(audio, options.clone()).await
            {
                // The mode cache accepts a just-finished preview during the
                // release grace, but never recreates canceled/finalized entries.
                if let Ok(modes) = metrix_writer_api::cleanup::modes::service().await {
                    modes
                        .prepare_draft(id, text.clone(), options.vocabulary.clone())
                        .await;
                }
                if capture.status().is_ok_and(|s| s.active && s.id == Some(id)) {
                    let _ = app.emit_to("main", "writer-preview", json!({"id":id,"text":text}));
                }
            }
        }
        app.state::<Shell>()
            .preview_sessions
            .lock()
            .unwrap()
            .remove(&id);
    });
    Ok(())
}

/// Start the final-quality pass on the just-finished capture while the WebView
/// still imports the audio and persists the job. Speculative only: the durable
/// job stays the authority and finds every window already cached. Any failure
/// here is silent; the job decodes normally.
pub(super) fn final_ahead(capture: Capture, id: uuid::Uuid, options: TranscriptionOptions) {
    tauri::async_runtime::spawn(async move {
        let audio = tauri::async_runtime::spawn_blocking(move || saved(&capture, id, false)).await;
        if let Ok(Ok(audio)) = audio {
            let _ = metrix_writer_api::local_stt::native_final(audio, options).await;
        }
    });
}
