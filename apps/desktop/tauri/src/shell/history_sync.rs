//! Main-window text history controls and account lifecycle integration.
use super::*;
use crate::account::{self, lifecycle, Service as Account};
use metrix_writer_api::account_sync::{
    history::{History, Page, Preview, Status},
    history_wire::HistoryMode,
    sync::AccountRef,
};
use std::sync::Arc;
use uuid::Uuid;

fn key(k: &account::AccountKey) -> AccountRef {
    AccountRef {
        project_id: k.project_id.clone(),
        uid: k.uid.clone(),
    }
}
pub struct Participant(pub Arc<History>);
impl lifecycle::Participant for Participant {
    fn suspend(&self, k: &account::AccountKey) {
        self.0.suspend(&key(k));
    }
    fn local_copy(&self, k: &account::AccountKey) -> std::result::Result<lifecycle::LocalCopy, ()> {
        self.0
            .copies(&key(k))
            .map(|(items, unsynced)| lifecycle::LocalCopy { items, unsynced })
            .map_err(|_| ())
    }
    fn keep_local_copy(&self, k: &account::AccountKey) -> std::result::Result<(), ()> {
        self.0.keep_copies(&key(k)).map_err(|_| ())
    }
    fn remove_local_copy(&self, k: &account::AccountKey) -> std::result::Result<(), ()> {
        self.0.remove_copies(&key(k)).map_err(|_| ())
    }
    fn apply_fence(
        &self,
        k: &account::AccountKey,
        f: &lifecycle::Fence,
    ) -> std::result::Result<(), ()> {
        if f.category == lifecycle::Category::Transcripts {
            self.0.fence(&key(k), f.everywhere).map_err(|_| ())
        } else {
            Ok(())
        }
    }
}
pub async fn observe(history: &History) -> Result<()> {
    use metrix_writer_api::recordings::RecordingStore;
    let epoch = history.source_epoch();
    let pipeline = metrix_writer_api::jobs::runtime()
        .await
        .map_err(|_| "unavailable")?;
    let revision = pipeline.jobs.revision();
    if history.source_is_current(epoch, revision) {
        return Ok(());
    }
    let changes = history
        .observed_revision(epoch)
        .and_then(|after| pipeline.jobs.changes_since(after));
    let incremental = changes.is_some();
    let (revision, jobs) = match changes {
        Some(changes) => changes,
        None => (
            revision,
            pipeline.jobs.all().await.map_err(|_| "unavailable")?,
        ),
    };
    let mut saved = Vec::with_capacity(jobs.len());
    for job in jobs {
        // Deleted/missing local recordings are not silently re-enrolled.
        if let Ok(recording) = pipeline.recordings.get(job.request.recording_id).await {
            saved.push((job, recording.metadata.created_at_ms));
        }
    }
    if incremental {
        history.observe_changes(epoch, &saved)
    } else {
        history.observe_at(epoch, &saved)
    }
    .map_err(portable::remote_kind)?;
    history.mark_source_current(epoch, revision);
    Ok(())
}
#[tauri::command]
pub async fn transcript_status(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<portable::Portable>>,
    account: tauri::State<'_, Account>,
) -> Result<Option<Status>> {
    main_only(&window)?;
    portable::signed_in(&account)
        .map(|(k, _)| state.history.status(&k).map_err(portable::remote_kind))
        .transpose()
}
#[tauri::command]
pub async fn transcript_preview(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<portable::Portable>>,
    account: tauri::State<'_, Account>,
    mode: HistoryMode,
) -> Result<Preview> {
    main_only(&window)?;
    observe(&state.history).await?;
    let (key, g, remote) = portable::remote(&account).await?;
    let current = portable::still(&account, &key, g);
    state
        .history
        .preview(&key, remote.as_ref(), mode, &current)
        .await
        .map_err(portable::remote_kind)
}
#[tauri::command]
pub async fn transcript_confirm(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<portable::Portable>>,
    account: tauri::State<'_, Account>,
    id: Uuid,
    trust: String,
) -> Result<()> {
    main_only(&window)?;
    observe(&state.history).await?;
    let (key, g, remote) = portable::remote(&account).await?;
    let current = portable::still(&account, &key, g);
    state
        .history
        .confirm(&key, remote.as_ref(), id, &trust, &current)
        .await
        .map_err(portable::remote_kind)?;
    state.nudge.notify_one();
    portable::emit(&app, "status");
    Ok(())
}
#[tauri::command]
pub async fn transcript_page(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<portable::Portable>>,
    account: tauri::State<'_, Account>,
    after: Option<Uuid>,
    restore: bool,
) -> Result<Page> {
    main_only(&window)?;
    if restore {
        let (key, g, remote) = portable::remote(&account).await?;
        let current = portable::still(&account, &key, g);
        state
            .history
            .restore(&key, remote.as_ref(), after, &current)
            .await
            .map_err(portable::remote_kind)
    } else {
        state
            .history
            .local_page(
                portable::signed_in(&account).as_ref().map(|(k, _)| k),
                after,
            )
            .map_err(portable::remote_kind)
    }
}
#[tauri::command]
pub async fn transcript_edit(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<portable::Portable>>,
    account: tauri::State<'_, Account>,
    parent: Uuid,
    text: String,
) -> Result<Uuid> {
    main_only(&window)?;
    let (key, _) = portable::signed_in(&account).ok_or("not_signed_in")?;
    let id = state
        .history
        .edit(&key, parent, text)
        .map_err(portable::remote_kind)?;
    state.nudge.notify_one();
    portable::emit(&app, "status");
    Ok(id)
}
#[tauri::command]
pub async fn transcript_delete(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<portable::Portable>>,
    account: tauri::State<'_, Account>,
    id: Uuid,
) -> Result<()> {
    main_only(&window)?;
    let (key, _) = portable::signed_in(&account).ok_or("not_signed_in")?;
    state
        .history
        .delete(&key, id)
        .map_err(portable::remote_kind)?;
    state.nudge.notify_one();
    portable::emit(&app, "status");
    Ok(())
}
#[tauri::command]
pub async fn transcript_turn_off(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<portable::Portable>>,
    account: tauri::State<'_, Account>,
) -> Result<()> {
    main_only(&window)?;
    let (key, g, remote) = portable::remote(&account).await?;
    let current = portable::still(&account, &key, g);
    state
        .history
        .turn_off(&key, remote.as_ref(), &current)
        .await
        .map_err(portable::remote_kind)?;
    portable::emit(&app, "status");
    Ok(())
}
