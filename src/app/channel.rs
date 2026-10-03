//! Detached channel draft with explicit RAM application.
//!
//! The editor owns a copy of one checked channel. Name and frequency subviews
//! retain a field-entry snapshot, so Exit discards that subview's changes.
//! Exit from the profile list discards the complete draft.
//!
//! Apply returns the checked draft and its original slot. The caller changes
//! the RAM bank only then; no editing event writes Flash or retunes RF.
//! Mode and step cycle within the draft, and cannot create arbitrary values.
//!
//! Names occupy eight printable ASCII bytes padded with spaces. Up and Down
//! select a character; F advances and Star moves back. Digits insert directly.
//! Unsupported imported punctuation remains intact until that cell is edited.
//! Frequency input accepts six or seven kHz digits automatically when complete
//! and admitted. The current frequency chooses the initial length; F on an
//! empty field switches it. Acceptance returns to the profile list without
//! applying the channel to RAM. F otherwise deletes a digit. A digit after
//! a rejected complete value starts over; EXIT retains field cancellation.

use super::operation::{Channel, Tune};
use crate::drivers::{
    bk4829::receive::Band,
    keyboard::{Edge, Event, Key},
};
use crate::ui::{font::Size, widgets, Frame};

const ALPHABET: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-./";

#[derive(Clone, Copy)]
enum Mode {
    List,
    Name,
    Frequency,
}

pub(super) enum Effect {
    Cancel,
    Refuse,
    Apply(usize, Channel),
}

pub(super) struct Editor {
    slot: usize,
    draft: Channel,
    before: Channel,
    selected: usize,
    mode: Mode,
    cursor: usize,
    input: u32,
    digits: usize,
    wide: bool,
    invalid: bool,
    route_editor: Option<super::tx_route::Editor>,

    signal_editor: Option<super::tx_signal::Editor>,
}

const fn digit(key: Key) -> Option<u8> {
    match key {
        Key::Zero => Some(0), Key::One => Some(1), Key::Two => Some(2),
        Key::Three => Some(3), Key::Four => Some(4), Key::Five => Some(5),
        Key::Six => Some(6), Key::Seven => Some(7), Key::Eight => Some(8),
        Key::Nine => Some(9), _ => None,
    }
}

const fn character(value: u8, up: bool) -> u8 {
    let mut found = 0;
    let mut index = 0;
    while index < ALPHABET.len() {
        if ALPHABET[index] == value {
            found = index;
            break;
        }
        index += 1;
    }
    let next = if up {
        (found + 1) % ALPHABET.len()
    } else {
        (found + ALPHABET.len() - 1) % ALPHABET.len()
    };
    ALPHABET[next]
}

impl Editor {
    pub(super) const fn new(slot: usize, channel: Channel) -> Self {
        assert!(slot < 16 && channel.valid());
        Self {
            slot,
            draft: channel,
            before: channel,
            selected: 0,
            mode: Mode::List,
            cursor: 0,
            input: 0,
            digits: 0,
            wide: false,
            invalid: false,
            route_editor: None,

            signal_editor: None,
        }
    }

    /// Accept only this detached field. The live bank remains unchanged
    /// until Apply RAM returns the complete checked channel.
    const fn accept_frequency(&mut self) -> Option<Effect> {
        let frequency = match crate::frequency_entry::hertz(
            self.input, self.digits, self.wide,
        ) {
            Some(value) if Band::receive(value).is_some() => value,
            _ => {
                self.invalid = true;
                return Some(Effect::Refuse);
            }
        };

        self.draft.tune.frequency = frequency;
        self.mode = Mode::List;
        self.invalid = false;
        None
    }

    pub(super) const fn key(&mut self, event: Event) -> Option<Effect> {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return None;
        }
        let down = matches!(event.edge, Edge::Down);

        if let Some(editor) = self.signal_editor.as_mut() {
            match editor.key(event) {
                Some(super::tx_signal::Effect::Cancel) => self.signal_editor = None,
                Some(super::tx_signal::Effect::Apply(value)) => {
                    if self.selected == 6 {
                        self.draft.rx_signal = value;
                    } else {
                        self.draft.tx_signal = value;
                    }
                    self.signal_editor = None;
                }
                None => {}
            }
            return None;
        }



