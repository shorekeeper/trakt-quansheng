//! Detached signaling editor shared by RX/TX, VFOs, and channels.
//!
//! Type selects Off, CTCSS, DCS normal, or DCS inverted. Value opens a
//! candidate subview; Apply returns one checked selection. EXIT discards
//! a value edit first, then the complete draft. No key repeat confirms.
//!
//! In the value subview, arrows cycle admitted values. DCS also accepts
//! exactly three octal digits; F deletes a digit. Digits 8 and 9 cannot
//! enter a DCS code. Partial or uncatalogued input cannot be accepted.
//! CTCSS values use the checked frequency catalog rather than free input.
//!
//! Type changes retain the detached CTCSS and DCS candidates, but only one
//! encoded selection can be applied. No event writes settings or RF.

use crate::drivers::keyboard::{Edge, Event, Key};
use crate::{ctcss, dcs};
use crate::ui::{font::Size, Frame};

pub(super) enum Effect {
    Cancel,
    Apply(u16),
}

pub(super) struct Editor {
    kind: u8,
    tone: u16,
    code: u16,
    selected: usize,
    editing: bool,
    candidate: u16,
    input: u16,
    digits: usize,
    invalid: bool,
}

const fn digit(key: Key) -> Option<u16> {
    match key {
        Key::Zero => Some(0), Key::One => Some(1), Key::Two => Some(2),
        Key::Three => Some(3), Key::Four => Some(4), Key::Five => Some(5),
        Key::Six => Some(6), Key::Seven => Some(7), Key::Eight => Some(8),
        Key::Nine => Some(9), _ => None,
    }
}

impl Editor {
    pub(super) const fn new(value: u16) -> Self {
        assert!(crate::tx_signal::valid(value as u32));

        let (kind, tone, code) = match dcs::decode(value) {
            Some((code, inverted)) => (if inverted { 3 } else { 2 }, 670, code),
            None if value != 0 => (1, value, 0o023),
            None => (0, 670, 0o023),
        };

        Self {
            kind, tone, code, selected: 0, editing: false,
            candidate: 0, input: 0, digits: 0, invalid: false,
        }
    }

    const fn encoded(&self, value: u16) -> u16 {
        match self.kind {
            0 => 0,
            1 => value,
            _ => match dcs::encode(value, self.kind == 3) {
                Some(value) => value,
                None => panic!("invalid owned DCS draft"),
            },
        }
    }

    const fn selection(&self) -> u16 {
        self.encoded(if self.kind == 1 { self.tone } else { self.code })
    }

    pub(super) const fn key(&mut self, event: Event) -> Option<Effect> {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) { return None; }
        let down = matches!(event.edge, Edge::Down);

        if down && matches!(event.key, Key::Exit) {
            if self.editing {
                self.editing = false;
                self.invalid = false;
            } else {
                return Some(Effect::Cancel);
            }
            return None;
        }

        if self.editing {
            if matches!(event.key, Key::Up | Key::Down) {
                let up = matches!(event.key, Key::Up);
                self.digits = 0;
                self.input = 0;
                self.invalid = false;

                if self.kind == 1 {
                    let current = match ctcss::selection(self.candidate) {
                        Some(value) => value,
                        None => panic!("invalid CTCSS candidate"),
                    };
                    let count = ctcss::count() - 1;
                    let next = if up {
                        current % count + 1
                    } else {
                        (current + count - 2) % count + 1
                    };
                    self.candidate = match ctcss::at(next) {
                        Some(value) => value,
                        None => panic!("CTCSS candidate index rejected"),
                    };
                } else {
                    let current = match dcs::index(self.candidate) {
                        Some(value) => value,
                        None => panic!("invalid DCS candidate"),
                    };
                    let count = dcs::count();
                    let next = if up {
                        (current + 1) % count
                    } else {
                        (current + count - 1) % count
                    };
                    self.candidate = dcs::at(next);
                }
                return None;
            }

            if !down { return None; }

            if self.kind >= 2 {
                if let Some(value) = digit(event.key) {
                    if value > 7 || self.digits == 3 {
                        self.invalid = true;
                    } else {
                        self.input = self.input * 8 + value;
                        self.digits += 1;
                        self.invalid = false;
                    }
                    return None;
                }
                if matches!(event.key, Key::F) {
                    if self.digits != 0 {
                        self.input /= 8;
                        self.digits -= 1;
                    }
                    self.invalid = false;
                    return None;
                }
            }

            if matches!(event.key, Key::Menu) {
                if self.digits != 0 {
                    if self.digits != 3 || dcs::index(self.input).is_none() {
                        self.invalid = true;
                        return None;
                    }
                    self.candidate = self.input;
                }

                if self.kind == 1 { self.tone = self.candidate; }
                else { self.code = self.candidate; }
                self.editing = false;
                self.invalid = false;
            }
            return None;
        }

