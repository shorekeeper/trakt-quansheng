//! Host register checks for the production microphone processing owner.
//!
//! This model checks masks, coefficient selection, readback, and restoration.
//! It does not model analog clipping, gain dynamics, compressor acoustics,
//! microphone input impedance, or modulation deviation.

use std::cell::RefCell;

std::thread_local! {
    static WORDS: RefCell<[u16; 128]> = const { RefCell::new([0; 128]) };
}

fn read(register: u8) -> u16 {
    WORDS.with(|words| words.borrow()[register as usize])
}

fn write(register: u8, value: u16) {
    WORDS.with(|words| words.borrow_mut()[register as usize] = value);
}

#[path = "../src/drivers/bk4829/voice.rs"]
mod hardware;

fn seed() {
    WORDS.with(|words| {
        let mut words = words.borrow_mut();
        *words = [0; 128];
        words[0x19] = 0x1041;
        words[0x29] = 0xAA00;
        words[0x31] = 0x4004;
    });
}

#[test]
fn saved_selections_roundtrip_and_reject_reserved_bits() {
    assert_eq!(crate::voice::Settings::new().word(), 0);
    for word in 0..4 {
        let settings = crate::voice::Settings::decode(word).unwrap();
        assert_eq!(settings.word(), word);
    }
    assert!(crate::voice::Settings::decode(4).is_none());
}

#[test]
fn every_profile_preserves_unrelated_fields_and_restores_rx_words() {
    for compression in [false, true] {
        for agc in [false, true] {
            seed();
            let mut owner = hardware::Processing::new();
            let settings = crate::voice::Settings { compression, agc };
            owner.prepare(settings).unwrap();

            assert_eq!(read(0x19), if agc { 0x1041 } else { 0x9041 });
            assert_eq!(read(0x29), if compression { 0xAB40 } else { 0xAA00 });
            assert_eq!(read(0x31), 0x4004);

            // REG_31 installation belongs to the transmitter's next step.
            write(0x31, read(0x31) | owner.control());
            owner.check().unwrap();

            owner.tail();
            assert_eq!(owner.control(), 0);
            assert_eq!(read(0x31), 0x4004);
            owner.check().unwrap();

            owner.stop();
            assert_eq!(read(0x19), 0x1041);
            assert_eq!(read(0x29), 0xAA00);
            assert_eq!(read(0x31), 0x4004);
            assert_eq!(owner.control(), 0);
            owner.check().unwrap();
        }
    }
}

#[test]
fn mismatch_is_reported_without_corrective_reprogramming() {
    for register in [0x19u8, 0x29, 0x31] {
        seed();
        let mut owner = hardware::Processing::new();
        owner.prepare(crate::voice::Settings {
            compression: true, agc: true,
        }).unwrap();
        write(0x31, read(0x31) | owner.control());
        write(register, read(register) ^ if register == 0x31 { 8 } else { 1 });

        let damaged = read(register);
        assert_eq!(owner.check(), Err(0x7D00 + register as u32));
        assert_eq!(read(register), damaged);
        owner.stop();
    }
}

#[test]
fn double_preparation_cannot_replace_captured_restoration_state() {
    seed();
    let mut owner = hardware::Processing::new();
    owner.prepare(crate::voice::Settings {
        compression: true, agc: false,
    }).unwrap();
    assert_eq!(
        owner.prepare(crate::voice::Settings::new()), Err(0x7D01),
    );
    owner.stop();
    assert_eq!(read(0x19), 0x1041);
    assert_eq!(read(0x29), 0xAA00);
}