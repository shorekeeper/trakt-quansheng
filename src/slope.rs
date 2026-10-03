//! SlopeCast settings and shifted-frequency resolution without device access.
//!
//! SlopeCast emits FM on one side of the nominal AM receive frequency.
//! It relies on the receiving equipment's frequency-selective amplitude
//! response; it does not synthesize an AM envelope or guarantee compatibility.
//! LOWER and UPPER select RF offset direction, not an electrical phase.
//!
//! Settings are global and retained in all firmware compositions. Only a
//! build with fam and an explicitly enabled setting may resolve AM to TX.
//! Default is disabled, lower side, 5.0 kHz, wide FM TX. AM RX width remains
//! independent. No field represents measured deviation or RF power.
//!
//! Delta uses 100 Hz units, from 1 through 500. Packed settings use enable
//! in bit zero, upper side in bit one, narrow TX in bit two, and delta code
//! in bits 3..11. Delta code zero means the default 50 units; code 50 is
//! reserved to prevent a second encoding of the same setting. Other codes
//! 1..500 retain their numeric unit value. Higher bits are rejected.
//!
//! Snapshot word 111 stores this twelve-bit settings word above its existing
//! DW bit. Thus a historical zero extension restores disabled defaults.
//! Encoding is independent of fam so changing composition cannot erase saved
//! settings. Persistent encoding alone never grants runtime TX admission.
//!
//! Only AM simplex profiles use the shift. RX-only remains disabled; offsets
//! and split are rejected instead of silently stacking or replacing routes.
//! Both nominal and shifted frequencies must lie in the same existing TX
//! hardware band. Higher-level policy checks both with the chosen FM margin.
//! Calibration uses the actual shifted frequency. These helpers cannot
//! authorize PA, bypass carrier admission, or change any saved profile.

use crate::profile::{Band, Profile, Tx};

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
    pub(crate) upper: bool,
    pub(crate) narrow: bool,
    /// Positive offset in units of 100 Hz; valid range is 1..500 inclusive.
    pub(crate) delta: u16,
}

impl Settings {
    pub(crate) const fn new() -> Self {
        Self { enabled: false, upper: false, narrow: false, delta: 50 }
    }

    pub(crate) const fn valid(self) -> bool {
        self.delta >= 1 && self.delta <= 500
    }

    pub(crate) const fn word(self) -> u16 {
        assert!(self.valid());
        let delta = if self.delta == 50 { 0 } else { self.delta };
        self.enabled as u16 | ((self.upper as u16) << 1)
            | ((self.narrow as u16) << 2) | (delta << 3)
    }

    pub(crate) const fn decode(word: u32) -> Option<Self> {
        if word & !0xFFF != 0 {
            return None;
        }
        let delta = word >> 3;
        if delta == 50 || delta > 500 {
            return None;
        }

        Some(Self {
            enabled: word & 1 != 0,
            upper: word & 2 != 0,
            narrow: word & 4 != 0,
            delta: if delta == 0 { 50 } else { delta as u16 },
        })
    }

    /// Resolve only the RF shift. The selected route and complete profile
    /// are checked by resolve. The transmitter independently recomputes this
    /// shift before accepting the supplied actual transmit frequency.
    pub(crate) const fn frequency(self, nominal: u32) -> Result<u32, u32> {
        if !cfg!(feature = "fam") || !self.enabled {
            return Err(0x700F);
        }
        if !self.valid() {
            return Err(0x7010);
        }

        let band = match Band::of(nominal) {
            Some(band) => band,
            None => return Err(0x7012),
        };
        let delta = self.delta as u32 * 100;
        let actual = if self.upper {
            nominal.checked_add(delta)
        } else {
            nominal.checked_sub(delta)
        };

        match actual {
            Some(frequency) if frequency >= band.lower() && frequency <= band.upper() => {
                Ok(frequency)
            }
            _ => Err(0x7012),
        }
    }
}

/// Ordinary FM retains its original route semantics. AM remains refused
/// outside fam or with SlopeCast disabled, regardless of its stored route.
/// Resolution never mutates the tune, signaling, or RX/TX route.
pub(crate) const fn resolve(profile: Profile, settings: Settings) -> Result<u32, u32> {
    if profile.tune.am {
        if !cfg!(feature = "fam") || !settings.enabled {
            return Err(0x700F);
        }
        match profile.transmit {
            Tx::Simplex => settings.frequency(profile.tune.frequency),
            Tx::ReceiveOnly => Err(0x700C),
            _ => Err(0x7011),
        }
    } else {
        match profile.transmit.frequency(profile.tune.frequency) {
            Ok(Some(frequency)) => Ok(frequency),
            Ok(None) => Err(0x700C),
            Err(_) => Err(0x700B),
        }
    }
}

