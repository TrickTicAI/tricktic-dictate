//! Timing and token counts only. Never records prompts, text, context or credentials.
use crate::transcription::{ErrorCode, ModelSelection};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Instant,
};
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Diagnostics {
    /// No provider request was made because local processing capacity was busy.
    #[serde(default)]
    pub local_capacity_exhausted: bool,
    pub processing_ms: u64,
    pub first_response_ms: Option<u64>,
    pub response_complete_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thought_tokens: Option<u64>,
    pub reused_draft: bool,
    #[serde(default)]
    pub prepared_at_release: bool,
    /// The routed host that answered, where the provider names one. OpenRouter
    /// serves one model from many hosts, so which one answered is the difference
    /// between a fast dictation and a slow one.
    #[serde(default)]
    pub served_by: Option<String>,
    /// The mode's fallback model produced this result.
    #[serde(default)]
    pub fallback_used: bool,
    #[serde(default)]
    pub fallback_attempted: bool,
    #[serde(default)]
    pub original_used: bool,
    #[serde(default)]
    pub original_reason: Option<ErrorCode>,
    #[serde(default)]
    pub selected_model: Option<ModelSelection>,
    #[serde(default)]
    pub model_attempts: Vec<ModelAttempt>,
    #[serde(default)]
    pub delivery: Option<super::captured::DeliveryReceipt>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct ModelAttempt {
    #[serde(default)]
    pub local_capacity_exhausted: bool,
    pub selection: ModelSelection,
    pub role: String,
    pub state: String,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub started_after_ms: u64,
    pub error: Option<ErrorCode>,
    pub served_by: Option<String>,
}
impl Diagnostics {
    pub(super) fn is_empty(&self) -> bool {
        self.processing_ms == 0
            && !self.local_capacity_exhausted
            && self.first_response_ms.is_none()
            && self.response_complete_ms.is_none()
            && self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.thought_tokens.is_none()
            && !self.reused_draft
            && !self.prepared_at_release
            && self.served_by.is_none()
            && !self.fallback_used
            && !self.original_used
            && self.model_attempts.is_empty()
            && self.delivery.is_none()
    }
}
tokio::task_local! { static METRICS: Arc<Mutex<Diagnostics>>; }
pub(in crate::cleanup) fn update(f: impl FnOnce(&mut Diagnostics)) {
    let _ = METRICS.try_with(|m| {
        if let Ok(mut m) = m.lock() {
            f(&mut m);
        }
    });
}
pub(in crate::cleanup) async fn measure<T>(future: impl Future<Output = T>) -> (T, Diagnostics) {
    let metrics = Arc::new(Mutex::new(Diagnostics::default()));
    measure_into(metrics, future).await
}
pub(super) async fn measure_into<T>(
    metrics: Arc<Mutex<Diagnostics>>,
    future: impl Future<Output = T>,
) -> (T, Diagnostics) {
    let start = Instant::now();
    let result = METRICS.scope(metrics.clone(), future).await;
    let mut diagnostics = metrics.lock().unwrap().clone();
    diagnostics.processing_ms = start.elapsed().as_millis() as u64;
    (result, diagnostics)
}
