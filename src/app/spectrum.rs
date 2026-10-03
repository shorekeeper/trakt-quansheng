//! Sequential, muted RSSI overview using the existing receiver.
//!
//! Up to 64 points surround the current frequency within its receive band.
//! A same-band scan range clips the window. Every point waits for Receive
//! state and then for a new receiver sample; pre-retune metrics are not used.
//! One point has a 1000-tick deadline. Failure stops the sweep.
//!
//! Sampling stores raw RSSI and noise. A point passes the independent scope
//! threshold when RSSI is at least its threshold and, for FM, noise is no
//! greater than its threshold. AM ignores the FM noise discriminator.
//! This classification does not reuse or change normal SQL.
//!
//! Plot height is relative to the observed range with a minimum span of 32
//! indicator units. It is not calibrated dBm, an FFT, or a simultaneous view.
//! The receiver is muted for the entire overview, including paused inspection.
//!
//! Menu pauses or resumes. Arrows pause and select a point; F selects the
//! strongest measured point. Side2 returns that frequency to the VFO.
//! Exit restores the original operation state. No sweep sample changes
//! saved tuning positions or starts an automatic save.

use core::ptr::addr_of_mut;

use crate::app::{preferences::Preferences, time::reached};
use crate::drivers::{
    bk4829::receive::{Phase, Request, View},
    keyboard::{Edge, Event, Key},
};
use crate::ui::{font::Size, widgets, Frame};

#[repr(C)]
struct Samples {
    rssi: [u16; 64],
    noise: [u8; 64],
    valid: [u8; 64],
}

#[used]
#[no_mangle]
static mut TRAKT_SCOPE: Samples = Samples {
    rssi: [0; 64],
    noise: [0; 64],
    valid: [0; 64],
};

pub(super) enum Effect {
    Exit,
    Tune(u32),
}

const fn step(selection: u8) -> u32 {
    match selection {
        0 => 12_500,
        1 => 25_000,
        _ => 100_000,
    }
}

const fn window(center: u32, preferences: Preferences) -> (u32, usize, u32) {
    let (path_lower, path_upper) = crate::profile::receive_bounds(center);
    let same_band = crate::profile::receive_span(preferences.lower)
        == crate::profile::receive_span(center);

    // A saved range clips only a center inside the same contiguous interval.
    // Sharing an LNA does not permit a window that crosses the RF gap.
    let configured = same_band
        && center >= preferences.lower && center <= preferences.upper;
    let lower = if configured { preferences.lower } else { path_lower };
    let upper = if configured { preferences.upper } else { path_upper };
    let spacing = step(preferences.spectrum_step);
    let available = (upper - lower) / spacing + 1;
    let count = if available > 64 { 64 } else { available as usize };
    let span = (count as u32 - 1) * spacing;

    let desired = center.saturating_sub(count as u32 / 2 * spacing);
    let first = if desired < lower {
        lower
    } else if desired > upper - span {
        upper - span
    } else {
        desired
    };

    (first, count, spacing)
}

pub(super) struct Scope {
    samples: &'static mut Samples,
    active: bool,
    paused: bool,
    first: u32,
    count: usize,
    spacing: u32,
    index: usize,
    cursor: usize,
    matched: bool,
    sample_number: u32,
    deadline: u32,
    sweeps: u32,
    error: u32,
    narrow: bool,
    am: bool,
    threshold_rssi: u16,
    threshold_noise: u8,
}

impl Scope {
    /// construct once after BSS initialization. The static sample
    /// arrays belong exclusively to this foreground owner.
    pub(super) unsafe fn new() -> Self {
        Self {
            samples: unsafe { &mut *addr_of_mut!(TRAKT_SCOPE) },
            active: false,
            paused: false,
            first: 145_000_000,
            count: 64,
            spacing: 25_000,
            index: 0,
            cursor: 0,
            matched: false,
            sample_number: 0,
            deadline: 0,
            sweeps: 0,
            error: 0,
            narrow: false,
            am: false,
            threshold_rssi: 160,
            threshold_noise: 64,
        }
    }

    pub(super) fn active(&self) -> bool {
        self.active
    }

