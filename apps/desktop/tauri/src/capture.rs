//! One thread owns CPAL. Its realtime callback never waits for disk or the webview.
use crate::storage::{Journal, Recording, Result, Store};
use cpal::traits::{DeviceTrait, StreamTrait};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};
#[path = "shell/microphone_permission.rs"]
mod microphone_permission;
#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub active: bool,
    pub id: Option<uuid::Uuid>,
    pub elapsed: u64,
    pub saved_bytes: u64,
    pub error: Option<String>,
    pub rms: f32,
    pub peak: f32,
    pub level_sequence: u64,
    pub drain_confirmed: bool,
    /// The microphone is delivering sound, not just open. A Bluetooth headset
    /// switching to its call profile sends digital zeros for up to a second or
    /// so first, and words spoken then are not recorded.
    pub listening: bool,
}
/// How long an open microphone may send only digital zeros before it counts as
/// listening anyway. A muted or hardware-gated microphone must not look stuck.
const SILENT_START_LIMIT: Duration = Duration::from_secs(2);
/// Digital zeros are what a device sends before it has any signal; even a quiet
/// room's noise floor is never exactly zero.
fn audible(samples: &[i16]) -> bool {
    samples.iter().any(|sample| *sample != 0)
}
enum Request {
    Preview(
        uuid::Uuid,
        mpsc::Sender<Result<(Recording, std::path::PathBuf, u64)>>,
    ),
    Microphones(mpsc::Sender<Result<crate::microphones::MicrophoneList>>),
    SelectMicrophone(
        Option<String>,
        mpsc::Sender<Result<crate::microphones::MicrophoneList>>,
    ),
    PreferBuiltIn(
        bool,
        mpsc::Sender<Result<crate::microphones::MicrophoneList>>,
    ),
    Start(uuid::Uuid, String, mpsc::Sender<Result<Status>>),
    Stop(mpsc::Sender<Result<Status>>),
    Quiesce(mpsc::Sender<Result<Status>>),
    Status(mpsc::Sender<Status>),
    Get(uuid::Uuid, mpsc::Sender<Result<Option<Recording>>>),
    Delete(uuid::Uuid, mpsc::Sender<Result<()>>),
}
#[derive(Clone)]
pub struct Capture {
    sender: mpsc::Sender<Request>,
    pub store: Store,
}
struct Active {
    _model_lease: Option<metrix_writer_api::local_stt::managed::CapturePin>,
    stream: Option<cpal::Stream>,
    replay: Option<Replay>,
    journal: Journal,
    audio: mpsc::Receiver<Vec<i16>>,
    failed: Arc<AtomicBool>,
    started: Instant,
    committed: Instant,
    /// The first non-zero sample has arrived.
    heard: bool,
}
struct Replay {
    samples: Vec<i16>,
    rate: u32,
    channels: u16,
    cursor: usize,
}
impl Capture {
    pub fn new(store: Store) -> Self {
        Self::with_source(store, None)
    }
    #[cfg(feature = "latency-probe")]
    pub fn replay(store: Store, path: std::path::PathBuf) -> Self {
        Self::with_source(store, Some(path))
    }
    fn with_source(store: Store, replay: Option<std::path::PathBuf>) -> Self {
        let (sender, rx) = mpsc::channel();
        let disk = store.clone();
        std::thread::spawn(move || {
            let mut active: Option<Active> = None;
            let mut status = Status::default();
            let mut closing = false;
            let mut levels = LevelWindow::default();
            loop {
                if let Some(a) = active.as_mut() {
                    // macOS can continue delivering zeros after permission loss.
                    // Use authorization, never signal level, to detect denial.
                    if a.replay.is_none() && a.committed.elapsed() >= Duration::from_secs(1) {
                        if let Err(error) = microphone_permission::check_current() {
                            status.error = Some(format!("{error} Saved audio is retained."));
                            finish(&mut active, &mut status, true);
                        }
                    }
                }
                if let Some(a) = active.as_mut() {
                    if let Some(replay) = a.replay.as_mut() {
                        let available = (a.started.elapsed().as_millis() as usize
                            * replay.rate as usize
                            / 1000)
                            * usize::from(replay.channels);
                        let end = available.min(replay.samples.len());
                        if end > replay.cursor {
                            let samples = &replay.samples[replay.cursor..end];
                            levels.push(samples);
                            if a.journal.append(samples).is_err() {
                                a.failed.store(true, Ordering::Release);
                            }
                            replay.cursor = end;
                        }
                    }
                    for samples in a.audio.try_iter() {
                        if !a.heard && audible(&samples) {
                            a.heard = true;
                            metrix_writer_api::latency::mark(a.journal.metadata.id, "first_audio");
                        }
                        levels.push(&samples);
                        if a.journal.append(&samples).is_err() {
                            a.failed.store(true, Ordering::Release);
                            break;
                        }
                    }
                    status.elapsed = a.started.elapsed().as_millis() as u64;
                    if !status.listening
                        && (a.heard
                            || a.replay.is_some()
                            || a.started.elapsed() >= SILENT_START_LIMIT)
                    {
                        status.listening = true;
                        if !a.heard && a.replay.is_none() {
                            metrix_writer_api::latency::mark(
                                a.journal.metadata.id,
                                "listening_without_audio",
                            );
                        }
                    }
                    if a.committed.elapsed() >= Duration::from_secs(1) {
                        if a.journal.commit().is_err() {
                            a.failed.store(true, Ordering::Release);
                        }
                        status.saved_bytes = a.journal.metadata.byte_length;
                        a.committed = Instant::now();
                    }
                    if a.failed.load(Ordering::Acquire) {
                        status.error = Some(
                            "Microphone or storage interrupted. Saved audio is retained.".into(),
                        );
                        finish(&mut active, &mut status, true);
                    }
                }
                match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(Request::Preview(id, reply)) => {
                        let result = active
                            .as_ref()
                            .filter(|a| a.journal.metadata.id == id)
                            .and_then(|a| a.journal.preview())
                            .ok_or_else(|| "Prepared preview unavailable.".to_owned());
                        let _ = reply.send(result);
                    }
                    Ok(Request::Microphones(reply)) => {
                        let _ = reply.send(crate::microphones::list(&disk));
                    }
                    Ok(Request::SelectMicrophone(id, reply)) => {
                        let result = if active.is_some() {
                            Err("Stop recording before changing the microphone.".into())
                        } else {
                            crate::microphones::select(&disk, id)
                        };
                        let _ = reply.send(result);
                    }
                    Ok(Request::PreferBuiltIn(enabled, reply)) => {
                        let _ = reply.send(crate::microphones::prefer_built_in(&disk, enabled));
                    }
                    Ok(Request::Start(id, language, reply)) => {
                        let result = if closing {
                            Err("Recorder is shutting down.".into())
                        } else if active.is_some() {
                            Err("Recording is already active.".into())
                        } else {
                            start(&disk, id, language, replay.as_deref()).map(|a| {
                                metrix_writer_api::latency::mark(id, "capture_started");
                                active = Some(a);
                                levels = LevelWindow::default();
                                status = Status {
                                    active: true,
                                    id: Some(id),
                                    // A replayed fixture has no device to wait for.
                                    listening: replay.is_some(),
                                    ..Default::default()
                                };
                                status.clone()
                            })
                        };
                        let _ = reply.send(result);
                    }
                    Ok(Request::Quiesce(reply)) => {
                        closing = true;
                        finish(&mut active, &mut status, false);
                        if !status.drain_confirmed {
                            if let Some(id) = status.id {
                                // A user may have freed disk space after a failed
                                // quit. Re-run journal recovery before confirming.
                                if let Ok(Some(metadata)) = disk.get(id, false) {
                                    let length = std::fs::metadata(disk.path(id).join("audio.wav"))
                                        .map(|m| m.len())
                                        .ok();
                                    if length == Some(metadata.byte_length.max(80))
                                        && metadata.capture_state != "capturing"
                                    {
                                        status.drain_confirmed = true;
                                        status.error = None;
                                        status.saved_bytes = metadata.byte_length;
                                    }
                                }
                            }
                        }
                        let _ = reply.send(Ok(status.clone()));
                    }
                    Ok(Request::Stop(reply)) => {
                        finish(&mut active, &mut status, false);
                        let _ = reply.send(Ok(status.clone()));
                    }
                    Ok(Request::Status(reply)) => {
                        if status.active && levels.samples > 0 {
                            (status.rms, status.peak) = levels.take();
                            status.level_sequence += 1;
                        }
                        let _ = reply.send(status.clone());
                    }
                    Ok(Request::Get(id, reply)) => {
                        let _ = reply.send(disk.get(
                            id,
                            active.as_ref().is_some_and(|a| a.journal.metadata.id == id),
                        ));
                    }
                    Ok(Request::Delete(id, reply)) => {
                        let _ = reply.send(
                            if active.as_ref().is_some_and(|a| a.journal.metadata.id == id) {
                                Err("Stop recording before deleting audio.".into())
                            } else {
                                disk.delete(id)
                            },
                        );
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        finish(&mut active, &mut status, true);
                        break;
                    }
                }
            }
        });
        Self { sender, store }
    }
    pub fn microphones(&self) -> Result<crate::microphones::MicrophoneList> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::Microphones(tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn select_microphone(
        &self,
        id: Option<String>,
    ) -> Result<crate::microphones::MicrophoneList> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::SelectMicrophone(id, tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn prefer_built_in(&self, enabled: bool) -> Result<crate::microphones::MicrophoneList> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::PreferBuiltIn(enabled, tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn start(&self, id: uuid::Uuid, language: String) -> Result<Status> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::Start(id, language, tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn quiesce(&self) -> Result<Status> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::Quiesce(tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn stop(&self) -> Result<Status> {
        let (tx, rx) = mpsc::channel();
        self.sender.send(Request::Stop(tx)).map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn preview(&self, id: uuid::Uuid) -> Result<(Recording, std::path::PathBuf, u64)> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::Preview(id, tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn status(&self) -> Result<Status> {
        let (tx, rx) = mpsc::channel();
        self.sender.send(Request::Status(tx)).map_err(unavailable)?;
        rx.recv().map_err(unavailable)
    }
    pub fn list(&self) -> Result<Vec<Recording>> {
        self.store.list_with_recovery(|id| self.get(id))
    }
    pub fn get(&self, id: uuid::Uuid) -> Result<Option<Recording>> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::Get(id, tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
    pub fn delete(&self, id: uuid::Uuid) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Request::Delete(id, tx))
            .map_err(unavailable)?;
        rx.recv().map_err(unavailable)?
    }
}
fn unavailable(_: impl std::fmt::Display) -> String {
    "Native recorder is unavailable.".into()
}
fn finish(active: &mut Option<Active>, status: &mut Status, interrupted: bool) {
    if let Some(a) = active.take() {
        let Active {
            stream,
            mut journal,
            audio,
            failed,
            ..
        } = a;
        drop(stream); // Release microphone before draining queued PCM.
        let mut bad = interrupted || failed.load(Ordering::Acquire);
        for samples in audio.try_iter() {
            if bad {
                break;
            }
            if journal.append(&samples).is_err() {
                bad = true;
                break;
            }
        }
        status.drain_confirmed = journal.finish(bad).is_ok();
        if !status.drain_confirmed {
            status.error = Some(
                "Final audio save failed; retained PCM can be recovered after relaunch.".into(),
            );
        }
        if bad && status.error.is_none() {
            status.error =
                Some("Capture was interrupted. Previously saved audio is retained.".into());
        }
        status.saved_bytes = journal.metadata.byte_length;
        status.elapsed = journal.metadata.duration_ms;
    }
    status.active = false;
    status.listening = false;
    status.rms = 0.;
    status.peak = 0.;
}
fn start(
    store: &Store,
    id: uuid::Uuid,
    language: String,
    replay: Option<&std::path::Path>,
) -> Result<Active> {
    if let Some(path) = replay {
        let bytes = std::fs::read(path).map_err(|_| "Replay fixture unavailable.".to_owned())?;
        let wav = writer_dictation_audio::wav_pcm16(&bytes)
            .map_err(|_| "Replay requires PCM16 WAV.".to_owned())?;
        let journal = store.create(id, language, wav.rate, wav.channels)?;
        let samples = wav
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| i16::from_le_bytes([s[0], s[1]]))
            .collect();
        return Ok(Active {
            _model_lease: None,
            stream: None,
            replay: Some(Replay {
                samples,
                rate: wav.rate,
                channels: wav.channels,
                cursor: 0,
            }),
            journal,
            audio: mpsc::channel().1,
            failed: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            committed: Instant::now(),
            heard: false,
        });
    }
    microphone_permission::with_access(|| start_microphone(store, id, language))
}
fn start_microphone(store: &Store, id: uuid::Uuid, language: String) -> Result<Active> {
    // An unavailable/downloading model never prevents recording. The pin tells
    // subsequent destructive model actions to wait until capture has drained.
    let model_lease = metrix_writer_api::desktop_settings::global()
        .ok()
        .map(|setup| setup.runtime.capture_pin());
    let (device, built_in_instead) = crate::microphones::input(store)?;
    if built_in_instead {
        metrix_writer_api::latency::detail(id, "microphone", "built_in_for_bluetooth");
    }
    let config = device.default_input_config().map_err(|_| {
        "Cannot open microphone. Check system permissions and input device.".to_owned()
    })?;
    let (tx, audio) = mpsc::sync_channel(64);
    let failed = Arc::new(AtomicBool::new(false));
    let mut journal = store.create(id, language, config.sample_rate().0, config.channels())?;
    let result = match config.sample_format() {
        cpal::SampleFormat::I8 => stream::<i8>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::I32 => stream::<i32>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::I64 => stream::<i64>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::U8 => stream::<u8>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::U32 => stream::<u32>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::U64 => stream::<u64>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::F64 => stream::<f64>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::F32 => stream::<f32>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::I16 => stream::<i16>(&device, &config.into(), tx, failed.clone()),
        cpal::SampleFormat::U16 => stream::<u16>(&device, &config.into(), tx, failed.clone()),
        _ => Err("The microphone's default sample format is unsupported.".into()),
    };
    let stream = match result {
        Ok(s) => s,
        Err(e) => {
            let _ = journal.finish(true);
            return Err(e);
        }
    };
    if stream.play().is_err() {
        let _ = journal.finish(true);
        return Err("Cannot start microphone. Check system microphone permissions.".into());
    }
    Ok(Active {
        _model_lease: model_lease,
        stream: Some(stream),
        replay: None,
        journal,
        audio,
        failed,
        started: Instant::now(),
        committed: Instant::now(),
        heard: false,
    })
}
fn stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: mpsc::SyncSender<Vec<i16>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    i16: cpal::FromSample<T>,
{
    let error = failed.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if failed.load(Ordering::Acquire) {
                    return;
                }
                let samples = data
                    .iter()
                    .map(|v| cpal::Sample::to_sample::<i16>(*v))
                    .collect();
                if tx.try_send(samples).is_err() {
                    failed.store(true, Ordering::Release);
                }
            },
            move |_| {
                error.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|_| "Cannot initialize microphone capture.".into())
}

/// Aggregate every callback between UI polls so a quiet last buffer cannot
/// erase a short spoken sound. Audio samples themselves remain unmodified.
#[derive(Default)]
struct LevelWindow {
    squares: f64,
    samples: u64,
    peak: f32,
}
impl LevelWindow {
    fn push(&mut self, samples: &[i16]) {
        for sample in samples {
            let value = f32::from(*sample) / 32768.;
            self.squares += f64::from(value) * f64::from(value);
            self.peak = self.peak.max(value.abs());
        }
        self.samples += samples.len() as u64;
    }
    fn take(&mut self) -> (f32, f32) {
        let value = (
            (self.squares / self.samples.max(1) as f64).sqrt() as f32,
            self.peak,
        );
        *self = Self::default();
        value
    }
}
#[cfg(test)]
mod meter_tests {
    use super::LevelWindow;
    #[test]
    fn speech_is_not_erased_by_the_last_silent_callback() {
        let mut levels = LevelWindow::default();
        levels.push(&[16384, -16384]);
        levels.push(&[0, 0]);
        let (rms, peak) = levels.take();
        assert!((rms - 0.35355338).abs() < 0.00001);
        assert_eq!(peak, 0.5);
        assert_eq!(levels.take(), (0., 0.));
    }
}

#[cfg(test)]
mod interruption_tests {
    use super::*;
    #[test]
    fn only_a_non_zero_sample_counts_as_sound() {
        assert!(!audible(&[]));
        assert!(!audible(&[0; 320]));
        let mut quiet = [0; 320];
        quiet[319] = -1;
        assert!(audible(&quiet));
    }
    #[test]
    fn microphone_callback_failure_retains_committed_pcm_and_stops_metering() {
        let root =
            std::env::temp_dir().join(format!("writer-capture-security-{}", uuid::Uuid::new_v4()));
        let store = Store::open(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4();
        let mut journal = store.create(id, "en".into(), 16000, 1).unwrap();
        journal.append(&[1200; 1600]).unwrap();
        journal.commit().unwrap();
        let (_, audio) = mpsc::sync_channel(1);
        // CPAL reports permission/device loss through this same failure flag.
        let mut active = Some(Active {
            _model_lease: None,
            stream: None,
            replay: None,
            journal,
            audio,
            failed: Arc::new(AtomicBool::new(true)),
            started: Instant::now(),
            committed: Instant::now(),
            heard: false,
        });
        let mut status = Status {
            active: true,
            id: Some(id),
            rms: 0.5,
            peak: 0.8,
            listening: true,
            ..Default::default()
        };
        finish(&mut active, &mut status, true);
        assert!(!status.active && !status.listening && status.drain_confirmed && active.is_none());
        assert_eq!((status.rms, status.peak), (0., 0.));
        assert!(status.error.unwrap().contains("retained"));
        let saved = store.get(id, false).unwrap().unwrap();
        assert_eq!(saved.capture_state, "interrupted");
        assert_eq!(saved.duration_ms, 100);
        let audio = store.audio(id).unwrap();
        assert_eq!(audio.len() as u64, saved.byte_length);
        assert!(audio.len() >= 3200);
        std::fs::remove_dir_all(root).unwrap();
    }
}
