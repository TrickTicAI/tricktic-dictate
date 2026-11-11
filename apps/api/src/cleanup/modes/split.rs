//! Sentence-boundary chunks share one deadline; completion order never changes text order.
use super::{diagnostics, processing, *};
use crate::{cleanup::failure, transcription::ErrorCode, vocabulary::TextSegment};
use futures_util::{stream, FutureExt, StreamExt};

const WORDS: usize = 60;
// Leave the production per-device work limit unchanged.
const PARALLEL: usize = 2;

pub(super) fn groups(segments: &[TextSegment]) -> Vec<Vec<TextSegment>> {
    let total: usize = segments
        .iter()
        .map(|s| match s {
            TextSegment::Editable(s) => s.split_whitespace().count(),
            _ => 0,
        })
        .sum();
    if total <= WORDS {
        return vec![segments.to_vec()];
    }
    let mut groups = vec![];
    let mut group = vec![];
    let mut words = 0;
    for segment in segments {
        let TextSegment::Editable(text) = segment else {
            group.push(segment.clone());
            continue;
        };
        let mut start = 0;
        let mut chars = text.char_indices().peekable();
        while let Some((_, c)) = chars.next() {
            if !(c == '\n'
                || matches!(c, '.' | '!' | '?' | '…')
                    && chars.peek().is_none_or(|(_, c)| c.is_whitespace()))
            {
                continue;
            }
            while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                chars.next();
            }
            let end = chars.peek().map_or(text.len(), |(i, _)| *i);
            words += text[start..end].split_whitespace().count();
            group.push(TextSegment::Editable(text[start..end].into()));
            start = end;
            if words >= WORDS {
                groups.push(std::mem::take(&mut group));
                words = 0;
            }
        }
        if start < text.len() {
            words += text[start..].split_whitespace().count();
            group.push(TextSegment::Editable(text[start..].into()));
        }
    }
    if !group.is_empty() {
        groups.push(group);
    }
    // Adjacent editable sentences belong to one prompt span so the model can
    // improve paragraph flow within a chunk, while exact corrections stay hidden.
    for group in &mut groups {
        let mut merged = Vec::new();
        for segment in std::mem::take(group) {
            match (merged.last_mut(), segment) {
                (Some(TextSegment::Editable(previous)), TextSegment::Editable(next)) => {
                    previous.push_str(&next)
                }
                (_, next) => merged.push(next),
            }
        }
        *group = merged;
    }
    groups
}

