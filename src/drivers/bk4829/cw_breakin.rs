//! Optional CW receive windows under the admitted transmitter's ownership.
//!
//! This owner does not start or finish a transmission attempt. It temporarily
//! selects RX between marks of one simplex CW session. The transmitter retains
//! policy, calibration, battery checks, input cancellation, and its original
//! foreground and SysTick deadlines. Storage and ordinary receiver pumping
//! remain excluded throughout that session.
//!
//! capture reads the settled CW receiver before it is parked. prepare reads
//! the completed muted TX profile before any mark. Five passive words, the
//! frequency pair, and the exact RF controls distinguish the two destinations.
//! Neither transition replays the receiver presets or the TX setup sequence.
//!
//! A falling mark first uses the transmitter's existing PA-off edge.
//! Tone1 frequency is already zero. AF is muted, and REG_30=0302 keeps the
//! AF DAC and local DSP powered while RF links and the synthesizer are
//! excluded. RX restores its captured words, shifted frequency, one LNA,
//! REG_37=9F1F, and REG_30=BFF1. MIC ADC, TX DSP, PA gain, PA gate, and
//! PA drive stay off.
//!
//! The external amplifier remains powered throughout the keying session.
//! The transmitter enables it during initial local-audio preparation and
//! disables it during complete shutdown. Receive-window transitions never
//! write PA8.
//!
//! REG_48's DAC-gain nibble falls to zero before changing AF or RF destination.
//! Up to four serviced one-tick steps reach zero; the volume and upper fields
//! stay captured and unchanged. The sidetone owner retains mark frequency
//! during this falling fade. At zero output gain, the selected zero-to-four
//! tick release allowance precedes oscillator release and RX programming.
//! The interval starts after the zero-gain write; repeated polls cannot
//! shorten it. LINEAR with two release ticks is the default.
//!
//! LINEAR, SOFT and SMOOTH falling profiles share the same four-step limit
//! and service cadence. Shape adds no new wait. Release consumes part of
//! the receive window but does not postpone PA removal. The choices are
//! frozen for the complete session and cannot modify an active attempt.
//!
//! RX gain rises after its checked AF admission. TX gain rises while the
//! existing relock and AF waits run with Tone1 at zero. A subsequent mark
//! requires the complete gain and TX checks. A preparation request arriving
//! during the falling fade waits for oscillator release before gain rises.
//!
//! These audio ramps consume part of the available receive interval but do
//! not alter the keyer's gap deadline, PA edge sequence, or synthesizer waits.
//! RF remains PA-off during preparation. A short gap may cancel entry to RX
//! while still on prepared TX. Complete shutdown restores the captured gain
//! word after disabling the amplifier.
//!
//! Gain codes do not establish linear acoustic amplitude or AF bias control.
//! The ramps reduce abrupt output-level changes; remaining switching clicks
//! and sidetone quality require radio testing.
//!
//! The selected RX wait begins after the destination writes. Only after its
//! exact readback succeeds is AF_BASEBAND2 selected. The amplifier is already
//! powered; the wait controls receive AF admission, not amplifier startup.
//! Zero means no additional software wait, not instantaneous RF or audio
//! recovery. A following mark may interrupt RX settling and request TX.
//!
//! Electronic keying requests TX preparation before its complete element gap
//! expires when a held contact or retained paddle memory already requires
//! another element. Preparation is not a mark: PA remains off until the
//! keyer's separate mark demand. An unexpected straight-key closure or a
//! paddle assertion after this advance point still needs a normal turnaround.
//!
//! The scheduling allowance starts at twenty observed ticks and increases to
//! the longest observed RX-to-TX turnaround plus two ticks. Measurement spans
//! the source checkpoint, destination writes, relock wait, and the enclosing
//! transmitter's final check. It includes foreground scheduling but not timer
//! periods lost under interrupt masking. It is neither a guaranteed upper
//! bound nor a measured hardware minimum. Each new session resets it.
//!
//! Return to TX selects AF_ALAM once while Tone1 frequency is zero.
//! The RX board path is then disabled. The local-only bridge preserves
//! AF DAC power while frequency and captured TX words are restored.
//! REG_30=C3F2 keeps MIC ADC and PA gain disabled.
//!
//! RF relock retains the reference two-tick wait; local AF retains the
//! ten-tick preparation allowance. Both start after destination programming
//! and run concurrently. Complete destination readback and the transmitter's
//! full TX profile check precede another mark. The amplifier is not restarted.
//! These waits do not establish measured PLL lock or audio bias stability.
//!
//! RX speaker admission follows the selected experimental RX wait rather than
//! the ordinary receiver startup sequence. TX local AF has its separate
//! preparation allowance. Preserving DAC power does not establish AF bias
//! continuity, absence of clicks, receiver recovery, or carrier extinction.
//! RX waits and local sidetone quality require radio testing. No RF ramp or
//! replacement mark-edge sequence is introduced.
//!
//! The receiver object stays suspended. These windows supply speaker audio,
//! not continuous decoder observations; interrupted RX symbols are not joined
//! across the operator's own marks. Tone1 remains silent during each window
//! and retains its session configuration for the existing sidetone owner.
//!
//! All work is foreground-only. One pump performs at most one transition,
//! with a fixed number of register operations and no blocking wait. Deadlines
//! use fresh tick snapshots after register work. A transition failure returns
//! to the transmitter's ordinary complete RF-off cleanup, never a retry.