        if let Some(editor) = self.route_editor.as_mut() {
            match editor.key(event) {
                Some(super::tx_route::Effect::Refuse) => return Some(Effect::Refuse),
                Some(super::tx_route::Effect::Cancel) => self.route_editor = None,
                Some(super::tx_route::Effect::Apply(route)) => {
                    self.draft.transmit = route;
                    self.route_editor = None;
                }
                None => {}
            }
            return None;
        }

        if matches!(event.key, Key::Exit) && down {
            if matches!(self.mode, Mode::List) {
                return Some(Effect::Cancel);
            }
            self.draft = self.before;
            self.mode = Mode::List;
            self.invalid = false;
            return None;
        }

        match self.mode {
            Mode::Name => {
                if matches!(event.key, Key::Up | Key::Down) {
                    self.draft.name[self.cursor] = character(
                        self.draft.name[self.cursor],
                        matches!(event.key, Key::Up),
                    );
                } else if down {
                    match event.key {
                        Key::F => self.cursor = (self.cursor + 1) & 7,
                        Key::Star => self.cursor = (self.cursor + 7) & 7,
                        Key::Menu => self.mode = Mode::List,
                        key => {
                            if let Some(value) = digit(key) {
                                self.draft.name[self.cursor] = b'0' + value;
                                self.cursor = (self.cursor + 1) & 7;
                            }
                        }
                    }
                }
            }
            Mode::Frequency => {
                if !down { return None; }
                self.invalid = false;

                if let Some(value) = digit(event.key) {
                    if self.digits == crate::frequency_entry::count(self.wide) {
                        self.input = 0;
                        self.digits = 0;
                    }
                    self.input = self.input * 10 + value as u32;
                    self.digits += 1;
                    if self.digits == crate::frequency_entry::count(self.wide) {
                        return self.accept_frequency();
                    }
                } else if matches!(event.key, Key::F) {
                    if self.digits != 0 {
                        self.input /= 10;
                        self.digits -= 1;
                    } else {
                        self.wide = !self.wide;
                    }
                } else if matches!(event.key, Key::Menu) {
                    return self.accept_frequency();
                }
            }
            Mode::List => {
                if matches!(event.key, Key::Up | Key::Down) {
                    self.selected = if matches!(event.key, Key::Up) {
                        (self.selected + 7) % 8
                    } else {
                        (self.selected + 1) % 8
                    };
                } else if matches!(event.key, Key::Menu) && down {
                    match self.selected {
                        0 => {
                            self.before = self.draft;
                            self.cursor = 0;
                            self.mode = Mode::Name;
                        }
                        1 => {
                            self.before = self.draft;
                            self.input = 0;
                            self.digits = 0;
                            self.invalid = false;
                            self.wide = self.draft.tune.frequency >= 1_000_000_000;
                            self.mode = Mode::Frequency;
                        }
                        2 => {
                            let flags = (self.draft.tune.flags() + 1) & 3;
                            self.draft.tune.narrow = flags & 1 != 0;
                            self.draft.tune.am = flags & 2 != 0;
                        }
                        3 => self.draft.tune.step = match self.draft.tune.step {
                            12_500 => 25_000,
                            25_000 => 100_000,
                            _ => 12_500,
                        },
                        4 => {
                            self.route_editor = Some(super::tx_route::Editor::new(
                                self.draft.tune.frequency,
                                self.draft.transmit,
                            ));
                        }
                        5 => {
                            self.signal_editor = Some(super::tx_signal::Editor::new(
                                self.draft.tx_signal,
                            ));
                        }
                        6 => {
                            self.signal_editor = Some(super::tx_signal::Editor::new(
                                self.draft.rx_signal,
                            ));
                        }
                        _ => {
                            if self.draft.valid() {
                                return Some(Effect::Apply(self.slot, self.draft));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn preview(&self) -> [u8; 8] {
        crate::frequency_entry::preview(self.input, self.digits, self.wide)
    }

    pub(super) fn render(&self, frame: &mut Frame) {
        if let Some(editor) = self.signal_editor.as_ref() {
            editor.render(frame, self.selected == 6);
            return;
        }


        if let Some(editor) = self.route_editor.as_ref() {
            editor.render(frame);
            return;
        }
        frame.clear();
        frame.text(2, 0, b"CHANNEL EDIT");
        widgets::number(frame, 92, 1, self.slot as u32 + 1, Size::Small);
        frame.fill(0, 8, 128, 1, true);

        match self.mode {
            Mode::Name => {
                frame.text_size(2, 11, b"EIGHT ASCII CHARACTERS", Size::Small);
                frame.text_size(8, 21, &self.draft.name, Size::Large);
                frame.invert(8 + self.cursor as i16 * 12, 21, 12, 16);
                frame.text_size(2, 43, b"UP/DOWN CHAR / F NEXT", Size::Small);
                frame.text_size(2, 50, b"STAR BACK / DIGITS INSERT", Size::Small);
                frame.text_size(2, 58, b"MENU ACCEPT / EXIT CANCEL", Size::Small);
            }
            Mode::Frequency => {
                frame.text_size(10, 18, &self.preview(), Size::Large);
                frame.text_size(2, 43, if self.invalid {
                    if self.digits < crate::frequency_entry::count(self.wide) {
                        crate::frequency_entry::required(self.wide)
                    } else {
                        b"REFUSE: RX OUT OF RANGE"
                    }
                } else {
                    crate::frequency_entry::hint(self.wide)
                }, Size::Small);
                frame.text_size(2, 50, if self.invalid && self.digits == crate::frequency_entry::count(self.wide) {
                    b"F DELETE / DIGIT RESTART"
                } else {
                    b"F DELETE / EMPTY F: 6 OR 7"
                }, Size::Small);
                frame.text_size(2, 58, b"EXIT CANCELS THIS FIELD", Size::Small);
            }
            Mode::List => {
                let labels: [&[u8]; 8] = [
                    b"Name", b"Frequency", b"Mode", b"Step",
                    b"TX route", b"TX signal", b"RX signal", b"Apply RAM",
                ];
                let first = self.selected.saturating_sub(3);
                for (index, label) in labels.iter().enumerate().skip(first).take(4) {
                    let y = 11 + (index - first) as i16 * 10;
                    frame.text_size(3, y, label, Size::Small);
                    match index {
                        0 => frame.text_size(70, y, &self.draft.name, Size::Small),
                        1 => widgets::frequency(
                            frame, 70, y, self.draft.tune.frequency, Size::Small,
                        ),
                        2 => frame.text_size(90, y, self.draft.tune.name(), Size::Small),
                        3 => widgets::number(
                            frame, 90, y, self.draft.tune.step, Size::Small,
                        ),
                        4 => frame.text_size(70, y, self.draft.transmit.name(), Size::Small),
                        5 => frame.text_size(90, y, &crate::tx_signal::text(self.draft.tx_signal), Size::Small),
                        6 => frame.text_size(90, y, &crate::tx_signal::text(self.draft.rx_signal), Size::Small),
                        _ => frame.text_size(114, y, b">", Size::Small),
                    }
                    if index == self.selected {
                        frame.invert(0, y - 1, 128, 8);
                    }
                }
                widgets::scrollbar(frame, labels.len(), first, 4);
                frame.text_size(2, 58, b"DRAFT ONLY / EXIT CANCELS", Size::Small);
            }
        }
    }
}

const fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

const _: () = {
    assert!(character(b' ', true) == b'A');
    assert!(character(b'A', false) == b' ');
    assert!(character(b' ', false) == b'/');
    assert!(character(b'/', true) == b' ');

    let original = Channel {
        tune: Tune { frequency: 145_500_000, step: 12_500, am: false, narrow: false },
        transmit: crate::profile::Tx::Simplex,
        tx_signal: 0,
        rx_signal: 0,
        name: [b' '; 8],
    };
    let mut editor = Editor::new(1, original);

    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Up)).is_none());
    assert!(editor.draft.name[0] == b'A');
    assert!(editor.key(press(Key::Exit)).is_none());
    assert!(editor.draft.name[0] == b' ');

    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Up)).is_none());
    assert!(editor.key(press(Key::F)).is_none());
    assert!(editor.key(press(Key::Up)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.draft.name[0] == b'A' && editor.draft.name[1] == b'A');

    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    let digits = [Key::Four, Key::Three, Key::Three, Key::Five, Key::Zero, Key::Zero];
    let mut index = 0;
    while index < digits.len() {
        assert!(editor.key(press(digits[index])).is_none());
        index += 1;
    }
    assert!(matches!(editor.mode, Mode::List));
    assert!(editor.draft.tune.frequency == 433_500_000);

    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.draft.tune.narrow);
    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.draft.tune.step == 25_000);
    editor.selected = 5;

    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Exit)).is_none());
    assert!(editor.draft.tx_signal == 0);

    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.draft.tx_signal == 670);
    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.draft.rx_signal == 670);
    assert!(editor.draft.tx_signal == 670);
    assert!(editor.key(press(Key::Down)).is_none());

    match editor.key(press(Key::Menu)) {
        Some(Effect::Apply(1, channel)) => {
            assert!(channel.valid());
            assert!(channel.tune.frequency == 433_500_000);
            assert!(channel.tune.narrow);
            assert!(channel.name[0] == b'A' && channel.name[1] == b'A');
        }
        _ => panic!("channel draft was not applied"),
    }
    assert!(original.tune.frequency == 145_500_000);
    assert!(original.name[0] == b' ');

    // Six digits can select the upper interval below 1000 MHz.
    // The final digit accepts the field but does not apply the channel.
    let mut upper_six = Editor::new(0, original);
    upper_six.selected = 1;
    assert!(upper_six.key(press(Key::Menu)).is_none());
    index = 0;
    while index < 6 {
        assert!(upper_six.key(press(Key::Nine)).is_none());
        if index < 5 {
            assert!(matches!(upper_six.mode, Mode::Frequency));
            assert!(upper_six.draft.tune.frequency == original.tune.frequency);
        }
        index += 1;
    }
    assert!(matches!(upper_six.mode, Mode::List));
    assert!(!upper_six.invalid);
    assert!(upper_six.draft.tune.frequency == 999_999_000);
    assert!(matches!(
        upper_six.key(press(Key::Exit)), Some(Effect::Cancel),
    ));

    // 777.777 MHz is inside the excluded interval. Refusal retains the
    // candidate for deletion or restart without changing the draft frequency.
    let mut invalid = Editor::new(0, original);
    invalid.selected = 1;
    assert!(invalid.key(press(Key::Menu)).is_none());
    index = 0;
    while index < 6 {
        let effect = invalid.key(press(Key::Seven));
        if index == 5 {
            assert!(matches!(effect, Some(Effect::Refuse)));
        } else {
            assert!(effect.is_none());
        }
        index += 1;
    }
    assert!(matches!(invalid.key(press(Key::Menu)), Some(Effect::Refuse)));
    assert!(invalid.invalid);
    assert!(invalid.input == 777777 && invalid.digits == 6);
    assert!(invalid.draft.tune.frequency == original.tune.frequency);
    assert!(invalid.key(press(Key::F)).is_none());
    assert!(invalid.digits == 5 && invalid.input == 77777);
    assert!(matches!(invalid.key(press(Key::Zero)), Some(Effect::Refuse)));
    assert!(invalid.key(press(Key::One)).is_none());
    assert!(invalid.digits == 1 && invalid.input == 1);
    assert!(invalid.draft.tune.frequency == original.tune.frequency);
    assert!(invalid.key(press(Key::Exit)).is_none());
    assert!(matches!(invalid.key(press(Key::Exit)), Some(Effect::Cancel)));

    let mut held = Editor::new(0, original);
    let repeated_menu = Event { key: Key::Menu, edge: Edge::Repeat };
    assert!(held.key(repeated_menu).is_none());
    assert!(matches!(held.mode, Mode::List));
};
const _: () = {
    let original = Channel {
        tune: Tune {
            frequency: 433_500_000, step: 12_500, narrow: false, am: false,
        },
        transmit: crate::profile::Tx::ReceiveOnly,
        tx_signal: 0, rx_signal: 0, name: *b"UPPER RX",
    };
    let mut editor = Editor::new(0, original);
    editor.selected = 1;
    assert!(editor.key(press(Key::Menu)).is_none());
    assert!(editor.key(press(Key::F)).is_none());
    assert!(editor.wide);

    let digits = [
        Key::One, Key::Three, Key::Zero, Key::Zero,
        Key::Zero, Key::Zero, Key::Zero,
    ];
    let mut index = 0;
    while index < digits.len() {
        assert!(editor.key(press(digits[index])).is_none());
        index += 1;
    }
    assert!(matches!(editor.mode, Mode::List));
    assert!(editor.draft.tune.frequency == 1_300_000_000);
    assert!(original.tune.frequency == 433_500_000);
    assert!(matches!(editor.key(press(Key::Exit)), Some(Effect::Cancel)));
};
