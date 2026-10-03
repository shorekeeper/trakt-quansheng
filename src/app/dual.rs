//! Saved Dual Watch selection over one physical receiver.
//!
//! enabled is the persistent user choice. selected and listening are runtime
//! indices only: selected follows the user's tuning and TX selection, while
//! listening is the temporary RX destination. Switching reception never
//! mutates either profile or the settings baseline.
//!
//! An eligible silent profile dwells for 300 observed ticks after settling.
//! Carrier SQL holds that destination independently of RX-signaling audio.
//! Switching can resume 500 ticks after the last observed open carrier.
//! An unsettled receiver starts a fresh dwell; tune latency is not consumed
//! as time spent listening to a settled profile.
//!
//! prepare runs before request construction. PTT and engaged TX pin reception
//! to the selected VFO. Ineligible UI/RF modes park there without clearing
//! enabled. Load applies the saved boolean through set, discarding transient
//! timers and positions. Defaults and Reset RAM construct disabled state.
//!
//! This owner performs no I/O and cannot authorize transmission. Selected-VFO
//! PTT qualification belongs to its own bounded state machine and cannot
//! reuse the other VFO's carrier history.

use super::time::reached;

#[derive(Clone, Copy)]
enum Phase {
    Parked,
    Wait,
    Dwell,
    Hold,
}

pub(super) struct Watch {
    enabled: bool,
    selected: usize,
    listening: usize,
    phase: Phase,
    deadline: u32,
}

impl Watch {
    pub(super) const fn new() -> Self {
        Self {
            enabled: false,
            selected: 0,
            listening: 0,
            phase: Phase::Parked,
            deadline: 0,
        }
    }

    pub(super) const fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) const fn set(&mut self, enabled: bool) {
        *self = Self::new();
        self.enabled = enabled;
    }

    pub(super) const fn listening(&self) -> usize {
        self.listening
    }

    pub(super) const fn prepare(&mut self, selected: usize, eligible: bool, pinned: bool) {
        assert!(selected < 2);

        if self.selected != selected {
            self.selected = selected;
            self.listening = selected;
            self.phase = Phase::Wait;
        }

        if !self.enabled || !eligible || pinned {
            self.listening = selected;
            self.phase = Phase::Parked;
        } else if matches!(self.phase, Phase::Parked) {
            self.listening = selected;
            self.phase = Phase::Wait;
        }
    }

    pub(super) const fn observe(&mut self, now: u32, settled: bool, carrier: bool) {
        if !self.enabled || matches!(self.phase, Phase::Parked) {
            return;
        }

        if !settled {
            self.phase = Phase::Wait;
            return;
        }

        if carrier {
            self.phase = Phase::Hold;
            self.deadline = now.wrapping_add(500);
            return;
        }

        if matches!(self.phase, Phase::Wait) {
            self.phase = Phase::Dwell;
            self.deadline = now.wrapping_add(300);
            return;
        }

        if reached(now, self.deadline) {
            self.listening ^= 1;
            self.phase = Phase::Wait;
        }
    }

    pub(super) const fn status(&self) -> &'static [u8] {
        match self.phase {
            Phase::Parked => b"DW PAUSED",
            Phase::Wait => b"DW TUNING",
            Phase::Dwell => b"DW WATCHING",
            Phase::Hold => b"DW CARRIER HOLD",
        }
    }
}

const _: () = {
    let mut watch = Watch::new();
    assert!(!watch.enabled());
    watch.prepare(1, true, false);
    watch.observe(1000, true, false);
    assert!(watch.listening() == 1);

    watch.set(true);
    watch.prepare(0, true, false);
    watch.observe(0, false, false);
    watch.observe(50, true, false);
    watch.observe(349, true, false);
    assert!(watch.listening() == 0);
    watch.observe(350, true, false);
    assert!(watch.listening() == 1);

    // The next destination waits for its own receive configuration.
    watch.observe(351, false, false);
    watch.observe(400, true, true);
    watch.observe(600, true, true);
    watch.observe(1099, true, false);
    assert!(watch.listening() == 1);
    watch.observe(1100, true, false);
    assert!(watch.listening() == 0);

    watch.observe(1150, true, false);
    watch.observe(1450, true, false);
    assert!(watch.listening() == 1);

    // PTT cannot inherit the other VFO's receive destination.
    watch.prepare(0, true, true);
    assert!(watch.listening() == 0);
    watch.observe(5000, true, false);
    assert!(watch.listening() == 0);
    watch.prepare(0, true, false);
    watch.observe(5001, true, false);
    watch.observe(5300, true, false);
    assert!(watch.listening() == 0);
    watch.observe(5301, true, false);
    assert!(watch.listening() == 1);

    // Ineligibility preserves the preference but parks on selected RX.
    watch.prepare(0, false, false);
    assert!(watch.enabled() && watch.listening() == 0);
    watch.observe(9000, true, false);
    assert!(watch.listening() == 0);

    watch.prepare(1, true, false);
    assert!(watch.listening() == 1);
    watch.observe(9001, true, false);
    watch.observe(9301, true, false);
    assert!(watch.listening() == 0);

    let mut wrapped = Watch::new();
    wrapped.set(true);
    wrapped.prepare(0, true, false);
    wrapped.observe(u32::MAX - 100, true, false);
    wrapped.observe(198, true, false);
    assert!(wrapped.listening() == 0);
    wrapped.observe(199, true, false);
    assert!(wrapped.listening() == 1);
};