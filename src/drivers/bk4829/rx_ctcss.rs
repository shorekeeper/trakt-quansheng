//! Foreground receive-signaling events and a separate audio gate.
//!
//! Configuration runs with RF links stopped and the amplifier muted.
//! CTCSS retains REG_51/REG_07 programming and REG_52=028F.
//! DCS reuses the checked word programmer with the requested N/I word.
//! PA, MIC ADC, and TX links remain excluded by receiver control checks.
//!
//! CTCSS enables REG_02 events 0040/0080. DCS enables 0100/0200.
//! Polling acknowledges at most one event set: read REG_0C, write zero to
//! REG_02, then read REG_02. DCS event classification is read afterward
//! from REG_0C bits 15..14. No MCU interrupt or unbounded drain loop exists.
//!
//! CTCSS 0040 admits and 0080 closes. DCS 0100 admits only with classifier
//! one; 0200 closes. Classifier two denotes the programmed word's complement,
//! not an alternative accepted polarity. Simultaneous open/close closes.
//! These actions follow reference application use, not compatibility IRQ names.
//!
//! Gate tracks signaling independently of carrier SQL. Carrier closure
//! clears admission. Initial recognition may precede carrier opening.
//! A sample gap over 100 ticks requires retuning and fresh acquisition.
//! Monitor bypasses signaling; SQL zero does not. PTT/Spectrum suppress audio.
//!
//! Diagnostic words retain current REG_0C, last nonzero REG_02, accumulated
//! event bits, and either current REG_68 for CTCSS or the classifier from the
//! last DCS event. History resets on park or tune. These observations are
//! not measured tone frequency, decoder confidence, or a TX authorization.
//! No signaling search or reverse-burst generation is performed.

use super::{read, subtone, write};

const FOUND: u16 = 1 << 6;
const LOST: u16 = 1 << 7;
const MASK: u16 = FOUND | LOST;
const DCS_MATCH: u16 = 1 << 8;
const DCS_CLOSE: u16 = 1 << 9;
const DCS_MASK: u16 = DCS_MATCH | DCS_CLOSE;

const fn digital(value: u16) -> bool {
    value & 0x8000 != 0
}

const fn event_mask(value: u16) -> u16 {
    if value == 0 { 0 } else if digital(value) { DCS_MASK } else { MASK }
}

/// Convert hardware events into the existing gate actions. DCS type one
/// means the programmed word matched, including a programmed inverse word.
/// Type two is its complement and cannot open audio. Unknown classification
/// or simultaneous open/close events revoke admission.
/// CTCSS retains the independently verified bit-six/bit-seven behavior.
const fn admission_events(value: u16, status: u16, flags: u16) -> u16 {
    if value == 0 {
        0
    } else if !digital(value) {
        flags & MASK
    } else if flags & DCS_CLOSE != 0 {
        LOST
    } else if flags & DCS_MATCH != 0 {
        if (status >> 14) & 3 == 1 { FOUND } else { LOST }
    } else {
        0
    }
}

pub(super) fn configure(value: u16) -> Result<(), u32> {
    if !crate::tx_signal::valid(value as u32) {
        return Err(0x5104);
    }

    write(0x3F, 0);
    write(0x02, 0);
    let _ = read(0x02);

    if digital(value) {
        // The shared programmer writes the checked DCS word and clock.
        // The receiver owns parked RF here; PA and TX links stay disabled.
        subtone::configure_tx(value)?;
    } else {
        subtone::configure(value)?;
        if value != 0 {
            write(0x52, 0x028F);
        }
    }

    if value != 0 {
        write(0x3F, event_mask(value));
    }
    Ok(())
}

pub(super) fn check(value: u16) -> Result<(), u32> {
    if read(0x51) != subtone::tx_control(value) {
        return Err(0x5151);
    }
    if read(0x3F) != event_mask(value) {
        return Err(0x513F);
    }
    if value != 0 && !digital(value) && read(0x52) != 0x028F {
        return Err(0x5152);
    }
    Ok(())
}

