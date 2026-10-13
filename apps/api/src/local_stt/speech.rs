//! Conservative speech endpoints. VAD only changes disposable inference windows;
//! the native journal retains every original frame. Uncertain input is decoded.
const FRAME: usize = 512; // 256 PCM16 samples / 16ms
const PAD: usize = 5_120; // 160ms
const QUIET: usize = 10_240; // 320ms

fn scores(pcm: &[u8]) -> impl Iterator<Item = (usize, f32)> + '_ {
    let mut detector = earshot::Detector::default();
    pcm.as_chunks::<FRAME>()
        .0
        .iter()
        .enumerate()
        .map(move |(i, bytes)| {
            let mut samples = [0i16; 256];
            for (s, b) in samples.iter_mut().zip(bytes.as_chunks::<2>().0.iter()) {
                *s = i16::from_le_bytes([b[0], b[1]]);
            }
            ((i + 1) * FRAME, detector.predict_i16(&samples))
        })
}

/// A conservative gate for a whole recognition window, not a pause detector.
/// Room tone is not all-zero PCM. Only suppress it when every 16 ms frame is
/// below -36 dBFS RMS AND Earshot finds no speech even at its 0.5 threshold.
/// Either louder input or a single possible speech frame keeps recognition on.
/// Include a padded partial frame: a short word/onset must not escape detection.
pub(super) fn speechless(pcm: &[u8]) -> bool {
    if !pcm.len().is_multiple_of(2) {
        return false;
    }
    if pcm.iter().all(|b| *b == 0) {
        return true;
    }
    let mut detector = earshot::Detector::default();
    for bytes in pcm.chunks(FRAME) {
        let mut samples = [0i16; 256];
        for (sample, b) in samples.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *sample = i16::from_le_bytes([b[0], b[1]]);
        }
        let count = bytes.len() / 2;
        let energy = samples[..count]
            .iter()
            .map(|s| f64::from(*s).powi(2))
            .sum::<f64>();
        if energy >= count as f64 * 512.0_f64.powi(2) {
            return false;
        }
        let score = detector.predict_i16(&samples);
        if !score.is_finite() || score >= 0.5 {
            return false;
        }
    }
    true
}

/// First stable phrase boundary. Low confidence and incomplete frames never
/// trigger a cut; a later growing prefix cannot move a published boundary.
pub(super) fn boundary(pcm: &[u8]) -> Option<usize> {
    let mut confident = 0;
    let mut last_activity = 0;
    for (end, score) in scores(pcm) {
        if score > 0.5 {
            confident += 1;
        }
        if score > 0.1 || !score.is_finite() {
            last_activity = end;
        }
        // Keep the established minimum linguistic context for committed phrases.
        // Shorter windows may be speculated, but cannot become a final prefix:
        // held-out Hebrew evaluation exposed errors at sub-two-second splits.
        if confident >= 4
            && last_activity + PAD >= 64_000
            && end.saturating_sub(last_activity) >= QUIET
        {
            return Some(last_activity + PAD);
        }
    }
    None
}

/// Exact digital silence needs no learned threshold and preserves stable cuts
/// for generated fixtures and devices that zero their inactive microphone input.
pub(super) fn zero_boundary(pcm: &[u8]) -> Option<usize> {
    let mut zero_start = None;
    for (i, frame) in pcm.as_chunks::<320>().0.iter().enumerate() {
        if frame.iter().all(|b| *b == 0) {
            let start = *zero_start.get_or_insert(i * 320);
            if start >= 64_000 && (i + 1) * 320 - start >= 9_600 {
                return Some(start + 3_200);
            }
        } else {
            zero_start = None;
        }
    }
    None
}

/// A pause boundary at or before a cap is published within this much further
/// audio: 320 ms of confirmed quiet after the 160 ms pad, or 300 ms of exact
/// zeros after its 100 ms. Waiting for it keeps a cap cut from overriding a
/// real phrase end that was about to settle.
pub(super) const SETTLED: usize = 6_400; // 200ms

