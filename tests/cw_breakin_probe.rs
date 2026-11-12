#![deny(warnings)]

//! Host register tests for the production CW receive-window owner.
//!
//! The model checks exact destinations, gain-nibble preservation, fade
//! cadence, preparation deadlines, cancellation, and readback errors.
//! It models neither analog AF bias nor PLL recovery, PA voltage, acoustic
//! clicks, or radiated RF. Mark edges remain with the real transmitter.

use std::cell::RefCell;

mod app {
    pub(crate) mod time {
        pub(crate) const fn reached(now: u32, deadline: u32) -> bool {
            now.wrapping_sub(deadline) < 0x8000_0000
        }
    }
}

mod mcu {
    use std::cell::Cell;

    std::thread_local! {
        static NOW: Cell<u32> = const { Cell::new(0) };
    }

    pub(crate) mod tick {
        pub(crate) fn snapshot() -> u32 {
            super::NOW.with(|now| now.get())
        }
    }

    pub(crate) fn set_tick(now: u32) {
        NOW.with(|value| value.set(now));
    }
}

struct Registers {
    words: [u16; 128],
    writes: Vec<(u8, u16)>,
}

std::thread_local! {
    static RF: RefCell<Registers> = const { RefCell::new(Registers {
        words: [0; 128], writes: Vec::new(),
    }) };
}

fn read(register: u8) -> u16 {
    RF.with(|rf| rf.borrow().words[register as usize])
}

fn write(register: u8, value: u16) {
    RF.with(|rf| {
        let mut rf = rf.borrow_mut();
        rf.words[register as usize] = value;
        rf.writes.push((register, value));
    });
}

fn verify_tuple(expected: [u16; 5]) -> Result<[u16; 5], u32> {
    let registers = [0x30u8, 0x33, 0x36, 0x37, 0x47];
    let mut actual = [0; 5];
    for index in 0..registers.len() {
        actual[index] = read(registers[index]);
        let mask = if index == 0 { 0x7FFF } else { 0xFFFF };
        if actual[index] & mask != expected[index] & mask {
            return Err(0x7E00 + registers[index] as u32);
        }
    }
    Ok(actual)
}

fn verify_transmit(expected: [u16; 5]) -> Result<[u16; 5], u32> {
    if expected != [0xC3F2, 0x9000, 0, 0x9D1F, 0x6242] {
        return Err(0x7E07);
    }
    verify_tuple(expected)
}

fn verify_cw_receive(expected: [u16; 5]) -> Result<[u16; 5], u32> {
    if expected[0] != 0xBFF1
        || !matches!(expected[1], 0x9044 | 0x9048)
        || expected[2] != 0 || expected[3] != 0x9F1F
        || !matches!(expected[4], 0x6042 | 0x6542)
    {
        return Err(0x7E07);
    }
    verify_tuple(expected)
}

// Pure input vocabulary used by the production sidetone owner.
mod cw {
    pub(crate) const fn valid_pitch(pitch: u16) -> bool {
        pitch >= 400 && pitch <= 1000 && pitch % 100 == 0
    }
}

mod sound {
    pub(super) const fn frequency_word(hz: u16) -> u16 {
        assert!(hz <= 3000);
        (((hz as u32 * 1_353_245) + 65_536) >> 17) as u16
    }
}

#[path = "../src/cw_tail.rs"]
mod cw_tail;

#[path = "../src/drivers/bk4829/cw_audio.rs"]
mod cw_audio;

#[path = "../src/drivers/bk4829/cw_breakin.rs"]
mod hardware;

const TX: [u16; 5] = [0xC3F2, 0x9000, 0, 0x9D1F, 0x6242];
const GAIN: u16 = 0xB37E;

fn set_frequency(value: u32) {
    write(0x38, value as u16);
    write(0x39, (value >> 16) as u16);
}

fn install(controls: [u16; 5]) {
    for (register, value) in [0x30u8, 0x33, 0x36, 0x37, 0x47]
        .into_iter().zip(controls)
    {
        write(register, value);
    }
}

fn prepared(wait: u32, nominal: u32) -> hardware::BreakIn {
    prepared_options(wait, nominal, 0)
}

