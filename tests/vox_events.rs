//! Host checks of the production comparator-event request state machine.
//!
//! Inputs are synthetic hardware-state observations. These tests establish
//! software deadlines and cancellation, not comparator polarity on a radio.

use crate::vox::{EventGate, Sample, Settings};

fn sample(tick: u32, voice: Option<bool>) -> Sample {
    Sample { tick, voice, events: 0 }
}

fn settings(hang: u8) -> Settings {
    Settings { enabled: true, hang, ..Settings::new() }
}

fn quiet(gate: &mut EventGate, settings: Settings, start: u32) {
    for delta in (0..=100).step_by(10) {
        let tick = start.wrapping_add(delta);
        assert!(!gate.observe(
            tick, settings, Some(sample(tick, Some(false))), true, false,
        ));
    }
}

fn open(gate: &mut EventGate, settings: Settings, tick: u32) {
    assert!(!gate.observe(
        tick, settings, Some(sample(tick, Some(true))), true, false,
    ));
    let next = tick.wrapping_add(10);
    assert!(gate.observe(
        next, settings, Some(sample(next, Some(true))), true, false,
    ));
}

#[test]
fn first_voice_opens_without_initial_quiet_but_unknown_cannot_open() {
    let settings = settings(1);
    let mut gate = EventGate::new();
    for tick in (0..=100).step_by(10) {
        assert!(!gate.observe(
            tick, settings, Some(sample(tick, None)), true, false,
        ));
    }
    open(&mut gate, settings, 110);

    // Failure locks continued voice. One fresh known quiet observation
    // releases the lock without another hundred-tick silence dwell.
    gate.cancel();
    for tick in (130..=200).step_by(10) {
        assert!(!gate.observe(
            tick, settings, Some(sample(tick, Some(true))), true, false,
        ));
    }
    assert!(!gate.observe(
        210, settings, Some(sample(210, Some(false))), true, false,
    ));
    open(&mut gate, settings, 220);

    // Startup polling without a sample must not introduce a quiet lock.
    let mut idle = EventGate::new();
    assert!(!idle.observe(0, settings, None, true, false));
    assert!(!idle.observe(10, settings, None, true, false));
    open(&mut idle, settings, 20);
}

#[test]
fn all_hold_choices_follow_last_voice_not_repeated_quiet() {
    for hang in 1..=10 {
        let settings = settings(hang);
        let mut gate = EventGate::new();
        quiet(&mut gate, settings, 0);
        open(&mut gate, settings, 110);
        let until = 120 + hang as u32 * 1000;
        for tick in (130..until).step_by(10) {
            assert!(gate.observe(
                tick, settings, Some(sample(tick, Some(false))), true, false,
            ));
        }
        assert!(!gate.observe(
            until, settings, Some(sample(until, Some(false))), true, false,
        ));
        assert!(!gate.requested(until));
    }
}

#[test]
fn setup_handoff_cannot_renew_voice_or_extend_hold() {
    let settings = settings(1);
    let mut gate = EventGate::new();
    quiet(&mut gate, settings, 0);
    open(&mut gate, settings, 110);
    for tick in 121..200 {
        assert!(gate.pause(tick));
    }
    for tick in (200..1120).step_by(10) {
        assert!(gate.observe(
            tick, settings, Some(sample(tick, None)), true, false,
        ));
    }
    assert!(!gate.observe(
        1120, settings, Some(sample(1120, None)), true, false,
    ));
}

#[test]
fn actual_tx_voice_event_can_renew_the_original_hold() {
    let settings = settings(1);
    let mut gate = EventGate::new();
    quiet(&mut gate, settings, 0);
    open(&mut gate, settings, 110);
    assert!(gate.pause(130));
    assert!(gate.observe(
        200, settings, Some(sample(200, Some(true))), true, false,
    ));
    for tick in (210..1200).step_by(10) {
        assert!(gate.observe(
            tick, settings, Some(sample(tick, Some(false))), true, false,
        ));
    }
    assert!(!gate.observe(
        1200, settings, Some(sample(1200, Some(false))), true, false,
    ));
}

#[test]
fn stale_input_and_physical_inhibition_require_new_quiet() {
    let settings = settings(10);
    for kind in 0..4 {
        let mut gate = EventGate::new();
        quiet(&mut gate, settings, 0);
        open(&mut gate, settings, 110);
        let (tick, input, eligible, inhibited) = match kind {
            0 => (130, None, true, false),
            1 => (141, Some(sample(141, Some(true))), true, false),
            2 => (130, Some(sample(130, Some(true))), false, false),
            _ => (130, Some(sample(130, Some(true))), true, true),
        };
        assert!(!gate.observe(tick, settings, input, eligible, inhibited));
        for next in (tick + 10..tick + 110).step_by(10) {
            assert!(!gate.observe(
                next, settings, Some(sample(next, Some(true))), true, false,
            ));
        }
    }
}

#[test]
fn completion_cannot_restart_held_voice_after_timeout() {
    let settings = settings(10);
    let mut gate = EventGate::new();
    quiet(&mut gate, settings, 0);
    open(&mut gate, settings, 110);
    gate.cancel();
    for tick in (130..1000).step_by(10) {
        assert!(!gate.observe(
            tick, settings, Some(sample(tick, Some(true))), true, false,
        ));
    }
    quiet(&mut gate, settings, 1000);
    open(&mut gate, settings, 1110);
}

#[test]
fn duplicate_observation_cannot_qualify_or_complete_attack() {
    let settings = settings(1);
    let mut gate = EventGate::new();
    quiet(&mut gate, settings, 0);
    for now in 110..=130 {
        assert!(!gate.observe(
            now, settings, Some(sample(110, Some(true))), true, false,
        ));
    }
    assert!(gate.observe(
        130, settings, Some(sample(130, Some(true))), true, false,
    ));
}

#[test]
fn unsupported_saved_threshold_never_reaches_hardware_request() {
    let mut settings = settings(1);
    settings.threshold = 2048;
    assert!(Settings::decode(settings.word()).is_some());
    assert!(settings.hardware_threshold().is_none());
    let mut gate = EventGate::new();
    for tick in (0..200).step_by(10) {
        assert!(!gate.observe(
            tick, settings, Some(sample(tick, Some(true))), true, false,
        ));
    }
}

#[test]
fn event_deadlines_cross_tick_wrap() {
    let settings = settings(1);
    let start = u32::MAX - 50;
    let mut gate = EventGate::new();
    quiet(&mut gate, settings, start);
    open(&mut gate, settings, start.wrapping_add(110));
    for delta in (130..1120).step_by(10) {
        let tick = start.wrapping_add(delta);
        assert!(gate.observe(
            tick, settings, Some(sample(tick, Some(false))), true, false,
        ));
    }
    let end = start.wrapping_add(1120);
    assert!(!gate.observe(
        end, settings, Some(sample(end, Some(false))), true, false,
    ));
}