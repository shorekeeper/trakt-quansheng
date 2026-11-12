//! Microphone AGC and TX compression under transmitter ownership.
//!
//! The supplied UV-K5 V3 reference driver selects compression ratio 2:1,
//! zero-point code 86, and noise-point code 64 in REG_29. Their complete word
//! is AB40. REG_31 bit three enables companding. The existing RX expander
//! word has ratio zero and is not changed by this TX owner.
//!
//! MIC AGC is controlled by REG_19 bit 15: clear enables it, set disables it.
//! All other REG_19 bits remain as captured. No undocumented AGC strength,
//! attack, release, analog input gain, or automatic normalization is invented.
//!
//! prepare captures REG_19 and REG_29 before changing them. It runs at the
//! existing muted microphone-configuration position before PA admission.
//! The transmitter sets REG_31 bit three at its ordinary following position.
//! Exact readback is checked by the same pre-PA and running TX checkpoints.
//!
//! Synthetic tail entry clears companding before MIC ADC is excluded and
//! the tone or modem owner starts. AGC and passive compressor coefficients
//! remain owned until complete RF-off cleanup. stop clears companding and
//! restores both captured words; the transmitter separately restores its
//! captured REG_31 word. No enabled profile is retained into RX.
//!
//! This owner performs no frequency, RF-link, PA, event-mask, AF-route,
//! amplifier, or deadline write. CW and automatic acknowledgements do not
//! prepare it. Register retention establishes configuration only, not a
//! measured compression ratio, clipping margin, or RF deviation.

use super::{read, write};
use crate::voice::Settings;

const COMPRESSOR: u16 = (2 << 14) | (86 << 7) | 64;

const fn agc_word(original: u16, enabled: bool) -> u16 {
    if enabled { original & !0x8000 } else { original | 0x8000 }
}

pub(crate) struct Processing {
    active: bool,
    compressed: bool,
    saved: [u16; 2],
    expected: [u16; 2],
}

impl Processing {
    pub(crate) const fn new() -> Self {
        Self {
            active: false, compressed: false, saved: [0; 2], expected: [0; 2],
        }
    }

    /// Caller owns muted TX preparation with no admitted PA.
    /// No other context may change these microphone registers while active.
    pub(crate) fn prepare(&mut self, settings: Settings) -> Result<(), u32> {
        if self.active {
            return Err(0x7D01);
        }

        self.saved = [read(0x19), read(0x29)];
        self.expected = [
            agc_word(self.saved[0], settings.agc),
            if settings.compression { COMPRESSOR } else { self.saved[1] },
        ];
        self.active = true;
        self.compressed = settings.compression;

        write(0x19, self.expected[0]);
        if settings.compression {
            write(0x29, self.expected[1]);
        }
        self.check_words()
    }

    /// The transmitter owns the complete REG_31 word and applies this bit
    /// at its established modulation-configuration position.
    pub(crate) const fn control(&self) -> u16 {
        if self.active && self.compressed { 8 } else { 0 }
    }

    fn check_words(&self) -> Result<(), u32> {
        if self.active {
            if read(0x19) != self.expected[0] {
                return Err(0x7D19);
            }
            if read(0x29) != self.expected[1] {
                return Err(0x7D29);
            }
        }
        Ok(())
    }

    pub(crate) fn check(&self) -> Result<(), u32> {
        self.check_words()?;
        if self.active && read(0x31) & 8 != self.control() {
            return Err(0x7D31);
        }
        Ok(())
    }

    /// Stop microphone companding before synthetic generator ownership.
    /// Clearing only bit three preserves VOX and the event-owner contract.
    pub(crate) fn tail(&mut self) {
        if self.control() != 0 {
            write(0x31, read(0x31) & !8);
        }
        self.compressed = false;
    }

    /// Call after PA and RF links are off. Restore passive coefficients and
    /// AGC choice without restoring any live transmitter or receive resource.
    pub(crate) fn stop(&mut self) {
        if self.active {
            self.tail();
            write(0x19, self.saved[0]);
            write(0x29, self.saved[1]);
        }
        self.active = false;
        self.compressed = false;
    }
}

const _: () = {
    assert!(COMPRESSOR == 0xAB40);
    assert!(agc_word(0x1041, true) == 0x1041);
    assert!(agc_word(0x1041, false) == 0x9041);
    assert!(agc_word(0xFFFF, true) == 0x7FFF);
    assert!(agc_word(0, false) == 0x8000);
    assert!(Processing::new().control() == 0);
};