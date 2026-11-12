//! Local CW sidetone under the admitted transmitter's exclusive ownership.
//!
//! The BK4829 reference CW keyer uses AF_ALAM, selector two, while REG_50
//! retains the TX modulation-mute command. AF DAC enable is REG_30 bit nine.
//! Tone1 remains enabled across a session; REG_71 changes between the
//! selected pitch and zero at mark/space edges. Rewriting REG_70 at every
//! edge is avoided, following the reference's audio-pop handling.
//!
//! Full break-in removes RF before fading the local output. During that
//! falling output-gain ramp this owner retains the mark frequency instead
//! of abruptly setting REG_71 to zero at full output gain. The transmitter
//! completes oscillator release at zero output gain before entering RX.
//! Semi-break-in and practice retain their ordinary immediate frequency
//! updates. The brief fading sidetone tail is not additional RF emission.
//!
//! This owner changes only REG_70 and REG_71. Transmitter owns AF selection,
//! AF DAC enable, external amplifier timing, PA controls, and cleanup order.
//! MIC ADC remains disabled. No RX, PA, frequency, power, or TX-unmute write
//! is issued here. The shared tone conversion uses integer hertz.
//!
//! Tone gain code 28 matches the established local-tone default. The receive
//! calibration's output gain and physical volume control remain unchanged.
//! The gain is not a calibrated sound-pressure or RF-deviation value.
//! Sidetone uses the selected RX pitch and is enabled during CW TX.
//!
//! Start captures REG_71 only after the caller checks parked RF.
//! The generator must initially be disabled. Start produces silence.
//! Transmitter enables the local DAC and speaker, then overlaps their
//! settling deadline with RF preparation without restarting the audio path.
//! Both audio settling and RF checks precede cw_ready. Practice uses the
//! same local preparation but never progresses into an RF setup sequence.
//!
//! Cleanup runs after PA, RF links, and the external amplifier are off.
//! It disables Tone1 and restores the captured oscillator word. It does not
//! restore an enabled generator, AF route, amplifier, or TX permission.
//! REG_50 readback has no assumed mirror contract. The enclosing owner
//! continues issuing its established BB18 mute command and checks MIC ADC,
//! FSK, subaudio, the exact link tuple, and these generator registers.
//!
//! Register retention is not a measurement of local audio or RF emissions.
//! Audible sidetone and absence of transmitted tone require separate tests.

use super::{read, write};

pub(super) const CONTROL: u16 = 0x8000 | (28 << 8);

pub(super) struct Sidetone {
    active: bool,
    down: bool,
    release_pending: bool,
    pitch_word: u16,
    original_word: u16,
    control: u16,
}

impl Sidetone {
    pub(super) const fn new() -> Self {
        Self {
            active: false, down: false, release_pending: false,
            pitch_word: 0, original_word: 0,
            control: 0,
        }
    }

    pub(super) const fn active(&self) -> bool {
        self.active
    }

    pub(super) const fn control(&self) -> u16 {
        if self.active { self.control } else { 0 }
    }

    /// True while the oscillator carries the mark frequency. During a
    /// full-break-in output fade this can outlast the already removed RF
    /// mark. Radiated mark observations belong to the transmitter controls.
    pub(super) const fn down(&self) -> bool {
        self.active && self.down
    }

    pub(super) const fn release_pending(&self) -> bool {
        self.active && self.release_pending
    }

    /// Expected oscillator word from owned state, not a register observation.
    pub(super) const fn expected_word(&self) -> u16 {
        if self.active && self.down { self.pitch_word } else { 0 }
    }

    /// Retain oscillator frequency while the full-break-in output gain
    /// falls. This grants no RF permission and performs no register write.
    /// The transmitter calls key(false) once the output gain reaches zero,
    /// before selecting RX. Cancellation disables the generator directly.
    pub(super) fn defer_release(&mut self) {
        self.release_pending = self.active && self.down;
    }

    pub(super) fn start(&mut self, pitch: u16, volume: u8) -> Result<(), u32> {
        if self.active || volume > 127
            || !crate::cw::valid_pitch(pitch) || read(0x70) != 0
        {
            return Err(0x7A06);
        }

        self.original_word = read(0x71);
        self.pitch_word = super::sound::frequency_word(pitch);
        self.down = false;
        self.release_pending = false;
        self.active = true;
        self.control = if volume == 0 { 0 } else { 0x8000 | ((volume as u16) << 8) };

        write(0x71, 0);
        write(0x70, self.control);
        self.check()
    }

    /// Called after the transmitter finishes the corresponding RF edge.
    /// Keeping the generator enabled avoids restarting it for every element.
    pub(super) fn key(&mut self, down: bool) {
        self.release_pending = false;
        if self.active && self.down != down {
            write(0x71, if down { self.pitch_word } else { 0 });
            self.down = down;
        }
    }

    pub(super) fn check(&self) -> Result<(), u32> {
        if self.active
            && (read(0x70) != self.control
                || read(0x71) != if self.down { self.pitch_word } else { 0 })
        {
            return Err(0x7A06);
        }
        Ok(())
    }

    pub(super) fn cancel(&mut self) {
        if self.active {
            write(0x70, 0);
            write(0x71, self.original_word);
            self.active = false;
            self.down = false;
            self.release_pending = false;
        }
    }
}

const _: () = {
    assert!(CONTROL == 0x9C00);
    assert!(CONTROL & 0x0080 == 0);
    assert!(0xC3F2u16 ^ 0xC1F2 == 1 << 9);
    assert!(0xC3FAu16 ^ 0xC1FA == 1 << 9);
    assert!(0xC3FAu16 & 4 == 0);
    assert!(0xC3F2u16 & 12 == 0);
};