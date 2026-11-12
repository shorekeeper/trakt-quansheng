//! Host register model for the production BK4829 comparator owner.
//!
//! Only passive configuration and the event acknowledgement contract are
//! modeled. Each test has thread-local registers. This checks software
//! ownership, entry sampling, retained events, and cancellation; it does not
//! execute the complete transmitter or establish physical VOX behavior.

use std::cell::RefCell;

struct Registers {
    words: [u16; 128],
    pending: u16,
    consumed: u16,
}

std::thread_local! {
    static REGISTERS: RefCell<Registers> = const { RefCell::new(Registers {
        words: [0; 128], pending: 0, consumed: 0,
    }) };
}

fn read(register: u8) -> u16 {
    REGISTERS.with(|state| {
        let state = state.borrow();
        match register {
            0x0C => if state.pending != 0 { 1 } else { 0 },
            0x02 => state.consumed,
            _ => state.words[register as usize],
        }
    })
}

fn write(register: u8, value: u16) {
    REGISTERS.with(|state| {
        let mut state = state.borrow_mut();
        if register == 0x02 {
            assert_eq!(value, 0);
            state.consumed = state.pending;
            state.pending = 0;
        } else {
            state.words[register as usize] = value;
        }
    });
}

#[path = "../src/drivers/bk4829/vox.rs"]
mod hardware;

fn seed() {
    REGISTERS.with(|state| {
        let mut state = state.borrow_mut();
        state.words = [0; 128];
        state.words[0x46] = 0x600A;
        state.words[0x79] = 0x1234;
        state.words[0x7A] = 0x5678;
        state.pending = 0;
        state.consumed = 0;
    });
}

fn event(flags: u16) {
    REGISTERS.with(|state| state.borrow_mut().pending = flags);
}

fn requested_gate() -> (crate::vox::EventGate, crate::vox::Settings) {
    use crate::vox::{EventGate, Sample, Settings};

    let settings = Settings { enabled: true, hang: 1, ..Settings::new() };
    let mut gate = EventGate::new();
    for tick in (0..=100).step_by(10) {
        assert!(!gate.observe(
            tick, settings,
            Some(Sample { tick, voice: Some(false), events: 0x0020 }),
            true, false,
        ));
    }
    assert!(!gate.observe(
        110, settings,
        Some(Sample { tick: 110, voice: Some(true), events: 0x0010 }),
        true, false,
    ));
    assert!(gate.observe(
        120, settings,
        Some(Sample { tick: 120, voice: Some(true), events: 0x0010 }),
        true, false,
    ));
    (gate, settings)
}

#[test]
fn entry_poll_supplies_unknown_observation_before_required_tx_sampling() {
    seed();
    let (mut gate, settings) = requested_gate();
    assert!(gate.pause(150));

    let mut detector = hardware::Detector::new();
    detector.start(160, 128).unwrap();
    assert!(detector.sample(200).is_none());

    // The setup executor performs this poll before publishing On.
    detector.pump(200);
    let sample = detector.sample(200).unwrap();
    assert_eq!(sample.tick, 200);
    assert_eq!(sample.voice, None);
    assert_eq!(sample.events, 0);
    assert!(gate.observe(200, settings, Some(sample), true, false));
    assert!(!gate.completed());

    event(0x0020);
    for tick in (210..1120).step_by(10) {
        detector.pump(tick);
        assert!(gate.observe(
            tick, settings, detector.sample(tick), true, false,
        ));
    }
    detector.pump(1120);
    assert!(!gate.observe(
        1120, settings, detector.sample(1120), true, false,
    ));
    assert!(gate.completed());
}

#[test]
fn an_absent_entry_sample_cancels_instead_of_retaining_a_stale_request() {
    seed();
    let (mut gate, settings) = requested_gate();
    assert!(gate.pause(150));

    let mut detector = hardware::Detector::new();
    detector.start(160, 128).unwrap();
    assert!(!gate.observe(
        200, settings, detector.sample(200), true, false,
    ));
    assert!(!gate.requested(200));
    assert!(!gate.completed());
}

