//! Binary reply encoding into caller-owned stable transmission storage.
//!
//! Every reply has a twenty-byte header: eight format bytes, then little-endian
//! payload length, payload CRC32, and observed tick. CRC covers payload only.
//!
//! TKFB uses version 1, width 128, height 64, page format 1, and 1024 payload
//! bytes. TKCH exports schema 9 with sixteen twenty-byte slots, totaling 320
//! bytes. TKCA uses version 1 and no payload; status 1 means validated/staged,
//! and status 2 means semantic rejection.
//!
//! The endpoint must be idle before encoding and retain the bytes through
//! drain completion. These functions allocate no packet buffer, access no
//! peripheral, schedule no retry, and cannot claim application or committed Save.

use crate::storage::calibration::crc32;
use crate::ui::Frame;

const fn put(bytes: &mut [u8; 1044], offset: usize, value: u32) {
    let encoded = value.to_le_bytes();
    let mut index = 0;
    while index < encoded.len() {
        bytes[offset + index] = encoded[index];
        index += 1;
    }
}

const fn header(
    bytes: &mut [u8; 1044],
    format: [u8; 8],
    length: usize,
    now: u32,
) {
    assert!(length <= 1024);

    let mut index = 0;
    while index < format.len() {
        bytes[index] = format[index];
        index += 1;
    }

    let crc = crc32(bytes.split_at(20).1.split_at(length).0);
    put(bytes, 8, length as u32);
    put(bytes, 12, crc);
    put(bytes, 16, now);
}

pub(crate) fn screenshot(bytes: &mut [u8; 1044], now: u32, frame: &Frame) {
    for index in 0..1024 {
        bytes[20 + index] = frame.byte(index);
    }

    header(bytes, [b'T', b'K', b'F', b'B', 1, 128, 64, 1], 1024, now);
}

/// The application writes the complete bank before the header is finalized.
pub(crate) fn channels(
    bytes: &mut [u8; 1044],
    now: u32,
    fill: impl FnOnce(&mut [u8]),
) {
    fill(&mut bytes[20..340]);
    header(bytes, [b'T', b'K', b'C', b'H', 9, 16, 20, 0], 320, now);
}

pub(crate) const fn acknowledgement(
    bytes: &mut [u8; 1044],
    now: u32,
    status: u8,
) {
    assert!(status == 1 || status == 2);
    header(bytes, [b'T', b'K', b'C', b'A', 1, status, 0, 0], 0, now);
}

const fn word(bytes: &[u8; 1044], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3],
    ])
}

const _: () = {
    let mut bytes = [0xA5u8; 1044];

    header(
        &mut bytes,
        [b'T', b'K', b'F', b'B', 1, 128, 64, 1],
        1024,
        0x1234_5678,
    );
    assert!(bytes[2] == b'F' && bytes[3] == b'B');
    assert!(word(&bytes, 8) == 1024);
    assert!(word(&bytes, 12) == crc32(bytes.split_at(20).1));
    assert!(word(&bytes, 16) == 0x1234_5678);
    assert!(bytes[16] == 0x78 && bytes[19] == 0x12);

    header(
        &mut bytes,
        [b'T', b'K', b'C', b'H', 9, 16, 20, 0],
        320,
        99,
    );
    assert!(bytes[2] == b'C' && bytes[3] == b'H');
    assert!(word(&bytes, 8) == 320);
    assert!(word(&bytes, 12) == crc32(bytes.split_at(20).1.split_at(320).0));
    assert!(word(&bytes, 16) == 99);

    acknowledgement(&mut bytes, 100, 1);
    assert!(bytes[3] == b'A' && bytes[5] == 1);
    assert!(word(&bytes, 8) == 0 && word(&bytes, 12) == 0);
    assert!(word(&bytes, 16) == 100);

    acknowledgement(&mut bytes, 101, 2);
    assert!(bytes[5] == 2 && word(&bytes, 16) == 101);
    assert!(bytes[20] == 0xA5);
};