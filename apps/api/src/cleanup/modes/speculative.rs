//! Disposable cloud formatting. A durable final job is the only delivery authority.
//!
//! Nothing is sent while the person is still speaking: every earlier draft
//! re-sent the whole transcript so far and was rarely the final wording. At
//! release one draft of the newest preview starts while the final recognition
//! pass runs, and only to the chosen model; the backup joins it only through the
//! final processing, on failure or once it is clearly slow.
use super::*;
use crate::transcription::{ProviderId, Vocabulary};
use processing::{Measured, Prepared};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{watch, Mutex},
    task::AbortHandle,
};

const RELEASE_GRACE: Duration = Duration::from_millis(120);
/// How many recent releases decide whether a release draft is still worth
/// sending, and how many are observed before it can be switched off.
const REUSE_HISTORY: usize = 10;
const REUSE_LEARNING: usize = 4;
#[derive(Default)]
pub(super) struct Drafts {
    entries: Mutex<HashMap<Uuid, Entry>>,
    /// Whether the newest preview at release matched the final transcript,
    /// recorded whether or not a draft was sent, so the record recovers by itself
    /// once previews start matching again.
    reuse: std::sync::Mutex<VecDeque<bool>>,
}
struct Entry {
    created: Instant,
    latest: Option<Snapshot>,
    released: bool,
    release_ready: bool,
    catchup_started: bool,
    candidate: Option<Candidate>,
}
struct Snapshot {
    key: String,
    attempt: ProcessingAttempt,
    vocabulary: Vocabulary,
}
struct Candidate {
    key: String,
    raw_text: String,
    started: Instant,
    result: watch::Receiver<Option<Measured>>,
    task: AbortHandle,
}
impl Drop for Candidate {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn key(attempt: &ProcessingAttempt, vocabulary: &Vocabulary) -> Option<String> {
    key_with_raw(attempt, vocabulary, &attempt.raw_text)
}
fn key_with_raw(attempt: &ProcessingAttempt, vocabulary: &Vocabulary, raw: &str) -> Option<String> {
    serde_json::to_string(&(
        &attempt.profile,
        &attempt.request.context,
        raw,
        vocabulary,
        attempt.prompt_version,
    ))
    .ok()
}
/// The final transcript is the draft's input or that input with one appended
/// sentence mark, under the same settings. A cheap shape check for the reuse
/// record; actual reuse of an ending still needs the stricter check below.
fn could_reuse(key: &str, raw: &str, attempt: &ProcessingAttempt, vocabulary: &Vocabulary) -> bool {
    matches!(
        attempt.raw_text.strip_prefix(raw),
        Some("" | "." | "?" | "!")
    ) && key_with_raw(attempt, vocabulary, raw).as_deref() == Some(key)
}
// Only a *completed, validated* result can resolve an appended sentence mark.
// Never strip punctuation globally: quotes, decimals, ellipses and corrections
// can change meaning, and a new punctuation-bearing vocabulary rule can match.
impl Candidate {
    fn matches_completed_ending(
        &self,
        attempt: &ProcessingAttempt,
        vocabulary: &Vocabulary,
    ) -> bool {
        if key_with_raw(attempt, vocabulary, &self.raw_text).as_ref() != Some(&self.key) {
            return false;
        }
        let Some(mark) = attempt.raw_text.strip_prefix(&self.raw_text) else {
            return false;
        };
        if !matches!(mark, "." | "?" | "!")
            || !self
                .raw_text
                .chars()
                .next_back()
                .is_some_and(char::is_alphabetic)
        {
            return false;
        }
        let last_word = self.raw_text.split_whitespace().next_back().unwrap_or("");
        if last_word.contains(['.', '@', '/', '\\', ':']) {
            return false;
        }
        let Ok(before) = crate::vocabulary::process(&self.raw_text, vocabulary) else {
            return false;
        };
        let Ok(after) = crate::vocabulary::process(&attempt.raw_text, vocabulary) else {
            return false;
        };
        use crate::vocabulary::TextSegment;
        let (
            Some((TextSegment::Editable(a), prefix_a)),
            Some((TextSegment::Editable(b), prefix_b)),
        ) = (
            before.segments().split_last(),
            after.segments().split_last(),
        )
        else {
            return false;
        };
        if prefix_a != prefix_b || b.strip_prefix(a) != Some(mark) {
            return false;
        }
        let measured = self.result.borrow();
        let Some((Ok(text), _)) = measured.as_ref() else {
            return false;
        };
        // The model must already have selected this exact single sentence mark.
        text.trim_end().strip_suffix(mark).is_some_and(|body| {
            body.chars()
                .next_back()
                .is_some_and(|c| !matches!(c, '.' | '?' | '!' | '…'))
        })
    }
}
impl Entry {
    /// Starts the one release draft, with the chosen model only.
    fn start(&mut self, id: Uuid, processor: Arc<dyn processing::Processor>, worthwhile: bool) {
        let Some(latest) = self.latest.as_ref() else {
            return;
        };
        if self.candidate.is_some() {
            return;
        }
        if !worthwhile {
            crate::latency::detail(id, "mode_draft_skipped", "rarely_reused");
            return;
        }
        if latest
            .attempt
            .profile
            .text_model
            .as_ref()
            .is_some_and(|m| processor.cooling_down(m))
        {
            // Reserve provider capacity for the final request.
            return;
        }
        self.catchup_started = true;
        let (send, receive) = watch::channel(None);
        crate::latency::count(id, "mode_draft_requests");
        crate::latency::count(id, "mode_release_draft_requests");
        let mut attempt = latest.attempt.clone();
        let timeout = processing::polish_timeout(&attempt.profile, processor.as_ref());
        // A draft never starts the backup: if the final transcript matches, the
        // final processing races the backup against this same request.
        attempt.profile.fallback_model = None;
        let vocabulary = latest.vocabulary.clone();
        let task = tokio::spawn(async move {
            let measured = diagnostics::measure(async {
                tokio::time::timeout(
                    timeout,
                    processing::run(&attempt, &vocabulary, processor.as_ref()),
                )
                .await
                .unwrap_or_else(|_| Err(processing::invalid("Draft processing timed out.")))
            })
            .await;
            let _ = send.send(Some(measured));
        });
        self.candidate = Some(Candidate {
            key: latest.key.clone(),
            raw_text: latest.attempt.raw_text.clone(),
            started: Instant::now(),
            result: receive,
            task: task.abort_handle(),
        });
    }
}
impl Drafts {
    /// Send release drafts until most recent releases show the preview rarely
    /// matches the final transcript.
    fn worthwhile(&self) -> bool {
        let reuse = self.reuse.lock().unwrap();
        reuse.len() < REUSE_LEARNING || reuse.iter().filter(|hit| **hit).count() * 2 >= reuse.len()
    }
    fn observe(&self, hit: bool) {
        let mut reuse = self.reuse.lock().unwrap();
        if reuse.len() == REUSE_HISTORY {
            reuse.pop_front();
        }
        reuse.push_back(hit);
    }
    pub(super) async fn begin(&self, id: Uuid) {
        let mut entries = self.entries.lock().await;
        if entries.contains_key(&id) {
            return;
        }
        entries.retain(|_, e| e.created.elapsed() < Duration::from_secs(3600));
        if entries.len() >= 16 {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, e)| e.created)
                .map(|(id, _)| *id)
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            id,
            Entry {
                created: Instant::now(),
                latest: None,
                released: false,
                release_ready: false,
                catchup_started: false,
                candidate: None,
            },
        );
    }
    pub(super) async fn discard(&self, id: Uuid) {
        self.entries.lock().await.remove(&id);
    }
    pub(super) async fn clear(&self) {
        self.entries.lock().await.clear();
    }
    pub(super) async fn release(&self, id: Uuid) -> bool {
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.get_mut(&id) else {
            return false;
        };
        if entry.released {
            return false;
        }
        entry.released = true;
        true
    }
    pub(super) async fn catch_up(&self, id: Uuid, processor: Arc<dyn processing::Processor>) {
        let worthwhile = self.worthwhile();
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.get_mut(&id) else {
            return;
        };
        if entry.released && !entry.catchup_started {
            entry.release_ready = true;
            entry.start(id, processor, worthwhile);
        }
    }
    /// Remembers the newest preview. Nothing is sent before release.
    pub(super) async fn offer(
        &self,
        id: Uuid,
        attempt: ProcessingAttempt,
        vocabulary: Vocabulary,
        processor: Arc<dyn processing::Processor>,
    ) {
        if attempt.profile.kind != ModeKind::Super
            || !attempt.profile.process_while_speaking
            || attempt
                .profile
                .text_model
                .as_ref()
                .is_none_or(|m| matches!(m.provider, ProviderId::Local))
            || attempt.raw_text.trim().len() < 12
            || attempt.raw_text.len() > 20_000
        {
            return;
        }
        let Some(key) = key(&attempt, &vocabulary) else {
            return;
        };
        if key.len() > 100_000 {
            return;
        }
        let worthwhile = self.worthwhile();
        let mut entries = self.entries.lock().await;
        // New bindings alone create entries. Late callbacks cannot resurrect
        // canceled/evicted captures or finalization that already consumed one.
        let Some(entry) = entries.get_mut(&id) else {
            return;
        };
        if entry.catchup_started {
            return;
        }
        entry.latest = Some(Snapshot {
            key,
            attempt,
            vocabulary,
        });
        // A preview that finishes just after release is the newest wording.
        if entry.released && entry.release_ready {
            entry.start(id, processor, worthwhile);
        }
    }
    /// The release draft for this final input, if there is one: already
    /// answered, or still running for the final processing to continue.
    pub(super) async fn take(
        &self,
        id: Uuid,
        attempt: &ProcessingAttempt,
        vocabulary: &Vocabulary,
    ) -> Option<Prepared> {
        let entry = self.entries.lock().await.remove(&id);
        let observed = entry.as_ref().and_then(|e| {
            e.candidate
                .as_ref()
                .map(|c| (c.key.as_str(), c.raw_text.as_str()))
                .or_else(|| {
                    e.latest
                        .as_ref()
                        .filter(|_| e.released)
                        .map(|l| (l.key.as_str(), l.attempt.raw_text.as_str()))
                })
                .map(|(key, raw)| could_reuse(key, raw, attempt, vocabulary))
        });
        if let Some(hit) = observed {
            self.observe(hit);
        }
        let Some(candidate) = entry.and_then(|e| e.candidate) else {
            crate::latency::detail(
                id,
                "mode_draft_miss",
                if attempt.profile.process_while_speaking {
                    "no_candidate"
                } else {
                    "disabled"
                },
            );
            return None;
        };
        crate::latency::detail(
            id,
            "mode_draft_age_ms",
            candidate.started.elapsed().as_millis() as u64,
        );
        crate::latency::detail(
            id,
            "mode_draft_input_bytes",
            candidate.raw_text.len() as u64,
        );
        crate::latency::detail(id, "mode_final_input_bytes", attempt.raw_text.len() as u64);
        let exact = candidate.key == key(attempt, vocabulary)?;
        let ending = !exact && candidate.matches_completed_ending(attempt, vocabulary);
        if !exact && !ending {
            crate::latency::detail(
                id,
                "mode_draft_miss",
                if candidate.raw_text == attempt.raw_text {
                    "settings_changed"
                } else {
                    "input_changed"
                },
            );
            crate::latency::detail(
                id,
                "mode_draft_shared_prefix_bytes",
                candidate
                    .raw_text
                    .bytes()
                    .zip(attempt.raw_text.bytes())
                    .take_while(|(a, b)| a == b)
                    .count() as u64,
            );
            return None;
        }
        let started = candidate.started;
        let mut candidate = candidate;
        Some(Prepared {
            started,
            result: Box::pin(async move {
                let measured = loop {
                    if let Some(measured) = candidate.result.borrow().clone() {
                        break measured;
                    }
                    if candidate.result.changed().await.is_err() {
                        crate::latency::detail(id, "mode_draft_miss", "interrupted");
                        break (
                            Err(processing::invalid("Draft processing stopped.")),
                            Diagnostics::default(),
                        );
                    }
                };
                if measured.0.is_ok() {
                    diagnostics::update(|d| {
                        d.reused_draft = true;
                        d.prepared_at_release = true;
                    });
                    crate::latency::detail(
                        id,
                        "mode_draft_reuse_match",
                        if ending { "sentence_ending" } else { "exact" },
                    );
                    crate::latency::mark(id, "mode_draft_reused");
                } else {
                    crate::latency::detail(id, "mode_draft_failed", "chosen_model");
                }
                measured
            }),
        })
    }
}
impl ModeService {
    pub async fn prepare_draft(
        self: &Arc<Self>,
        id: Uuid,
        raw_text: String,
        vocabulary: Vocabulary,
    ) {
        let Ok(Some(captured)) = self.captured_mode(id).await else {
            return;
        };
        if captured.context_pending || captured.context_erased {
            return;
        }
        let attempt = ProcessingAttempt {
            request_digest: None,
            id: Uuid::nil(),
            job_id: Uuid::nil(),
            created_at_ms: 0,
            request: ProcessRequest {
                operation_id: Uuid::nil(),
                profile_id: captured.profile.id.clone(),
                profile_revision: captured.profile.revision,
                context: captured.context,
            },
            profile: captured.profile,
            input_text: String::new(),
            raw_text,
            prompt_version: PROMPT_VERSION,
            state: AttemptState::Pending,
            diagnostics: Diagnostics::default(),
        };
        self.drafts
            .offer(id, attempt, vocabulary, self.processor.clone())
            .await;
    }
    /// Does not delay audio finalization. A brief native timer lets an already
    /// running recognition preview publish its newest result just after Stop.
    pub async fn release_draft(self: &Arc<Self>, id: Uuid) {
        if !self.drafts.release(id).await {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RELEASE_GRACE).await;
            service.drafts.catch_up(id, service.processor.clone()).await;
        });
    }
    pub async fn discard_draft(&self, id: Uuid) {
        self.drafts.discard(id).await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocabulary::empty_vocabulary;
    use std::sync::atomic::{AtomicUsize, Ordering};
    impl Drafts {
        /// The newest preview at release, as the desktop sends it.
        pub(super) async fn released(
            &self,
            id: Uuid,
            input: ProcessingAttempt,
            vocabulary: Vocabulary,
            processor: Arc<dyn processing::Processor>,
        ) {
            self.offer(id, input, vocabulary, processor.clone()).await;
            self.release(id).await;
            self.catch_up(id, processor).await;
        }
    }
    /// Takes the release draft as final processing would, with the reuse flags
    /// it reports into the final attempt.
    pub(super) async fn taken(
        cache: &Drafts,
        id: Uuid,
        attempt: &ProcessingAttempt,
        vocabulary: &Vocabulary,
    ) -> Option<Measured> {
        let prepared = cache.take(id, attempt, vocabulary).await?;
        let ((result, _), metrics) = diagnostics::measure(prepared.result).await;
        Some((result, metrics))
    }
    struct Echo(AtomicUsize);
    struct BusyOnce {
        calls: AtomicUsize,
        local: bool,
    }
    #[async_trait::async_trait]
    impl processing::Processor for BusyOnce {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                diagnostics::update(|d| d.local_capacity_exhausted = self.local);
                return Err(crate::cleanup::failure(
                    crate::transcription::ErrorCode::RateLimited,
                    "Synthetic contention",
                    true,
                ));
            }
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            Ok(serde_json::json!({"spans":input["editable_spans"]}).to_string())
        }
    }
    #[tokio::test]
    async fn final_retries_a_locally_refused_draft_but_not_a_provider_rate_limit() {
        for local in [true, false] {
            let cache = Drafts::default();
            let id = Uuid::new_v4();
            cache.begin(id).await;
            let input = attempt();
            let provider = Arc::new(BusyOnce {
                calls: AtomicUsize::new(0),
                local,
            });
            cache
                .released(id, input.clone(), empty_vocabulary(), provider.clone())
                .await;
            finish(&cache, id).await;
            let prepared = cache.take(id, &input, &empty_vocabulary()).await;
            let result =
                processing::run_prepared(&input, &empty_vocabulary(), provider.as_ref(), prepared)
                    .await;
            // A refused draft never reached the provider; a rate-limited one did,
            // and without a backup the original is kept rather than asking again.
            assert_eq!(result.is_ok(), local);
            assert_eq!(
                provider.calls.load(Ordering::SeqCst),
                if local { 2 } else { 1 }
            );
        }
    }
    #[async_trait::async_trait]
    impl processing::Processor for Echo {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            Ok(serde_json::json!({"spans":input["editable_spans"]}).to_string())
        }
    }
    pub(super) fn attempt() -> ProcessingAttempt {
        let mut profile = ModeSettings::default().profiles[1].clone();
        profile.process_while_speaking = true;
        profile.text_model = Some(ModelSelection {
            provider: ProviderId::Gemini,
            model: "gemini-3.8-flash".into(),
        });
        ProcessingAttempt {
            request_digest: None,
            id: Uuid::nil(),
            job_id: Uuid::nil(),
            created_at_ms: 0,
            request: ProcessRequest {
                operation_id: Uuid::nil(),
                profile_id: profile.id.clone(),
                profile_revision: profile.revision,
                context: ContextSnapshot::default(),
            },
            profile,
            raw_text: "hello this is the complete draft".into(),
            input_text: String::new(),
            prompt_version: PROMPT_VERSION,
            state: AttemptState::Pending,
            diagnostics: Diagnostics::default(),
        }
    }
    pub(super) async fn finish(cache: &Drafts, id: Uuid) {
        let mut rx = cache.entries.lock().await[&id]
            .candidate
            .as_ref()
            .unwrap()
            .result
            .clone();
        loop {
            if rx.borrow().is_some() {
                return;
            }
            rx.changed().await.unwrap();
        }
    }
    #[tokio::test]
    async fn nothing_is_sent_while_speaking_and_one_draft_at_release() {
        let cache = Drafts::default();
        let id = Uuid::new_v4();
        cache.begin(id).await;
        let processor = Arc::new(Echo(AtomicUsize::new(0)));
        let mut input = attempt();
        for index in 0..20 {
            input.raw_text = format!("hello this is complete draft {index}");
            // Repeated, unchanged previews used to be what started a draft.
            for _ in 0..2 {
                cache
                    .offer(id, input.clone(), empty_vocabulary(), processor.clone())
                    .await;
            }
        }
        assert_eq!(processor.0.load(Ordering::SeqCst), 0);
        cache.release(id).await;
        cache.catch_up(id, processor.clone()).await;
        finish(&cache, id).await;
        assert_eq!(processor.0.load(Ordering::SeqCst), 1);
        // Later previews and timers never send a second draft.
        input.raw_text = "hello this is the newest draft".into();
        cache
            .offer(id, input.clone(), empty_vocabulary(), processor.clone())
            .await;
        cache.catch_up(id, processor.clone()).await;
        assert_eq!(processor.0.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn final_reuse_requires_exact_context_vocabulary_profile_and_prompt_version() {
        for mutation in 0..5 {
            let cache = Drafts::default();
            let id = Uuid::new_v4();
            cache.begin(id).await;
            let processor = Arc::new(Echo(AtomicUsize::new(0)));
            let mut input = attempt();
            let mut vocabulary = empty_vocabulary();
            cache
                .released(id, input.clone(), vocabulary.clone(), processor)
                .await;
            finish(&cache, id).await;
            match mutation {
                0 => input.request.context.application = "new app".into(),
                1 => input.profile.revision += 1,
                2 => vocabulary.snippets.push(crate::transcription::Snippet {
                    phrase: "hello".into(),
                    text: "new value".into(),
                }),
                3 => input.prompt_version += 1,
                _ => input.raw_text.push_str(" actually tomorrow"),
            }
            assert!(cache.take(id, &input, &vocabulary).await.is_none());
        }
    }
    #[tokio::test]
    async fn local_models_never_receive_drafts() {
        let cache = Drafts::default();
        let id = Uuid::new_v4();
        cache.begin(id).await;
        let processor = Arc::new(Echo(AtomicUsize::new(0)));
        let mut input = attempt();
        input.profile.text_model = Some(ModelSelection {
            provider: ProviderId::Local,
            model: "qwen3-4b-instruct-2507-q4".into(),
        });
        cache
            .released(id, input.clone(), empty_vocabulary(), processor.clone())
            .await;
        assert_eq!(processor.0.load(Ordering::SeqCst), 0);
        assert!(cache.take(id, &input, &empty_vocabulary()).await.is_none());
    }
    #[tokio::test]
    async fn release_drafts_stop_when_previews_rarely_match_and_resume_when_they_do() {
        let cache = Drafts::default();
        let processor = Arc::new(Echo(AtomicUsize::new(0)));
        let dictation = |raw: &str, final_raw: &str| {
            let (raw, final_raw) = (raw.to_owned(), final_raw.to_owned());
            let (cache, processor) = (&cache, processor.clone());
            async move {
                let id = Uuid::new_v4();
                cache.begin(id).await;
                let mut input = attempt();
                input.raw_text = raw;
                let before = processor.0.load(Ordering::SeqCst);
                cache
                    .released(id, input.clone(), empty_vocabulary(), processor.clone())
                    .await;
                let sent = cache.entries.lock().await[&id].candidate.is_some();
                if sent {
                    finish(cache, id).await;
                }
                input.raw_text = final_raw;
                let prepared = cache.take(id, &input, &empty_vocabulary()).await;
                let _ = processing::run_prepared(
                    &input,
                    &empty_vocabulary(),
                    processor.as_ref(),
                    prepared,
                )
                .await;
                (sent, processor.0.load(Ordering::SeqCst) - before)
            }
        };
        // Each miss costs a draft and the final; the first misses still send.
        for _ in 0..REUSE_LEARNING {
            assert_eq!(
                dictation("the preview wording", "the final wording").await,
                (true, 2)
            );
        }
        // Mostly misses: no draft is sent, so the dictation costs one request.
        assert_eq!(
            dictation("the preview wording", "the final wording").await,
            (false, 1)
        );
        // Previews that match are still observed without sending anything.
        for _ in 0..5 {
            assert_eq!(
                dictation("the final wording", "the final wording").await,
                (false, 1)
            );
        }
        // Half of the recent releases matched again, so drafts resume, and a
        // matching draft is the dictation's only request.
        assert_eq!(
            dictation("the final wording", "the final wording").await,
            (true, 1)
        );
    }
}

#[cfg(test)]
mod release_tests {
    use super::tests::taken;
    use super::*;
    use crate::vocabulary::empty_vocabulary;
    use std::sync::atomic::{AtomicUsize, Ordering};
    pub(super) struct Slow(pub(super) AtomicUsize);
    #[async_trait::async_trait]
    impl processing::Processor for Slow {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            Ok(serde_json::json!({"spans":input["editable_spans"]}).to_string())
        }
    }
    fn input(raw: &str) -> ProcessingAttempt {
        let mut profile = ModeSettings::default().profiles[1].clone();
        profile.text_model = Some(ModelSelection {
            provider: ProviderId::Gemini,
            model: "gemini-3.8-flash".into(),
        });
        ProcessingAttempt {
            request_digest: None,
            id: Uuid::nil(),
            job_id: Uuid::nil(),
            created_at_ms: 0,
            request: ProcessRequest {
                operation_id: Uuid::nil(),
                profile_id: profile.id.clone(),
                profile_revision: profile.revision,
                context: ContextSnapshot::default(),
            },
            profile,
            raw_text: raw.into(),
            input_text: String::new(),
            prompt_version: PROMPT_VERSION,
            state: AttemptState::Pending,
            diagnostics: Diagnostics::default(),
        }
    }
    #[tokio::test]
    async fn newest_snapshot_survives_cooldown_and_release_reuses_catchup_once() {
        let cache = Drafts::default();
        let id = Uuid::new_v4();
        cache.begin(id).await;
        let provider = Arc::new(Slow(AtomicUsize::new(0)));
        let old = input("The meeting is on Tuesday.");
        for _ in 0..2 {
            cache
                .offer(id, old.clone(), empty_vocabulary(), provider.clone())
                .await;
        }
        assert_eq!(
            provider.0.load(Ordering::SeqCst),
            0,
            "nothing while speaking"
        );
        assert!(cache.release(id).await);
        // The latest recognition may finish just after Stop. It replaces the
        // remembered snapshot before the release draft starts.
        let final_input = input("The meeting is on Tuesday, actually Thursday.");
        cache
            .offer(
                id,
                final_input.clone(),
                empty_vocabulary(),
                provider.clone(),
            )
            .await;
        cache.catch_up(id, provider.clone()).await;
        cache.catch_up(id, provider.clone()).await;
        let (text, metrics) = taken(&cache, id, &final_input, &empty_vocabulary())
            .await
            .unwrap();
        assert!(text.unwrap().contains("Thursday"));
        assert!(metrics.reused_draft && metrics.prepared_at_release);
        assert_eq!(provider.0.load(Ordering::SeqCst), 1);
        assert!(!cache.release(id).await);
        cache.catch_up(id, provider.clone()).await;
        assert_eq!(
            provider.0.load(Ordering::SeqCst),
            1,
            "finalized captures cannot restart"
        );
    }
    #[tokio::test]
    async fn release_sends_one_draft_and_cancellation_prevents_it() {
        for canceled in [false, true] {
            let cache = Drafts::default();
            let id = Uuid::new_v4();
            cache.begin(id).await;
            let provider = Arc::new(Slow(AtomicUsize::new(0)));
            let final_input = input("The complete final dictation.");
            cache
                .offer(
                    id,
                    final_input.clone(),
                    empty_vocabulary(),
                    provider.clone(),
                )
                .await;
            cache.release(id).await;
            if canceled {
                cache.discard(id).await;
            }
            cache.catch_up(id, provider.clone()).await;
            let result = taken(&cache, id, &final_input, &empty_vocabulary()).await;
            assert_eq!(result.is_some(), !canceled);
            assert_eq!(provider.0.load(Ordering::SeqCst), usize::from(!canceled));
        }
    }
    #[tokio::test]
    async fn first_preview_after_grace_can_start_once_but_finalization_wins_the_race() {
        let cache = Drafts::default();
        let id = Uuid::new_v4();
        cache.begin(id).await;
        let provider = Arc::new(Slow(AtomicUsize::new(0)));
        cache.release(id).await;
        cache.catch_up(id, provider.clone()).await;
        let final_input = input("The preview finished after release.");
        cache
            .offer(
                id,
                final_input.clone(),
                empty_vocabulary(),
                provider.clone(),
            )
            .await;
        assert!(
            taken(&cache, id, &final_input, &empty_vocabulary())
                .await
                .unwrap()
                .1
                .prepared_at_release
        );
        let next = Uuid::new_v4();
        cache.begin(next).await;
        cache
            .offer(
                next,
                final_input.clone(),
                empty_vocabulary(),
                provider.clone(),
            )
            .await;
        cache.release(next).await;
        assert!(cache
            .take(next, &final_input, &empty_vocabulary())
            .await
            .is_none());
        cache.catch_up(next, provider.clone()).await;
        assert_eq!(
            provider.0.load(Ordering::SeqCst),
            1,
            "late timer must not rebill a finalized capture"
        );
    }

    #[tokio::test]
    async fn changing_words_after_catchup_still_requires_fresh_final_processing() {
        let cache = Drafts::default();
        let id = Uuid::new_v4();
        cache.begin(id).await;
        let provider = Arc::new(Slow(AtomicUsize::new(0)));
        cache
            .offer(
                id,
                input("The meeting is on Tuesday."),
                empty_vocabulary(),
                provider.clone(),
            )
            .await;
        cache.release(id).await;
        cache.catch_up(id, provider.clone()).await;
        let changed = input("The meeting is on Thursday.");
        assert!(cache
            .take(id, &changed, &empty_vocabulary())
            .await
            .is_none());
        assert_eq!(
            crate::latency::snapshot(id)["details"]["mode_draft_miss"],
            "input_changed"
        );
    }
}

#[cfg(test)]
mod efficiency_tests;
#[cfg(test)]
mod live_replay;
