//! Channel browser and confirmed RAM operations.
//!
//! Browsing owns only cursor state. It does not change the active receive
//! channel until Use is accepted. An action captures its source slot before
//! destination selection, so cursor movement cannot change the source.
//!
//! Copy into an occupied slot replaces it after confirmation. Move into an
//! occupied slot exchanges the two complete profiles; the confirmation names
//! that behavior explicitly. Empty destinations receive a normal move.
//!
//! Effects describe changes for Panel to apply. This module never mutates
//! the bank, writes Flash, or accesses RF. Bank methods validate the effect
//! again before mutation. Menu repeats cannot confirm a destructive action.
//!
//! Exit from confirmation returns to destination selection or the action
//! list. Exit from destination selection returns to the action list.
//! Only Exit from the browser returns to the normal radio screen.

use super::operation::Bank;
use crate::drivers::keyboard::{Edge, Event, Key};
use crate::ui::{font::Size, widgets, Frame};

#[derive(Clone, Copy)]
enum Command {
    Use,
    Edit,
    Copy,
    Move,
    Delete,
    Store,
}

#[derive(Clone, Copy)]
struct Item {
    label: &'static [u8],
    hint: &'static [u8],
    command: Command,
}

const POPULATED: [Item; 6] = [
    Item {
        label: b"Use",
        hint: b"Select this receive channel",
        command: Command::Use,
    },
    Item {
        label: b"Edit",
        hint: b"Detached profile and name draft",
        command: Command::Edit,
    },
    Item {
        label: b"Copy",
        hint: b"Source remains unchanged",
        command: Command::Copy,
    },
    Item {
        label: b"Move",
        hint: b"Occupied destination exchanges",
        command: Command::Move,
    },
    Item {
        label: b"Delete",
        hint: b"Clear RAM slot; Save is separate",
        command: Command::Delete,
    },
    Item {
        label: b"Store VFO",
        hint: b"Replace profile; keep slot name",
        command: Command::Store,
    },
];

const EMPTY: [Item; 1] = [
    Item {
        label: b"Store VFO",
        hint: b"Populate from the active VFO",
        command: Command::Store,
    },
];

#[derive(Clone, Copy)]
enum Phase {
    Browse,
    Actions,
    Destination(Command),
    Confirm(Command),
}

pub(super) enum Effect {
    Home,
    Use(usize),
    Edit(usize),
    Copy {
        source: usize,
        destination: usize,
    },
    Move {
        source: usize,
        destination: usize,
    },
    Delete(usize),
    Store(usize),
}

pub(super) struct Browser {
    selected: usize,
    source: usize,
    destination: usize,
    action: usize,
    phase: Phase,
    same_slot: bool,
}

impl Browser {
    pub(super) const fn new() -> Self {
        Self {
            selected: 0,
            source: 0,
            destination: 0,
            action: 0,
            phase: Phase::Browse,
            same_slot: false,
        }
    }

    pub(super) const fn open(&mut self, slot: usize) {
        assert!(slot < Bank::capacity());

        self.selected = slot;
        self.source = slot;
        self.destination = slot;
        self.action = 0;
        self.phase = Phase::Browse;
        self.same_slot = false;
    }

    pub(super) const fn after_change(&mut self, slot: usize) {
        self.open(slot);
    }

