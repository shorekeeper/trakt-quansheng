//! Saved autosave, scan, and Spectrum preferences.
//!
//! Snapshot words 96..107 store autosave mode, dwell, hold, resume policy,
//! scan bounds and direction, skipped channels, and Spectrum settings.
//! Validation rejects unknown modes, invalid ranges, and oversized masks.
//!
//! Defaults are applied when an admitted record has no assigned preference
//! fields. Loading performs no migration write. Spectrum preferences remain
//! validated and serialized even when its runtime module is absent, so a
//! composition change does not erase them. Reset RAM selects defaults.
//!
//! Autosave waits at least 2000 ticks after a settings interaction and keeps
//! a 30000-tick cooldown after an automatic attempt. A failed automatic Save
//! blocks further automatic attempts until a validated Load or successful
//! Save establishes a usable state. This is scheduling policy, not a retry
//! mechanism inside the Flash driver.
//!
//! Spectrum thresholds are independent of normal carrier SQL. They describe
//! raw sequential observations, not calibrated power or spectral measurements.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Preferences {
    pub(crate) autosave: u8,
    pub(crate) dwell: u8,
    pub(crate) hold: u8,
    pub(crate) resume: u8,
    pub(crate) lower: u32,
    pub(crate) upper: u32,
    pub(crate) reverse: bool,
    pub(crate) skipped: u16,
    pub(crate) spectrum_step: u8,
    pub(crate) spectrum_rssi: u16,
    pub(crate) spectrum_noise: u8,
    pub(crate) spectrum: bool,
}

const fn band(frequency: u32) -> u32 {
    crate::profile::receive_span(frequency) as u32
}

impl Preferences {
    pub(crate) const fn new() -> Self {
        Self {
            autosave: 1,
            dwell: 1,
            hold: 1,
            resume: 0,
            lower: 136_000_000,
            upper: 174_000_000,
            reverse: false,
            skipped: 0,
            spectrum_step: 1,
            spectrum_rssi: 160,
            spectrum_noise: 64,
            spectrum: false,
        }
    }

    pub(crate) const fn valid(self) -> bool {
        let lower_band = band(self.lower);
        let upper_band = band(self.upper);

        self.autosave <= 2
            && self.dwell <= 3
            && self.hold <= 3
            && self.resume <= 2
            && lower_band != 0
            && lower_band == upper_band
            && self.lower < self.upper
            && self.spectrum_step <= 2
            && self.spectrum_rssi <= 511
            && self.spectrum_noise <= 127
    }

    pub(crate) const fn dwell_ticks(self) -> u32 {
        [200, 300, 500, 1000][self.dwell as usize]
    }

    pub(crate) const fn hold_ticks(self) -> u32 {
        [300, 500, 1000, 2000][self.hold as usize]
    }

    pub(crate) const fn skipped(self, slot: usize) -> bool {
        slot < 16 && self.skipped & (1u16 << slot) != 0
    }

    pub(crate) const fn set_skipped(&mut self, slot: usize, skipped: bool) {
        assert!(slot < 16);

        let mask = 1u16 << slot;

        if skipped {
            self.skipped |= mask;
        } else {
            self.skipped &= !mask;
        }
    }

    pub(crate) const fn write(self, words: &mut [u32; 148]) {
        assert!(self.valid());

        words[96] = self.autosave as u32;
        words[97] = self.dwell as u32;
        words[98] = self.hold as u32;
        words[99] = self.resume as u32;
        words[100] = self.lower;
        words[101] = self.upper;
        words[102] = self.reverse as u32;
        words[103] = self.skipped as u32;
        words[104] = self.spectrum_step as u32;
        words[105] = self.spectrum_rssi as u32;
        words[106] = self.spectrum_noise as u32;
        words[107] = self.spectrum as u32;
    }

