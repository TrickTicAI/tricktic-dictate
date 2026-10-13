use super::{failure, TextProvider};
use crate::{transcription::*, vocabulary};

// A text-request bound, not an audio-duration cutoff. Raw transcripts remain usable.
const MAX_TEXT_BYTES: usize = 100_000;

/// Each non-whitespace token containing @ is exact, even if not a conventional email.
/// This deliberately overprotects addresses, Unicode local parts and surrounding punctuation.
fn email_parts(input: &str) -> Vec<(bool, &str)> {
    let mut parts = vec![];
    let mut start = 0;
    let mut offset = 0;
    while offset < input.len() {
        let token_end = input[offset..]
            .find(char::is_whitespace)
            .map_or(input.len(), |n| offset + n);
        if token_end > offset && input[offset..token_end].contains('@') {
            if start < offset {
                parts.push((false, &input[start..offset]));
            }
            parts.push((true, &input[offset..token_end]));
            start = token_end;
        }
        offset = token_end;
        while offset < input.len()
            && input[offset..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            offset += input[offset..]
                .chars()
                .next()
                .expect("in bounds")
                .len_utf8();
        }
    }
    if start < input.len() {
        parts.push((false, &input[start..]));
    }
    parts
}

pub(super) async fn clean(
    raw: &str,
    vocabulary: &Vocabulary,
    selection: &ModelSelection,
    provider: &dyn TextProvider,
) -> Result<String, ApiError> {
    if raw.len() > MAX_TEXT_BYTES {
        return Err(failure(ErrorCode::UnsupportedCapability, "Cleanup supports at most 100,000 UTF-8 text bytes per request. The complete original transcript is retained.", false));
    }
    let processed = vocabulary::process(raw, vocabulary)?;
    let inputs = processed.cleanup_inputs();
    let partitions: Vec<_> = inputs.iter().map(|input| email_parts(input)).collect();
    // Whitespace-only spans and protected values never need a provider call.
    let editable: Vec<String> = partitions
        .iter()
        .flatten()
        .filter(|(exact, s)| !exact && !s.trim().is_empty())
        .map(|(_, s)| (*s).into())
        .collect();
    let outputs = if editable.is_empty() {
        vec![]
    } else {
        provider.clean(selection, &editable).await?
    };
    if outputs.len() != editable.len() {
        return Err(failure(
            ErrorCode::InvalidProviderResponse,
            "Cleanup omitted or added text spans. The original transcript is available.",
            true,
        ));
    }
    for (input, output) in editable.iter().zip(&outputs) {
        if output.len() > input.len().saturating_mul(3) + 128 {
            return Err(failure(ErrorCode::InvalidProviderResponse, "Cleanup returned excessive output. Its output was rejected; the original transcript is available.", true));
        }
        super::validation::validate(input, output, false, false)?;
    }
    let mut outputs = outputs.iter();
    let restored: Vec<String> = partitions
        .iter()
        .map(|parts| {
            parts
                .iter()
                .map(|(exact, input)| {
                    if *exact || input.trim().is_empty() {
                        return input.to_string();
                    }
                    let output = outputs.next().expect("count validated");
                    // Keep boundaries around exact spans; a model trimming them cannot glue words.
                    let leading = &input[..input.len() - input.trim_start().len()];
                    let trailing = &input[input.trim_end().len()..];
                    format!("{leading}{}{trailing}", output.trim())
                })
                .collect()
        })
        .collect();
    processed.restore_cleaned(&restored)
}
