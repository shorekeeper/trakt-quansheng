//! Transmit-only CTCSS programming under the transmitter's RF ownership.
//!
//! The admitted values use the BK4829 reference port's CTC1 conversion.
//! Input is tenths of a hertz; the rounded word is
//! (value * 206488 + 50000) / 100000 and fits REG_07's thirteen-bit field.
//! REG_51 is 9040 for CTCSS: enable, analog mode, and gain code 64.
//! The gain code is not a measured deviation or a microphone gain setting.
//!
//! Configure runs while PA is disabled, at the existing subaudio setup
//! position. Off writes only REG_51 zero. REG_07 is multiplexed, so this
//! module does not invent a simple frequency-word readback contract.
//! The transmitter checks REG_51 with its other tuning controls.
//!
//! CTCSS remains configured through Roger. Shutdown removes PA first and
//! then disables REG_51. No reverse burst or additional RF tail is generated.

use super::write;

pub(super) const fn control(value: u16) -> u16 {
    if value == 0 { 0 } else { 0x9040 }
}

const fn frequency_word(value: u16) -> u16 {
    ((value as u32 * 206488 + 50000) / 100000) as u16
}

pub(super) fn configure(value: u16) -> Result<(), u32> {
    if !crate::ctcss::valid(value as u32) {
        return Err(0x700D);
    }

    write(0x51, control(value));
    if value != 0 {
        write(0x07, frequency_word(value));
    }
    Ok(())
}

/// TX uses one checked selector. Analog RX callers continue using configure
/// and control above, so adding DCS cannot change their detector setup.
pub(super) const fn tx_control(value: u16) -> u16 {
    if crate::dcs::decode(value).is_some() { 0xA033 } else { control(value) }
}

const fn dcs_setup(value: u16) -> Option<[u16; 4]> {
    match crate::dcs::codeword(value) {
        Some(word) => Some([
            0xA033,
            2775,
            (word & 0x0FFF) as u16,
            0x8000 | ((word >> 12) & 0x0FFF) as u16,
        ]),
        None => None,
    }
}

/// Follows the BK4829 reference port's actual register values rather than
/// the conflicting polarity comment beside REG_51. Normal and inverted
/// selections keep A033; inversion is applied once to the payload word.
/// REG_07 and REG_08 are multiplexed, so no simple readback is assumed.
/// The caller has already parked RX and has not yet admitted PA drive.
pub(super) fn configure_tx(value: u16) -> Result<(), u32> {
    if !crate::tx_signal::valid(value as u32) {
        return Err(0x700D);
    }

    if let Some(words) = dcs_setup(value) {
        write(0x51, words[0]);
        write(0x07, words[1]);
        write(0x08, words[2]);
        write(0x08, words[3]);
        Ok(())
    } else {
        configure(value)
    }
}

const _: () = {
    let normal = match dcs_setup(0x8013) {
        Some(words) => words,
        None => panic!("D023N setup rejected"),
    };
    assert!(normal[0] == 0xA033 && normal[1] == 2775);
    assert!(normal[2] == 0x0813 && normal[3] == 0x8763);

    let inverse = match dcs_setup(0xC013) {
        Some(words) => words,
        None => panic!("D023I setup rejected"),
    };
    assert!(inverse[0] == 0xA033 && inverse[1] == 2775);
    assert!(inverse[2] == 0x07EC && inverse[3] == 0x809C);
    assert!(dcs_setup(1000).is_none());
    assert!(tx_control(0) == 0 && tx_control(1000) == 0x9040);
};

const _: () = {
    assert!(control(0) == 0);
    assert!(control(1000) == 0x9040);
    assert!(frequency_word(670) == 1383);
    assert!(frequency_word(1000) == 2065);
    assert!(frequency_word(2541) == 5247);
    assert!(frequency_word(2541) < 8192);
};