    pub(super) fn start(&mut self, now: u32, request: Request, preferences: Preferences) {
        self.active = preferences.spectrum;
        self.paused = false;
        self.error = 0;
        self.sweeps = 0;
        self.narrow = request.narrow;
        self.am = request.am;
        self.threshold_rssi = preferences.spectrum_rssi;
        self.threshold_noise = preferences.spectrum_noise;

        if !self.active {
            return;
        }

        let (first, count, spacing) = window(request.frequency, preferences);
        self.first = first;
        self.count = count;
        self.spacing = spacing;
        self.cursor = ((request.frequency.saturating_sub(first) / spacing) as usize)
            .min(count - 1);

        self.samples.rssi.fill(0);
        self.samples.noise.fill(0);
        self.samples.valid.fill(0);

        self.begin_point(0, now);
    }

    fn begin_point(&mut self, index: usize, now: u32) {
        self.index = index;
        self.matched = false;
        self.deadline = now.wrapping_add(1000);
    }

    fn frequency(&self, index: usize) -> u32 {
        self.first + index as u32 * self.spacing
    }

    pub(super) fn request(&self) -> Request {
        let index = if self.paused { self.cursor } else { self.index };

        Request {
            enabled: self.active,
            frequency: self.frequency(index),
            am: self.am, narrow: self.narrow,
            monitor: false,
            squelch: 0,
            rx_signal: 0,
        }
    }

    pub(super) fn stop(&mut self) {
        self.active = false;
    }

    pub(super) fn observe(&mut self, now: u32, receiver: View) {
        if !self.active || self.paused || self.error != 0 {
            return;
        }

        if matches!(receiver.phase, Phase::Failed) {
            self.error = receiver.error;
            self.paused = true;
            return;
        }

        if reached(now, self.deadline) {
            self.error = 0x5201;
            self.paused = true;
            return;
        }

        if !matches!(receiver.phase, Phase::Receive)
            || receiver.frequency != self.frequency(self.index)
            || receiver.narrow != self.narrow
            || receiver.am != self.am
        {
            self.matched = false;
            return;
        }

        if !self.matched {
            self.matched = true;
            self.sample_number = receiver.sample_number;
            return;
        }

        if receiver.sample_number == self.sample_number {
            return;
        }

        self.samples.rssi[self.index] = receiver.rssi as u16;
        self.samples.noise[self.index] = receiver.noise as u8;
        self.samples.valid[self.index] = 1;

        let next = self.index + 1;
        if next == self.count {
            self.sweeps = self.sweeps.saturating_add(1);
            self.begin_point(0, now);
        } else {
            self.begin_point(next, now);
        }
    }

    fn strongest(&self) -> Option<usize> {
        let mut best = None;
        let mut maximum = 0;

        for index in 0..self.count {
            if self.samples.valid[index] != 0 {
                let value = self.samples.rssi[index];

                if best.is_none() || value > maximum {
                    best = Some(index);
                    maximum = value;
                }
            }
        }

        best
    }

    pub(super) fn key(&mut self, event: Event, now: u32) -> Option<Effect> {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return None;
        }

        let down = matches!(event.edge, Edge::Down);

        if matches!(event.key, Key::Exit) && down {
            self.stop();
            return Some(Effect::Exit);
        }

        if !self.active {
            return None;
        }

        if matches!(event.key, Key::Up | Key::Down) {
            self.paused = true;
            self.cursor = if matches!(event.key, Key::Up) {
                (self.cursor + 1) % self.count
            } else {
                (self.cursor + self.count - 1) % self.count
            };
        } else if down {
            match event.key {
                Key::Menu if self.error == 0 => {
                    self.paused = !self.paused;

                    if !self.paused {
                        self.begin_point(self.cursor, now);
                    }
                }
                Key::F => {
                    if let Some(index) = self.strongest() {
                        self.cursor = index;
                        self.paused = true;
                    }
                }
                Key::Side2 => {
                    let frequency = self.frequency(self.cursor);
                    self.stop();
                    return Some(Effect::Tune(frequency));
                }
                _ => {}
            }
        }