    const fn items(&self, bank: &Bank) -> &'static [Item] {
        if bank.channel(self.source).is_some() {
            &POPULATED
        } else {
            &EMPTY
        }
    }

    const fn shift(value: usize, count: usize, previous: bool) -> usize {
        if previous {
            (value + count - 1) % count
        } else {
            (value + 1) % count
        }
    }

    pub(super) const fn key(&mut self, event: Event, bank: &Bank) -> Option<Effect> {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return None;
        }

        let down = matches!(event.edge, Edge::Down);

        if matches!(event.key, Key::Exit) && down {
            self.same_slot = false;

            self.phase = match self.phase {
                Phase::Browse => return Some(Effect::Home),
                Phase::Actions => Phase::Browse,
                Phase::Destination(_) => Phase::Actions,
                Phase::Confirm(command) => match command {
                    Command::Copy | Command::Move => Phase::Destination(command),
                    _ => Phase::Actions,
                },
            };

            return None;
        }

        if matches!(event.key, Key::Up | Key::Down) {
            let previous = matches!(event.key, Key::Up);
            self.same_slot = false;

            match self.phase {
                Phase::Browse => {
                    self.selected = Self::shift(
                        self.selected,
                        Bank::capacity(),
                        previous,
                    );
                }
                Phase::Actions => {
                    self.action = Self::shift(
                        self.action,
                        self.items(bank).len(),
                        previous,
                    );
                }
                Phase::Destination(_) => {
                    self.destination = Self::shift(
                        self.destination,
                        Bank::capacity(),
                        previous,
                    );
                }
                Phase::Confirm(_) => {}
            }

            return None;
        }

        if !matches!(event.key, Key::Menu) || !down {
            return None;
        }

        match self.phase {
            Phase::Browse => {
                self.source = self.selected;
                self.destination = self.selected;
                self.action = 0;
                self.phase = Phase::Actions;
            }
            Phase::Actions => {
                let item = self.items(bank)[self.action];

                match item.command {
                    Command::Use => return Some(Effect::Use(self.source)),
                    Command::Edit => return Some(Effect::Edit(self.source)),
                    Command::Copy | Command::Move => {
                        self.destination = (self.source + 1) % Bank::capacity();
                        self.phase = Phase::Destination(item.command);
                    }
                    Command::Delete | Command::Store => {
                        self.phase = Phase::Confirm(item.command);
                    }
                }
            }
            Phase::Destination(command) => {
                if self.destination == self.source {
                    self.same_slot = true;
                } else {
                    self.same_slot = false;
                    self.phase = Phase::Confirm(command);
                }
            }
            Phase::Confirm(command) => {
                return Some(match command {
                    Command::Copy => Effect::Copy {
                        source: self.source,
                        destination: self.destination,
                    },
                    Command::Move => Effect::Move {
                        source: self.source,
                        destination: self.destination,
                    },
                    Command::Delete => Effect::Delete(self.source),
                    Command::Store => Effect::Store(self.source),
                    Command::Use => Effect::Use(self.source),
                    Command::Edit => Effect::Edit(self.source),
                });
            }
        }

        None
    }

    fn slot_number(frame: &mut Frame, x: i16, y: i16, slot: usize) {
        let number = slot + 1;
        let text = [
            b'0' + (number / 10) as u8,
            b'0' + (number % 10) as u8,
        ];

        frame.text(x, y, &text);
    }

    fn slot_summary(
        frame: &mut Frame,
        bank: &Bank,
        slot: usize,
        label: &[u8],
        y: i16,
    ) {
        frame.text_size(2, y, label, Size::Small);
        Self::slot_number(frame, 30, y - 1, slot);

        match bank.channel(slot) {
            Some(channel) => {
                frame.text_size(
                    50,
                    y,
                    if channel.named() {
                        &channel.name
                    } else {
                        b"UNNAMED"
                    },
                    Size::Small,
                );

                widgets::frequency(
                    frame,
                    30,
                    y + 8,
                    channel.tune.frequency,
                    Size::Small,
                );

                frame.text_size(
                    84,
                    y + 8,
                    channel.tune.name(),
                    Size::Small,
                );
            }
            None => {
                frame.text_size(50, y, b"EMPTY", Size::Small);
            }
        }
    }

    fn list(&self, frame: &mut Frame, bank: &Bank, dirty: bool) {
        frame.text(2, 0, b"CHANNELS");

        widgets::number(frame, 91, 1, bank.count(), Size::Small);
        frame.text_size(105, 1, b"/16", Size::Small);
        frame.fill(0, 8, 128, 1, true);

        let first = self.selected.saturating_sub(3);

        for row in 0..4 {
            let slot = first + row;

            if slot >= Bank::capacity() {
                break;
            }

            let y = 11 + row as i16 * 11;
            Self::slot_number(frame, 2, y, slot);

            match bank.channel(slot) {
                Some(channel) => {
                    frame.text(
                        19,
                        y,
                        if channel.named() {
                            &channel.name
                        } else {
                            b"UNNAMED"
                        },
                    );

                    if channel.tune.am || channel.tune.narrow {
                        frame.text_size(
                            73, y + 1,
                            if channel.tune.am { b"A" } else { b"N" },
                            Size::Small,
                        );
                    }

                    widgets::frequency(
                        frame,
                        83,
                        y + 1,
                        channel.tune.frequency,
                        Size::Small,
                    );
                }
                None => {
                    frame.text(19, y, b"EMPTY");
                }
            }

            if slot == self.selected {
                frame.invert(0, y - 1, 123, 10);
            }
        }

        widgets::scrollbar(frame, Bank::capacity(), first, 4);

        frame.text_size(
            2,
            57,
            if dirty {
                b"* RAM UNSAVED / EXIT BACK"
            } else {
                b"MENU ACTIONS / EXIT BACK"
            },
            Size::Small,
        );
    }

    fn actions(&self, frame: &mut Frame, bank: &Bank) {
        frame.text(2, 0, b"CHANNEL ACTIONS");
        Self::slot_number(frame, 106, 0, self.source);
        frame.fill(0, 8, 128, 1, true);

        let items = self.items(bank);
        let first = self.action.saturating_sub(3);

        for row in 0..4 {
            let index = first + row;

            if index >= items.len() {
                break;
            }

            let y = 11 + row as i16 * 10;
            frame.text(3, y, items[index].label);
            frame.text(112, y, b">");

            if index == self.action {
                frame.invert(0, y - 1, 123, 9);
            }
        }

        widgets::scrollbar(frame, items.len(), first, 4);
        frame.text_size(2, 52, items[self.action].hint, Size::Small);
        frame.text_size(2, 58, b"MENU SELECT / EXIT BACK", Size::Small);
    }

    fn destination(&self, frame: &mut Frame, bank: &Bank, command: Command) {
        frame.text(
            2,
            0,
            if matches!(command, Command::Copy) {
                b"COPY DESTINATION"
            } else {
                b"MOVE DESTINATION"
            },
        );
        frame.fill(0, 8, 128, 1, true);

        Self::slot_summary(frame, bank, self.source, b"FROM", 13);
        Self::slot_summary(frame, bank, self.destination, b"TO", 34);

        frame.text_size(
            2,
            57,
            if self.same_slot {
                b"CHOOSE A DIFFERENT SLOT"
            } else {
                b"UP/DOWN SLOT / MENU NEXT"
            },
            Size::Small,
        );
    }

    fn confirmation(
        &self,
        frame: &mut Frame,
        bank: &Bank,
        vfo: crate::profile::Profile,
        command: Command,
        store_allowed: bool,
    ) {
        let occupied = bank.channel(self.destination).is_some();

        let title: &[u8] = match command {
            Command::Copy => b"CONFIRM COPY",
            Command::Move if occupied => b"CONFIRM EXCHANGE",
            Command::Move => b"CONFIRM MOVE",
            Command::Delete => b"CONFIRM DELETE",
            Command::Store => b"CONFIRM STORE VFO",
            Command::Use | Command::Edit => b"CONFIRM",
        };

        frame.text(2, 0, title);
        frame.fill(0, 8, 128, 1, true);

        match command {
            Command::Store => {
                frame.text_size(2, 13, b"ACTIVE VFO", Size::Small);
                widgets::frequency(frame, 50, 13, vfo.tune.frequency, Size::Small);
                frame.text_size(2, 21, vfo.transmit.name(), Size::Small);
                match vfo.transmit_frequency() {
                    Ok(Some(frequency)) => widgets::frequency(
                        frame, 50, 21, frequency, Size::Small,
                    ),
                    Ok(None) => frame.text_size(50, 21, b"TX DISABLED", Size::Small),
                    Err(_) => frame.text_size(50, 21, b"OUT OF BAND", Size::Small),
                }
                Self::slot_summary(frame, bank, self.source, b"TO", 30);
            }
            Command::Delete => {
                Self::slot_summary(frame, bank, self.source, b"SLOT", 15);
            }
            _ => {
                Self::slot_summary(frame, bank, self.source, b"FROM", 13);
                Self::slot_summary(frame, bank, self.destination, b"TO", 31);
            }
        }

        let warning: &[u8] = match command {
            Command::Copy if occupied => b"DESTINATION WILL BE REPLACED",
            Command::Copy => b"SOURCE WILL REMAIN",
            Command::Move if occupied => b"BOTH PROFILES WILL BE EXCHANGED",
            Command::Move => b"SOURCE SLOT WILL BECOME EMPTY",
            Command::Delete => b"RAM DELETE / SAVED COPY STAYS",
            Command::Store if !store_allowed => b"INVALID VFO PROFILE",
            Command::Store => b"RAM REPLACE / SLOT NAME STAYS",
            Command::Use | Command::Edit => b"RAM OPERATION",
        };

        frame.text_size(2, 49, warning, Size::Small);
        frame.text_size(2, 58, if matches!(command, Command::Store) && !store_allowed {
            b"STORE BLOCKED / EXIT BACK"
        } else {
            b"MENU APPLY / EXIT CANCEL"
        }, Size::Small);
    }

    pub(super) fn render(
        &self, frame: &mut Frame, bank: &Bank, vfo: crate::profile::Profile,
        dirty: bool, store_allowed: bool,
    ) {
        frame.clear();

        match self.phase {
            Phase::Browse => self.list(frame, bank, dirty),
            Phase::Actions => self.actions(frame, bank),
            Phase::Destination(command) => self.destination(frame, bank, command),
            Phase::Confirm(command) => self.confirmation(
                frame, bank, vfo, command, store_allowed,
            ),
        }
    }
}

const fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

const _: () = {
    use super::operation::{Channel, Tune};

    let channel = Channel {
        tune: Tune {
            frequency: 433_700_000,
            step: 12_500,
            am: false, narrow: true,
        },
        transmit: crate::profile::Tx::Simplex,
        tx_signal: 0,
        rx_signal: 0,
        name: *b"LOCAL   ",
    };

    let mut bank = Bank::new();
    assert!(bank.update(0, channel));

    let mut browser = Browser::new();

    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Actions));

    match browser.key(press(Key::Menu), &bank) {
        Some(Effect::Use(0)) => {}
        _ => panic!("channel selection failed"),
    }

    browser.open(0);
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(browser.key(press(Key::Down), &bank).is_none());
    assert!(browser.key(press(Key::Down), &bank).is_none());
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Destination(Command::Copy)));
    assert!(browser.source == 0 && browser.destination == 1);

    assert!(browser.key(press(Key::Up), &bank).is_none());
    assert!(browser.destination == 0);
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(browser.same_slot);

    assert!(browser.key(press(Key::Down), &bank).is_none());
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Confirm(Command::Copy)));

    let repeated_menu = Event { key: Key::Menu, edge: Edge::Repeat };
    assert!(browser.key(repeated_menu, &bank).is_none());
    assert!(bank.count() == 1);

    match browser.key(press(Key::Menu), &bank) {
        Some(Effect::Copy { source: 0, destination: 1 }) => {}
        _ => panic!("copy confirmation failed"),
    }

    assert!(bank.count() == 1);

    browser.open(0);
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    browser.action = 3;
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Destination(Command::Move)));
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Confirm(Command::Move)));

    assert!(browser.key(press(Key::Exit), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Destination(Command::Move)));
    assert!(browser.key(press(Key::Exit), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Actions));
    assert!(browser.key(press(Key::Exit), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Browse));
    assert!(matches!(browser.key(press(Key::Exit), &bank), Some(Effect::Home)));

    browser.open(15);
    assert!(browser.key(press(Key::Down), &bank).is_none());
    assert!(browser.selected == 0);
    assert!(browser.key(press(Key::Up), &bank).is_none());
    assert!(browser.selected == 15);

    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(browser.items(&bank).len() == 1);
    assert!(browser.key(press(Key::Menu), &bank).is_none());
    assert!(matches!(browser.phase, Phase::Confirm(Command::Store)));
    assert!(browser.key(repeated_menu, &bank).is_none());

    match browser.key(press(Key::Menu), &bank) {
        Some(Effect::Store(15)) => {}
        _ => panic!("empty-slot store confirmation failed"),
    }

    assert!(bank.count() == 1);
    assert!(browser.key(press(Key::Ptt), &bank).is_none());
};