//! Fields decoded from the board's 512-byte factory block.
//!
//! Battery entries are six little-endian words at 140. Miscellaneous
//! calibration begins at 188: signed crystal correction, two filter words,
//! volume gain, and DAC gain. Raw values are retained without replacement.
//!
//! Flags are plausibility checks, not factory authenticity:
//! bit 0 rejects an all-zero or all-FF block; bit 1 checks the first battery
//! pair; bit 2 checks crystal and audio-gain ranges. Battery units are not
//! inferred, and the remaining four entries are not ordered or overridden.
//!
//! CRC32 is reflected ISO-HDLC, initial and final XOR FFFFFFFF.

#[derive(Clone, Copy)]
pub(crate) struct Summary {
    pub(crate) crc32: u32,
    pub(crate) flags: u32,
    pub(crate) battery: [u16; 6],
    pub(crate) crystal: i16,
    pub(crate) filter_low: u16,
    pub(crate) filter_high: u16,
    pub(crate) volume: u8,
    pub(crate) dac: u8,
}

impl Summary {
    pub(crate) const fn empty() -> Self {
        Self {
            crc32: 0,
            flags: 0,
            battery: [0; 6],
            crystal: 0,
            filter_low: 0,
            filter_high: 0,
            volume: 0,
            dac: 0,
        }
    }
}

/// Reflected CRC32 over exactly the supplied bytes, without allocation or
/// device access. Runtime callers share this call boundary. Constant format
/// fixtures evaluate the same implementation during compilation.
#[inline(never)]
pub(crate) const fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    let mut index = 0;
    while index < bytes.len() {
        crc ^= bytes[index] as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        index += 1;
    }
    !crc
}

const fn word(bytes: &[u8; 512], offset: usize) -> u16 {
    bytes[offset] as u16 | ((bytes[offset + 1] as u16) << 8)
}

pub(crate) const fn decode(bytes: &[u8; 512]) -> Summary {
    let mut zero = true;
    let mut erased = true;
    let mut index = 0;
    while index < bytes.len() {
        zero = zero && bytes[index] == 0;
        erased = erased && bytes[index] == 255;
        index += 1;
    }
    let present = !zero && !erased;

    let mut battery = [0u16; 6];
    index = 0;
    while index < battery.len() {
        battery[index] = word(bytes, 0x140 + index * 2);
        index += 1;
    }

    let crystal = word(bytes, 0x188) as i16;
    let volume = bytes[0x18E];
    let dac = bytes[0x18F];

    let battery_ok = present && battery[0] > 0
        && battery[0] < battery[1] && battery[1] <= 4095;
    let misc_ok = present && crystal >= -1000 && crystal <= 1000
        && volume < 64 && dac < 16;

    Summary {
        crc32: crc32(bytes),
        flags: present as u32 | ((battery_ok as u32) << 1) | ((misc_ok as u32) << 2),
        battery,
        crystal,
        filter_low: word(bytes, 0x18A),
        filter_high: word(bytes, 0x18C),
        volume,
        dac,
    }
}

const _: () = {
    assert!(crc32(b"123456789") == 0xCBF4_3926);
    assert!(crc32(b"") == 0);
    assert!(decode(&[0; 512]).flags == 0);
    assert!(decode(&[255; 512]).flags == 0);

    let mut bytes = [0u8; 512];
    bytes[0x140] = 0x6C;
    bytes[0x141] = 7;
    bytes[0x142] = 0xD0;
    bytes[0x143] = 7;
    bytes[0x188] = 0xFB;
    bytes[0x189] = 0xFF;
    bytes[0x18A] = 0x46;
    bytes[0x18C] = 0x50;
    bytes[0x18E] = 58;
    bytes[0x18F] = 8;

    let summary = decode(&bytes);
    assert!(summary.flags == 7);
    assert!(summary.battery[0] == 1900);
    assert!(summary.battery[1] == 2000);
    assert!(summary.crystal == -5);
    assert!(summary.filter_low == 70 && summary.filter_high == 80);

    bytes[0x18E] = 64;
    assert!(decode(&bytes).flags == 3);
    bytes[0x18E] = 58;
    bytes[0x142] = bytes[0x140];
    bytes[0x143] = bytes[0x141];
    assert!(decode(&bytes).flags == 5);
};