    pub(crate) const fn read(words: &[u32; 148]) -> Option<Self> {
        if words[96] > 2
            || words[97] > 3
            || words[98] > 3
            || words[99] > 2
            || words[102] > 1
            || words[103] > 65535
            || words[104] > 2
            || words[105] > 511
            || words[106] > 127
            || words[107] > 1
        {
            return None;
        }

        let preferences = Self {
            autosave: words[96] as u8,
            dwell: words[97] as u8,
            hold: words[98] as u8,
            resume: words[99] as u8,
            lower: words[100],
            upper: words[101],
            reverse: words[102] != 0,
            skipped: words[103] as u16,
            spectrum_step: words[104] as u8,
            spectrum_rssi: words[105] as u16,
            spectrum_noise: words[106] as u8,
            spectrum: words[107] != 0,
        };

        if preferences.valid() {
            Some(preferences)
        } else {
            None
        }
    }
}

pub(crate) struct Autosave {
    changed_at: u32,
    attempted_at: u32,
    attempted: bool,
    failed: bool,
}

impl Autosave {
    pub(crate) const fn new() -> Self {
        Self {
            changed_at: 0,
            attempted_at: 0,
            attempted: false,
            failed: false,
        }
    }

    pub(crate) fn changed(&mut self, now: u32) {
        self.changed_at = now;
    }

    pub(crate) fn checked(&mut self, now: u32) {
        self.changed_at = now;
        self.failed = false;
    }

    pub(crate) fn failed(&mut self) {
        self.failed = true;
    }

    /// No automatic retry follows an error. A recent settings interaction
    /// postpones saving, and every attempted automatic write starts the
    /// thirty-second cooldown even if the driver later rejects it.
    pub(crate) const fn due(
        &self,
        now: u32,
        mode: u8,
        dirty: bool,
        eligible: bool,
    ) -> bool {
        mode == 2
            && dirty
            && eligible
            && !self.failed
            && now.wrapping_sub(self.changed_at) >= 2000
            && (!self.attempted || now.wrapping_sub(self.attempted_at) >= 30000)
    }

    pub(crate) fn attempted(&mut self, now: u32) {
        self.attempted_at = now;
        self.attempted = true;
    }
}

const _: () = {
    let defaults = Preferences::new();
    assert!(defaults.valid());
    assert!(defaults.dwell_ticks() == 300);
    assert!(defaults.hold_ticks() == 500);

    let mut words = [0u32; 148];
    defaults.write(&mut words);

    let restored = match Preferences::read(&words) {
        Some(restored) => restored,
        None => panic!("default preferences rejected"),
    };

    assert!(restored.autosave == 1);
    assert!(restored.lower == 136_000_000);
    assert!(restored.upper == 174_000_000);
    assert!(!restored.spectrum);

    let mut channels = defaults;
    channels.set_skipped(0, true);
    channels.set_skipped(15, true);
    assert!(channels.skipped(0));
    assert!(channels.skipped(15));
    assert!(!channels.skipped(1));
    channels.set_skipped(0, false);
    assert!(!channels.skipped(0));
    assert!(channels.skipped(15));

    words[103] = 65536;
    assert!(Preferences::read(&words).is_none());

    defaults.write(&mut words);
    words[101] = 433_500_000;
    assert!(Preferences::read(&words).is_none());

    defaults.write(&mut words);
    words[100] = words[101];
    assert!(Preferences::read(&words).is_none());

    let quiet = Autosave::new();
    assert!(!quiet.due(1999, 2, true, true));
    assert!(quiet.due(2000, 2, true, true));
    assert!(!quiet.due(2000, 1, true, true));
    assert!(!quiet.due(2000, 0, true, true));
    assert!(!quiet.due(2000, 2, false, true));
    assert!(!quiet.due(2000, 2, true, false));

    let cooled = Autosave {
        changed_at: 1000,
        attempted_at: 2000,
        attempted: true,
        failed: false,
    };

    assert!(!cooled.due(31999, 2, true, true));
    assert!(cooled.due(32000, 2, true, true));

    let blocked = Autosave {
        failed: true,
        ..Autosave::new()
    };

    assert!(!blocked.due(100000, 2, true, true));
};
const _: () = {
    let high = Preferences {
        lower: 840_000_000, upper: 1_300_000_000,
        ..Preferences::new()
    };
    assert!(high.valid());
    let mut gap = high;
    gap.lower = 500_000_000;
    assert!(!gap.valid());
    gap.lower = 630_000_000;
    assert!(!gap.valid());
};
