//! Status and text-message application over the existing FSK owner.
//!
//! F cycles Status, Inbox, and Compose. Status retains selectors 1..6 and
//! normal Menu/Exit return. Inbox arrows select arrival-ordered messages;
//! Zero clears the RAM inbox. Menu on a populated inbox opens a reply with
//! its sender as destination and an empty draft; Menu on an empty inbox
//! returns Home. Compose Menu selects local address, destination, or text.
//! Arrows change the selected value. Address digits form a detached three-
//! digit candidate, accepted automatically when complete. Local address is
//! 1..999; destination 0..999, where zero means broadcast. Partial address
//! input and rejected local zero block sending without changing an address.
//!
//! Text contains at most 52 printable ASCII bytes. Arrows cycle characters,
//! digits insert directly. Star release advances the cursor; holding Star
//! moves backward. Side2 release deletes the current character; holding
//! Side2 clears the draft. Two Small rows display 26 cells each. Cursor motion
//! does not append text; editing the cell immediately after the current text
//! extends it. Unsupported received punctuation is displayed as supplied.
//!
//! PTT transmission remains the existing admitted voice attempt followed by
//! one packet on normal release. Compose supplies a checked immutable message;
//! Status supplies its existing selector. Inbox has no transmit admission.
//! Releasing PTT during setup produces no packet. No UI key bypasses TX
//! policy, calibration, battery, carrier qualification, or the original TOT.
//!
//! Observation of the packet tail followed by error-free local completion
//! marks Sent. It does not mean peer delivery. A new sequence is assigned
//! after that completion. An unchanged failed attempt retains its sequence
//! for a retry, but editing after any attempted transmission allocates a new
//! one. A reply also allocates a new sequence. Sequence wraps from 65535 to
//! one, never zero. This avoids treating revised text as a retained duplicate.
//!
//! The inbox accepts local and broadcast messages and suppresses retained
//! sender/sequence duplicates. It owns four bounded entries and no framebuffer.
//! Addresses, draft, inbox, and sequence are volatile. No Flash write occurs.
//!
//! ACK On requests confirmation for addressed messages and permits automatic
//! responses to explicit requests addressed to the current local ID. Broadcast
//! never requests a response. Inbox duplicates may request another Ack because
//! the previous response may have been lost.
//!
//! One Ack waits for RF admission for at most 2500 observed ticks. Another
//! request cannot overwrite that slot. Once acquired, it remains immutable
//! until the transmitter reports completion or cancellation. The RF owner
//! supplies a separate permission and performs quiet acquisition, policy,
//! calibration, battery, MIC exclusion, and complete RF-off verification.
//! Physical PTT cancels the automatic response and requires normal release
//! before another attempt. No fake PTT edge is supplied.
//!
//! Manual local completion starts an exact three-second AckWait when the
//! captured message requested it. CONFIRMED means a matching software packet
//! was received, not authenticated peer identity. No automatic message retry
//! is scheduled. ACK selection remains volatile like the addresses.

use crate::drivers::{
    bk4829::{fsk::View, transmit::View as TxView},
    keyboard::{Edge, Event, Key},
};
use crate::link::{AckWait, AddressInput, Inbox, Message, Packet, Reception, TEXT_BYTES};
use crate::ui::{font::Size, widgets, Frame};

const ALPHABET: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-./?";

#[derive(Clone, Copy)]
enum Page {
    Status,
    Inbox,
    Compose,
    Settings,
}

fn digit(key: Key) -> Option<u8> {
    match key {
        Key::Zero => Some(0), Key::One => Some(1), Key::Two => Some(2),
        Key::Three => Some(3), Key::Four => Some(4), Key::Five => Some(5),
        Key::Six => Some(6), Key::Seven => Some(7), Key::Eight => Some(8),
        Key::Nine => Some(9), _ => None,
    }
}

