//! Exclusive BK4829 tone ownership for local cues and transmitted tails.
//!
//! Local audio enables AF DAC and DSP without PLL, RX links, MIC ADC, or PA.
//! Its exact control tuple is verified before the speaker is enabled.
//! Completion restores the audio gain and tone frequency, then parks RF.
//!
//! Tone tails run only within an already admitted transmitter. The caller
//! disables MIC ADC first; synthetic tones must not mix with live speech.
//! Tail completion and cancellation disable both generators and FSK.
//!
//! The MDC option is the reference port's fixed FSK Roger pattern. It has
//! no configurable unit identifier and is not a general MDC1200 encoder.
//! The hardware clocks its payload; foreground does not schedule symbols.
//! Its fixed wait bounds emission time but does not prove successful decode.
//!
//! Frequency words use the BK4829 port's rounded oscillator conversion.
//! No audio-register readback is interpreted as proof of mute.

use super::{inhibit, read, verify, verify_transmit, write};
use crate::app::{
    sound::{self, Note, Timeline, Update},
    time::reached,
};
use crate::mcu;
use crate::startup;

const LOCAL: [u16; 5] = [0x0302, 0x9000, 0, 0x9D1F, 0x6342];

const fn frequency_word(hz: u16) -> u16 {
    assert!(hz <= 3000);
    (((hz as u32 * 1_353_245) + 65_536) >> 17) as u16
}

fn amplifier(on: bool) {
    unsafe {
        mcu::write(mcu::GPIO_A + 0x18, if on { 1 << 8 } else { 1 << 24 });
    }
}

fn tone(hz: u16, gain: u16) {
    write(0x70, 0);
    if hz != 0 {
        write(0x71, frequency_word(hz));
        write(0x70, 0x8000 | (gain << 8));
    }
}

#[derive(Clone, Copy)]
enum LocalPhase {
    Idle,
    DspSettle,
    SpeakerSettle,
    Playing,
    MuteSettle,
    SpeakerOff,
}

const DSP_SETTLE: u32 = 10;
const SPEAKER_SETTLE: u32 = 60;
const MUTE_SETTLE: u32 = 5;
const SPEAKER_OFF_SETTLE: u32 = 5;

pub(crate) struct Local {
    phase: LocalPhase,
    timeline: Option<Timeline>,
    notes: &'static [Note],
    wake_at: u32,
    deadline: u32,
    gain_register: u16,
    frequency_register: u16,
    gain: u16,
}

impl Local {
    pub(crate) const fn new() -> Self {
        Self {
            phase: LocalPhase::Idle,
            timeline: None,
            notes: &[],
            wake_at: 0,
            deadline: 0,
            gain_register: 0,
            frequency_register: 0,
            gain: 28,
        }
    }

    pub(crate) fn active(&self) -> bool {
        !matches!(self.phase, LocalPhase::Idle)
    }

    pub(crate) fn controls(&self) -> Option<[u16; 5]> {
        if self.active() {
            Some(LOCAL)
        } else {
            None
        }
    }

    pub(crate) fn start(
        &mut self,
        now: u32,
        notes: &'static [Note],
    ) -> Result<(), u32> {
        self.start_gain(now, notes, 28)
    }

    /// Prepare a muted local path before enabling the speaker. The note
    /// timeline starts only after speaker settling, so its duration does
    /// not include amplifier startup.
    ///
    /// The caller must exclusively own the parked RF transport. All waits
    /// use foreground deadlines; no delay loop blocks input or watchdog work.
    pub(crate) fn start_gain(
        &mut self,
        now: u32,
        notes: &'static [Note],
        gain: u16,
    ) -> Result<(), u32> {
        if self.active() || notes.is_empty() || gain > 127 {
            return Err(0x7501);
        }

        self.gain_register = read(0x48);
        self.frequency_register = read(0x71);

        amplifier(false);
        unsafe {
            inhibit();
        }

        write(0x50, 0xBB18);
        write(0x70, 0);
        write(0x37, 0x9D1F);
        write(0x48, 0x33A8);
        write(0x47, 0x6342);
        write(0x30, 0x0302);

        if let Err(error) = verify_transmit(LOCAL) {
            self.park();
            return Err(error);
        }

        self.notes = notes;
        self.gain = gain;
        self.timeline = None;
        self.phase = LocalPhase::DspSettle;
        self.wake_at = now.wrapping_add(DSP_SETTLE);
        self.deadline = now.wrapping_add(600);

        Ok(())
    }

    fn park(&mut self) {
        // Cancellation removes speaker output immediately. Ordinary
        // completion reaches this after the separate mute/off delays.
        amplifier(false);
        write(0x50, 0xBB18);
        write(0x70, 0);
        write(0x30, 0);
        write(0x47, 0x6042);
        write(0x48, self.gain_register);
        write(0x71, self.frequency_register);

        self.timeline = None;
        self.notes = &[];
        self.phase = LocalPhase::Idle;

        if let Err(error) = verify() {
            startup::stop(error);
        }
    }

    /// PTT and cancellation do not wait for the completion ramp.
    pub(crate) fn cancel(&mut self) {
        if self.active() {
            self.park();
        }
    }

