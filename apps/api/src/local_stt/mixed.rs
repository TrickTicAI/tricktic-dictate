//! Mixed speech must not inherit one detected language for an entire recording.
//! Split normalized 16 kHz mono PCM at quiet boundaries, without losing/overlapping
//! samples. Each request gets fresh language detection and no previous text prompt.
use std::ops::Range;

const FRAME: usize = 160 * 2; // 10 ms of signed 16-bit PCM
const SECOND: usize = 16_000 * 2;
const WINDOW: usize = 30 * SECOND; // Whisper's acoustic context, not a recording limit

pub(super) fn ranges(pcm: &[u8]) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut quiet = None;
    for (frame, bytes) in pcm.chunks(FRAME).enumerate() {
        let position = frame * FRAME;
        let power = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| f64::from(i16::from_le_bytes([s[0], s[1]])).powi(2))
            .sum::<f64>()
            / (bytes.len() / 2) as f64;
        if power <= 32768_f64.powi(2) * 0.001 {
            // -30 dBFS RMS
            quiet.get_or_insert(position);
        } else if let Some(begin) = quiet.take() {
            let cut = ((begin + position) / 4) * 2;
            if position - begin >= 20 * FRAME
                && cut >= start + 2 * SECOND
                && pcm.len() - cut >= SECOND
            {
                result.push(start..cut);
                start = cut;
            }
        }
        if position >= start + WINDOW {
            result.push(start..position);
            start = position;
            quiet = None;
        }
    }
    if start < pcm.len() {
        result.push(start..pcm.len());
    }
    result
}

/// Capture-time segmentation: close a phrase *during* a pause, at a fixed
/// boundary that cannot move when more audio arrives. Continuous speech closes
/// at eight seconds so ordinary dictation can start inference before release.
/// The batch-only mixed-language splitter retains its original context policy.
pub(super) fn live_ranges(pcm: &[u8]) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut quiet = None;
    for (frame, bytes) in pcm.as_chunks::<FRAME>().0.iter().enumerate() {
        let position = frame * FRAME;
        let power = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| f64::from(i16::from_le_bytes([s[0], s[1]])).powi(2))
            .sum::<f64>()
            / 160.;
        if power <= 32768_f64.powi(2) * 0.001 {
            let begin = *quiet.get_or_insert(position);
            let cut = begin + 10 * FRAME;
            if position + FRAME - begin >= 30 * FRAME && cut >= start + 2 * SECOND {
                result.push(start..cut);
                start = cut;
                // Do not repeatedly split a single sustained silence.
            }
        } else {
            quiet = None;
        }
        if position >= start + 8 * SECOND {
            result.push(start..position);
            start = position;
            quiet = None;
        }
    }
    if start < pcm.len() {
        result.push(start..pcm.len());
    }
    result
}

pub(super) fn wav(pcm: &[u8]) -> Vec<u8> {
    let size = u32::try_from(pcm.len()).expect("bounded PCM window");
    let mut wav = Vec::with_capacity(pcm.len() + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(size + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    wav.extend_from_slice(&16000_u32.to_le_bytes());
    wav.extend_from_slice(&32000_u32.to_le_bytes());
    wav.extend_from_slice(b"\x02\0\x10\0data");
    wav.extend_from_slice(&size.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(seconds: usize) -> Vec<u8> {
        (0..seconds * 16000)
            .flat_map(|i| (if i % 2 == 0 { 12000_i16 } else { -12000_i16 }).to_le_bytes())
            .collect()
    }
    #[test]
    fn live_boundaries_never_move_as_pauses_grow_or_speech_resumes() {
        let mut audio = tone(3);
        audio.extend(vec![0; SECOND * 2]);
        audio.extend(tone(14));
        let final_ranges = live_ranges(&audio);
        let mut seen = Vec::new();
        for end in (FRAME..=audio.len()).step_by(FRAME * 7) {
            let ranges = live_ranges(&audio[..end]);
            let closed = &ranges[..ranges.len().saturating_sub(1)];
            assert!(closed.starts_with(&seen));
            assert!(final_ranges.starts_with(closed));
            seen = closed.to_vec();
        }
        let restored: Vec<_> = final_ranges
            .iter()
            .flat_map(|r| audio[r.clone()].iter().copied())
            .collect();
        assert_eq!(restored, audio);
        assert_eq!(final_ranges[0].end, 3 * SECOND + SECOND / 10);
    }
    #[test]
    fn continuous_live_speech_is_available_after_eight_seconds() {
        let audio = tone(20);
        assert_eq!(
            live_ranges(&audio),
            [
                0..8 * SECOND,
                8 * SECOND..16 * SECOND,
                16 * SECOND..20 * SECOND
            ]
        );
    }
    #[test]
    fn quiet_switch_preserves_every_sample_once() {
        let mut audio = tone(2);
        audio.extend(vec![0; SECOND / 2]);
        audio.extend(tone(4));
        let parts = ranges(&audio);
        assert_eq!(
            parts,
            [
                0..(2 * SECOND + SECOND / 4),
                (2 * SECOND + SECOND / 4)..audio.len()
            ]
        );
        let joined: Vec<_> = parts
            .iter()
            .flat_map(|r| audio[r.clone()].iter().copied())
            .collect();
        assert_eq!(joined, audio);
    }
    #[test]
    fn short_pauses_and_edges_do_not_make_tiny_requests() {
        let mut audio = vec![0; SECOND / 2];
        audio.extend(tone(1));
        audio.extend(vec![0; SECOND / 10]);
        audio.extend(tone(1));
        audio.extend(vec![0; SECOND]);
        let parts = ranges(&audio);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], 0..audio.len());
    }
    #[test]
    fn long_unbroken_audio_is_not_truncated() {
        let audio = tone(65);
        let parts = ranges(&audio);
        assert_eq!(
            parts,
            [
                0..30 * SECOND,
                30 * SECOND..60 * SECOND,
                60 * SECOND..65 * SECOND
            ]
        );
        let data = wav(&audio[parts[0].clone()]);
        assert_eq!(&data[44..], &audio[..30 * SECOND]);
        assert_eq!(
            u32::from_le_bytes(data[40..44].try_into().unwrap()) as usize,
            30 * SECOND
        );
    }
}
