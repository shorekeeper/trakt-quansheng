//! Checked CW receive, sidetone, and two concrete shortcut choices.
//!
//! Threshold uses raw nine-bit RSSI counts, from 1 through 511. Closing
//! threshold is four counts below opening, saturating at zero. This pair
//! belongs only to the decoder envelope and optional CW speaker gate.
//! It never replaces the separate carrier detector used for TX admission.
//!
//! RF gain zero selects the established automatic profile. Selections 1..5
//! fix one of the five existing receiver gain-table entries, lowest through
//! highest. They are register-table choices, not calibrated dB or percent.
//! Manual gain changes alter RSSI observations and require resetting the
//! envelope. RF attempts must reacquire with automatic gain before admission.
//!
//! Volume is Tone1's seven-bit gain code. Zero disables its generator;
//! 1..127 are hardware gain codes, not calibrated loudness. The default 28
//! retains the established sidetone code. RX output gain remains separate.
//!
//! Only Seven tap and Seven hold are assignable here. Action zero retains
//! the ordinary numeric tap and gives hold no action. Other choices open
//! the receive envelope, native gain/volume/threshold editor, CW settings,
//! or clear text. Exit, PTT, paddles, and menu confirmation are not remapped.
//!
//! Break-in zero retains semi-break-in: the transmitter stays prepared through
//! element gaps and the selected session hang. Choices 1..7 request RX after
//! every falling mark, with 50, 25, 10, 5, 2, 1, or zero additional RX-settle
//! ticks before speaker admission. All are experimental receive-window modes.
//! Zero settle adds no software wait; transport, relock and audio recovery
//! still take time. Training ignores this selection.
//!
//! Full break-in does not end the logical session at every space. Its original
//! timeout and keyer memory remain active. The ordinary hang still ends that
//! session after keyer inactivity, but no longer holds RF in TX during its
//! spaces. A pending electronic mark waits for checked TX readiness.
//!
//! The twenty-eight-bit word contains threshold in 0..8, gain in 9..11,
//! volume in 12..18, tap in 19..21, hold in 22..24, and break-in in 25..27.
//! XOR coding makes zero select threshold 160, AUTO, volume 28, ordinary
//! Seven tap, envelope on hold, and semi-break-in. Higher bits fail.
//! Snapshot global word seven stores this word above its low power nibble.
//! The record format owner restricts historical uses of the extension.
//!
//! These values own no hardware, live permit, input history, or allocation.

#[derive(Clone, Copy)]
pub(crate) struct Controls {
    pub(crate) threshold: u16,
    pub(crate) gain: u8,
    pub(crate) volume: u8,
    pub(crate) tap: u8,
    pub(crate) hold: u8,
    pub(crate) breakin: u8,
}

impl Controls {
    pub(crate) const fn new() -> Self {
        Self {
            threshold: 160, gain: 0, volume: 28, tap: 0, hold: 1, breakin: 0,
        }
    }

    pub(crate) const fn valid(self) -> bool {
        self.threshold >= 1 && self.threshold <= 511
            && self.gain < 6 && self.volume <= 127
            && self.tap < 7 && self.hold < 7 && self.breakin < 8
    }

    #[inline(never)]
    pub(crate) const fn word(self) -> u32 {
        assert!(self.valid());
        (self.threshold as u32
            | ((self.gain as u32) << 9)
            | ((self.volume as u32) << 12)
            | ((self.tap as u32) << 19)
            | ((self.hold as u32) << 22)
            | ((self.breakin as u32) << 25))
            ^ (160 | (28 << 12) | (1 << 22))
    }

    #[inline(never)]
    pub(crate) const fn decode(word: u32) -> Option<Self> {
        if word & !0x0FFF_FFFF != 0 {
            return None;
        }
        let fields = word ^ (160 | (28 << 12) | (1 << 22));
        let value = Self {
            threshold: (fields & 511) as u16,
            gain: ((fields >> 9) & 7) as u8,
            volume: ((fields >> 12) & 127) as u8,
            tap: ((fields >> 19) & 7) as u8,
            hold: ((fields >> 22) & 7) as u8,
            breakin: ((fields >> 25) & 7) as u8,
        };
        if value.valid() { Some(value) } else { None }
    }

    pub(crate) const fn close(self) -> u16 {
        self.threshold.saturating_sub(4)
    }

    /// Native editor indices. Threshold uses index zero for one raw count.
    pub(crate) const fn value(self, field: usize) -> u32 {
        match field {
            0 => self.threshold as u32 - 1,
            1 => self.gain as u32,
            2 => self.volume as u32,
            3 => self.tap as u32,
            4 => self.hold as u32,
            5 => self.breakin as u32,
            _ => panic!("unknown CW control"),
        }
    }

    pub(crate) const fn count(field: usize) -> u32 {
        match field {
            0 => 511,
            1 => 6,
            2 => 128,
            3 | 4 => 7,
            5 => 8,
            _ => panic!("unknown CW control"),
        }
    }