fn prepared_options(
    wait: u32,
    nominal: u32,
    tail_word: u8,
) -> hardware::BreakIn {
    RF.with(|rf| {
        let mut rf = rf.borrow_mut();
        rf.words = [0; 128];
        rf.writes.clear();
    });
    mcu::set_tick(0);

    let gpio = if nominal < 28_000_000 { 0x9044 } else { 0x9048 };
    install([0xBFF1, gpio, 0, 0x9F1F, 0x6542]);
    set_frequency(nominal - 70);
    write(0x2B, 0x0500);
    write(0x3D, 0);
    write(0x7D, 0xE920);
    write(0x7E, 0x302E);
    write(0x31, 0);
    write(0x48, GAIN);

    let mut owner = hardware::BreakIn::new();
    assert!(!owner.active());
    owner.capture(nominal, nominal - 70, wait, tail_word).unwrap();

    install(TX);
    set_frequency(nominal);
    write(0x2B, 0);
    write(0x3D, 0x1234);
    write(0x7D, 0xE940);
    write(0x7E, 0x302A);
    write(0x31, 0);
    write(0x70, 0x9C00);
    write(0x71, 0);
    owner.prepare(TX).unwrap();
    assert!(owner.active() && owner.tx_ready() && !owner.in_gap());
    RF.with(|rf| rf.borrow_mut().writes.clear());
    owner
}

fn pump(
    owner: &mut hardware::BreakIn,
    now: u32,
    prepare: bool,
    controls: [u16; 5],
) -> [u16; 5] {
    mcu::set_tick(now);
    owner.pump(now, prepare, controls).unwrap()
}

fn enter_rx(
    owner: &mut hardware::BreakIn,
    start: u32,
) -> [u16; 5] {
    let mut controls = pump(owner, start, false, TX);
    assert_eq!(controls, TX);
    assert!(!owner.tx_ready());
    for (delta, gain) in [(1, 10), (2, 6), (3, 2), (4, 0)] {
        controls = pump(owner, start.wrapping_add(delta), false, controls);
        assert_eq!(read(0x48), (GAIN & !15) | gain);
    }
    assert_eq!(controls[0], 0xBFF1);
    assert_eq!(controls[4], 0x6042);
    controls
}

#[test]
fn all_rx_waits_follow_fade_and_complete_destination_writes() {
    for wait in [50u32, 25, 10, 5, 2, 1, 0] {
        let mut owner = prepared(wait, 14_550_000);
        let rx = enter_rx(&mut owner, 100);
        let ready = 104 + wait;
        if wait != 0 {
            assert_eq!(pump(&mut owner, ready - 1, false, rx), rx);
            assert_eq!(read(0x48) & 15, 0);
        }
        let mut controls = pump(&mut owner, ready, false, rx);
        assert_eq!(controls[4], 0x6542);
        assert_eq!(read(0x48) & 15, 0);

        for (delta, gain) in [(1, 4), (2, 8), (3, 12), (4, 14)] {
            controls = pump(&mut owner, ready + delta, false, controls);
            assert_eq!(read(0x48), (GAIN & !15) | gain);
        }
        owner.check(controls).unwrap();
        assert!(!owner.tx_ready());
    }
}

#[test]
fn tx_preparation_fades_source_and_overlaps_gain_with_audio_wait() {
    let mut owner = prepared(0, 14_550_000);
    let rx = enter_rx(&mut owner, 10);
    let mut controls = pump(&mut owner, 14, false, rx);
    for now in 15..=18 {
        controls = pump(&mut owner, now, false, controls);
    }

    controls = pump(&mut owner, 20, true, controls);
    for now in 21..=24 {
        controls = pump(&mut owner, now, true, controls);
    }
    assert_eq!(controls, TX);
    assert_eq!(read(0x48) & 15, 0);
    assert!(!owner.tx_ready());

    for now in 25..=33 {
        controls = pump(&mut owner, now, true, controls);
        assert!(!owner.tx_ready());
    }
    assert_eq!(read(0x48), GAIN);
    controls = pump(&mut owner, 34, true, controls);
    assert_eq!(controls, TX);
    assert!(owner.tx_ready());

    // Final transmitter readback is represented by this later observation.
    owner.note_tx_ready(45);
    assert_eq!(owner.advance(), 27);
    assert_eq!(read(0x36), 0);
    assert_eq!(read(0x71), 0);
}

#[test]
fn early_preparation_finishes_falling_fade_without_rf_restart() {
    let mut owner = prepared(0, 14_550_000);
    let mut controls = pump(&mut owner, 10, false, TX);
    for (now, gain) in [(11, 10), (12, 6), (13, 2), (14, 0)] {
        controls = pump(&mut owner, now, true, controls);
        assert_eq!(controls, TX);
        assert!(!owner.tx_ready());
        assert_eq!(read(0x48) & 15, gain);
    }
    for (now, gain) in [(15, 4), (16, 8), (17, 12), (18, 14)] {
        controls = pump(&mut owner, now, true, controls);
        assert_eq!(controls, TX);
        assert_eq!(read(0x48) & 15, gain);
        assert_eq!(owner.tx_ready(), now == 18);
    }
    assert_eq!(read(0x48), GAIN);
    RF.with(|rf| {
        assert!(rf.borrow().writes.iter().all(|&(reg, _)| reg == 0x48));
    });
}

