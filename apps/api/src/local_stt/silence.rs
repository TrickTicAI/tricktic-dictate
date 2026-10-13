//! Remove only the middle of long, proven quiet spans from disposable PCM.
//! Offsets always refer to the original 16 kHz mono PCM16, never the saved WAV.
use std::ops::Range;

const FRAME: usize = 512; // 16 ms
const MIN_QUIET: usize = 5 * 32_000;
const PAD: usize = 16_000; // 500 ms at each seam

/// Conservative ceiling measured on retained Windows microphone PCM (MW-104 R-4).
pub(crate) const QUALIFIED_RMS: f64 = 32.;

struct Scanner {
    detector: earshot::Detector,
    rms_ceiling: f64,
    offset: usize,
    start: Option<usize>,
    removed: Vec<Range<usize>>,
}
impl Scanner {
    fn new(rms_ceiling: f64) -> Self {
        Self {
            detector: earshot::Detector::default(),
            rms_ceiling,
            offset: 0,
            start: None,
            removed: Vec::new(),
        }
    }
    fn close(&mut self) {
        if let Some(start) = self.start.take() {
            if self.offset - start >= MIN_QUIET {
                self.removed.push(start + PAD..self.offset - PAD);
            }
        }
    }
    fn push(&mut self, frame: &[u8]) {
        let samples: [i16; 256] =
            std::array::from_fn(|j| i16::from_le_bytes([frame[j * 2], frame[j * 2 + 1]]));
        let energy = samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
        let score = self.detector.predict_i16(&samples);
        let digital = samples.iter().all(|s| *s == 0);
        let quiet = digital
            || (energy <= 256. * self.rms_ceiling.powi(2)
                && score.is_finite()
                && score < 0.2
                && !periodic(&samples));
        if quiet {
            self.start.get_or_insert(self.offset);
        } else {
            self.close();
        }
        self.offset += FRAME;
    }
    fn finish(mut self) -> Vec<Range<usize>> {
        self.close();
        self.removed
    }
}

// A quiet periodic phoneme can fall below a learned detector's confidence.
// Protect it independently of level; false positives keep the source audio.
fn periodic(samples: &[i16; 256]) -> bool {
    let values: [f64; 128] = std::array::from_fn(|i| f64::from(samples[i * 2]));
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (20..=100).step_by(4).any(|lag| {
        let mut cross = 0.;
        let mut left = 0.;
        let mut right = 0.;
        for i in lag..values.len() {
            let a = values[i] - mean;
            let b = values[i - lag] - mean;
            cross += a * b;
            left += a * a;
            right += b * b;
        }
        left > 1. && right > 1. && cross > 0.90 * (left * right).sqrt()
    })
}

pub(crate) fn removed(pcm: &[u8], rms_ceiling: f64) -> Vec<Range<usize>> {
    if !pcm.len().is_multiple_of(2) || !rms_ceiling.is_finite() || rms_ceiling < 0. {
        return Vec::new();
    }
    let mut scanner = Scanner::new(rms_ceiling);
    for frame in pcm.chunks_exact(FRAME) {
        scanner.push(frame);
    }
    scanner.finish() // Partial final frames are always retained.
}

