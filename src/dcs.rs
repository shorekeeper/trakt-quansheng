//! Checked DCS selections and a hardware-clocked 23-bit transmit word.
//!
//! Codes use three octal digits. The catalog contains the 104 codes used
//! by the board reference port; these are numeric codes, not menu indices.
//! The host channel tool reads this catalog as data to avoid a second list.
//!
//! A stored sixteen-bit selection has bit 15 set for DCS, bit 14 for
//! inverted polarity, and the nine-bit octal code in bits 8..0.
//! Bits 13..9 must be zero. This namespace is disjoint from CTCSS values.
//!
//! The normal word contains twelve low data bits: 0800 OR the code.
//! Eleven high check bits are obtained by polynomial division using C75.
//! Inversion complements exactly twenty-three bits. It is not a second
//! independent hardware-polarity setting.
//!
//! Only a complete word is supplied to the chip. Foreground does not time
//! individual symbols. These checks do not establish RF deviation or
//! interoperability; normal and inverted operation need receiver testing.

const CODES: [u16; 104] = [
    0o023, 0o025, 0o026, 0o031, 0o032, 0o036, 0o043, 0o047,
    0o051, 0o053, 0o054, 0o065, 0o071, 0o072, 0o073, 0o074,
    0o114, 0o115, 0o116, 0o122, 0o125, 0o131, 0o132, 0o134,
    0o143, 0o145, 0o152, 0o155, 0o156, 0o162, 0o165, 0o172,
    0o174, 0o205, 0o212, 0o223, 0o225, 0o226, 0o243, 0o244,
    0o245, 0o246, 0o251, 0o252, 0o255, 0o261, 0o263, 0o265,
    0o266, 0o271, 0o274, 0o306, 0o311, 0o315, 0o325, 0o331,
    0o332, 0o343, 0o346, 0o351, 0o356, 0o364, 0o365, 0o371,
    0o411, 0o412, 0o413, 0o423, 0o431, 0o432, 0o445, 0o446,
    0o452, 0o454, 0o455, 0o462, 0o464, 0o465, 0o466, 0o503,
    0o506, 0o516, 0o523, 0o526, 0o532, 0o546, 0o565, 0o606,
    0o612, 0o624, 0o627, 0o631, 0o632, 0o654, 0o662, 0o664,
    0o703, 0o712, 0o723, 0o731, 0o732, 0o734, 0o743, 0o754,
];

pub(crate) const fn count() -> usize {
    CODES.len()
}

pub(crate) const fn at(index: usize) -> u16 {
    CODES[index]
}

pub(crate) const fn index(code: u16) -> Option<usize> {
    let mut index = 0;
    while index < CODES.len() {
        if CODES[index] == code { return Some(index); }
        index += 1;
    }
    None
}

pub(crate) const fn encode(code: u16, inverted: bool) -> Option<u16> {
    if index(code).is_none() {
        None
    } else {
        Some(0x8000 | ((inverted as u16) << 14) | code)
    }
}

pub(crate) const fn decode(value: u16) -> Option<(u16, bool)> {
    let code = value & 511;
    if value & 0xBE00 != 0x8000 || index(code).is_none() {
        None
    } else {
        Some((code, value & 0x4000 != 0))
    }
}

pub(crate) const fn codeword(value: u16) -> Option<u32> {
    let (code, inverted) = match decode(value) {
        Some(value) => value,
        None => return None,
    };

    let data = 0x800 | code as u32;
    let mut remainder = data << 11;
    let mut shift = 12;

    while shift != 0 {
        shift -= 1;
        if remainder & (1 << (shift + 11)) != 0 {
            remainder ^= 0xC75 << shift;
        }
    }

    let normal = data | (remainder << 12);
    Some(if inverted { normal ^ 0x7FFFFF } else { normal })
}

pub(crate) const fn text(value: u16) -> [u8; 5] {
    let (code, inverted) = match decode(value) {
        Some(value) => value,
        None => panic!("invalid DCS display value"),
    };
    [
        b'D',
        b'0' + ((code >> 6) & 7) as u8,
        b'0' + ((code >> 3) & 7) as u8,
        b'0' + (code & 7) as u8,
        if inverted { b'I' } else { b'N' },
    ]
}

const _: () = {
    assert!(count() == 104);
    assert!(matches!(encode(0o023, false), Some(0x8013)));
    assert!(matches!(encode(0o023, true), Some(0xC013)));
    assert!(matches!(codeword(0x8013), Some(0x763813)));
    assert!(matches!(codeword(0xC013), Some(0x09C7EC)));
    assert!(decode(0x4013).is_none());
    assert!(decode(0x8213).is_none());
    assert!(decode(0x8000).is_none());
    assert!(decode(0xFFFF).is_none());
    assert!(encode(0o777, false).is_none());

    let mut i = 0;
    while i < CODES.len() {
        if i != 0 { assert!(CODES[i] > CODES[i - 1]); }
        let normal = match encode(CODES[i], false) {
            Some(value) => value,
            None => panic!("DCS catalog entry rejected"),
        };
        let inverse = match encode(CODES[i], true) {
            Some(value) => value,
            None => panic!("inverted DCS catalog entry rejected"),
        };
        let a = match codeword(normal) {
            Some(value) => value,
            None => panic!("normal codeword rejected"),
        };
        let b = match codeword(inverse) {
            Some(value) => value,
            None => panic!("inverted codeword rejected"),
        };
        assert!(a <= 0x7FFFFF && b <= 0x7FFFFF);
        assert!(a ^ b == 0x7FFFFF);
        assert!(a & 0xFFF == 0x800 | CODES[i] as u32);
        assert!(text(normal)[4] == b'N' && text(inverse)[4] == b'I');
        i += 1;
    }
};