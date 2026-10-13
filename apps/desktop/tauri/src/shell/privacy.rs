use crate::storage::Result;
use std::path::Path;
/// Protect the desktop root before capture, settings, credentials or models.
pub fn protect(root: &Path) -> Result<()> {
    metrix_writer_api::helper_security::protect_private_dir(root).map_err(|_| {
        "Cannot protect private storage; use an owned directory without links or junctions.".into()
    })
}
