//! CW settings, contact filtering, and bounded electronic-key timing.
//!
//! Modes zero and one select a straight key on the first or second contact.
//! Local contacts are PTT and Side1; USB contacts are tip/D+ and ring/D-.
//! Modes two and three select Iambic A or B. The first contact is the
//! default dot paddle; reverse exchanges dot and dash. Straight modes
//! ignore reverse and the unselected contact.
//!
//! Input zero selects Local, one USB Test, and two USB Key. USB Test exposes
//! contacts but must not request transmission. USB Key uses the same keyer
//! as Local, including independent squeeze detection and A/B memory rules.
//! Panel saves the checked settings word. Loading choices never restores
//! contact qualification, keyer memory, a pending element, or a live session.
//! Changing the input choice requires a new complete release interval.
//!
//! The nineteen-bit word stores mode in 0..1, reverse in 2, WPM minus five
//! in 3..7, pitch steps above 400 Hz in 8..10, hang selection in 11..14,
//! gated audio in 15, input in 16..17, and QSK disable in 18. XOR with
//! the default field image makes zero the canonical default. Unknown
//! fields and values are rejected in every firmware composition.
//! Speed indices 0..30 select 5..35 WPM; index 31 remains reserved.
//! The default speed is 15 WPM. Keyer and both decoders use the same
//! checked selection and rounded dot duration. Record admission separately
//! restricts speed fields in formats which assign only 5..25 WPM.
//! QSK On uses delayed receive restoration between keying sessions;
//! QSK Off selects local practice. This is not full per-element break-in.
//!
//! Automatic dots last round(1200 / WPM) observed milliseconds. Dashes
//! last three units; the gap after an automatic element lasts one unit.
//! Held opposite paddles alternate elements. Brief opposite-paddle input
//! during an element is retained through its following gap.
//!
//! Releasing a squeeze in A finishes the current element without another.
//! B additionally retains one opposite element. A squeeze release during
//! the following element gap uses the same rule. Memory belongs only to
//! this keyer instance and is discarded by construction or cancellation.
//!
//! A late poll makes at most one timed phase transition. The next phase starts
//! at observation time, so a delayed foreground pass cannot compress a missing
//! gap into rapid RF transitions. A pending electronic mark requests TX but
//! starts its complete dot/dash duration only after the RF owner is ready.
//! Opposite-paddle and squeeze memory continue during that pending interval.
//! Straight keying remains live and is not prolonged by a later T/R return.
//! These are observed ticks, not measurements of the transmitted envelope.
//!
//! Contact assertion requires five observed milliseconds with a stable
//! candidate. Release is immediate at the next sample. A service gap
//! above twenty milliseconds clears the contact instead of retaining a
//! stale asserted key. Side1 sampling freshness is checked by its caller.
//!
//! RX frequency is nominal minus pitch. Both frequencies must belong to
//! the same admitted contiguous receive interval. Pitch is an integer
//! 400..1000 Hz in 100 Hz steps; the saved tuning profile is not changed.
//! gated selects optional RSSI-qualified speaker audio. Open audio remains
//! the default; decoding has its own envelope observations in either mode.
//!
//! These values and state machines perform no device access, allocate no
//! static buffer, and cannot authorize transmission. The RF owner retains
//! calibration, channel admission, cancellation, and complete-session TOT.

use crate::app::time::reached;

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub(crate) mode: u8,
    pub(crate) reverse: bool,
    pub(crate) wpm: u8,
    pub(crate) pitch: u16,
    pub(crate) hang: u16,
    pub(crate) gated: bool,
    pub(crate) input: u8,
    pub(crate) qsk: bool,
}

impl Settings {
    pub(crate) const fn new() -> Self {
        Self {
            mode: 2, reverse: false, wpm: 15, pitch: 700, hang: 700,
            gated: false, input: 0, qsk: true,
        }
    }

    pub(crate) const fn valid(self) -> bool {
        self.mode < 4 && self.input < 3
            && self.wpm >= 5 && self.wpm <= 35
            && valid_pitch(self.pitch)
            && (matches!(self.hang, 50 | 100 | 200)
                || (self.hang >= 300 && self.hang <= 1500
                    && self.hang % 100 == 0))
    }

