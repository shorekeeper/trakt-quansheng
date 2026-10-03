//! Read-only voice-TX amplitude observations from REG_64.
//!
//! Sampling occurs only during voice TX, with visualization enabled, at no
//! more than one read every twenty observed ticks. Setup, local permit,
//! Roger, and refusal audio are excluded. No gain or RF register is written.
//! The complete sixteen-bit register value is retained without a new mask.
//!
//! Display level is linear over a fixed span of 4096 raw counts after an
//! eighteen-count display floor. Full display scale is therefore raw 4114.
//! These are presentation constants, not measured noise, sound pressure,
//! deviation, microphone gain, or a hardware clipping threshold.
//! There is no automatic normalization or noise subtraction.
//!
//! Attack follows a rising observation immediately. Falling display level
//! decays by at most twelve units per accepted sample; the peak marker decays
//! by three. Both use the 0..255 display range, not register units.
//!
//! Minimum and maximum are raw extrema within one observed voice-TX run.
//! They include any startup transient that reaches voice state. They cannot
//! classify speech or establish a noise floor. History stores sixty-four
//! display levels, not PCM samples.
//!
//! Leaving voice state clears active display levels but retains the last
//! raw value, extrema, and history. The next voice run resets them before its
//! first sample. The record remains ninety-six bytes and starts entirely zero.
//! It has one foreground writer and foreground readers; no ISR accesses it.

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use super::read;

const SAMPLE_TICKS: u32 = 20;
const DISPLAY_FLOOR: u32 = 18;
const DISPLAY_SPAN: u32 = 4096;

const fn level(raw: u32) -> u32 {
    let value = raw.saturating_sub(DISPLAY_FLOOR);
    let limited = if value > DISPLAY_SPAN { DISPLAY_SPAN } else { value };
    limited * 255 / DISPLAY_SPAN
}

#[repr(C)]
#[derive(Clone, Copy)]
struct State {
    tick: u32,
    raw: u32,
    level: u32,
    peak: u32,
    index: u32,
    count: u32,
    active: u32,
    minimum: u16,
    maximum: u16,
    history: [u8; 64],
}

impl State {
    const fn empty() -> Self {
        Self {
            tick: 0,
            raw: 0,
            level: 0,
            peak: 0,
            index: 0,
            count: 0,
            active: 0,
            minimum: 0,
            maximum: 0,
            history: [0; 64],
        }
    }

    const fn due(&self, now: u32) -> bool {
        self.active == 0 || now.wrapping_sub(self.tick) >= SAMPLE_TICKS
    }

    const fn stop(&mut self) {
        self.active = 0;
        self.level = 0;
        self.peak = 0;
    }

    /// Accepts one raw observation after the sampling deadline. All state
    /// transitions are independent of the register transport and are also
    /// exercised by compiler-evaluated fixtures.
    const fn record(&mut self, now: u32, raw: u16) {
        assert!(self.due(now));

        if self.active == 0 {
            *self = Self::empty();
            self.active = 1;
        }

        if self.count == 0 || raw < self.minimum {
            self.minimum = raw;
        }
        if self.count == 0 || raw > self.maximum {
            self.maximum = raw;
        }

        self.tick = now;
        self.raw = raw as u32;

        let target = level(self.raw);
        if target >= self.level {
            self.level = target;
        } else {
            let falling = self.level.saturating_sub(12);
            self.level = if falling > target { falling } else { target };
        }

        let falling_peak = self.peak.saturating_sub(3);
        self.peak = if falling_peak > self.level {
            falling_peak
        } else {
            self.level
        };

        self.history[self.index as usize] = self.level as u8;
        self.index = (self.index + 1) & 63;
        if self.count < 64 {
            self.count += 1;
        }
    }
}

