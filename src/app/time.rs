//! Wrapping deadlines and phase-preserving periodic work.
//!
//! Comparisons are unambiguous only when the separation is below half
//! the u32 counter range. With millisecond ticks, all callers must resume
//! within that range; a stopped counter does not advance deadlines.
//!
//! Late periodic work runs once. Its next deadline remains on the original
//! phase, and the return value includes the skipped occurrences.

pub(crate) const fn reached(now: u32, deadline: u32) -> bool {
    now.wrapping_sub(deadline) < 0x8000_0000
}

pub(crate) struct Periodic {
    next: u32,
    period: u32,
}

impl Periodic {
    pub(crate) const fn new(now: u32, period: u32) -> Self {
        assert!(period > 0 && period < 0x8000_0000);
        Self {
            next: now.wrapping_add(period),
            period,
        }
    }

    /// Returns zero before the deadline, otherwise the number of elapsed
    /// occurrences. The caller executes one and accounts for the rest.
    pub(crate) const fn poll(&mut self, now: u32) -> u32 {
        if !reached(now, self.next) {
            return 0;
        }
        let count = now.wrapping_sub(self.next) / self.period + 1;
        self.next = self.next.wrapping_add(count.wrapping_mul(self.period));
        count
    }
}

const _: () = {
    assert!(reached(10, 10));
    assert!(!reached(9, 10));
    assert!(reached(3, 0xFFFF_FFFE));
    assert!(!reached(0xFFFF_FFFE, 3));
    assert!(!reached(0x8000_0000, 0));

    let mut periodic = Periodic::new(0, 10);
    assert!(periodic.poll(9) == 0);
    assert!(periodic.poll(10) == 1);
    assert!(periodic.poll(35) == 2);
    assert!(periodic.next == 40);
    assert!(periodic.poll(40) == 1);

    let mut wrapped = Periodic::new(0xFFFF_FFFC, 10);
    assert!(wrapped.next == 6);
    assert!(wrapped.poll(0xFFFF_FFFF) == 0);
    assert!(wrapped.poll(6) == 1);
    assert!(wrapped.next == 16);

    let mut catch_up = Periodic::new(0, 7);
    assert!(catch_up.poll(28_679) == 4_097);
    assert!(catch_up.next == 28_686);
};