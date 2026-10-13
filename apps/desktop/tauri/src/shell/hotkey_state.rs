//! Pure physical-key state machine. No input text is retained.
#[derive(Default)]
pub struct ChordState {
    down: std::collections::BTreeSet<i32>,
    active: Option<Vec<i32>>,
}
#[derive(Debug, PartialEq)]
pub enum Edge {
    Press,
    Release,
}
pub fn normalized(key: i32) -> i32 {
    match key {
        0xa0 | 0xa1 => 0x10,
        0xa2 | 0xa3 => 0x11,
        0xa4 | 0xa5 => 0x12,
        0x5c => 0x5b,
        _ => key,
    }
}
impl ChordState {
    #[cfg(any(windows, target_os = "macos", test))]
    pub fn reset(&mut self) -> bool {
        let active = self.active.is_some();
        *self = Self::default();
        active
    }

    #[cfg(any(windows, test))]
    pub fn update(&mut self, key: i32, pressed: bool, chord: &[i32]) -> Option<Edge> {
        if pressed {
            if !self.down.insert(key) {
                return None;
            }
        } else {
            self.down.remove(&key);
        }
        self.edge(pressed, chord)
    }

    /// Atomically reconcile Mac modifier flags. This includes modifiers held
    /// before the tap started, and avoids transient chords between flag updates.
    #[cfg(target_os = "macos")]
    pub fn update_with_modifiers(
        &mut self,
        key: i32,
        pressed: bool,
        modifiers: &[i32],
        chord: &[i32],
    ) -> Option<Edge> {
        let was_down = self.down.contains(&key);
        self.down
            .retain(|key| !matches!(normalized(*key), 0x10 | 0x11 | 0x12 | 0x5b));
        self.down.extend(modifiers);
        if pressed {
            self.down.insert(key);
        } else {
            self.down.remove(&key);
        }
        self.edge(pressed && !was_down, chord)
    }

    fn edge(&mut self, pressed: bool, chord: &[i32]) -> Option<Edge> {
        let down: std::collections::BTreeSet<_> =
            self.down.iter().map(|k| normalized(*k)).collect();
        if let Some(active) = &self.active {
            if active.iter().all(|key| !down.contains(key)) {
                self.active = None;
                return Some(Edge::Release);
            }
        } else if pressed
            && !chord.is_empty()
            && down.len() == chord.len()
            && chord.iter().all(|key| down.contains(key))
        {
            self.active = Some(chord.to_vec());
            return Some(Edge::Press);
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn fn_edges_repeat_release_and_tap_recovery() {
        let mut state = ChordState::default();
        assert_eq!(state.update(0xff, true, &[0xff]), Some(Edge::Press));
        assert_eq!(state.update(0xff, true, &[0xff]), None);
        assert_eq!(state.update(0xff, false, &[0xff]), Some(Edge::Release));
        assert!(!state.reset());
        state.update(0xff, true, &[0xff]);
        assert!(state.reset());
        assert_eq!(state.update(0xff, true, &[0xff]), Some(Edge::Press));
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn initial_mac_flags_prevent_a_false_bare_fn_chord() {
        let mut state = ChordState::default();
        assert_eq!(
            state.update_with_modifiers(0xff, true, &[0xa2], &[0xff]),
            None
        );
        assert_eq!(state.update_with_modifiers(0xa2, false, &[], &[0xff]), None);
        assert_eq!(state.update_with_modifiers(0xff, false, &[], &[0xff]), None);
        assert_eq!(
            state.update_with_modifiers(0xff, true, &[], &[0xff]),
            Some(Edge::Press)
        );
    }
    #[test]
    fn idle_recovery_clears_lost_release_and_unrelated_keys() {
        let mut state = ChordState::default();
        state.update(0x41, true, &[0x5b, 0x11]);
        assert!(!state.reset());
        state.update(0x5b, true, &[0x5b, 0x11]);
        assert_eq!(state.update(0x11, true, &[0x5b, 0x11]), Some(Edge::Press));
        assert!(state.reset());
        assert!(!state.reset());
        state.update(0x11, true, &[0x5b, 0x11]);
        assert_eq!(state.update(0x5b, true, &[0x5b, 0x11]), Some(Edge::Press));
    }
    #[test]
    fn either_order_and_release_order() {
        for keys in [[0x5b, 0xa4], [0xa5, 0x5c]] {
            let mut s = ChordState::default();
            assert_eq!(s.update(keys[0], true, &[0x5b, 0x12]), None);
            assert_eq!(s.update(keys[1], true, &[0x5b, 0x12]), Some(Edge::Press));
            assert_eq!(s.update(keys[1], true, &[0x5b, 0x12]), None);
            assert_eq!(s.update(keys[0], false, &[0x5b, 0x12]), None);
            assert_eq!(s.update(keys[1], false, &[0x5b, 0x12]), Some(Edge::Release));
        }
    }
    #[test]
    fn extra_modifier_does_not_trigger_and_both_sides_release() {
        let mut s = ChordState::default();
        s.update(0x11, true, &[0x5b, 0x12]);
        s.update(0x5b, true, &[0x5b, 0x12]);
        assert_eq!(s.update(0x12, true, &[0x5b, 0x12]), None);
        let mut s = ChordState::default();
        s.update(0x5b, true, &[0x5b, 0x12]);
        assert_eq!(s.update(0xa4, true, &[0x5b, 0x12]), Some(Edge::Press));
        s.update(0xa5, true, &[0x5b, 0x12]);
        s.update(0xa4, false, &[0x5b, 0x12]);
        assert_eq!(s.update(0x5b, false, &[0x5b, 0x12]), None);
        assert_eq!(s.update(0xa5, false, &[0x5b, 0x12]), Some(Edge::Release));
    }
}
