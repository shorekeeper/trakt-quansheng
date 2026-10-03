//! Battery-sense ADC on PB0, channel eight.
//!
//! PCLK is divided by eight and the channel uses 239.5 sampling cycles.
//! ADC calibration, enable settling, and conversions advance in foreground
//! passes. No poll waits for a peripheral flag.
//!
//! The eight-sample average is still an ADC code. Converting it to battery
//! voltage requires the board calibration, which is not read here.
//! A valid zero is distinguishable from an unavailable result.
//!
//! Calibration has a 50-tick deadline and conversion a 10-tick deadline.
//! Failure disables ADC and invalidates the displayed result without retry.



use crate::app::time::reached;
use crate::mcu;

const ADC: usize = 0x4001_2400;
const SOFTWARE_TRIGGER: u32 = 7 << 17;

#[repr(u32)]
#[derive(Clone, Copy)]
enum Phase {
    Disabled,
    Calibrating,
    Settling,
    Idle,
    Converting,
    Dead,
}

struct Average {
    values: [u16; 8],
    index: usize,
    count: usize,
    sum: u32,
}

impl Average {
    const fn new() -> Self {
        Self { values: [0; 8], index: 0, count: 0, sum: 0 }
    }

    const fn add(&mut self, value: u32) -> bool {
        if value > 4095 {
            return false;
        }
        self.sum -= self.values[self.index] as u32;
        self.values[self.index] = value as u16;
        self.sum += value;
        self.index = (self.index + 1) & 7;
        if self.count < 8 {
            self.count += 1;
        }
        true
    }

