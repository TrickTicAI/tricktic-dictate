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
/// Saved dictation data against its limit, from the storage budget's last
/// measurement: cheap enough to ask after every dictation.
#[derive(Serialize)]
pub struct StorageBudgetStatus {
    pub used_bytes: u64,
    pub limit_bytes: u64,
}
/// The storage limit as Settings shows it.
#[derive(Serialize)]
pub struct StorageLimit {
    pub limit_bytes: u64,
    pub default_bytes: u64,
    /// WRITER_DEVICE_STORAGE_BYTES sets it; the choice is read-only.
    pub fixed_by_environment: bool,
    /// Free space on the drive holding saved data, when the system says.
    pub free_bytes: Option<u64>,
}
impl Pipeline {
    /// Desktop only: applies the profile's saved storage limit, unless the
    /// environment fixes it, and lets the user change it from then on. The
    /// aggregate cap follows the device limit up on the desktop.
    pub(crate) async fn use_storage_setting(&self, setting: crate::limits::setting::LimitSetting) {
        let budget = &self.recordings.budget;
        let saved = if setting.fixed_by_environment {
            None
        } else {
            setting.load().await
        };
        budget.set_device_limit(saved.unwrap_or(budget.limits.device_storage_bytes), true);
        let _ = self.storage_setting.set(setting);
    }
    pub fn storage_limit(&self) -> StorageLimit {
        let budget = &self.recordings.budget;
        StorageLimit {
            limit_bytes: budget.device_limit(),
            default_bytes: budget.limits.device_storage_bytes,
            fixed_by_environment: self
                .storage_setting
                .get()
                .is_none_or(|s| s.fixed_by_environment),
            free_bytes: self
                .preview_root
                .parent()
                .and_then(crate::limits::setting::free_bytes),
        }
    }
    /// Saves and applies a new limit in whole GB. Takes effect for the next
    /// admission; nothing is deleted when it is below current usage.
    pub async fn set_storage_limit(&self, gigabytes: u64) -> Result<StorageLimit, ApiError> {
        let setting = self.storage_setting.get().ok_or_else(|| {
            crate::recordings::disk::error(
                ErrorCode::InvalidRequest,
                "The storage limit can only be changed in the desktop app.",
                false,
            )
        })?;
        let bytes = gigabytes.saturating_mul(crate::limits::setting::GIB);
        setting.save(bytes).await?;
        self.recordings.budget.set_device_limit(bytes, true);
        Ok(self.storage_limit())
    }
    /// `None` until the startup measurement has finished. Never walks.
    pub fn storage_budget(&self) -> Option<StorageBudgetStatus> {
        let data = self.preview_root.parent()?;
        let budget = &self.recordings.budget;
        Some(StorageBudgetStatus {
            used_bytes: budget.cached_usage(data)?,
            limit_bytes: budget.device_limit(),
        })
    }
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
            data_limit_bytes: self.recordings.budget.device_limit(),
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
            self.references.delete_selected(ids).await?;
        }
        for provider in &self.providers {
            provider.clear_preview();
        }
        self.recordings.budget.remeasure();
        Ok(jobs)
    }
}
