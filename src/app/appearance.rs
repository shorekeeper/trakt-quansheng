//! Saved presentation choices in one checked word.
//!
//! Bits 0..16 retain the base presentation fields. Three four-bit row
//! assignments occupy 17..28. Row B is XOR-coded with one and row C with
//! two, so a zero extension selects Auto 1, Auto 2, Auto 3. Historical
//! presentation words therefore retain useful defaults without a write.
//! Bits 29..31 and unknown field values are rejected.
//!
//! Auto rows select TX/detail/meter for ordinary reception and RX text,
//! TX text, and key status for CW. Explicit assignments may duplicate or
//! hide rows. They do not move the fixed identity, primary field, or warning
//! footer, and cannot conceal those operational warnings.
//! Changing presentation does not configure RF.
//!
//! Layout: Clean, Panel, Detail. Primary: frequency or channel name.
//! Detail: profile, raw signal, battery, other VFO, uptime, or hidden.
//! Meter: hidden, continuous bar, or segments.
//! Battery: icon, voltage, or hidden. Menu: cursor, inverse, or outline.
//! Beeps: off, keys, actions, or all. TX visual: off, level, or history.
//! Hints controls optional explanations, not error or policy warnings.

use super::menu::Setting;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) layout: u8,
    pub(crate) primary: u8,
    pub(crate) detail: u8,
    pub(crate) meter: u8,
    pub(crate) battery: u8,
    pub(crate) menu: u8,
    pub(crate) beeps: u8,
    pub(crate) tx_visual: u8,
    pub(crate) hints: bool,
    pub(crate) rows: [u8; 3],
}

impl Settings {
    pub(crate) const fn new() -> Self {
        Self {
            layout: 0,
            primary: 0,
            detail: 0,
            meter: 1,
            battery: 0,
            menu: 1,
            beeps: 0,
            tx_visual: 0,
            hints: false,
            rows: [0, 1, 2],
        }
    }

    pub(crate) const fn word(self) -> u32 {
        self.layout as u32
            | ((self.primary as u32) << 2)
            | ((self.detail as u32) << 3)
            | ((self.meter as u32) << 6)
            | ((self.battery as u32) << 8)
            | ((self.menu as u32) << 10)
            | ((self.beeps as u32) << 12)
            | ((self.tx_visual as u32) << 14)
            | ((self.hints as u32) << 16)
            | ((self.rows[0] as u32) << 17)
            | (((self.rows[1] ^ 1) as u32) << 21)
            | (((self.rows[2] ^ 2) as u32) << 25)
    }

    /// Decode the complete presentation word through one runtime boundary.
    /// Snapshot admission and Panel restoration use identical field checks.
    #[inline(never)]
    pub(crate) const fn decode(word: u32) -> Option<Self> {
        let value = Self {
            layout: (word & 3) as u8,
            primary: ((word >> 2) & 1) as u8,
            detail: ((word >> 3) & 7) as u8,
            meter: ((word >> 6) & 3) as u8,
            battery: ((word >> 8) & 3) as u8,
            menu: ((word >> 10) & 3) as u8,
            beeps: ((word >> 12) & 3) as u8,
            tx_visual: ((word >> 14) & 3) as u8,
            hints: word & (1 << 16) != 0,
            rows: [
                ((word >> 17) & 15) as u8,
                (((word >> 21) & 15) as u8) ^ 1,
                (((word >> 25) & 15) as u8) ^ 2,
            ],
        };

        if word & !0x1FFF_FFFF != 0
            || value.rows[0] >= 14
            || value.rows[1] >= 14
            || value.rows[2] >= 14
            || value.layout > 2
            || value.detail > 5
            || value.meter > 2
            || value.battery > 2
            || value.menu > 2
            || value.tx_visual > 2
        {
            None
        } else {
            Some(value)
        }
    }

    pub(super) const fn configure(&mut self, setting: Setting, value: u32) -> bool {
        match setting {
            Setting::HomeLayout if value < 3 => self.layout = value as u8,
            Setting::HomePrimary if value < 2 => self.primary = value as u8,
            Setting::HomeDetail if value < 6 => self.detail = value as u8,
            Setting::HomeMeter if value < 3 => self.meter = value as u8,
            Setting::BatteryDisplay if value < 3 => self.battery = value as u8,
            Setting::MenuStyle if value < 3 => self.menu = value as u8,
            Setting::UiBeep if value < 4 => self.beeps = value as u8,
            Setting::TxVisual if value < 3 => self.tx_visual = value as u8,
            Setting::UiHints if value < 2 => self.hints = value != 0,
            Setting::HomeRowA if value < 14 => self.rows[0] = value as u8,
            Setting::HomeRowB if value < 14 => self.rows[1] = value as u8,
            Setting::HomeRowC if value < 14 => self.rows[2] = value as u8,
            _ => return false,
        }

        true
    }
}

const _: () = {
    let original = Settings::new();

    let restored = match Settings::decode(original.word()) {
        Some(value) => value,
        None => panic!("appearance defaults rejected"),
    };

    assert!(restored.layout == 0);
    assert!(restored.primary == 0);
    assert!(restored.meter == 1);
    assert!(restored.menu == 1);
    assert!(restored.beeps == 0);
    assert!(!restored.hints);

    assert!(Settings::decode(1 << 29).is_none());
    assert!(Settings::decode(3).is_none());
    assert!(Settings::decode(6 << 3).is_none());
    assert!(Settings::decode(3 << 6).is_none());
    assert!(Settings::decode(3 << 8).is_none());
    assert!(Settings::decode(3 << 10).is_none());
    assert!(Settings::decode(3 << 14).is_none());

    let mut settings = Settings::new();

    assert!(settings.configure(Setting::UiBeep, 3));
    assert!(settings.beeps == 3);
    assert!(!settings.configure(Setting::UiBeep, 4));
    assert!(settings.beeps == 3);
    assert!(!settings.configure(Setting::Rx, 1));

    assert!(settings.configure(Setting::TxVisual, 2));
    assert!(settings.configure(Setting::HomePrimary, 1));
    assert!(settings.configure(Setting::UiHints, 1));

    let checked = match Settings::decode(settings.word()) {
        Some(value) => value,
        None => panic!("appearance roundtrip failed"),
    };

    assert!(checked.tx_visual == 2);
    assert!(checked.primary == 1);
    assert!(checked.hints);

    assert!(restored.rows[0] == 0);
    assert!(restored.rows[1] == 1);
    assert!(restored.rows[2] == 2);
    assert!(original.word() & !0x1FFFF == 0);

    let choices = [Setting::HomeRowA, Setting::HomeRowB, Setting::HomeRowC];
    let mut row = 0;
    while row < choices.len() {
        let mut selection = 0;
        while selection < 14 {
            let mut value = Settings::new();
            assert!(value.configure(choices[row], selection));
            let loaded = match Settings::decode(value.word()) {
                Some(value) => value,
                None => panic!("Home row rejected"),
            };
            assert!(loaded.rows[row] == selection as u8);
            assert!(loaded.layout == original.layout);
            assert!(loaded.meter == original.meter);
            selection += 1;
        }

        let mut value = Settings::new();
        assert!(!value.configure(choices[row], 14));
        row += 1;
    }
};