    pub(crate) const fn configure(&mut self, field: usize, value: u32) -> bool {
        if field > 5 || value >= Self::count(field) {
            return false;
        }
        match field {
            0 => self.threshold = value as u16 + 1,
            1 => self.gain = value as u8,
            2 => self.volume = value as u8,
            3 => self.tap = value as u8,
            4 => self.hold = value as u8,
            _ => self.breakin = value as u8,
        }
        true
    }

    /// Arrow adjustments clamp rather than wrapping from minimum to maximum.
    pub(crate) const fn shift(&mut self, field: usize, up: bool, step: u32) {
        assert!(field < 3 && step != 0);
        let current = self.value(field);
        let maximum = Self::count(field) - 1;
        let candidate = if up {
            current.saturating_add(step)
        } else {
            current.saturating_sub(step)
        };
        let next = if candidate > maximum { maximum } else { candidate };
        assert!(self.configure(field, next));
    }
}

/// None retains semi-break-in. Some(0) requests a receive window without
/// an added software settle interval; it does not promise immediate audio.
pub(crate) const fn breakin_ticks(value: u8) -> Option<u32> {
    match value {
        0 => None,
        1 => Some(50),
        2 => Some(25),
        3 => Some(10),
        4 => Some(5),
        5 => Some(2),
        6 => Some(1),
        7 => Some(0),
        _ => panic!("unknown CW break-in choice"),
    }
}

pub(crate) const fn breakin_name(value: u32) -> &'static [u8] {
    match value {
        0 => b"SEMI",
        1 => b"FULL 50MS",
        2 => b"FULL 25MS",
        3 => b"FULL 10MS",
        4 => b"FULL 5MS",
        5 => b"FULL 2MS",
        6 => b"FULL 1MS",
        _ => b"FULL 0MS",
    }
}

const _: () = {
    assert!(breakin_ticks(0).is_none());
    assert!(matches!(breakin_ticks(1), Some(50)));
    assert!(matches!(breakin_ticks(7), Some(0)));
    assert!(!breakin_name(0).is_empty());
};

pub(crate) const fn gain_name(value: u32) -> &'static [u8] {
    match value {
        0 => b"AUTO",
        1 => b"FIX 1",
        2 => b"FIX 2",
        3 => b"FIX 3",
        4 => b"FIX 4",
        _ => b"FIX 5",
    }
}

pub(crate) const fn action_name(value: u32) -> &'static [u8] {
    match value {
        0 => b"NORMAL",
        1 => b"ENVELOPE",
        2 => b"RF GAIN",
        3 => b"TONE VOL",
        4 => b"THRESHOLD",
        5 => b"CW MENU",
        _ => b"CLEAR TEXT",
    }
}

const _: () = {
    assert!(Controls::new().word() == 0);
    assert!(Controls::new().close() == 156);
    assert!(Controls::decode(1 << 28).is_none());
    assert!(Controls::decode(u32::MAX).is_none());

    let mut field = 0;
    while field < 6 {
        let mut index = 0;
        while index < Controls::count(field) {
            let mut value = Controls::new();
            assert!(value.configure(field, index));
            let loaded = match Controls::decode(value.word()) {
                Some(value) => value,
                None => panic!("CW controls rejected"),
            };
            assert!(loaded.value(field) == index);
            assert!(loaded.word() == value.word());
            index += 1;
        }
        field += 1;
    }

    let mut value = Controls::new();
    value.shift(0, false, 1000);
    assert!(value.threshold == 1 && value.close() == 0);
    value.shift(0, true, 1000);
    assert!(value.threshold == 511);
    value.shift(2, false, 1000);
    assert!(value.volume == 0);
    assert!(!gain_name(0).is_empty());
    assert!(!action_name(0).is_empty());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_carrier_threshold_is_independent_and_roundtrips() {
        let mut value = Controls::new();
        assert!(value.configure(0, 104));
        assert_eq!(value.threshold, 105);
        assert_eq!(value.close(), 101);
        assert!(value.configure(1, 5));
        assert!(value.configure(2, 0));
        assert!(value.configure(3, 3));
        assert!(value.configure(4, 1));
        let loaded = Controls::decode(value.word()).unwrap();
        assert_eq!(loaded.word(), value.word());
        assert_eq!(loaded.threshold, 105);
        assert_eq!(loaded.gain, 5);
        assert_eq!(loaded.volume, 0);
        assert_eq!(loaded.tap, 3);
        assert_eq!(loaded.hold, 1);
    }

    #[test]
    fn invalid_choices_do_not_modify_the_owned_word() {
        let mut value = Controls::new();
        for field in 0..6 {
            let before = value.word();
            assert!(!value.configure(field, Controls::count(field)));
            assert_eq!(value.word(), before);
        }
        assert!(!value.configure(6, 0));
    }
}