/// Four ASCII cells in kHz with one decimal place, including leading zero.
/// The value is still a frequency shift, not a measured deviation.
pub(crate) const fn khz(units: u16) -> [u8; 4] {
    assert!(units >= 1 && units <= 500);
    [
        b'0' + (units / 100) as u8,
        b'0' + ((units / 10) % 10) as u8,
        b'.',
        b'0' + (units % 10) as u8,
    ]
}

const _: () = {
    assert!(Settings::new().word() == 0);
    let defaults = match Settings::decode(0) {
        Some(value) => value,
        None => panic!("SlopeCast defaults rejected"),
    };
    assert!(!defaults.enabled && !defaults.upper && !defaults.narrow);
    assert!(defaults.delta == 50);

    let mut delta = 1;
    while delta <= 500 {
        let mut flags = 0;
        while flags < 8 {
            let settings = Settings {
                enabled: flags & 1 != 0,
                upper: flags & 2 != 0,
                narrow: flags & 4 != 0,
                delta,
            };
            let restored = match Settings::decode(settings.word() as u32) {
                Some(value) => value,
                None => panic!("SlopeCast word rejected"),
            };
            assert!(restored.word() == settings.word());
            assert!(restored.delta == delta);
            assert!(restored.enabled == settings.enabled);
            assert!(restored.upper == settings.upper);
            assert!(restored.narrow == settings.narrow);
            flags += 1;
        }
        delta += 1;
    }

    assert!(Settings::decode(50 << 3).is_none());
    assert!(Settings::decode(501 << 3).is_none());
    assert!(Settings::decode(1 << 12).is_none());
    assert!(Settings::decode(u32::MAX).is_none());
    assert!(khz(1)[3] == b'1');
    assert!(khz(40)[1] == b'4');
    assert!(khz(50)[1] == b'5');
    assert!(khz(500)[0] == b'5');
};

const _: () = {
    use crate::profile::Tune;

    let mut profile = Profile::simplex(Tune {
        frequency: 433_500_000, step: 12_500, narrow: false, am: true,
    });
    let mut settings = Settings::new();
    assert!(matches!(resolve(profile, settings), Err(0x700F)));
    settings.enabled = true;

    if cfg!(feature = "fam") {
        assert!(matches!(resolve(profile, settings), Ok(433_495_000)));
        settings.delta = 40;
        assert!(matches!(resolve(profile, settings), Ok(433_496_000)));
        settings.upper = true;
        assert!(matches!(resolve(profile, settings), Ok(433_504_000)));

        settings.delta = 0;
        assert!(matches!(resolve(profile, settings), Err(0x7010)));
        settings.delta = 501;
        assert!(matches!(resolve(profile, settings), Err(0x7010)));
        settings.delta = 50;

        profile.transmit = Tx::ReceiveOnly;
        assert!(matches!(resolve(profile, settings), Err(0x700C)));
        profile.transmit = Tx::Split(145_500_000);
        assert!(matches!(resolve(profile, settings), Err(0x7011)));
        profile.transmit = Tx::Minus(600_000);
        assert!(matches!(resolve(profile, settings), Err(0x7011)));
        profile.transmit = Tx::Simplex;

        profile.tune.frequency = 470_000_000;
        assert!(matches!(resolve(profile, settings), Err(0x7012)));
        settings.upper = false;
        profile.tune.frequency = 136_000_000;
        assert!(matches!(resolve(profile, settings), Err(0x7012)));
        profile.tune.frequency = 118_025_000;
        assert!(matches!(resolve(profile, settings), Err(0x7012)));
        profile.tune.frequency = 1_000_000_000;
        assert!(matches!(resolve(profile, settings), Err(0x7012)));
    } else {
        assert!(matches!(resolve(profile, settings), Err(0x700F)));
        settings.upper = true;
        settings.delta = 40;
        assert!(matches!(resolve(profile, settings), Err(0x700F)));
    }

    // Enabling the global setting cannot shift ordinary FM transmission.
    profile.tune.frequency = 145_500_000;
    profile.tune.am = false;
    profile.transmit = Tx::Minus(600_000);
    assert!(matches!(resolve(profile, settings), Ok(144_900_000)));
    profile.transmit = Tx::Simplex;
    assert!(matches!(resolve(profile, settings), Ok(145_500_000)));
};