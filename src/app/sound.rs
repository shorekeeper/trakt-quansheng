//! Saved sound choices and bounded foreground note timelines.
//!
//! The settings word stores Roger and permit selections in separate bytes.
//! Its third byte stores boot-disable: zero means boot audio enabled.
//! The remaining byte is zero. Invalid selections and reserved values are
//! rejected. A zero settings word enables only the local boot cue.
//!
//! A note with frequency zero denotes silence. On a delayed foreground pass,
//! the next note starts at observation time; missed notes are not compressed.
//! Each hardware sound owner enforces an independent complete-sequence
//! deadline and handles cancellation through its resource-owner cleanup.
//!
//! Durations use observed SysTick increments. They are not measured audio
//! time, and a Timeline alone cannot authorize local or transmitted playback.

use crate::app::time::reached;

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub(crate) roger: u8,
    pub(crate) permit: u8,
    pub(crate) boot: bool,
}

impl Settings {
    pub(crate) const fn new() -> Self {
        Self { roger: 0, permit: 0, boot: true }
    }

    pub(crate) const fn word(self) -> u32 {
        self.roger as u32
            | ((self.permit as u32) << 8)
            | ((!self.boot as u32) << 16)
    }

    pub(crate) const fn decode(word: u32) -> Option<Self> {
        let roger = word & 255;
        let permit = (word >> 8) & 255;
        let boot_disabled = (word >> 16) & 255;

        if word >> 24 != 0 || roger > 4 || permit > 3 || boot_disabled > 1 {
            None
        } else {
            Some(Self {
                roger: roger as u8,
                permit: permit as u8,
                boot: boot_disabled == 0,
            })
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Note {
    pub(crate) hz: u16,
    pub(crate) ticks: u16,
}

const BOOT: [Note; 5] = [
    Note { hz: 700, ticks: 45 },
    Note { hz: 0, ticks: 20 },
    Note { hz: 1000, ticks: 45 },
    Note { hz: 0, ticks: 20 },
    Note { hz: 1400, ticks: 80 },
];

const TICK: [Note; 1] = [
    Note { hz: 1200, ticks: 70 },
];

const RISE: [Note; 3] = [
    Note { hz: 900, ticks: 45 },
    Note { hz: 0, ticks: 15 },
    Note { hz: 1400, ticks: 70 },
];

const PAIR: [Note; 3] = [
    Note { hz: 1200, ticks: 45 },
    Note { hz: 0, ticks: 25 },
    Note { hz: 1200, ticks: 65 },
];

const SINGLE: [Note; 1] = [
    Note { hz: 1200, ticks: 110 },
];

const FALL: [Note; 3] = [
    Note { hz: 1500, ticks: 65 },
    Note { hz: 0, ticks: 20 },
    Note { hz: 1000, ticks: 80 },
];

const TRIPLE: [Note; 5] = [
    Note { hz: 1100, ticks: 40 },
    Note { hz: 0, ticks: 20 },
    Note { hz: 1400, ticks: 40 },
    Note { hz: 0, ticks: 20 },
    Note { hz: 1100, ticks: 80 },
];

const TX_ERROR: [Note; 3] = [
    Note { hz: 400, ticks: 80 },
    Note { hz: 0, ticks: 35 },
    Note { hz: 400, ticks: 80 },
];

pub(crate) const fn tx_error() -> &'static [Note] {
    &TX_ERROR
}

pub(crate) const fn boot() -> &'static [Note] {
    &BOOT
}

pub(crate) const fn permit(selection: u8) -> &'static [Note] {
    match selection {
        1 => &TICK,
        2 => &RISE,
        3 => &PAIR,
        _ => &[],
    }
}

pub(crate) const fn roger(selection: u8) -> &'static [Note] {
    match selection {
        1 => &SINGLE,
        2 => &FALL,
        3 => &TRIPLE,
        _ => &[],
    }
}

pub(crate) enum Update {
    Wait,
    Note(u16),
    Done,
}

pub(crate) struct Timeline {
    notes: &'static [Note],
    index: usize,
    deadline: u32,
}

impl Timeline {
    pub(crate) const fn new(notes: &'static [Note], now: u32) -> Self {
        assert!(!notes.is_empty());
        assert!(notes[0].ticks != 0);
        Self {
            notes,
            index: 0,
            deadline: now.wrapping_add(notes[0].ticks as u32),
        }
    }

    pub(crate) const fn first(&self) -> u16 {
        self.notes[0].hz
    }

    pub(crate) const fn poll(&mut self, now: u32) -> Update {
        if self.index == self.notes.len() {
            return Update::Done;
        }
        if !reached(now, self.deadline) {
            return Update::Wait;
        }

        self.index += 1;
        if self.index == self.notes.len() {
            Update::Done
        } else {
            let note = self.notes[self.index];
            assert!(note.ticks != 0);
            self.deadline = now.wrapping_add(note.ticks as u32);
            Update::Note(note.hz)
        }
    }
}

const _: () = {
    assert!(Settings::new().word() == 0);
    assert!(matches!(Settings::decode(0), Some(Settings {
        roger: 0, permit: 0, boot: true,
    })));
    assert!(matches!(Settings::decode(0x010304), Some(Settings {
        roger: 4, permit: 3, boot: false,
    })));
    assert!(Settings::decode(5).is_none());
    assert!(Settings::decode(0x0400).is_none());
    assert!(Settings::decode(0x020000).is_none());
    assert!(Settings::decode(0x01000000).is_none());

    let mut selection = 0;
    while selection <= 4 {
        let mut permit_selection = 0;
        while permit_selection <= 3 {
            let value = Settings {
                roger: selection,
                permit: permit_selection,
                boot: false,
            };
            let decoded = match Settings::decode(value.word()) {
                Some(decoded) => decoded,
                None => panic!("audio settings rejected"),
            };
            assert!(decoded.roger == selection);
            assert!(decoded.permit == permit_selection);
            assert!(!decoded.boot);
            permit_selection += 1;
        }
        selection += 1;
    }

    let mut timeline = Timeline::new(&FALL, 0);
    assert!(timeline.first() == 1500);
    assert!(matches!(timeline.poll(64), Update::Wait));
    assert!(matches!(timeline.poll(65), Update::Note(0)));
    assert!(matches!(timeline.poll(84), Update::Wait));
    assert!(matches!(timeline.poll(85), Update::Note(1000)));
    assert!(matches!(timeline.poll(164), Update::Wait));
    assert!(matches!(timeline.poll(165), Update::Done));
    assert!(matches!(timeline.poll(200), Update::Done));

    let mut wrapped = Timeline::new(&TICK, u32::MAX - 20);
    assert!(matches!(wrapped.poll(48), Update::Wait));
    assert!(matches!(wrapped.poll(49), Update::Done));

    let mut delayed = Timeline::new(&FALL, 0);
    assert!(matches!(delayed.poll(100), Update::Note(0)));
    assert!(matches!(delayed.poll(119), Update::Wait));
    assert!(matches!(delayed.poll(120), Update::Note(1000)));
    assert!(permit(0).is_empty());
    assert!(roger(0).is_empty());
    assert!(roger(4).is_empty());
};