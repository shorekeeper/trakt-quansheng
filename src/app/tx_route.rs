//! Detached VFO transmit-route editor.
//!
//! Mode, offset, and split candidates remain local until Apply.
//! A value field accepts the sixth kHz digit automatically after route
//! validation. This closes the field, not the complete draft. F removes the
//! last digit; a digit after six rejected digits starts a fresh candidate.
//! EXIT discards the current field, then the entire draft.
//! MENU repeats never cycle modes, accept values, or apply the draft.
//!
//! Offsets are positive ten-hertz values represented by Tx; this editor's
//! numeric input uses whole kHz. Existing finer values survive if not edited.
//! Apply checks the effective TX frequency against the captured RX frequency.
//! RX-only deliberately resolves to no TX frequency.
//!
//! The panel keeps its original requested RX state but masks receiver
//! admission while this screen is open. Closing the editor resumes ordinary
//! receive dispatch. This module never accesses RF or writes storage.

use crate::drivers::keyboard::{Edge, Event, Key};
use crate::profile::{Band, Tx};
use crate::ui::{font::Size, widgets, Frame};

pub(super) enum Effect {
    Cancel,
    Refuse,
    Apply(Tx),
}

pub(super) struct Editor {
    receive: u32,
    mode: u8,
    offset: u32,
    split: u32,
    selected: usize,
    editing: bool,
    input: u32,
    digits: usize,
    invalid: bool,
}

const fn digit(key: Key) -> Option<u8> {
    match key {
        Key::Zero => Some(0), Key::One => Some(1), Key::Two => Some(2),
        Key::Three => Some(3), Key::Four => Some(4), Key::Five => Some(5),
        Key::Six => Some(6), Key::Seven => Some(7), Key::Eight => Some(8),
        Key::Nine => Some(9), _ => None,
    }
}

impl Editor {
    pub(super) const fn new(receive: u32, route: Tx) -> Self {
        assert!(Band::receive(receive).is_some() && route.word().is_ok());

        let default_offset = if receive < 400_000_000 { 600_000 } else { 5_000_000 };
        let (mode, offset, split) = match route {
            Tx::Simplex => (0, default_offset, receive),
            Tx::Plus(value) => (1, value, receive),
            Tx::Minus(value) => (2, value, receive),
            Tx::Split(value) => (3, default_offset, value),
            Tx::ReceiveOnly => (4, default_offset, receive),
        };

        Self {
            receive, mode, offset, split,
            selected: 0, editing: false, input: 0, digits: 0, invalid: false,
        }
    }

    const fn route(&self) -> Tx {
        match self.mode {
            0 => Tx::Simplex,
            1 => Tx::Plus(self.offset),
            2 => Tx::Minus(self.offset),
            3 => Tx::Split(self.split),
            _ => Tx::ReceiveOnly,
        }
    }

    /// Resolve the candidate through the existing TX route rules. Automatic
    /// field completion cannot use receive-only extensions as TX frequencies.
    const fn accept_value(&mut self) -> Option<Effect> {
        let value = self.input * 1000;
        let candidate = match self.mode {
            1 => Tx::Plus(value),
            2 => Tx::Minus(value),
            3 => Tx::Split(value),
            _ => {
                self.invalid = true;
                return Some(Effect::Refuse);
            }
        };

        if self.digits != 6 || candidate.frequency(self.receive).is_err() {
            self.invalid = true;
            return Some(Effect::Refuse);
        }

        if self.mode == 3 {
            self.split = value;
        } else {
            self.offset = value;
        }
        self.editing = false;
        self.invalid = false;
        None
    }

    pub(super) const fn key(&mut self, event: Event) -> Option<Effect> {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return None;
        }
        let down = matches!(event.edge, Edge::Down);

        if matches!(event.key, Key::Exit) && down {
            if self.editing {
                self.editing = false;
                self.invalid = false;
            } else {
                return Some(Effect::Cancel);
            }
            return None;
        }

