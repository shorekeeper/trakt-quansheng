#![deny(warnings)]

//! Host execution of the production VOX software state machine.
//!
//! The production wrapping-clock helper is compiled directly. RF register
//! code is absent. Tests supply synthetic qualified amplitude observations;
//! they do not establish a hardware microphone source or VOX sensitivity.

mod app {
    #[path = "../../src/app/time.rs"]
    pub(crate) mod time;
}

#[path = "../src/vox.rs"]
mod vox;

#[path = "../src/voice.rs"]
mod voice;

#[path = "vox_events.rs"]
mod vox_events;

#[path = "vox_handoff.rs"]
mod vox_handoff;

#[path = "vox_disabled.rs"]
mod vox_disabled;

#[path = "voice_processing.rs"]
mod voice_processing;

#[test]
fn hardware_range_and_observation_fields_remain_explicit() {
    let observation = vox::Sample { tick: 0, voice: None, events: 0x0030 };
    assert_eq!(observation.events, 0x0030);
    assert_eq!(observation.voice, None);
    assert_eq!(vox::Settings::count(1), 2047);
    assert_eq!(vox::Settings::new().hardware_threshold(), Some(128));
    assert_eq!(vox::EventGate::new().status(0), b"VOX ARMED");
}