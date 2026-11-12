//! Saved microphone processing choices for voice transmission.
//!
//! Compression selects the BK4829 reference TX compressor profile, nominally
//! 2:1. MIC AGC selects REG_19 bit 15 clear for enabled or set for disabled.
//! These are hardware operating selections, not measured acoustic gain,
//! modulation deviation, attack time, or an input-voltage clipping limit.
//!
//! Defaults retain compression Off and MIC AGC On. The current board receive
//! preset already has MIC AGC enabled. Explicit On preserves every other
//! REG_19 field rather than installing another microphone profile.
//!
//! The two-bit word stores compression in bit zero and AGC disable in bit
//! one. Zero retains defaults. Snapshot sound word six stores this word in
//! bits 24..25. Older record formats reserve that extension and load defaults
//! without a migration write. Bits 26..30 belong to the separately checked
//! CW local-audio choice word; bit 31 remains reserved. This owner encodes
//! and decodes only its own two bits.
//!
//! Choices are global across firmware compositions. Only the concrete voice
//! transmitter applies them. RX, CW, local tones, and automatic FSK packets
//! neither apply the profile nor obtain a transmit request from these choices.
//! The existing manual FSK voice interval may use microphone processing;
//! its synthetic packet tail disables compression before generator ownership.
//!
//! Compression and AGC cannot repair analog input clipping or protect an
//! external source from excessive voltage. There is no software PCM buffer,
//! independent automatic-level loop, or calibrated loudness target.

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub(crate) compression: bool,
    pub(crate) agc: bool,
}

impl Settings {
    pub(crate) const fn new() -> Self {
        Self { compression: false, agc: true }
    }

    pub(crate) const fn word(self) -> u32 {
        self.compression as u32 | (((!self.agc) as u32) << 1)
    }

    pub(crate) const fn decode(word: u32) -> Option<Self> {
        if word & !3 != 0 {
            None
        } else {
            Some(Self {
                compression: word & 1 != 0,
                agc: word & 2 == 0,
            })
        }
    }
}

const _: () = {
    let defaults = Settings::new();
    assert!(defaults.word() == 0);
    assert!(!defaults.compression && defaults.agc);

    let mut word = 0;
    while word < 4 {
        let restored = match Settings::decode(word) {
            Some(value) => value,
            None => panic!("voice processing choices rejected"),
        };
        assert!(restored.word() == word);
        word += 1;
    }
    assert!(Settings::decode(4).is_none());
    assert!(Settings::decode(u32::MAX).is_none());
};