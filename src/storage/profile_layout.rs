//! Packed payload codec over a borrowed journal buffer.
//!
//! The payload occupies 448 bytes:
//!   0..95: twenty-four global words
//!   96..127: VFO A and B, each a twelve-byte profile plus other-band frequency
//!   128..447: sixteen twenty-byte channels, profile followed by eight name bytes
//!
//! Global word seven stores power in its low nibble and the checked
//! twenty-eight-bit CW control word above it. Its highest three bits select
//! optional receive windows during an admitted CW session. No native struct
//! or additional global word is serialized. Historical restrictions belong
//! to record; active RF ownership is never restored from these fields.
//!
//! Global word six combines sound choices in bits 0..23, microphone choices
//! in 24..25, and the checked CW local-audio word in 26..30. Bit 31 is reserved.
//! The local-audio defaults use a zero extension. Record admission determines
//! which historical formats assign this extension; packing does not migrate
//! a live fade or retain hardware state.
//!
//! Global words 0..7 map Snapshot words 0..7. Global words 8..23 map Snapshot
//! words 96..111. Global word 22 stores HAM disable in bit zero and the
//! checked twenty-one-bit VOX choice word in bits 1..21. Bits 22..31 remain
//! reserved. Zero selects HAM On and VOX Off with default threshold and hold.
//! VOX choice persistence does not provide microphone observations or TX
//! permission. This assignment does not enlarge Snapshot or payload.
//!
//! Global word 23 stores DW enable in bit zero and the checked twelve-bit
//! SlopeCast word in bits
//! 1..12. Bits 13..31 store the checked nineteen-bit CW settings word,
//! including QSK disable in bit 31. Zero selects HAM On, DW Off, disabled
//! SlopeCast, and default CW choices with RF enabled. Every composition
//! retains the complete word. Historical bit restrictions belong to record.
//!
//! VFO profile word zero additionally stores CW in bit 31. This is a
//! VFO-envelope field, not an extension of Tune or channel-wire encoding.
//! AM and CW cannot both be set. Channel profile word zero still reserves
//! bit 31 and current channel slots cannot carry CW.
//!
//! Snapshot words 112..113 map the VFO route words; 114..129 map channel
//! routes. Snapshot signaling pairs occupy 130..131 for VFOs and 132..147
//! for channels. RAM pairs store TX low and RX high. Packed signaling stores
//! RX low and TX high. ctcss performs the checked half-word conversion.
//!
//! A packed profile stores ten-hertz RX units, width, AM, and step in its first
//! word, route in its second, and directional signaling in its third.
//! Other-band positions remain full little-endian hertz values. Every byte of
//! an empty channel slot must be zero. Populated names use eight printable
//! ASCII bytes padded with spaces. No native struct layout is serialized.
//!
//! Pack requires an exact output length and a valid Snapshot before mutation.
//! Unpack checks an exact input length, constructs a detached Snapshot, and
//! returns it only after complete validation. Hidden module preferences are
//! retained, not filtered according to the running composition.
//!
//! The codec performs no I/O, version selection, CRC, commit, or generation
//! handling. The record owner applies format-specific field restrictions
//! before passing a record to trusted snapshot extraction.

use super::record::Snapshot;
use crate::profile::Tune;

pub(super) const PAYLOAD_BYTES: usize = 448;
pub(super) const VFO_START: usize = 96;
pub(super) const CHANNEL_START: usize = 128;
pub(super) const CHANNEL_BYTES: usize = 20;

const GLOBAL_WORDS: [usize; 24] = [
    0, 1, 2, 3, 4, 5, 6, 7,
    96, 97, 98, 99, 100, 101, 102, 103,
    104, 105, 106, 107, 108, 109, 110, 111,
];

const fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3],
    ])
}

const fn put(bytes: &mut [u8], offset: usize, value: u32) {
    let encoded = value.to_le_bytes();
    let mut index = 0;
    while index < encoded.len() {
        bytes[offset + index] = encoded[index];
        index += 1;
    }
}

/// The complete Snapshot has already passed validation. Only the first
/// profile word belongs to this helper. The caller writes route and signal
/// words directly, without first constructing and storing simplex zeroes.
const fn put_tune(bytes: &mut [u8], offset: usize, tune: Tune) {
    let word = match tune.word() {
        Ok(word) => word,
        Err(_) => panic!("validated snapshot contains invalid tune"),
    };
    put(bytes, offset, word);
}

/// Structural parsing only: the tuning word must have known step and flag
/// encodings. RX intervals, routes, signaling pairs, and names are checked
/// together by Snapshot::valid before unpack can return any state.
const fn get_tune(bytes: &[u8], offset: usize) -> Option<Tune> {
    match Tune::decode_fields(word(bytes, offset)) {
        Ok(tune) => Some(tune),
        Err(_) => None,
    }
}