    pub(crate) fn pump(&mut self, now: u32) -> Result<(), u32> {
        if !self.active() {
            return Ok(());
        }

        if reached(now, self.deadline) {
            self.park();
            return Err(0x7502);
        }

        match self.phase {
            LocalPhase::DspSettle => {
                if reached(now, self.wake_at) {
                    // Keep both the generator and AF input silent while
                    // the external amplifier reaches its operating state.
                    amplifier(true);
                    self.phase = LocalPhase::SpeakerSettle;
                    self.wake_at = now.wrapping_add(SPEAKER_SETTLE);
                }
            }

            LocalPhase::SpeakerSettle => {
                if reached(now, self.wake_at) {
                    let timeline = Timeline::new(self.notes, now);

                    tone(timeline.first(), self.gain);
                    write(0x50, 0x3B18);

                    self.timeline = Some(timeline);
                    self.phase = LocalPhase::Playing;
                }
            }

            LocalPhase::Playing => {
                let update = self.timeline.as_mut().unwrap().poll(now);

                match update {
                    Update::Wait => {}

                    Update::Note(hz) => {
                        tone(hz, self.gain);
                    }

                    Update::Done => {
                        write(0x50, 0xBB18);
                        write(0x70, 0);

                        self.timeline = None;
                        self.phase = LocalPhase::MuteSettle;
                        self.wake_at = now.wrapping_add(MUTE_SETTLE);
                    }
                }
            }

            LocalPhase::MuteSettle => {
                if reached(now, self.wake_at) {
                    amplifier(false);
                    self.phase = LocalPhase::SpeakerOff;
                    self.wake_at = now.wrapping_add(SPEAKER_OFF_SETTLE);
                }
            }

            LocalPhase::SpeakerOff => {
                if reached(now, self.wake_at) {
                    self.park();
                }
            }

            LocalPhase::Idle => {}
        }

        Ok(())
    }
}

// Fixed FSK Roger wire profile from the reference BK4829 port.
const MDC_SETUP: [(u8, u16); 9] = [
    (0x58, 0x37C3),
    (0x72, 0x3065),
    (0x70, 0x00C3),
    (0x5D, 0x0D00),
    (0x59, 0x8068),
    (0x59, 0x0068),
    (0x5A, 0x5555),
    (0x5B, 0x5555),
    (0x5C, 0xAA30),
];

const MDC_WORDS: [u16; 7] = [
    0xF1A2, 0x7446, 0x61A4, 0x6544, 0x4E8A, 0xE044, 0xEA84,
];

enum TailPhase {
    Idle,
    Tone(Timeline),
    MdcSetup(usize),
    MdcWait(u32),
    MdcTransmit(u32),
}

pub(crate) struct Tail {
    phase: TailPhase,
    deadline: u32,
    saved: [u16; 3],
}

impl Tail {
    pub(crate) const fn new() -> Self {
        Self {
            phase: TailPhase::Idle,
            deadline: 0,
            saved: [0; 3],
        }
    }

    /// Requires a live, checked transmitter with MIC ADC disabled.
    /// The caller retains its admitted PA profile and original TX deadline;
    /// this sound owner neither selects power nor extends that deadline.
    pub(crate) fn start(&mut self, now: u32, selection: u8) {
        assert!(selection >= 1 && selection <= 4);
        assert!(matches!(self.phase, TailPhase::Idle));

        self.saved = [read(0x71), read(0x72), read(0x5D)];
        self.deadline = now.wrapping_add(600);
        write(0x50, 0xBB18);
        write(0x70, 0);
        write(0x58, 0);
        write(0x59, 0);

        if selection == 4 {
            self.phase = TailPhase::MdcSetup(0);
        } else {
            let timeline = Timeline::new(sound::roger(selection), now);
            tone(timeline.first(), 32);
            write(0x50, 0x3B18);
            self.phase = TailPhase::Tone(timeline);
        }
    }

    /// Cancellation does not wait for FSK or a note deadline.
    /// The RF owner must remove PA drive before invoking this cleanup.
    pub(crate) fn cancel(&mut self) {
        if matches!(self.phase, TailPhase::Idle) { return; }

        write(0x70, 0);
        write(0x59, 0);
        write(0x58, 0);
        write(0x50, 0xBB18);
        write(0x71, self.saved[0]);
        write(0x72, self.saved[1]);
        write(0x5D, self.saved[2]);
        self.phase = TailPhase::Idle;
    }

    /// True ends the tail; the caller immediately disables PA and RF.
    /// A missed deadline truncates the sound instead of extending RF time.
    pub(crate) fn pump(&mut self, now: u32) -> bool {
        if reached(now, self.deadline) {
            return true;
        }

        match &mut self.phase {
            TailPhase::Idle => true,
            TailPhase::Tone(timeline) => match timeline.poll(now) {
                Update::Wait => false,
                Update::Note(hz) => {
                    tone(hz, 32);
                    false
                }
                Update::Done => true,
            },
            TailPhase::MdcSetup(index) => {
                if *index < MDC_SETUP.len() {
                    let (register, value) = MDC_SETUP[*index];
                    write(register, value);
                } else {
                    write(0x5F, MDC_WORDS[*index - MDC_SETUP.len()]);
                }

                *index += 1;
                if *index == MDC_SETUP.len() + MDC_WORDS.len() {
                    self.phase = TailPhase::MdcWait(now.wrapping_add(20));
                }
                false
            }
            TailPhase::MdcWait(deadline) => {
                if reached(now, *deadline) {
                    write(0x50, 0x3B18);
                    write(0x59, 0x0868);
                    self.phase = TailPhase::MdcTransmit(now.wrapping_add(180));
                }
                false
            }
            TailPhase::MdcTransmit(deadline) => reached(now, *deadline),
        }
    }
}

const _: () = {
    assert!(frequency_word(0) == 0);
    assert!(frequency_word(1000) == 10324);
    assert!(frequency_word(1200) == 12389);
    assert!(LOCAL[0] & 0x00FC == 0);
    assert!(LOCAL[1] == 0x9000 && LOCAL[2] == 0);
    assert!(MDC_SETUP.len() == 9);
    assert!(MDC_WORDS.len() == 7);
    assert!(MDC_SETUP[0].0 == 0x58);
    assert!(MDC_SETUP[3].0 == 0x5D);
};