/// Owns only a disposable normalized copy, never the original or its durable sidecar.
pub(crate) struct PreparedCopy {
    pub audio: crate::providers::SavedAudio,
    temporary: Option<std::path::PathBuf>,
}
impl Drop for PreparedCopy {
    fn drop(&mut self) {
        if let Some(path) = &self.temporary {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(crate) async fn prepared_copy(
    audio: &crate::providers::SavedAudio,
) -> Result<PreparedCopy, crate::transcription::ApiError> {
    let owned = audio.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek, Write};
        let fail = || super::audio_missing();
        let source = owned.prepared.as_ref().ok_or_else(fail)?;
        let at = std::time::Instant::now();
        let mut input = std::fs::File::open(&source.path).map_err(|_| fail())?;
        if source.byte_length == 0
            || source.byte_length % 2 != 0
            || input.metadata().map_err(|_| fail())?.len() != source.byte_length
        {
            return Err(fail());
        }
        let mut scanner = Scanner::new(QUALIFIED_RMS);
        let mut frame = [0; FRAME];
        for _ in 0..source.byte_length / FRAME as u64 {
            input.read_exact(&mut frame).map_err(|_| fail())?;
            scanner.push(&frame);
        }
        let removed = scanner.finish();
        let after = source.byte_length - removed.iter().map(|r| r.len() as u64).sum::<u64>();
        if removed.is_empty() {
            record(
                owned.metadata.id,
                source.byte_length,
                after,
                at.elapsed().as_millis() as u64,
            );
            return Ok(PreparedCopy {
                audio: owned,
                temporary: None,
            });
        }
        let path = source
            .path
            .with_file_name(format!(".inference-{}.pcm", uuid::Uuid::new_v4()));
        let mut copy = PreparedCopy {
            audio: owned.clone(),
            temporary: Some(path.clone()),
        };
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| fail())?;
        let mut offset = 0;
        let mut buffer = [0; 64 * 1024];
        for range in removed.into_iter().chain(std::iter::once(
            source.byte_length as usize..source.byte_length as usize,
        )) {
            input
                .seek(std::io::SeekFrom::Start(offset as u64))
                .map_err(|_| fail())?;
            let mut remaining = range.start - offset;
            while remaining > 0 {
                let count = remaining.min(buffer.len());
                input.read_exact(&mut buffer[..count]).map_err(|_| fail())?;
                output.write_all(&buffer[..count]).map_err(|_| fail())?;
                remaining -= count;
            }
            offset = range.end;
        }
        output.flush().map_err(|_| fail())?;
        copy.audio.prepared = Some(crate::providers::PreparedAudio {
            path,
            byte_length: after,
        });
        record(
            owned.metadata.id,
            source.byte_length,
            after,
            at.elapsed().as_millis() as u64,
        );
        Ok(copy)
    })
    .await
    .map_err(|_| super::audio_missing())?
}

pub(crate) fn compact(pcm: &[u8], removed: &[Range<usize>]) -> Vec<u8> {
    let mut output = Vec::with_capacity(pcm.len() - removed.iter().map(Range::len).sum::<usize>());
    let mut offset = 0;
    for range in removed {
        output.extend_from_slice(&pcm[offset..range.start]);
        offset = range.end;
    }
    output.extend_from_slice(&pcm[offset..]);
    output
}

pub(crate) fn prepare(id: uuid::Uuid, pcm: &[u8]) -> Vec<u8> {
    let at = std::time::Instant::now();
    let removed = removed(pcm, QUALIFIED_RMS);
    let output = compact(pcm, &removed);
    record(
        id,
        pcm.len() as u64,
        output.len() as u64,
        at.elapsed().as_millis() as u64,
    );
    output
}

pub(crate) fn record(id: uuid::Uuid, before: u64, after: u64, elapsed: u64) {
    let measured = crate::transcription::AudioPreparation {
        input_bytes: before,
        output_bytes: after,
        input_ms: before / 32,
        output_ms: after / 32,
        removed_ms: before.saturating_sub(after) / 32,
        preparation_ms: elapsed,
    };
    if let Ok(mut values) = measurements().lock() {
        if values.len() >= 64 && !values.contains_key(&id) {
            values.clear();
        }
        values.insert(id, measured);
    }
    crate::latency::detail(id, "silence_input_bytes", before);
    crate::latency::detail(id, "silence_output_bytes", after);
    crate::latency::detail(id, "silence_input_ms", before / 32);
    crate::latency::detail(id, "silence_output_ms", after / 32);
    crate::latency::detail(id, "silence_removed_ms", before.saturating_sub(after) / 32);
    crate::latency::detail(id, "silence_prepare_ms", elapsed);
}

fn measurements() -> &'static std::sync::Mutex<
    std::collections::HashMap<uuid::Uuid, crate::transcription::AudioPreparation>,