#[used]
#[no_mangle]
static mut TRAKT_TX_METER: State = State::empty();

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) active: bool,
    pub(crate) raw: u32,
    pub(crate) minimum: u32,
    pub(crate) maximum: u32,
    pub(crate) level: u32,
    pub(crate) peak: u32,
    pub(crate) index: usize,
    pub(crate) count: usize,
    pub(crate) history: [u8; 64],
}

#[inline(never)]
pub(crate) fn sample(now: u32, voice_active: bool) {
    let mut state = unsafe { read_volatile(addr_of!(TRAKT_TX_METER)) };

    if !voice_active {
        if state.active != 0 {
            state.stop();
            unsafe {
                write_volatile(addr_of_mut!(TRAKT_TX_METER), state);
            }
        }
        return;
    }

    if !state.due(now) {
        return;
    }

    state.record(now, read(0x64));

    unsafe {
        write_volatile(addr_of_mut!(TRAKT_TX_METER), state);
    }
}

#[inline(never)]
pub(crate) fn view() -> View {
    let state = unsafe { read_volatile(addr_of!(TRAKT_TX_METER)) };

    View {
        active: state.active != 0,
        raw: state.raw,
        minimum: state.minimum as u32,
        maximum: state.maximum as u32,
        level: state.level,
        peak: state.peak,
        index: state.index as usize,
        count: state.count as usize,
        history: state.history,
    }
}

const _: () = {
    assert!(core::mem::size_of::<State>() == 96);
    assert!(level(0) == 0);
    assert!(level(18) == 0);
    assert!(level(19) == 0);
    assert!(level(27) == 0);
    assert!(level(35) == 1);
    assert!(level(100) == 5);
    assert!(level(170) == 9);
    assert!(level(1042) == 63);
    assert!(level(1450) == 89);
    assert!(level(2066) == 127);
    assert!(level(2300) == 142);
    assert!(level(4113) == 254);
    assert!(level(4114) == 255);
    assert!(level(65535) == 255);
    assert!(level(u32::MAX) == 255);

    let mut raw = 0;
    let mut previous = 0;
    while raw < 4096 {
        let current = level(raw);
        assert!(current >= previous && current <= 255);
        previous = current;
        raw += 1;
    }

    let mut state = State::empty();
    assert!(state.due(0));
    state.record(0, 100);
    assert!(state.active == 1 && state.count == 1);
    assert!(state.raw == 100);
    assert!(state.minimum == 100 && state.maximum == 100);
    assert!(state.level == 5 && state.peak == 5);
    assert!(!state.due(19));
    assert!(state.due(20));

    state.record(20, 4114);
    assert!(state.level == 255 && state.peak == 255);
    assert!(state.minimum == 100 && state.maximum == 4114);

    state.record(40, 18);
    assert!(state.level == 243 && state.peak == 252);
    assert!(state.minimum == 18 && state.maximum == 4114);
    assert!(state.history[2] == 243);

    state.stop();
    assert!(state.active == 0 && state.level == 0 && state.peak == 0);
    assert!(state.raw == 18);
    assert!(state.minimum == 18 && state.maximum == 4114);
    assert!(state.count == 3);

    state.record(41, 75);
    assert!(state.active == 1 && state.count == 1 && state.index == 1);
    assert!(state.minimum == 75 && state.maximum == 75);
    assert!(state.history[1] == 0);

    let mut history = State::empty();
    let mut index = 0;
    while index < 70 {
        history.record(index * 20, (18 + index * 16) as u16);
        index += 1;
    }
    assert!(history.count == 64 && history.index == 6);
    assert!(history.minimum == 18 && history.maximum == 1122);
    assert!(history.history[5] as u32 == level(1122));

    let mut wrapped = State::empty();
    wrapped.record(u32::MAX - 10, 65535);
    assert!(!wrapped.due(8));
    assert!(wrapped.due(9));
    wrapped.record(9, 0);
    assert!(wrapped.minimum == 0 && wrapped.maximum == 65535);
    assert!(wrapped.count == 2);
};