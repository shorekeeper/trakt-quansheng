//! CW receive AGC table selection for this BK4829 board profile.
//!
//! The reference port's bk4819-regs.h describes REG_7E bit 15 as fixed
//! versus automatic AGC and bits 14..12 as the fixed table index. The five
//! established Trakt table words remain unchanged: index -1 uses REG_14,
//! and indices 0..3 use REG_10..13. Menu FIX 1..5 selects those entries.
//! Labels describe table positions, not measured gain or attenuation.
//!
//! Automatic uses the established REG_7E=302E preset. Fixed choices modify
//! only its upper mode/index nibble; the low DC-filter fields remain 02E.
//! No table word, LNA GPIO, PA, RX link, bandwidth, or frequency is changed.
//!
//! Configure runs only inside the receiver's muted tune path. The receiver
//! checks the resulting word with its ordinary readback checkpoints. Normal
//! voice reception and muted TX carrier acquisition select AUTO.
//! The transmitter still captures and restores REG_7E independently.
//!
//! Manual gain changes affect raw RSSI. Decoder thresholds must be chosen
//! at the selected gain. An RF attempt must collect new carrier observations
//! in AUTO before admitting PA; a reduced-gain observation is insufficient.

use super::{read, write};

const fn word(selection: u8) -> u16 {
    assert!(selection < 6);
    if selection == 0 {
        0x302E
    } else {
        let index = if selection == 1 { 4 } else { selection - 2 };
        0x802E | ((index as u16) << 12)
    }
}

pub(super) fn configure(selection: u8) {
    write(0x7E, word(selection));
}

pub(super) fn check(selection: u8) -> Result<(), u32> {
    if read(0x7E) == word(selection) { Ok(()) } else { Err(0x507E) }
}

const _: () = {
    assert!(word(0) == 0x302E);
    assert!(word(1) == 0xC02E);
    assert!(word(2) == 0x802E);
    assert!(word(3) == 0x902E);
    assert!(word(4) == 0xA02E);
    assert!(word(5) == 0xB02E);
    let mut selection = 0;
    while selection < 6 {
        assert!(word(selection) & 0x0FFF == 0x002E);
        selection += 1;
    }
};