//! BK4829 receive demodulator selection under the receiver's ownership.
//!
//! The caller first mutes the external amplifier and stops RF links.
//! REG_31 bit zero selects AM; bits 3..1 remain clear as in the FM path.
//! REG_42 and REG_2A use the board reference port's AM/FM pairs. Their
//! undocumented internal fields are not assigned speculative meanings.
//!
//! The reference is armel/uv-k1-k5v3-firmware-custom, App/radio.c,
//! RADIO_SetModulation, and App/driver/bk4829.c filter selection.
//! Normal AF output remains REG_47=6142 when audible, 6042 when muted.
//! AF selector seven is not used. Amplifier timing belongs to Receiver.
//!
//! Narrow AM uses 345C; wide AM uses 3028. These are register presets,
//! not measured occupied bandwidths or an implementation of 8.33 kHz
//! channel numbering. FM retains 4048 for narrow and 3028 for wide.
//!
//! The established AGC table, calibrated output gain, audio filters, and
//! AFC-disabled REG_73 preset remain unchanged. No MCU audio sampling,
//! software gain loop, or transmit-envelope control is introduced.
//!
//! Every tune writes both sides of the AM/FM register pair, preventing
//! retention of AM coefficients after a change back to FM. Readback is
//! checked at normal receiver checkpoints, not treated as proof of RF
//! performance or intelligible AM audio.

use super::{read, write};

pub(super) const fn bandwidth(am: bool, narrow: bool) -> u16 {
    if !narrow { 0x3028 } else if am { 0x345C } else { 0x4048 }
}

const fn coefficients(am: bool) -> [u16; 2] {
    if am { [0x6F5C, 0x7434] } else { [0x6B5A, 0x7400] }
}

/// Requires muted audio, stopped RF links, and exclusive foreground access.
/// Only receive demodulator registers are touched here.
pub(super) fn configure(am: bool) {
    let values = coefficients(am);
    write(0x31, (read(0x31) & !15) | am as u16);
    write(0x42, values[0]);
    write(0x2A, values[1]);
}

pub(super) fn check(am: bool) -> Result<(), u32> {
    let values = coefficients(am);
    if read(0x31) & 15 != am as u16 {
        return Err(0x5031);
    }
    if read(0x42) != values[0] {
        return Err(0x5042);
    }
    if read(0x2A) != values[1] {
        return Err(0x502A);
    }
    Ok(())
}

const _: () = {
    assert!(bandwidth(false, false) == 0x3028);
    assert!(bandwidth(false, true) == 0x4048);
    assert!(bandwidth(true, false) == 0x3028);
    assert!(bandwidth(true, true) == 0x345C);
    assert!(coefficients(false)[0] == 0x6B5A);
    assert!(coefficients(false)[1] == 0x7400);
    assert!(coefficients(true)[0] == 0x6F5C);
    assert!(coefficients(true)[1] == 0x7434);
};