        if matches!(event.key, Key::Up | Key::Down) {
            self.selected = if matches!(event.key, Key::Up) {
                (self.selected + 2) % 3
            } else {
                (self.selected + 1) % 3
            };
        } else if down && matches!(event.key, Key::Menu) {
            match self.selected {
                0 => self.kind = (self.kind + 1) % 4,
                1 if self.kind != 0 => {
                    self.candidate = if self.kind == 1 { self.tone } else { self.code };
                    self.editing = true;
                    self.input = 0;
                    self.digits = 0;
                    self.invalid = false;
                }
                2 => return Some(Effect::Apply(self.selection())),
                _ => {}
            }
        }

        None
    }

    fn preview(&self) -> [u8; 5] {
        if self.digits == 0 {
            return crate::tx_signal::text(self.encoded(self.candidate));
        }

        let mut text = [b'D', b'-', b'-', b'-', if self.kind == 3 { b'I' } else { b'N' }];
        let mut value = self.input;
        for index in (0..self.digits).rev() {
            text[1 + index] = b'0' + (value & 7) as u8;
            value >>= 3;
        }
        text
    }

    pub(super) fn render(&self, frame: &mut Frame, receive: bool) {
        frame.clear();
        frame.text(2, 0, if receive { b"RX SIGNAL" } else { b"TX SIGNAL" });
        frame.fill(0, 8, 128, 1, true);

        if self.editing {
            frame.text_size(20, 22, &self.preview(), Size::Large);
            frame.text_size(2, 44, if self.invalid {
                b"INVALID OR INCOMPLETE CODE"
            } else if self.kind >= 2 {
                b"ARROWS OR THREE OCTAL DIGITS"
            } else {
                b"UP/DOWN SELECT FREQUENCY"
            }, Size::Small);
            frame.text_size(2, 51, if self.kind >= 2 {
                b"F DELETE / DIGITS 0 THROUGH 7"
            } else {
                b"TENTHS OF HZ"
            }, Size::Small);
            frame.text_size(2, 58, b"MENU ACCEPT / EXIT CANCEL", Size::Small);
            return;
        }

        let kind: &[u8] = match self.kind {
            0 => b"OFF",
            1 => b"CTCSS",
            2 => b"DCS N",
            _ => b"DCS I",
        };
        frame.text_size(3, 16, b"Type", Size::Small);
        frame.text_size(70, 16, kind, Size::Small);
        frame.text_size(3, 28, b"Value", Size::Small);
        frame.text_size(70, 28, &crate::tx_signal::text(self.selection()), Size::Small);
        frame.text_size(3, 40, b"Apply", Size::Small);
        frame.invert(0, 15 + self.selected as i16 * 12, 128, 9);
        frame.text_size(2, 51, if receive {
            b"RX AUDIO FILTER / TX UNCHANGED"
        } else {
            b"ONE TX SIGNAL / RX UNCHANGED"
        }, Size::Small);
        frame.text_size(2, 58, b"MENU SELECT / EXIT CANCEL", Size::Small);
    }
}

const fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

const _: () = {
    let mut editor = Editor::new(0);
    let repeat = Event { key: Key::Menu, edge: Edge::Repeat };
    assert!(editor.key(repeat).is_none() && editor.kind == 0);
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.kind == 1 && editor.selection() == 670);
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.kind == 2 && editor.selection() == 0x8013);

    editor.selected = 1;
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Zero)).is_none());
    assert!(editor.key(press(Key::Two)).is_none());
    assert!(editor.key(press(Key::Five)).is_none());
    assert!(editor.key(repeat).is_none() && editor.editing);
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(!editor.editing && editor.code == 0o025);

    editor.selected = 0;
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.kind == 3 && editor.selection() == 0xC015);
    editor.selected = 2;
    assert!(matches!(editor.key(press(Key::Menu)), Some(Effect::Apply(0xC015))));

    let mut invalid = Editor::new(0x8013);
    invalid.selected = 1;
    assert!(invalid.key(press(Key::Menu)).is_none());
    assert!(invalid.key(press(Key::Eight)).is_none());
    assert!(invalid.invalid && invalid.digits == 0);
    assert!(invalid.key(press(Key::Zero)).is_none());
    assert!(invalid.key(press(Key::Menu)).is_none());
    assert!(invalid.editing && invalid.invalid);
    assert!(invalid.key(press(Key::Exit)).is_none());
    assert!(!invalid.editing && invalid.selection() == 0x8013);
    assert!(matches!(invalid.key(press(Key::Exit)), Some(Effect::Cancel)));

    let mut tone = Editor::new(670);
    tone.selected = 1;
    assert!(tone.key(press(Key::Menu)).is_none());
    assert!(tone.key(press(Key::Down)).is_none());
    assert!(tone.candidate == 2541);
    assert!(tone.key(press(Key::Up)).is_none());
    assert!(tone.candidate == 670);
    assert!(tone.key(press(Key::Exit)).is_none());
    assert!(tone.selection() == 670);
};