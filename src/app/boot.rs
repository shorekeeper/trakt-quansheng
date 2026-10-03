//! Boot presentation over the existing foreground acquisition work.
//!
//! LCD, calibration, journal loading, and the first battery result progress
//! while this screen is visible. The static font requires no rasterization
//! cache. Loading makes no automatic storage write.
//!
//! Normal controls and RF admission wait until the four tasks terminate
//! and the screen has been visible for at least 800 nominal ticks.
//! A five-second upper bound releases the UI with a warning instead of
//! concealing failed acquisition behind an endless progress animation.
//! RF and storage drivers retain their independent error handling.

use crate::app::time::reached;
use crate::drivers::adc::Reading;
use crate::storage::View;
use crate::ui::{font::Size, widgets, Frame};

pub(super) struct Boot {
    active: bool,
    completed: u32,
    errors: u32,
    timed_out: bool,
}

impl Boot {
    pub(super) const fn new() -> Self {
        Self { active: true, completed: 0, errors: 0, timed_out: false }
    }

    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// Returns true once when the UI may leave the boot presentation.
    pub(super) fn poll(
        &mut self,
        now: u32,
        lcd_ready: bool,
        memory: View,
        journal_busy: bool,
        battery: Reading,
    ) -> bool {
        if !self.active { return false; }

        if lcd_ready { self.completed |= 1; }
        if memory.ready || memory.error != 0 { self.completed |= 2; }
        if memory.error != 0 { self.errors |= 2; }
        if !journal_busy { self.completed |= 4; }
        if battery.average.is_some() || battery.error != 0 { self.completed |= 8; }
        if battery.error != 0 { self.errors |= 8; }

        self.timed_out = reached(now, 5_000) && self.completed != 15;
        if (self.completed == 15 && reached(now, 800)) || self.timed_out {
            self.active = false;
            return true;
        }
        false
    }

    pub(super) fn render(&self, frame: &mut Frame) {
        frame.clear();
        frame.text_size(34, 7, b"TRAKT", Size::Large);
        frame.text_size(12, 27, crate::build_info::LABEL, Size::Small);
        frame.text_size(12, 35, crate::build_info::VERSION, Size::Small);

        let count = self.completed.count_ones();
        widgets::bar(frame, 10, 44, 108, 6, count, 4);
        frame.text_size(10, 54, if self.timed_out || self.errors != 0 {
            b"STARTUP HAS WARNINGS"
        } else {
            b"LCD CAL SETTINGS ADC"
        }, Size::Small);
    }
}