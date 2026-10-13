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
    pending: Option<(u64, Duration)>,
    nonce: u64,
    failures: u32,
    presentation_failed: bool,
}
impl Health {
    pub fn acknowledge_if(&mut self, nonce: u64, presented: bool) {
        if self.pending.is_some_and(|(expected, _)| expected == nonce) {
            self.presentation_failed = !presented;
            if presented {
                self.pending = None;
                self.failures = 0;
            }
        }
    }

    pub fn presentation_failed(&self) -> bool {
        self.presentation_failed
    }

    pub fn check(&mut self, now: Duration, visible: bool) -> Check {
        let resumed = self
            .last_tick
            .replace(now)
            .is_some_and(|last| now.saturating_sub(last) > SUSPENDED);
        if !visible || resumed {
            self.pending = None;
            self.presentation_failed = false;
        }
        if !visible {
            return Check::Idle;
        }
        if let Some((nonce, since)) = self.pending {
            if now.saturating_sub(since) < TIMEOUT {
                // Retrying the same challenge also covers a document whose
                // listeners were not ready at the first send. Keep its deadline.
                return Check::Ping(nonce);
            }
            self.failures = self.failures.saturating_add(1);
            // Leave a full grace period for the replacement document to load.
            self.pending = None;
            return if self.failures == 1 {
                Check::Reload
            } else {
                Check::Recreate
            };
        }
        self.nonce += 1;
        self.pending = Some((self.nonce, now));
        self.presentation_failed = false;
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
}