    /// Hang selections retain codes 0..12 for 300..1500 ticks.
    /// Codes 13..15 select 50, 100, and 200 ticks. A shorter selected
    /// delay does not eliminate receiver settling or TX preparation.
    pub(crate) const fn hang_at(index: u32) -> u16 {
        assert!(index < 16);
        match index {
            13 => 50,
            14 => 100,
            15 => 200,
            _ => 300 + index as u16 * 100,
        }
    }

    pub(crate) const fn hang_index(self) -> u32 {
        match self.hang {
            50 => 13,
            100 => 14,
            200 => 15,
            value => {
                assert!(value >= 300 && value <= 1500 && value % 100 == 0);
                ((value - 300) / 100) as u32
            }
        }
    }

    /// Eight absolute menu fields: key, reversal, WPM, pitch, hang,
    /// RX speaker gate, contact source, and QSK/RF selection.
    /// QSK Off means local practice; it never admits a radiated session.
    pub(crate) const fn configure(&mut self, field: usize, value: u32) -> bool {
        match field {
            0 if value < 4 => self.mode = value as u8,
            1 if value < 2 => self.reverse = value != 0,
            2 if value < 31 => self.wpm = 5 + value as u8,
            3 if value < 7 => self.pitch = 400 + value as u16 * 100,
            4 if value < 16 => self.hang = Self::hang_at(value),
            5 if value < 2 => self.gated = value != 0,
            6 if value < 3 => self.input = value as u8,
            7 if value < 2 => self.qsk = value != 0,
            _ => return false,
        }
        true
    }

    /// Relative steps use the same checked vocabulary as the native menu.
    /// This helper changes choices only; the caller discards live state.
    pub(crate) const fn change(&mut self, field: usize, increase: bool) {
        assert!(self.valid() && field < 8);
        let (current, count) = match field {
            0 => (self.mode as u32, 4),
            1 => (self.reverse as u32, 2),
            2 => (self.wpm as u32 - 5, 31),
            3 => ((self.pitch as u32 - 400) / 100, 7),
            4 => (self.hang_index(), 16),
            5 => (self.gated as u32, 2),
            6 => (self.input as u32, 3),
            _ => (self.qsk as u32, 2),
        };
        let next = if increase {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        assert!(self.configure(field, next));
    }

    /// Nineteen checked bits. Bits 0..17 retain the field layout with
    /// default XOR coding. Bit 18 stores QSK disable, so zero still means
    /// the complete default selection with RF enabled.
    #[inline(never)]
    pub(crate) const fn word(self) -> u32 {
        assert!(self.valid());
        let fields = self.mode as u32
            | ((self.reverse as u32) << 2)
            | (((self.wpm - 5) as u32) << 3)
            | ((((self.pitch - 400) / 100) as u32) << 8)
            | (self.hang_index() << 11)
            | ((self.gated as u32) << 15)
            | ((self.input as u32) << 16)
            | (((!self.qsk) as u32) << 18);
        fields ^ (2 | (10 << 3) | (3 << 8) | (4 << 11))
    }

    #[inline(never)]
    pub(crate) const fn decode(word: u32) -> Option<Self> {
        if word & !0x7FFFF != 0 {
            return None;
        }
        let fields = word ^ (2 | (10 << 3) | (3 << 8) | (4 << 11));
        let value = Self {
            mode: (fields & 3) as u8,
            reverse: fields & 4 != 0,
            wpm: 5 + ((fields >> 3) & 31) as u8,
            pitch: 400 + ((fields >> 8) & 7) as u16 * 100,
            hang: Self::hang_at((fields >> 11) & 15),
            gated: fields & (1 << 15) != 0,
            input: ((fields >> 16) & 3) as u8,
            qsk: fields & (1 << 18) == 0,
        };
        if value.valid() { Some(value) } else { None }
    }

    /// Speed-field restriction for records assigning 5..25 WPM.
    /// Complete word validation belongs to decode; this helper checks only
    /// the XOR-coded speed field without decoding another settings object.
    pub(crate) const fn speed25(word: u32) -> bool {
        (((word >> 3) & 31) ^ 10) <= 20
    }

    /// Historical settings assign eighteen bits and only hang codes 0..12.
    /// A recomputed record CRC cannot assign practice or shorter break-in
    /// delays to a format that did not define them.
    pub(crate) const fn legacy(word: u32) -> bool {
        let fields = word ^ (2 | (10 << 3) | (3 << 8) | (4 << 11));
        word & !0x3FFFF == 0
            && (fields >> 11) & 15 <= 12
            && Self::speed25(word)
            && Self::decode(word).is_some()
    }

    pub(crate) const fn usb_input(self) -> bool {
        self.input != 0
    }

    pub(crate) const fn testing(self) -> bool {
        self.input == 1
    }

    pub(crate) const fn input_name(self) -> &'static [u8] {
        match self.input {
            0 => b"LOCAL",
            1 => b"USB TEST",
            2 => b"USB KEY",
            _ => b"INVALID",
        }
    }