use super::{read, verify_cw_receive, verify_transmit, write};
use crate::app::time::reached;
use crate::mcu;

const PASSIVE: [u8; 5] = [0x2B, 0x3D, 0x7D, 0x7E, 0x31];
const TX_WAIT: u32 = 2;
const TX_AUDIO_WAIT: u32 = 10;

const BRIDGE: u16 = 0x0302;
const MUTED: u16 = 0x6042;
const RX_AF: u16 = 0x6542;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Tx,
    RxFade,
    RxSettle,
    Rx,
    TxFade,
    TxSettle,
}

pub(super) struct BreakIn {
    active: bool,
    prepared: bool,
    phase: Phase,
    deadline: u32,
    rx_wait: u32,
    advance: u32,
    tx_started: u32,
    audio_word: u16,
    audio_level: u8,
    audio_target: u8,
    audio_next: u32,
    audio_options: crate::cw_tail::Settings,
    audio_from: u8,
    audio_step: u8,
    tone_word: u16,
    rx_frequency: u32,
    tx_frequency: u32,
    rx_words: [u16; 5],
    tx_words: [u16; 5],
    rx_controls: [u16; 5],
    tx_controls: [u16; 5],
}

impl BreakIn {
    pub(super) const fn new() -> Self {
        Self {
            active: false, prepared: false, phase: Phase::Tx,
            deadline: 0, rx_wait: 0, advance: 20, tx_started: 0,
            audio_word: 0, audio_level: 0, audio_target: 0, audio_next: 0,
            audio_options: crate::cw_tail::Settings::new(),
            audio_from: 0, audio_step: 0, tone_word: 0,
            rx_frequency: 0, tx_frequency: 0,
            rx_words: [0; 5], tx_words: [0; 5],
            rx_controls: [0; 5], tx_controls: [0; 5],
        }
    }

    pub(super) const fn active(&self) -> bool {
        self.active
    }

    pub(super) const fn tx_ready(&self) -> bool {
        self.active && self.prepared && matches!(self.phase, Phase::Tx)
    }

    pub(super) const fn in_gap(&self) -> bool {
        self.active && self.prepared && !matches!(self.phase, Phase::Tx)
    }

    pub(super) const fn advance(&self) -> u32 {
        self.advance
    }

    /// Called after the enclosing transmitter's complete TX check succeeds.
    /// Keep the longest observation within this session; no observation
    /// reduces the initial allowance or modifies a hardware settling wait.
    pub(super) fn note_tx_ready(&mut self, now: u32) {
        assert!(self.tx_ready());
        let observed = now.wrapping_sub(self.tx_started).saturating_add(2);
        self.advance = self.advance.max(observed);
    }

