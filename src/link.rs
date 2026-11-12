//! Addressed messages over the existing 72-byte FSK transport.
//!
//! The application packet contains 36 little-endian FIFO words. Words 0..1
//! hold TKL1. Word 2 contains version one in its low byte and packet kind in
//! its high byte: one for text, two for acknowledgement. Word 3 is a nonzero
//! message sequence, word 4 the sender, and word 5 the destination. Sender
//! zero is invalid; destination zero is broadcast. Word 6 stores the byte
//! length in bits 0..5 and an acknowledgement request in bit 8. All other
//! bits are reserved. Broadcast and acknowledgement packets cannot request
//! another acknowledgement. Words 7..32 contain at most 52 printable ASCII
//! bytes, low byte first.
//! Unused text bytes and word 33 must be zero. Word 34 contains CRC-16/
//! CCITT-FALSE over words 0..33 in wire byte order. Word 35 is A55A.
//!
//! Acknowledgements have an addressed destination, no text, and the sequence
//! of the received message. Their sender is the receiving station. Broadcast
//! messages are never acknowledged. An acknowledgement establishes receipt
//! by a peer claiming that address, not identity or authentication.
//!
//! TKL1 is separate from the existing TKF1 status vocabulary. The transport
//! must dispatch complete captures by format rather than reinterpret status
//! words as messages. This module accesses no hardware or persistent storage.
//!
//! Firmware uses manual message transmission, the bounded inbox, and AckWait.
//! Its RF owner separately admits queued acknowledgement transmissions.
//! Delivery remains host-only: automatic message retries are not connected.
//!
//! Inbox keeps four messages in arrival order. A full inbox discards the
//! oldest complete entry. A duplicate sender/sequence pair is not inserted
//! again while that pair remains in the inbox. Eviction and clear remove
//! duplicate memory; this is not a permanent replay filter.
//!
//! Delivery tracks one immutable addressed attempt. The RF caller reports
//! local packet completion only after its modem reports completion. That
//! event starts a 1500-tick acknowledgement wait. At most three local sends
//! are permitted, within a fixed 10000-tick application deadline. A retry
//! request is not RF permission: the caller must independently admit every
//! transmission, including carrier, battery, cancellation, and TX limits.
//!
//! Waiting acknowledgements are accepted only for exact sender, destination,
//! and sequence. Broadcast completes as Sent, never Confirmed. Local failure,
//! expiry, and cancellation have distinct results. All deadlines use wrapping
//! observed ticks and require observation within half the counter range.

pub(crate) const WORDS: usize = 36;
pub(crate) const TEXT_BYTES: usize = 52;
const INBOX_ENTRIES: usize = 4;
#[cfg(test)]
const ACK_TICKS: u32 = 1500;
#[cfg(test)]
const TOTAL_TICKS: u32 = 10000;
#[cfg(test)]
const MAX_SENDS: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) sender: u16,
    pub(crate) destination: u16,
    pub(crate) sequence: u16,
    length: u8,
    ack: bool,
    text: [u8; TEXT_BYTES],
}

impl Message {
    /// Copy one checked ASCII message into owned bounded storage. Invalid
    /// input returns None without truncating text or changing an address.
    pub(crate) const fn new(
        sender: u16,
        destination: u16,
        sequence: u16,
        text: &[u8],
    ) -> Option<Self> {
        if sender == 0 || sequence == 0 || text.is_empty()
            || text.len() > TEXT_BYTES
        {
            return None;
        }

        let mut value = Self {
            sender, destination, sequence,
            length: text.len() as u8,
            ack: false,
            text: [0; TEXT_BYTES],
        };
        let mut index = 0;
        while index < text.len() {
            if text[index] < 32 || text[index] > 126 {
                return None;
            }
            value.text[index] = text[index];
            index += 1;
        }
        Some(value)
    }

    pub(crate) const fn text(&self) -> &[u8] {
        self.text.split_at(self.length as usize).0
    }

