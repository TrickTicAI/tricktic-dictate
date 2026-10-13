//! Bounded, shell-free ffmpeg invocation. No conversion files or stderr logging.
use super::{audio_too_large, error, timeout_error};
use crate::transcription::{ApiError, ErrorCode};
use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub(super) async fn run(
    executable: &Path,
    args: &[OsString],
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, ApiError> {
    let mut command = owned_command(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW, including health probes
    let mut child = command.spawn().map_err(|_| {
        error(
            ErrorCode::ProviderUnavailable,
            "Local ffmpeg runtime could not start.",
            true,
        )
    })?;
    #[cfg(any(windows, target_os = "macos"))]
    let _job = match super::worker_job::WorkerJob::attach(child.id().unwrap_or(0)) {
        Some(job) => job,
        None => {
            let _ = child.kill().await;
            return Err(failed());
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = Vec::new();
        stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| failed())?;
        if bytes.len() > limit {
            return Err(audio_too_large());
        }
        if !child.wait().await.map_err(|_| failed())?.success() {
            return Err(failed());
        }
        Ok(bytes)
    })
    .await
    .unwrap_or_else(|_| Err(timeout_error()));
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

fn failed() -> ApiError {
    error(
        ErrorCode::ProviderFailed,
        "Audio conversion failed; saved audio is retained for retry.",
        true,
    )
}

/// Packaged macOS workers run under a native parent-death supervisor. Explicit
/// external tools and test fixtures still get a private group for normal exit.
pub(crate) fn owned_command(executable: &Path) -> Command {
    #[cfg(target_os = "macos")]
    {
        let host = executable.with_file_name("writer-worker-host");
        let mut command = if host.is_file() {
            let mut command = Command::new(host);
            command.arg(executable);
            command
        } else {
            Command::new(executable)
        };
        command.process_group(0);
        crate::helper_security::configure(&mut command);
        command
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut command = Command::new(executable);
        crate::helper_security::configure(&mut command);
        command
    }
}
