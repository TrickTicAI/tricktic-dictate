//! PCM16 takes the in-process path; compressed containers retain the fixed ffmpeg decoder.
use super::{audio_too_large, error, ApiError, ErrorCode};
use std::path::Path;

pub(crate) async fn convert(
    path: &Path,
    limit: usize,
    raw: bool,
) -> Result<Option<Vec<u8>>, ApiError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let fail = || {
            error(
                ErrorCode::AudioNotSaved,
                "Cannot read saved PCM audio.",
                true,
            )
        };
        let mut file = std::fs::File::open(path).map_err(|_| fail())?;
        let mut magic = [0; 12];
        if file.read_exact(&mut magic).is_err()
            || !matches!(&magic[..4], b"RIFF" | b"RF64")
            || &magic[8..] != b"WAVE"
        {
            return Ok(None);
        }
        let mut bytes = magic.to_vec();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| fail())?;
        if bytes.len() > limit {
            return Err(audio_too_large());
        }
        let Ok(wav) = writer_dictation_audio::wav_pcm16(&bytes) else {
            return Ok(None);
        };
        let mut resampler =
            writer_dictation_audio::Resampler::new(wav.rate, wav.channels).map_err(|_| fail())?;
        let mut output = Vec::new();
        for chunk in wav.data.chunks(8192 * usize::from(wav.channels)) {
            let samples: Vec<_> = chunk
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect();
            resampler.push(&samples, &mut output).map_err(|_| fail())?;
            if output.len() > limit / 2 {
                return Err(audio_too_large());
            }
        }
        resampler.finish(&mut output).map_err(|_| fail())?;
        if output.len() > limit / 2 {
            return Err(audio_too_large());
        }
        let pcm: Vec<_> = output.iter().flat_map(|s| s.to_le_bytes()).collect();
        Ok(Some(if raw { pcm } else { super::mixed::wav(&pcm) }))
    })
    .await
    .map_err(|_| {
        error(
            ErrorCode::ProviderFailed,
            "PCM conversion failed; saved audio is retained.",
            true,
        )
    })?
}
