use super::*;
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
pub struct StorageUsage {
    pub data_bytes: u64,
    pub data_limit_bytes: u64,
    pub audio_bytes: u64,
    pub models_bytes: u64,
    pub state_bytes: u64,
}
impl Pipeline {
    pub async fn storage_usage(
        &self,
        models: PathBuf,
        state: PathBuf,
    ) -> Result<StorageUsage, ApiError> {
        let data = self.preview_root.parent().unwrap().to_owned();
        let (data_bytes, audio_bytes, models_bytes, state_bytes) =
            crate::limits::storage::usage(data, models, state).await?;
        Ok(StorageUsage {
            data_bytes,
            audio_bytes,
            models_bytes,
            state_bytes,
            data_limit_bytes: self.recordings.budget.limits.device_storage_bytes,
        })
    }
    pub async fn clear_selected_storage(
        &self,
        ids: &[Uuid],
        history: bool,
    ) -> Result<Vec<Uuid>, ApiError> {
        let batches = self.batch_service().await?;
        let _batch_guard = batches.begin_deletion().await?;
        let setup = crate::desktop_settings::global()
            .ok()
            .filter(|s| self.mode_root == s.layout.state_dir.join("modes"));
        let _maintenance = setup
            .as_ref()
            .map(|s| s.runtime.maintenance())
            .transpose()?;
        let modes = self.mode_service().await?;
        let processing = modes.begin_history_deletion().await?;
        let _preview = crate::local_stt::pause_previews()?;
        let jobs = self
            .jobs
            .delete_selected(&self.recordings, ids, history)
            .await?;
        if history {
            batches.clear_selected(ids).await?;
            processing.commit_selected(ids, &jobs).await?;
        }
        for provider in &self.providers {
            provider.clear_preview();
        }
        Ok(jobs)
    }
}
