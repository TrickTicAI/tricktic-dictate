//! A visible panel must answer a native challenge. Sleeping the computer or
//! hiding the panel does not count as a failed challenge.
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(8);
const SUSPENDED: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq)]
pub enum Check {
    Idle,
    Ping(u64),
    Reload,
    Recreate,
}

#[derive(Default)]
pub struct Health {
    last_tick: Option<Duration>,
    observed_at: Option<Duration>,
    visible: bool,
    unanswered_visible: Duration,
    pending: Option<u64>,
    nonce: u64,
    failures: u32,
    presentation_failed: bool,
}
impl Health {
    pub fn acknowledge_if(&mut self, nonce: u64, presented: bool) {
        if self.pending == Some(nonce) {
            self.presentation_failed = !presented;
            if presented {
                self.pending = None;
                self.failures = 0;
                self.unanswered_visible = Duration::ZERO;
            }
        }
    }

    pub fn presentation_failed(&self) -> bool {
        self.presentation_failed
    }

    pub fn recovering(&self) -> bool {
        self.presentation_failed || self.failures > 0
    }

    /// An occluded/locked display is no evidence of either failure or recovery.
    /// Start a fresh challenge when it can be inspected, retaining escalation.
    pub fn defer(&mut self, nonce: u64) {
        if self.pending == Some(nonce) {
            self.pending = None;
        }
    }

    /// Account for actual expected-visible intervals, including appearances
    /// shorter than the polling interval. Hiding pauses the budget; only a
    /// displayed reply proves recovery and clears it.
    pub fn visibility(&mut self, now: Duration, visible: bool) -> bool {
        let resumed = self
            .last_tick
            .is_some_and(|last| now.saturating_sub(last) > SUSPENDED);
        if resumed {
            self.pending = None;
            self.unanswered_visible = Duration::ZERO;
            self.presentation_failed = false;
        }
        if self.visible && self.pending.is_some() {
            if let Some(last) = self.observed_at {
                self.unanswered_visible += now.saturating_sub(last);
            }
        }
        self.observed_at = Some(now);
        let changed = self.visible != visible;
        self.visible = visible;
        if !visible {
            // A later appearance needs a fresh visual challenge. Its budget
            // still includes earlier visible time that was never answered.
            self.pending = None;
        }
        changed
    }

