//! Portable settings and dictionary: manual export/import without an account
//! and optional account sync (MW-OUATH-6), implementing the MW-OUATH-1 v1
//! contract and the MW-OUATH-5 Firestore protocol for the desktop.
//! See docs/testing/account-sync/desktop-sync.md.
pub mod desktop;
pub mod firestore;
pub mod history;
mod history_disk;
pub mod history_wire;
pub mod json;
pub mod local;
#[cfg(test)]
mod memory;
pub mod plan;
pub mod portable;
pub mod sync;
pub mod wire;

#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod tests;

use crate::transcription::ApiError;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

fn changes() -> &'static tokio::sync::watch::Sender<u64> {
    static CHANGES: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();
    CHANGES.get_or_init(|| tokio::sync::watch::channel(0).0)
}
/// Called after a portable store (settings, dictionary, modes) saved, so an
/// active sync can queue the change. It never blocks or fails the save.
pub fn note_local_change() {
    changes().send_modify(|n| *n = n.wrapping_add(1));
}
pub fn local_changes() -> tokio::sync::watch::Receiver<u64> {
    changes().subscribe()
}

/// The desktop's portable service and sync engine over the in-process stores.
/// State lives in `<state_dir>/account-sync/`.
pub async fn open_desktop(state_dir: PathBuf) -> Result<Arc<sync::Engine>, ApiError> {
    let setup = crate::desktop_settings::global()?;
    let modes = crate::cleanup::modes::service().await.ok();
    let stores = Arc::new(desktop::DesktopStores {
        setup: setup.clone(),
        vocabulary: crate::vocabulary::global_store(),
        modes,
    });
    let caps = Arc::new(desktop::DesktopCapabilities { setup });
    let dir = state_dir.join("account-sync");
    let local = local::Service::open(dir.clone(), stores, caps).await?;
    sync::Engine::open(dir.join("accounts"), local).await
}
