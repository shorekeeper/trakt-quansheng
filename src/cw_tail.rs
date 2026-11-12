//! Saved local CW sidetone-release choices.
//!
//! Shape selects the falling DAC-gain profile used by full break-in.
//! LINEAR retains equal code decrements. SOFT attenuates more strongly
//! at the beginning; SMOOTH retains more level in its first step.
//! Each profile reaches zero in at most four serviced one-tick steps.
//! These are DAC-gain codes, not calibrated acoustic-amplitude curves.
//!
//! Release selects zero through four observed ticks at zero output gain
//! before the sidetone oscillator frequency is cleared. The interval begins
//! after the zero-gain write. Default LINEAR with two release ticks retains
//! the established local audio behavior. Semi-break-in and practice do not
//! use these choices.
//!
//! Shape introduces no additional wait state or new RF edge. Release is an
//! explicit local-audio allowance: increasing it consumes receive-window
//! time. The electronic keyer anticipates TX preparation using its existing
//! session turnaround observation. Neither choice changes Morse element
//! durations, PA-off order, synthesizer waits, or complete-session TOT.
//!
//! The five-bit word stores shape in bits 0..1 and release in bits 2..4.
//! XOR coding makes zero select LINEAR and two release ticks. Shape three,
//! release codes five through seven, and higher bits are rejected.
//! Snapshot sound word six stores this word in bits 26..30; bit 31 remains
//! reserved. Choices persist in every composition but only a CW transmitter
//! with full break-in applies them. No hardware or live deadline is stored.

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub(crate) shape: u8,
    pub(crate) release: u8,
}

impl Settings {
    pub(crate) const fn new() -> Self {
        Self { shape: 0, release: 2 }
    }

    pub(crate) const fn valid(self) -> bool {
        self.shape < 3 && self.release <= 4
    }

    pub(crate) const fn word(self) -> u8 {
        assert!(self.valid());
        (self.shape | (self.release << 2)) ^ (2 << 2)
    }

    #[inline(never)]
    pub(crate) const fn decode(word: u32) -> Option<Self> {
        if word & !31 != 0 {
            return None;
        }
        let fields = word as u8 ^ (2 << 2);
        let value = Self {
            shape: fields & 3,
            release: (fields >> 2) & 7,
        };
        if value.valid() { Some(value) } else { None }
    }

    pub(crate) const fn count(field: usize) -> u32 {
        match field {
            0 => 3,
            1 => 5,
            _ => panic!("unknown CW tail setting"),
        }
    }

    pub(crate) const fn value(self, field: usize) -> u32 {
        match field {
            0 => self.shape as u32,
            1 => self.release as u32,
            _ => panic!("unknown CW tail setting"),
        }
    }

    pub(crate) const fn configure(&mut self, field: usize, value: u32) -> bool {
        if field > 1 || value >= Self::count(field) {
            return false;
        }
        if field == 0 {
            self.shape = value as u8;
        } else {
            self.release = value as u8;
        }
        true
    }
}

pub(crate) const fn shape_name(value: u32) -> &'static [u8] {
    match value {
        0 => b"LINEAR",
        1 => b"SOFT",
        _ => b"SMOOTH",
    }
}

/// Falling output code after one of four serviced steps.
/// full is the captured DAC-gain nibble; from is the level at fade entry.
/// LINEAR uses the captured full-level step size, retaining the ordinary
/// behavior when a receive fade is interrupted at a partial level.
/// Shaped profiles use rounded-up fractions of their entry level, so a
/// nonzero input reaches zero only at their final step.
/// No timestamp, generator command, or RF permission is changed here.
pub(crate) const fn falling(
    full: u8,
    from: u8,
    step: u8,
    shape: u8,
) -> u8 {
    assert!(full <= 15 && from <= full && step >= 1 && step <= 4 && shape < 3);

    if shape == 0 {
        let quarter = (full + 3) / 4;
        let increment = if quarter == 0 { 1 } else { quarter };
        let decrement = increment * step;
        return from.saturating_sub(decrement);
    }

    let weights = if shape == 1 {
        [8u8, 3, 1, 0]
    } else {
        [14u8, 8, 2, 0]
    };
    ((from as u16 * weights[step as usize - 1] as u16 + 15) / 16) as u8
}

const _: () = {
    let defaults = Settings::new();
    assert!(defaults.word() == 0);
    assert!(defaults.valid());
    assert!(!shape_name(0).is_empty());
    assert!(Settings::decode(32).is_none());
    assert!(Settings::decode(3).is_none());

    let mut shape = 0;
    while shape < 3 {
        let mut release = 0;
        while release <= 4 {
            let value = Settings { shape, release };
            let loaded = match Settings::decode(value.word() as u32) {
                Some(value) => value,
                None => panic!("CW tail word rejected"),
            };
            assert!(loaded.shape == shape && loaded.release == release);
            assert!(loaded.word() == value.word());
            release += 1;
        }
        shape += 1;
    }

    let mut choices = Settings::new();
    assert!(choices.configure(0, 2));
    assert!(choices.configure(1, 4));
    assert!(choices.value(0) == 2 && choices.value(1) == 4);
    assert!(Settings::count(0) == 3 && Settings::count(1) == 5);
    assert!(!choices.configure(0, 3));
    assert!(!choices.configure(1, 5));
    assert!(!choices.configure(2, 0));

    assert!(falling(14, 14, 1, 0) == 10);
    assert!(falling(14, 14, 2, 0) == 6);
    assert!(falling(14, 14, 3, 0) == 2);
    assert!(falling(14, 14, 4, 0) == 0);

    assert!(falling(14, 14, 1, 1) == 7);
    assert!(falling(14, 14, 2, 1) == 3);
    assert!(falling(14, 14, 3, 1) == 1);
    assert!(falling(14, 14, 4, 1) == 0);

    assert!(falling(14, 14, 1, 2) == 13);
    assert!(falling(14, 14, 2, 2) == 7);
    assert!(falling(14, 14, 3, 2) == 2);
    assert!(falling(14, 14, 4, 2) == 0);

    let mut full = 0;
    while full <= 15 {
        let mut from = 0;
        while from <= full {
            let mut shape = 0;
            while shape < 3 {
                let mut previous = from;
                let mut step = 1;
                while step <= 4 {
                    let value = falling(full, from, step, shape);
                    assert!(value <= previous);
                    previous = value;
                    step += 1;
                }
                assert!(previous == 0);
                shape += 1;
            }
            from += 1;
        }
        full += 1;
    }
};