    pub fn check(&mut self, now: Duration, visible: bool) -> Check {
        self.visibility(now, visible);
        self.last_tick = Some(now);
        if !visible {
            return Check::Idle;
        }
        if self.unanswered_visible >= TIMEOUT {
            self.failures = self.failures.saturating_add(1);
            self.pending = None;
            self.unanswered_visible = Duration::ZERO;
            return if self.failures == 1 {
                Check::Reload
            } else {
                Check::Recreate
            };
        }
        if let Some(nonce) = self.pending {
            return Check::Ping(nonce);
        }
        self.nonce += 1;
        self.pending = Some(self.nonce);
        Check::Ping(self.nonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seconds(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn a_missing_reply_reloads_then_recreates_and_a_reply_ends_recovery() {
        let mut h = Health::default();
        assert_eq!(h.check(seconds(0), true), Check::Ping(1));
        for at in [2, 4, 6] {
            assert_eq!(h.check(seconds(at), true), Check::Ping(1));
        }
        assert_eq!(h.check(seconds(8), true), Check::Reload);
        assert_eq!(h.check(seconds(10), true), Check::Ping(2));
        h.acknowledge_if(1, true); // An old document must not satisfy a new challenge.
        for at in [12, 14, 16] {
            assert_eq!(h.check(seconds(at), true), Check::Ping(2));
        }
        assert_eq!(h.check(seconds(18), true), Check::Recreate);
        assert_eq!(h.check(seconds(20), true), Check::Ping(3));
        h.acknowledge_if(3, true);
        assert_eq!(h.check(seconds(22), true), Check::Ping(4));
        for at in [24, 26, 28] {
            h.check(seconds(at), true);
        }
        assert_eq!(h.check(seconds(30), true), Check::Reload);
    }

    #[test]
    fn sleep_and_hidden_time_do_not_trigger_recovery() {
        let mut h = Health::default();
        assert_eq!(h.check(seconds(0), true), Check::Ping(1));
        assert_eq!(h.check(seconds(60), true), Check::Ping(2));
        assert_eq!(h.check(seconds(62), false), Check::Idle);
        assert_eq!(h.check(seconds(64), true), Check::Ping(3));
        h.acknowledge_if(2, true);
        assert_eq!(h.check(seconds(66), true), Check::Ping(3));
        h.acknowledge_if(3, true);
        assert_eq!(h.check(seconds(68), true), Check::Ping(4));
    }

    #[test]
    fn live_frames_on_a_blank_surface_do_not_postpone_recovery() {
        let mut h = Health::default();
        assert_eq!(h.check(seconds(0), true), Check::Ping(1));
        for at in [2, 4, 6] {
            h.acknowledge_if(1, false);
            assert!(h.presentation_failed());
            assert_eq!(h.check(seconds(at), true), Check::Ping(1));
        }
        h.acknowledge_if(1, false);
        assert_eq!(h.check(seconds(8), true), Check::Reload);
        assert_eq!(h.check(seconds(10), true), Check::Ping(2));
        for at in [12, 14, 16] {
            h.acknowledge_if(2, false);
            assert_eq!(h.check(seconds(at), true), Check::Ping(2));
        }
        assert_eq!(h.check(seconds(18), true), Check::Recreate);
        assert_eq!(h.check(seconds(20), true), Check::Ping(3));
        h.acknowledge_if(3, true);
        assert!(!h.presentation_failed());
        assert_eq!(h.check(seconds(22), true), Check::Ping(4));
    }

    #[test]
    fn an_unavailable_display_cannot_claim_recovery_or_force_a_reload() {
        let mut h = Health::default();
        assert_eq!(h.check(seconds(0), true), Check::Ping(1));
        h.acknowledge_if(1, false);
        for at in [2, 4, 6] {
            h.check(seconds(at), true);
        }
        assert_eq!(h.check(seconds(8), true), Check::Reload);
        assert!(h.recovering());
        assert_eq!(h.check(seconds(10), true), Check::Ping(2));
        h.defer(1); // An old document cannot postpone this challenge.
        assert_eq!(h.check(seconds(12), true), Check::Ping(2));
        h.defer(2);
        assert!(h.recovering());
        assert_eq!(h.check(seconds(14), true), Check::Ping(3));
        h.acknowledge_if(3, true);
        assert!(!h.recovering());
    }

    #[test]
    fn short_dictations_accumulate_unanswered_visible_time_and_escalate() {
        let mut h = Health::default();
        for cycle in 0..4 {
            assert_eq!(h.check(seconds(cycle * 4), true), Check::Ping(cycle + 1));
            h.acknowledge_if(cycle + 1, false);
            assert_eq!(h.check(seconds(cycle * 4 + 2), false), Check::Idle);
            assert!(h.presentation_failed());
        }
        assert_eq!(h.check(seconds(16), true), Check::Reload);
        // Hiding during reload must not erase escalation if the renderer
        // remains broken after reloading the document.
        h.check(seconds(18), false);
        for cycle in 0..4 {
            assert_eq!(
                h.check(seconds(20 + cycle * 4), true),
                Check::Ping(cycle + 5)
            );
            h.check(seconds(22 + cycle * 4), false);
        }
        assert_eq!(h.check(seconds(36), true), Check::Recreate);
    }

    #[test]
    fn sub_poll_appearances_count_but_hidden_intervals_do_not() {
        let mut h = Health::default();
        for cycle in 0..16 {
            let at = Duration::from_millis(cycle * 2000);
            assert!(h.visibility(at, true));
            assert_eq!(h.check(at, true), Check::Ping(cycle + 1));
            assert!(h.visibility(at + Duration::from_millis(500), false));
            assert_eq!(h.check(at + seconds(1), false), Check::Idle);
        }
        // Eight visible seconds across 32 wall-clock seconds.
        assert_eq!(h.check(seconds(32), true), Check::Reload);
    }

    #[test]
    fn only_a_current_displayed_reply_clears_the_accumulated_budget() {
        let mut h = Health::default();
        h.check(seconds(0), true);
        h.check(seconds(2), false);
        assert_eq!(h.check(seconds(4), true), Check::Ping(2));
        h.acknowledge_if(1, true); // Previous appearance, even in the same document.
        h.check(seconds(6), true);
        assert_eq!(h.unanswered_visible, seconds(4));
        h.acknowledge_if(2, true);
        assert_eq!(h.unanswered_visible, Duration::ZERO);
        assert_eq!(h.check(seconds(8), true), Check::Ping(3));
        for at in [10, 12, 14] {
            assert_eq!(h.check(seconds(at), true), Check::Ping(3));
        }
        assert_eq!(h.check(seconds(16), true), Check::Reload);
    }

    #[test]
    fn occlusion_pauses_budget_without_erasing_earlier_evidence() {
        let mut h = Health::default();
        h.check(seconds(0), true);
        h.check(seconds(2), true);
        h.defer(1);
        for at in [4, 6, 8, 10, 12] {
            let Check::Ping(nonce) = h.check(seconds(at), true) else {
                panic!("occlusion triggered repair")
            };
            h.defer(nonce);
        }
        assert_eq!(h.unanswered_visible, seconds(2));
        assert_eq!(h.check(seconds(14), true), Check::Ping(7));
        h.check(seconds(16), true);
        h.check(seconds(18), true);
        assert_eq!(h.check(seconds(20), true), Check::Reload);
    }

    #[test]
    fn resume_discards_unanswered_time_without_charging_sleep() {
        let mut h = Health::default();
        h.check(seconds(0), true);
        h.check(seconds(2), true);
        h.check(seconds(4), false);
        assert_eq!(h.unanswered_visible, seconds(4));
        assert_eq!(h.check(seconds(60), true), Check::Ping(2));
        assert_eq!(h.unanswered_visible, Duration::ZERO);
        for at in [62, 64, 66] {
            assert_eq!(h.check(seconds(at), true), Check::Ping(2));
        }
        assert_eq!(h.check(seconds(68), true), Check::Reload);
    }
}