/// Where to cut a window that reached its cap without a settled pause: the
/// quietest ~100 ms in it, so the boundary falls between words instead of
/// through whichever one is being spoken when the cap arrives. The span is
/// searched only after the window's first confident speech, so a silent lead-in
/// never becomes a window of its own, and never before the two seconds of
/// context a committed phrase needs. The latest of equally quiet spans wins,
/// which keeps windows long; the same bytes always give the same cut, so a
/// growing prefix cannot move it. A window with no confident speech has no such
/// point and closes at the cap.
pub(super) fn quietest(pcm: &[u8], limit: usize) -> Option<usize> {
    const SPAN: usize = 6; // ~96 ms, longer than a stop closure inside a word
    const HALF: usize = SPAN / 2;
    let first = scores(&pcm[..limit.min(pcm.len())]).position(|(_, score)| score > 0.5)?;
    let from = ((first + 1) * FRAME + PAD).max(64_000);
    let energy: Vec<f64> = pcm[..(limit + HALF * FRAME).min(pcm.len())]
        .as_chunks::<FRAME>()
        .0
        .iter()
        .map(|frame| {
            frame
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| f64::from(i16::from_le_bytes([s[0], s[1]])).powi(2))
                .sum()
        })
        .collect();
    let mut quietest: Option<(f64, usize)> = None;
    for frame in (from / FRAME)..=(limit / FRAME) {
        if frame < HALF || frame + HALF > energy.len() {
            continue;
        }
        let level: f64 = energy[frame - HALF..frame + HALF].iter().sum();
        if quietest.is_none_or(|(lowest, _)| level <= lowest) {
            quietest = Some((level, frame * FRAME));
        }
    }
    quietest.map(|(_, cut)| cut)
}

/// Re-evaluate with preceding acoustic context, so an arbitrary low-volume
/// tail is never dropped merely because the detector started cold in silence.
pub(super) fn trailing_quiet(context: &[u8], tail_start: usize) -> bool {
    if context[tail_start..].iter().all(|b| *b == 0) {
        return true;
    }
    let mut confident = 0;
    let mut last_activity = 0;
    for (end, score) in scores(context) {
        if score > 0.5 {
            confident += 1;
        }
        if score > 0.1 || !score.is_finite() {
            last_activity = end;
        }
    }
    confident >= 4
        && last_activity + PAD <= tail_start
        && context.len().saturating_sub(last_activity) >= QUIET
        && partial_frame_quiet(context)
}

/// A partial trailing frame (under 16 ms) cannot carry a word, but audible
/// energy in it is a possible onset and still waits for final decoding. Real
/// microphone noise is far below this -40 dBFS bound; a plosive is far above it.
fn partial_frame_quiet(context: &[u8]) -> bool {
    let remainder = context.len() % FRAME;
    context[context.len() - remainder..]
        .as_chunks::<2>()
        .0
        .iter()
        .all(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() < 328)
}

/// Deterministic 16 kHz signals for tests that need the detector's real
/// judgement: a synthetic tone is not speech to it, and a microphone's pause is
/// not digital silence.
#[cfg(test)]
pub(super) mod fixtures {
    /// Voiced-like audio Earshot classifies as speech: a 120 Hz harmonic stack
    /// with a formant-shaped tilt, opened and closed by 4 Hz syllables.
    pub fn voiced(samples: usize) -> Vec<u8> {
        (0..samples)
            .flat_map(|i| {
                let t = i as f64 / 16_000.0;
                let harmonics: f64 = (1..=30)
                    .map(|k| {
                        let f = 120.0 * f64::from(k);
                        let tilt = (-((f - 700.0) / 500.0).powi(2)).exp()
                            + 0.6 * (-((f - 1_200.0) / 600.0).powi(2)).exp();
                        tilt * (std::f64::consts::TAU * f * t).sin()
                    })
                    .sum();
                let syllable = 0.5 - 0.5 * (std::f64::consts::TAU * 4.0 * t).cos();
                let sample = harmonics * syllable * 0.1 * 32_768.0 / 3.0;
                (sample.clamp(-32_768.0, 32_767.0) as i16).to_le_bytes()
            })
            .collect()
    }