    const fn value(&self) -> Option<u32> {
        if self.count == 0 {
            None
        } else {
            Some((self.sum + self.count as u32 / 2) / self.count as u32)
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Reading {
    pub(crate) average: Option<u32>,
    pub(crate) raw: Option<u32>,
    pub(crate) error: u32,
}

#[repr(C)]
struct AdcState {
    signature: u32,
    tick: u32,
    phase: u32,
    raw: u32,
    average: u32,
    samples: u32,
    sample_tick: u32,
    age: u32,
    failures: u32,
    error: u32,
    adc_hz: u32,
    control: u32,
    status: u32,
    reserved: [u32; 3],
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_ADC: AdcState = unsafe { core::mem::zeroed() };

pub(crate) struct Battery {
    phase: Phase,
    deadline: u32,
    average: Average,
    raw: u32,
    samples: u32,
    sample_tick: u32,
    error: u32,
    adc_hz: u32,
}

impl Battery {
    /// Owns ADC1 and PB0. Other ADC channels and internal reference paths
    /// remain disabled.
    ///
    /// no other context may access ADC1 or reconfigure PB0 or the
    /// ADC clock divider. PCLK must remain unchanged.
    pub(crate) unsafe fn initialize(pclk: u32) -> Option<Self> {
        if !(1_000_000..=72_000_000).contains(&pclk) {
            return None;
        }

        unsafe {
            mcu::enable_gpio();
            mcu::modify(mcu::GPIO_B + 12, 3, 0);
            mcu::modify(mcu::GPIO_B, 3, 3);

            mcu::modify(mcu::RCC + 0x40, 1 << 9, 1 << 9);
            let _ = mcu::read(mcu::RCC + 0x40);
            mcu::modify(mcu::RCC + 0x30, 1 << 9, 1 << 9);
            mcu::modify(mcu::RCC + 0x30, 1 << 9, 0);
            mcu::modify(mcu::RCC, 3 << 21, 3 << 21);

            mcu::write(ADC + 4, 0);
            mcu::write(ADC + 8, SOFTWARE_TRIGGER);
            mcu::write(ADC + 0x14, 7 << 24);
            mcu::write(ADC + 0x30, 0);
            mcu::write(ADC + 0x34, 0);
            mcu::write(ADC + 0x38, 8);
            mcu::write(ADC, 0);
        }

        Some(Self {
            phase: Phase::Disabled,
            deadline: 2,
            average: Average::new(),
            raw: 0,
            samples: 0,
            sample_tick: 0,
            error: 0,
            adc_hz: pclk / 8,
        })
    }

    fn fail(&mut self, error: u32) {
        unsafe { mcu::write(ADC + 8, 0) };
        self.phase = Phase::Dead;
        self.error = error;
    }

    pub(crate) fn pump(&mut self, now: u32) {
        match self.phase {
            Phase::Disabled if reached(now, self.deadline) => {
                unsafe { mcu::write(ADC + 8, SOFTWARE_TRIGGER | 4) };
                self.deadline = now.wrapping_add(50);
                self.phase = Phase::Calibrating;
            }
            Phase::Calibrating => {
                if unsafe { mcu::read(ADC + 8) } & 4 == 0 {
                    unsafe { mcu::write(ADC + 8, SOFTWARE_TRIGGER | 1) };
                    self.deadline = now.wrapping_add(2);
                    self.phase = Phase::Settling;
                } else if reached(now, self.deadline) {
                    self.fail(1);
                }
            }
            Phase::Settling if reached(now, self.deadline) => {
                self.phase = Phase::Idle;
                self.deadline = now;
            }
            Phase::Idle if reached(now, self.deadline) => {
                unsafe {
                    // ADC status flags clear on writing zero.
                    mcu::write(ADC, 0);
                    mcu::write(ADC + 8, SOFTWARE_TRIGGER | 1 | (1 << 20) | (1 << 22));
                }
                self.deadline = now.wrapping_add(10);
                self.phase = Phase::Converting;
            }
            Phase::Converting => {
                let status = unsafe { mcu::read(ADC) };
                if status & (1 << 5) != 0 {
                    self.fail(3);
                } else if status & 2 != 0 {
                    let raw = unsafe { mcu::read(ADC + 0x50) } & 0xFFFF;
                    if !self.average.add(raw) {
                        self.fail(4);
                    } else {
                        self.raw = raw;
                        self.samples = self.samples.saturating_add(1);
                        self.sample_tick = now;
                        self.deadline = now.wrapping_add(250);
                        self.phase = Phase::Idle;
                    }
                } else if reached(now, self.deadline) {
                    self.fail(2);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn reading(&self, now: u32) -> Reading {
        let fresh = self.error == 0 && self.samples != 0
            && now.wrapping_sub(self.sample_tick) < 1_000;
        Reading {
            average: if fresh { self.average.value() } else { None },
            raw: if fresh { Some(self.raw) } else { None },
            error: self.error,
        }
    }

    pub(crate) fn publish(&self, now: u32) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let state = AdcState {
            signature: 0,
            tick: now,
            phase: self.phase as u32,
            raw: self.raw,
            average: self.average.value().unwrap_or(0),
            samples: self.samples,
            sample_tick: self.sample_tick,
            age: now.wrapping_sub(self.sample_tick),
            failures: (self.error != 0) as u32,
            error: self.error,
            adc_hz: self.adc_hz,
            control: unsafe { mcu::read(ADC + 8) },
            status: unsafe { mcu::read(ADC) },
            reserved: [0; 3],
        };
        crate::observation::publish!(TRAKT_ADC, state, 0x5441_4354);
    }
}

const _: () = {
    assert!(core::mem::size_of::<AdcState>() == 64);
    let mut average = Average::new();
    assert!(average.value().is_none());
    assert!(average.add(0));
    assert!(matches!(average.value(), Some(0)));
    assert!(average.add(4095));
    assert!(matches!(average.value(), Some(2048)));
    assert!(!average.add(4096));
    assert!(average.count == 2);

    let mut index = 0;
    while index < 8 {
        assert!(average.add(1000));
        index += 1;
    }
    assert!(matches!(average.value(), Some(1000)));
    assert!(average.count == 8);
    assert!(average.add(1008));
    assert!(matches!(average.value(), Some(1001)));
    assert!(!reached(0xFFFF_FFFD, 3));
    assert!(reached(3, 3));
};