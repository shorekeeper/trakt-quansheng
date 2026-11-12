//! Checked VOX choices, hardware-event requests, and amplitude timing fixtures.
//!
//! CW firmware compositions retain the choice encoding for Save/Load
//! compatibility but omit operational VOX navigation and requests. Their
//! BK4829 comparator owner is absent, including during FM reception and
//! manual voice transmission. Non-CW compositions use the event gate and
//! concrete comparator owner described below. Pure timing fixtures remain
//! independent of firmware composition and do not authorize RF.
//!
//! Operational detection uses the BK4829 comparator events, not an assumed
//! microphone amplitude register during ordinary reception. The hardware
//! threshold field admits 1..2047. Native and graphical editors select the
//! same raw comparator threshold. Lower values increase sensitivity.
//! Default 128 is an operating choice, not measured microphone sensitivity.
//! No threshold denotes PCM, sound pressure, gain, or measured deviation.
//!
//! Persistence retains its sixteen-bit threshold representation. Values above
//! 2047 remain readable but cannot configure hardware or create a request.
//! Editing presents an admitted candidate; confirmation is required to replace
//! the retained value. No oversized threshold is masked into another value.
//!
//! Hang is 1..10 seconds, default three. One second is 1000 nominal SysTick
//! increments. Enable defaults off. The checked twenty-one-bit choice word
//! contains threshold in 0..15, hang minus one in 16..19, and enable in 20.
//! XOR coding makes zero select all defaults. Higher bits, zero threshold,
//! and hang codes 10..15 are rejected.
//!
//! Snapshot word 110 retains HAM disable in bit zero and stores this word
//! in bits 1..21. The record owner restricts historical use of those bits.
//! Neither a choice word nor a restored enabled choice creates a TX request.
//!
//! Session accepts only caller-qualified microphone amplitude observations.
//! The caller must establish microphone routing, register interpretation,
//! resource ownership, and fresh timestamps before supplying them. RX RSSI,
//! a VOX event of unknown meaning, and a cached TX meter are not substitutes.
//! The amplitude Session below has no firmware sampling call. Operational
//! EventGate instead consumes observations from the concrete BK4829 owners.
//!
//! Entry requires at least 100 observed ticks of continuous amplitude below
//! the closing threshold. Opening then requires ten ticks at or above the
//! selected threshold. Closing uses one eighth of the opening threshold as
//! hysteresis, with at least one count and a minimum closing threshold of one.
//! A session retains its request until the selected hang expires after its
//! last accepted observation at or above the closing threshold.
//!
//! An observation must be at most twenty ticks old. Accepted timestamps must
//! advance with gaps no greater than twenty ticks. Repeated timestamps cannot
//! qualify silence, complete attack, change amplitude, or renew hang. Missing,
//! future, stale, or discontinuous observations discard the session and its
//! release qualification. Normal hang expiry also requires fresh silence.
//!
//! The caller supplies eligibility and inhibition on every pass. Physical
//! PTT, EXIT cancellation, RF errors, timeout, boot audio, local feedback,
//! storage ownership, and unsupported operating modes must inhibit requests.
//! RF timeout must remain latched until this session has been cancelled;
//! an expiring UI error is not an adequate failure input.
//!
//! The output is a request only. A hardware owner must independently enforce
//! TX policy, carrier admission, calibration, battery, immutable attempt
//! settings, and the original TOT. Session cannot enable PA, fabricate a
//! physical PTT edge, schedule a packet, refresh TOT, or bypass RF cleanup.
//!
//! All state belongs to one foreground caller. There is no static allocation,
//! peripheral access, audio buffer, interrupt handler, or background task.

use crate::app::time::reached;

const DEFAULT_THRESHOLD: u16 = 128;
const DEFAULT_HANG: u8 = 3;
const WORD_MASK: u32 = 0x001F_FFFF;
const DEFAULT_FIELDS: u32 =
    DEFAULT_THRESHOLD as u32 | (((DEFAULT_HANG - 1) as u32) << 16);
const SAMPLE_LIMIT: u32 = 20;
const QUIET_TICKS: u32 = 100;
const ATTACK_TICKS: u32 = 10;

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
    pub(crate) threshold: u16,
    /// Hold after the last accepted voice observation, in whole seconds.
    pub(crate) hang: u8,
}

