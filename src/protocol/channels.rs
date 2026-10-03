//! Fixed channel request framing with one immutable staged payload.
//!
//! LF-terminated headers:
//!   TKCE: request current-schema export
//!   TKCI: import schema 1
//!   TKDI: import schema 2
//!   TKEI: import schema 3
//!   TKFI: import schema 4
//!   TKGI: import schema 5
//!   TKHI: import schema 6
//!   TKII: import schema 7
//!   TKJI: import schema 8
//!   TKKI: import schema 9
//!
//! Each import body contains exactly 320 bytes plus four little-endian CRC32
//! bytes. The header fixes its schema. A complete CRC-checked payload remains
//! immutable until the application owner releases it.
//!
//! An import header received while pending starts a discard-only body. It
//! cannot replace bytes or schema, even if the pending payload is released
//! before the discarded body's final byte. Binary mode includes the final CRC
//! byte and excludes ordinary command matching for the complete body.
//!
//! A partial body expires after more than 1000 observed ticks without a byte.
//! Framing reset preserves a complete staged import. No sequence number exists
//! to identify a delayed body tail after resynchronization.
//!
//! The decoder has no peripheral access. The transport supplies bytes, ticks,
//! and loss notifications. The application separately validates channel fields
//! and obtains local confirmation before RAM mutation.

use core::ptr::addr_of_mut;

#[repr(C)]
pub(super) struct Payload {
    pub(super) bytes: [u8; 324],
}

#[used]
#[no_mangle]
static mut TRAKT_CHANNEL_RX: Payload = Payload { bytes: [0; 324] };

pub(crate) enum Event {
    None,
    Export,
    Import,
}

pub(crate) struct Decoder<'a> {
    payload: &'a mut Payload,
    prefix: [u8; 5],
    matched: usize,
    position: usize,
    receiving: bool,
    discard: bool,
    pending: bool,
    version: u8,
    last_byte: u32,
}

impl Decoder<'static> {
    /// Construct once after BSS initialization. The foreground decoder
    /// exclusively owns TRAKT_CHANNEL_RX; neither an ISR nor another
    /// decoder may borrow the buffer while this instance exists.
    pub(crate) unsafe fn new() -> Self {
        Self::with_payload(unsafe { &mut *addr_of_mut!(TRAKT_CHANNEL_RX) })
    }
}

impl<'a> Decoder<'a> {
    pub(super) const fn with_payload(payload: &'a mut Payload) -> Self {
        Self {
            payload,
            prefix: [0; 5],
            matched: 0,
            position: 0,
            receiving: false,
            discard: false,
            pending: false,
            version: 0,
            last_byte: 0,
        }
    }

    /// Includes discard-only bodies. The UART owner must suppress its
    /// ordinary command matcher for every byte consumed in either mode.
    pub(crate) const fn receiving(&self) -> bool {
        self.receiving
    }

    pub(crate) const fn version(&self) -> u8 {
        if self.pending { self.version } else { 0 }
    }

    pub(crate) const fn pending(&self) -> bool {
        self.pending
    }

    pub(crate) const fn data(&self) -> Option<&[u8]> {
        if self.pending {
            Some(self.payload.bytes.split_at(320).0)
        } else {
            None
        }
    }

    /// Releases the staged payload without changing an in-progress
    /// discard decision or resetting command/body framing.
    pub(crate) const fn finish(&mut self) {
        self.pending = false;
    }

    pub(crate) const fn reset_partial(&mut self) {
        self.receiving = false;
        self.discard = false;
        self.position = 0;
        self.matched = 0;
    }

    pub(crate) const fn expire(&mut self, now: u32) {
        if self.receiving && now.wrapping_sub(self.last_byte) > 1000 {
            self.reset_partial();
        }
    }