fn character(value: u8, previous: bool) -> u8 {
    let at = ALPHABET.iter().position(|&byte| byte == value).unwrap_or(0);
    let next = if previous {
        (at + ALPHABET.len() - 1) % ALPHABET.len()
    } else {
        (at + 1) % ALPHABET.len()
    };
    ALPHABET[next]
}

pub(super) struct Lab {
    selected: u8,
    view: View,
    page: Page,
    inbox: Inbox,
    cursor: usize,
    local: u16,
    destination: u16,
    sequence: u16,
    draft: [u8; TEXT_BYTES],
    length: usize,
    text_cursor: usize,
    field: u8,
    address_input: AddressInput,
    tap: super::multitap::Input,
    address_invalid: bool,
    star_held: bool,
    delete_held: bool,
    attempted: bool,
    tx_active: bool,
    tx_status: &'static [u8],
    ack_enabled: bool,
    ack_pending: Option<Packet>,
    ack_deadline: u32,
    ack_wait: AckWait,
    ack_transmitting: bool,
    confirmed: bool,
    no_reply: bool,
    tail_seen: bool,
    sent: bool,
    error: u32,
}

impl Lab {
    pub(super) const fn new() -> Self {
        let mut draft = [b' '; TEXT_BYTES];
        draft[0] = b'H';
        draft[1] = b'E';
        draft[2] = b'L';
        draft[3] = b'L';
        draft[4] = b'O';
        Self {
            selected: 1,
            view: View::empty(),
            page: Page::Compose,
            inbox: Inbox::new(),
            cursor: 0,
            local: 1,
            destination: 0,
            sequence: 1,
            draft,
            length: 5,
            text_cursor: 0,
            field: 2,
            address_input: AddressInput::new(),
            tap: super::multitap::Input::new(),
            address_invalid: false,
            star_held: false,
            delete_held: false,
            attempted: false,
            tx_active: false,
            tx_status: b"",
            ack_enabled: false,
            ack_pending: None,
            ack_deadline: 0,
            ack_wait: AckWait::new(),
            ack_transmitting: false,
            confirmed: false,
            no_reply: false,
            tail_seen: false,
            sent: false,
            error: 0,
        }
    }

    pub(super) const fn selected(&self) -> u8 {
        self.selected
    }

    /// Only the Compose page supplies a message. Empty text cannot become
    /// a packet. The RF transmitter freezes this value at its new attempt.
    // UI admission and RF context construction share checked message creation.
    #[inline(never)]
    pub(super) fn message(&self) -> Option<Message> {
        if !matches!(self.page, Page::Compose)
            || self.address_input.active() || self.address_invalid
        {
            return None;
        }
        Message::new(
            self.local, self.destination, self.sequence, &self.draft[..self.length],
        ).map(|message| message.with_ack(self.ack_enabled))
    }

    pub(super) fn can_send(&self) -> bool {
        match self.page {
            Page::Status => true,
            Page::Inbox | Page::Settings => false,
            Page::Compose => self.message().is_some(),
        }
    }

    /// Return a queued response only while its sender remains the current ID.
    /// The application never turns this projection into physical PTT.
    pub(super) fn ack_packet(&self) -> Option<Packet> {
        if !self.ack_enabled {
            return None;
        }
        match self.ack_pending {
            Some(Packet::Ack { sender, .. }) if sender == self.local => self.ack_pending,
            _ => None,
        }
    }