impl Settings {
    pub(crate) const fn new() -> Self {
        Self {
            enabled: false,
            threshold: DEFAULT_THRESHOLD,
            hang: DEFAULT_HANG,
        }
    }

    pub(crate) const fn valid(self) -> bool {
        self.threshold != 0 && self.hang >= 1 && self.hang <= 10
    }

    #[inline(never)]
    pub(crate) const fn word(self) -> u32 {
        assert!(self.valid());
        (self.threshold as u32
            | (((self.hang - 1) as u32) << 16)
            | ((self.enabled as u32) << 20))
            ^ DEFAULT_FIELDS
    }

    #[inline(never)]
    pub(crate) const fn decode(word: u32) -> Option<Self> {
        if word & !WORD_MASK != 0 {
            return None;
        }
        let fields = word ^ DEFAULT_FIELDS;
        let value = Self {
            enabled: fields & (1 << 20) != 0,
            threshold: (fields & 65535) as u16,
            hang: 1 + ((fields >> 16) & 15) as u8,
        };
        if value.valid() { Some(value) } else { None }
    }

    /// Menu fields are enable, raw threshold, and hang seconds.
    /// Threshold index zero means one count; hang index zero means one second.
    pub(crate) const fn count(field: usize) -> u32 {
        match field {
            0 => 2,
            1 => 2047,
            2 => 10,
            _ => panic!("unknown VOX setting"),
        }
    }

    pub(crate) const fn value(self, field: usize) -> u32 {
        assert!(self.valid());
        match field {
            0 => self.enabled as u32,
            1 => if self.threshold > 2047 { 2046 } else { self.threshold as u32 - 1 },
            2 => self.hang as u32 - 1,
            _ => panic!("unknown VOX setting"),
        }
    }

    pub(crate) const fn configure(&mut self, field: usize, value: u32) -> bool {
        if field > 2 || value >= Self::count(field) {
            return false;
        }
        match field {
            0 => self.enabled = value != 0,
            1 => self.threshold = value as u16 + 1,
            _ => self.hang = value as u8 + 1,
        }
        true
    }

    /// Graphical adjustments clamp instead of wrapping across the full range.
    pub(crate) const fn shift_threshold(&mut self, increase: bool, step: u32) {
        assert!(self.valid() && step != 0);
        let current = self.threshold as u32;
        let candidate = if increase {
            current.saturating_add(step)
        } else {
            current.saturating_sub(step)
        };
        self.threshold = if candidate == 0 {
            1
        } else if candidate > 2047 {
            2047
        } else {
            candidate as u16
        };
    }

    pub(crate) const fn close(self) -> u16 {
        assert!(self.valid());
        let margin = self.threshold / 8;
        let margin = if margin == 0 { 1 } else { margin };
        let closing = self.threshold.saturating_sub(margin);
        if closing == 0 { 1 } else { closing }
    }

    /// None preserves an unsupported saved value without truncating it.
    pub(crate) const fn hardware_threshold(self) -> Option<u16> {
        if self.threshold != 0 && self.threshold <= 2047 {
            Some(self.threshold)
        } else {
            None
        }
    }

    pub(crate) const fn hang_ticks(self) -> u32 {
        assert!(self.valid());
        self.hang as u32 * 1000
    }
}

/// One advancing foreground observation of the enabled hardware detector.
/// voice is unavailable until an actual opening or closing event arrives.
/// events retains the last nonzero VOX subset of a consumed event set.
/// Ordinary polls advance tick without erasing that event history. Zero
/// means no VOX event has been observed in this detector ownership period.
/// Neither events nor voice is a microphone-amplitude measurement.
#[derive(Clone, Copy)]
pub(crate) struct Sample {
    pub(crate) tick: u32,
    pub(crate) voice: Option<bool>,
    pub(crate) events: u16,
}