#[test]
fn rx_settling_can_be_interrupted_before_receive_af_admission() {
    let mut owner = prepared(50, 43_350_000);
    let rx = enter_rx(&mut owner, 10);
    assert_eq!(rx[1], 0x9048);
    let rx = pump(&mut owner, 15, true, rx);
    let tx = pump(&mut owner, 16, true, rx);
    assert_eq!(tx, TX);
    assert_eq!(read(0x47), 0x6242);
    assert!(!owner.tx_ready());
    let tx = pump(&mut owner, 26, true, tx);
    assert_eq!(tx, TX);
    assert!(owner.tx_ready());
}

#[test]
fn delayed_service_performs_one_gain_step_without_compressing_ramp() {
    let mut owner = prepared(0, 14_550_000);
    let controls = pump(&mut owner, 10, false, TX);
    let controls = pump(&mut owner, 100, false, controls);
    assert_eq!(controls, TX);
    assert_eq!(read(0x48) & 15, 10);
    let controls = pump(&mut owner, 100, false, controls);
    assert_eq!(controls, TX);
    assert_eq!(read(0x48) & 15, 10);
    let controls = pump(&mut owner, 101, false, controls);
    assert_eq!(controls, TX);
    assert_eq!(read(0x48) & 15, 6);
}

#[test]
fn transition_log_changes_af_only_at_zero_gain_and_excludes_pa() {
    let mut owner = prepared(0, 14_550_000);
    let rx = enter_rx(&mut owner, 10);
    let mut controls = pump(&mut owner, 14, false, rx);
    for now in 15..=18 {
        controls = pump(&mut owner, now, false, controls);
    }
    controls = pump(&mut owner, 20, true, controls);
    for now in 21..=24 {
        controls = pump(&mut owner, now, true, controls);
    }
    assert_eq!(controls, TX);

    RF.with(|rf| {
        let rf = rf.borrow();
        let mut gain = GAIN;
        for &(register, value) in &rf.writes {
            match register {
                0x48 => {
                    assert_eq!(value & !15, GAIN & !15);
                    gain = value;
                }
                0x47 => assert_eq!(gain & 15, 0),
                0x30 => {
                    assert_ne!(value & 0x0200, 0);
                    assert_eq!(value & 0x000C, 0);
                    assert_ne!(value & 3, 3);
                }
                0x33 => assert_eq!(value & 0x0021, 0),
                _ => {}
            }
            assert_ne!(register, 0x36);
            assert_ne!(register, 0x50);
            assert_ne!(register, 0x70);
        }
    });
}

#[test]
fn readback_error_does_not_admit_rx_af_or_repair_passive_configuration() {
    let mut owner = prepared(5, 14_550_000);
    let rx = enter_rx(&mut owner, 10);
    write(0x7E, 0x802E);
    mcu::set_tick(19);
    assert_eq!(owner.pump(19, false, rx), Err(0x7E7E));
    assert_eq!(read(0x7E), 0x802E);
    assert_eq!(read(0x47), 0x6042);
    assert_eq!(read(0x48) & 15, 0);
}

#[test]
fn gain_readback_is_exact_and_cleanup_restores_captured_word() {
    let mut owner = prepared(0, 14_550_000);
    let rx = enter_rx(&mut owner, 10);
    write(0x48, 0xB380);
    assert_eq!(owner.check(rx), Err(0x7E48));
    assert_eq!(read(0x48), 0xB380);
    owner.clear();
    assert_eq!(read(0x48), GAIN);
    assert!(!owner.active() && !owner.tx_ready() && !owner.in_gap());
}

#[test]
fn oscillator_frequency_survives_fade_and_stops_before_rx_entry() {
    let mut owner = prepared(0, 14_550_000);
    write(0x70, 0);
    let mut tone = cw_audio::Sidetone::new();
    tone.start(700, 28).unwrap();
    assert!(tone.active());
    assert_eq!(tone.control(), cw_audio::CONTROL);
    tone.key(true);
    tone.defer_release();
    assert!(tone.down() && tone.release_pending());

    let pitch = tone.expected_word();
    owner.space_tone(pitch);
    RF.with(|rf| rf.borrow_mut().writes.clear());
    let mut controls = pump(&mut owner, 10, false, TX);

    for now in 11..=16 {
        mcu::set_tick(now);
        owner.service_audio(now);
        if tone.release_pending() && owner.audio_release_ready(now) {
            tone.key(false);
        }
        owner.space_tone(tone.expected_word());
        controls = owner.pump(now, false, controls).unwrap();
        tone.check().unwrap();

        if now >= 14 {
            assert_eq!(read(0x48) & 15, 0);
        }
        if now < 16 {
            assert_eq!(read(0x71), pitch);
            assert!(tone.down());
            assert_eq!(controls, TX);
            assert!(!owner.audio_release_ready(now));
        } else {
            assert!(owner.audio_release_ready(now));
            assert_eq!(read(0x71), 0);
            assert!(!tone.down() && !tone.release_pending());
            assert_eq!(controls[0], 0xBFF1);
        }
    }

    RF.with(|rf| {
        let rf = rf.borrow();
        let mut gain = GAIN;
        let mut releases = 0;
        for &(register, value) in &rf.writes {
            if register == 0x48 {
                gain = value;
            }
            if register == 0x71 {
                assert_eq!(gain & 15, 0);
                assert_eq!(value, 0);
                releases += 1;
            }
        }
        assert_eq!(releases, 1);
    });

    tone.cancel();
    assert!(!tone.active() && !tone.release_pending());
    owner.clear();
}