    /// Broadcast never asks every receiver to answer simultaneously.
    pub(crate) const fn with_ack(mut self, enabled: bool) -> Self {
        self.ack = enabled && self.destination != 0;
        self
    }

    pub(crate) const fn ack_requested(&self) -> bool {
        self.ack
    }

    /// Exact attempt identity, including text. The transmitter compares its
    /// captured message against the current UI request without constructing
    /// an encoded packet or changing the message sequence.
    pub(crate) const fn same(&self, other: &Self) -> bool {
        if self.sender != other.sender || self.destination != other.destination
            || self.sequence != other.sequence || self.length != other.length
            || self.ack != other.ack
        {
            return false;
        }

        let mut index = 0;
        while index < self.length as usize {
            if self.text[index] != other.text[index] {
                return false;
            }
            index += 1;
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Packet {
    Text(Message),
    Ack { sender: u16, destination: u16, sequence: u16 },
}

const fn crc_byte(mut crc: u16, value: u8) -> u16 {
    crc ^= (value as u16) << 8;
    let mut bit = 0;
    while bit < 8 {
        crc = if crc & 0x8000 != 0 {
            (crc << 1) ^ 0x1021
        } else {
            crc << 1
        };
        bit += 1;
    }
    crc
}

#[inline(never)]
const fn checksum(words: &[u16; WORDS]) -> u16 {
    let mut crc = 0xFFFF;
    let mut index = 0;
    while index < 34 {
        crc = crc_byte(crc, words[index] as u8);
        crc = crc_byte(crc, (words[index] >> 8) as u8);
        index += 1;
    }
    crc
}

/// Encode only valid packet values. Ack fields are checked here because
/// the enum can be constructed directly by an application caller.
#[inline(never)]
pub(crate) const fn encode(packet: Packet) -> Option<[u16; WORDS]> {
    let mut words = [0u16; WORDS];
    words[0] = 0x4B54;
    words[1] = 0x314C;

    match packet {
        Packet::Text(message) => {
            words[2] = 0x0101;
            words[3] = message.sequence;
            words[4] = message.sender;
            words[5] = message.destination;
            words[6] = message.length as u16 | ((message.ack as u16) << 8);
            let mut index = 0;
            while index < message.length as usize {
                words[7 + index / 2] |=
                    (message.text[index] as u16) << ((index & 1) * 8);
                index += 1;
            }
        }
        Packet::Ack { sender, destination, sequence } => {
            if sender == 0 || destination == 0 || sequence == 0 {
                return None;
            }
            words[2] = 0x0201;
            words[3] = sequence;
            words[4] = sender;
            words[5] = destination;
        }
    }

    words[34] = checksum(&words);
    words[35] = 0xA55A;
    Some(words)
}

/// Exact complete-frame admission. Unused bytes must be zero even with a
/// recomputed CRC. Text is not trimmed, case-folded, or interpreted as commands.
#[inline(never)]
pub(crate) const fn decode(words: &[u16; WORDS]) -> Option<Packet> {
    if words[0] != 0x4B54 || words[1] != 0x314C
        || words[2] as u8 != 1
        || words[3] == 0 || words[4] == 0
        || words[6] & !0x013F != 0
        || words[6] & 0x003F > TEXT_BYTES as u16
        || words[33] != 0 || words[35] != 0xA55A
        || words[34] != checksum(words)
    {
        return None;
    }

    let kind = words[2] >> 8;
    let length = (words[6] & 0x003F) as usize;
    let requested = words[6] & 0x0100 != 0;
    if kind == 1 {
        if length == 0 || (requested && words[5] == 0) { return None; }
    } else if kind == 2 {
        if length != 0 || words[5] == 0 || requested { return None; }
    } else {
        return None;
    }

    let mut text = [0u8; TEXT_BYTES];
    let mut index = 0;
    while index < TEXT_BYTES {
        let byte = (words[7 + index / 2] >> ((index & 1) * 8)) as u8;
        if index < length {
            if byte < 32 || byte > 126 { return None; }
            text[index] = byte;
        } else if byte != 0 {
            return None;
        }
        index += 1;
    }

    if kind == 2 {
        Some(Packet::Ack {
            sender: words[4], destination: words[5], sequence: words[3],
        })
    } else {
        Some(Packet::Text(Message {
            sender: words[4], destination: words[5], sequence: words[3],
            length: length as u8, ack: requested, text,
        }))
    }
}

/// Detached three-digit decimal address input. Partial digits never replace
/// the application's address. After a complete candidate, the next digit
/// starts another candidate. Zero admission belongs to the selected field:
/// destination zero broadcasts, while a local sender must remain nonzero.
pub(crate) struct AddressInput {
    value: u16,
    digits: usize,
}

impl AddressInput {
    pub(crate) const fn new() -> Self {
        Self { value: 0, digits: 0 }
    }

    pub(crate) const fn clear(&mut self) {
        *self = Self::new();
    }

    pub(crate) const fn active(&self) -> bool {
        self.digits != 0
    }

    /// Return a complete candidate only after the third decimal digit.
    /// Malformed input leaves the candidate unchanged.
    pub(crate) const fn push(&mut self, digit: u8) -> Option<u16> {
        if digit > 9 {
            return None;
        }
        if self.digits == 3 {
            self.clear();
        }
        self.value = self.value * 10 + digit as u16;
        self.digits += 1;
        if self.digits == 3 { Some(self.value) } else { None }
    }

    pub(crate) const fn preview(&self) -> [u8; 3] {
        let mut text = [b'-'; 3];
        let mut value = self.value;
        let mut index = self.digits;
        while index > 0 {
            index -= 1;
            text[index] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        text
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reception {
    Ignored,
    Inserted,
    Duplicate,
}

pub(crate) struct Inbox {
    entries: [Option<Message>; INBOX_ENTRIES],
    first: usize,
    count: usize,
}

impl Inbox {
    pub(crate) const fn new() -> Self {
        Self { entries: [None; INBOX_ENTRIES], first: 0, count: 0 }
    }

    pub(crate) const fn clear(&mut self) {
        *self = Self::new();
    }

    pub(crate) const fn count(&self) -> usize {
        self.count
    }

    /// Index zero is the oldest retained message. The borrow cannot outlive
    /// an insertion or clear performed through the mutable inbox owner.
    pub(crate) fn get(&self, index: usize) -> Option<&Message> {
        if index >= self.count {
            None
        } else {
            self.entries[(self.first + index) % INBOX_ENTRIES].as_ref()
        }
    }

    pub(crate) fn receive(&mut self, local: u16, message: Message) -> Reception {
        if local == 0 || message.sender == local
            || (message.destination != 0 && message.destination != local)
        {
            return Reception::Ignored;
        }

        for index in 0..self.count {
            if let Some(previous) = self.get(index) {
                if previous.sender == message.sender
                    && previous.sequence == message.sequence
                {
                    return Reception::Duplicate;
                }
            }
        }

        let at = if self.count == INBOX_ENTRIES {
            let at = self.first;
            self.first = (self.first + 1) % INBOX_ENTRIES;
            at
        } else {
            let at = (self.first + self.count) % INBOX_ENTRIES;
            self.count += 1;
            at
        };
        self.entries[at] = Some(message);
        Reception::Inserted
    }
}

/// A duplicate still needs another acknowledgement: the earlier Ack may
/// have been lost. Ignored messages and broadcasts produce no Ack request.
/// The returned packet is data only, not automatic transmitter admission.
pub(crate) const fn acknowledgement(
    local: u16, message: Message, reception: Reception,
) -> Option<Packet> {
    if local == 0 || message.destination != local
        || !message.ack_requested() || message.sender == local
        || matches!(reception, Reception::Ignored)
    {
        None
    } else {
        Some(Packet::Ack {
            sender: local,
            destination: message.sender,
            sequence: message.sequence,
        })
    }
}

const fn reached(now: u32, deadline: u32) -> bool {
    now.wrapping_sub(deadline) < 0x8000_0000
}

/// One confirmation wait started after local message transmission completes.
/// The exact peer, local address, and sequence are retained for 3000 observed
/// ticks. No packet or RF request is created by this object. Expiry clears
/// the wait permanently; late or unrelated acknowledgements cannot confirm it.
pub(crate) struct AckWait {
    local: u16,
    peer: u16,
    sequence: u16,
    deadline: u32,
    active: bool,
}

impl AckWait {
    pub(crate) const fn new() -> Self {
        Self { local: 0, peer: 0, sequence: 0, deadline: 0, active: false }
    }

    pub(crate) const fn clear(&mut self) {
        self.active = false;
    }

    pub(crate) const fn active(&self) -> bool {
        self.active
    }

    pub(crate) const fn start(&mut self, now: u32, message: Message) {
        self.local = message.sender;
        self.peer = message.destination;
        self.sequence = message.sequence;
        self.deadline = now.wrapping_add(3000);
        self.active = message.ack_requested() && message.destination != 0;
    }

    /// True reports expiry once. A repeated poll cannot repeat that event.
    pub(crate) const fn poll(&mut self, now: u32) -> bool {
        if self.active && reached(now, self.deadline) {
            self.active = false;
            true
        } else {
            false
        }
    }

    pub(crate) const fn receive(&mut self, now: u32, packet: Packet) -> bool {
        if !self.active || reached(now, self.deadline) {
            return false;
        }

        match packet {
            Packet::Ack { sender, destination, sequence }
                if sender == self.peer && destination == self.local
                    && sequence == self.sequence =>
            {
                self.active = false;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Ready,
    Sending,
    Waiting,
    Sent,
    Confirmed,
    NoReply,
    LocalFailure,
    Cancelled,
    Expired,
}

#[cfg(test)]
pub(crate) struct Delivery {
    message: Message,
    state: State,
    sends: u8,
    deadline: u32,
    ack_deadline: u32,
}

#[cfg(test)]
impl Delivery {
    pub(crate) const fn new(now: u32, message: Message) -> Self {
        Self {
            message, state: State::Ready, sends: 0,
            deadline: now.wrapping_add(TOTAL_TICKS),
            ack_deadline: 0,
        }
    }

    pub(crate) const fn state(&self) -> State {
        self.state
    }

    pub(crate) const fn sends(&self) -> u8 {
        self.sends
    }

    /// Called after the RF owner admits a fresh transmission. Returning a
    /// packet marks that local send in progress but does not access hardware.
    pub(crate) fn begin_send(&mut self, now: u32) -> Option<Packet> {
        self.poll(now);
        if self.state != State::Ready { return None; }
        self.sends += 1;
        self.state = State::Sending;
        Some(Packet::Text(self.message))
    }

    /// Hardware completion is supplied by the caller, not inferred from
    /// elapsed time. A late completion cannot revive an expired delivery.
    pub(crate) fn local_complete(&mut self, now: u32, success: bool) {
        self.poll(now);
        if self.state != State::Sending { return; }
        if !success {
            self.state = State::LocalFailure;
        } else if self.message.destination == 0 {
            self.state = State::Sent;
        } else {
            self.state = State::Waiting;
            self.ack_deadline = now.wrapping_add(ACK_TICKS);
        }
    }

    /// A late Ack may confirm a previous send while a retry awaits RF
    /// admission. It cannot confirm a packet before the first local send
    /// completed, during another send, or after a terminal result.
    pub(crate) fn receive(&mut self, now: u32, packet: Packet) -> bool {
        self.poll(now);
        if !(self.state == State::Waiting
            || (self.state == State::Ready && self.sends != 0))
        {
            return false;
        }

        if let Packet::Ack { sender, destination, sequence } = packet {
            if sender == self.message.destination
                && destination == self.message.sender
                && sequence == self.message.sequence
            {
                self.state = State::Confirmed;
                return true;
            }
        }
        false
    }

    pub(crate) fn cancel(&mut self) {
        if matches!(self.state, State::Ready | State::Sending | State::Waiting) {
            self.state = State::Cancelled;
        }
    }

    /// A deadline requests no RF operation. Ready means that an application
    /// retry may be considered by the normal transmitter admission path.
    pub(crate) fn poll(&mut self, now: u32) {
        if !matches!(self.state, State::Ready | State::Sending | State::Waiting) {
            return;
        }
        if reached(now, self.deadline) {
            self.state = State::Expired;
        } else if self.state == State::Waiting && reached(now, self.ack_deadline) {
            self.state = if self.sends < MAX_SENDS {
                State::Ready
            } else {
                State::NoReply
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(destination: u16, sequence: u16) -> Message {
        Message::new(10, destination, sequence, b"HELLO").unwrap()
    }

    fn ack(sequence: u16) -> Packet {
        Packet::Ack { sender: 20, destination: 10, sequence }
    }

    fn reseal(words: &mut [u16; WORDS]) {
        words[34] = checksum(words);
    }

    #[test]
    fn address_input_requires_three_digits_and_preserves_leading_zeroes() {
        let mut input = AddressInput::new();
        assert!(!input.active());
        assert_eq!(input.preview(), *b"---");
        assert_eq!(input.push(0), None);
        assert!(input.active());
        assert_eq!(input.preview(), *b"0--");
        assert_eq!(input.push(0), None);
        assert_eq!(input.preview(), *b"00-");
        assert_eq!(input.push(2), Some(2));
        assert_eq!(input.preview(), *b"002");
        assert_eq!(input.push(1), None);
        assert_eq!(input.preview(), *b"1--");
        input.clear();
        assert!(!input.active());
    }

    #[test]
    fn address_input_checks_every_supported_address() {
        for address in 0u16..=999 {
            let mut input = AddressInput::new();
            assert_eq!(input.push((address / 100) as u8), None);
            assert_eq!(input.push((address / 10 % 10) as u8), None);
            assert_eq!(input.push((address % 10) as u8), Some(address));
            let before = input.preview();
            assert_eq!(input.push(10), None);
            assert_eq!(input.preview(), before);
        }
    }

    #[test]
    fn crc_known_vector() {
        let mut crc = 0xFFFF;
        for &byte in b"123456789" {
            crc = crc_byte(crc, byte);
        }
        assert_eq!(crc, 0x29B1);
    }

    #[test]
    fn every_text_length_roundtrips_with_exact_padding() {
        for length in 1..=TEXT_BYTES {
            let text = [b'X'; TEXT_BYTES];
            let original = Message::new(65535, 0, 65535, &text[..length]).unwrap();
            let words = encode(Packet::Text(original)).unwrap();
            assert_eq!(decode(&words), Some(Packet::Text(original)));
            assert_eq!(original.text(), &text[..length]);
            assert_eq!(words[0].to_le_bytes(), *b"TK");
            assert_eq!(words[1].to_le_bytes(), *b"L1");
            assert_eq!(words[33], 0);
        }
    }

    #[test]
    fn message_constructor_rejects_invalid_input_without_truncation() {
        assert!(Message::new(0, 20, 1, b"A").is_none());
        assert!(Message::new(10, 20, 0, b"A").is_none());
        assert!(Message::new(10, 20, 1, b"").is_none());
        assert!(Message::new(10, 20, 1, &[b'A'; TEXT_BYTES + 1]).is_none());
        for byte in 0u8..=255 {
            let accepted = Message::new(10, 20, 1, &[byte]).is_some();
            assert_eq!(accepted, (32..=126).contains(&byte));
        }
    }

    #[test]
    fn every_wire_bit_is_checked() {
        for packet in [Packet::Text(message(20, 77)), ack(77)] {
            let clean = encode(packet).unwrap();
            for word in 0..WORDS {
                for bit in 0..16 {
                    let mut damaged = clean;
                    damaged[word] ^= 1 << bit;
                    assert!(decode(&damaged).is_none());
                }
            }
        }
    }

    #[test]
    fn crc_cannot_grant_reserved_fields_or_noncanonical_padding() {
        let clean = encode(Packet::Text(message(20, 1))).unwrap();
        for (index, value) in [
            (2, 0x0301), (2, 0x0102), (3, 0), (4, 0),
            (6, 0), (6, 53), (33, 1), (32, 1),
        ] {
            let mut damaged = clean;
            damaged[index] = value;
            reseal(&mut damaged);
            assert!(decode(&damaged).is_none());
        }

        let mut damaged = clean;
        damaged[9] |= 0x4100;
        reseal(&mut damaged);
        assert!(decode(&damaged).is_none());

        let mut damaged = clean;
        damaged[7] = 0x7F41;
        reseal(&mut damaged);
        assert!(decode(&damaged).is_none());
    }

    #[test]
    fn acknowledgement_has_no_text_and_requires_addressed_destination() {
        assert_eq!(decode(&encode(ack(1)).unwrap()), Some(ack(1)));
        for packet in [
            Packet::Ack { sender: 0, destination: 10, sequence: 1 },
            Packet::Ack { sender: 20, destination: 0, sequence: 1 },
            Packet::Ack { sender: 20, destination: 10, sequence: 0 },
        ] {
            assert!(encode(packet).is_none());
        }
        let mut words = encode(ack(1)).unwrap();
        words[7] = 1;
        reseal(&mut words);
        assert!(decode(&words).is_none());
    }

    #[test]
    fn inbox_filters_addresses_and_keeps_arrival_order() {
        let mut inbox = Inbox::new();
        assert_eq!(inbox.receive(20, message(30, 1)), Reception::Ignored);
        assert_eq!(inbox.receive(10, message(20, 1)), Reception::Ignored);
        assert_eq!(inbox.receive(0, message(0, 1)), Reception::Ignored);
        assert_eq!(inbox.count(), 0);

        for sequence in 1..=6 {
            assert_eq!(inbox.receive(20, message(20, sequence)), Reception::Inserted);
        }
        assert_eq!(inbox.count(), 4);
        for index in 0..4 {
            assert_eq!(inbox.get(index).unwrap().sequence, index as u16 + 3);
        }
        assert!(inbox.get(4).is_none());
        assert_eq!(inbox.receive(20, message(20, 6)), Reception::Duplicate);
        assert_eq!(inbox.count(), 4);

        inbox.clear();
        assert_eq!(inbox.count(), 0);
        assert_eq!(inbox.receive(20, message(0, 6)), Reception::Inserted);
    }

    #[test]
    fn duplicates_are_acknowledged_but_broadcasts_are_not() {
        let addressed = message(20, 1).with_ack(true);
        for result in [Reception::Inserted, Reception::Duplicate] {
            assert_eq!(acknowledgement(20, addressed, result), Some(ack(1)));
        }
        assert!(acknowledgement(20, addressed, Reception::Ignored).is_none());
        assert!(acknowledgement(20, message(0, 1), Reception::Inserted).is_none());
        assert!(acknowledgement(30, addressed, Reception::Inserted).is_none());
    }

    #[test]
    fn confirmation_requires_exact_peer_and_sequence_after_completion() {
        let mut delivery = Delivery::new(0, message(20, 7));
        assert!(!delivery.receive(0, ack(7)));
        assert!(delivery.begin_send(0).is_some());
        assert!(!delivery.receive(1, ack(7)));
        delivery.local_complete(10, true);
        assert_eq!(delivery.state(), State::Waiting);

        for packet in [
            ack(8),
            Packet::Ack { sender: 21, destination: 10, sequence: 7 },
            Packet::Ack { sender: 20, destination: 11, sequence: 7 },
            Packet::Text(message(20, 7)),
        ] {
            assert!(!delivery.receive(11, packet));
        }
        assert!(delivery.receive(12, ack(7)));
        assert_eq!(delivery.state(), State::Confirmed);
        delivery.poll(50000);
        delivery.cancel();
        assert_eq!(delivery.state(), State::Confirmed);
    }

    #[test]
    fn retries_are_bounded_and_never_start_themselves() {
        let mut delivery = Delivery::new(0, message(20, 1));
        for attempt in 0..3 {
            let start = attempt * 1600;
            assert_eq!(delivery.state(), State::Ready);
            assert!(delivery.begin_send(start).is_some());
            assert!(delivery.begin_send(start).is_none());
            delivery.local_complete(start + 10, true);
            delivery.poll(start + 1509);
            assert_eq!(delivery.state(), State::Waiting);
            delivery.poll(start + 1510);
        }
        assert_eq!(delivery.sends(), 3);
        assert_eq!(delivery.state(), State::NoReply);
        assert!(delivery.begin_send(5000).is_none());
        assert!(!delivery.receive(5000, ack(1)));
    }

    #[test]
    fn late_ack_can_cancel_a_retry_before_rf_admission() {
        let mut delivery = Delivery::new(0, message(20, 1));
        delivery.begin_send(0);
        delivery.local_complete(10, true);
        delivery.poll(1510);
        assert_eq!(delivery.state(), State::Ready);
        assert!(delivery.receive(1511, ack(1)));
        assert!(delivery.begin_send(1512).is_none());
        assert_eq!(delivery.sends(), 1);
    }

    #[test]
    fn broadcast_is_sent_without_claiming_delivery() {
        let mut delivery = Delivery::new(0, message(0, 1));
        delivery.begin_send(0);
        delivery.local_complete(10, true);
        assert_eq!(delivery.state(), State::Sent);
        assert!(!delivery.receive(11, ack(1)));
    }

    #[test]
    fn failure_cancellation_and_total_expiry_are_terminal() {
        let mut failed = Delivery::new(0, message(20, 1));
        failed.begin_send(0);
        failed.local_complete(10, false);
        assert_eq!(failed.state(), State::LocalFailure);
        assert!(failed.begin_send(20).is_none());

        let mut cancelled = Delivery::new(0, message(20, 1));
        cancelled.cancel();
        assert_eq!(cancelled.state(), State::Cancelled);
        assert!(cancelled.begin_send(20).is_none());

        let mut expired = Delivery::new(0, message(20, 1));
        expired.begin_send(0);
        expired.local_complete(10000, true);
        assert_eq!(expired.state(), State::Expired);
        assert!(!expired.receive(10001, ack(1)));
    }

    #[test]
    fn deadlines_cross_tick_wrap() {
        let start = u32::MAX - 100;
        let mut delivery = Delivery::new(start, message(20, 1));
        delivery.begin_send(start);
        delivery.local_complete(start.wrapping_add(10), true);
        delivery.poll(start.wrapping_add(1509));
        assert_eq!(delivery.state(), State::Waiting);
        delivery.poll(start.wrapping_add(1510));
        assert_eq!(delivery.state(), State::Ready);
        delivery.poll(start.wrapping_add(10000));
        assert_eq!(delivery.state(), State::Expired);
    }

    #[test]
    fn acknowledgement_selection_roundtrips_without_extending_the_packet() {
        let original = message(20, 7);
        let requested = original.with_ack(true);
        assert!(!original.ack_requested());
        assert!(requested.ack_requested());
        assert!(!original.same(&requested));

        let words = encode(Packet::Text(requested)).unwrap();
        assert_eq!(words[6], 0x0105);
        assert_eq!(decode(&words), Some(Packet::Text(requested)));
        assert_eq!(words.len() * 2, 72);

        let broadcast = message(0, 7).with_ack(true);
        assert!(!broadcast.ack_requested());
        assert!(acknowledgement(20, original, Reception::Inserted).is_none());
    }

    #[test]
    fn recomputed_crc_cannot_request_broadcast_ack_or_acknowledge_an_ack() {
        let mut broadcast = encode(Packet::Text(message(0, 7))).unwrap();
        broadcast[6] |= 0x0100;
        reseal(&mut broadcast);
        assert!(decode(&broadcast).is_none());

        let mut response = encode(ack(7)).unwrap();
        response[6] |= 0x0100;
        reseal(&mut response);
        assert!(decode(&response).is_none());

        let mut reserved = encode(Packet::Text(message(20, 7))).unwrap();
        reserved[6] |= 0x0040;
        reseal(&mut reserved);
        assert!(decode(&reserved).is_none());
    }

    #[test]
    fn confirmation_wait_requires_exact_peer_and_completed_local_send() {
        let mut wait = AckWait::new();
        assert!(!wait.active());
        assert!(!wait.receive(0, ack(7)));
        wait.start(100, message(20, 7).with_ack(true));
        assert!(wait.active());

        for packet in [
            ack(8),
            Packet::Ack { sender: 21, destination: 10, sequence: 7 },
            Packet::Ack { sender: 20, destination: 11, sequence: 7 },
            Packet::Text(message(20, 7)),
        ] {
            assert!(!wait.receive(101, packet));
        }

        assert!(wait.receive(3099, ack(7)));
        assert!(!wait.active());
        assert!(!wait.poll(3100));
        assert!(!wait.receive(3101, ack(7)));
    }

    #[test]
    fn confirmation_wait_expires_once_and_can_be_cancelled() {
        let mut wait = AckWait::new();
        wait.start(100, message(20, 7).with_ack(true));
        assert!(!wait.poll(3099));
        assert!(!wait.receive(3100, ack(7)));
        assert!(wait.poll(3100));
        assert!(!wait.poll(3101));

        wait.start(5000, message(20, 8).with_ack(true));
        wait.clear();
        assert!(!wait.active());
        assert!(!wait.receive(5001, ack(8)));

        wait.start(6000, message(0, 9).with_ack(true));
        assert!(!wait.active());
        wait.start(6000, message(20, 9));
        assert!(!wait.active());
    }

    #[test]
    fn confirmation_wait_crosses_tick_wrap() {
        let start = u32::MAX - 100;
        let mut wait = AckWait::new();
        wait.start(start, message(20, 7).with_ack(true));
        assert!(!wait.poll(start.wrapping_add(2999)));
        assert!(wait.receive(start.wrapping_add(2999), ack(7)));

        wait.start(start, message(20, 7).with_ack(true));
        assert!(wait.poll(start.wrapping_add(3000)));
        assert!(!wait.receive(start.wrapping_add(3001), ack(7)));
    }

    #[test]
    fn immutable_attempt_comparison_checks_all_message_fields() {
        let original = message(20, 7);
        assert!(original.same(&original));
        assert!(!original.same(&message(21, 7)));
        assert!(!original.same(&message(20, 8)));
        let changed = Message::new(10, 20, 7, b"HELLX").unwrap();
        assert!(!original.same(&changed));
        let changed = Message::new(11, 20, 7, b"HELLO").unwrap();
        assert!(!original.same(&changed));
    }

    #[test]
    fn storage_is_bounded_without_static_or_heap_buffers() {
        assert!(core::mem::size_of::<Message>() <= 60);
        assert!(core::mem::size_of::<Inbox>() <= 272);
        assert!(core::mem::size_of::<Delivery>() <= 80);
        assert_eq!(WORDS * 2, 72);
    }
}