/// Operational request from the BK4829 comparator.
///
/// Initial entry accepts the first actual voice event without requiring a
/// preceding quiet event. Unknown detector state cannot invent voice.
/// Ten ticks of advancing asserted observations reject a one-poll assertion;
/// there is no separate initial silence timer.
///
/// Cancellation of an active request locks subsequent automatic admission.
/// A fresh known quiet observation releases that lock immediately, without
/// another silence dwell. Continuous voice therefore cannot restart after
/// timeout, failed admission, physical PTT cancellation, or input loss.
/// Cancellation while idle discards attack timing but adds no initial lock.
///
/// Setup handoff discards cached detector input while retaining only the
/// original finite hold deadline. No missing or unknown setup observation
/// renews hold. TX must provide its own actual comparator event to renew it.
///
/// completed is a one-pass normal outcome only when a continuous advancing
/// quiet observation reaches hold expiry. Missing input, unknown state,
/// inhibition, and setup expiry cannot authorize Roger. RF admission and
/// the immutable complete-attempt TOT belong to Transmitter, not this gate.
pub(crate) struct EventGate {
    word: u32,
    sampled: bool,
    last: u32,
    qualified: bool,
    attacking: bool,
    attack_at: u32,
    session: bool,
    until: u32,
    completed: bool,
}

impl EventGate {
    pub(crate) const fn new() -> Self {
        Self {
            word: 0, sampled: false, last: 0, qualified: true,
            attacking: false, attack_at: 0, session: false, until: 0,
            completed: false,
        }
    }

    /// Active cancellation requires voice release before another request.
    /// Idle cancellation does not impose a startup silence requirement.
    pub(crate) const fn cancel(&mut self) {
        let word = self.word;
        let qualified = self.qualified && !self.session;
        *self = Self::new();
        self.word = word;
        self.qualified = qualified;
    }

    pub(crate) const fn requested(&self, now: u32) -> bool {
        self.session && !reached(now, self.until)
    }

    pub(crate) const fn completed(&self) -> bool {
        self.completed
    }

    /// Read-only software admission state, not measured microphone level
    /// or a guarantee that independent RF admission will succeed.
    pub(crate) const fn status(&self, now: u32) -> &'static [u8] {
        if self.completed {
            b"VOX HOLD COMPLETE"
        } else if self.requested(now) {
            b"VOX VOICE / HOLD"
        } else if !self.qualified {
            b"VOX WAIT VOICE RELEASE"
        } else if self.attacking {
            b"VOX ATTACK QUALIFY"
        } else {
            b"VOX ARMED"
        }
    }

    /// Known finite resource handoff only. Pausing cannot renew hold or
    /// carry a previous voice latch into the new detector owner.
    pub(crate) const fn pause(&mut self, now: u32) -> bool {
        self.completed = false;
        self.sampled = false;
        self.attacking = false;
        if self.session && reached(now, self.until) {
            self.cancel();
        }
        self.requested(now)
    }

    pub(crate) const fn observe(
        &mut self,
        now: u32,
        settings: Settings,
        sample: Option<Sample>,
        eligible: bool,
        inhibited: bool,
    ) -> bool {
        self.completed = false;
        if !settings.valid() || settings.hardware_threshold().is_none() {
            self.cancel();
            return false;
        }

        let word = settings.word();
        if word != self.word {
            self.cancel();
            self.word = word;
        }
        if !settings.enabled || !eligible || inhibited {
            self.cancel();
            return false;
        }

        let sample = match sample {
            Some(value) if now.wrapping_sub(value.tick) <= SAMPLE_LIMIT => value,
            _ => {
                self.cancel();
                return false;
            }
        };

        if self.sampled && sample.tick.wrapping_sub(self.last) > SAMPLE_LIMIT {
            self.cancel();
        }
        if self.session && reached(now, self.until) {
            let normal = self.sampled
                && sample.tick != self.last
                && matches!(sample.voice, Some(false));
            self.cancel();
            self.completed = normal;
        }
        if self.sampled && sample.tick == self.last {
            return self.requested(now);
        }

        self.sampled = true;
        self.last = sample.tick;
        let voice = match sample.voice {
            Some(value) => value,
            None => {
                self.attacking = false;
                return self.requested(now);
            }
        };

        if self.session {
            if voice {
                self.until = sample.tick.wrapping_add(settings.hang_ticks());
            }
            return self.requested(now);
        }

        if !voice {
            self.qualified = true;
            self.attacking = false;
            return false;
        }
        if !self.qualified {
            return false;
        }
        if !self.attacking {
            self.attacking = true;
            self.attack_at = sample.tick;
            return false;
        }
        if sample.tick.wrapping_sub(self.attack_at) < ATTACK_TICKS {
            return false;
        }

        self.session = true;
        self.until = sample.tick.wrapping_add(settings.hang_ticks());
        true
    }
}