    pub(crate) const fn mode_name(self) -> &'static [u8] {
        if self.usb_input() && self.mode < 2 {
            if self.mode == 0 { b"STRAIGHT TIP" } else { b"STRAIGHT RING" }
        } else {
            mode_name(self.mode)
        }
    }

    /// Bit zero denotes dot and bit one dash. Arguments are contacts of
    /// the selected source. Straight modes ignore reversal and contact two
    /// or one respectively. Test and practice admission belong to owners.
    pub(crate) const fn contacts(self, first: bool, second: bool) -> u8 {
        match self.mode {
            0 => first as u8,
            1 => second as u8,
            _ if self.reverse => second as u8 | ((first as u8) << 1),
            _ => first as u8 | ((second as u8) << 1),
        }
    }
}

pub(crate) const fn mode_name(mode: u8) -> &'static [u8] {
    match mode {
        0 => b"STRAIGHT PTT",
        1 => b"STRAIGHT SIDE1",
        2 => b"IAMBIC A",
        3 => b"IAMBIC B",
        _ => b"INVALID",
    }
}

pub(crate) const fn valid_pitch(pitch: u16) -> bool {
    pitch >= 400 && pitch <= 1000 && pitch % 100 == 0
}

pub(crate) const fn rx_frequency(nominal: u32, pitch: u16) -> Option<u32> {
    if !valid_pitch(pitch) {
        return None;
    }

    let actual = match nominal.checked_sub(pitch as u32) {
        Some(actual) => actual,
        None => return None,
    };
    let span = crate::profile::receive_span(nominal);

    if span != 0 && span == crate::profile::receive_span(actual) {
        Some(actual)
    } else {
        None
    }
}

pub(crate) struct Contact {
    candidate: bool,
    since: u32,
    held: bool,
    last: u32,
    sampled: bool,
}

impl Contact {
    pub(crate) const fn new() -> Self {
        Self {
            candidate: false, since: 0, held: false,
            last: 0, sampled: false,
        }
    }