        if self.editing {
            if !down { return None; }
            self.invalid = false;

            if let Some(value) = digit(event.key) {
                if self.digits == 6 {
                    self.input = 0;
                    self.digits = 0;
                }
                self.input = self.input * 10 + value as u32;
                self.digits += 1;
                if self.digits == 6 {
                    return self.accept_value();
                }
            } else if matches!(event.key, Key::F) {
                if self.digits != 0 {
                    self.input /= 10;
                    self.digits -= 1;
                }
            } else if matches!(event.key, Key::Menu) {
                return self.accept_value();
            }
            return None;
        }

        if matches!(event.key, Key::Up | Key::Down) {
            self.selected = if matches!(event.key, Key::Up) {
                (self.selected + 2) % 3
            } else {
                (self.selected + 1) % 3
            };
            self.invalid = false;
        } else if matches!(event.key, Key::Menu) && down {
            self.invalid = false;
            match self.selected {
                0 => self.mode = (self.mode + 1) % 5,
                1 if self.mode >= 1 && self.mode <= 3 => {
                    self.editing = true;
                    self.input = 0;
                    self.digits = 0;
                }
                2 => {
                    let route = self.route();
                    if route.frequency(self.receive).is_ok() {
                        return Some(Effect::Apply(route));
                    }
                    self.invalid = true;
                    return Some(Effect::Refuse);
                }
                _ => {}
            }
        }

        None
    }

    fn preview(&self) -> [u8; 7] {
        let mut text = [b'-', b'-', b'-', b'.', b'-', b'-', b'-'];
        let mut value = self.input;
        for index in (0..self.digits).rev() {
            let position = if index < 3 { index } else { index + 1 };
            text[position] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        text
    }

    pub(super) fn render(&self, frame: &mut Frame) {
        frame.clear();
        frame.text(2, 0, b"TX ROUTE");
        frame.fill(0, 8, 128, 1, true);

        if self.editing {
            frame.text_size(2, 12, if self.mode == 3 {
                b"TX FREQUENCY / SIX KHZ DIGITS"
            } else {
                b"OFFSET / SIX KHZ DIGITS"
            }, Size::Small);
            frame.text_size(10, 24, &self.preview(), Size::Large);
            frame.text_size(2, 46, if self.invalid {
                if self.digits < 6 {
                    b"REFUSE: SIX DIGITS REQUIRED"
                } else {
                    b"REFUSE: TX VALUE OR RANGE"
                }
            } else {
                b"SIX KHZ DIGITS / AUTO ACCEPT"
            }, Size::Small);
            frame.text_size(2, 58, if self.invalid && self.digits == 6 {
                b"F DELETE / DIGIT RESTART / EXIT"
            } else {
                b"F DELETE / EXIT CANCEL"
            }, Size::Small);
            return;
        }

        frame.text_size(2, 11, b"RX", Size::Small);
        widgets::frequency(frame, 18, 11, self.receive, Size::Small);
        frame.text_size(2, 20, b"TX", Size::Small);

        match self.route().frequency(self.receive) {
            Ok(Some(frequency)) => widgets::frequency(
                frame, 18, 20, frequency, Size::Small,
            ),
            Ok(None) => frame.text_size(18, 20, b"DISABLED", Size::Small),
            Err(_) => frame.text_size(18, 20, b"OUT OF BAND", Size::Small),
        }

        frame.text_size(3, 31, b"Mode", Size::Small);
        frame.text_size(61, 31, self.route().name(), Size::Small);
        frame.text_size(3, 40, if self.mode == 3 { b"TX freq" } else { b"Offset" }, Size::Small);

        if self.mode >= 1 && self.mode <= 3 {
            widgets::frequency(
                frame, 61, 40,
                if self.mode == 3 { self.split } else { self.offset },
                Size::Small,
            );
        } else {
            frame.text_size(61, 40, b"--", Size::Small);
        }

        frame.text_size(3, 49, b"Apply RAM", Size::Small);
        frame.invert(0, 30 + self.selected as i16 * 9, 128, 8);
        frame.text_size(2, 58, if self.invalid {
            b"REFUSE: TX OUT OF BAND"
        } else {
            b"MENU SELECT / EXIT CANCEL"
        }, Size::Small);
    }
}

const fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

const _: () = {
    let mut editor = Editor::new(145_700_000, Tx::Simplex);
    let repeat = Event { key: Key::Menu, edge: Edge::Repeat };
    assert!(editor.key(repeat).is_none());
    assert!(editor.mode == 0);
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.mode == 1);
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.mode == 2);

    editor.selected = 1;
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.editing);

    let digits = [Key::Zero, Key::Zero, Key::Zero, Key::Six, Key::Zero, Key::Zero];
    let mut index = 0;
    while index < digits.len() {
        assert!(editor.key(press(digits[index])).is_none());
        index += 1;
    }
    assert!(!editor.editing && editor.offset == 600_000);
    editor.selected = 2;
    assert!(matches!(editor.key(press(Key::Menu)), Some(Effect::Apply(Tx::Minus(600_000)))));

    let mut blocked = Editor::new(174_000_000, Tx::Plus(600_000));
    blocked.selected = 2;
    assert!(matches!(blocked.key(press(Key::Menu)), Some(Effect::Refuse)));
    assert!(blocked.invalid);
    blocked.mode = 4;
    assert!(matches!(blocked.key(press(Key::Menu)), Some(Effect::Apply(Tx::ReceiveOnly))));

    let mut cancelled = Editor::new(433_700_000, Tx::Split(145_700_000));
    cancelled.selected = 1;
    assert!(cancelled.key(press(Key::Menu)).is_none());
    assert!(cancelled.key(press(Key::Nine)).is_none());
    assert!(cancelled.key(press(Key::Exit)).is_none());
    assert!(cancelled.split == 145_700_000);
    assert!(matches!(cancelled.key(press(Key::Exit)), Some(Effect::Cancel)));

    let mut offset = Editor::new(145_500_000, Tx::Plus(600_000));
    offset.selected = 1;
    assert!(offset.key(press(Key::Menu)).is_none());
    let digits = [
        Key::Zero, Key::Zero, Key::Zero,
        Key::Zero, Key::Zero, Key::Four,
    ];
    index = 0;
    while index < digits.len() {
        assert!(offset.key(press(digits[index])).is_none());
        if index < 5 {
            assert!(offset.editing && offset.offset == 600_000);
        }
        index += 1;
    }
    assert!(!offset.editing && offset.offset == 4000);
    assert!(matches!(offset.route(), Tx::Plus(4000)));
    offset.selected = 2;
    assert!(matches!(
        offset.key(press(Key::Menu)),
        Some(Effect::Apply(Tx::Plus(4000))),
    ));

    let mut split = Editor::new(145_500_000, Tx::Split(433_500_000));
    split.selected = 1;
    assert!(split.key(press(Key::Menu)).is_none());
    let digits = [
        Key::Five, Key::Zero, Key::Zero,
        Key::Zero, Key::Zero, Key::Zero,
    ];
    index = 0;
    while index < digits.len() {
        let effect = split.key(press(digits[index]));
        if index == 5 {
            assert!(matches!(effect, Some(Effect::Refuse)));
        } else {
            assert!(effect.is_none());
        }
        index += 1;
    }
    assert!(split.editing && split.invalid);
    assert!(split.split == 433_500_000);
    assert!(split.key(press(Key::Four)).is_none());
    assert!(split.digits == 1 && split.input == 4);
    assert!(split.key(press(Key::F)).is_none());
    assert!(split.digits == 0);
    assert!(matches!(split.key(press(Key::Menu)), Some(Effect::Refuse)));
    assert!(split.key(press(Key::Exit)).is_none());
    assert!(split.split == 433_500_000);
};