    /// Capture an exact settled CW profile, in ten-hertz synthesizer units.
    /// The enclosing admission check already established simplex, pitch,
    /// AUTO gain, a fresh carrier observation, and matching selected RX.
    /// This readback separately rejects an unexpected hardware destination.
    pub(super) fn capture(
        &mut self,
        nominal: u32,
        shifted: u32,
        rx_wait: u32,
        tail_word: u8,
    ) -> Result<(), u32> {
        let audio_options = crate::cw_tail::Settings::decode(tail_word as u32)
            .ok_or(0x7E08u32)?;
        if self.active || nominal <= shifted || nominal - shifted > 100
            || rx_wait > 50
        {
            return Err(0x7E01);
        }

        let gpio = if nominal < 28_000_000 { 0x9044 } else { 0x9048 };
        let af = read(0x47);
        if !matches!(af, MUTED | RX_AF) {
            return Err(0x7E47);
        }
        let controls = [0xBFF1, gpio, 0, 0x9F1F, af];
        let _ = verify_cw_receive(controls)?;
        if Self::frequency() != shifted {
            return Err(0x7E38);
        }

        let words = Self::save();
        let audio_word = read(0x48);
        if audio_word & 0xF000 != 0xB000 {
            return Err(0x7E48);
        }
        if words[0] != 0x0500 || words[1] != 0
            || words[3] != 0x302E || words[4] & 15 != 0
        {
            return Err(0x7E02);
        }

        self.rx_frequency = shifted;
        self.tx_frequency = nominal;
        self.rx_wait = rx_wait;
        self.advance = 20;
        self.tx_started = 0;
        self.audio_options = audio_options;
        self.audio_word = audio_word;
        self.audio_level = (audio_word & 15) as u8;
        self.audio_target = self.audio_level;
        self.audio_from = self.audio_level;
        self.audio_step = 0;
        self.audio_next = 0;
        self.tone_word = 0;
        self.rx_words = words;
        self.rx_controls = [0xBFF1, gpio, 0, 0x9F1F, MUTED];
        self.phase = Phase::Tx;
        self.active = true;
        self.prepared = false;
        Ok(())
    }

    /// Capture TX passive words only after the normal full setup succeeds.
    /// The supplied tuple is the complete unkeyed CW sidetone state.
    pub(super) fn prepare(&mut self, controls: [u16; 5]) -> Result<(), u32> {
        if !self.active || self.prepared
            || controls != [0xC3F2, 0x9000, 0, 0x9D1F, 0x6242]
        {
            return Err(0x7E03);
        }
        let _ = verify_transmit(controls)?;
        if Self::frequency() != self.tx_frequency {
            return Err(0x7E38);
        }
        if read(0x48) != self.audio_word {
            return Err(0x7E48);
        }
        self.tx_words = Self::save();
        self.tx_controls = controls;
        self.prepared = true;
        Ok(())
    }

    /// Complete session cleanup restores the captured output word after the
    /// enclosing transmitter disables PA, RF links, and the amplifier.
    /// Ordinary transitions never restore another owner's enabled state.
    pub(super) fn clear(&mut self) {
        if self.active {
            write(0x48, self.audio_word);
        }
        self.active = false;
        self.prepared = false;
        self.phase = Phase::Tx;
        self.audio_level = (self.audio_word & 15) as u8;
        self.audio_target = self.audio_level;
    }

    pub(super) const fn audio_muted(&self) -> bool {
        self.audio_level == 0
    }

    /// Zero gain must persist through the local settling allowance before
    /// REG_71 is changed. audio_next holds either the next gain-step deadline
    /// or, at zero gain, this oscillator-release deadline.
    ///
    /// The allowance begins after the gain write, using a fresh snapshot.
    /// It is an experimental output-settling interval, not a hardware mute
    /// specification. PA has already been disabled by the ordinary RF edge.
    pub(super) const fn audio_release_ready(&self, now: u32) -> bool {
        self.audio_muted() && reached(now, self.audio_next)
    }

    /// The sidetone owner supplies its expected oscillator word after the
    /// falling RF edge. Nonzero frequency is allowed only before RX entry,
    /// while the output fade still belongs to prepared TX.
    ///
    /// This method performs no register access. Complete readback compares
    /// against the supplied owned state, never against a captured bus value.
    pub(super) fn space_tone(&mut self, word: u16) {
        assert!(word == 0 || matches!(self.phase, Phase::Tx | Phase::RxFade));
        self.tone_word = word;
    }

    const fn full_gain(&self) -> u8 {
        (self.audio_word & 15) as u8
    }

    const fn gain_ready(&self) -> bool {
        self.audio_level == self.audio_target
    }

