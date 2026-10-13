//! Deterministic streaming normalization for dictation. No devices, files, model,
//! timers or persistence: callers can journal original audio independently.
use std::collections::VecDeque;
const OUTPUT_RATE: u64 = 16_000;

pub struct Resampler {
    rate: u64,
    channels: usize,
    phases: Vec<Vec<f64>>,
    radius: usize,
    samples: VecDeque<f64>,
    base: u64,
    frames: u64,
    emitted: u64,
    partial: Vec<i16>,
    finished: bool,
}
impl Resampler {
    pub fn new(rate: u32, channels: u16) -> Result<Self, &'static str> {
        if !(8_000..=384_000).contains(&rate) || !(1..=32).contains(&channels) {
            return Err("unsupported sample rate or channel count");
        }
        fn gcd(mut a: u64, mut b: u64) -> u64 {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        }
        let rate = u64::from(rate);
        let phase_step = gcd(rate, OUTPUT_RATE);
        let radius = 32 * rate.div_ceil(48_000) as usize;
        let cutoff = 0.45 * (OUTPUT_RATE as f64 / rate as f64).min(1.0);
        // Standard device rates use exact phases; cap coefficient memory for odd rates.
        let phase_count = (OUTPUT_RATE / phase_step).min(256);
        let phases = (0..phase_count)
            .map(|phase| {
                let fraction = phase as f64 / phase_count as f64;
                let mut weights: Vec<f64> = (-(radius as i32)..=radius as i32)
                    .map(|offset| {
                        let x = offset as f64 - fraction;
                        if x.abs() >= radius as f64 {
                            return 0.;
                        }
                        let sinc = if x.abs() < 1e-12 {
                            2. * cutoff
                        } else {
                            (2. * std::f64::consts::PI * cutoff * x).sin()
                                / (std::f64::consts::PI * x)
                        };
                        let window = 0.42
                            + 0.5 * (std::f64::consts::PI * x / radius as f64).cos()
                            + 0.08 * (2. * std::f64::consts::PI * x / radius as f64).cos();
                        sinc * window
                    })
                    .collect();
                let total: f64 = weights.iter().sum();
                for value in &mut weights {
                    *value /= total;
                }
                weights
            })
            .collect();
        Ok(Self {
            rate,
            channels: channels as usize,
            phases,
            radius,
            samples: VecDeque::new(),
            base: 0,
            frames: 0,
            emitted: 0,
            partial: Vec::new(),
            finished: false,
        })
    }
    /// Interleaved i16 input. Callback boundaries need not align to sample frames.
    pub fn push(&mut self, input: &[i16], output: &mut Vec<i16>) -> Result<(), &'static str> {
        if self.finished {
            return Err("resampler already finalized");
        }
        for sample in input {
            self.partial.push(*sample);
            if self.partial.len() == self.channels {
                let mono =
                    self.partial.iter().map(|s| f64::from(*s)).sum::<f64>() / self.channels as f64;
                self.partial.clear();
                self.frames += 1;
                if self.rate == OUTPUT_RATE {
                    output.push(mono.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
                    self.emitted += 1;
                    continue;
                }
                self.samples.push_back(mono);
                self.emit(false, output);
            }
        }
        Ok(())
    }
    /// Flush the filter's bounded lookahead without changing output duration.
    pub fn finish(&mut self, output: &mut Vec<i16>) -> Result<(), &'static str> {
        if self.finished {
            return Ok(());
        }
        if !self.partial.is_empty() {
            return Err("incomplete channel frame");
        }
        self.emit(true, output);
        self.finished = true;
        Ok(())
    }
    fn emit(&mut self, finalizing: bool, output: &mut Vec<i16>) {
        let target = self.frames * OUTPUT_RATE / self.rate;
        while self.emitted < target {
            let numerator = self.emitted * self.rate;
            let center = numerator / OUTPUT_RATE;
            if !finalizing && center + self.radius as u64 >= self.frames {
                break;
            }
            let value = if self.rate == OUTPUT_RATE {
                self.samples[(center - self.base) as usize]
            } else {
                let weights = &self.phases
                    [((numerator % OUTPUT_RATE) * self.phases.len() as u64 / OUTPUT_RATE) as usize];
                weights
                    .iter()
                    .enumerate()
                    .map(|(index, weight)| {
                        let position = center as i64 + index as i64 - self.radius as i64;
                        if position < 0 || position as u64 >= self.frames {
                            0.
                        } else {
                            self.samples[(position as u64 - self.base) as usize] * weight
                        }
                    })
                    .sum()
            };
            output.push(value.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
            self.emitted += 1;
            let retain =
                (self.emitted * self.rate / OUTPUT_RATE).saturating_sub(self.radius as u64);
            while self.base < retain && !self.samples.is_empty() {
                self.samples.pop_front();
                self.base += 1;
            }
        }
    }
    pub fn retained_frames(&self) -> usize {
        self.samples.len()
    }
}

