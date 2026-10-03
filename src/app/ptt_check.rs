//! Bounded receive qualification before a Dual Watch transmission.
//!
//! Qualification requires a matching settled RX profile and advancing
//! receiver sample numbers. A detected carrier rejects the attempt even
//! when CTCSS/DCS keeps the speaker closed.
//!
//! At least 120 observed ticks must separate the first accepted observation
//! from a later new sample. A gap over 100 ticks restarts the quiet interval.
//! Repeated reads of one sample never complete qualification. Counter
//! saturation therefore fails closed instead of authorizing stale data.
//!
//! The overall 1000-tick deadline is fixed at the PTT attempt. Retuning,
//! delayed samples, and restarted quiet intervals cannot extend it.
//! The RF owner separately checks policy, battery, calibration, cancellation,
//! and immutable attempt parameters. This object performs no device access.
//!
//! These are observations of the foreground receiver, not proof that a
//! channel will remain idle after the final sample or during transmission.

use super::time::reached;

pub(crate) struct Check {
    deadline: u32,
    since: u32,
    last_tick: u32,
    sample: u32,
    observing: bool,
}

impl Check {
    pub(crate) const fn new(now: u32) -> Self {
        Self {
            deadline: now.wrapping_add(1000),
            since: 0,
            last_tick: 0,
            sample: 0,
            observing: false,
        }
    }

    pub(crate) const fn poll(
        &mut self,
        now: u32,
        settled: bool,
        carrier: bool,
        sample: u32,
    ) -> Result<bool, u32> {
        if reached(now, self.deadline) {
            return Err(0x700E);
        }

        if !settled {
            self.observing = false;
            return Ok(false);
        }

        if carrier {
            return Err(0x7005);
        }

        if !self.observing {
            self.observing = true;
            self.since = now;
            self.last_tick = now;
            self.sample = sample;
            return Ok(false);
        }

        if sample != self.sample {
            if now.wrapping_sub(self.last_tick) > 100 {
                self.since = now;
            }
            self.last_tick = now;
            self.sample = sample;
            return Ok(now.wrapping_sub(self.since) >= 120);
        }

        if now.wrapping_sub(self.last_tick) > 100 {
            self.observing = false;
        }
        Ok(false)
    }
}

const _: () = {
    let mut check = Check::new(0);
    assert!(matches!(check.poll(0, false, true, 7), Ok(false)));
    assert!(matches!(check.poll(50, true, false, 8), Ok(false)));
    assert!(matches!(check.poll(70, true, false, 9), Ok(false)));
    assert!(matches!(check.poll(150, true, false, 10), Ok(false)));
    assert!(matches!(check.poll(169, true, false, 11), Ok(false)));
    assert!(matches!(check.poll(170, true, false, 12), Ok(true)));

    let mut busy = Check::new(0);
    assert!(matches!(busy.poll(0, true, false, 1), Ok(false)));
    assert!(matches!(busy.poll(60, true, true, 4), Err(0x7005)));

    let mut stale = Check::new(0);
    assert!(matches!(stale.poll(0, true, false, 1), Ok(false)));
    assert!(matches!(stale.poll(120, true, false, 1), Ok(false)));
    assert!(matches!(stale.poll(121, true, false, 1), Ok(false)));
    assert!(matches!(stale.poll(200, true, false, 2), Ok(false)));
    assert!(matches!(stale.poll(241, true, false, 3), Ok(true)));

    let mut gap = Check::new(0);
    assert!(matches!(gap.poll(0, true, false, 1), Ok(false)));
    assert!(matches!(gap.poll(200, true, false, 2), Ok(false)));
    assert!(matches!(gap.poll(300, true, false, 3), Ok(false)));
    assert!(matches!(gap.poll(320, true, false, 4), Ok(true)));

    let mut retuned = Check::new(0);
    assert!(matches!(retuned.poll(0, true, false, 1), Ok(false)));
    assert!(matches!(retuned.poll(80, false, false, 2), Ok(false)));
    assert!(matches!(retuned.poll(100, true, false, 3), Ok(false)));
    assert!(matches!(retuned.poll(200, true, false, 4), Ok(false)));
    assert!(matches!(retuned.poll(220, true, false, 5), Ok(true)));
    assert!(matches!(retuned.poll(1000, true, false, 6), Err(0x700E)));

    let mut wrapped = Check::new(u32::MAX - 50);
    assert!(matches!(wrapped.poll(u32::MAX - 50, true, false, 1), Ok(false)));
    assert!(matches!(wrapped.poll(9, true, false, 2), Ok(false)));
    assert!(matches!(wrapped.poll(69, true, false, 3), Ok(true)));
    assert!(matches!(wrapped.poll(949, true, false, 4), Err(0x700E)));
};