> {
    static VALUES: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<uuid::Uuid, crate::transcription::AudioPreparation>,
        >,
    > = std::sync::OnceLock::new();
    VALUES.get_or_init(Default::default)
}
pub(crate) fn clear(id: uuid::Uuid) {
    let _ = take(id);
}
pub(crate) fn take(id: uuid::Uuid) -> Option<crate::transcription::AudioPreparation> {
    measurements().lock().ok()?.remove(&id)
}
pub(crate) fn wav(pcm: &[u8]) -> Vec<u8> {
    super::mixed::wav(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_short_pauses_and_offsets_and_does_not_modify_original() {
        let voice = super::super::speech::fixtures::voiced(32_000);
        let mut pcm = voice.clone();
        pcm.extend(vec![0; 2 * 32_000]);
        pcm.extend(&voice);
        assert!(removed(&pcm, 0.).is_empty());
        pcm.extend(vec![0; 20 * 32_000]);
        pcm.extend(&voice);
        let original = pcm.clone();
        let ranges = removed(&pcm, 0.);
        assert_eq!(ranges.len(), 1);
        let output = compact(&pcm, &ranges);
        assert_eq!(pcm, original);
        assert!(output[..6 * 32_000] == pcm[..6 * 32_000]);
        assert!(output.ends_with(&voice));
        assert!(ranges[0].start.is_multiple_of(2));
        assert!(ranges[0].end.is_multiple_of(2));
        assert!(output.len() < pcm.len());
    }
    #[test]
    fn quiet_speech_and_partial_frames_and_invalid_input_are_kept() {
        let pcm: Vec<u8> = (0..16000 * 8)
            .flat_map(|i| {
                (((i as f64 * 2. * std::f64::consts::PI * 180. / 16000.).sin() * 16.) as i16)
                    .to_le_bytes()
            })
            .collect();
        assert!(removed(&pcm, 32.).is_empty());
        let mut zeros = vec![0; 8 * 32_000];
        zeros.extend_from_slice(&5_i16.to_le_bytes());
        let output = compact(&zeros, &removed(&zeros, 0.));
        assert!(output.ends_with(&5_i16.to_le_bytes()));
        assert!(removed(&[0; 5], 64.).is_empty());
    }
    #[test]
    fn long_silence_fixture_fits_cloud_upload_without_a_duration_cutoff() {
        let mut pcm = super::super::speech::fixtures::voiced(60 * 16_000);
        pcm.extend(vec![0; 20 * 60 * 32_000]);
        pcm.extend(super::super::speech::fixtures::voiced(71 * 16_000));
        let output = compact(&pcm, &removed(&pcm, 0.));
        assert!(pcm.len() > 25_000_000);
        assert!(output.len() + 44 < 25_000_000);
        assert!((131 * 32_000..133 * 32_000).contains(&output.len()));
    }
    #[test]
    #[ignore = "Owner-supplied private microphone PCM; numeric measurements only"]
    fn microphone_measurement() {
        let pcm =
            std::fs::read(std::env::var("WRITER_SILENCE_PROOF_PCM").expect("private PCM path"))
                .unwrap();
        let ceiling = std::env::var("WRITER_SILENCE_PROOF_RMS")
            .unwrap_or("32".into())
            .parse()
            .unwrap();
        let mut detector = earshot::Detector::default();
        let mut counts = [0u64; 4];
        let mut quiet_scores = Vec::new();
        for frame in pcm.chunks_exact(FRAME) {
            let samples: [i16; 256] =
                std::array::from_fn(|i| i16::from_le_bytes([frame[i * 2], frame[i * 2 + 1]]));
            let energy = samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 256.;
            let score = detector.predict_i16(&samples);
            counts[0] += 1;
            if energy <= ceiling * ceiling {
                counts[1] += 1;
                quiet_scores.push(score);
                if score >= 0.2 {
                    counts[2] += 1;
                }
                if periodic(&samples) {
                    counts[3] += 1;
                }
            }
        }
        quiet_scores.sort_by(f32::total_cmp);
        if quiet_scores.is_empty() {
            quiet_scores.push(0.);
        }
        println!(
            "classification_frames={counts:?}, quiet_score_quantiles={:?}",
            [0.0, 0.1, 0.5, 0.9, 1.0]
                .map(|q| quiet_scores[((quiet_scores.len() - 1) as f64 * q) as usize])
        );
        let ranges = removed(&pcm, ceiling);
        println!(
            "{}",
            serde_json::json!({"input_ms":pcm.len()/32,"rms_ceiling":ceiling,"removed_ms":ranges.iter().map(Range::len).sum::<usize>()/32,"removed_ranges_ms":ranges.iter().map(|r|vec![r.start/32,r.end/32]).collect::<Vec<_>>() })
        );
    }
}
