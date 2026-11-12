//! Fixed status packets and bounded FIFO collection without device access.
//!
//! The wire contains 36 little-endian words, totaling 72 bytes. Words zero
//! and one contain ASCII TKF1. Word two stores version one in its low byte
//! and a status selector in its high byte. Word three stores a nonzero
//! sequence. Words 4..33 are reserved zeroes. Word 34 contains CRC-16/
//! CCITT-FALSE over the first 68 bytes; word 35 contains terminal A55A.
//!
//! Status selectors 1..6 identify CHECK, RECEIVED, COME HERE, WAIT,
//! READY, and CANCEL. No native Rust structure is serialized. Validation
//! checks magic, version, selector, sequence, reserved words, CRC, and
//! terminal before exposing a packet.
//!
//! The sequence advances locally for each requested packet and resets on
//! application restart. There is no station identifier, duplicate filter,
//! delivery acknowledgement, encryption, or authentication in this format.
//! A valid packet is an observation, never a command to operate RF or storage.
//!
//! Collection accepts exactly 36 words. A new synchronization boundary
//! discards an incomplete predecessor. A fixed 1000-tick partial-frame
//! deadline is not extended by additional words. Overflow and expiration
//! discard the partial frame without interpreting its stale tail.
//!
//! This module owns no static RAM and performs no peripheral access.
//! Hardware FIFO words and event boundaries are supplied by the RF owner.

pub(crate) const WORDS: usize = 36;
pub(crate) const STATUS_COUNT: u8 = 6;

#[derive(Clone, Copy)]
pub(crate) struct Packet {
    pub(crate) sequence: u16,
    pub(crate) status: u8,
}

pub(crate) const fn label(status: u8) -> &'static [u8] {
    match status {
        1 => b"CHECK",
        2 => b"RECEIVED",
        3 => b"COME HERE",
        4 => b"WAIT",
        5 => b"READY",
        6 => b"CANCEL",
        _ => b"NONE",
    }
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

/// The low byte of each FIFO word precedes its high byte. Hardware
/// scrambling is separate and does not participate in this software CRC.
#[inline(never)]
const fn checksum(words: &[u16; WORDS]) -> u16 {
    let mut crc = 0xFFFFu16;
    let mut index = 0;

    while index < 34 {
        crc = crc_byte(crc, words[index] as u8);
        crc = crc_byte(crc, (words[index] >> 8) as u8);
        index += 1;
    }

    crc
}

pub(crate) const fn encode(status: u8, sequence: u16) -> [u16; WORDS] {
    assert!(status >= 1 && status <= STATUS_COUNT && sequence != 0);

    let mut words = [0u16; WORDS];
    words[0] = 0x4B54;
    words[1] = 0x3146;
    words[2] = 1 | ((status as u16) << 8);
    words[3] = sequence;
    words[34] = checksum(&words);
    words[35] = 0xA55A;
    words
}

pub(crate) const fn decode(words: &[u16; WORDS]) -> Option<Packet> {
    let status = (words[2] >> 8) as u8;

    if words[0] != 0x4B54 || words[1] != 0x3146
        || words[2] as u8 != 1
        || status == 0 || status > STATUS_COUNT
        || words[3] == 0
        || words[35] != 0xA55A
        || words[34] != checksum(words)
    {
        return None;
    }

    let mut index = 4;
    while index < 34 {
        if words[index] != 0 {
            return None;
        }
        index += 1;
    }

    Some(Packet { sequence: words[3], status })
}

pub(crate) struct Collector {
    words: [u16; WORDS],
    count: usize,
    active: bool,
    deadline: u32,
}

impl Collector {
    pub(crate) const fn new() -> Self {
        Self {
            words: [0; WORDS],
            count: 0,
            active: false,
            deadline: 0,
        }
    }

    pub(crate) const fn count(&self) -> usize {
        self.count
    }

    pub(crate) const fn discard(&mut self) {
        self.count = 0;
        self.active = false;
    }

    /// Establish a new hardware-reported frame boundary. No old word
    /// participates unless it is replaced by a word of the new collection.
    pub(crate) const fn sync(&mut self, now: u32) {
        self.count = 0;
        self.active = true;
        self.deadline = now.wrapping_add(1000);
    }

    pub(crate) const fn push(&mut self, now: u32, word: u16) -> bool {
        if !self.active {
            self.sync(now);
        }
        if self.count == WORDS {
            self.discard();
            return false;
        }

        self.words[self.count] = word;
        self.count += 1;
        true
    }