const _: () = {
    let mut gate = EventGate::new();
    assert!(gate.status(0).len() == 9);
    gate.cancel();
    assert!(!gate.requested(0) && !gate.completed());
};
pub(crate) struct Session {
    word: u32,
    sampled: bool,
    last: u32,
    quiet: bool,
    quiet_at: u32,
    qualified: bool,
    attacking: bool,
    attack_at: u32,
    session: bool,
    until: u32,
}

impl Session {
    pub(crate) const fn new() -> Self {
        Self {
            word: 0, sampled: false, last: 0,
            quiet: false, quiet_at: 0, qualified: false,
            attacking: false, attack_at: 0, session: false, until: 0,
        }
    }

    /// Discard every live observation while retaining only choice identity.
    /// This cannot retain silence qualification after cancellation or timeout.
    pub(crate) const fn cancel(&mut self) {
        let word = self.word;
        *self = Self::new();
        self.word = word;
    }

    pub(crate) const fn qualified(&self) -> bool {
        self.qualified
    }

    /// Read-only projection. A request cannot survive observation loss merely
    /// because the foreground has not called observe again.
    pub(crate) const fn requested(&self, now: u32) -> bool {
        self.session && self.sampled
            && now.wrapping_sub(self.last) <= SAMPLE_LIMIT
            && !reached(now, self.until)
    }

    /// sample contains the timestamp and amplitude of one qualified MIC
    /// observation. None is unavailable input, not measured silence.
    /// eligible excludes unsupported scope; inhibited includes cancellation,
    /// physical PTT, ownership conflicts, and latched RF failures.
    pub(crate) const fn observe(
        &mut self,
        now: u32,
        settings: Settings,
        sample: Option<(u32, u16)>,
        eligible: bool,
        inhibited: bool,
    ) -> bool {
        if !settings.valid() {
            self.cancel();
            return false;
        }

        let word = settings.word();
        if self.word != word {
            self.cancel();
            self.word = word;
        }

        if !settings.enabled || !eligible || inhibited {
            self.cancel();
            return false;
        }

        let (tick, raw) = match sample {
            Some(value) if now.wrapping_sub(value.0) <= SAMPLE_LIMIT => value,
            _ => {
                self.cancel();
                return false;
            }
        };

        if self.session && reached(now, self.until) {
            self.cancel();
        }
        if self.sampled && tick.wrapping_sub(self.last) > SAMPLE_LIMIT {
            self.cancel();
        }
        if self.sampled && tick == self.last {
            return self.requested(now);
        }

        self.sampled = true;
        self.last = tick;

        if !self.qualified {
            if raw < settings.close() {
                if !self.quiet {
                    self.quiet = true;
                    self.quiet_at = tick;
                }
                if tick.wrapping_sub(self.quiet_at) >= QUIET_TICKS {
                    self.qualified = true;
                }
            } else {
                self.quiet = false;
            }
            return false;
        }

        if self.session {
            if raw >= settings.close() {
                self.until = tick.wrapping_add(settings.hang_ticks());
            }
            return self.requested(now);
        }

        if raw < settings.threshold {
            self.attacking = false;
            return false;
        }
        if !self.attacking {
            self.attacking = true;
            self.attack_at = tick;
            return false;
        }
        if tick.wrapping_sub(self.attack_at) < ATTACK_TICKS {
            return false;
        }

        self.session = true;
        self.until = tick.wrapping_add(settings.hang_ticks());
        self.requested(now)
    }
}

const _: () = {
    let defaults = Settings::new();
    assert!(defaults.word() == 0);
    assert!(!defaults.enabled && defaults.threshold == 128 && defaults.hang == 3);
    assert!(defaults.close() == 112 && defaults.hang_ticks() == 3000);
    assert!(Settings::decode(1 << 21).is_none());
    assert!(Settings::decode(u32::MAX).is_none());
    assert!(Settings::decode(DEFAULT_FIELDS).is_none());

    let mut field = 0;
    while field < 3 {
        let mut value = Settings::new();
        let last = Settings::count(field) - 1;
        assert!(value.configure(field, last));
        let loaded = match Settings::decode(value.word()) {
            Some(value) => value,
            None => panic!("VOX setting rejected"),
        };
        assert!(loaded.value(field) == last);
        let before = value.word();
        assert!(!value.configure(field, last + 1));
        assert!(value.word() == before);
        field += 1;
    }

    let mut settings = Settings::new();
    assert!(!settings.configure(3, 0));
    settings.shift_threshold(false, u32::MAX);
    assert!(settings.threshold == 1 && settings.close() == 1);
    settings.shift_threshold(true, u32::MAX);
    assert!(settings.threshold == 2047 && settings.close() == 1792);
    assert!(settings.hardware_threshold().is_some());
    settings.threshold = 2048;
    assert!(settings.hardware_threshold().is_none());
};

