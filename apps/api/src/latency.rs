//! Bounded, non-content monotonic diagnostics. Never authoritative session state.
pub mod measurements;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct Event {
    pub stage: &'static str,
    pub elapsed_ms: u64,
}
struct Trace {
    started: Instant,
    stop_ms: Option<u64>,
    events: Vec<Event>,
    /// Small numeric or enumerated facts about how recognition ran: the encoder
    /// context sent, the backend, byte counts, call counts. Never text.
    details: BTreeMap<&'static str, serde_json::Value>,
}
static TRACES: OnceLock<Mutex<HashMap<Uuid, Trace>>> = OnceLock::new();
fn traces() -> &'static Mutex<HashMap<Uuid, Trace>> {
    TRACES.get_or_init(Default::default)
}
fn with_trace(id: Uuid, f: impl FnOnce(&mut Trace)) {
    let Ok(mut traces) = traces().lock() else {
        return;
    };
    if traces.len() >= 64 && !traces.contains_key(&id) {
        if let Some(oldest) = traces
            .iter()
            .min_by_key(|(_, t)| t.started)
            .map(|(id, _)| *id)
        {
            traces.remove(&oldest);
        }
    }
    let trace = traces.entry(id).or_insert_with(|| Trace {
        started: Instant::now(),
        stop_ms: None,
        events: Vec::new(),
        details: BTreeMap::new(),
    });
    f(trace);
}
/// Start a trace at an instant already past, such as the shortcut press that
/// preceded the recording's ID, and mark it. Later marks measure from there.
pub fn begin(id: Uuid, stage: &'static str, at: Instant) {
    with_trace(id, |trace| {
        if trace.events.is_empty() {
            trace.started = at.min(trace.started);
        }
    });
    mark_since(id, stage, at);
}
/// Mark a stage that happened at an earlier instant, placed among the other
/// events in time order.
fn mark_since(id: Uuid, stage: &'static str, at: Instant) {
    with_trace(id, |trace| {
        let elapsed_ms = at.saturating_duration_since(trace.started).as_millis() as u64;
        if trace.events.len() == 256 {
            trace.events.remove(0);
        }
        let position = trace.events.partition_point(|e| e.elapsed_ms <= elapsed_ms);
        trace.events.insert(position, Event { stage, elapsed_ms });
    });
}
/// Mark a stage another process on this machine observed, such as the panel's
/// WebView painting a state, by its wall-clock time in Unix milliseconds. Only
/// as exact as the two clocks agree. A time well in the future or more than a
/// minute old is dropped rather than guessed.
pub fn mark_at_wall(id: Uuid, stage: &'static str, unix_ms: f64) {
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return;
    };
    let age_ms = now.as_secs_f64() * 1000. - unix_ms;
    if !(-1000. ..=60_000.).contains(&age_ms) {
        return;
    }
    if let Some(at) = Instant::now().checked_sub(Duration::from_secs_f64(age_ms.max(0.) / 1000.)) {
        mark_since(id, stage, at);
    }
}
pub fn mark(id: Uuid, stage: &'static str) {
    with_trace(id, |trace| {
        let elapsed_ms = trace.started.elapsed().as_millis() as u64;
        if stage == "stop_accepted" && trace.stop_ms.is_none() {
            trace.stop_ms = Some(elapsed_ms);
        }
        if trace.events.len() == 256 {
            trace.events.remove(0);
        }
        trace.events.push(Event { stage, elapsed_ms });
    });
}
/// Record one non-content fact. Strings are limited to short enumerations
/// (a backend name, a profile label); transcript text must never be passed.
pub fn detail(id: Uuid, key: &'static str, value: impl Into<serde_json::Value>) {
    let value = value.into();
    // Whitelist facts by key AND value type. A short transcript, object,
    // credential or app name is just as private as a long one.
    let accepted = match key {
        "audio_ctx"
        | "final_window_ms"
        | "prepared_bytes"
        | "committed_bytes_at_final"
        | "preview_tail_ms_at_stop"
        | "cloud_upload_bytes"
        | "silence_input_bytes"
        | "silence_output_bytes"
        | "silence_input_ms"
        | "silence_output_ms"
        | "silence_removed_ms"
        | "silence_prepare_ms"
        | "mode_draft_age_ms"
        | "mode_draft_input_bytes"
        | "mode_final_input_bytes"
        | "mode_draft_shared_prefix_bytes" => value.as_u64().is_some(),
        "no_timestamps" => value.is_boolean(),
        "mode" => matches!(value.as_str(), Some("regular" | "super" | "command")),
        "backend" => value
            .as_str()
            .is_some_and(|s| matches!(s, "vulkan" | "cpu" | "cuda" | "metal")),
        "model" => value
            .as_str()
            .is_some_and(|s| crate::local_stt::catalog::spec(s).is_some()),
        "microphone" => value == "built_in_for_bluetooth",
        "cloud_upload" => matches!(value.as_str(), Some("original" | "prepared_16k_mono")),
        "preview_retention" => matches!(
            value.as_str(),
            Some(
                "window_changed" | "final_window_matches" | "quiet_tail" | "tail_not_proven_quiet"
            )
        ),
        "mode_draft_miss" => matches!(
            value.as_str(),
            Some(
                "no_candidate" | "disabled" | "settings_changed" | "input_changed" | "interrupted"
            )
        ),
        "mode_draft_failed" => value == "chosen_model",
        "mode_draft_skipped" => value == "rarely_reused",
        "mode_draft_reuse_match" => matches!(value.as_str(), Some("sentence_ending" | "exact")),
        _ => false,
    };
    if !accepted {
        return;
    }
    with_trace(id, |trace| {
        if trace.details.len() < 32 || trace.details.contains_key(key) {
            trace.details.insert(key, value);
        }
    });
}
/// Add to a counter detail, starting from zero.
pub fn count(id: Uuid, key: &'static str) {
    if !matches!(
        key,
        "inference_calls"
            | "preview_inference_calls"
            | "final_inference_calls"
            | "recognition_cache_hits"
            | "mode_draft_requests"
            | "mode_release_draft_requests"
    ) {
        return;
    }
    with_trace(id, |trace| {
        let next = trace
            .details
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .saturating_add(1);
        if trace.details.len() < 32 || trace.details.contains_key(key) {
            trace.details.insert(key, next.into());
        }
    });
}
pub fn snapshot(id: Uuid) -> serde_json::Value {
    let Ok(traces) = traces().lock() else {
        return serde_json::Value::Null;
    };
    traces.get(&id).map_or(serde_json::Value::Null, |trace| {
        serde_json::json!({
            "recording_id": id, "stop_elapsed_ms": trace.stop_ms, "events": trace.events,
            "details": trace.details,
            "scope": "monotonic native stages; paste_sent is not target acknowledgment"
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn details_are_bounded_non_content_facts() {
        let id = Uuid::new_v4();
        detail(id, "audio_ctx", 768);
        detail(id, "backend", "vulkan");
        detail(id, "raw_text", "short private text");
        detail(id, "backend", serde_json::json!({"secret":"credential"}));
        detail(id, "audio_ctx", "short private text");
        count(id, "inference_calls");
        count(id, "inference_calls");
        mark(id, "stop_accepted");
        let s = snapshot(id);
        assert_eq!(s["details"]["audio_ctx"], 768);
        assert_eq!(s["details"]["backend"], "vulkan");
        assert!(s["details"].get("raw_text").is_none());
        assert_eq!(s["details"]["inference_calls"], 2);
        assert_eq!(s["stop_elapsed_ms"], s["events"][0]["elapsed_ms"]);
    }
    #[test]
    fn a_trace_can_begin_at_an_earlier_press() {
        let id = Uuid::new_v4();
        let pressed = Instant::now() - std::time::Duration::from_millis(40);
        begin(id, "shortcut_pressed", pressed);
        mark(id, "capture_started");
        let s = snapshot(id);
        assert_eq!(s["events"][0]["stage"], "shortcut_pressed");
        assert_eq!(s["events"][0]["elapsed_ms"], 0);
        assert!(s["events"][1]["elapsed_ms"].as_u64().unwrap() >= 40);
    }
    #[test]
    fn a_wall_clock_stage_lands_in_time_order() {
        let id = Uuid::new_v4();
        let pressed = Instant::now() - Duration::from_millis(400);
        begin(id, "shortcut_pressed", pressed);
        mark(id, "panel_listening");
        let wall = |ago_ms: f64| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
                * 1000.
                - ago_ms
        };
        // Reported after panel_listening, but observed 200 ms after the press.
        mark_at_wall(id, "panel_listening_received", wall(200.));
        mark_at_wall(id, "future", wall(-5000.));
        mark_at_wall(id, "stale", wall(120_000.));
        mark_at_wall(id, "nan", f64::NAN);
        let s = snapshot(id);
        let stages: Vec<_> = s["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["stage"].as_str().unwrap())
            .collect();
        assert_eq!(
            stages,
            [
                "shortcut_pressed",
                "panel_listening_received",
                "panel_listening"
            ]
        );
        let received = s["events"][1]["elapsed_ms"].as_u64().unwrap();
        assert!((195..400).contains(&received), "{received}");
    }
}

#[cfg(all(test, windows))]
mod windows_baseline;
