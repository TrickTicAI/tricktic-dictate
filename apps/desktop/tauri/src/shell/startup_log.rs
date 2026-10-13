//! A plain record of how each launch went, because a release build has nowhere
//! else to say it: `windows_subsystem = "windows"` discards stdout/stderr and a
//! failure before the window exists is only ever a message box the user then
//! dismisses. Without this, a sign-in launch that misbehaved leaves no evidence
//! at all and the next report can only be guessed at.
//!
//! This is diagnostics, never dictation: it records milestone names, flags and
//! bounded error strings. Transcripts, audio, credentials and paths the user did
//! not choose never reach it.
use std::{
    io::{Seek, SeekFrom, Write},
    path::Path,
};

/// Keeps a boot's worth of history without ever growing unbounded, matching the
/// packaged worker log's cap.
const MAX_BYTES: u64 = 64 * 1024;

fn line(root: &Path, message: &str) -> std::io::Result<()> {
    let state = root.join("State");
    std::fs::create_dir_all(&state)?;
    let path = state.join("startup.log");
    // Truncating at the cap keeps the most recent launches, which are the ones a
    // report is ever about. A log that cannot be written never fails a launch.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.seek(SeekFrom::End(0))?;
    writeln!(file, "{} {message}", crate::storage::now())
}

/// Records one launch milestone. Milestones are fixed strings chosen here, so a
/// caller cannot write arbitrary content into the file.
pub fn mark(root: &Path, message: &str) {
    let _ = line(root, message);
}
