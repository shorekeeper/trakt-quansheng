//! Expiring TX error presentation, separate from the diagnostic code.
//!
//! The transmitter owns the actual last outcome and PTT qualification.
//! report records only whether that outcome needs a visible notification
//! and its fixed deadline. Nonzero outcomes are visible for 2000 observed
//! ticks; zero dismisses the presentation. Reporting is tied to attempt
//! outcomes, not to held input or repeated screen reads.
//!
//! current filters the diagnostic code without changing it. It accepts
//! the current tick so a temporarily unpumped transmitter cannot freeze
//! the visible lifetime during another foreground owner's bounded work.
//! poll permanently retires an expired notification, preventing it from
//! reappearing when the u32 tick counter eventually wraps.
//!
//! Deadlines have the same half-counter-range observation requirement as
//! other foreground timers. Observed ticks do not account for SysTick
//! periods lost while interrupts are masked.
//!
//! No method changes transmitter phase, release history, RF controls,
//! sound playback, saved settings, or the diagnostic error field.

use super::time::reached;

pub(crate) struct Notice {
    deadline: u32,
    active: bool,
}

impl Notice {
    pub(crate) const fn new() -> Self {
        Self { deadline: 0, active: false }
    }

    /// Called for a new attempt outcome, never to refresh an existing one.
    pub(crate) const fn report(&mut self, now: u32, code: u32) {
        self.active = code != 0;
        self.deadline = now.wrapping_add(2000);
    }

    pub(crate) const fn poll(&mut self, now: u32) {
        if self.active && reached(now, self.deadline) {
            self.active = false;
        }
    }

    /// Zero hides the UI error. The supplied diagnostic code is not erased.
    pub(crate) const fn current(&self, now: u32, code: u32) -> u32 {
        if self.active && !reached(now, self.deadline) {
            code
        } else {
            0
        }
    }
}

const _: () = {
    let errors = [0x700F, 0x7001, 0x7005, 0x7004, 0x7006, 0x7601];
    let mut index = 0;
    while index < errors.len() {
        let code = errors[index];
        let mut notice = Notice::new();
        assert!(notice.current(0, code) == 0);

        notice.report(100, code);
        assert!(notice.current(100, code) == code);
        assert!(notice.current(2099, code) == code);
        assert!(notice.current(2100, code) == 0);

        // Repeated observation does not renew the deadline.
        notice.poll(200);
        notice.poll(1000);
        notice.poll(2099);
        assert!(notice.current(2100, code) == 0);
        notice.poll(2100);
        assert!(!notice.active);

        // Even an eventual return to the old counter values stays hidden.
        assert!(notice.current(100, code) == 0);

        // Only a new reported outcome starts another lifetime.
        notice.report(3000, code);
        assert!(notice.current(4999, code) == code);
        assert!(notice.current(5000, code) == 0);
        notice.report(3100, 0);
        assert!(notice.current(3100, code) == 0);

        index += 1;
    }

    let mut wrapped = Notice::new();
    let start = u32::MAX - 100;
    wrapped.report(start, 0x700F);
    assert!(wrapped.current(1898, 0x700F) == 0x700F);
    assert!(wrapped.current(1899, 0x700F) == 0);
    wrapped.poll(1899);
    assert!(wrapped.current(start, 0x700F) == 0);
};