    pub(crate) const fn expire(&mut self, now: u32) -> bool {
        if self.active && crate::app::time::reached(now, self.deadline) {
            self.discard();
            true
        } else {
            false
        }
    }

    /// Return an exact complete capture for TKF1/TKL1 format dispatch.
    /// Neither format is admitted here. The RF owner validates it after
    /// collection, before publishing a status or message.
    pub(crate) const fn finish_words(&mut self) -> Option<[u16; WORDS]> {
        let result = if self.active && self.count == WORDS {
            Some(self.words)
        } else {
            None
        };
        self.discard();
        result
    }

    pub(crate) const fn finish(&mut self) -> Option<Packet> {
        let result = if self.active && self.count == WORDS {
            decode(&self.words)
        } else {
            None
        };

        self.discard();
        result
    }
}

const fn expect(packet: Option<Packet>, status: u8, sequence: u16) {
    match packet {
        Some(packet) => {
            assert!(packet.status == status);
            assert!(packet.sequence == sequence);
        }
        None => panic!("FSK packet rejected"),
    }
}

const _: () = {
    let vector = b"123456789";
    let mut crc = 0xFFFF;
    let mut index = 0;
    while index < vector.len() {
        crc = crc_byte(crc, vector[index]);
        index += 1;
    }
    assert!(crc == 0x29B1);

    let mut status = 1;
    while status <= STATUS_COUNT {
        let words = encode(status, 0x1234);
        assert!(words[0] as u8 == b'T');
        assert!((words[0] >> 8) as u8 == b'K');
        assert!(words[1] as u8 == b'F');
        assert!((words[1] >> 8) as u8 == b'1');
        expect(decode(&words), status, 0x1234);
        status += 1;
    }

    // Every transmitted bit participates in either CRC or explicit framing.
    let clean = encode(1, 77);
    index = 0;
    while index < WORDS {
        let mut bit = 0;
        while bit < 16 {
            let mut damaged = clean;
            damaged[index] ^= 1u16 << bit;
            assert!(decode(&damaged).is_none());
            bit += 1;
        }
        index += 1;
    }

    // A recomputed CRC cannot grant a reserved field or unknown selector.
    let mut invalid = clean;
    invalid[9] = 1;
    invalid[34] = checksum(&invalid);
    assert!(decode(&invalid).is_none());

    invalid = clean;
    invalid[2] = 0x0701;
    invalid[34] = checksum(&invalid);
    assert!(decode(&invalid).is_none());

    invalid = clean;
    invalid[3] = 0;
    invalid[34] = checksum(&invalid);
    assert!(decode(&invalid).is_none());

    invalid = clean;
    invalid[2] = 0x0102;
    invalid[34] = checksum(&invalid);
    assert!(decode(&invalid).is_none());
};

const _: () = {
    let words = encode(3, 51);
    let mut collector = Collector::new();

    assert!(collector.finish().is_none());
    collector.sync(0);
    let mut index = 0;
    while index < WORDS {
        assert!(collector.push(index as u32, words[index]));
        index += 1;
    }
    assert!(collector.count() == WORDS);
    expect(collector.finish(), 3, 51);
    assert!(collector.count() == 0);
    assert!(collector.finish().is_none());

    // A new sync discards an incomplete predecessor.
    collector.sync(100);
    assert!(collector.push(101, 0xFFFF));
    assert!(collector.push(102, 0xFFFF));
    collector.sync(200);
    index = 0;
    while index < WORDS {
        assert!(collector.push(201, words[index]));
        index += 1;
    }
    expect(collector.finish(), 3, 51);

    // Truncation and overflow cannot become an accepted complete frame.
    collector.sync(300);
    index = 0;
    while index < WORDS - 1 {
        assert!(collector.push(301, words[index]));
        index += 1;
    }
    assert!(collector.finish().is_none());

    collector.sync(400);
    index = 0;
    while index < WORDS {
        assert!(collector.push(401, words[index]));
        index += 1;
    }
    assert!(!collector.push(402, 0));
    assert!(collector.finish().is_none());

    // Additional words do not extend the original partial-frame deadline.
    collector.sync(u32::MAX - 500);
    assert!(collector.push(100, words[0]));
    assert!(!collector.expire(498));
    assert!(collector.expire(499));
    assert!(collector.finish().is_none());
};