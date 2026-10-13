//! Volatile, bounded reuse of exact PCM segments; never a replacement for a saved job.
use super::{LocalConfig, TranscriptionOptions};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
use uuid::Uuid;
const TTL: Duration = Duration::from_secs(600);
#[derive(Default)]
pub(super) struct SegmentCache {
    sessions: HashMap<Uuid, Instant>,
    entries: VecDeque<([u8; 32], String, Instant)>,
}
impl SegmentCache {
    pub fn activate(&mut self, id: Uuid) {
        self.sessions.retain(|_, at| at.elapsed() < TTL);
        if self.sessions.len() >= 16 {
            self.sessions.clear();
        }
        self.sessions.insert(id, Instant::now());
    }
    pub fn contains(&self, id: Uuid) -> bool {
        self.sessions.get(&id).is_some_and(|at| at.elapsed() < TTL)
    }
    pub fn get(&self, key: &[u8; 32]) -> Option<String> {
        self.entries
            .iter()
            .find(|(k, _, at)| k == key && at.elapsed() < TTL)
            .map(|(_, s, _)| s.clone())
    }
    pub fn put(&mut self, id: Uuid, key: [u8; 32], text: String) {
        self.activate(id);
        // At most 128 * 1 MiB (provider response ceiling), normally a few KiB.
        self.entries
            .retain(|(k, _, at)| k != &key && at.elapsed() < TTL);
        while self.entries.len() >= 128 {
            self.entries.pop_front();
        }
        self.entries.push_back((key, text, Instant::now()));
    }
}
pub(super) fn key(
    id: Uuid,
    c: &LocalConfig,
    options: &TranscriptionOptions,
    wav: &[u8],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    // Length-delimited serialized identity avoids ambiguous concatenation. Model
    // identity includes path + modification metadata so replacement invalidates reuse.
    let metadata = std::fs::metadata(&c.model_path).ok();
    let stamp = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos());
    let mut recognition_options = options.clone();
    // Cleanup runs after recognition and must not invalidate capture-time work.
    recognition_options.cleanup = crate::transcription::CleanupPolicy::Off {};
    digest.update(
        serde_json::to_vec(&(
            "writer-live-earshot122-context512-768-window14-v3",
            id,
            &c.inference_url,
            &c.model,
            &c.model_path,
            metadata.map(|m| m.len()),
            stamp,
            recognition_options,
        ))
        .expect("serializable options"),
    );
    digest.update(wav);
    digest.finalize().into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_cache_does_not_cross_keys() {
        let mut cache = SegmentCache::default();
        let id = Uuid::new_v4();
        cache.put(id, [1; 32], "שלום hello".into());
        assert!(cache.contains(id));
        assert!(!cache.contains(Uuid::new_v4()));
        assert_eq!(cache.get(&[1; 32]).as_deref(), Some("שלום hello"));
        assert_eq!(cache.get(&[2; 32]), None);
        for n in 0..200u16 {
            let mut k = [0; 32];
            k[..2].copy_from_slice(&n.to_le_bytes());
            cache.put(id, k, n.to_string());
        }
        assert_eq!(cache.entries.len(), 128);
        assert!(cache.get(&[1; 32]).is_none());
    }
}