    pub(super) fn observe(
        &mut self,
        view: View,
        packet: Option<Packet>,
        transmitter: TxView,
        automatic: bool,
        ack_result: Option<(Packet, bool)>,
    ) {
        let now = crate::mcu::tick::snapshot();
        let response = automatic || ack_result.is_some();
        self.view = view;
        self.tx_active = transmitter.active;
        self.tx_status = transmitter.status;
        self.ack_transmitting = automatic;

        if self.ack_wait.poll(now) {
            self.no_reply = true;
        }

        if let Some((finished, _completed)) = ack_result {
            if self.ack_pending == Some(finished) {
                self.ack_pending = None;
            }
        }

        // Temporary RF ineligibility does not renew the queue deadline.
        // The active transmitter retains its own fixed attempt and packet
        // deadlines; do not remove its immutable request during setup.
        if !response && crate::app::time::reached(now, self.ack_deadline) {
            self.ack_pending = None;
        }

        match packet {
            Some(Packet::Text(message)) => {
                let reception = self.inbox.receive(self.local, message);
                if matches!(reception, Reception::Inserted) {
                    self.cursor = self.inbox.count() - 1;
                }

                if self.ack_enabled && self.ack_pending.is_none() {
                    if let Some(packet) = crate::link::acknowledgement(
                        self.local, message, reception,
                    ) {
                        self.ack_pending = Some(packet);
                        self.ack_deadline = now.wrapping_add(2500);
                    }
                }
            }
            Some(packet @ Packet::Ack { .. }) => {
                if self.ack_wait.receive(now, packet) {
                    self.confirmed = true;
                    self.no_reply = false;
                }
            }
            None => {}
        }

        // Automatic responses must not advance the outgoing message's
        // sequence, mark it Sent, or establish a wait for another Ack.
        if !matches!(self.page, Page::Compose) || response {
            self.tail_seen = false;
            return;
        }

        if transmitter.active {
            self.tap.clear();
            self.attempted = true;
            self.sent = false;
            self.ack_wait.clear();
            self.confirmed = false;
            self.no_reply = false;
        }

        if transmitter.active && transmitter.status == b"FSK PACKET" {
            self.tail_seen = true;
            self.error = 0;
        } else if !transmitter.active && self.tail_seen {
            self.tail_seen = false;
            self.error = transmitter.error;
            self.sent = transmitter.error == 0;

            if self.sent {
                if let Some(message) = self.message() {
                    self.ack_wait.start(now, message);
                }
                self.next_sequence();
                self.attempted = false;
            }
        }
    }

    fn next_sequence(&mut self) {
        self.sequence = self.sequence.wrapping_add(1);
        if self.sequence == 0 {
            self.sequence = 1;
        }
    }

    /// A changed candidate after an uncertain attempt is a new message.
    /// Unchanged retries keep the previous sequence for duplicate suppression.
    fn changed(&mut self) {
        if self.attempted {
            self.next_sequence();
            self.attempted = false;
        }
        self.ack_wait.clear();
        self.confirmed = false;
        self.no_reply = false;
        self.sent = false;
        self.error = 0;
    }

    fn put(&mut self, byte: u8) {
        if self.text_cursor >= self.length || self.draft[self.text_cursor] != byte {
            self.changed();
            self.draft[self.text_cursor] = byte;
            self.length = self.length.max(self.text_cursor + 1);
        }
    }

    fn clear_draft(&mut self) {
        if self.length != 0 {
            self.changed();
        }
        self.draft.fill(b' ');
        self.length = 0;
        self.text_cursor = 0;
    }

    /// Copy only the sender address before mutating this application owner.
    /// Reply preparation never requests RF or changes the stored inbox entry.
    fn reply(&mut self) -> bool {
        let sender = match self.inbox.get(self.cursor) {
            Some(message) => message.sender,
            None => return false,
        };
        self.next_sequence();
        self.attempted = false;
        self.clear_draft();
        self.destination = sender;
        self.ack_wait.clear();
        self.confirmed = false;
        self.no_reply = false;
        self.address_input.clear();
        self.address_invalid = false;
        self.field = 2;
        self.star_held = false;
        self.delete_held = false;
        self.sent = false;
        self.error = 0;
        self.page = Page::Compose;
        true
    }

    fn delete_character(&mut self) {
        if self.text_cursor >= self.length {
            return;
        }
        self.changed();
        for index in self.text_cursor..self.length - 1 {
            self.draft[index] = self.draft[index + 1];
        }
        self.length -= 1;
        self.draft[self.length] = b' ';
        self.text_cursor = self.text_cursor.min(self.length);
    }

