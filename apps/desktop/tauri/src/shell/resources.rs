use crate::storage::Result;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
#[derive(Deserialize)]
struct Resource {
    path: String,
    bytes: u64,
    sha256: String,
}
/// Where the installer puts `runtime/`, `notices/` and `runtime-manifest.json`.
pub fn dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|_| "Application path unavailable.")?;
    #[cfg(windows)]
    let resources = exe.parent().map(Path::to_path_buf);
    #[cfg(target_os = "macos")]
    let resources = exe
        .parent()
        .and_then(Path::parent)
        .map(|contents| contents.join("Resources"));
    resources.ok_or_else(|| "Application path unavailable.".into())
}
pub fn verify(root: &Path) -> Result<()> {
    let manifest = root.join("runtime-manifest.json");
    if !manifest.exists() {
        return Ok(());
    } // Cloud-only/developer builds expose missing runtime in setup.
    let files: Vec<Resource> = serde_json::from_slice(
        &std::fs::read(manifest).map_err(|_| "Runtime manifest is unreadable.")?,
    )
    .map_err(|_| "Runtime manifest is invalid.")?;
    for file in files {
        if !file.path.starts_with("runtime/") && !file.path.starts_with("notices/") {
            return Err("Invalid packaged resource path.".into());
        }
        if Path::new(&file.path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("Invalid packaged resource path.".into());
        }
        let (bytes, hash) = metrix_writer_api::local_stt::managed::hash_file(&root.join(file.path))
            .map_err(|_| "A packaged runtime file is missing. Repair the installation.")?;
        if bytes != file.bytes || hash != file.sha256 {
            return Err("A packaged runtime file is damaged. Repair the installation; saved data is retained.".into());
        }
    }
    Ok(())
}