fn events() -> (u16, u16) {
    let status = read(0x0C);
    if status & 1 == 0 {
        return (status, 0);
    }
    write(0x02, 0);
    let flags = read(0x02);

    // The reference application reads DCS classification after consuming
    // the event. Do not substitute a pre-acknowledgement classification.
    let status = if flags & DCS_MASK != 0 { read(0x0C) } else { status };
    (status, flags)
}

/// Retain sparse events across ordinary samples with no pending request.
/// REG_68 is observational only and does not contribute to admission.
const fn debug_sample(
    previous: [u16; 4],
    status: u16,
    flags: u16,
    scan: u16,
) -> [u16; 4] {
    [
        status,
        if flags != 0 { flags } else { previous[1] },
        previous[2] | flags,
        scan,
    ]
}

pub(super) struct Gate {
    present: bool,
    sampled: bool,
    last_sample: u32,
    #[cfg(feature = "debug-tools")]
    raw: [u16; 4],
}

impl Gate {
    pub(super) const fn new() -> Self {
        Self {
            present: false,
            sampled: false,
            last_sample: 0,
            #[cfg(feature = "debug-tools")]
            raw: [0; 4],
        }
    }

    /// Called at the receiver's tone sample point. Acknowledge and consume
    /// at most one event set, then update the tone gate. Additional REG_68
    /// reads exist only with debug-tools and never authorize audio.
    pub(super) fn poll(&mut self, now: u32, value: u16) {
        let (status, flags) = events();
        self.observe(now, admission_events(value, status, flags));

        #[cfg(feature = "debug-tools")]
        {
            let detail = if digital(value) {
                if flags & DCS_MASK != 0 {
                    (status >> 14) & 3
                } else {
                    self.raw[3]
                }
            } else {
                read(0x68)
            };
            self.raw = debug_sample(self.raw, status, flags, detail);
        }
    }

    /// The view has a fixed shape for screen type checking. A build without
    /// diagnostics stores no raw history and returns an empty observation.
    pub(super) const fn diagnostic(&self) -> [u16; 4] {
        #[cfg(feature = "debug-tools")]
        {
            self.raw
        }

        #[cfg(not(feature = "debug-tools"))]
        {
            [0; 4]
        }
    }

    pub(super) const fn stale(&self, now: u32) -> bool {
        self.sampled && now.wrapping_sub(self.last_sample) > 100
    }

    pub(super) const fn observe(&mut self, now: u32, events: u16) {
        self.sampled = true;
        self.last_sample = now;
        if events & LOST != 0 {
            self.present = false;
        } else if events & FOUND != 0 {
            self.present = true;
        }
    }

    pub(super) const fn carrier_closed(&mut self) {
        self.present = false;
    }

    pub(super) const fn audio(
        &self,
        carrier_audio: bool,
        required: bool,
        monitor: bool,
        muted: bool,
    ) -> bool {
        carrier_audio && !muted && (monitor || !required || self.present)
    }
}

const _: () = {
    assert!(event_mask(0) == 0);
    assert!(event_mask(885) == 0x00C0);
    assert!(event_mask(0x8013) == 0x0300);
    assert!(event_mask(0xC013) == 0x0300);

    assert!(admission_events(0, 0x4000, 0x0100) == 0);
    assert!(admission_events(885, 0, 0x0040) == 0x0040);
    assert!(admission_events(885, 0, 0x0080) == 0x0080);
    assert!(admission_events(885, 0x4000, 0x0100) == 0);

    let selections = [0x8013, 0xC013];
    let mut index = 0;
    while index < selections.len() {
        let value = selections[index];
        assert!(admission_events(value, 0x4000, 0x0100) == 0x0040);
        assert!(admission_events(value, 0x8000, 0x0100) == 0x0080);
        assert!(admission_events(value, 0, 0x0100) == 0x0080);
        assert!(admission_events(value, 0xC000, 0x0100) == 0x0080);
        assert!(admission_events(value, 0x4000, 0x0200) == 0x0080);
        assert!(admission_events(value, 0x4000, 0x0300) == 0x0080);
        assert!(admission_events(value, 0x4000, 0x0040) == 0);

        let mut gate = Gate::new();
        gate.observe(0, admission_events(value, 0x4000, 0x0100));
        assert!(gate.audio(true, true, false, false));
        assert!(!gate.audio(false, true, false, false));
        assert!(!gate.audio(true, true, false, true));
        gate.observe(20, admission_events(value, 0x8000, 0x0100));
        assert!(!gate.audio(true, true, false, false));
        assert!(gate.audio(true, true, true, false));

        gate.observe(40, admission_events(value, 0x4000, 0x0100));
        gate.carrier_closed();
        assert!(!gate.audio(true, true, false, false));
        gate.observe(60, 0);
        assert!(!gate.audio(true, true, false, false));
        index += 1;
    }
};