#[test]
fn last_nonzero_vox_event_survives_ordinary_and_shared_tone_polls() {
    seed();
    let mut detector = hardware::Detector::new();
    detector.start(0, 128).unwrap();

    event(0x0010);
    detector.pump(0);
    let first = detector.sample(0).unwrap();
    assert_eq!(first.voice, Some(true));
    assert_eq!(first.events, 0x0010);

    detector.pump(10);
    assert_eq!(detector.sample(10).unwrap().events, 0x0010);

    event(0x0100);
    detector.pump(20);
    let shared = detector.sample(20).unwrap();
    assert_eq!(shared.voice, Some(true));
    assert_eq!(shared.events, 0x0010);

    event(0x0020);
    detector.pump(30);
    detector.pump(40);
    let quiet = detector.sample(40).unwrap();
    assert_eq!(quiet.voice, Some(false));
    assert_eq!(quiet.events, 0x0020);
    assert!(detector.sample(61).is_none());
}

#[test]
fn threshold_identity_and_cleanup_are_exact() {
    seed();
    let mut detector = hardware::Detector::new();
    assert!(!detector.active());
    assert!(detector.matches(0));

    detector.start(0, 128).unwrap();
    assert!(detector.active());
    assert!(detector.matches(128));
    assert!(!detector.matches(127));
    assert!(!detector.matches(0));
    assert_eq!(read(0x46), 0xA080);
    assert_eq!(read(0x79), 0x1870);
    assert_eq!(read(0x7A), 0x289A);
    detector.check().unwrap();

    detector.observe(0, 0x0010);
    assert_eq!(detector.sample(0).unwrap().voice, Some(true));
    detector.stop();
    assert!(!detector.active());
    assert!(detector.sample(0).is_none());
    assert_eq!(read(0x31) & 4, 0);
    assert_eq!(read(0x46), 0x600A);
    assert_eq!(read(0x79), 0x1234);
    assert_eq!(read(0x7A), 0x5678);

    detector.start(10, 37).unwrap();
    assert!(detector.matches(37));
    assert!(!detector.matches(128));
    assert_eq!(read(0x46), 0xA025);
    assert_eq!(read(0x79), 0x1821);
    detector.stop();
}

#[test]
fn configuration_mismatch_is_reported_without_repair() {
    seed();
    let mut detector = hardware::Detector::new();
    detector.start(0, 128).unwrap();
    write(0x46, 0xA081);
    assert_eq!(detector.check(), Err(0x7C46));
    assert_eq!(read(0x46), 0xA081);
    detector.stop();
}

#[test]
fn unknown_expiry_and_inhibited_cancellation_have_no_normal_completion() {
    let (mut gate, settings) = requested_gate();
    assert!(gate.pause(150));
    for tick in (200..1120).step_by(10) {
        assert!(gate.observe(
            tick, settings,
            Some(crate::vox::Sample { tick, voice: None, events: 0 }),
            true, false,
        ));
    }
    assert!(!gate.observe(
        1120, settings,
        Some(crate::vox::Sample { tick: 1120, voice: None, events: 0 }),
        true, false,
    ));
    assert!(!gate.completed());

    let (mut gate, settings) = requested_gate();
    assert!(!gate.observe(
        130, settings,
        Some(crate::vox::Sample {
            tick: 130, voice: Some(false), events: 0x0020,
        }),
        true, true,
    ));
    assert!(!gate.completed());
}

#[test]
fn completion_is_one_pass_and_requires_continuous_quiet_input() {
    let (mut gate, settings) = requested_gate();
    for tick in (130..1120).step_by(10) {
        assert!(gate.observe(
            tick, settings,
            Some(crate::vox::Sample { tick, voice: Some(false), events: 0x0020 }),
            true, false,
        ));
    }
    assert!(!gate.observe(
        1120, settings,
        Some(crate::vox::Sample {
            tick: 1120, voice: Some(false), events: 0x0020,
        }),
        true, false,
    ));
    assert!(gate.completed());
    assert!(!gate.observe(
        1130, settings,
        Some(crate::vox::Sample {
            tick: 1130, voice: Some(false), events: 0x0020,
        }),
        true, false,
    ));
    assert!(!gate.completed());

    let (mut gate, settings) = requested_gate();
    assert!(!gate.observe(
        1120, settings,
        Some(crate::vox::Sample {
            tick: 1120, voice: Some(false), events: 0x0020,
        }),
        true, false,
    ));
    assert!(!gate.completed());
}