#[inline(never)]
pub(super) const fn pack(
    state: &Snapshot,
    bytes: &mut [u8],
) -> bool {
    if bytes.len() != PAYLOAD_BYTES || !state.valid() {
        return false;
    }

    let mut index = 0;
    while index < bytes.len() {
        bytes[index] = 0;
        index += 1;
    }

    index = 0;
    while index < GLOBAL_WORDS.len() {
        put(bytes, index * 4, state.words[GLOBAL_WORDS[index]]);
        index += 1;
    }

    index = 0;
    while index < 2 {
        let source = 8 + index * 4;
        let target = VFO_START + index * 16;
        put_tune(bytes, target, Tune {
            frequency: state.words[source],
            step: state.words[source + 2],
            am: state.words[source + 3] & 2 != 0, narrow: state.words[source + 3] & 1 != 0,
        });
        if state.words[source + 3] & 4 != 0 {
            let tagged = word(bytes, target) | (1 << 31);
            put(bytes, target, tagged);
        }
        put(bytes, target + 4, state.words[112 + index]);
        put(bytes, target + 8, crate::ctcss::wire(state.words[130 + index]));
        put(bytes, target + 12, state.words[source + 1]);
        index += 1;
    }

    index = 0;
    while index < 16 {
        let source = 16 + index * 5;
        let target = CHANNEL_START + index * CHANNEL_BYTES;

        if state.words[source] != 0 {
            put_tune(bytes, target, Tune {
                frequency: state.words[source],
                step: state.words[source + 1],
                am: state.words[source + 2] & 2 != 0, narrow: state.words[source + 2] & 1 != 0,
            });
            put(bytes, target + 4, state.words[114 + index]);
            put(bytes, target + 8, crate::ctcss::wire(state.words[132 + index]));
            put(bytes, target + 12, state.words[source + 3]);
            put(bytes, target + 16, state.words[source + 4]);
        }

        index += 1;
    }

    true
}

#[inline(never)]
pub(super) const fn unpack(bytes: &[u8]) -> Option<Snapshot> {
    if bytes.len() != PAYLOAD_BYTES {
        return None;
    }

    let mut state = Snapshot { words: [0; 148] };

    let mut index = 0;
    while index < GLOBAL_WORDS.len() {
        state.words[GLOBAL_WORDS[index]] = word(bytes, index * 4);
        index += 1;
    }

    index = 0;
    while index < 2 {
        let source = VFO_START + index * 16;
        let target = 8 + index * 4;
        let tune = match Tune::decode_fields(word(bytes, source) & !(1 << 31)) {
            Ok(tune) => tune,
            Err(_) => return None,
        };
        state.words[130 + index] = word(bytes, source + 8).rotate_left(16);

        state.words[target] = tune.frequency;
        state.words[target + 1] = word(bytes, source + 12);
        state.words[target + 2] = tune.step;
        state.words[target + 3] = tune.flags()
            | ((word(bytes, source) >> 29) & 4);
        state.words[112 + index] = word(bytes, source + 4);
        index += 1;
    }

    index = 0;
    while index < 16 {
        let source = CHANNEL_START + index * CHANNEL_BYTES;
        let target = 16 + index * 5;
        let mut empty = true;
        let mut byte = 0;

        while byte < CHANNEL_BYTES {
            empty = empty && bytes[source + byte] == 0;
            byte += 1;
        }

        if !empty {
            let tune = match get_tune(bytes, source) {
                Some(tune) => tune,
                None => return None,
            };
            state.words[target] = tune.frequency;
            state.words[target + 1] = tune.step;
            state.words[target + 2] = tune.flags();
            state.words[114 + index] = word(bytes, source + 4);
            state.words[132 + index] = word(bytes, source + 8).rotate_left(16);
            state.words[target + 3] = word(bytes, source + 12);
            state.words[target + 4] = word(bytes, source + 16);
        }

        index += 1;
    }

    if state.valid() { Some(state) } else { None }
}

const _: () = {
    assert!(GLOBAL_WORDS.len() * 4 == VFO_START);
    assert!(VFO_START + 2 * 16 == CHANNEL_START);
    assert!(CHANNEL_START + 16 * CHANNEL_BYTES == PAYLOAD_BYTES);
    assert!(32 + PAYLOAD_BYTES == 480);
    assert!(32 + PAYLOAD_BYTES <= 504);
    assert!(508 + 4 == 512);

    // Every current Snapshot word belongs to exactly one region.
    let mut covered = [false; 148];
    let mut route = 112;
    while route < 148 {
        covered[route] = true;
        route += 1;
    }
    let mut index = 0;
    while index < GLOBAL_WORDS.len() {
        let word = GLOBAL_WORDS[index];
        assert!(word < covered.len() && !covered[word]);
        covered[word] = true;
        index += 1;
    }

    index = 8;
    while index < 96 {
        assert!(!covered[index]);
        covered[index] = true;
        index += 1;
    }

    index = 0;
    while index < covered.len() {
        assert!(covered[index]);
        index += 1;
    }
};