        None
    }

    pub(super) fn render(&self, frame: &mut Frame) {
        frame.clear();
        frame.text_size(2, 0, b"SPECTRUM / RAW RSSI", Size::Small);
        frame.fill(0, 8, 128, 1, true);

        if !self.active {
            frame.text(2, 20, b"SPECTRUM DISABLED");
            frame.text_size(2, 38, b"SCAN > SPECTRUM: ON", Size::Small);
            frame.text_size(2, 58, b"EXIT HOME", Size::Small);
            return;
        }

        widgets::frequency(
            frame, 2, 11, self.frequency(self.cursor), Size::Small,
        );

        frame.text_size(
            58, 11, if self.paused { b"PAUSE" } else { b"SWEEP" }, Size::Small,
        );

        widgets::number(frame, 88, 11, self.sweeps, Size::Small);

        let mut minimum = 511u16;
        let mut maximum = 0u16;
        let mut measured = false;

        for index in 0..self.count {
            if self.samples.valid[index] != 0 {
                minimum = minimum.min(self.samples.rssi[index]);
                maximum = maximum.max(self.samples.rssi[index]);
                measured = true;
            }
        }

        if !measured {
            minimum = 0;
        }

        let span = maximum.saturating_sub(minimum).max(32) as u32;

        for index in 0..self.count {
            let x = index as i16 * 2;
            if self.samples.valid[index] == 0 {
                frame.pixel(i32::from(x), 44, true);
                continue;
            }

            let relative = self.samples.rssi[index].saturating_sub(minimum) as u32;
            let height = widgets::extent(relative, span, 23) as u16 + 1;
            frame.fill(x, 45 - height as i16, 1, height, true);

            if self.samples.rssi[index] >= self.threshold_rssi
                && (self.am || self.samples.noise[index] <= self.threshold_noise)
            {
                frame.pixel(i32::from(x) + 1, 19, true);
            }
        }

        frame.fill(self.cursor as i16 * 2, 18, 1, 28, true);

        frame.text_size(2, 48, b"R", Size::Small);
        if self.samples.valid[self.cursor] != 0 {
            widgets::number(
                frame, 10, 48, self.samples.rssi[self.cursor] as u32, Size::Small,
            );
            frame.text_size(29, 48, b"N", Size::Small);
            widgets::number(
                frame, 37, 48, self.samples.noise[self.cursor] as u32, Size::Small,
            );
        } else {
            frame.text_size(10, 48, b"--", Size::Small);
        }

        frame.text_size(58, 48, b"MIN/MAX", Size::Small);
        widgets::number(frame, 88, 48, minimum as u32, Size::Small);
        widgets::number(frame, 108, 48, maximum as u32, Size::Small);

        if self.error != 0 {
            frame.text_size(2, 57, b"ERR", Size::Small);
            frame.hex(22, 56, self.error);
        } else {
            frame.text_size(
                2, 58, b"F PEAK / SIDE2 TUNE / EXIT", Size::Small,
            );
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<Samples>() == 256);

    let preferences = Preferences::new();
    let (first, count, spacing) = window(145_500_000, preferences);
    assert!(first == 144_700_000);
    assert!(count == 64 && spacing == 25_000);
    assert!(first + (count as u32 - 1) * spacing <= 174_000_000);

    let (first, count, spacing) = window(400_000_000, preferences);
    assert!(first == 399_200_000 && count == 64);
    assert!(first + (count as u32 - 1) * spacing <= 629_999_990);

    let (first, count, spacing) = window(629_999_990, preferences);
    assert!(first + (count as u32 - 1) * spacing == 629_999_990);

    let (first, count, spacing) = window(50_000_000, preferences);
    assert!(first == 50_000_000 && count == 64 && spacing == 25_000);

    let (first, count, spacing) = window(280_000_000, preferences);
    assert!(first == 280_000_000);
    assert!(first + (count as u32 - 1) * spacing <= 629_999_990);

    let (first, count, spacing) = window(279_999_990, preferences);
    assert!(first + (count as u32 - 1) * spacing == 279_999_990);

    let (first, count, spacing) = window(118_000_000, preferences);
    assert!(first <= 118_000_000);
    assert!(first + (count as u32 - 1) * spacing >= 118_000_000);

    let short = Preferences {
        lower: 145_000_000,
        upper: 145_100_000,
        spectrum_step: 1,
        ..Preferences::new()
    };

    let (first, count, spacing) = window(145_050_000, short);
    assert!(first == 145_000_000 && count == 5 && spacing == 25_000);
};
const _: () = {
    let preferences = Preferences::new();
    let (first, count, spacing) = window(840_000_000, preferences);
    assert!(first == 840_000_000);
    assert!(first + (count as u32 - 1) * spacing <= 1_300_000_000);

    let (first, count, spacing) = window(1_300_000_000, preferences);
    assert!(first + (count as u32 - 1) * spacing == 1_300_000_000);

    let low_high_path = Preferences {
        lower: 400_000_000, upper: 470_000_000,
        ..Preferences::new()
    };
    let (first, count, spacing) = window(1_000_000_000, low_high_path);
    assert!(first <= 1_000_000_000);
    assert!(first + (count as u32 - 1) * spacing >= 1_000_000_000);
    assert!(first >= 840_000_000);
};
