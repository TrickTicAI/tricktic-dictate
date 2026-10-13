//! One cheap authenticated request per cloud provider, so a stored key can be
//! proven without dictating first.
//!
//! This is deliberately *not* a transcription. It answers only "did the
//! provider accept this key", which is strictly less than
//! [`CloudView::verified`](super::CloudView::verified) — that still means a real
//! transcript succeeded. Both are reported, and neither is presented as the
//! other.
//!
//! The key is read from the same trusted store the transcription adapters read.
//! Nothing here accepts a key over HTTP, echoes one back, or logs one: the
//! header is marked sensitive and only the outcome leaves this module.
use crate::transcription::ProviderId;
use reqwest::header::HeaderValue;
use serde::Serialize;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1";
pub const GEMINI_ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const GROK_ENDPOINT: &str = "https://api.x.ai/v1";
pub const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1";

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    /// The provider answered successfully. It says nothing about transcript quality.
    Accepted,
    /// The provider rejected the key itself (401/403).
    Rejected,
    /// No usable answer: network failure, timeout, or any other status.
    Unreachable,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct CloudCheck {
    pub outcome: CheckOutcome,
    pub checked_at_ms: u64,
}

/// Base URLs for the check requests. Held per [`Setup`](super::Setup) so tests
/// can point at a loopback fixture instead of reaching a real provider.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub openai: String,
    pub gemini: String,
    pub grok: String,
    pub openrouter: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            openai: OPENAI_ENDPOINT.to_owned(),
            gemini: GEMINI_ENDPOINT.to_owned(),
            grok: GROK_ENDPOINT.to_owned(),
            openrouter: OPENROUTER_ENDPOINT.to_owned(),
        }
    }
}
impl Endpoints {
    pub fn base(&self, provider: &ProviderId) -> Option<&str> {
        match provider {
            ProviderId::Openai => Some(&self.openai),
            ProviderId::Gemini => Some(&self.gemini),
            ProviderId::Grok => Some(&self.grok),
            ProviderId::Openrouter => Some(&self.openrouter),
            ProviderId::Local => None,
        }
    }
}

/// Stable cache key. [`ProviderId`] is not `Eq`, and a discriminant is not a map key.
pub fn label(provider: &ProviderId) -> Option<&'static str> {
    match provider {
        ProviderId::Openai => Some("openai"),
        ProviderId::Gemini => Some("gemini"),
        ProviderId::Grok => Some("grok"),
        ProviderId::Openrouter => Some("openrouter"),
        ProviderId::Local => None,
    }
}

pub fn provider_from(label: &str) -> Option<ProviderId> {
    match label {
        "openai" => Some(ProviderId::Openai),
        "gemini" => Some(ProviderId::Gemini),
        "grok" => Some(ProviderId::Grok),
        "openrouter" => Some(ProviderId::Openrouter),
        _ => None,
    }
}

/// Lists the provider's models with the stored key.
///
/// A model listing is the cheapest authenticated call both providers publish,
/// it sends no audio and no transcript, and its failure modes map cleanly onto
/// the three outcomes. The response body is discarded unread.
pub async fn run(provider: &ProviderId, key: &str, base: &str) -> CheckOutcome {
    let Some(mut header) = HeaderValue::from_str(&match provider {
        ProviderId::Openai | ProviderId::Grok | ProviderId::Openrouter => format!("Bearer {key}"),
        _ => key.to_owned(),
    })
    .ok()
    .filter(|_| key.is_ascii() && !key.chars().any(char::is_control)) else {
        return CheckOutcome::Rejected;
    };
    header.set_sensitive(true);
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
    else {
        return CheckOutcome::Unreachable;
    };
    let request = match provider {
        ProviderId::Openai | ProviderId::Grok => client
            .get(format!("{base}/models"))
            .header("authorization", header),
        ProviderId::Gemini => client
            .get(format!("{base}/models"))
            .header("x-goog-api-key", header),
        // OpenRouter's model list is public and would accept any key; this
        // route describes the calling key and answers 401 for a bad one.
        ProviderId::Openrouter => client
            .get(format!("{base}/key"))
            .header("authorization", header),
        ProviderId::Local => return CheckOutcome::Unreachable,
    };
    match request.send().await {
        Ok(response) if response.status().is_success() => CheckOutcome::Accepted,
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => CheckOutcome::Rejected,
        Ok(_) | Err(_) => CheckOutcome::Unreachable,
    }
}