const _: () = {
    let mut settings = Settings::new();
    settings.enabled = true;
    let mut session = Session::new();
    let mut tick = 0;
    while tick <= 100 {
        assert!(!session.observe(tick, settings, Some((tick, 0)), true, false));
        tick += 10;
    }
    assert!(session.qualified());
    assert!(!session.observe(110, settings, Some((110, 128)), true, false));
    assert!(session.observe(120, settings, Some((120, 128)), true, false));
    assert!(session.requested(140));
    assert!(!session.requested(141));

    tick = 130;
    while tick < 3120 {
        assert!(session.observe(tick, settings, Some((tick, 0)), true, false));
        tick += 10;
    }
    assert!(!session.observe(3120, settings, Some((3120, 0)), true, false));
    assert!(!session.qualified());
    assert!(!session.observe(3130, settings, Some((3130, 65535)), true, false));
    session.cancel();
    assert!(!session.requested(3130));
};

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(hang: u8) -> Settings {
        Settings { enabled: true, hang, ..Settings::new() }
    }

    fn quiet(session: &mut Session, settings: Settings, start: u32) {
        for delta in (0..=100).step_by(10) {
            let tick = start.wrapping_add(delta);
            assert!(!session.observe(tick, settings, Some((tick, 0)), true, false));
        }
        assert!(session.qualified());
    }

    fn open(session: &mut Session, settings: Settings, tick: u32) {
        assert!(!session.observe(
            tick, settings, Some((tick, settings.threshold)), true, false,
        ));
        let next = tick.wrapping_add(10);
        assert!(session.observe(
            next, settings, Some((next, settings.threshold)), true, false,
        ));
    }

    #[test]
    fn every_threshold_and_hold_roundtrips() {
        for threshold in 1..=65535 {
            let value = Settings {
                threshold, ..Settings::new()
            };
            let loaded = Settings::decode(value.word()).unwrap();
            assert_eq!(loaded.threshold, threshold);
            assert_eq!(loaded.word(), value.word());
            assert!(loaded.close() <= threshold);
            assert!(loaded.close() != 0);
        }
        for hang in 1..=10 {
            for enabled in [false, true] {
                let value = Settings { enabled, hang, ..Settings::new() };
                let loaded = Settings::decode(value.word()).unwrap();
                assert_eq!(loaded.enabled, enabled);
                assert_eq!(loaded.hang, hang);
                assert_eq!(loaded.hang_ticks(), hang as u32 * 1000);
            }
        }
    }

    #[test]
    fn every_word_has_one_checked_encoding() {
        for word in 0..=WORD_MASK {
            if let Some(value) = Settings::decode(word) {
                assert!(value.valid());
                assert_eq!(value.word(), word);
            }
        }
    }

    #[test]
    fn all_hold_choices_expire_at_the_original_deadline() {
        for hang in 1..=10 {
            let settings = settings(hang);
            let mut session = Session::new();
            quiet(&mut session, settings, 0);
            open(&mut session, settings, 110);
            let until = 120 + settings.hang_ticks();
            for tick in (130..until).step_by(10) {
                assert!(session.observe(
                    tick, settings, Some((tick, 0)), true, false,
                ));
            }
            assert!(!session.observe(
                until, settings, Some((until, 0)), true, false,
            ));
            assert!(!session.qualified());
        }
    }

    #[test]
    fn boot_voice_and_short_attack_cannot_open() {
        let settings = settings(1);
        let mut session = Session::new();
        for tick in (0..=500).step_by(10) {
            assert!(!session.observe(
                tick, settings, Some((tick, 65535)), true, false,
            ));
        }
        assert!(!session.qualified());
        quiet(&mut session, settings, 510);
        assert!(!session.observe(
            620, settings, Some((620, settings.threshold)), true, false,
        ));
        assert!(!session.observe(625, settings, Some((625, 0)), true, false));
        open(&mut session, settings, 630);
    }

    #[test]
    fn hysteresis_renews_hold_only_on_accepted_observations() {
        let settings = settings(1);
        let mut session = Session::new();
        quiet(&mut session, settings, 0);
        open(&mut session, settings, 110);
        assert!(session.observe(
            130, settings, Some((130, settings.close())), true, false,
        ));
        assert!(session.observe(
            140, settings, Some((140, settings.close() - 1)), true, false,
        ));
        for tick in (150..1130).step_by(10) {
            assert!(session.observe(tick, settings, Some((tick, 0)), true, false));
        }
        assert!(!session.observe(1130, settings, Some((1130, 0)), true, false));
    }

    #[test]
    fn duplicate_timestamp_cannot_qualify_attack_or_renew_hold() {
        let settings = settings(1);
        let mut session = Session::new();
        for now in 0..=20 {
            assert!(!session.observe(now, settings, Some((0, 0)), true, false));
        }
        assert!(!session.qualified());
        quiet(&mut session, settings, 30);
        assert!(!session.observe(
            140, settings, Some((140, settings.threshold)), true, false,
        ));
        for now in 140..=160 {
            assert!(!session.observe(
                now, settings, Some((140, settings.threshold)), true, false,
            ));
        }
        assert!(session.observe(
            160, settings, Some((160, settings.threshold)), true, false,
        ));
        assert!(session.observe(170, settings, Some((170, 0)), true, false));

        // A changed value at the old timestamp is not a new measurement.
        assert!(session.observe(
            180, settings, Some((170, 65535)), true, false,
        ));
        for tick in (190..1160).step_by(10) {
            assert!(session.observe(tick, settings, Some((tick, 0)), true, false));
        }
        assert!(!session.observe(1160, settings, Some((1160, 0)), true, false));
    }

    #[test]
    fn missing_stale_future_and_discontinuous_input_cancel() {
        let settings = settings(10);
        for kind in 0..4 {
            let mut session = Session::new();
            quiet(&mut session, settings, 0);
            open(&mut session, settings, 110);
            let (now, sample) = match kind {
                0 => (130, None),
                1 => (141, Some((120, 65535))),
                2 => (130, Some((131, 65535))),
                _ => (141, Some((141, 65535))),
            };
            assert!(!session.observe(now, settings, sample, true, false));
            assert!(!session.qualified());
            assert!(!session.requested(now));
            assert!(!session.observe(
                now + 10, settings, Some((now + 10, 65535)), true, false,
            ));
        }
    }

    #[test]
    fn inhibition_scope_and_setting_changes_discard_release_history() {
        let settings = settings(10);
        for kind in 0..4 {
            let mut session = Session::new();
            quiet(&mut session, settings, 0);
            open(&mut session, settings, 110);
            let mut changed = settings;
            if kind == 2 { changed.enabled = false; }
            if kind == 3 { changed.threshold += 1; }
            assert!(!session.observe(
                130, changed, Some((130, 65535)), kind != 0, kind == 1,
            ));
            assert!(!session.qualified());
            assert!(!session.requested(130));
            assert!(!session.observe(
                140, settings, Some((140, 65535)), true, false,
            ));
        }
    }

    #[test]
    fn invalid_settings_fail_closed_without_panicking() {
        for invalid in [
            Settings { threshold: 0, ..settings(1) },
            Settings { hang: 0, ..settings(1) },
            Settings { hang: 11, ..settings(1) },
        ] {
            let mut session = Session::new();
            assert!(!session.observe(
                0, invalid, Some((0, 65535)), true, false,
            ));
        }
    }

    #[test]
    fn deadlines_and_qualification_cross_tick_wrap() {
        let settings = settings(1);
        let start = u32::MAX - 50;
        let mut session = Session::new();
        quiet(&mut session, settings, start);
        open(&mut session, settings, start.wrapping_add(110));
        for delta in (130..1120).step_by(10) {
            let tick = start.wrapping_add(delta);
            assert!(session.observe(tick, settings, Some((tick, 0)), true, false));
        }
        let end = start.wrapping_add(1120);
        assert!(!session.observe(end, settings, Some((end, 0)), true, false));
    }
}