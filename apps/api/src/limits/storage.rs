use super::{busy, Limits};
use crate::{recordings::disk::storage, transcription::ApiError};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
mod file_identity;

/// A recent scan reserves a quarter of the *remaining* capacity for untracked
/// writes (native journals or another process), not a quarter of the whole
/// quota. The latter caused every save to walk once history reached 75% usage.
/// Old samples retain the original, wider allowance. Small budgets and requests
/// within the floors still need an exact walk; quotas themselves never change.
fn margin(limit: u64, used: u64, recent: bool) -> u64 {
    let headroom = if recent {
        limit.saturating_sub(used)
    } else {
        limit
    };
    (headroom / 4).max(64 * 1024 * 1024)
}
fn entry_margin(limit: usize, used: usize, recent: bool) -> usize {
    let headroom = if recent {
        limit.saturating_sub(used)
    } else {
        limit
    };
    (headroom / 4).max(1024)
}
const RECENT: Duration = Duration::from_secs(30);
/// A dictation makes several reservations within a few seconds; one walk after
/// the burst covers them all without competing with the saves themselves.
const SETTLE: Duration = Duration::from_secs(2);

pub(crate) struct StorageBudget {
    root: PathBuf,
    extra_roots: Mutex<Vec<PathBuf>>,
    pub limits: Limits,
    pub uploads: Arc<Semaphore>,
    admission: tokio::sync::Mutex<()>,
    reserved: Mutex<HashMap<PathBuf, u64>>,
    ledger: Mutex<Ledger>,
}
/// Directory walks cost seconds on a large Windows history, so admission reuses
/// the last walk while sufficient headroom remains. What Writer released since a
/// walk began may be missing from that walk, so it is added back until a newer
/// walk covers it; that keeps a cached total an upper bound for Writer's own
/// writes. Lock order: `reserved`, then `ledger`, then `extra_roots`.
#[derive(Default)]
struct Ledger {
    /// Every reservation ever released: bytes, and how many.
    released_bytes: u64,
    released: u64,
    walks: u64,
    #[cfg(test)]
    admission_walks: u64,
    samples: HashMap<PathBuf, Sample>,
    refreshing: bool,
    stale: bool,
}
#[derive(Clone, Copy)]
struct Sample {
    walk: u64,
    started: Instant,
    bytes: u64,
    entries: usize,
    released_bytes: u64,
    released: u64,
}
#[derive(Clone, Copy)]
struct WalkStart {
    walk: u64,
    started: Instant,
    released_bytes: u64,
    released: u64,
}
impl Ledger {
    fn begin(&mut self) -> WalkStart {
        self.walks += 1;
        self.stale = false;
        WalkStart {
            walk: self.walks,
            started: Instant::now(),
            released_bytes: self.released_bytes,
            released: self.released,
        }
    }
    fn record(&mut self, start: WalkStart, measured: HashMap<PathBuf, (u64, usize)>) {
        for (path, (bytes, entries)) in measured {
            let sample = Sample {
                walk: start.walk,
                started: start.started,
                bytes,
                entries,
                released_bytes: start.released_bytes,
                released: start.released,
            };
            let newer = self
                .samples
                .get(&path)
                .is_none_or(|previous| previous.walk < start.walk);
            if newer {
                self.samples.insert(path, sample);
            }
        }
    }
    /// A sample's usage plus everything released since its walk began.
    fn bounded(&self, path: &Path) -> Option<(u64, usize, bool)> {
        let sample = self.samples.get(path)?;
        let released = self.released.saturating_sub(sample.released) as usize;
        Some((
            sample
                .bytes
                .saturating_add(self.released_bytes.saturating_sub(sample.released_bytes)),
            sample.entries.saturating_add(released.saturating_mul(4)),
            sample.started.elapsed() <= RECENT,
        ))
    }
}
impl StorageBudget {
    pub fn new(root: PathBuf, limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            root,
            extra_roots: Mutex::new(Vec::new()),
            uploads: Arc::new(Semaphore::new(limits.uploads)),
            limits,
            admission: tokio::sync::Mutex::new(()),
            reserved: Mutex::new(HashMap::new()),
            ledger: Mutex::new(Ledger::default()),
        })
    }
    pub fn include(self: &Arc<Self>, root: PathBuf) {
        if !root.starts_with(&self.root) {
            let mut roots = self.extra_roots.lock().unwrap();
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        // Measure once in the background so the first dictation does not walk.
        self.refresh(Duration::ZERO);
    }
    pub async fn reserve(
        self: &Arc<Self>,
        owner: &Path,
        path: PathBuf,
        bytes: u64,
    ) -> Result<Reservation, ApiError> {
        let _guard = self.admission.lock().await;
        if self.admit_cached(owner, &path, bytes) {
            self.refresh(SETTLE);
            return Ok(Reservation {
                budget: self.clone(),
                path,
            });
        }
        let extras = self.extra_roots.lock().unwrap().clone();
        let start = {
            let mut ledger = self.ledger.lock().unwrap();
            #[cfg(test)]
            {
                ledger.admission_walks += 1;
            }
            ledger.begin()
        };
        let mut paths = vec![self.root.clone(), owner.to_owned()];
        paths.extend(extras.iter().cloned());
        let measured = measure(paths).await?;
        let (mut used, mut global_entries) = measured[&self.root];
        for root in &extras {
            let (bytes, entries) = measured[root];
            used = used.saturating_add(bytes);
            global_entries = global_entries.saturating_add(entries);
        }
        let (local, entries) = measured[owner];
        let mut reserved = self.reserved.lock().unwrap();
        self.ledger.lock().unwrap().record(start, measured);
        let total: u64 = reserved.values().copied().sum();
        let own: u64 = reserved
            .iter()
            .filter(|(p, _)| p.starts_with(owner))
            .map(|(_, n)| *n)
            .sum();
        // Counting streamed bytes as well as their reservation is conservative.
        if used.saturating_add(total).saturating_add(bytes) > self.limits.storage_bytes
            || local.saturating_add(own).saturating_add(bytes) > self.limits.device_storage_bytes
            || global_entries.saturating_add(reserved.len().saturating_mul(4))
                >= self.limits.storage_entries
            || entries >= self.limits.storage_entries
        {
            return Err(busy("Storage budget reached; export/delete history or raise the configured budget before retrying. Saved audio is retained."));
        }
        if reserved.contains_key(&path) {
            return Err(busy(
                "This recording is already uploading; query its ID before retrying.",
            ));
        }
        reserved.insert(path.clone(), bytes);
        Ok(Reservation {
            budget: self.clone(),
            path,
        })
    }
    #[cfg(test)]
    pub(super) fn walks(&self) -> u64 {
        self.ledger.lock().unwrap().walks
    }
    #[cfg(test)]
    pub(super) fn admission_walks(&self) -> u64 {
        self.ledger.lock().unwrap().admission_walks
    }
    #[cfg(test)]
    pub(super) fn age_samples(&self) {
        for sample in self.ledger.lock().unwrap().samples.values_mut() {
            sample.started = Instant::now() - RECENT - Duration::from_secs(1);
        }
    }
    /// Admits from the last walk only when every budget keeps its margin free.
    /// Anything else, including a first request or a new root, walks.
    fn admit_cached(&self, owner: &Path, path: &Path, bytes: u64) -> bool {
        let mut reserved = self.reserved.lock().unwrap();
        if reserved.contains_key(path) {
            return false;
        }
        let ledger = self.ledger.lock().unwrap();
        let extras = self.extra_roots.lock().unwrap().clone();
        let Some((mut used, mut global_entries, mut recent)) = ledger.bounded(&self.root) else {
            return false;
        };
        for root in &extras {
            let Some((bytes, entries, root_recent)) = ledger.bounded(root) else {
                return false;
            };
            used = used.saturating_add(bytes);
            global_entries = global_entries.saturating_add(entries);
            recent &= root_recent;
        }
        let Some((local, entries, local_recent)) = ledger.bounded(owner) else {
            return false;
        };
        let total: u64 = reserved.values().copied().sum();
        let own: u64 = reserved
            .iter()
            .filter(|(p, _)| p.starts_with(owner))
            .map(|(_, n)| *n)
            .sum();
        let limits = &self.limits;
        let fits = used
            .saturating_add(total)
            .saturating_add(bytes)
            .saturating_add(margin(limits.storage_bytes, used, recent))
            <= limits.storage_bytes
            && local
                .saturating_add(own)
                .saturating_add(bytes)
                .saturating_add(margin(limits.device_storage_bytes, local, local_recent))
                <= limits.device_storage_bytes
            && global_entries
                .saturating_add(reserved.len().saturating_add(1).saturating_mul(4))
                .saturating_add(entry_margin(limits.storage_entries, global_entries, recent))
                < limits.storage_entries
            && entries.saturating_add(entry_margin(limits.storage_entries, entries, local_recent))
                < limits.storage_entries;
        if fits {
            reserved.insert(path.to_owned(), bytes);
        }
        fits
    }
    /// One background walk at a time; a request during a walk queues one more.
    fn refresh(self: &Arc<Self>, delay: Duration) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        {
            let mut ledger = self.ledger.lock().unwrap();
            if ledger.refreshing {
                ledger.stale = true;
                return;
            }
            ledger.refreshing = true;
        }
        let budget: Weak<Self> = Arc::downgrade(self);
        runtime.spawn(async move {
            let mut delay = delay;
            loop {
                tokio::time::sleep(delay).await;
                delay = SETTLE;
                let Some(budget) = budget.upgrade() else {
                    return;
                };
                let (start, mut paths) = {
                    let mut ledger = budget.ledger.lock().unwrap();
                    let paths: Vec<PathBuf> = ledger.samples.keys().cloned().collect();
                    (ledger.begin(), paths)
                };
                paths.push(budget.root.clone());
                paths.extend(budget.extra_roots.lock().unwrap().iter().cloned());
                let measured = measure(paths).await;
                let mut ledger = budget.ledger.lock().unwrap();
                if let Ok(measured) = measured {
                    ledger.record(start, measured);
                }
                if !ledger.stale {
                    ledger.refreshing = false;
                    return;
                }
            }
        });
    }
}
pub(crate) struct Reservation {
    budget: Arc<StorageBudget>,
    path: PathBuf,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut reserved = self.budget.reserved.lock().unwrap();
        if let Some(bytes) = reserved.remove(&self.path) {
            let mut ledger = self.budget.ledger.lock().unwrap();
            ledger.released_bytes = ledger.released_bytes.saturating_add(bytes);
            ledger.released = ledger.released.saturating_add(1);
        }
    }
}
/// Bytes and entries below each path, in one pass: a path inside another is
/// counted while its ancestor is walked. No symlink traversal. Include sidecars,
/// tombstones and incomplete writes.
pub(super) async fn measure(
    paths: Vec<PathBuf>,
) -> Result<HashMap<PathBuf, (u64, usize)>, ApiError> {
    Ok(measure_detail(paths, None).await?.0)
}
/// Read-only desktop usage, using the same file identity rules as admission.
pub(crate) async fn usage(
    data: PathBuf,
    models: PathBuf,
    state: PathBuf,
) -> Result<(u64, u64, u64, u64), ApiError> {
    let (totals, audio) = measure_detail(
        vec![data.clone(), models.clone(), state.clone()],
        Some(data.clone()),
    )
    .await?;
    Ok((totals[&data].0, audio, totals[&models].0, totals[&state].0))
}
async fn measure_detail(
    paths: Vec<PathBuf>,
    audio_root: Option<PathBuf>,
) -> Result<(HashMap<PathBuf, (u64, usize)>, u64), ApiError> {
    tokio::task::spawn_blocking(move || {
        let mut audio_bytes = 0_u64;
        let mut audio_seen = HashMap::<file_identity::Identity, u64>::new();
        let paths: BTreeSet<PathBuf> = paths.into_iter().collect();
        let mut totals: HashMap<PathBuf, (u64, usize)> =
            paths.iter().map(|p| (p.clone(), (0, 0))).collect();
        // Native import links the journal's WAV/PCM into the API history. Those
        // names consume entries, but their shared bytes consume storage once.
        // Keep identities per quota root: each owner's allowance remains independent.
        let mut seen: HashMap<PathBuf, HashMap<file_identity::Identity, u64>> =
            paths.iter().map(|p| (p.clone(), HashMap::new())).collect();
        let tops = paths
            .iter()
            .filter(|p| !paths.iter().any(|q| q != *p && p.starts_with(q)));
        for top in tops {
            let mut stack = vec![top.clone()];
            while let Some(path) = stack.pop() {
                let dir = match std::fs::read_dir(&path) {
                    Ok(dir) => dir,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(storage(e)),
                };
                for entry in dir {
                    let entry = entry.map_err(storage)?;
                    let kind = entry.file_type().map_err(storage)?;
                    let path = entry.path();
                    let mut bytes = 0;
                    let mut identity = None;
                    if kind.is_dir() {
                        stack.push(path.clone());
                    } else if kind.is_file() {
                        match entry.metadata() {
                            Ok(m) => {
                                bytes = m.len();
                                identity = file_identity::get(&path, &m);
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => return Err(storage(e)),
                        }
                    }
                    if audio_root.as_ref().is_some_and(|root| {
                        path.strip_prefix(root).ok().is_some_and(|relative| {
                            let parts: Vec<_> = relative.components().collect();
                            parts.len() == 3
                                && matches!(
                                    parts[0].as_os_str().to_str(),
                                    Some("recordings" | "native-recordings")
                                )
                                && matches!(
                                    parts[2].as_os_str().to_str(),
                                    Some("audio" | "audio.wav" | "prepared.pcm")
                                )
                        })
                    }) {
                        let added = if let Some(id) = identity {
                            let previous = audio_seen.entry(id).or_default();
                            let added = bytes.saturating_sub(*previous);
                            *previous = (*previous).max(bytes);
                            added
                        } else {
                            bytes
                        };
                        audio_bytes = audio_bytes.saturating_add(added);
                    }
                    for (root, total) in totals.iter_mut() {
                        // A nested path's own directory belongs to its parent.
                        if path.starts_with(root) && path != *root {
                            let added = if let Some(identity) = identity {
                                let previous =
                                    seen.get_mut(root).unwrap().entry(identity).or_default();
                                // Capture can grow during a walk. Retain the largest
                                // observed size rather than depending on traversal order.
                                let added = bytes.saturating_sub(*previous);
                                *previous = (*previous).max(bytes);
                                added
                            } else {
                                bytes
                            };
                            total.0 = total.0.saturating_add(added);
                            total.1 = total.1.saturating_add(1);
                        }
                    }
                }
            }
        }
        Ok((totals, audio_bytes))
    })
    .await
    .map_err(storage)?
}
