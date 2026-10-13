//! Offline host preparation using Writer's existing audio resampler.
//! Run with private input/output WAV paths; never overwrites the source or output.
#[allow(dead_code)]
#[path = "../../../crates/dictation-audio/src/lib.rs"]
mod dictation_audio;
use std::fs::{self, OpenOptions};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("Usage: normalize INPUT.wav OUTPUT.wav (private paths outside Git)".into());
    }
    let input = fs::read(&args[1])?;
    let source = dictation_audio::wav_pcm16(&input)?;
    let mut resampler = dictation_audio::Resampler::new(source.rate, source.channels)?;
    let mut output = Vec::new();
    for chunk in source.data.chunks(65536) {
        let samples: Vec<i16> = chunk
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]))
            .collect();
        resampler.push(&samples, &mut output)?;
    }
    resampler.finish(&mut output)?;
    let bytes = u32::try_from(output.len().checked_mul(2).ok_or("WAV too large")?)?;
    let riff_size = bytes.checked_add(36).ok_or("RIFF too large")?;
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[2])?;
    out.write_all(b"RIFF")?;
    out.write_all(&riff_size.to_le_bytes())?;
    out.write_all(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00")?;
    out.write_all(&16000_u32.to_le_bytes())?;
    out.write_all(&32000_u32.to_le_bytes())?;
    out.write_all(b"\x02\x00\x10\x00data")?;
    out.write_all(&bytes.to_le_bytes())?;
    for sample in &output {
        out.write_all(&sample.to_le_bytes())?;
    }
    out.sync_all()?;
    println!(
        "Converted {} Hz / {} channels to 16000 Hz mono; {} samples",
        source.rate,
        source.channels,
        output.len()
    );
    Ok(())
}
