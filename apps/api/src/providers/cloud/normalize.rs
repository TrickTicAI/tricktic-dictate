//! Stream the existing PCM16 WAV normalization to a disposable sidecar.
//! Same WAV fields and shared resampler as local_stt::pcm; no codec selection changes.
use super::*;
use std::{
    io::{Read, Seek, Write},
    path::PathBuf,
};
const LIMIT: u64 = 256 * 1024 * 1024;

pub(super) struct Normalized {
    pub audio: SavedAudio,
    path: PathBuf,
}
impl Drop for Normalized {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(super) async fn normalize(audio: &SavedAudio) -> Option<Normalized> {
    let audio = audio.clone();
    tokio::task::spawn_blocking(move || convert(audio).ok().flatten())
        .await
        .ok()
        .flatten()
}
fn convert(mut audio: SavedAudio) -> std::io::Result<Option<Normalized>> {
    let mut input = std::fs::File::open(&audio.path)?;
    let length = input.metadata()?.len();
    if length != audio.metadata.byte_length || length > LIMIT {
        return Ok(None);
    }
    let mut magic = [0; 12];
    if input.read_exact(&mut magic).is_err()
        || !matches!(&magic[..4], b"RIFF" | b"RF64")
        || &magic[8..] != b"WAVE"
    {
        return Ok(None);
    }
    let mut cursor = 12u64;
    let mut format = None;
    let mut rf64_length = None;
    while cursor + 8 <= length {
        input.seek(std::io::SeekFrom::Start(cursor))?;
        let mut header = [0; 8];
        input.read_exact(&mut header)?;
        let kind = &header[..4];
        let declared = u32::from_le_bytes(header[4..].try_into().unwrap());
        let size = if kind == b"data" && declared == u32::MAX {
            let Some(size) = rf64_length else {
                return Ok(None);
            };
            size
        } else {
            declared as u64
        };
        let start = cursor + 8;
        let Some(end) = start.checked_add(size).filter(|end| *end <= length) else {
            return Ok(None);
        };
        if kind == b"ds64" && size >= 28 {
            let mut fields = [0; 28];
            input.read_exact(&mut fields)?;
            rf64_length = Some(u64::from_le_bytes(fields[8..16].try_into().unwrap()));
        } else if kind == b"fmt " && size >= 16 {
            let mut fields = [0; 16];
            input.read_exact(&mut fields)?;
            let field = |at| u16::from_le_bytes(fields[at..at + 2].try_into().unwrap());
            let rate = u32::from_le_bytes(fields[4..8].try_into().unwrap());
            let channels = field(2);
            if field(0) != 1
                || field(14) != 16
                || channels == 0
                || channels > 32
                || field(12) != channels * 2
                || !(8000..=384000).contains(&rate)
            {
                return Ok(None);
            }
            format = Some((rate, channels));
        } else if kind == b"data" {
            let Some((rate, channels)) = format else {
                return Ok(None);
            };
            if size % (channels as u64 * 2) != 0 {
                return Ok(None);
            }
            let path = audio
                .path
                .with_file_name(format!(".cloud-{}.pcm", uuid::Uuid::new_v4()));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options.open(&path)?;
            audio.prepared = Some(crate::providers::PreparedAudio {
                path: path.clone(),
                byte_length: 0,
            });
            let mut normalized = Normalized { audio, path };
            let mut resampler = writer_dictation_audio::Resampler::new(rate, channels)
                .map_err(std::io::Error::other)?;
            let mut remaining = size;
            let mut written = 0;
            // Match the prior converter's input boundaries and reuse bounded buffers.
            let mut buffer = vec![0; 8192 * usize::from(channels)];
            let mut samples = Vec::with_capacity(buffer.len() / 2);
            let mut converted = Vec::new();
            while remaining > 0 {
                let count = remaining.min(buffer.len() as u64) as usize;
                input.read_exact(&mut buffer[..count])?;
                samples.clear();
                samples.extend(
                    buffer[..count]
                        .chunks_exact(2)
                        .map(|s| i16::from_le_bytes([s[0], s[1]])),
                );
                converted.clear();
                resampler
                    .push(&samples, &mut converted)
                    .map_err(std::io::Error::other)?;
                write_pcm(&mut output, &converted, &mut written)?;
                remaining -= count as u64;
            }
            converted.clear();
            resampler
                .finish(&mut converted)
                .map_err(std::io::Error::other)?;
            write_pcm(&mut output, &converted, &mut written)?;
            output.flush()?;
            normalized.audio.prepared.as_mut().unwrap().byte_length = written;
            return Ok(Some(normalized));
        }
        cursor = end + size % 2;
    }
    Ok(None)
}
fn write_pcm(
    output: &mut std::fs::File,
    samples: &[i16],
    written: &mut u64,
) -> std::io::Result<()> {
    *written += samples.len() as u64 * 2;
    if *written > LIMIT {
        return Err(std::io::Error::other(
            "Normalized PCM exceeds supported size.",
        ));
    }
    // Bound the serialization buffer independently of resampling ratio.
    for chunk in samples.chunks(24 * 1024) {
        let bytes: Vec<_> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        output.write_all(&bytes)?;
    }
    Ok(())
}
