//! Detached frequency-range editor.
//!
//! Each bound accepts six or seven digits in kHz. Complete bounds must belong
//! to one contiguous RX interval with lower below upper; a shared hardware
//! path cannot authorize a range across the gap.
//! Editing never writes preferences or retunes the receiver.
//!
//! MENU opens a field; its last required digit accepts automatically.
//! F on an empty field selects six or seven digits, otherwise it deletes.
//! Invalid complete input remains editable; a subsequent digit starts over.
//! MENU cannot accept a prefix.
//! Exit cancels the field, then cancels the complete range from the list.
//! Only Apply returns both checked bounds to the panel; field completion
//! does not bypass the ordering and same-path checks on the complete pair.

use crate::drivers::{
    bk4829::receive::Band,
    keyboard::{Edge, Event, Key},
};
use crate::ui::{font::Size, widgets, Frame};

pub(super) enum Effect {
    Cancel,
    Refuse,
    Apply(u32, u32),
}

pub(super) struct Editor {
    lower: u32,
    upper: u32,
    selected: usize,
    editing: bool,
    value: u32,
    digits: usize,
    wide: bool,
    invalid: bool,
}

const fn digit(key: Key) -> Option<u8> {
    match key {
        Key::Zero => Some(0),
        Key::One => Some(1),
        Key::Two => Some(2),
        Key::Three => Some(3),
        Key::Four => Some(4),
        Key::Five => Some(5),
        Key::Six => Some(6),
        Key::Seven => Some(7),
        Key::Eight => Some(8),
        Key::Nine => Some(9),
        _ => None,
    }
}

const fn valid(lower: u32, upper: u32) -> bool {
    if lower >= upper {
        return false;
    }

    let span = crate::profile::receive_span(lower);
    span != 0 && span == crate::profile::receive_span(upper)
}

impl Editor {
    pub(super) const fn new(lower: u32, upper: u32) -> Self {
        assert!(valid(lower, upper));

        Self {
            lower,
            upper,
            selected: 0,
            editing: false,
            value: 0,
            digits: 0,
            wide: false,
            invalid: false,
        }
    }

    const fn accept_frequency(&mut self) -> Option<Effect> {
        let frequency = match crate::frequency_entry::hertz(
            self.value, self.digits, self.wide,
        ) {
            Some(value) if Band::receive(value).is_some() => value,
            _ => {
                self.invalid = true;
                return Some(Effect::Refuse);
            }
        };

        if self.selected == 0 {
            self.lower = frequency;
        } else {
            self.upper = frequency;
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
            if !down {
                return None;
            }

            self.invalid = false;

            if let Some(digit) = digit(event.key) {
                if self.digits == crate::frequency_entry::count(self.wide) {
                    self.value = 0;
                    self.digits = 0;
                }
                self.value = self.value * 10 + digit as u32;
                self.digits += 1;
                if self.digits == crate::frequency_entry::count(self.wide) {
                    return self.accept_frequency();
                }
            } else if matches!(event.key, Key::F) {
                if self.digits != 0 {
                    self.value /= 10;
                    self.digits -= 1;
                } else {
                    self.wide = !self.wide;
                }
            } else if matches!(event.key, Key::Menu) {
                return self.accept_frequency();
            }

            return None;
        }

        if matches!(event.key, Key::Up | Key::Down) {
            self.selected = if matches!(event.key, Key::Up) {
                (self.selected + 2) % 3
            } else {
                (self.selected + 1) % 3
            };
        } else if matches!(event.key, Key::Menu) && down {
            if self.selected < 2 {
                self.wide = if self.selected == 0 {
                    self.lower >= 1_000_000_000
                } else {
                    self.upper >= 1_000_000_000
                };
                self.editing = true;
                self.value = 0;
                self.digits = 0;
                self.invalid = false;
            } else if valid(self.lower, self.upper) {
                return Some(Effect::Apply(self.lower, self.upper));
            } else {
                self.invalid = true;
                return Some(Effect::Refuse);
            }
        }

        None
    }

    fn preview(&self) -> [u8; 8] {
        crate::frequency_entry::preview(self.value, self.digits, self.wide)
    }

