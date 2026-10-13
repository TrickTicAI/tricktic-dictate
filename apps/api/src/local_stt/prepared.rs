//! Bounded reads of the native journal's committed normalized prefix.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Longest live window decoded in one pass: 14 s of 16 kHz PCM16. With the 0.4 s
/// boundary lookahead it fits the qualified 768-position (15.36 s) encoder
/// context, so ordinary continuous dictation is one capped pass rather than an
/// 8 s phrase plus a tail. Eight seconds was the earlier cap under full context.
pub(super) const LIVE_WINDOW: usize = 14 * 32_000;
const LOOKAHEAD: usize = 12_800;
const _: () = assert!(LIVE_WINDOW + LOOKAHEAD <= super::SHORT_WINDOW_BYTES);
/// Acoustic context read before a tail when deciding whether it is quiet.
const LOOKBACK: u64 = 32_768; // one second, aligned VAD frames

#[derive(Clone)]
struct Prefix {
    offset: u64,
    text: String,
    digest: Sha256,
    at: Instant,
    /// Newest speculative decode of the open window that starts at `offset`: its
    /// absolute end and segment key. The final pass reuses it only when every
    /// frame after that end is quiet, so releasing after a pause costs no pass.
    provisional: Option<(u64, [u8; 32])>,
}
#[derive(Default)]
pub(super) struct Cache {
    prefixes: HashMap<[u8; 32], Prefix>,
}
impl Cache {
    fn get(&mut self, key: &[u8; 32]) -> Option<Prefix> {
        self.prefixes
            .retain(|_, p| p.at.elapsed() < Duration::from_secs(600));
        self.prefixes.get(key).cloned()
    }
    fn put(&mut self, key: [u8; 32], prefix: Prefix) {
        if self.prefixes.len() >= 16 && !self.prefixes.contains_key(&key) {
            self.prefixes.clear();
        }
        self.prefixes.insert(key, prefix);
    }
}

fn window_ranges(pcm: &[u8]) -> Vec<std::ops::Range<usize>> {
    // RMS alone mistakes quiet phonemes for pauses. Only a VAD-confirmed pause
    // (or exact digital silence) closes a phrase before the bounded live window.
    // Reaching that bound is not itself a phrase end, so cut where the window is
    // quietest instead of through the word being spoken when it arrives.
    let boundary = speech::boundary(pcm)
        .or_else(|| speech::zero_boundary(pcm))
        .filter(|end| *end <= LIVE_WINDOW)
        .or_else(|| {
            (pcm.len() >= LIVE_WINDOW + speech::SETTLED)
                .then(|| speech::quietest(pcm, LIVE_WINDOW).unwrap_or(LIVE_WINDOW))
        });
    if let Some(end) = boundary {
        vec![0..end, end..pcm.len()]
    } else {
        std::iter::once(0..pcm.len()).collect()
    }
}

/// Whether the prepared audio after `end` carries no further speech, judged with
/// one second of preceding context so the detector never starts cold.
async fn quiet_after(file: &mut tokio::fs::File, end: u64, total: u64) -> bool {
    if end >= total {
        return end == total;
    }
    let lookback = end.min(LOOKBACK);
    let mut context = vec![0; (lookback + total - end) as usize];
    if file
        .seek(std::io::SeekFrom::Start(end - lookback))
        .await
        .is_err()
        || file.read_exact(&mut context).await.is_err()
    {
        return false;
    }
    speech::trailing_quiet(&context, lookback as usize)
}