    /// Room tone near -60 dBFS: far below speech, never digital silence, and
    /// the level a real microphone's pauses actually sit at.
    pub fn room(samples: usize) -> Vec<u8> {
        let mut seed = 7u32;
        (0..samples)
            .flat_map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
                ((noise * 57.0) as i16).to_le_bytes()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_window_gate_rejects_room_tone_but_keeps_short_and_soft_speech() {
        assert!(speechless(&[]));
        for samples in [80, 3_200, 16_000, 240_000] {
            assert!(speechless(&vec![0; samples * 2]));
            assert!(speechless(&fixtures::room(samples)));
        }
        assert!(!speechless(&[0])); // malformed PCM is not evidence of silence
        assert!(!speechless(&fixtures::voiced(3_200)));
        let soft: Vec<u8> = fixtures::voiced(16_000)
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|b| (i16::from_le_bytes(*b) / 20).to_le_bytes())
            .collect();
        assert!(
            !speechless(&soft),
            "possible soft speech must still be decoded"
        );
        // Loud unknown input and a partial-frame onset remain uncertain.
        let loud: Vec<u8> = (0..256).flat_map(|_| 10_000i16.to_le_bytes()).collect();
        assert!(!speechless(&loud));
        let mut onset = fixtures::room(256);
        onset.extend(4_000i16.to_le_bytes());
        assert!(!speechless(&onset));
    }
    #[test]
    fn silence_can_be_skipped_but_unclassified_soft_input_cannot() {
        assert!(trailing_quiet(&vec![0; 32_000], 16_000));
        let soft: Vec<_> = (0..16_001)
            .flat_map(|i| (if i % 2 == 0 { 10i16 } else { -10i16 }).to_le_bytes())
            .collect();
        assert!(!trailing_quiet(&soft, 16_000));
        assert_eq!(boundary(&soft), None);
    }
    #[test]
    fn faint_noise_in_a_partial_last_frame_is_quiet_but_an_onset_is_not() {
        let whole = vec![0; FRAME * 3];
        assert!(partial_frame_quiet(&whole));
        let mut partial = whole.clone();
        partial.extend(3i16.to_le_bytes());
        partial.extend((-200i16).to_le_bytes());
        assert!(partial_frame_quiet(&partial));
        partial.extend(4000i16.to_le_bytes());
        assert!(!partial_frame_quiet(&partial));
        // Exact silence after the tail start never needs the detector at all.
        let mut pcm: Vec<u8> = (0..16000)
            .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
            .collect();
        let tail_start = pcm.len();
        pcm.extend(vec![0; 16_386]);
        assert!(trailing_quiet(&pcm, tail_start));
    }
    #[test]
    fn room_tone_is_speechless_but_still_not_a_committed_pause() {
        assert!(
            scores(&fixtures::voiced(16_000))
                .filter(|(_, score)| *score > 0.5)
                .count()
                >= 4
        );
        assert_eq!(
            scores(&fixtures::room(16_000))
                .filter(|(_, score)| *score > 0.5)
                .count(),
            0
        );
        // The conservative activity threshold still reads room tone as possible
        // voice, so a stop in talking does not close a phrase by itself.
        let spoken = [
            fixtures::voiced(3 * 16_000),
            fixtures::room(16_000),
            fixtures::voiced(16_000),
        ]
        .concat();
        assert_eq!(boundary(&spoken), None);
    }
    #[test]
    fn the_quietest_span_is_the_gap_and_needs_confident_speech_first() {
        let mut pcm = fixtures::voiced(5 * 16_000);
        pcm.extend(fixtures::room(6_400)); // 0.4 s between phrases
        pcm.extend(fixtures::voiced(4 * 16_000));
        let cut = quietest(&pcm, 8 * 32_000).expect("voiced audio has a quietest span");
        assert!(
            (5 * 32_000..5 * 32_000 + 12_800).contains(&cut),
            "cut at {cut} bytes is outside the 5.0-5.4 s gap"
        );
        assert_eq!(quietest(&fixtures::room(10 * 16_000), 8 * 32_000), None);
    }
}
