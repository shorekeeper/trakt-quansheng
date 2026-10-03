//! CTCSS catalog and directional signaling word conversion.
//!
//! CTCSS frequencies are integer tenths of a hertz. Zero disables signaling.
//! The fifty admitted values span 67.0 through 254.1 Hz. Menu indices are
//! navigation positions and are never serialized as frequencies.
//!
//! The RAM signaling word stores TX in bits 15..0 and RX in bits 31..16.
//! Journal and channel-wire words store RX low and TX high. memory and wire
//! perform the explicit half-word conversion without allocating a buffer.
//!
//! Each half admits checked CTCSS or the tagged DCS selection defined by dcs.
//! RX and TX remain independent. Version-specific restrictions belong to
//! record and channel validation, not to this conversion primitive.
//!
//! The text helper formats CTCSS only. Generic signal text belongs to
//! tx_signal, which also recognizes tagged DCS. No helper in this module
//! accesses hardware or establishes measured tone deviation.

const VALUES: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915,
    948, 974, 1000, 1035, 1072, 1109, 1148, 1188, 1230, 1273,
    1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679,
    1713, 1738, 1773, 1799, 1835, 1862, 1899, 1928, 1966, 1995,
    2035, 2065, 2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
];

pub(crate) const fn count() -> u32 {
    VALUES.len() as u32 + 1
}

pub(crate) const fn at(selection: u32) -> Option<u16> {
    if selection == 0 {
        Some(0)
    } else if selection <= VALUES.len() as u32 {
        Some(VALUES[selection as usize - 1])
    } else {
        None
    }
}

pub(crate) const fn selection(value: u16) -> Option<u32> {
    if value == 0 { return Some(0); }
    let mut index = 0;
    while index < VALUES.len() {
        if VALUES[index] == value { return Some(index as u32 + 1); }
        index += 1;
    }
    None
}

pub(crate) const fn valid(value: u32) -> bool {
    value <= u16::MAX as u32 && selection(value as u16).is_some()
}

pub(crate) const fn pair_valid(value: u32) -> bool {
    crate::tx_signal::valid(value & 65535)
        && crate::tx_signal::valid(value >> 16)
}

pub(crate) const fn pair(transmit: u16, receive: u16) -> u32 {
    assert!(crate::tx_signal::valid(transmit as u32)
        && crate::tx_signal::valid(receive as u32));
    transmit as u32 | ((receive as u32) << 16)
}

/// Pack one TX-only wire word. RX can be added in its low half.
pub(crate) const fn word(value: u16) -> u32 {
    assert!(valid(value as u32));
    (value as u32) << 16
}

pub(crate) const fn wire(memory: u32) -> u32 {
    assert!(pair_valid(memory));
    memory.rotate_left(16)
}

pub(crate) const fn memory(wire: u32) -> Option<u32> {
    let value = wire.rotate_left(16);
    if pair_valid(value) { Some(value) } else { None }
}

/// Return TX after checking both wire halves.
pub(crate) const fn from_word(word: u32) -> Option<u16> {
    match memory(word) {
        Some(value) => Some(value as u16),
        None => None,
    }
}

pub(crate) const fn text(value: u16) -> [u8; 5] {
    assert!(valid(value as u32));
    if value == 0 { return *b" OFF "; }
    let hz = value / 10;
    [
        if hz >= 100 { b'0' + (hz / 100) as u8 } else { b' ' },
        b'0' + ((hz / 10) % 10) as u8,
        b'0' + (hz % 10) as u8,
        b'.',
        b'0' + (value % 10) as u8,
    ]
}

const _: () = {
    assert!(count() == 51);
    assert!(valid(0) && valid(670) && valid(1000) && valid(2541));
    assert!(!valid(1) && !valid(669) && !valid(2542) && !valid(65536));
    assert!(at(51).is_none());
    assert!(from_word(1).is_none());
    assert!(matches!(from_word(1000), Some(0)));
    assert!(matches!(from_word(word(1000)), Some(1000)));

    let mut tx = 0;
    while tx < count() {
        let transmit = match at(tx) {
            Some(value) => value,
            None => panic!("TX selection rejected"),
        };
        let mut rx = 0;
        while rx < count() {
            let receive = match at(rx) {
                Some(value) => value,
                None => panic!("RX selection rejected"),
            };
            let packed = pair(transmit, receive);
            assert!(pair_valid(packed));
            let encoded = wire(packed);
            assert!(encoded >> 16 == transmit as u32);
            assert!(encoded & 65535 == receive as u32);
            assert!(matches!(memory(encoded), Some(value) if value == packed));
            rx += 1;
        }
        tx += 1;
    }

    let mut index = 0;
    while index < VALUES.len() {
        let value = VALUES[index];
        if index != 0 { assert!(value > VALUES[index - 1]); }
        assert!(matches!(selection(value), Some(v) if v == index as u32 + 1));
        index += 1;
    }
    assert!(!pair_valid(1 << 16));
    let mixed = pair(0x8013, 885);
    assert!(pair_valid(mixed));
    assert!(matches!(memory(wire(mixed)), Some(value) if value == mixed));
    assert!(matches!(memory(0x8013), Some(0x8013_0000)));
    let both = pair(0xC013, 0x8015);
    assert!(pair_valid(both));
    assert!(matches!(memory(wire(both)), Some(value) if value == both));
    assert!(memory(0x8213).is_none());
    assert!(text(670)[1] == b'6');
    assert!(text(2541)[4] == b'1');
};