#[test]
fn every_tail_choice_preserves_gain_fields_and_release_deadline() {
    for shape in 0..3 {
        for release in 0..=4 {
            let options = cw_tail::Settings { shape, release };
            let mut owner = prepared_options(0, 14_550_000, options.word());

            write(0x70, 0);
            let mut tone = cw_audio::Sidetone::new();
            tone.start(700, 28).unwrap();
            tone.key(true);
            tone.defer_release();
            let pitch = tone.expected_word();
            owner.space_tone(pitch);

            let mut controls = pump(&mut owner, 10, false, TX);
            let mut zero_at = None;
            let mut entered_at = None;

            for now in 11..=20 {
                mcu::set_tick(now);
                owner.service_audio(now);

                if zero_at.is_none() && owner.audio_muted() {
                    zero_at = Some(now);
                }
                if tone.release_pending() && owner.audio_release_ready(now) {
                    tone.key(false);
                }
                owner.space_tone(tone.expected_word());
                controls = owner.pump(now, false, controls).unwrap();

                assert_eq!(read(0x48) & !15, GAIN & !15);
                if now <= 14 {
                    assert_eq!(
                        read(0x48) & 15,
                        cw_tail::falling(14, 14, (now - 10) as u8, shape) as u16,
                    );
                }

                if controls[0] == 0xBFF1 {
                    entered_at = Some(now);
                    break;
                }
                assert_eq!(read(0x71), pitch);
                assert_eq!(read(0x36), 0);
            }

            assert_eq!(zero_at, Some(14));
            assert_eq!(entered_at, Some(14 + release as u32));
            assert_eq!(read(0x71), 0);
            assert!(!tone.release_pending());
            tone.check().unwrap();
            tone.cancel();
            owner.clear();
            assert_eq!(read(0x48), GAIN);
        }
    }
}

#[test]
fn invalid_local_tail_word_cannot_capture_a_session() {
    let mut owner = prepared(0, 14_550_000);
    owner.clear();
    RF.with(|rf| rf.borrow_mut().writes.clear());
    assert_eq!(
        owner.capture(14_550_000, 14_549_930, 0, 32),
        Err(0x7E08),
    );
    assert!(!owner.active());
    RF.with(|rf| assert!(rf.borrow().writes.is_empty()));
}

#[test]
fn capture_rejects_wrong_profile_without_hardware_writes() {
    let mut owner = prepared(5, 14_550_000);
    owner.clear();
    RF.with(|rf| rf.borrow_mut().writes.clear());
    assert!(owner.capture(14_550_000, 14_549_930, 5, 0).is_err());
    RF.with(|rf| assert!(rf.borrow().writes.is_empty()));
}

#[test]
fn calibration_bit_and_audio_deadlines_cross_counter_wrap() {
    let mut owner = prepared(5, 14_550_000);
    write(0x30, TX[0] & 0x7FFF);
    owner.check(TX).unwrap();

    let start = u32::MAX - 2;
    let rx = enter_rx(&mut owner, start);
    write(0x30, 0x3FF1);
    assert_eq!(pump(&mut owner, 5, false, rx), rx);
    let audible = pump(&mut owner, 6, false, rx);
    assert_eq!(audible[4], 0x6542);

    let mut controls = audible;
    for now in 7..=10 {
        controls = pump(&mut owner, now, false, controls);
    }
    let start = u32::MAX - 5;
    controls = pump(&mut owner, start, true, controls);
    for delta in 1..=4 {
        controls = pump(&mut owner, start.wrapping_add(delta), true, controls);
    }
    assert_eq!(controls, TX);
    assert!(!owner.tx_ready());
    assert_eq!(pump(&mut owner, 7, true, controls), TX);
    assert!(!owner.tx_ready());
    pump(&mut owner, 8, true, controls);
    assert!(owner.tx_ready());
}