    pub(crate) const fn sample(&mut self, now: u32, raw: bool) -> bool {
        if self.sampled && now.wrapping_sub(self.last) > 20 {
            *self = Self::new();
        }

        self.sampled = true;
        self.last = now;

        if !raw {
            self.candidate = false;
            self.held = false;
            self.since = now;
        } else {
            if !self.candidate {
                self.candidate = true;
                self.since = now;
            }
            if now.wrapping_sub(self.since) >= 5 {
                self.held = true;
            }
        }

        self.held
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Pending,
    Mark,
    Gap,
}

pub(crate) struct Keyer {
    phase: Phase,
    deadline: u32,
    last_element: u8,
    memory: u8,
    previous: u8,
    squeezed: bool,
}

impl Keyer {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            deadline: 0,
            last_element: 2,
            memory: 0,
            previous: 0,
            squeezed: false,
        }
    }

    pub(crate) const fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle) || self.memory != 0
    }

    /// Request TX preparation before a scheduled electronic gap expires.
    /// This does not start an element or consume paddle memory. Only an
    /// already observed held contact or retained element can request early
    /// preparation. An unexpected later closure cannot be predicted.
    ///
    /// advance is the RF owner's observed turnaround allowance in ticks.
    /// Preparation can reduce the available receive window but cannot
    /// shorten the keyer's complete inter-element gap.
    pub(crate) const fn prepare_tx(
        &self, now: u32, keys: u8, advance: u32,
    ) -> bool {
        assert!(keys < 4);
        match self.phase {
            Phase::Mark | Phase::Pending => true,
            Phase::Gap => {
                let remaining = self.deadline.wrapping_sub(now);
                keys | self.memory != 0
                    && (reached(now, self.deadline) || remaining <= advance)
            }
            Phase::Idle => false,
        }
    }

    const fn unit(settings: Settings) -> u32 {
        (1200 + settings.wpm as u32 / 2) / settings.wpm as u32
    }

    /// Ordinary prepared-TX timing. The readiness-aware entry below is used
    /// when a receive window can postpone the start of an electronic mark.
    pub(crate) const fn poll(
        &mut self, now: u32, keys: u8, settings: Settings,
    ) -> bool {
        self.poll_ready(now, keys, settings, true)
    }

    /// True requests a mark, including a pending mark which needs TX restored.
    /// mark_ready starts an electronic element's duration, not its permission.
    /// RF admission and the original complete-session cutoff remain external.
    pub(crate) const fn poll_ready(
        &mut self,
        now: u32,
        keys: u8,
        settings: Settings,
        mark_ready: bool,
    ) -> bool {
        assert!(keys < 4 && settings.valid());

        if settings.mode < 2 {
            self.phase = if keys & 1 != 0 { Phase::Mark } else { Phase::Idle };
            self.memory = 0;
            self.previous = keys;
            self.squeezed = false;
            return keys & 1 != 0;
        }

        if !matches!(self.phase, Phase::Idle) {
            let opposite = self.last_element ^ 3;
            self.memory |= (keys & !self.previous) | (keys & opposite);
            if keys == 3 {
                self.squeezed = true;
            }

            if keys == 0 && self.previous != 0 && self.squeezed {
                self.memory = if settings.mode == 3 { opposite } else { 0 };
                self.squeezed = false;
            }
        }
        self.previous = keys;

        match self.phase {
            Phase::Pending => {
                if mark_ready {
                    self.phase = Phase::Mark;
                    let units = if self.last_element == 1 { 1 } else { 3 };
                    self.deadline = now.wrapping_add(Self::unit(settings) * units);
                }
                return true;
            }
            Phase::Mark => {
                if reached(now, self.deadline) {
                    self.phase = Phase::Gap;
                    self.deadline = now.wrapping_add(Self::unit(settings));
                    return false;
                }
                return true;
            }
            Phase::Gap => {
                if !reached(now, self.deadline) {
                    return false;
                }
                self.phase = Phase::Idle;
            }
            Phase::Idle => {}
        }

        let available = keys | self.memory;
        if available == 0 {
            self.squeezed = false;
            return false;
        }

        let opposite = self.last_element ^ 3;
        let selected = if available & opposite != 0 {
            opposite
        } else {
            self.last_element
        };

        self.last_element = selected;
        self.memory = 0;
        self.squeezed = keys == 3;
        self.phase = if mark_ready { Phase::Mark } else { Phase::Pending };
        if mark_ready {
            let units = if selected == 1 { 1 } else { 3 };
            self.deadline = now.wrapping_add(Self::unit(settings) * units);
        }
        true
    }
}

const _: () = {
    let defaults = Settings::new();
    assert!(defaults.valid());
    assert!(!defaults.gated);
    assert!(defaults.contacts(true, false) == 1);
    assert!(defaults.contacts(false, true) == 2);
    assert!(defaults.contacts(true, true) == 3);

    let mut reversed = defaults;
    reversed.reverse = true;
    assert!(reversed.contacts(true, false) == 2);
    assert!(reversed.contacts(false, true) == 1);

    let mut straight = defaults;
    straight.mode = 0;
    assert!(straight.contacts(false, true) == 0);
    straight.mode = 1;
    assert!(straight.contacts(true, false) == 0);

    let mut contact = Contact::new();
    assert!(!contact.sample(0, true));
    assert!(!contact.sample(4, true));
    assert!(contact.sample(5, true));
    assert!(!contact.sample(6, false));
    assert!(!contact.sample(7, true));
    assert!(contact.sample(12, true));
    assert!(!contact.sample(40, true));
};