const _: () = {
    // Literal event values keep these checks independent of symbolic
    // names that could otherwise make an inverted mapping self-consistent.
    assert!(MASK == 0x00C0);
    let mut raw = Gate::new();
    assert!(!raw.audio(true, true, false, false));

    raw.observe(0, 0x0040);
    assert!(raw.audio(true, true, false, false));
    assert!(!raw.audio(false, true, false, false));
    assert!(!raw.audio(true, true, false, true));

    raw.observe(20, 0);
    assert!(raw.audio(true, true, false, false));

    raw.observe(40, 0x0080);
    assert!(!raw.audio(true, true, false, false));
    assert!(raw.audio(true, true, true, false));

    raw.observe(60, 0x0040);
    assert!(raw.audio(true, true, false, false));
    raw.observe(80, 0x00C0);
    assert!(!raw.audio(true, true, false, false));

    raw.observe(100, 0x0040);
    raw.carrier_closed();
    assert!(!raw.audio(true, true, false, false));
    raw.observe(120, 0);
    assert!(!raw.audio(true, true, false, false));
    raw.observe(140, 0x0040);
    assert!(raw.audio(true, true, false, false));
};

const _: () = {
    let first = debug_sample([0; 4], 0x0401, FOUND, 0x8123);
    assert!(first[0] == 0x0401);
    assert!(first[1] == FOUND && first[2] == FOUND);
    assert!(first[3] == 0x8123);

    let idle = debug_sample(first, 0x0400, 0, 0x8456);
    assert!(idle[0] == 0x0400 && idle[3] == 0x8456);
    assert!(idle[1] == FOUND && idle[2] == FOUND);

    let lost = debug_sample(idle, 1, LOST, 0xFFFF);
    assert!(lost[1] == LOST && lost[2] == MASK);

    let mut unrelated = Gate::new();
    unrelated.observe(0, 1 << 9);
    assert!(!unrelated.audio(true, true, false, false));

    unrelated.observe(20, FOUND | (1 << 9));
    assert!(unrelated.audio(true, true, false, false));

    unrelated.observe(40, LOST | (1 << 9));
    assert!(!unrelated.audio(true, true, false, false));
};

const _: () = {
    let mut gate = Gate::new();
    assert!(!gate.audio(true, true, false, false));
    assert!(gate.audio(true, false, false, false));
    assert!(gate.audio(true, true, true, false));
    assert!(!gate.audio(true, true, true, true));
    assert!(!gate.audio(false, false, true, false));

    // FOUND may precede the ordinary SQL gate opening.
    gate.observe(0, FOUND);
    assert!(!gate.audio(false, true, false, false));
    gate.observe(60, 0);
    assert!(gate.audio(true, true, false, false));
    assert!(!gate.stale(160));
    assert!(gate.stale(161));

    gate.observe(80, LOST);
    assert!(!gate.audio(true, true, false, false));
    gate.observe(100, FOUND);
    assert!(gate.audio(true, true, false, false));
    gate.observe(120, FOUND | LOST);
    assert!(!gate.audio(true, true, false, false));

    gate.observe(140, FOUND);
    gate.carrier_closed();
    assert!(!gate.audio(true, true, false, false));
    gate.observe(160, 0);
    assert!(!gate.audio(true, true, false, false));
    gate.observe(180, FOUND);
    assert!(gate.audio(true, true, false, false));

    let mut wrapped = Gate::new();
    wrapped.observe(u32::MAX - 50, FOUND);
    assert!(!wrapped.stale(49));
    assert!(wrapped.stale(50));
};