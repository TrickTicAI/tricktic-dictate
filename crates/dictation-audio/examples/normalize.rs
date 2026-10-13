use writer_dictation_audio::{wav_pcm16, Resampler};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let wav = wav_pcm16(&bytes).unwrap();
    let input: Vec<_> = wav
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let mut times = Vec::new();
    let mut normalized = Vec::new();
    for _ in 0..20 {
        let start = std::time::Instant::now();
        let mut converter = Resampler::new(wav.rate, wav.channels).unwrap();
        normalized.clear();
        for chunk in input.chunks(1024) {
            converter.push(chunk, &mut normalized).unwrap();
        }
        converter.finish(&mut normalized).unwrap();
        times.push(start.elapsed().as_micros());
    }
    let data: Vec<_> = normalized.iter().flat_map(|s| s.to_le_bytes()).collect();
    let len = data.len() as u32;
    let mut output = b"RIFF".to_vec();
    output.extend((len + 36).to_le_bytes());
    output.extend(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    output.extend(16000u32.to_le_bytes());
    output.extend(32000u32.to_le_bytes());
    output.extend(b"\x02\0\x10\0data");
    output.extend(len.to_le_bytes());
    output.extend(data);
    std::fs::write(&args[2], output).unwrap();
    times.sort_unstable();
    println!(
        "frames={} median_us={} max_us={}",
        normalized.len(),
        times[10],
        times[19]
    );
}
