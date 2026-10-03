//! One mutually exclusive signaling selection for either direction.
//!
//! Zero disables subaudio. A recognized CTCSS value retains its integer
//! tenths-of-hertz representation. Tagged DCS values use dcs's checked
//! sixteen-bit encoding. Arbitrary values and reserved bits are rejected.
//!
//! RX and TX store independent values using this same checked encoding.
//! This module changes no hardware and owns no dynamic or static buffer.

pub(crate) const fn valid(value: u32) -> bool {
    value <= u16::MAX as u32
        && (crate::ctcss::valid(value) || crate::dcs::decode(value as u16).is_some())
}

pub(crate) const fn text(value: u16) -> [u8; 5] {
    assert!(valid(value as u32));
    if crate::dcs::decode(value).is_some() {
        crate::dcs::text(value)
    } else {
        crate::ctcss::text(value)
    }
}

const _: () = {
    assert!(valid(0) && valid(1000) && valid(2541));
    assert!(valid(0x8013) && valid(0xC013));
    assert!(!valid(1001));
    assert!(!valid(0x4013) && !valid(0x8213));
    assert!(!valid(65536));
    assert!(text(0x8013)[0] == b'D');
    assert!(text(0x8013)[4] == b'N');
    assert!(text(0xC013)[4] == b'I');
};