    /// True returns Home. Repeated arrows edit only their selected field;
    /// a repeat cannot change page, confirm, or request transmission.
    // Message editing and tab navigation remain outside Panel dispatch.
    #[inline(never)]
    pub(super) fn key(&mut self, event: Event) -> bool {
        if matches!(event.edge, Edge::Down | Edge::Hold)
            && (digit(event.key).is_none()
                || !matches!(self.page, Page::Compose)
                || self.field != 2)
        {
            self.tap.clear();
        }
        if matches!(self.page, Page::Compose) && self.field == 2 {
            match event.key {
                Key::Star => {
                    match event.edge {
                        Edge::Down => self.star_held = false,
                        Edge::Hold => {
                            self.star_held = true;
                            self.text_cursor = if self.text_cursor == 0 {
                                self.length.min(TEXT_BYTES - 1)
                            } else {
                                self.text_cursor - 1
                            };
                        }
                        Edge::Up if !self.star_held => {
                            let last = self.length.min(TEXT_BYTES - 1);
                            self.text_cursor = if self.text_cursor >= last {
                                0
                            } else {
                                self.text_cursor + 1
                            };
                        }
                        _ => {}
                    }
                    return false;
                }
                Key::Side2 => {
                    match event.edge {
                        Edge::Down => self.delete_held = false,
                        Edge::Hold => {
                            self.delete_held = true;
                            self.clear_draft();
                        }
                        Edge::Up if !self.delete_held => self.delete_character(),
                        _ => {}
                    }
                    return false;
                }
                _ => {}
            }
        }

        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return false;
        }
        let down = matches!(event.edge, Edge::Down);
        if down && matches!(event.key, Key::Exit) {
            return true;
        }
        if down && matches!(event.key, Key::F) {
            self.address_input.clear();
            self.address_invalid = false;
            self.star_held = false;
            self.delete_held = false;
            self.page = match self.page {
                Page::Compose => Page::Inbox,
                Page::Inbox => Page::Settings,
                Page::Settings => Page::Status,
                Page::Status => Page::Compose,
            };
            self.field = if matches!(self.page, Page::Settings) { 0 } else { 2 };
            self.tail_seen = false;
            return false;
        }