pub struct Pcm16<'a> {
    pub rate: u32,
    pub channels: u16,
    pub data: &'a [u8],
}
/// Locate PCM within real RIFF/RF64 headers, including native journals' JUNK/ds64.
/// Unsupported codecs return an error so callers can retain their existing decoder.
pub fn wav_pcm16(bytes: &[u8]) -> Result<Pcm16<'_>, &'static str> {
    if bytes.len() < 12 || !matches!(&bytes[..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return Err("not WAV");
    }
    let u32_at = |p: usize| -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(
            bytes
                .get(p..p + 4)
                .ok_or("truncated WAV")?
                .try_into()
                .unwrap(),
        ))
    };
    let mut cursor = 12usize;
    let mut format = None;
    let mut rf64_length = None;
    while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let kind = &bytes[cursor..cursor + 4];
        let declared = u32_at(cursor + 4)?;
        let length = if kind == b"data" && declared == u32::MAX {
            rf64_length.ok_or("RF64 missing ds64")?
        } else {
            declared as usize
        };
        let start = cursor + 8;
        let end = start.checked_add(length).ok_or("WAV size overflow")?;
        let data = bytes.get(start..end).ok_or("truncated WAV chunk")?;
        if kind == b"ds64" && data.len() >= 28 {
            rf64_length = Some(
                usize::try_from(u64::from_le_bytes(data[8..16].try_into().unwrap()))
                    .map_err(|_| "RF64 too large")?,
            );
        } else if kind == b"fmt " && data.len() >= 16 {
            let field = |at| u16::from_le_bytes(data[at..at + 2].try_into().unwrap());
            let rate = u32::from_le_bytes(data[4..8].try_into().unwrap());
            let channels = field(2);
            if field(0) != 1
                || field(14) != 16
                || channels == 0
                || channels > 32
                || field(12) != channels * 2
                || !(8000..=384000).contains(&rate)
            {
                return Err("unsupported WAV format");
            }
            format = Some((rate, channels));
        } else if kind == b"data" {
            let (rate, channels) = format.ok_or("WAV missing format")?;
            if data.len() % (channels as usize * 2) != 0 {
                return Err("incomplete WAV frame");
            }
            return Ok(Pcm16 {
                rate,
                channels,
                data,
            });
        }
        cursor = end.checked_add(length % 2).ok_or("WAV size overflow")?;
    }
    Err("WAV missing audio")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn normalize(rate: u32, input: &[i16], chunk: usize) -> Vec<i16> {
        let mut r = Resampler::new(rate, 2).unwrap();
        let mut out = Vec::new();
        for block in input.chunks(chunk) {
            r.push(block, &mut out).unwrap();
            assert!(r.retained_frames() < 2 * r.radius + 4);
        }
        r.finish(&mut out).unwrap();
        out
    }
    #[test]
    fn riff_and_rf64_native_journals_find_pcm_without_a_fixed_header_size() {
        for rf64 in [false, true] {
            let mut wav = if rf64 {
                b"RF64".to_vec()
            } else {
                b"RIFF".to_vec()
            };
            wav.extend(76u32.to_le_bytes());
            wav.extend(b"WAVE");
            wav.extend(if rf64 { b"ds64" } else { b"JUNK" });
            wav.extend(28u32.to_le_bytes());
            wav.extend(76u64.to_le_bytes());
            wav.extend(4u64.to_le_bytes());
            wav.extend(2u64.to_le_bytes());
            wav.extend(0u32.to_le_bytes());
            wav.extend(b"fmt ");
            wav.extend(16u32.to_le_bytes());
            wav.extend(1u16.to_le_bytes());
            wav.extend(1u16.to_le_bytes());
            wav.extend(16000u32.to_le_bytes());
            wav.extend(32000u32.to_le_bytes());
            wav.extend(2u16.to_le_bytes());
            wav.extend(16u16.to_le_bytes());
            wav.extend(b"data");
            wav.extend(if rf64 { u32::MAX } else { 4 }.to_le_bytes());
            wav.extend([1, 0, 2, 0]);
            let pcm = wav_pcm16(&wav).unwrap();
            assert_eq!((pcm.rate, pcm.channels), (16000, 1));
            assert_eq!(pcm.data, [1, 0, 2, 0]);
            wav.pop();
            assert!(wav_pcm16(&wav).is_err());
        }
    }
    #[test]
    fn malformed_wav_sizes_and_non_pcm_are_rejected() {
        let mut wav = b"RIFF\0\0\0\0WAVEJUNK".to_vec();
        wav.extend(u32::MAX.to_le_bytes());
        assert!(wav_pcm16(&wav).is_err());
        assert!(wav_pcm16(b"not audio").is_err());
    }
    #[test]
    fn callback_boundaries_do_not_change_audio_or_final_sample_count() {
        for rate in [16000, 44100, 47999, 48000, 96000] {
            let samples: Vec<i16> = (0..rate * 2)
                .map(|i| ((i as f64 * 0.13).sin() * 12000.) as i16)
                .collect();
            let whole = normalize(rate, &samples, samples.len());
            assert_eq!(whole.len(), 16000);
            for size in [1, 79, 1024] {
                assert_eq!(normalize(rate, &samples, size), whole);
            }
        }
    }
    #[test]
    fn downsampling_rejects_aliases_and_preserves_speech_band() {
        fn rms(frequency: f64) -> f64 {
            let input: Vec<i16> = (0..48000)
                .flat_map(|n| {
                    let s = (12000.
                        * (2. * std::f64::consts::PI * frequency * n as f64 / 48000.).sin())
                        as i16;
                    [s, s]
                })
                .collect();
            let out = normalize(48000, &input, 1024);
            (out[100..15900]
                .iter()
                .map(|s| f64::from(*s).powi(2))
                .sum::<f64>()
                / 15800.)
                .sqrt()
        }
        let speech = rms(1000.);
        assert!((speech - 8485.).abs() < 100.);
        assert!(rms(12000.) / speech < 0.01);
    }
    #[test]
    fn subsecond_audio_flush_is_exact_and_finalization_is_idempotent() {
        let mut r = Resampler::new(48000, 1).unwrap();
        let mut out = Vec::new();
        r.push(&[1000; 12000], &mut out).unwrap();
        r.finish(&mut out).unwrap();
        assert_eq!(out.len(), 4000);
        r.finish(&mut out).unwrap();
        assert_eq!(out.len(), 4000);
        assert!(r.push(&[1], &mut out).is_err());
    }
    #[test]
    fn incomplete_channel_frame_is_not_silently_dropped() {
        let mut r = Resampler::new(48000, 2).unwrap();
        let mut out = Vec::new();
        r.push(&[1], &mut out).unwrap();
        assert!(r.finish(&mut out).is_err());
        r.push(&[1], &mut out).unwrap();
        assert!(r.finish(&mut out).is_ok());
    }
}