    fn fade(&mut self, up: bool) {
        self.audio_from = self.audio_level;
        self.audio_step = 0;
        self.audio_target = if up { self.full_gain() } else { 0 };
        let wait = if !up && self.audio_muted() {
            self.audio_options.release as u32
        } else {
            1
        };
        self.audio_next = mcu::tick::snapshot().wrapping_add(wait);
    }

    /// Change only REG_48's DAC-gain nibble. The captured volume and all
    /// upper fields remain intact. At most one step runs per foreground
    /// call; a delayed call cannot compress the missing steps.
    ///
    /// Rising gain retains equal code steps. Falling gain uses the selected
    /// profile from its entry level and reaches zero within four steps.
    /// All profiles share the existing one-tick service cadence; a repeated
    /// timestamp cannot advance them. AF selection changes only after output
    /// reaches zero and the sidetone owner completes oscillator release.
    /// Hardware gain codes are not calibrated acoustic amplitude.
    pub(super) fn service_audio(&mut self, now: u32) {
        if self.gain_ready() || !reached(now, self.audio_next) {
            return;
        }
        let step = ((self.full_gain() + 3) / 4).max(1);
        self.audio_level = if self.audio_level < self.audio_target {
            self.audio_level.saturating_add(step).min(self.audio_target)
        } else {
            self.audio_step = self.audio_step.saturating_add(1).min(4);
            crate::cw_tail::falling(
                self.full_gain(), self.audio_from,
                self.audio_step, self.audio_options.shape,
            )
        };
        write(
            0x48,
            (self.audio_word & !15) | self.audio_level as u16,
        );
        let wait = if self.audio_muted() {
            self.audio_options.release as u32
        } else {
            1
        };
        self.audio_next = mcu::tick::snapshot().wrapping_add(wait);
    }

    fn save() -> [u16; 5] {
        let mut words = [0; 5];
        for index in 0..words.len() {
            words[index] = read(PASSIVE[index]);
        }
        words
    }

    fn restore(words: [u16; 5]) {
        for index in 0..words.len() {
            write(PASSIVE[index], words[index]);
        }
    }

    fn frequency() -> u32 {
        read(0x38) as u32 | ((read(0x39) as u32) << 16)
    }

    fn set_frequency(value: u32) {
        write(0x38, value as u16);
        write(0x39, (value >> 16) as u16);
    }


    /// Check a complete destination, never an intermediate bridge state.
    /// VCO calibration bit 15 is excluded by the control readback helper.
    #[inline(never)]
    pub(super) fn check(&self, expected: [u16; 5]) -> Result<(), u32> {
        if !self.active || !self.prepared {
            return Err(0x7E04);
        }

        let receiving = matches!(
            self.phase, Phase::RxSettle | Phase::Rx | Phase::TxFade,
        );
        if receiving {
            if expected != self.rx_controls {
                return Err(0x7E05);
            }
            let _ = verify_cw_receive(self.rx_controls)?;
        } else {
            if expected != self.tx_controls {
                return Err(0x7E06);
            }
            let _ = verify_transmit(expected)?;
        }

        let frequency = if receiving { self.rx_frequency } else { self.tx_frequency };
        if Self::frequency() != frequency {
            return Err(0x7E38);
        }

        let words = if receiving { self.rx_words } else { self.tx_words };
        for index in 0..words.len() {
            if read(PASSIVE[index]) != words[index] {
                return Err(0x7E00 + PASSIVE[index] as u32);
            }
        }
        for register in [0x51u8, 0x58, 0x59, 0x3F] {
            if read(register) != 0 {
                return Err(0x7E00 + register as u32);
            }
        }
        if read(0x71) != self.tone_word {
            return Err(0x7E71);
        }
        if read(0x48) != (self.audio_word & !15) | self.audio_level as u16 {
            return Err(0x7E48);
        }
        Ok(())
    }

    fn start_rx(&mut self) -> Result<[u16; 5], u32> {
        if self.audio_level != 0 {
            return Err(0x7E48);
        }
        if self.tone_word != 0 {
            return Err(0x7E71);
        }
        self.rx_controls[4] = MUTED;
        write(0x47, MUTED);
        write(0x30, BRIDGE);
        write(0x37, 0x9F1F);
        Self::set_frequency(self.rx_frequency);
        Self::restore(self.rx_words);
        write(0x33, self.rx_controls[1]);
        write(0x30, self.rx_controls[0]);

        self.phase = Phase::RxSettle;
        self.deadline = mcu::tick::snapshot().wrapping_add(self.rx_wait);
        // PA and amplifier are off. The complete destination checkpoint
        // runs before audio admission or before another transition.
        Ok(self.rx_controls)
    }

