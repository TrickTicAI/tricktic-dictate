//! Bounded streaming write shared by ingestion and its ENOSPC fixture.
use super::{
    disk::{error, storage},
    AudioReader,
};
use crate::transcription::{ApiError, ErrorCode};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn receive(
    mut reader: AudioReader,
    mut file: tokio::fs::File,
    expected: u64,
    idle: Duration,
) -> Result<u64, ApiError> {
    let mut buffer = vec![0; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = tokio::time::timeout(idle, reader.read(&mut buffer))
            .await
            .map_err(|_| crate::limits::idle())?
            .map_err(|_| {
                error(
                    ErrorCode::Interrupted,
                    "Upload was interrupted; retry uploading the retained browser recording.",
                    true,
                )
            })?;
        if count == 0 {
            break;
        }
        if total + count as u64 > expected {
            return Err(error(
                ErrorCode::InvalidRequest,
                "Audio exceeds the declared byte length; upload was not committed.",
                false,
            ));
        }
        file.write_all(&buffer[..count]).await.map_err(storage)?;
        total += count as u64;
    }
    // Includes flushing Tokio's buffered write: ENOSPC can surface here.
    file.sync_all().await.map_err(storage)?;
    if total != expected {
        return Err(error(
            ErrorCode::Interrupted,
            "Upload is incomplete; retry uploading the retained browser recording.",
            true,
        ));
    }
    Ok(total)
}