    pub(crate) const fn byte(&mut self, value: u8, now: u32) -> Event {
        self.last_byte = now;

        if self.receiving {
            if !self.discard {
                self.payload.bytes[self.position] = value;
            }
            self.position += 1;

            if self.position == self.payload.bytes.len() {
                self.receiving = false;
                self.position = 0;

                let discarded = self.discard;
                self.discard = false;

                if !discarded {
                    let expected = u32::from_le_bytes([
                        self.payload.bytes[320], self.payload.bytes[321],
                        self.payload.bytes[322], self.payload.bytes[323],
                    ]);
                    let bytes = self.payload.bytes.split_at(320).0;

                    if crate::storage::calibration::crc32(bytes) == expected {
                        self.pending = true;
                        return Event::Import;
                    }
                }
            }

            return Event::None;
        }

        let matches = match self.matched {
            0 => value == b'T',
            1 => value == b'K',
            2 => matches!(value, b'C' | b'D' | b'E' | b'F' | b'G' | b'H' | b'I' | b'J' | b'K'),
            3 => value == b'I' || (self.prefix[2] == b'C' && value == b'E'),
            4 => value == b'\n',
            _ => false,
        };

        if !matches {
            self.matched = if value == b'T' {
                self.prefix[0] = b'T';
                1
            } else {
                0
            };
            return Event::None;
        }

        self.prefix[self.matched] = value;
        self.matched += 1;

        if self.matched == self.prefix.len() {
            self.matched = 0;

            if self.prefix[3] == b'E' {
                return Event::Export;
            }

            self.position = 0;
            self.receiving = true;
            self.discard = self.pending;
            if !self.discard {
                self.version = match self.prefix[2] {
                    b'C' => 1,
                    b'D' => 2,
                    b'E' => 3,
                    b'F' => 4,
                    b'G' => 5,
                    b'H' => 6,
                    b'I' => 7,
                    b'J' => 8,
                    _ => 9,
                };
            }
        }

        Event::None
    }
}

pub(super) const fn fixture() -> [u8; 324] {
    let mut bytes = [0u8; 324];

    // These bytes intentionally exercise framing, not channel semantics.
    // Command-looking bytes inside a body must remain binary data.
    let commands = b"TKSS\nTKCE\nTKCI\n";
    let mut index = 0;
    while index < commands.len() {
        bytes[index] = commands[index];
        index += 1;
    }

    let crc = crate::storage::calibration::crc32(bytes.split_at(320).0);
    let encoded = crc.to_le_bytes();
    index = 0;
    while index < encoded.len() {
        bytes[320 + index] = encoded[index];
        index += 1;
    }

    bytes
}

const fn feed_quiet(decoder: &mut Decoder<'_>, bytes: &[u8], now: u32) {
    let mut index = 0;
    while index < bytes.len() {
        assert!(matches!(decoder.byte(bytes[index], now), Event::None));
        index += 1;
    }
}

const fn complete(
    decoder: &mut Decoder<'_>,
    bytes: &[u8; 324],
    now: u32,
) -> Event {
    feed_quiet(decoder, bytes.split_at(323).0, now);
    decoder.byte(bytes[323], now)
}

const fn check_staged(decoder: &Decoder<'_>, bytes: &[u8; 324]) {
    assert!(decoder.pending());

    let staged = match decoder.data() {
        Some(staged) => staged,
        None => panic!("staged import missing"),
    };

    assert!(staged.len() == 320);
    let mut index = 0;
    while index < staged.len() {
        assert!(staged[index] == bytes[index]);
        index += 1;
    }
}

