//! Where live recognition would cut saved recordings, and how much speech is
//! still undecoded when the speaker stops. Needs private audio that never
//! enters this repository; reports durations and counts only, never text.
//! `WRITER_REPLAY_PCM_DIR=<dir with <id>/prepared.pcm> cargo test -p
//! metrix-writer-api live_cut_replay -- --ignored --nocapture`
use super::{prepared::window_ranges_within, speech};

const BYTES_PER_SECOND: f64 = 32_000.0;

/// Seconds of audio in the window still open at the end of the recording:
/// what the final pass decodes after Stop unless a speculative decode of it
/// can be reused.
fn open_window(pcm: &[u8], limit: usize) -> (usize, f64) {
    let mut offset = 0;
    let mut windows = 0;
    loop {
        let read = &pcm[offset..(offset + limit + 12_800).min(pcm.len())];
        let ranges = window_ranges_within(read, limit);
        let Some(range) = ranges.first() else {
            return (windows, 0.0);
        };
        windows += 1;
        if ranges.len() < 2 || offset + range.end >= pcm.len() {
            return (windows, (pcm.len() - offset) as f64 / BYTES_PER_SECOND);
        }
        offset += range.end;
    }
}

fn quantile(values: &mut [f64], q: f64) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    values[((values.len() - 1) as f64 * q).round() as usize]
}

#[test]
#[ignore = "needs WRITER_REPLAY_PCM_DIR with private prepared recordings"]
fn live_cut_replay() {
    let dir = std::env::var("WRITER_REPLAY_PCM_DIR").expect("WRITER_REPLAY_PCM_DIR");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("readable directory")
        .filter_map(|entry| Some(entry.ok()?.path().join("prepared.pcm")))
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    let recordings: Vec<Vec<u8>> = files
        .iter()
        .map(|path| std::fs::read(path).expect("readable prepared audio"))
        .filter(|pcm| pcm.len() >= 10 * 32_000)
        .collect();
    println!("{} recording(s) of 10 s or longer", recordings.len());
    for seconds in [14usize, 30, 60, 300] {
        let limit = seconds * 32_000;
        let (mut open, mut long_open) = (vec![], vec![]);
        for pcm in &recordings {
            let (_, tail) = open_window(pcm, limit);
            open.push(tail);
            if pcm.len() as f64 / BYTES_PER_SECOND >= 45.0 {
                long_open.push(tail);
            }
        }
        let long = if long_open.is_empty() {
            String::new()
        } else {
            format!(
                "; recordings >= 45 s (n={}): median {:.1} s, p90 {:.1} s, max {:.1} s",
                long_open.len(),
                quantile(&mut long_open.clone(), 0.5),
                quantile(&mut long_open.clone(), 0.9),
                quantile(&mut long_open, 1.0)
            )
        };
        println!(
            "limit {seconds:>3} s: open window at stop median {:.1} s, p90 {:.1} s, max {:.1} s{long}",
            quantile(&mut open.clone(), 0.5),
            quantile(&mut open.clone(), 0.9),
            quantile(&mut open, 1.0),
        );
    }
    // Whether the last 0.3/0.6/1.0 s of each recording would count as a quiet
    // tail, so the speculative decode before it could be reused at Stop.
    for tail_ms in [300usize, 600, 1_000] {
        let tail = tail_ms * 32;
        let mut quiet = 0;
        for pcm in &recordings {
            let start = pcm.len().saturating_sub(tail + 32_768);
            let context = &pcm[start..];
            let at = context.len() - tail;
            if speech::trailing_quiet(context, at) {
                quiet += 1;
            }
        }
        println!("last {tail_ms} ms quiet: {quiet}/{n}", n = recordings.len());
    }
}