        match self.page {
            Page::Status => {
                if matches!(event.key, Key::Up) {
                    self.selected = if self.selected == 1 { 6 } else { self.selected - 1 };
                } else if matches!(event.key, Key::Down) {
                    self.selected = if self.selected == 6 { 1 } else { self.selected + 1 };
                } else if down {
                    if matches!(event.key, Key::Menu) {
                        return true;
                    }
                    if let Some(value) = digit(event.key) {
                        if (1..=6).contains(&value) {
                            self.selected = value;
                        }
                    }
                }
            }
            Page::Inbox => {
                let count = self.inbox.count();
                if count != 0 && matches!(event.key, Key::Up | Key::Down) {
                    self.cursor = if matches!(event.key, Key::Up) {
                        (self.cursor + count - 1) % count
                    } else {
                        (self.cursor + 1) % count
                    };
                } else if down && matches!(event.key, Key::Zero) {
                    self.inbox.clear();
                    self.cursor = 0;
                } else if down && matches!(event.key, Key::Menu) {
                    return !self.reply();
                }
            }
            Page::Compose | Page::Settings => {
                if down && matches!(event.key, Key::Menu) {
                    self.address_input.clear();
                    self.address_invalid = false;
                    self.star_held = false;
                    self.delete_held = false;
                    self.field = (self.field + 1) % 3;
                } else if down && self.field < 2 && digit(event.key).is_some() {
                    let value = digit(event.key).unwrap();
                    self.address_invalid = false;
                    if let Some(address) = self.address_input.push(value) {
                        if self.field == 0 && address == 0 {
                            self.address_invalid = true;
                        } else {
                            let previous = if self.field == 0 {
                                self.local
                            } else {
                                self.destination
                            };
                            if address != previous {
                                self.changed();
                            }
                            if self.field == 0 {
                                self.local = address;
                            } else {
                                self.destination = address;
                            }
                            self.address_input.clear();
                        }
                    }
                } else if matches!(event.key, Key::Up | Key::Down) {
                    let previous = matches!(event.key, Key::Down);
                    if self.field < 2 {
                        self.changed();
                        self.address_input.clear();
                        self.address_invalid = false;
                        let value = if self.field == 0 {
                            &mut self.local
                        } else {
                            &mut self.destination
                        };
                        let minimum = if self.field == 0 { 1 } else { 0 };
                        *value = if previous {
                            if *value == minimum { 999 } else { *value - 1 }
                        } else if *value == 999 {
                            minimum
                        } else {
                            *value + 1
                        };
                        self.sent = false;
                        self.error = 0;
                    } else if matches!(self.page, Page::Settings) {
                        self.changed();
                        self.ack_enabled = !self.ack_enabled;
                        self.ack_pending = None;
                    } else {
                        self.put(character(self.draft[self.text_cursor], previous));
                    }
                } else if down && self.field == 2 && matches!(self.page, Page::Compose) {
                    if let Some(value) = digit(event.key) {
                        let now = crate::mcu::tick::snapshot();
                        if let Some((advance, byte)) = self.tap.press(value, now) {
                            if advance {
                                if self.text_cursor == TEXT_BYTES - 1 {
                                    self.tap.clear();
                                    return false;
                                }
                                self.text_cursor += 1;
                            }
                            self.put(byte);
                        }
                    }
                }
            }
        }
        false
    }


    fn status(&self, frame: &mut Frame, frequency: u32) {
        widgets::frequency(frame, 2, 11, frequency, Size::Small);
        frame.text_size(
            70, 11, if self.view.armed { b"RX ARMED" } else { b"RX WAIT" },
            Size::Small,
        );
        frame.text_size(2, 21, b"SEND", Size::Small);
        widgets::number(frame, 24, 21, self.selected as u32, Size::Small);
        frame.text_size(34, 21, crate::fsk::label(self.selected), Size::Small);
        frame.text_size(2, 31, b"LAST", Size::Small);
        frame.text_size(24, 31, crate::fsk::label(self.view.last_status), Size::Small);
        widgets::number(frame, 76, 31, self.view.last_sequence as u32, Size::Small);

        // Status selection and the last received status are operational.
        // Raw counters and hardware-event fields belong to debug-tools.
        // The constant guard removes their drawing path from normal FSK.
        if !cfg!(feature = "debug-tools") {
            return;
        }

        frame.text_size(2, 41, b"RX", Size::Small);
        widgets::number(frame, 14, 41, self.view.accepted, Size::Small);
        frame.text_size(64, 41, b"BAD", Size::Small);
        widgets::number(frame, 80, 41, self.view.rejected, Size::Small);
        frame.text_size(2, 49, b"IRQ", Size::Small);
        frame.hex(16, 48, self.view.last_events as u32);
        frame.text_size(70, 49, b"W", Size::Small);
        widgets::number(frame, 78, 49, self.view.pending_words as u32, Size::Small);
        frame.text_size(94, 49, b"D", Size::Small);
        widgets::number(frame, 102, 49, self.view.discarded, Size::Small);
        frame.text_size(2, 58, b"PTT RELEASE SEND / F MESSAGES", Size::Small);
    }

    /// Draw a bounded text region, 24 Small cells per row. Each pane has
    /// room for all 52 message bytes across three rows. No text scrolls.
    fn pane(frame: &mut Frame, y: i16, text: &[u8]) {
        for row in 0..3 {
            let first = row * 24;
            if first >= text.len() {
                break;
            }
            let last = (first + 24).min(text.len());
            frame.text_size(30, y + row as i16 * 7, &text[first..last], Size::Small);
        }
    }

    /// Cached raw RSSI only for a settled receiver matching this screen's
    /// frequency. Five rising bars use the 0..511 indicator range. Carrier
    /// and speaker gating do not determine meter availability.
    fn header(
        frame: &mut Frame,
        frequency: u32,
        receiver: crate::drivers::bk4829::receive::View,
        voltage: Option<u32>,
        ack_enabled: bool,
    ) {
        frame.text_size(
            2, 1, if ack_enabled { b"ACK ON" } else { b"ACK OFF" }, Size::Small,
        );
        widgets::frequency(frame, 36, 1, frequency, Size::Small);
        widgets::battery_at(frame, 86, voltage);

        let valid = receiver.frequency == frequency && receiver.error == 0
            && matches!(
                receiver.phase, crate::drivers::bk4829::receive::Phase::Receive,
            );
        let filled = widgets::extent(
            if valid { receiver.rssi } else { 0 }, 511, 5,
        );
        for index in 0..5 {
            let height = if index < filled { index + 2 } else { 1 };
            frame.fill(
                109 + index as i16 * 3, 7 - height as i16,
                2, height as u16, true,
            );
        }
        frame.fill(0, 8, 128, 1, true);
    }

    /// Fixed five-cell badge inside the 27-pixel address column. Both the
    /// prefix and three-digit value share one permanent inverse background.
    /// Clearing its four corner pixels provides a one-pixel stepped edge.
    /// A selected address has an underline outside the badge.
    fn address_badge(
        frame: &mut Frame,
        top: i16,
        prefix: &[u8; 2],
        address: u16,
        candidate: Option<[u8; 3]>,
        selected: bool,
    ) {
        let digits = candidate.unwrap_or(if address <= 999 {
            [
                b'0' + (address / 100) as u8,
                b'0' + (address / 10 % 10) as u8,
                b'0' + (address % 10) as u8,
            ]
        } else {
            *b"???"
        });
        let label = [prefix[0], prefix[1], digits[0], digits[1], digits[2]];
        frame.text_size(2, top + 2, &label, Size::Small);
        frame.invert(0, top, 26, 9);
        frame.pixel(0, top as i32, false);
        frame.pixel(25, top as i32, false);
        frame.pixel(0, top as i32 + 8, false);
        frame.pixel(25, top as i32 + 8, false);
        if selected {
            frame.fill(1, top + 10, 25, 1, true);
        }
    }

    fn compose(&self, frame: &mut Frame) {
        frame.fill(27, 9, 1, 55, true);
        frame.fill(28, 33, 100, 1, true);

        let candidate = if self.address_input.active() {
            Some(self.address_input.preview())
        } else {
            None
        };
        Self::address_badge(
            frame, 11, b"ME", self.local,
            if self.field == 0 { candidate } else { None },
            self.field == 0,
        );
        Self::address_badge(
            frame, 36, b"TO", self.destination,
            if self.field == 1 { candidate } else { None },
            self.field == 1,
        );
        if self.destination == 0 {
            frame.text_size(2, 49, b"ALL", Size::Small);
        }

        Self::pane(frame, 11, &self.draft[..self.length]);
        if self.field == 2 {
            frame.fill(
                30 + (self.text_cursor % 24) as i16 * 4,
                17 + (self.text_cursor / 24) as i16 * 7,
                3, 1, true,
            );
        }

        if let Some(message) = self.inbox.get(self.inbox.count().saturating_sub(1)) {
            Self::pane(frame, 36, message.text());
        }

        let status: &[u8] = if self.address_invalid {
            b"BAD ID"
        } else if self.address_input.active() {
            b"3 DIGITS"
        } else if self.ack_transmitting {
            b"ACK TX"
        } else if self.tx_active {
            if self.tail_seen { b"SENDING" } else { self.tx_status }
        } else if self.error != 0 {
            b"TX FAILED"
        } else if self.ack_wait.active() {
            b"WAIT ACK"
        } else if self.confirmed {
            b"CONFIRMED"
        } else if self.no_reply {
            b"NO REPLY"
        } else if self.sent {
            b"SENT"
        } else {
            b""
        };
        frame.text_size(30, 58, status, Size::Small);
    }

    fn inbox_list(&self, frame: &mut Frame) {
        if self.inbox.count() == 0 {
            frame.text_size(2, 14, b"NO MESSAGES", Size::Small);
        }

        for index in 0..self.inbox.count() {
            let message = match self.inbox.get(index) {
                Some(message) => message,
                None => continue,
            };
            let y = 11 + index as i16 * 10;
            widgets::number(frame, 2, y, message.sender as u32, Size::Small);
            let text = message.text();
            frame.text_size(28, y, &text[..text.len().min(24)], Size::Small);
            if index == self.cursor {
                frame.invert(0, y - 1, 128, 8);
            }
        }

        if let Some(message) = self.inbox.get(self.cursor) {
            if message.text().len() > 24 {
                let text = message.text();
                frame.text_size(28, 51, &text[24..text.len().min(48)], Size::Small);
            }
        }
        frame.text_size(22, 58, b"MENU REPLY / 0 CLEAR", Size::Small);
    }

    fn settings(&self, frame: &mut Frame) {
        let labels: [&[u8]; 3] = [
            b"LOCAL ADDRESS", b"DESTINATION", b"ACK",
        ];

        for (index, label) in labels.iter().enumerate() {
            let y = 13 + index as i16 * 12;
            frame.text_size(3, y, label, Size::Small);

            if index == 2 {
                frame.text_size(
                    94, y, if self.ack_enabled { b"ON" } else { b"OFF" },
                    Size::Small,
                );
            } else {
                let value = if index == 0 { self.local } else { self.destination };
                if self.field as usize == index && self.address_input.active() {
                    frame.text_size(94, y, &self.address_input.preview(), Size::Small);
                } else if index == 1 && value == 0 {
                    frame.text_size(94, y, b"ALL", Size::Small);
                } else {
                    widgets::number(frame, 94, y, value as u32, Size::Small);
                }
            }

            if self.field as usize == index {
                frame.invert(0, y - 1, 128, 8);
            }
        }

        frame.text_size(2, 45, if self.address_invalid {
            b"LOCAL ADDRESS MUST BE 001-999"
        } else if self.field == 2 {
            b"ON: REQUEST ACK AND ANSWER"
        } else {
            b"3 DIGITS APPLY / ARROWS ADJUST"
        }, Size::Small);
        frame.text_size(2, 51, b"RAM SETTINGS / NO AUTO RETRY", Size::Small);
        frame.text_size(22, 58, b"MENU FIELD / F NEXT", Size::Small);
    }

    /// Four fixed tabs share the operational header. The index sits at the
    /// bottom left, outside both message panes. Input and rendering use the
    /// same page state; F changes pages without retuning or saving settings.
    // FSK tabs share one screen-composition entry outside Panel rendering.
    #[inline(never)]
    pub(super) fn render(
        &self,
        frame: &mut Frame,
        frequency: u32,
        receiver: crate::drivers::bk4829::receive::View,
        voltage: Option<u32>,
    ) {
        frame.clear();
        Self::header(frame, frequency, receiver, voltage, self.ack_enabled);

        match self.page {
            Page::Compose => self.compose(frame),
            Page::Inbox => self.inbox_list(frame),
            Page::Settings => self.settings(frame),
            Page::Status => {
                // Keep the existing diagnostic/status-selector page.
                self.status(frame, frequency);
                frame.fill(0, 57, 128, 7, false);
                frame.text_size(22, 58, b"PTT STATUS / F HOME", Size::Small);
            }
        }

        let index = match self.page {
            Page::Compose => b'1',
            Page::Inbox => b'2',
            Page::Settings => b'3',
            Page::Status => b'4',
        };
        frame.text_size(2, 58, &[index, b'/', b'4'], Size::Small);
    }
}