pub(super) async fn polish(
    attempt: &ProcessingAttempt,
    groups: Vec<Vec<TextSegment>>,
    provider: &dyn processing::Processor,
    selected: &ModelSelection,
    prepared: Option<processing::Prepared>,
) -> Result<String, ApiError> {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(
            attempt
                .profile
                .polish_timeout_ms()
                .min(5_000)
                .saturating_sub(25),
        );
    if let Some(prepared) = prepared {
        if let Ok((Ok(text), metrics)) = tokio::time::timeout_at(deadline, prepared.result).await {
            diagnostics::update(|d| {
                d.model_attempts.extend(metrics.model_attempts);
                d.original_sentences += metrics.original_sentences;
                d.reused_draft = true;
            });
            return Ok(text);
        }
    }
    let mut outputs: Vec<String> = groups
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|s| match s {
                    TextSegment::Editable(s) | TextSegment::Exact(s) => s.as_str(),
                })
                .collect()
        })
        .collect();
    let mut succeeded = vec![false; groups.len()];
    let mut error = failure(
        ErrorCode::ProviderTimeout,
        "Polishing exceeded its deadline. Original text is retained.",
        true,
    );
    let futures: Vec<_> = groups
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, group)| {
            async move {
                let (result, metrics) = diagnostics::measure(processing::super_segments(
                    attempt, &group, provider, selected, None,
                ))
                .await;
                (index, result, metrics)
            }
            .boxed()
        })
        .collect();
    let mut pending = stream::iter(futures).buffer_unordered(PARALLEL);
    while let Ok(Some((index, result, metrics))) =
        tokio::time::timeout_at(deadline, pending.next()).await
    {
        diagnostics::update(|d| {
            d.model_attempts.extend(metrics.model_attempts);
            d.fallback_attempted |= metrics.fallback_attempted;
            d.fallback_used |= metrics.fallback_used;
            d.original_sentences += metrics.original_sentences;
            d.selected_model = metrics.selected_model.or(d.selected_model.take());
            for (total, value) in [
                (&mut d.input_tokens, metrics.input_tokens),
                (&mut d.output_tokens, metrics.output_tokens),
                (&mut d.thought_tokens, metrics.thought_tokens),
            ] {
                if let Some(value) = value {
                    *total = Some(total.unwrap_or(0) + value);
                }
            }
        });
        match result {
            Ok(text) => {
                outputs[index] = text;
                succeeded[index] = true;
            }
            Err(e) => error = e,
        }
    }
    if !succeeded.iter().any(|ok| *ok) {
        return Err(error);
    }
    // A refused or unfinished chunk cannot erase other completed polish.
    // Exact corrections retain their bytes, and raw speech stays in the attempt.
    for (index, group) in groups.iter().enumerate() {
        if !succeeded[index] {
            let mut kept = 0;
            outputs[index] = group
                .iter()
                .map(|s| match s {
                    TextSegment::Exact(s) => s.clone(),
                    TextSegment::Editable(s) => {
                        kept += s
                            .split_inclusive(['.', '!', '?', '\n'])
                            .filter(|s| s.chars().any(char::is_alphanumeric))
                            .count() as u32;
                        super::normalize::polished(s)
                    }
                })
                .collect();
            diagnostics::update(|d| d.original_sentences += kept);
        }
    }
    let text = outputs.concat();
    if text.len() > 400_000 {
        return Err(failure(
            ErrorCode::InvalidProviderResponse,
            "Polished text exceeds the output limit.",
            false,
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn attempt(raw: &str) -> ProcessingAttempt {
        let mut profile = ModeSettings::default().profiles[1].clone();
        profile.text_model = Some(ModelSelection {
            provider: crate::transcription::ProviderId::Openrouter,
            model: "mistralai/mistral-small-2603".into(),
        });
        profile.polish_check = PolishCheck::Off;
        ProcessingAttempt {
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
            request_digest: None,
            raw_text: raw.into(),
            input_text: raw.into(),
            prompt_version: PROMPT_VERSION,
            state: AttemptState::Pending,
            diagnostics: Diagnostics::default(),
            glossary: Glossary::default(),
        }
    }
    fn long() -> String {
        format!("{}first. {}second.", "word ".repeat(80), "מילה ".repeat(80))
    }
    #[test]
    fn splitting_is_lossless_and_never_cuts_identifiers_or_exact_values() {
        let raw = long();
        let input = vec![
            TextSegment::Editable(raw.clone()),
            TextSegment::Exact("CRM-236 — exact".into()),
        ];
        let chunks = groups(&input);
        assert!(chunks.len() > 1);
        let joined: String = chunks
            .iter()
            .flatten()
            .map(|s| match s {
                TextSegment::Editable(s) | TextSegment::Exact(s) => s.as_str(),
            })
            .collect();
        assert_eq!(joined, format!("{raw}CRM-236 — exact"));
        assert_eq!(
            groups(&[TextSegment::Editable("word ".repeat(200))]).len(),
            1
        );
    }
    struct Parallel {
        barrier: tokio::sync::Barrier,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl processing::Processor for Parallel {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.barrier.wait().await; // Neither chunk can finish until both have started.
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            Ok(serde_json::json!({"spans": input["editable_spans"]}).to_string())
        }
    }
    #[tokio::test]
    async fn chunks_run_concurrently_and_reassemble_in_order() {
        let raw = long();
        let provider = Parallel {
            barrier: tokio::sync::Barrier::new(2),
            calls: AtomicUsize::new(0),
        };
        let output = processing::run(
            &attempt(&raw),
            &crate::vocabulary::empty_vocabulary(),
            &provider,
        )
        .await
        .unwrap();
        assert_eq!(output, raw);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }
    struct Partial;
    #[async_trait::async_trait]
    impl processing::Processor for Partial {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            if input["editable_spans"][0]
                .as_str()
                .unwrap()
                .contains("second")
            {
                return Err(failure(ErrorCode::InvalidProviderResponse, "fixture", true));
            }
            let spans: Vec<String> = input["editable_spans"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().replace("first", "First"))
                .collect();
            Ok(serde_json::json!({"spans":spans}).to_string())
        }
    }
    #[tokio::test]
    async fn failed_chunk_keeps_its_original_without_discarding_other_polish() {
        let raw = long();
        let (result, metrics) = diagnostics::measure(processing::run(
            &attempt(&raw),
            &crate::vocabulary::empty_vocabulary(),
            &Partial,
        ))
        .await;
        assert_eq!(result.unwrap(), raw.replace("first", "First"));
        assert_eq!(metrics.original_sentences, 1);
        assert_eq!(
            metrics
                .model_attempts
                .iter()
                .filter(|m| m.state == "failed")
                .count(),
            1
        );
    }
    struct Retry {
        calls: AtomicUsize,
        error: ErrorCode,
        always: bool,
    }
    #[async_trait::async_trait]
    impl processing::Processor for Retry {
        async fn generate(
            &self,
            _: &ModelSelection,
            _: &str,
            input: &str,
        ) -> Result<String, ApiError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 || self.always {
                return Err(failure(self.error.clone(), "fixture", true));
            }
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            Ok(serde_json::json!({"spans": input["editable_spans"]}).to_string())
        }
    }
    #[tokio::test]
    async fn retries_only_transient_errors_once() {
        for error in [
            ErrorCode::RateLimited,
            ErrorCode::InvalidProviderResponse,
            ErrorCode::ProviderUnavailable,
        ] {
            for always in [false, true] {
                let retryable = matches!(
                    error,
                    ErrorCode::RateLimited | ErrorCode::InvalidProviderResponse
                );
                let provider = Retry {
                    calls: AtomicUsize::new(0),
                    error: error.clone(),
                    always,
                };
                let result = processing::run(
                    &attempt("hello"),
                    &crate::vocabulary::empty_vocabulary(),
                    &provider,
                )
                .await;
                assert_eq!(result.is_ok(), retryable && !always);
                assert_eq!(
                    provider.calls.load(Ordering::SeqCst),
                    if retryable { 2 } else { 1 }
                );
            }
        }
    }
}
