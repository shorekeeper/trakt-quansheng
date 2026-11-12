//! BK4829 VOX comparator under one concrete RX or voice-TX owner.
//!
//! Register programming follows the supplied UV-K5 V3 BK4829 reference port:
//! REG_46 is A000 OR the eleven-bit opening threshold; REG_79 is 1800 OR
//! the closing threshold. REG_7A retains the reference word 289A. Its
//! contradictory delay comment is not used as a timing specification.
//! REG_31 bit two enables VOX. Neither REG_71 nor MIC ADC is configured.
//!
//! The application reference starts its voice request on event 0010 and
//! clears voice on 0020, despite their LOST/FOUND compatibility names.
//! Both events together clear voice. An actual event establishes the latch;
//! silence is not invented from a zero register read after initialization.
//!
//! Receiver shares one event read with directional signaling. Voice TX has
//! exclusive event ownership and uses pump. No second event consumer exists
//! on either path. FSK, CW, local sound, and packet tails exclude this owner.
//! MASK belongs to the enclosing event-mask owner, not this configuration.
//!
//! Activation captures three passive registers, programs thresholds and delay,
//! then enables bit two. Cleanup clears that bit before restoring the passive
//! registers. It never restores an enabled comparator or changes the event
//! mask, frequency, PA, RF links, AF selection, amplifier, or microphone gain.
//!
//! Observations are sampled no more than once per ten nominal ticks. Getter
//! freshness is limited to twenty ticks. Register checks verify retention,
//! not acoustic sensitivity, electrical microphone activity, or RF emissions.
//! A bus or configuration mismatch is returned to the existing RF owner.

use super::{read, write};
use crate::app::time::reached;
use crate::vox::Sample;

pub(crate) const MASK: u16 = 0x0030;
const REGISTERS: [u8; 3] = [0x46, 0x79, 0x7A];

const fn words(threshold: u16) -> Option<[u16; 3]> {
    if threshold == 0 || threshold > 2047 {
        return None;
    }
    let margin = threshold / 8;
    let margin = if margin == 0 { 1 } else { margin };
    let close = threshold.saturating_sub(margin);
    let close = if close == 0 { 1 } else { close };
    Some([0xA000 | threshold, 0x1800 | close, 0x289A])
}

const fn voice(previous: Option<bool>, events: u16) -> Option<bool> {
    if events & 0x0020 != 0 {
        Some(false)
    } else if events & 0x0010 != 0 {
        Some(true)
    } else {
        previous
    }
}

pub(crate) struct Detector {
    active: bool,
    threshold: u16,
    saved: [u16; 3],
    next: u32,
    sample: Option<Sample>,
}

impl Detector {
    pub(crate) const fn new() -> Self {
        Self {
            active: false, threshold: 0, saved: [0; 3],
            next: 0, sample: None,
        }
    }

    pub(crate) const fn active(&self) -> bool {
        self.active
    }

    /// Exact installed comparator choice. A nonzero-to-nonzero change
    /// requires the same muted reconfiguration as an enable transition.
    pub(crate) const fn matches(&self, threshold: u16) -> bool {
        if self.active {
            self.threshold == threshold
        } else {
            threshold == 0
        }
    }

    pub(crate) fn start(&mut self, now: u32, threshold: u16) -> Result<(), u32> {
        let expected = words(threshold).ok_or(0x7C01u32)?;
        if self.active || read(0x31) & 4 != 0 {
            return Err(0x7C02);
        }
        for index in 0..REGISTERS.len() {
            self.saved[index] = read(REGISTERS[index]);
        }
        self.active = true;
        self.threshold = threshold;
        self.next = now;
        self.sample = None;

        for index in 0..REGISTERS.len() {
            write(REGISTERS[index], expected[index]);
        }
        write(0x31, read(0x31) | 4);
        self.check()
    }

    pub(crate) fn check(&self) -> Result<(), u32> {
        if !self.active {
            return Ok(());
        }
        let expected = words(self.threshold).ok_or(0x7C01u32)?;
        if read(0x31) & 4 == 0 {
            return Err(0x7C31);
        }
        for index in 0..REGISTERS.len() {
            if read(REGISTERS[index]) != expected[index] {
                return Err(0x7C00 + REGISTERS[index] as u32);
            }
        }
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        if self.active {
            write(0x31, read(0x31) & !4);
            for index in 0..REGISTERS.len() {
                write(REGISTERS[index], self.saved[index]);
            }
        }
        self.active = false;
        self.sample = None;
    }

    /// Receiver supplies the complete event set already consumed for tones.
    /// No register read or independent acknowledgement occurs here.
    pub(crate) fn observe(&mut self, now: u32, events: u16) {
        if !self.active {
            return;
        }
        let previous = self.sample.and_then(|sample| sample.voice);
        let comparator = events & MASK;
        let retained = if comparator != 0 {
            comparator
        } else {
            self.sample.map_or(0, |sample| sample.events)
        };
        self.sample = Some(Sample {
            tick: now, voice: voice(previous, events), events: retained,
        });
        self.next = now.wrapping_add(10);
    }

    /// Exclusive voice-TX use only. At most one pending event set is consumed.
    pub(crate) fn pump(&mut self, now: u32) {
        if !self.active || !reached(now, self.next) {
            return;
        }
        let events = if read(0x0C) & 1 != 0 {
            write(0x02, 0);
            read(0x02)
        } else {
            0
        };
        self.observe(now, events);
    }

    pub(crate) fn sample(&self, now: u32) -> Option<Sample> {
        self.sample.filter(|sample| {
            self.active && now.wrapping_sub(sample.tick) <= 20
        })
    }
}

const _: () = {
    assert!(MASK == 0x0030);
    assert!(words(0).is_none() && words(2048).is_none());
    let minimum = match words(1) {
        Some(value) => value,
        None => panic!("minimum VOX threshold rejected"),
    };
    assert!(minimum[0] == 0xA001 && minimum[1] == 0x1801);
    let maximum = match words(2047) {
        Some(value) => value,
        None => panic!("maximum VOX threshold rejected"),
    };
    assert!(maximum[0] == 0xA7FF && maximum[1] == 0x1F00);
    assert!(maximum[2] == 0x289A);
    assert!(voice(None, 0).is_none());
    assert!(matches!(voice(None, 0x0010), Some(true)));
    assert!(matches!(voice(None, 0x0020), Some(false)));
    assert!(matches!(voice(Some(true), 0x0030), Some(false)));
    assert!(matches!(voice(Some(true), 0x0100), Some(true)));
};