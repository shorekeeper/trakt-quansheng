//! One bounded straight-key attack retained across transmitter preparation.
//!
//! A new admitted contact request arms one capture. If the contact is still
//! held when TX becomes ready, output follows the physical contact normally.
//! If it was released during preparation, the captured duration is replayed
//! once after readiness. Captured duration is limited to 5..2000 ticks.
//!
//! This is deliberately one initial mark, not a queue of arbitrary straight
//! key activity. Additional closures during preparation cannot accumulate an
//! unbounded delayed message. Electronic paddle memory belongs to Keyer and
//! does not use this object.
//!
//! The application cancels this capture on input loss, errors, settings or
//! profile changes, and session cancellation. A retained attack grants no TX
//! permission and cannot extend the transmitter's original complete-session
//! timeout. Training and radiated CW use the same capture behavior.
//!
//! All durations are observed ticks. Readiness is supplied by the concrete
//! RF/audio owner; no register, peripheral, heap, or static buffer is owned.

use crate::app::time::reached;

pub(crate) struct Lead {
    state: u8,
    started: u32,
    duration: u32,
    until: u32,
}

impl Lead {
    pub(crate) const fn new() -> Self {
        Self { state: 0, started: 0, duration: 0, until: 0 }
    }

    pub(crate) const fn arm(&mut self, now: u32) {
        self.state = 1;
        self.started = now;
        self.duration = 0;
        self.until = 0;
    }

    /// ready false records release but cannot produce a mark. State zero
    /// follows live input; state one captures; two holds a completed attack;
    /// three plays that attack once. A fresh held contact takes precedence
    /// over the remaining replay, without producing another queued attack.
    pub(crate) const fn poll(&mut self, now: u32, held: bool, ready: bool) -> bool {
        if self.state == 1 && !held {
            let elapsed = now.wrapping_sub(self.started);
            self.duration = if elapsed < 5 {
                5
            } else if elapsed > 2000 {
                2000
            } else {
                elapsed
            };
            self.state = 2;
        }

        if !ready {
            return false;
        }

        if held {
            self.state = 0;
            return true;
        }

        if self.state == 2 {
            self.until = now.wrapping_add(self.duration);
            self.state = 3;
        }

        if self.state == 3 {
            if !reached(now, self.until) {
                return true;
            }
            self.state = 0;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_attack_survives_setup_and_plays_exactly_once() {
        let mut lead = Lead::new();
        lead.arm(10);
        assert!(!lead.poll(10, true, false));
        assert!(!lead.poll(40, false, false));
        assert!(!lead.poll(200, false, false));
        assert!(lead.poll(300, false, true));
        assert!(lead.poll(329, false, true));
        assert!(!lead.poll(330, false, true));
        assert!(!lead.poll(400, false, true));
    }

    #[test]
    fn a_held_contact_at_readiness_follows_live_release() {
        let mut lead = Lead::new();
        lead.arm(0);
        assert!(!lead.poll(100, true, false));
        assert!(lead.poll(200, true, true));
        assert!(!lead.poll(201, false, true));
        assert!(!lead.poll(300, false, true));
    }

    #[test]
    fn capture_is_bounded_and_cancellation_drops_it() {
        let mut lead = Lead::new();
        lead.arm(0);
        assert!(!lead.poll(5000, false, false));
        assert!(lead.poll(6000, false, true));
        assert!(!lead.poll(8000, false, true));

        lead.arm(9000);
        assert!(!lead.poll(9010, false, false));
        lead = Lead::new();
        assert!(!lead.poll(9500, false, true));
    }

    #[test]
    fn a_new_live_closure_does_not_queue_a_second_attack() {
        let mut lead = Lead::new();
        lead.arm(0);
        assert!(!lead.poll(30, false, false));
        assert!(!lead.poll(100, true, false));
        assert!(lead.poll(200, true, true));
        assert!(!lead.poll(201, false, true));
        assert!(!lead.poll(300, false, true));
    }

    #[test]
    fn capture_and_replay_cross_tick_wrap() {
        let mut lead = Lead::new();
        lead.arm(u32::MAX - 10);
        assert!(!lead.poll(9, false, false));
        assert!(lead.poll(u32::MAX - 5, false, true));
        assert!(lead.poll(13, false, true));
        assert!(!lead.poll(14, false, true));
    }
}