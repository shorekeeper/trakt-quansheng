//! Live RSSI envelope over time, with no receiver frequency sweep.
//!
//! Sixty-four columns retain raw nine-bit observations at no more than one
//! sample every twenty observed ticks. A full window spans approximately
//! 1.28 seconds. Samples come from the existing CW receiver; drawing makes
//! no register reads, retunes, or independent carrier decisions.
//!
//! Repeated sample timestamps cannot advance the plot. Unavailable RX or a
//! gap above one hundred ticks inserts a blank discontinuity. Such gaps are
//! not joined or presented as measured silence. The plot is cleared on open.
//!
//! Vertical limits include the displayed threshold and all retained samples,
//! with a minimum span of 32 raw counts. A dashed line marks the opening
//! threshold; the four-count closing hysteresis remains in Controls.
//! Vertical scaling is a presentation aid, not calibrated RF power.
//!
//! Arrows change the selected control immediately in RAM. Menu selects
//! threshold, RF table choice, or sidetone volume. F selects one/five-count
//! adjustment for threshold and volume; RF table choice always steps by one.
//! Exit returns Home. There is no TX admission on this screen.
//!
//! This object owns 128 sample bytes, not a framebuffer or PCM buffer.

use crate::cw_controls::Controls;
use crate::ui::{font::Size, widgets, Frame};

const EMPTY: u16 = u16::MAX;

pub(super) struct Scope {
    samples: [u16; 64],
    head: usize,
    last: u32,
    sampled: bool,
    broken: bool,
    pub(super) field: usize,
    pub(super) coarse: bool,
}

impl Scope {
    pub(super) const fn new() -> Self {
        Self {
            samples: [EMPTY; 64], head: 0, last: 0,
            sampled: false, broken: true, field: 0, coarse: false,
        }
    }

    pub(super) fn open(&mut self) {
        *self = Self::new();
    }

    fn push(&mut self, value: u16) {
        self.samples[self.head] = value;
        self.head = (self.head + 1) & 63;
    }

    pub(super) fn observe(&mut self, sample: Option<(u32, u32)>) {
        let (tick, rssi) = match sample {
            Some(value) => value,
            None => {
                if !self.broken {
                    self.push(EMPTY);
                }
                self.broken = true;
                return;
            }
        };

        if self.sampled && tick.wrapping_sub(self.last) < 20 {
            return;
        }
        if self.sampled && tick.wrapping_sub(self.last) > 100 && !self.broken {
            self.push(EMPTY);
        }
        self.push(rssi.min(511) as u16);
        self.last = tick;
        self.sampled = true;
        self.broken = false;
    }

    // Scope drawing borrows the sole framebuffer outside Panel composition.
    #[inline(never)]
    pub(super) fn render(&self, frame: &mut Frame, controls: Controls) {
        frame.clear();
        frame.text_size(2, 1, b"CW RSSI / TIME", Size::Small);
        frame.text_size(86, 1, b"NO RF", Size::Small);
        frame.fill(0, 8, 128, 1, true);

        let selected = match self.field {
            0 => b"THRESHOLD" as &[u8],
            1 => b"RF GAIN",
            _ => b"TONE VOLUME",
        };
        frame.text_size(2, 11, selected, Size::Small);
        if self.field == 1 {
            frame.text_size(
                66, 11, crate::cw_controls::gain_name(controls.gain as u32),
                Size::Small,
            );
        } else {
            widgets::number(
                frame, 66, 11,
                if self.field == 0 {
                    controls.threshold as u32
                } else {
                    controls.volume as u32
                },
                Size::Small,
            );
        }
        frame.text_size(
            98, 11, if self.coarse { b"STEP 5" } else { b"STEP 1" },
            Size::Small,
        );

        let mut lower = controls.threshold.saturating_sub(16) as u32;
        let mut upper = (controls.threshold as u32 + 16).min(511);
        for &sample in &self.samples {
            if sample != EMPTY {
                lower = lower.min(sample as u32);
                upper = upper.max(sample as u32);
            }
        }
        let span = upper.saturating_sub(lower).max(32);

        let threshold_y = 47 - widgets::extent(
            (controls.threshold as u32).saturating_sub(lower), span, 26,
        ) as i16;
        for x in (0..128).step_by(4) {
            frame.fill(x, threshold_y, 2, 1, true);
        }

        for column in 0..64 {
            let value = self.samples[(self.head + column) & 63];
            if value == EMPTY {
                continue;
            }
            let height = widgets::extent(
                (value as u32).saturating_sub(lower), span, 26,
            ) as u16 + 1;
            frame.fill(column as i16 * 2, 48 - height as i16, 1, height, true);
        }

        frame.text_size(2, 50, b"TH", Size::Small);
        widgets::number(frame, 14, 50, controls.threshold as u32, Size::Small);
        frame.text_size(38, 50, b"RF", Size::Small);
        frame.text_size(
            50, 50, crate::cw_controls::gain_name(controls.gain as u32),
            Size::Small,
        );
        frame.text_size(82, 50, b"VOL", Size::Small);
        widgets::number(frame, 100, 50, controls.volume as u32, Size::Small);
        frame.text_size(2, 58, b"ARROWS / MENU FIELD / F / EXIT", Size::Small);
    }
}