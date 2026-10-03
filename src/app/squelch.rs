//! Timed SQL state independent of the monitor audio override.
//!
//! Opening requires all three open thresholds for 60 ticks. Closing requires
//! failure of at least one close threshold for 120 ticks. A returning signal
//! cancels the pending close; a failed opening sample cancels the pending open.
//!
//! Monitor, SQL zero, and PTT discard the detector history. Bypass may return
//! open audio, but is_open remains false. Leaving bypass therefore cannot
//! inherit its audio state as a detected carrier.
//!
//! Measurements are expected at 20-tick intervals. A gap over 100 ticks
//! discards history rather than interpreting a stale candidate as continuous.

use crate::app::time::reached;
use crate::storage::squelch::Thresholds;

#[derive(Clone, Copy)]
pub(crate) struct Metrics {
    pub(crate) rssi: u32,
    pub(crate) noise: u32,
    pub(crate) glitch: u32,
}

pub(crate) struct Gate {
    open: bool,
    candidate: bool,
    changed: u32,
    last_sample: u32,
    sampled: bool,
}

impl Gate {
    pub(crate) const fn new() -> Self {
        Self {
            open: false,
            candidate: false,
            changed: 0,
            last_sample: 0,
            sampled: false,
        }
    }

    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) const fn update(
        &mut self,
        now: u32,
        metrics: Metrics,
        thresholds: Thresholds,
        bypass: bool,
        ptt: bool,
    ) -> bool {
        if bypass || ptt {
            *self = Self::new();
            return bypass && !ptt;
        }

        if self.sampled && now.wrapping_sub(self.last_sample) > 100 {
            *self = Self::new();
        }
        self.sampled = true;
        self.last_sample = now;

        let wanted = if self.open {
            metrics.rssi >= thresholds.close_rssi
                && metrics.noise <= thresholds.close_noise
                && metrics.glitch <= thresholds.close_glitch
        } else {
            metrics.rssi >= thresholds.open_rssi
                && metrics.noise <= thresholds.open_noise
                && metrics.glitch <= thresholds.open_glitch
        };

        if wanted != self.candidate {
            self.candidate = wanted;
            self.changed = now;
        }

        let delay = if wanted { 60 } else { 120 };
        if wanted != self.open && reached(now, self.changed.wrapping_add(delay)) {
            self.open = wanted;
        }
        self.open
    }
}

const _: () = {
    let thresholds = Thresholds {
        open_rssi: 100,
        close_rssi: 90,
        open_noise: 40,
        close_noise: 60,
        open_glitch: 70,
        close_glitch: 90,
        adjusted: false,
    };
    let good = Metrics { rssi: 100, noise: 40, glitch: 70 };
    let held = Metrics { rssi: 95, noise: 50, glitch: 80 };
    let bad = Metrics { rssi: 89, noise: 100, glitch: 100 };

    let mut gate = Gate::new();
    assert!(!gate.update(0, good, thresholds, false, false));
    assert!(!gate.update(20, good, thresholds, false, false));
    assert!(!gate.update(40, good, thresholds, false, false));
    assert!(gate.update(60, good, thresholds, false, false));
    assert!(gate.is_open());
    assert!(gate.update(80, held, thresholds, false, false));

    let mut now = 100;
    while now < 220 {
        assert!(gate.update(now, bad, thresholds, false, false));
        now += 20;
    }
    assert!(!gate.update(220, bad, thresholds, false, false));

    assert!(gate.update(240, bad, thresholds, true, false));
    assert!(!gate.is_open());
    assert!(!gate.update(260, held, thresholds, false, false));
    assert!(!gate.update(280, held, thresholds, false, false));
    assert!(!gate.update(300, good, thresholds, false, false));
    assert!(!gate.update(320, good, thresholds, false, false));
    assert!(!gate.update(340, good, thresholds, false, false));
    assert!(gate.update(360, good, thresholds, false, false));

    assert!(!gate.update(380, good, thresholds, true, true));
    assert!(!gate.is_open());
    assert!(!gate.update(400, good, thresholds, false, false));
    assert!(!gate.update(420, good, thresholds, false, false));
    assert!(!gate.update(440, good, thresholds, false, false));
    assert!(gate.update(460, good, thresholds, false, false));

    assert!(!gate.update(1_000, good, thresholds, false, false));

    let mut wrapped = Gate::new();
    assert!(!wrapped.update(0xFFFF_FFF0, good, thresholds, false, false));
    assert!(!wrapped.update(4, good, thresholds, false, false));
    assert!(!wrapped.update(24, good, thresholds, false, false));
    assert!(wrapped.update(44, good, thresholds, false, false));

    let noisy = Metrics { rssi: 511, noise: 41, glitch: 0 };
    let glitchy = Metrics { rssi: 511, noise: 0, glitch: 71 };
    let mut closed = Gate::new();
    assert!(!closed.update(0, noisy, thresholds, false, false));
    assert!(!closed.update(20, glitchy, thresholds, false, false));
    assert!(!closed.update(40, noisy, thresholds, false, false));
};