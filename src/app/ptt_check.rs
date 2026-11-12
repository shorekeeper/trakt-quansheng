//! Bounded receive qualification before a queued transmission.
//!
//! Dual Watch requires the advancing-sample quiet interval described below.
//! CW instead consumes a fresh settled AUTO-gain observation with immediate
//! conservative RSSI occupancy. Its separate poll_cw retains the fixed
//! acquisition deadline without adding another quiet dwell after RX settles.
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

    /// CW caller supplies an exact settled profile with a fresh AUTO-gain
    /// RSSI observation. Carrier includes an immediate conservative raw-RSSI
    /// comparison supplied by Receiver, not only delayed SQL opening.
    ///
    /// No additional continuous-quiet interval is imposed here. The fixed
    /// acquisition deadline still applies while waiting for RX restoration.
    /// Dual Watch retains poll and its independent 120-tick quiet interval.
    #[cfg(feature = "cw")]
    pub(crate) const fn poll_cw(
        &self,
        now: u32,
        settled: bool,
        carrier: bool,
    ) -> Result<bool, u32> {
        if reached(now, self.deadline) {
            Err(0x700E)
        } else if !settled {
            Ok(false)
        } else if carrier {
            Err(0x7005)
        } else {
            Ok(true)
        }
    }

    /// VOX consumes a fresh exact selected-RX observation. Receiver supplies
    /// immediate occupancy from its complete closing-threshold conditions
    /// as well as established SQL. No extra continuous-quiet dwell is added.
    /// The original fixed acquisition deadline still bounds restoration.
    pub(crate) const fn poll_vox(
        &self,
        now: u32,
        settled: bool,
        carrier: bool,
    ) -> Result<bool, u32> {
        if reached(now, self.deadline) {
            Err(0x700E)
        } else if !settled {
            Ok(false)
        } else if carrier {
            Err(0x7005)
        } else {
            Ok(true)
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

#[cfg(feature = "cw")]
const _: () = {
    let check = Check::new(100);

    // Invalid or unsettled RX cannot authorize transmission.
    assert!(matches!(check.poll_cw(100, false, false), Ok(false)));
    assert!(matches!(check.poll_cw(101, false, true), Ok(false)));

    // A fresh settled observation needs no additional quiet dwell.
    assert!(matches!(check.poll_cw(102, true, false), Ok(true)));
    assert!(matches!(check.poll_cw(102, true, true), Err(0x7005)));

    // Delayed restoration cannot renew the original acquisition deadline.
    assert!(matches!(check.poll_cw(1099, true, false), Ok(true)));
    assert!(matches!(check.poll_cw(1100, true, false), Err(0x700E)));
    assert!(matches!(check.poll_cw(1100, false, false), Err(0x700E)));

    let wrapped = Check::new(u32::MAX - 50);
    assert!(matches!(wrapped.poll_cw(0, true, false), Ok(true)));
    assert!(matches!(wrapped.poll_cw(948, false, false), Ok(false)));
    assert!(matches!(wrapped.poll_cw(949, true, false), Err(0x700E)));
};

const _: () = {
    let immediate = Check::new(100);
    assert!(matches!(immediate.poll_vox(100, false, false), Ok(false)));
    assert!(matches!(immediate.poll_vox(101, true, false), Ok(true)));
    assert!(matches!(immediate.poll_vox(101, true, true), Err(0x7005)));
    assert!(matches!(immediate.poll_vox(1100, true, false), Err(0x700E)));

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