const _: () = {
    let defaults = Settings::new();
    assert!(defaults.word() == 0);

    let restored = match Settings::decode(0) {
        Some(value) => value,
        None => panic!("CW defaults rejected"),
    };
    assert!(restored.valid());
    assert!(restored.mode == 2 && restored.wpm == 15);
    assert!(restored.pitch == 700 && restored.hang == 700);
    assert!(!restored.usb_input() && !restored.testing());
    assert!(!restored.input_name().is_empty());
    assert!(!restored.mode_name().is_empty());

    let mut value = defaults;
    value.change(6, true);
    assert!(value.testing());
    assert!(Settings::decode(value.word()).is_some());
    assert!(Settings::decode(1 << 19).is_none());
    assert!(Settings::legacy(Settings::new().word()));

    // These fixtures also type-check the concrete timing vocabulary when
    // a composition retains settings but has no reachable CW RF owner.
    let mut configured = Settings::new();
    assert!(configured.configure(2, 30));
    assert!(configured.wpm == 35);
    assert!(!configured.configure(2, 31));
    assert!(configured.wpm == 35);
    assert!(!configured.configure(8, 0));
    assert!(configured.configure(7, 0));
    assert!(!configured.qsk);
    assert!(!Settings::legacy(configured.word()));
    assert!(configured.valid());

    let mut keyer = Keyer::new();
    assert!(!keyer.busy());
    assert!(!keyer.poll(0, 0, defaults));
    assert!(matches!(rx_frequency(145_500_000, 700), Some(145_499_300)));
};

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: u8) -> Settings {
        Settings { mode, wpm: 20, ..Settings::new() }
    }

    #[test]
    fn straight_keys_follow_only_the_selected_contact() {
        for mode in 0..2 {
            let s = settings(mode);
            let mut keyer = Keyer::new();
            let keys = s.contacts(mode == 0, mode == 1);
            assert!(keyer.poll(0, keys, s));
            assert!(keyer.busy());
            assert!(!keyer.poll(1, 0, s));
            assert!(!keyer.busy());
        }
    }

    #[test]
    fn held_squeeze_alternates_with_full_gaps() {
        let s = settings(2);
        let mut k = Keyer::new();
        assert!(k.poll(0, 3, s));
        assert!(k.poll(59, 3, s));
        assert!(!k.poll(60, 3, s));
        assert!(!k.poll(119, 3, s));
        assert!(k.poll(120, 3, s));
        assert!(k.poll(299, 3, s));
        assert!(!k.poll(300, 3, s));
        assert!(k.poll(360, 3, s));
    }

    #[test]
    fn squeeze_release_distinguishes_a_and_b() {
        for release in [10u32, 80] {
            for mode in [2u8, 3] {
                let s = settings(mode);
                let mut k = Keyer::new();
                assert!(k.poll(0, 3, s));

                if release > 60 {
                    assert!(!k.poll(60, 3, s));
                    assert!(!k.poll(release, 0, s));
                } else {
                    assert!(k.poll(release, 0, s));
                    assert!(!k.poll(60, 0, s));
                }

                assert_eq!(k.poll(120, 0, s), mode == 3);
                if mode == 3 {
                    assert!(k.poll(299, 0, s));
                    assert!(!k.poll(300, 0, s));
                    assert!(!k.poll(360, 0, s));
                }
                assert!(!k.busy());
            }
        }
    }

    #[test]
    fn opposite_tap_is_retained_without_a_squeeze() {
        let s = settings(2);
        let mut k = Keyer::new();
        assert!(k.poll(0, 1, s));
        assert!(k.poll(10, 2, s));
        assert!(k.poll(20, 0, s));
        assert!(!k.poll(60, 0, s));
        assert!(k.poll(120, 0, s));
        assert!(k.poll(299, 0, s));
        assert!(!k.poll(300, 0, s));
        assert!(!k.poll(360, 0, s));
        assert!(!k.busy());
    }

    #[test]
    fn early_preparation_preserves_the_scheduled_gap() {
        let s = settings(2);
        let mut k = Keyer::new();
        assert!(k.poll_ready(0, 1, s, true));
        assert!(!k.poll_ready(60, 1, s, true));
        assert!(!k.prepare_tx(99, 1, 20));
        assert!(k.prepare_tx(100, 1, 20));

        // Preparation is not a mark and cannot advance the gap deadline.
        assert!(!k.poll_ready(100, 1, s, true));
        assert!(!k.poll_ready(119, 1, s, true));
        assert!(k.poll_ready(120, 1, s, true));
        assert!(k.poll_ready(179, 1, s, true));
        assert!(!k.poll_ready(180, 1, s, true));
    }

    #[test]
    fn preparation_requires_a_held_or_remembered_element() {
        let s = settings(2);
        let mut k = Keyer::new();
        assert!(k.poll_ready(0, 1, s, true));
        assert!(!k.poll_ready(60, 0, s, true));
        assert!(!k.prepare_tx(110, 0, 20));

        let mut remembered = Keyer::new();
        assert!(remembered.poll_ready(0, 1, s, true));
        assert!(remembered.poll_ready(10, 2, s, true));
        assert!(remembered.poll_ready(20, 0, s, true));
        assert!(!remembered.poll_ready(60, 0, s, true));
        assert!(remembered.prepare_tx(100, 0, 20));
    }

    #[test]
    fn preparation_deadline_crosses_tick_wrap() {
        let s = settings(2);
        let start = u32::MAX - 70;
        let mut k = Keyer::new();
        assert!(k.poll_ready(start, 1, s, true));
        assert!(!k.poll_ready(start.wrapping_add(60), 1, s, true));
        assert!(!k.prepare_tx(start.wrapping_add(99), 1, 20));
        assert!(k.prepare_tx(start.wrapping_add(100), 1, 20));
        assert!(!k.poll_ready(start.wrapping_add(119), 1, s, true));
        assert!(k.poll_ready(start.wrapping_add(120), 1, s, true));
    }

    #[test]
    fn pending_element_keeps_its_full_duration_and_opposite_memory() {
        let s = settings(2);
        let mut k = Keyer::new();
        assert!(k.poll_ready(0, 1, s, false));
        assert!(k.poll_ready(20, 2, s, false));
        assert!(k.poll_ready(40, 0, s, false));
        assert!(k.poll_ready(100, 0, s, true));
        assert!(k.poll_ready(159, 0, s, true));
        assert!(!k.poll_ready(160, 0, s, true));
        assert!(!k.poll_ready(219, 0, s, true));
        assert!(k.poll_ready(220, 0, s, false));
        assert!(k.poll_ready(300, 0, s, true));
        assert!(k.poll_ready(479, 0, s, true));
        assert!(!k.poll_ready(480, 0, s, true));
        assert!(!k.poll_ready(540, 0, s, true));
        assert!(!k.busy());
    }

    #[test]
    fn straight_release_is_not_extended_by_rf_readiness() {
        let s = settings(0);
        let mut k = Keyer::new();
        assert!(k.poll_ready(0, 1, s, false));
        assert!(!k.poll_ready(1, 0, s, false));
        assert!(!k.poll_ready(2, 0, s, true));
        assert!(!k.busy());
    }

    #[test]
    fn delayed_poll_does_not_compress_a_gap() {
        let s = settings(2);
        let mut k = Keyer::new();
        assert!(k.poll(0, 1, s));
        assert!(!k.poll(200, 1, s));
        assert!(!k.poll(259, 1, s));
        assert!(k.poll(260, 1, s));
        assert!(k.poll(319, 1, s));
        assert!(!k.poll(320, 1, s));
    }

    #[test]
    fn deadlines_and_contacts_cross_counter_wrap() {
        let s = settings(2);
        let mut k = Keyer::new();
        assert!(k.poll(u32::MAX - 20, 1, s));
        assert!(k.poll(38, 1, s));
        assert!(!k.poll(39, 1, s));

        let mut c = Contact::new();
        assert!(!c.sample(u32::MAX - 2, true));
        assert!(c.sample(2, true));
        assert!(!c.sample(3, false));
        assert!(!c.sample(50, true));
    }

    #[test]
    fn settings_remain_valid_after_every_ui_step() {
        let mut s = Settings::new();
        for field in 0..8 {
            for increase in [false, true] {
                for _ in 0..64 {
                    s.change(field, increase);
                    assert!(s.valid());
                    assert_ne!(mode_name(s.mode), b"INVALID");
                }
            }
        }
        assert_eq!(mode_name(9), b"INVALID");
    }

    #[test]
    fn every_speed_roundtrips_without_extending_historical_fields() {
        for wpm in 5..=35 {
            let mut value = Settings::new();
            assert!(value.configure(2, wpm as u32 - 5));
            assert_eq!(value.wpm, wpm);
            assert!(value.valid());

            let restored = Settings::decode(value.word()).unwrap();
            assert_eq!(restored.wpm, wpm);
            assert_eq!(restored.word(), value.word());
            assert_eq!(Settings::speed25(value.word()), wpm <= 25);
            assert_eq!(Settings::legacy(value.word()), wpm <= 25);
        }

        let mut value = Settings::new();
        assert!(value.configure(2, 30));
        let before = value.word();
        assert!(!value.configure(2, 31));
        assert_eq!(value.word(), before);

        // Index 31 would mean 36 WPM and remains invalid even when all
        // other fields contain their canonical default representations.
        let reserved = ((31u32 ^ 10) << 3) | (before & !(31 << 3));
        assert!(Settings::decode(reserved).is_none());
    }

    #[test]
    fn settings_words_roundtrip_all_independent_controls() {
        assert_eq!(Settings::new().word(), 0);

        for field in 0..8 {
            let mut settings = Settings::new();
            for _ in 0..64 {
                settings.change(field, true);
                let loaded = Settings::decode(settings.word()).unwrap();
                assert_eq!(loaded.word(), settings.word());
                assert_eq!(loaded.mode, settings.mode);
                assert_eq!(loaded.reverse, settings.reverse);
                assert_eq!(loaded.wpm, settings.wpm);
                assert_eq!(loaded.pitch, settings.pitch);
                assert_eq!(loaded.hang, settings.hang);
                assert_eq!(loaded.gated, settings.gated);
                assert_eq!(loaded.input, settings.input);
            }
        }

        for word in 0..=0x7FFFF {
            if let Some(settings) = Settings::decode(word) {
                assert!(settings.valid());
                assert_eq!(settings.word(), word);
            }
        }
        assert!(Settings::decode(1 << 19).is_none());
        assert!(Settings::decode(u32::MAX).is_none());
    }

    #[test]
    fn input_selection_keeps_contact_mapping_and_test_state_explicit() {
        let mut s = Settings::new();
        assert!(!s.usb_input() && !s.testing());
        assert_eq!(s.input_name(), b"LOCAL");
        assert_eq!(s.mode_name(), b"IAMBIC A");

        s.change(6, true);
        assert!(s.usb_input() && s.testing());
        assert_eq!(s.input_name(), b"USB TEST");
        assert_eq!(s.contacts(true, false), 1);
        assert_eq!(s.contacts(false, true), 2);
        assert_eq!(s.contacts(true, true), 3);

        s.change(6, true);
        assert!(s.usb_input() && !s.testing());
        assert_eq!(s.input_name(), b"USB KEY");
        s.reverse = true;
        assert_eq!(s.contacts(true, false), 2);
        assert_eq!(s.contacts(false, true), 1);

        s.mode = 0;
        assert_eq!(s.mode_name(), b"STRAIGHT TIP");
        assert_eq!(s.contacts(true, false), 1);
        assert_eq!(s.contacts(false, true), 0);
        s.mode = 1;
        assert_eq!(s.mode_name(), b"STRAIGHT RING");
        assert_eq!(s.contacts(true, false), 0);
        assert_eq!(s.contacts(false, true), 1);

        s.change(6, true);
        assert_eq!(s.input_name(), b"LOCAL");
        assert_eq!(s.mode_name(), b"STRAIGHT SIDE1");
        s.change(6, false);
        assert_eq!(s.input, 2);
        assert!(s.valid());
        s.input = 3;
        assert!(!s.valid());
        assert_eq!(s.input_name(), b"INVALID");
    }

    #[test]
    fn receive_shift_cannot_cross_a_path_or_gap() {
        assert_eq!(rx_frequency(145_500_000, 700), Some(145_499_300));
        assert_eq!(rx_frequency(433_500_000, 700), Some(433_499_300));
        assert_eq!(rx_frequency(280_000_000, 700), None);
        assert_eq!(rx_frequency(840_000_000, 700), None);
        assert_eq!(rx_frequency(50_000_000, 700), None);
        assert_eq!(rx_frequency(145_500_000, 701), None);
    }
}