impl LocalProvider {
    /// Whether the speculative request on the worker will serve the final pass:
    /// it starts at the committed offset and either is exactly the final first
    /// window (a closed phrase) or is followed only by quiet frames. The durable
    /// path still verifies the prefix bytes before any reuse.
    pub(super) async fn retainable(
        &self,
        c: &LocalConfig,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> bool {
        let Some(active) = self.active_preview.lock().unwrap().clone() else {
            return false;
        };
        let Some((start, end)) = active
            .window
            .filter(|_| active.recording == audio.metadata.id)
        else {
            return false;
        };
        let Some(prepared) = audio.prepared.as_ref() else {
            return false;
        };
        let identity = segment_cache::key(audio.metadata.id, c, options, &[]);
        let offset = self
            .prepared_segments
            .lock()
            .unwrap()
            .get(&identity)
            .map_or(0, |p| p.offset);
        crate::latency::detail(
            audio.metadata.id,
            "preview_tail_ms_at_stop",
            prepared.byte_length.saturating_sub(end) / 32,
        );
        if start != offset || end <= offset || end > prepared.byte_length {
            crate::latency::detail(audio.metadata.id, "preview_retention", "window_changed");
            return false;
        }
        let Ok(mut file) = tokio::fs::File::open(&prepared.path).await else {
            return false;
        };
        let length = ((prepared.byte_length - offset) as usize).min(LIVE_WINDOW + LOOKAHEAD);
        let mut pcm = vec![0; length];
        if file.seek(std::io::SeekFrom::Start(offset)).await.is_err()
            || file.read_exact(&mut pcm).await.is_err()
        {
            return false;
        }
        let covered = (end - offset) as usize;
        if covered > pcm.len()
            || segment_cache::key(audio.metadata.id, c, options, &mixed::wav(&pcm[..covered]))
                != active.key
        {
            return false;
        }
        if window_ranges(&pcm)
            .first()
            .is_some_and(|r| offset + r.end as u64 == end)
        {
            crate::latency::detail(
                audio.metadata.id,
                "preview_retention",
                "final_window_matches",
            );
            return true;
        }
        let quiet = quiet_after(&mut file, end, prepared.byte_length).await;
        crate::latency::detail(
            audio.metadata.id,
            "preview_retention",
            if quiet {
                "quiet_tail"
            } else {
                "tail_not_proven_quiet"
            },
        );
        quiet
    }

    pub(super) async fn prepared_text(
        &self,
        c: &LocalConfig,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
        preview: bool,
    ) -> Result<String, ApiError> {
        let prepared = audio.prepared.as_ref().expect("trusted prepared source");
        let mut file = tokio::fs::File::open(&prepared.path)
            .await
            .map_err(|_| audio_missing())?;
        if prepared.byte_length == 0
            || !prepared.byte_length.is_multiple_of(2)
            || file.metadata().await.map_err(|_| audio_missing())?.len() < prepared.byte_length
        {
            return Err(audio_missing());
        }
        let identity = segment_cache::key(audio.metadata.id, c, options, &[]);
        let mut prefix = self
            .prepared_segments
            .lock()
            .unwrap()
            .get(&identity)
            .filter(|p| p.offset <= prepared.byte_length);
        if !preview {
            // Registration may copy across volumes. Verify exact normalized prefix
            // bytes before reusing provisional work in a durable final result.
            if let Some(saved) = prefix.as_ref() {
                let mut digest = Sha256::new();
                let mut remaining = saved.offset;
                let mut buffer = vec![0; 64 * 1024];
                while remaining > 0 {
                    let count = remaining.min(buffer.len() as u64) as usize;
                    file.read_exact(&mut buffer[..count])
                        .await
                        .map_err(|_| audio_missing())?;
                    digest.update(&buffer[..count]);
                    remaining -= count as u64;
                }
                if digest.finalize() != saved.digest.clone().finalize() {
                    prefix = None;
                }
            }
        }
        let mut prefix = prefix.unwrap_or(Prefix {
            offset: 0,
            text: String::new(),
            digest: Sha256::new(),
            at: Instant::now(),
            provisional: None,
        });
        let mut offset = prefix.offset;
        let mut text = prefix.text.clone();
        let mut inferred = false;
        if !preview {
            crate::latency::detail(audio.metadata.id, "prepared_bytes", prepared.byte_length);
            crate::latency::detail(audio.metadata.id, "committed_bytes_at_final", offset);
        }
        let append = |text: &mut String, part: &str| -> Result<(), ApiError> {
            if text.len().saturating_add(part.len()).saturating_add(1) > RESPONSE_LIMIT {
                return Err(invalid_response());
            }
            append_phrase(text, part);
            Ok(())
        };
        // One live window plus stable-boundary lookahead. Memory never grows with
        // recording duration; the original file is not loaded or sent through JS.
        while offset < prepared.byte_length {
            let length =
                (prepared.byte_length - offset).min((LIVE_WINDOW + LOOKAHEAD) as u64) as usize;
            let mut pcm = vec![0; length];
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|_| audio_missing())?;
            file.read_exact(&mut pcm)
                .await
                .map_err(|_| audio_missing())?;
            // Recheck the growing window before cache reuse. A quiet preview is
            // provisional: do not commit its offset, since later soft speech
            // may change the detector's judgement of the same prefix.
            let digital_silence = pcm.iter().all(|b| *b == 0);
            if digital_silence || (text.is_empty() && speech::speechless(&pcm)) {
                crate::latency::mark(
                    audio.metadata.id,
                    if digital_silence {
                        "digital_silence_skipped"
                    } else {
                        "no_speech_skipped"
                    },
                );
                if preview && !digital_silence {
                    break;
                }
                offset += pcm.len() as u64;
                prefix.offset = offset;
                prefix.digest.update(&pcm);
                prefix.at = Instant::now();
                prefix.provisional = None;
                self.prepared_segments
                    .lock()
                    .unwrap()
                    .put(identity, prefix.clone());
                continue;
            }
            // Use preceding context to distinguish a completed speech tail from
            // quiet new speech. Never discard the source PCM or a cold-VAD guess.
            if offset > 0 && !text.trim().is_empty() {
                let lookback = offset.min(LOOKBACK);
                let mut context = vec![0; lookback as usize + pcm.len()];
                file.seek(std::io::SeekFrom::Start(offset - lookback))
                    .await
                    .map_err(|_| audio_missing())?;
                file.read_exact(&mut context)
                    .await
                    .map_err(|_| audio_missing())?;
                if speech::trailing_quiet(&context, lookback as usize) {
                    crate::latency::mark(audio.metadata.id, "quiet_tail_skipped");
                    offset += pcm.len() as u64;
                    prefix.offset = offset;
                    prefix.digest.update(&pcm);
                    prefix.at = Instant::now();
                    prefix.provisional = None;
                    self.prepared_segments
                        .lock()
                        .unwrap()
                        .put(identity, prefix.clone());
                    continue;
                }
            }
            // The newest speculative decode already covers every voiced frame
            // during a pause. Reuse it for previews too, without committing the
            // open offset; resumed speech must still be decoded normally.
            #[cfg(test)]
            let reuse_pause = !preview || self.preview_pause_reuse;
            #[cfg(not(test))]
            let reuse_pause = true;
            if reuse_pause {
                let provisional = prefix.provisional.filter(|(end, _)| {
                    prefix.offset == offset && *end > offset && *end <= prepared.byte_length
                });
                if let Some((end, key)) = provisional {
                    let covered = (end - offset) as usize;
                    let cached = self.segments.lock().unwrap().get(&key);
                    if let Some(part) = cached.filter(|_| {
                        covered <= pcm.len()
                            && segment_cache::key(
                                audio.metadata.id,
                                c,
                                options,
                                &mixed::wav(&pcm[..covered]),
                            ) == key
                    }) {
                        if quiet_after(&mut file, end, prepared.byte_length).await {
                            crate::latency::mark(audio.metadata.id, "provisional_window_reused");
                            crate::latency::count(audio.metadata.id, "recognition_cache_hits");
                            append(&mut text, &part)?;
                            if preview {
                                break;
                            }
                            prefix.digest.update(&pcm[..covered]);
                            offset = end;
                            prefix.offset = offset;
                            prefix.text = text.clone();
                            prefix.at = Instant::now();
                            prefix.provisional = None;
                            self.prepared_segments
                                .lock()
                                .unwrap()
                                .put(identity, prefix.clone());
                            continue;
                        }
                    }
                }
            }
            let ranges = window_ranges(&pcm);
            let Some(range) = ranges.first() else {
                break;
            };
            let closed = ranges.len() > 1 && range.end + 3_200 <= pcm.len();
            let wav = mixed::wav(&pcm[range.clone()]);
            let key = segment_cache::key(audio.metadata.id, c, options, &wav);
            let cached = self.segments.lock().unwrap().get(&key);
            let part = if let Some(cached) = cached {
                crate::latency::mark(audio.metadata.id, "recognition_cache_hit");
                crate::latency::count(audio.metadata.id, "recognition_cache_hits");
                cached
            } else {
                if preview && inferred {
                    break;
                }
                let window = Some((offset, offset + range.end as u64));
                let part = self
                    .infer(c, wav, options, preview, audio.metadata.id, window)
                    .await?;
                self.segments
                    .lock()
                    .unwrap()
                    .put(audio.metadata.id, key, part.clone());
                inferred = true;
                part
            };
            append(&mut text, &part)?;
            let end = offset + range.end as u64;
            if closed {
                prefix.digest.update(&pcm[range.clone()]);
                prefix.offset = end;
                prefix.text = text.clone();
                prefix.at = Instant::now();
                prefix.provisional = None;
                self.prepared_segments
                    .lock()
                    .unwrap()
                    .put(identity, prefix.clone());
            } else if preview {
                // Open window: remember what was speculated so a Stop shortly
                // after can reuse it. The committed prefix does not advance.
                prefix.provisional = Some((end, key));
                prefix.at = Instant::now();
                self.prepared_segments
                    .lock()
                    .unwrap()
                    .put(identity, prefix.clone());
            }
            offset = end;
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn low_amplitude_is_not_a_committed_speech_boundary() {
        let pcm: Vec<_> = (0..4 * 16000)
            .flat_map(|i| {
                let amplitude = if (2 * 16000..3 * 16000).contains(&i) {
                    10i16
                } else {
                    12000i16
                };
                (if i % 2 == 0 { amplitude } else { -amplitude }).to_le_bytes()
            })
            .collect();
        assert_eq!(
            window_ranges(&pcm),
            std::iter::once(0..pcm.len()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn exact_silence_boundaries_are_stable_and_every_frame_is_retained() {
        let mut pcm: Vec<_> = (0..3 * 16000)
            .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
            .collect();
        pcm.extend(vec![0; 32_000]);
        let first = window_ranges(&pcm[..3 * 32_000 + 9_600])[0].clone();
        pcm.extend(
            (0..16000).flat_map(|i| (if i % 2 == 0 { 10000i16 } else { -10000i16 }).to_le_bytes()),
        );
        let all = window_ranges(&pcm);
        assert_eq!(all[0], first);
        assert_eq!(
            all.iter()
                .flat_map(|r| pcm[r.clone()].iter().copied())
                .collect::<Vec<_>>(),
            pcm
        );
    }
    #[test]
    fn continuous_speech_closes_at_the_capped_live_window_not_eight_seconds() {
        let tone = |n: usize| -> Vec<u8> {
            (0..n)
                .flat_map(|i| (if i % 2 == 0 { 12000i16 } else { -12000i16 }).to_le_bytes())
                .collect()
        };
        let ten = tone(10 * 16000);
        assert_eq!(window_ranges(&ten), vec![0..ten.len()]);
        let long = tone(15 * 16000);
        assert_eq!(window_ranges(&long)[0], 0..LIVE_WINDOW);
        assert_eq!(LIVE_WINDOW + LOOKAHEAD, 14 * 32_000 + 12_800);
    }
    #[test]
    fn a_pause_inside_a_short_dictation_does_not_split_it() {
        // Stopping mid-sentence to think is still one sentence, and a
        // microphone's pause is not digital silence.
        let mut pcm = speech::fixtures::voiced(2 * 16_000);
        pcm.extend(speech::fixtures::room(2 * 16_000));
        pcm.extend(speech::fixtures::voiced(2 * 16_000));
        assert_eq!(window_ranges(&pcm), vec![0..pcm.len()]);
    }
    #[test]
    fn a_capped_window_cuts_at_its_quietest_moment_not_at_the_bound() {
        let mut pcm = speech::fixtures::voiced(12 * 16_000);
        pcm.extend(speech::fixtures::room(6_400)); // 0.4 s between phrases
        pcm.extend(speech::fixtures::voiced(4 * 16_000));
        let ranges = window_ranges(&pcm);
        assert!(
            (12 * 32_000..12 * 32_000 + 12_800).contains(&ranges[0].end),
            "cut at {} bytes is outside the 12.0-12.4 s gap",
            ranges[0].end
        );
        assert_ne!(ranges[0].end, LIVE_WINDOW);
        assert_eq!(ranges.iter().map(|r| r.len()).sum::<usize>(), pcm.len());
    }
    #[test]
    fn a_capped_cut_does_not_move_as_the_recording_grows() {
        let mut pcm = speech::fixtures::voiced(12 * 16_000);
        pcm.extend(speech::fixtures::room(6_400));
        pcm.extend(speech::fixtures::voiced(6 * 16_000));
        let settled = LIVE_WINDOW + speech::SETTLED;
        let first = window_ranges(&pcm[..settled])[0].clone();
        for length in [settled + 32_000, settled + 96_000, pcm.len()] {
            assert_eq!(window_ranges(&pcm[..length])[0], first, "length={length}");
        }
    }
    #[test]
    fn a_silent_lead_in_never_becomes_a_window_of_its_own() {
        let mut pcm = speech::fixtures::room(3 * 16_000);
        pcm.extend(speech::fixtures::voiced(13 * 16_000));
        let cut = window_ranges(&pcm)[0].end;
        assert!(cut > 3 * 32_000, "cut at {cut} bytes is inside the lead-in");
    }
    #[test]
    fn a_window_without_confident_speech_still_closes_at_the_bound() {
        let pcm = speech::fixtures::room(16 * 16_000);
        assert_eq!(window_ranges(&pcm)[0], 0..LIVE_WINDOW);
    }
    /// Where live windows are cut across a directory of saved recordings.
    /// Ignored by default: it needs private audio that never enters this
    /// repository, and it reports offsets and levels only, never text.
    /// `WRITER_REPLAY_PCM_DIR=<dir> cargo test -p metrix-writer-api
    /// replay_saved_recordings -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn replay_saved_recordings_reports_cut_placement() {
        let dir = std::env::var("WRITER_REPLAY_PCM_DIR").expect("WRITER_REPLAY_PCM_DIR");
        let level = |pcm: &[u8]| {
            let square: f64 = pcm
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| (f64::from(i16::from_le_bytes([s[0], s[1]])) / 32_768.0).powi(2))
                .sum();
            10.0 * (square / (pcm.len() / 2).max(1) as f64).max(1e-12).log10()
        };
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("readable directory")
            .filter_map(|entry| Some(entry.ok()?.path().join("prepared.pcm")))
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        let (mut windows, mut cuts, mut loud) = (0usize, 0usize, 0usize);
        for path in &files {
            let pcm = std::fs::read(path).expect("readable prepared audio");
            let mut offset = 0;
            let mut placed = Vec::new();
            while offset < pcm.len() {
                let read = &pcm[offset..(offset + LIVE_WINDOW + LOOKAHEAD).min(pcm.len())];
                let ranges = window_ranges(read);
                let Some(range) = ranges.first() else { break };
                windows += 1;
                if ranges.len() > 1 {
                    let cut = offset + range.end;
                    let at = level(&pcm[cut.saturating_sub(1_600)..(cut + 1_600).min(pcm.len())]);
                    cuts += 1;
                    loud += usize::from(at > -45.0);
                    placed.push(format!("{:.2}s {at:.0} dBFS", cut as f64 / 32_000.0));
                }
                offset += range.end;
            }
            println!(
                "{:>6.1}s {} window(s) [{}]",
                pcm.len() as f64 / 32_000.0,
                placed.len() + 1,
                placed.join(", ")
            );
        }
        println!(
            "{} recording(s), {windows} window(s), {cuts} cut(s), {loud} inside speech (> -45 dBFS)",
            files.len()
        );
    }
}