    fn start_tx(&mut self) -> Result<[u16; 5], u32> {
        if self.audio_level != 0 {
            return Err(0x7E48);
        }
        // Tone1 frequency is already zero and PA is off. Select the final
        // local route once without cycling the session's amplifier or
        // moving AF through Mute and then back to AF_ALAM.
        write(0x47, self.tx_controls[4]);
        write(0x33, 0x9000);
        write(0x30, BRIDGE);
        Self::set_frequency(self.tx_frequency);
        write(0x37, self.tx_controls[3]);
        Self::restore(self.tx_words);
        write(0x30, self.tx_controls[0]);

        self.phase = Phase::TxSettle;
        // Start after all destination writes. RF relock and local AF
        // settling overlap; neither deadline is consumed by those writes.
        // Electronic keying prepares this destination before its gap ends.
        self.deadline = mcu::tick::snapshot().wrapping_add(
            TX_WAIT.max(TX_AUDIO_WAIT),
        );
        // Tone1 remains at zero frequency during preparation. Restore gain
        // concurrently with the unchanged RF and local-AF settling waits.
        self.fade(true);
        // No PA admission here. Complete destination and transmitter
        // profile checks precede another mark; amplifier power is retained.
        Ok(self.tx_controls)
    }

    /// prepare requests a TX destination without permission to enable PA.
    /// The caller applies mark edges independently through cw_key. Audio
    /// gain falls before destination programming and rises before TX
    /// readiness. Prepared spaces stay in TX while preparation is requested,
    /// preventing repeated RX/TX cycling before an element starts.
    pub(super) fn pump(
        &mut self,
        now: u32,
        prepare: bool,
        expected: [u16; 5],
    ) -> Result<[u16; 5], u32> {
        if !self.active || !self.prepared {
            return Err(0x7E04);
        }

        self.service_audio(now);

        match self.phase {
            Phase::Tx => {
                if prepare {
                    return Ok(expected);
                }
                if expected != self.tx_controls {
                    return Err(0x7E06);
                }
                self.check(expected)?;
                self.fade(false);
                self.phase = Phase::RxFade;
                Ok(expected)
            }
            Phase::RxFade => {
                if prepare && self.gain_ready() && self.tone_word == 0 {
                    // RF has not left prepared TX. Finish the falling
                    // fade and oscillator release before raising gain,
                    // without an unnecessary synthesizer restart.
                    self.tx_started = mcu::tick::snapshot();
                    self.check(expected)?;
                    self.fade(true);
                    self.phase = Phase::TxSettle;
                    self.deadline = mcu::tick::snapshot();
                    return Ok(expected);
                }
                if self.gain_ready() && self.tone_word == 0 {
                    self.check(expected)?;
                    return self.start_rx();
                }
                Ok(expected)
            }
            Phase::RxSettle | Phase::Rx => {
                if prepare {
                    // Include source verification and fade in the observed
                    // preparation allowance used by the electronic keyer.
                    self.tx_started = mcu::tick::snapshot();
                    self.check(expected)?;
                    self.fade(false);
                    self.phase = Phase::TxFade;
                    return Ok(expected);
                }
                if matches!(self.phase, Phase::RxSettle)
                    && reached(now, self.deadline)
                {
                    self.check(expected)?;
                    write(0x47, RX_AF);
                    self.rx_controls[4] = RX_AF;
                    self.phase = Phase::Rx;
                    let _ = verify_cw_receive(self.rx_controls)?;
                    self.fade(true);
                    return Ok(self.rx_controls);
                }
                Ok(expected)
            }
            Phase::TxFade => {
                if self.gain_ready() {
                    self.check(expected)?;
                    return self.start_tx();
                }
                Ok(expected)
            }
            Phase::TxSettle => {
                if reached(now, self.deadline) && self.gain_ready() {
                    self.check(expected)?;
                    self.phase = Phase::Tx;
                }
                Ok(expected)
            }
        }
    }
}