const _: () = {
    assert!(core::mem::size_of::<Payload>() == 324);
    assert!(crate::storage::calibration::crc32(b"123456789") == 0xCBF4_3926);

    let packet = fixture();
    let mut payload = Payload { bytes: [0; 324] };
    let mut decoder = Decoder::with_payload(&mut payload);

    assert!(!decoder.receiving());
    assert!(!decoder.pending());
    assert!(decoder.data().is_none());

    // Noise and overlapping first bytes do not lose the next valid prefix.
    feed_quiet(&mut decoder, b"noiseTTKC", 0);
    assert!(matches!(decoder.byte(b'E', 0), Event::None));
    assert!(matches!(decoder.byte(b'\n', 0), Event::Export));
    assert!(!decoder.receiving());

    feed_quiet(&mut decoder, b"TKCX\n", 0);
    feed_quiet(&mut decoder, b"TKCI\n", 0);
    assert!(decoder.receiving() && !decoder.discard);
    assert!(matches!(complete(&mut decoder, &packet, 0), Event::Import));
    assert!(!decoder.receiving());
    check_staged(&decoder, &packet);

    // An occupied buffer drains another complete body without modification.
    let mut unwanted = packet;
    unwanted[0] = b'X';
    feed_quiet(&mut decoder, b"TKCI\n", 1);
    assert!(decoder.receiving() && decoder.discard);
    feed_quiet(&mut decoder, &unwanted, 1);
    assert!(!decoder.receiving());
    check_staged(&decoder, &packet);

    // Explicit parser resynchronization preserves a staged complete import.
    feed_quiet(&mut decoder, b"TKCI\n", 2);
    feed_quiet(&mut decoder, unwanted.split_at(41).0, 2);
    decoder.reset_partial();
    assert!(!decoder.receiving() && !decoder.discard);
    check_staged(&decoder, &packet);

    // Releasing the staged import cannot turn a discard body into a new one.
    feed_quiet(&mut decoder, b"TKCI\n", 3);
    feed_quiet(&mut decoder, unwanted.split_at(16).0, 3);
    decoder.finish();
    assert!(!decoder.pending());
    assert!(decoder.receiving() && decoder.discard);
    feed_quiet(&mut decoder, unwanted.split_at(16).1, 3);
    assert!(!decoder.receiving());
    assert!(decoder.data().is_none());

    feed_quiet(&mut decoder, b"TKCI\n", 4);
    assert!(matches!(complete(&mut decoder, &packet, 4), Event::Import));
    check_staged(&decoder, &packet);

    // Every CRC byte participates in validation.
    let mut index = 320;
    while index < 324 {
        decoder.finish();
        let mut corrupted = packet;
        corrupted[index] ^= 1;

        feed_quiet(&mut decoder, b"TKCI\n", 5);
        feed_quiet(&mut decoder, &corrupted, 5);
        assert!(!decoder.receiving());
        assert!(!decoder.pending());
        index += 1;
    }

    let mut corrupted = packet;
    corrupted[319] ^= 1;
    feed_quiet(&mut decoder, b"TKCI\n", 6);
    feed_quiet(&mut decoder, &corrupted, 6);
    assert!(!decoder.pending());

    // Timeout arithmetic crosses the tick counter's wrapping boundary.
    let start = u32::MAX - 500;
    feed_quiet(&mut decoder, b"TKCI\n", start);
    feed_quiet(&mut decoder, packet.split_at(10).0, start);
    decoder.expire(499);
    assert!(decoder.receiving());
    decoder.expire(500);
    assert!(!decoder.receiving() && !decoder.pending());

    feed_quiet(&mut decoder, b"TKCI\n", 600);
    assert!(matches!(complete(&mut decoder, &packet, 600), Event::Import));
    decoder.reset_partial();
    decoder.expire(9000);
    check_staged(&decoder, &packet);

    // Truncation around slot, DMA-ring, payload, and CRC boundaries.
    let lengths = [0usize, 1, 19, 20, 255, 256, 319, 320, 321, 322, 323];
    index = 0;
    while index < lengths.len() {
        let mut work = Payload { bytes: [0; 324] };
        let mut partial = Decoder::with_payload(&mut work);

        feed_quiet(&mut partial, b"TKCI\n", 0);
        feed_quiet(&mut partial, packet.split_at(lengths[index]).0, 0);
        assert!(partial.receiving() && !partial.pending());

        partial.expire(1000);
        assert!(partial.receiving());
        partial.expire(1001);
        assert!(!partial.receiving() && !partial.pending());

        feed_quiet(&mut partial, b"TKCE", 1002);
        assert!(matches!(partial.byte(b'\n', 1002), Event::Export));
        index += 1;
    }
};