    pub(super) fn render(&self, frame: &mut Frame) {
        frame.clear();
        frame.text(2, 0, b"SCAN RANGE");
        frame.fill(0, 8, 128, 1, true);

        if self.editing {
            frame.text_size(
                2,
                12,
                if self.selected == 0 { b"LOWER KHZ" } else { b"UPPER KHZ" },
                Size::Small,
            );
            frame.text_size(10, 24, &self.preview(), Size::Large);
            frame.text_size(
                2,
                48,
                if self.invalid {
                    if self.digits < crate::frequency_entry::count(self.wide) {
                        crate::frequency_entry::required(self.wide)
                    } else {
                        b"REFUSE: RX OUT OF RANGE"
                    }
                } else {
                    crate::frequency_entry::hint(self.wide)
                },
                Size::Small,
            );
        } else {
            frame.text(4, 15, b"Lower");
            widgets::frequency(frame, 58, 16, self.lower, Size::Small);
            frame.text(4, 28, b"Upper");
            widgets::frequency(frame, 58, 29, self.upper, Size::Small);
            frame.text(4, 41, b"Apply");

            frame.invert(0, 14 + self.selected as i16 * 13, 128, 10);
            frame.text_size(
                2,
                52,
                if self.invalid {
                    b"REFUSE: ORDER / RX PATH"
                } else {
                    b"RAM DRAFT / SAVE SEPARATELY"
                },
                Size::Small,
            );
        }

        frame.text_size(2, 58, if self.editing {
            if self.invalid && self.digits == crate::frequency_entry::count(self.wide) {
                b"F DELETE / DIGIT RESTART / EXIT"
            } else {
                b"F DEL / EMPTY F:6/7 / EXIT"
            }
        } else {
            b"MENU SELECT / EXIT CANCEL"
        }, Size::Small);
    }
}

const _: () = {
    assert!(valid(136_000_000, 174_000_000));
    assert!(valid(433_000_000, 434_000_000));
    assert!(!valid(433_000_000, 433_000_000));
    assert!(!valid(434_000_000, 433_000_000));
    assert!(!valid(145_000_000, 433_000_000));
    assert!(!valid(630_000_000, 433_000_000));

    let mut editor = Editor::new(136_000_000, 174_000_000);
    editor.selected = 2;

    let repeated = Event { key: Key::Menu, edge: Edge::Repeat };
    assert!(editor.key(repeated).is_none());

    let accepted = editor.key(Event { key: Key::Menu, edge: Edge::Down });
    assert!(matches!(accepted, Some(Effect::Apply(136_000_000, 174_000_000))));

    let mut field = Editor::new(136_000_000, 174_000_000);
    assert!(field.key(Event { key: Key::Menu, edge: Edge::Down }).is_none());
    let digits = [
        Key::Zero, Key::Five, Key::Zero,
        Key::Zero, Key::Zero, Key::Zero,
    ];
    let mut index = 0;
    while index < digits.len() {
        assert!(field.key(Event {
            key: digits[index], edge: Edge::Down,
        }).is_none());
        if index < 5 {
            assert!(field.editing && field.lower == 136_000_000);
        }
        index += 1;
    }
    assert!(!field.editing && field.lower == 50_000_000);
    assert!(field.upper == 174_000_000);
    assert!(matches!(
        field.key(Event { key: Key::Exit, edge: Edge::Down }),
        Some(Effect::Cancel),
    ));

    let mut invalid = Editor::new(136_000_000, 174_000_000);
    assert!(invalid.key(Event { key: Key::Menu, edge: Edge::Down }).is_none());
    let digits = [
        Key::Six, Key::Three, Key::Zero,
        Key::Zero, Key::Zero, Key::Zero,
    ];
    index = 0;
    while index < digits.len() {
        let effect = invalid.key(Event {
            key: digits[index], edge: Edge::Down,
        });
        if index == 5 {
            assert!(matches!(effect, Some(Effect::Refuse)));
        } else {
            assert!(effect.is_none());
        }
        index += 1;
    }
    assert!(invalid.editing && invalid.invalid);
    assert!(invalid.lower == 136_000_000);
    assert!(invalid.key(Event { key: Key::One, edge: Edge::Down }).is_none());
    assert!(invalid.digits == 1 && invalid.value == 1);
    assert!(invalid.key(Event { key: Key::Exit, edge: Edge::Down }).is_none());
    assert!(invalid.lower == 136_000_000);

    invalid.lower = 200_000_000;
    invalid.selected = 2;
    assert!(matches!(
        invalid.key(Event { key: Key::Menu, edge: Edge::Down }),
        Some(Effect::Refuse),
    ));
};
const _: () = {
    assert!(valid(840_000_000, 1_300_000_000));
    assert!(!valid(500_000_000, 1_000_000_000));
    assert!(!valid(630_000_000, 1_000_000_000));

    let mut editor = Editor::new(840_000_000, 1_300_000_000);
    editor.selected = 1;
    assert!(editor.key(Event { key: Key::Menu, edge: Edge::Down }).is_none());
    assert!(editor.wide);
    let digits = [
        Key::One, Key::Two, Key::Zero, Key::Zero,
        Key::Zero, Key::Zero, Key::Zero,
    ];
    let mut index = 0;
    while index < digits.len() {
        assert!(editor.key(Event {
            key: digits[index], edge: Edge::Down,
        }).is_none());
        index += 1;
    }
    assert!(!editor.editing && editor.upper == 1_200_000_000);
};
