//! Settings record admission, serialization, and recovery selection.
//!
//! Each sector contains a 512-byte record followed by erased bytes. The
//! header stores TKD1, format, payload length, and generation. Payload starts
//! at byte 32. CRC32 at 504 covers bytes 0..503; TOK1 at 508 commits the record.
//! Unassigned header and trailing body bytes must be FF.
//!
//! The writer emits format 19 with a 448-byte packed payload. The application
//! Snapshot contains 148 words; its RAM representation is not copied directly
//! to Flash. profile_layout maps global settings, VFOs, and channel profiles.
//!
//! Accepted input contracts:
//!   Format  Payload  Profile layout and admitted signaling
//!   1       256 B    Linear fields; unnamed twelve-byte simplex channels
//!   2..4    384 B    Linear fields; twenty-byte named simplex channels
//!   5       432 B    Linear fields including scan and autosave preferences
//!   6..7    448 B    Linear fields including presentation choices
//!   8       448 B    Packed simplex profiles; signaling words zero
//!   9       448 B    Packed VFO routes; channel routes and signaling zero
//!   10      448 B    Packed VFO and channel routes; signaling zero
//!   11      448 B    TX CTCSS; RX signaling zero
//!   12      448 B    Independent RX/TX CTCSS
//!   13      448 B    TX CTCSS/DCS; RX CTCSS
//!   14..15  448 B    Independent RX/TX CTCSS/DCS
//!   16      448 B    AM selector and 108..136 MHz receive extension
//!   17      448 B    RX coverage 50 MHz through 629999990 Hz
//!   18      448 B    Upper RX interval; packed frequency bit 26 in bit 30
//!   19      448 B    SlopeCast settings above the saved DW bit
//!
//! Additional field constraints apply independently of payload length:
//! sound word 6 is zero in formats 1..2; power word 7 is zero in 1..3 and
//! limited to Low1..Low5 in 4..6. Format 6 requires timeout word 109 zero.
//! The application supplies 30-second TOT when the input has no assigned TOT.
//! Unassigned preferences use checked defaults; unnamed channels receive spaces.
//!
//! Saved HAM is admitted by packed formats 8..19; other admitted inputs
//! enable the filter. Saved DW is admitted by formats 15 through 19. Its global
//! word 23 bit zero stores DW. Format 19 stores checked SlopeCast settings
//! in bits 1..12. Earlier packed formats reserve those higher bits. Formats
//! 6..14 reserve the complete DW word. Zero SlopeCast bits select disabled,
//! lower-side, 5.0 kHz, wide TX defaults without a migration write.
//! Runtime RX, monitor, scan position, receiver ownership, live deadlines,
//! and pending confirmation steps are outside the persistent representation.
//!
//! Field restrictions are checked even when a record has a matching CRC.
//! A recognized incomplete record is reclaimable, but cannot become the
//! selected saved state without commit and complete validation. Unknown
//! formats are not reclaimed. Equal generations with differing CRCs and
//! half-range generation ambiguity are errors, not sector-order tie breakers.

const MAGIC: u32 = 0x3144_4B54;
pub(crate) const COMMIT: u32 = 0x314B_4F54;

#[derive(Clone, Copy)]
pub(crate) struct Snapshot {
    /// Words 0..111 retain the application settings representation.
    /// Words 112 and 113 encode VFO A and B transmit routes.
    /// Words 114..129 encode the sixteen channel routes; empty slots use zero.
    /// Words 130..131 store VFO tone pairs, 132..147 channel tone pairs.
    /// Each RAM word has TX in its low half and RX in its high half,
    /// each encoded as CTCSS tenths of a hertz or tagged DCS.
    /// Empty channels require a zero pair. Width words use bit zero for
    /// narrow and bit one for AM; all other width bits are rejected.
    pub(crate) words: [u32; 148],
}

const fn band(value: u32) -> u32 {
    match crate::profile::Band::receive(value) {
        Some(crate::profile::Band::Vhf) => 1,
        Some(crate::profile::Band::Uhf) => 2,
        None => 0,
    }
}

const fn step(value: u32) -> bool {
    matches!(value, 12_500 | 25_000 | 100_000)
}

const fn printable(word: u32) -> bool {
    let bytes = word.to_le_bytes();
    let mut index = 0;
    while index < 4 {
        if bytes[index] < 32 || bytes[index] > 126 {
            return false;
        }
        index += 1;
    }
    true
}

/// Frequency vocabulary for admitted historical record formats.
/// The current format is handled before this helper is called.
#[inline(never)]
const fn historical_frequency(frequency: u32, version: u32) -> bool {
    match version {
        1..=15 => crate::profile::Band::of(frequency).is_some(),
        16 => crate::profile::Band::receive_v16(frequency).is_some(),
        17 => crate::profile::Band::receive_v17(frequency).is_some(),
        _ => false,
    }
}

impl Snapshot {
    /// Additional historical restrictions, not complete Snapshot validation.
    /// The record decoder calls this only after validating the current field
    /// representation. Format 18 needs no narrower historical vocabulary.
    ///
    /// Older formats validate both positions of each VFO, every populated
    /// channel, and both scan bounds. Formats before 16 additionally forbid
    /// the AM flag. Empty-slot shape and all nonfrequency settings belong
    /// to Snapshot::valid and cannot be admitted by this helper alone.
    #[inline(never)]
    const fn compatible(&self, version: u32) -> bool {
        if version >= 18 {
            return true;
        }

        let w = &self.words;
        let mut at = 8;
        while at < 16 {
            if !historical_frequency(w[at], version)
                || !historical_frequency(w[at + 1], version)
                || (version < 16 && w[at + 3] > 1)
            {
                return false;
            }
            at += 4;
        }

        while at < 96 {
            if w[at] != 0
                && (!historical_frequency(w[at], version)
                    || (version < 16 && w[at + 2] > 1))
            {
                return false;
            }
            at += 5;
        }

        historical_frequency(w[100], version)
            && historical_frequency(w[101], version)
    }

    /// Format 17 has no upper RX interval. All frequency-bearing regions
    /// are checked without constructing another Snapshot-sized object.
    #[inline(never)]
    pub(crate) const fn rx17(&self) -> bool {
        self.compatible(17)
    }

    /// Format 16 admits AM but retains its original RX intervals.
    /// Check both VFO positions, populated channels, and scan bounds.
    /// Width and signaling restrictions are checked by the other validators.
    #[inline(never)]
    pub(crate) const fn rx16(&self) -> bool {
        self.compatible(16)
    }

    /// Historical records have neither AM nor aviation RX. Check their
    /// vocabulary separately from the current logical representation so
    /// a recomputed CRC cannot extend an older format's field meanings.
    #[inline(never)]
    pub(crate) const fn legacy(&self) -> bool {
        self.compatible(15)
    }

    pub(crate) const fn valid(&self) -> bool {
        let w = &self.words;
        if w[0] > 1 || !matches!(w[1], 125 | 250 | 500)
            || w[2] > 1 || w[3] > 15 || w[4] > 1 || w[5] > 9
            || crate::app::sound::Settings::decode(w[6]).is_none()
            || w[7] >= crate::app::tx_options::POWER_COUNT as u32
        {
            return false;
        }

        let mut tone = 130;
        while tone < 148 {
            if !crate::ctcss::pair_valid(w[tone]) { return false; }
            tone += 1;
        }

        let mut index = 8;
        while index < 16 {
            let current = band(w[index]);
            let other = band(w[index + 1]);
            if current == 0 || other == 0 || current == other
                || !step(w[index + 2]) || w[index + 3] > 3
            {
                return false;
            }
            index += 4;
        }

        while index < 96 {
            let route = w[114 + (index - 16) / 5];
            if crate::profile::Tx::decode(route).is_err()
                || (w[index] == 0
                    && (route != 0 || w[132 + (index - 16) / 5] != 0))
            {
                return false;
            }
            if w[index] == 0 {
                if w[index + 1] != 0 || w[index + 2] != 0
                    || w[index + 3] != 0 || w[index + 4] != 0
                {
                    return false;
                }
            } else if band(w[index]) == 0 || !step(w[index + 1])
                || w[index + 2] > 3
                || !printable(w[index + 3]) || !printable(w[index + 4])
            {
                return false;
            }
            index += 5;
        }
        crate::app::preferences::Preferences::read(w).is_some()
            && crate::app::appearance::Settings::decode(w[108]).is_some()
            && w[109] < crate::app::tx_options::timeout_count()
            && w[110] <= 1
            && crate::slope::Settings::decode(w[111] >> 1).is_some()
            && crate::profile::Tx::decode(w[112]).is_ok()
            && crate::profile::Tx::decode(w[113]).is_ok()
    }
}

pub(crate) const fn word(bytes: &[u8; 512], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3],
    ])
}

pub(crate) const fn put(bytes: &mut [u8; 512], offset: usize, value: u32) {
    let encoded = value.to_le_bytes();
    let mut index = 0;
    while index < 4 {
        bytes[offset + index] = encoded[index];
        index += 1;
    }
}

/// Header and body occupy bytes 0..503. The stored CRC and independent
/// commit word are outside that range. Borrowing the prefix creates no
/// record-sized temporary and does not change format-specific CRC coverage.
const fn checksum(bytes: &[u8; 512]) -> u32 {
    super::calibration::crc32(bytes.split_at(504).0)
}

const _: () = {
    let original = [0xA5u8; 512];
    let expected = checksum(&original);

    // Neither CRC storage nor commit storage participates in the checksum.
    let mut metadata = original;
    let mut index = 504;
    while index < metadata.len() {
        metadata[index] ^= 0xFF;
        index += 1;
    }
    assert!(checksum(&metadata) == expected);

    // Both ends of the covered region participate.
    let mut first = original;
    first[0] ^= 1;
    assert!(checksum(&first) != expected);

    let mut last = original;
    last[503] ^= 1;
    assert!(checksum(&last) != expected);
};

pub(crate) const fn owned(bytes: &[u8; 512]) -> bool {
    word(bytes, 0) == MAGIC
        && ((word(bytes, 4) == 1 && word(bytes, 8) == 256)
            || (matches!(word(bytes, 4), 2 | 3 | 4) && word(bytes, 8) == 384)
            || (word(bytes, 4) == 5 && word(bytes, 8) == 432)
            || (matches!(word(bytes, 4), 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19) && word(bytes, 8) == 448))
}

/// Decodes a recognized body into the current RAM shape. Callers must
/// validate commit, CRC, padding, and field constraints before applying it.
#[inline(never)]
pub(crate) const fn snapshot(bytes: &[u8; 512]) -> Snapshot {
    assert!(owned(bytes));

    if matches!(word(bytes, 4), 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19) {
        let payload = bytes.split_at(32).1
            .split_at(super::profile_layout::PAYLOAD_BYTES).0;
        return match super::profile_layout::unpack(payload) {
            Some(state) => state,
            None => panic!("invalid packed snapshot"),
        };
    }

    let mut state = Snapshot { words: [0; 148] };
    crate::app::preferences::Preferences::new().write(&mut state.words);
    state.words[108] = crate::app::appearance::Settings::new().word();
    state.words[109] = crate::app::tx_options::DEFAULT_TOT as u32;

    let mut index = 0;
    while index < 16 {
        state.words[index] = word(bytes, 32 + index * 4);
        index += 1;
    }

    if word(bytes, 4) >= 2 {
        let count = if word(bytes, 4) >= 6 {
            112
        } else if word(bytes, 4) >= 5 {
            108
        } else {
            96
        };

        while index < count {
            state.words[index] = word(bytes, 32 + index * 4);
            index += 1;
        }
    } else {
        let mut slot = 0;
        while slot < 16 {
            let source = 32 + (16 + slot * 3) * 4;
            let target = 16 + slot * 5;
            state.words[target] = word(bytes, source);
            state.words[target + 1] = word(bytes, source + 4);
            state.words[target + 2] = word(bytes, source + 8);

            if state.words[target] != 0 {
                state.words[target + 3] = 0x2020_2020;
                state.words[target + 4] = 0x2020_2020;
            }
            slot += 1;
        }
    }

    if word(bytes, 4) < 7 {
        state.words[109] = crate::app::tx_options::DEFAULT_TOT as u32;
    }

    state
}

/// Writes version nineteen directly into caller-owned journal storage.
/// The payload borrows its exact region without a second payload array.
/// Commit remains erased until the independent commit operation.
#[inline(never)]
pub(crate) const fn encode_into(
    bytes: &mut [u8; 512],
    state: &Snapshot,
    generation: u32,
) {
    assert!(state.valid());

    let mut index = 0;
    while index < bytes.len() {
        bytes[index] = 255;
        index += 1;
    }

    put(bytes, 0, MAGIC);
    put(bytes, 4, 19);
    put(bytes, 8, super::profile_layout::PAYLOAD_BYTES as u32);
    put(bytes, 12, generation);

    let payload = bytes.split_at_mut(32).1
        .split_at_mut(super::profile_layout::PAYLOAD_BYTES).0;
    assert!(super::profile_layout::pack(state, payload));

    let crc = checksum(bytes);
    put(bytes, 504, crc);
}

/// Value-returning form used by compiler-evaluated format tests.
pub(crate) const fn encode(state: Snapshot, generation: u32) -> [u8; 512] {
    let mut bytes = [255u8; 512];
    encode_into(&mut bytes, &state, generation);
    bytes
}

pub(crate) const fn erased(bytes: &[u8; 512]) -> bool {
    let mut index = 0;
    while index < 512 {
        if bytes[index] != 255 { return false; }
        index += 1;
    }
    true
}

pub(crate) const fn valid(bytes: &[u8; 512], committed: bool) -> bool {
    if !owned(bytes)
        || word(bytes, 508) != if committed { COMMIT } else { u32::MAX }
    {
        return false;
    }

    if word(bytes, 4) < 3 && word(bytes, 56) != 0 {
        return false;
    }
    if word(bytes, 4) < 4 && word(bytes, 60) != 0 {
        return false;
    }

    let mut index = 16;
    while index < 32 {
        if bytes[index] != 255 { return false; }
        index += 1;
    }

    index = 32 + word(bytes, 8) as usize;
    while index < 504 {
        if bytes[index] != 255 { return false; }
        index += 1;
    }

    // Format-specific reserved fields remain reserved even when the record CRC
    // is recalculated. Field admission is separate from checksum integrity.
    if word(bytes, 4) < 7 && word(bytes, 60) > 4 {
        return false;
    }

    if word(bytes, 4) == 6 && word(bytes, 468) != 0 {
        return false;
    }

    // Linear formats 6 and 7 reserve their HAM and DW words. Both must be zero;
    // migration initializes the corresponding saved choices from defaults.
    if matches!(word(bytes, 4), 6 | 7)
        && (word(bytes, 472) != 0 || word(bytes, 476) != 0)
    {
        return false;
    }

    if word(bytes, 504) != checksum(bytes) {
        return false;
    }

    if matches!(word(bytes, 4), 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19) {
        let version = word(bytes, 4);

        // Formats 14 and later use the complete current route/signaling
        // vocabulary. Earlier formats inspect each profile once for all
        // additional restrictions before current Snapshot admission.
        if version < 14 {
            let mut index = 0;
            while index < 18 {
                let at = if index < 2 {
                    32 + super::profile_layout::VFO_START + index * 16
                } else {
                    32 + super::profile_layout::CHANNEL_START
                        + (index - 2) * super::profile_layout::CHANNEL_BYTES
                };

                let route_reserved = if index < 2 {
                    version < 9
                } else {
                    version < 10
                };
                if route_reserved && word(bytes, at + 4) != 0 {
                    return false;
                }

                let signal = word(bytes, at + 8);
                if (version < 11 && signal != 0)
                    || (version == 11 && signal & 65535 != 0)
                    || (version < 13 && !crate::ctcss::valid(signal >> 16))
                    || (version < 14 && !crate::ctcss::valid(signal & 65535))
                {
                    return false;
                }

                index += 1;
            }
        }

        // Only format 19 assigns the higher bits of the shared DW word.
        if word(bytes, 4) < 19 && word(bytes, 124) & !1 != 0 {
            return false;
        }

        // Packed formats without saved DW require global word 23 to be zero.
        if word(bytes, 4) < 15 && word(bytes, 32 + 92) != 0 {
            return false;
        }

        let payload = bytes.split_at(32).1
            .split_at(super::profile_layout::PAYLOAD_BYTES).0;
        match super::profile_layout::unpack(payload) {
            Some(state) => state.compatible(word(bytes, 4)),
            None => false,
        }
    } else {
        let state = snapshot(bytes);
        state.valid() && state.legacy()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Meta {
    pub(crate) erased: bool,
    pub(crate) owned: bool,
    pub(crate) valid: bool,
    pub(crate) generation: u32,
    pub(crate) crc: u32,
    pub(crate) version: u32,
}

impl Meta {
    pub(crate) const fn unknown() -> Self {
        Self {
            erased: false, owned: false, valid: false,
            generation: 0, crc: 0, version: 0,
        }
    }

    pub(crate) const fn inspect(bytes: &[u8; 512]) -> Self {
        Self {
            erased: erased(bytes),
            owned: owned(bytes),
            valid: valid(bytes, true),
            generation: word(bytes, 12),
            crc: word(bytes, 504),
            version: if owned(bytes) { word(bytes, 4) } else { 0 },
        }
    }

    pub(crate) const fn reclaimable(self) -> bool {
        self.erased || self.owned
    }
}

pub(crate) const fn choose(a: Meta, b: Meta) -> Result<Option<usize>, u32> {
    if !a.valid && !b.valid { return Ok(None); }
    if a.valid && !b.valid { return Ok(Some(0)); }
    if !a.valid && b.valid { return Ok(Some(1)); }

    let distance = b.generation.wrapping_sub(a.generation);
    if distance == 0 {
        if a.crc != b.crc { return Err(604); }
        return Ok(Some(0));
    }
    if distance == 0x8000_0000 { return Err(604); }
    Ok(Some(if distance < 0x8000_0000 { 1 } else { 0 }))
}

const fn fixture() -> Snapshot {
    let mut state = Snapshot { words: [0; 148] };
    crate::app::preferences::Preferences::new().write(&mut state.words);
    state.words[108] = crate::app::appearance::Settings::new().word();
    state.words[109] = crate::app::tx_options::DEFAULT_TOT as u32;
    state.words[0] = 1;
    state.words[1] = 250;
    state.words[5] = 3;
    state.words[8] = 145_500_000;
    state.words[9] = 433_500_000;
    state.words[10] = 12_500;
    state.words[12] = 433_500_000;
    state.words[13] = 145_500_000;
    state.words[14] = 12_500;

    state.words[16] = 145_500_000;
    state.words[17] = 12_500;
    state.words[18] = 0;
    state.words[19] = u32::from_le_bytes(*b"VHF ");
    state.words[20] = u32::from_le_bytes(*b"    ");

    state.words[21] = 433_500_000;
    state.words[22] = 25_000;
    state.words[23] = 1;
    state.words[24] = u32::from_le_bytes(*b"UHF ");
    state.words[25] = u32::from_le_bytes(*b"    ");
    state
}

/// Check a field at its first admitted packed format and at the preceding
/// format. Both records have a correct CRC and commit. Only the field
/// vocabulary changes; checksum validity cannot grant a reserved meaning.
const fn check_profile_boundary(version: u32, offset: usize, value: u32) {
    let mut bytes = encode(fixture(), 700);
    put(&mut bytes, 4, version);
    put(&mut bytes, offset, value);
    let crc = checksum(&bytes);
    put(&mut bytes, 504, crc);
    put(&mut bytes, 508, COMMIT);
    assert!(valid(&bytes, true));

    put(&mut bytes, 4, version - 1);
    let crc = checksum(&bytes);
    put(&mut bytes, 504, crc);
    assert!(!valid(&bytes, true));
}

const _: () = check_profile_boundary(
    9, 32 + super::profile_layout::VFO_START + 4, 4 << 26,
);
const _: () = check_profile_boundary(
    10, 32 + super::profile_layout::CHANNEL_START + 4, 4 << 26,
);
const _: () = check_profile_boundary(
    11, 32 + super::profile_layout::VFO_START + 8, 1000 << 16,
);
const _: () = check_profile_boundary(
    11, 32 + super::profile_layout::CHANNEL_START + 8, 1000 << 16,
);
const _: () = check_profile_boundary(
    12, 32 + super::profile_layout::VFO_START + 8, 885,
);
const _: () = check_profile_boundary(
    12, 32 + super::profile_layout::CHANNEL_START + 8, 885,
);
const _: () = check_profile_boundary(
    13, 32 + super::profile_layout::VFO_START + 8, 0x8013_0000,
);
const _: () = check_profile_boundary(
    13, 32 + super::profile_layout::CHANNEL_START + 8, 0x8013_0000,
);
const _: () = check_profile_boundary(
    14, 32 + super::profile_layout::VFO_START + 8, 0xC015,
);
const _: () = check_profile_boundary(
    14, 32 + super::profile_layout::CHANNEL_START + 8, 0xC015,
);

/// Compiler fixtures exercise format-specific field admission and complete
/// record validation, including required zeroes in reserved positions.
const _: () = {
    use crate::profile::Tx;

    // With zero routes the packed payload is a genuine v8 payload.
    let mut old = encode(fixture(), 80);
    put(&mut old, 4, 8);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));

    let mut state = snapshot(&old);
    assert!(state.words[112] == 0 && state.words[113] == 0);

    state.words[112] = match Tx::Minus(600_000).word() {
        Ok(word) => word,
        Err(_) => panic!("VFO A route rejected"),
    };
    state.words[113] = match Tx::Split(145_700_000).word() {
        Ok(word) => word,
        Err(_) => panic!("VFO B route rejected"),
    };
    state.words[110] = 1;
    assert!(state.valid());

    let mut next = encode(state, 81);
    assert!(valid(&next, false));
    assert!(matches!(choose(Meta::inspect(&old), Meta::inspect(&next)), Ok(Some(0))));
    put(&mut next, 508, COMMIT);
    assert!(valid(&next, true));
    assert!(matches!(choose(Meta::inspect(&old), Meta::inspect(&next)), Ok(Some(1))));

    let restored = snapshot(&next);
    let mut index = 0;
    while index < state.words.len() {
        assert!(state.words[index] == restored.words[index]);
        index += 1;
    }

    // Recomputed CRC cannot give v8 a non-simplex VFO meaning.
    let mut disguised = next;
    put(&mut disguised, 4, 8);
    let crc = checksum(&disguised);
    put(&mut disguised, 504, crc);
    assert!(!valid(&disguised, true));

    state.words[112] = match Tx::ReceiveOnly.word() {
        Ok(word) => word,
        Err(_) => panic!("RX-only route rejected"),
    };
    let encoded = encode(state, 82);
    assert!(valid(&encoded, false));
    assert!(matches!(
        Tx::decode(snapshot(&encoded).words[112]),
        Ok(Tx::ReceiveOnly),
    ));

    // Invalid effective TX remains stored; transmission must reject it.
    state.words[8] = 174_000_000;
    state.words[112] = match Tx::Plus(600_000).word() {
        Ok(word) => word,
        Err(_) => panic!("blocked offset rejected as data"),
    };
    assert!(state.valid());
    let encoded = encode(state, 83);
    assert!(valid(&encoded, false));
    assert!(snapshot(&encoded).words[112] == state.words[112]);

    state.words[112] = 7 << 26;
    assert!(!state.valid());
};

const _: () = {
    use crate::profile::Tx;

    let mut state = fixture();
    let mut old = encode(state, 90);
    put(&mut old, 4, 9);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));
    let migrated = snapshot(&old);
    let mut slot = 0;
    while slot < 16 {
        assert!(migrated.words[114 + slot] == 0);
        slot += 1;
    }

    let routes = [
        Tx::Simplex, Tx::Plus(600_000), Tx::Minus(600_000),
        Tx::Split(433_700_000), Tx::ReceiveOnly,
    ];
    slot = 0;
    while slot < 16 {
        let at = 16 + slot * 5;
        state.words[at] = 145_700_000;
        state.words[at + 1] = 12_500;
        state.words[at + 2] = (slot & 1) as u32;
        state.words[at + 3] = u32::from_le_bytes(*b"ROUTE   ".split_at(4).0.first_chunk::<4>().unwrap());
        state.words[at + 4] = u32::from_le_bytes(*b"    ");
        state.words[114 + slot] = match routes[slot % routes.len()].word() {
            Ok(word) => word,
            Err(_) => panic!("channel route fixture rejected"),
        };
        slot += 1;
    }
    assert!(state.valid());

    let mut saved = encode(state, 91);
    assert!(valid(&saved, false));
    assert!(matches!(choose(Meta::inspect(&old), Meta::inspect(&saved)), Ok(Some(0))));
    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    let restored = snapshot(&saved);
    let mut index = 0;
    while index < state.words.len() {
        assert!(state.words[index] == restored.words[index]);
        index += 1;
    }

    // Format-specific labels cannot acquire routed channels through a new CRC.
    let mut version = 8;
    while version <= 9 {
        let mut disguised = saved;
        put(&mut disguised, 4, version);
        let crc = checksum(&disguised);
        put(&mut disguised, 504, crc);
        assert!(!valid(&disguised, true));
        version += 1;
    }

    // Empty slots cannot carry a hidden route.
    let mut empty = fixture();
    empty.words[114 + 15] = 4 << 26;
    assert!(!empty.valid());
};

const _: () = {
    // A zero-signaling payload is a genuine v10 payload.
    let mut old = encode(fixture(), 110);
    put(&mut old, 4, 10);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));

    let mut state = snapshot(&old);
    let mut index = 130;
    while index < 148 {
        assert!(state.words[index] == 0);
        index += 1;
    }

    state.words[130] = 670;
    state.words[131] = 2541;
    state.words[132] = 1000;
    state.words[133] = 885;
    assert!(state.valid());

    let mut saved = encode(state, 111);
    assert!(valid(&saved, false));
    assert!(matches!(choose(Meta::inspect(&old), Meta::inspect(&saved)), Ok(Some(0))));
    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    let loaded = snapshot(&saved);
    index = 0;
    while index < state.words.len() {
        assert!(loaded.words[index] == state.words[index]);
        index += 1;
    }

    let mut disguised = saved;
    put(&mut disguised, 4, 10);
    let crc = checksum(&disguised);
    put(&mut disguised, 504, crc);
    assert!(!valid(&disguised, true));

    state.words[130] = 1001;
    assert!(!state.valid());
    state.words[130] = 1000;
    state.words[147] = 1000;
    assert!(!state.valid());

    let mut bad = saved;
    put(&mut bad, 32 + super::profile_layout::VFO_START + 8, 1);
    let crc = checksum(&bad);
    put(&mut bad, 504, crc);
    assert!(!valid(&bad, true));
};

const _: () = {
    let mut state = fixture();
    state.words[130] = 1000;
    state.words[132] = 885;

    let mut old = encode(state, 120);
    put(&mut old, 4, 11);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));

    let migrated = snapshot(&old);
    assert!(migrated.words[130] == 1000);
    assert!(migrated.words[132] == 885);

    state.words[130] = crate::ctcss::pair(1000, 885);
    state.words[131] = crate::ctcss::pair(0, 670);
    state.words[132] = crate::ctcss::pair(885, 1000);
    state.words[133] = crate::ctcss::pair(2541, 0);
    assert!(state.valid());

    let mut saved = encode(state, 121);
    assert!(valid(&saved, false));
    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    let loaded = snapshot(&saved);
    let mut index = 0;
    while index < state.words.len() {
        assert!(loaded.words[index] == state.words[index]);
        index += 1;
    }

    let mut disguised = saved;
    put(&mut disguised, 4, 11);
    let crc = checksum(&disguised);
    put(&mut disguised, 504, crc);
    assert!(!valid(&disguised, true));

    state.words[130] = 1 << 16;
    assert!(!state.valid());
    state.words[130] = 0;
    state.words[147] = crate::ctcss::pair(0, 1000);
    assert!(!state.valid());
};

const _: () = {
    let mut state = fixture();
    state.words[130] = crate::ctcss::pair(1000, 885);
    state.words[132] = crate::ctcss::pair(670, 1000);
    let mut old = encode(state, 130);
    put(&mut old, 4, 12);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));

    let restored = snapshot(&old);
    assert!(restored.words[130] == state.words[130]);
    assert!(restored.words[132] == state.words[132]);

    state.words[130] = crate::ctcss::pair(0x8013, 885);
    state.words[131] = crate::ctcss::pair(0xC013, 0);
    state.words[132] = crate::ctcss::pair(0xC015, 1000);
    state.words[133] = crate::ctcss::pair(1000, 670);
    assert!(state.valid());

    let mut saved = encode(state, 131);
    assert!(valid(&saved, false));
    assert!(matches!(choose(Meta::inspect(&old), Meta::inspect(&saved)), Ok(Some(0))));
    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    let loaded = snapshot(&saved);
    let mut index = 0;
    while index < state.words.len() {
        assert!(state.words[index] == loaded.words[index]);
        index += 1;
    }

    let mut disguised = saved;
    put(&mut disguised, 4, 12);
    let crc = checksum(&disguised);
    put(&mut disguised, 504, crc);
    assert!(!valid(&disguised, true));

    state.words[130] = 0x8213_0000;
    assert!(!state.valid());
    state.words[130] = 0x8213;
    assert!(!state.valid());
};

const _: () = {
    let mut state = fixture();
    state.words[130] = crate::ctcss::pair(0x8013, 885);
    state.words[132] = crate::ctcss::pair(0xC015, 1000);

    let mut old = encode(state, 140);
    put(&mut old, 4, 13);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));

    let previous = snapshot(&old);
    assert!(previous.words[130] == state.words[130]);
    assert!(previous.words[132] == state.words[132]);

    state.words[130] = crate::ctcss::pair(0x8013, 0xC015);
    state.words[131] = crate::ctcss::pair(0, 0x8013);
    state.words[132] = crate::ctcss::pair(1000, 0x8013);
    state.words[133] = crate::ctcss::pair(0xC015, 885);
    assert!(state.valid());

    let mut saved = encode(state, 141);
    assert!(valid(&saved, false));
    assert!(matches!(choose(Meta::inspect(&old), Meta::inspect(&saved)), Ok(Some(0))));
    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    let loaded = snapshot(&saved);
    let mut index = 0;
    while index < state.words.len() {
        assert!(loaded.words[index] == state.words[index]);
        index += 1;
    }

    let mut disguised = saved;
    put(&mut disguised, 4, 13);
    let crc = checksum(&disguised);
    put(&mut disguised, 504, crc);
    assert!(!valid(&disguised, true));

    state.words[130] = 0x8213_0000;
    assert!(!state.valid());
    state.words[130] = 0;
    state.words[147] = crate::ctcss::pair(0, 0x8013);
    assert!(!state.valid());
};

const fn historical_fixture(
    state: Snapshot,
    version: u32,
    generation: u32,
) -> [u8; 512] {
    assert!(version >= 2 && version <= 7);

    let length = if version <= 4 {
        384
    } else if version == 5 {
        432
    } else {
        448
    };

    let mut bytes = [255u8; 512];
    put(&mut bytes, 0, MAGIC);
    put(&mut bytes, 4, version);
    put(&mut bytes, 8, length);
    put(&mut bytes, 12, generation);

    let mut index = 0;
    while index < length as usize / 4 {
        let value = if (index == 6 && version < 3)
            || (index == 7 && version < 4)
            || (index == 109 && version < 7)
            || index >= 110
        {
            0
        } else {
            state.words[index]
        };

        put(&mut bytes, 32 + index * 4, value);
        index += 1;
    }

    let crc = checksum(&bytes);
    put(&mut bytes, 504, crc);
    put(&mut bytes, 508, COMMIT);
    bytes
}

/// Construct the format-1 header and its 256-byte linear payload explicitly:
/// sixteen global words followed by sixteen unnamed twelve-byte channels.
/// The CRC and commit occupy their fixed record locations.
const fn legacy_fixture(generation: u32) -> [u8; 512] {
    let state = fixture();
    let mut bytes = [255u8; 512];
    put(&mut bytes, 0, MAGIC);
    put(&mut bytes, 4, 1);
    put(&mut bytes, 8, 256);
    put(&mut bytes, 12, generation);

    let mut index = 0;
    while index < 16 {
        put(&mut bytes, 32 + index * 4, state.words[index]);
        index += 1;
    }

    let mut slot = 0;
    while slot < 16 {
        let source = 16 + slot * 5;
        let target = 32 + (16 + slot * 3) * 4;
        put(&mut bytes, target, state.words[source]);
        put(&mut bytes, target + 4, state.words[source + 1]);
        put(&mut bytes, target + 8, state.words[source + 2]);
        slot += 1;
    }

    let crc = checksum(&bytes);
    put(&mut bytes, 504, crc);
    put(&mut bytes, 508, COMMIT);
    bytes
}

// The same record checks run with and without the Spectrum implementation.
// A missing runtime module must not change the persistent field vocabulary.
const _: () = {
    let mut state = fixture();
    let mut preferences = crate::app::preferences::Preferences::new();
    preferences.spectrum = true;
    preferences.spectrum_step = 2;
    preferences.spectrum_rssi = 304;
    preferences.spectrum_noise = 48;
    preferences.write(&mut state.words);

    let mut saved = encode(state, 123);
    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    assert!(word(&saved, 4) == 19 && word(&saved, 8) == 448);

    let loaded = snapshot(&saved);
    let mut index = 0;
    while index < state.words.len() {
        assert!(loaded.words[index] == state.words[index]);
        index += 1;
    }

    let checked = match crate::app::preferences::Preferences::read(&loaded.words) {
        Some(value) => value,
        None => panic!("retained Spectrum preferences rejected"),
    };
    assert!(checked.spectrum);
    assert!(checked.spectrum_step == 2);
    assert!(checked.spectrum_rssi == 304);
    assert!(checked.spectrum_noise == 48);

    let mut resaved = encode(loaded, 123);
    put(&mut resaved, 508, COMMIT);
    index = 0;
    while index < saved.len() {
        assert!(resaved[index] == saved[index]);
        index += 1;
    }
};

// Payload conversion is exercised against the current RAM field rules
// and the complete committed journal record.
const _: () = {
    use super::profile_layout::{
        pack, unpack, CHANNEL_BYTES, CHANNEL_START, PAYLOAD_BYTES, VFO_START,
    };
    use crate::profile::{Profile, Tune, Tx};

    let mut state = fixture();
    state.words[7] = 13;
    state.words[109] = 5;
    state.words[104] = 2;
    state.words[105] = 304;
    state.words[106] = 48;
    state.words[107] = 1;
    state.words[11] = 1;
    state.words[14] = 100_000;

    let mut slot = 0;
    while slot < 16 {
        let at = 16 + slot * 5;
        state.words[at] = if slot & 1 == 0 {
            145_500_000 + slot as u32 * 12_500
        } else {
            433_500_000 + slot as u32 * 12_500
        };
        state.words[at + 1] = if slot % 3 == 0 { 25_000 } else { 12_500 };
        state.words[at + 2] = (slot & 1) as u32;
        state.words[at + 3] = u32::from_le_bytes(*b"PACK");
        state.words[at + 4] = u32::from_le_bytes([
            b'A' + slot as u8, b' ', b' ', b' ',
        ]);
        slot += 1;
    }
    assert!(state.valid());

    let mut packed = [0xA5u8; PAYLOAD_BYTES];
    assert!(pack(&state, &mut packed));
    let restored = match unpack(&packed) {
        Some(value) => value,
        None => panic!("packed full bank rejected"),
    };

    let mut index = 0;
    while index < state.words.len() {
        assert!(restored.words[index] == state.words[index]);
        index += 1;
    }

    let mut repeated = [0u8; PAYLOAD_BYTES];
    assert!(pack(&restored, &mut repeated));
    index = 0;
    while index < PAYLOAD_BYTES {
        assert!(repeated[index] == packed[index]);
        index += 1;
    }

    // The journal wraps the packed payload with its independent commit.
    let mut record = encode(restored, 123);
    put(&mut record, 508, COMMIT);
    assert!(word(&record, 4) == 19);
    assert!(word(&record, 8) == 448);
    assert!(valid(&record, true));

    let loaded = snapshot(&record);
    assert!(pack(&loaded, &mut repeated));
    index = 0;
    while index < PAYLOAD_BYTES {
        assert!(repeated[index] == packed[index]);
        index += 1;
    }

    // Invalid input leaves the caller's output buffer unchanged.
    let mut invalid = state;
    invalid.words[8] = 630_000_000;
    let mut untouched = [0xA5u8; PAYLOAD_BYTES];
    assert!(!pack(&invalid, &mut untouched));
    index = 0;
    while index < PAYLOAD_BYTES {
        assert!(untouched[index] == 0xA5);
        index += 1;
    }

    let mut bad = packed;
    bad[0] = 2;
    assert!(unpack(&bad).is_none());

    bad = packed;
    bad[VFO_START + 3] |= 0x80;
    assert!(unpack(&bad).is_none());

    bad = packed;
    bad[CHANNEL_START + 12] = 0;
    assert!(unpack(&bad).is_none());

    bad = packed;
    bad[CHANNEL_START + 8] = 1;
    assert!(unpack(&bad).is_none());

    // Packed channel conversion preserves each checked route value, including
    // split and RX-only, without reducing it to a receive-only tuning tuple.
    let routes = [
        Tx::Plus(600_000),
        Tx::Minus(600_000),
        Tx::Split(433_500_000),
        Tx::ReceiveOnly,
    ];
    let mut route = 0;
    while route < routes.len() {
        let profile = Profile {
            tune: Tune {
                frequency: state.words[16],
                step: state.words[17],
                am: false, narrow: state.words[18] != 0,
            },
            transmit: routes[route],
        };
        let words = match profile.encode() {
            Ok(words) => words,
            Err(_) => panic!("extended channel fixture rejected"),
        };

        bad = packed;
        let mut word_index = 0;
        while word_index < words.len() {
            let bytes = words[word_index].to_le_bytes();
            let mut byte = 0;
            while byte < 4 {
                bad[CHANNEL_START + word_index * 4 + byte] = bytes[byte];
                byte += 1;
            }
            word_index += 1;
        }
        let decoded = match unpack(&bad) {
            Some(state) => state,
            None => panic!("packed channel route rejected"),
        };
        assert!(decoded.words[114] == words[1]);
        route += 1;
    }

    // Empty slots must be entirely zero, including their name bytes.
    let mut empty = state;
    index = 16;
    while index < 96 {
        empty.words[index] = 0;
        index += 1;
    }
    assert!(pack(&empty, &mut repeated));
    assert!(unpack(&repeated).is_some());

    slot = 0;
    while slot < 16 {
        let mut byte = 0;
        while byte < CHANNEL_BYTES {
            assert!(repeated[CHANNEL_START + slot * CHANNEL_BYTES + byte] == 0);
            byte += 1;
        }
        slot += 1;
    }

    repeated[PAYLOAD_BYTES - 1] = b' ';
    assert!(unpack(&repeated).is_none());
};

const _: () = {
    // Every admitted old format migrates with HAM enabled. Migration is
    // representational only: the original record stays unchanged.
    let mut source = fixture();
    source.words[6] = 0x010304;
    source.words[7] = 4;
    source.words[109] = 5;

    let mut version = 1;
    while version <= 7 {
        let old = if version == 1 {
            legacy_fixture(100 + version)
        } else {
            historical_fixture(source, version, 100 + version)
        };

        assert!(valid(&old, true));
        assert!(Meta::inspect(&old).version == version);

        let loaded = snapshot(&old);
        assert!(loaded.valid());
        assert!(loaded.words[110] == 0);

        let mut current = encode(loaded, 200 + version);
        assert!(valid(&current, false));
        assert!(!valid(&current, true));
        put(&mut current, 508, COMMIT);
        assert!(valid(&current, true));
        assert!(Meta::inspect(&current).version == 19);

        let restored = snapshot(&current);
        let mut index = 0;
        while index < loaded.words.len() {
            assert!(loaded.words[index] == restored.words[index]);
            index += 1;
        }
        assert!(valid(&old, true));
        version += 1;
    }

    let old = historical_fixture(fixture(), 7, 40);
    let previous = Meta::inspect(&old);
    let mut disabled = snapshot(&old);
    disabled.words[110] = 1;
    assert!(disabled.valid());

    let mut saved = encode(disabled, 41);
    assert!(valid(&saved, false));
    assert!(matches!(choose(previous, Meta::inspect(&saved)), Ok(Some(0))));

    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    assert!(matches!(choose(previous, Meta::inspect(&saved)), Ok(Some(1))));
    assert!(snapshot(&saved).words[110] == 1);
    assert!(snapshot(&old).words[110] == 0);

    // HAM disable is global word 22 at payload byte 88, not the old
    // linear Snapshot offset. It remains inside the payload CRC.
    assert!(word(&saved, 32 + 88) == 1);

    let mut enabled = snapshot(&saved);
    enabled.words[110] = 0;
    assert!(enabled.valid());
    let mut reenabled = encode(enabled, 42);
    put(&mut reenabled, 508, COMMIT);
    assert!(valid(&reenabled, true));
    assert!(snapshot(&reenabled).words[110] == 0);

    let mut invalid = disabled;
    invalid.words[110] = 2;
    assert!(!invalid.valid());

    // Recomputed CRC cannot legitimize an unknown HAM encoding.
    let mut bad = saved;
    put(&mut bad, 32 + 88, 2);
    let crc = checksum(&bad);
    put(&mut bad, 504, crc);
    assert!(!valid(&bad, true));

    // The linear-format validator rejects nonzero reserved global words even
    // when the CRC matches the modified body.
    version = 6;
    while version <= 7 {
        bad = historical_fixture(fixture(), version, 50);
        put(&mut bad, 472, 1);
        let crc = checksum(&bad);
        put(&mut bad, 504, crc);
        assert!(!valid(&bad, true));
        version += 1;
    }

    // Current records accept the checked channel route. Relabeling the payload
    // as format 9 must fail because that contract requires simplex channels.
    bad = saved;
    put(
        &mut bad,
        32 + super::profile_layout::CHANNEL_START + 4,
        4 << 26,
    );
    let crc = checksum(&bad);
    put(&mut bad, 504, crc);
    assert!(valid(&bad, true));

    put(&mut bad, 4, 9);
    let crc = checksum(&bad);
    put(&mut bad, 504, crc);
    assert!(!valid(&bad, true));

    bad = saved;
    put(
        &mut bad,
        32 + super::profile_layout::CHANNEL_START + 8,
        1,
    );
    let crc = checksum(&bad);
    put(&mut bad, 504, crc);
    assert!(!valid(&bad, true));

    // Exact payload length is checked before reading or changing bytes.
    let mut short = [0xA5u8; 447];
    let mut long = [0xA5u8; 449];
    assert!(!super::profile_layout::pack(&disabled, &mut short));
    assert!(!super::profile_layout::pack(&disabled, &mut long));
    assert!(super::profile_layout::unpack(&short).is_none());
    assert!(super::profile_layout::unpack(&long).is_none());

    let mut index = 0;
    while index < short.len() {
        assert!(short[index] == 0xA5);
        index += 1;
    }
    index = 0;
    while index < long.len() {
        assert!(long[index] == 0xA5);
        index += 1;
    }
};

const _: () = {
    assert!(core::mem::size_of::<Snapshot>() == 592);
    assert!(super::profile_layout::PAYLOAD_BYTES == 448);
    assert!(32 + super::profile_layout::PAYLOAD_BYTES <= 504);

    let state = fixture();
    assert!(state.valid());

    let mut direct = [0u8; 512];
    encode_into(&mut direct, &state, 42);
    let returned = encode(state, 42);
    let mut index = 0;
    while index < 512 {
        assert!(direct[index] == returned[index]);
        index += 1;
    }
    assert!(word(&direct, 4) == 19);
    assert!(word(&direct, 8) == 448);
    assert!(valid(&direct, false));
    assert!(!valid(&direct, true));

    let legacy = legacy_fixture(7);
    assert!(owned(&legacy));
    assert!(valid(&legacy, true));
    assert!(Meta::inspect(&legacy).version == 1);

    let migrated = snapshot(&legacy);
    assert!(migrated.valid());
    assert!(migrated.words[16] == 145_500_000);
    assert!(migrated.words[21] == 433_500_000);
    assert!(migrated.words[23] == 1);
    assert!(migrated.words[19] == 0x2020_2020);
    assert!(migrated.words[20] == 0x2020_2020);
    assert!(migrated.words[24] == 0x2020_2020);
    assert!(migrated.words[26] == 0);
    assert!(migrated.words[29] == 0);

    let mut next = encode(migrated, 8);
    let previous = Meta::inspect(&legacy);
    assert!(matches!(choose(previous, Meta::inspect(&next)), Ok(Some(0))));
    put(&mut next, 508, COMMIT);
    assert!(valid(&next, true));
    assert!(Meta::inspect(&next).version == 19);
    assert!(matches!(choose(previous, Meta::inspect(&next)), Ok(Some(1))));

    let roundtrip = snapshot(&next);
    index = 0;
    while index < 96 {
        assert!(roundtrip.words[index] == migrated.words[index]);
        index += 1;
    }

    let named = encode(state, 9);
    let restored_names = snapshot(&named);
    assert!(restored_names.words[19] == state.words[19]);
    assert!(restored_names.words[24] == state.words[24]);

    let v2 = historical_fixture(state, 2, 10);
    assert!(valid(&v2, true));
    assert!(Meta::inspect(&v2).version == 2);
    let v2_state = snapshot(&v2);
    assert!(v2_state.words[6] == 0);
    assert!(matches!(
        crate::app::sound::Settings::decode(v2_state.words[6]),
        Some(crate::app::sound::Settings { roger: 0, permit: 0, boot: true }),
    ));

    let mut audio_state = state;
    audio_state.words[6] = 0x010304;
    assert!(audio_state.valid());
    let mut audio_record = encode(audio_state, 11);
    put(&mut audio_record, 508, COMMIT);
    assert!(valid(&audio_record, true));
    assert!(snapshot(&audio_record).words[6] == 0x010304);

    let mut invalid_audio = audio_state;
    invalid_audio.words[6] = 5;
    assert!(!invalid_audio.valid());

    put(&mut audio_record, 4, 2);
    let wrong_crc = checksum(&audio_record);
    put(&mut audio_record, 504, wrong_crc);
    assert!(!valid(&audio_record, true));

    let v3 = historical_fixture(state, 3, 12);
    assert!(valid(&v3, true));
    assert!(Meta::inspect(&v3).version == 3);
    assert!(snapshot(&v3).words[7] == 0);

    let mut powered = state;
    powered.words[7] = 4;
    assert!(powered.valid());
    let mut powered_record = encode(powered, 13);
    put(&mut powered_record, 508, COMMIT);
    assert!(valid(&powered_record, true));
    assert!(snapshot(&powered_record).words[7] == 4);

    put(&mut powered_record, 4, 3);
    let old_version_crc = checksum(&powered_record);
    put(&mut powered_record, 504, old_version_crc);
    assert!(!valid(&powered_record, true));
    powered.words[7] = 14;
    assert!(!powered.valid());

    let v5 = historical_fixture(state, 5, 14);
    assert!(valid(&v5, true));
    assert!(Meta::inspect(&v5).version == 5);

    let v5_loaded = snapshot(&v5);
    assert!(v5_loaded.words[108] == crate::app::appearance::Settings::new().word());

    let mut customized = state;
    let mut appearance = crate::app::appearance::Settings::new();
    appearance.tx_visual = 2;
    appearance.beeps = 3;
    customized.words[108] = appearance.word();

    let mut customized_record = encode(customized, 15);
    put(&mut customized_record, 508, COMMIT);
    assert!(valid(&customized_record, true));
    assert!(snapshot(&customized_record).words[108] == appearance.word());

    customized.words[109] = 6;
    assert!(!customized.valid());

    // Format 6 requires payload word 109 to be zero.
    let v6 = historical_fixture(state, 6, 16);

    assert!(valid(&v6, true));
    assert!(Meta::inspect(&v6).version == 6);
    assert!(snapshot(&v6).words[109] == 1);

    let mut options = state;
    options.words[7] = 13;
    options.words[109] = 5;

    assert!(options.valid());

    let mut options_record = encode(options, 17);
    put(&mut options_record, 508, COMMIT);

    assert!(valid(&options_record, true));
    assert!(Meta::inspect(&options_record).version == 19);
    assert!(snapshot(&options_record).words[7] == 13);
    assert!(snapshot(&options_record).words[109] == 5);

    // Format 6 requires its timeout position to be zero, regardless of CRC.
    let mut disguised = options_record;
    put(&mut disguised, 4, 6);
    let disguised_crc = checksum(&disguised);
    put(&mut disguised, 504, disguised_crc);
    assert!(!valid(&disguised, true));

    options.words[7] = 14;
    assert!(!options.valid());

    options.words[7] = 0;
    options.words[109] = 6;
    assert!(!options.valid());

    let mut unknown = legacy;
    put(&mut unknown, 4, 20);
    assert!(!owned(&unknown));
    assert!(!Meta::inspect(&unknown).reclaimable());
    assert!(!valid(&unknown, true));

    let mut wrong_length = legacy;
    put(&mut wrong_length, 8, 384);
    assert!(!owned(&wrong_length));

    let mut invalid = state;
    invalid.words[19] = 0;
    assert!(!invalid.valid());
    invalid = state;
    invalid.words[19] = 0x8080_8080;
    assert!(!invalid.valid());
    invalid = state;
    invalid.words[29] = 0x2020_2020;
    assert!(!invalid.valid());
    invalid = state;
    invalid.words[0] = 2;
    assert!(!invalid.valid());
    invalid = state;
    invalid.words[9] = 146_000_000;
    assert!(!invalid.valid());

    let prepared = encode(migrated, 8);
    let mut prefix = [255u8; 512];
    index = 0;
    while index < 508 {
        prefix[index] = prepared[index];
        assert!(!valid(&prefix, true));
        assert!(matches!(choose(previous, Meta::inspect(&prefix)), Ok(Some(0))));
        index += 1;
    }
    assert!(valid(&prefix, false));

    let marker = COMMIT.to_le_bytes();
    index = 0;
    while index < 3 {
        prefix[508 + index] = marker[index];
        assert!(!valid(&prefix, true));
        assert!(matches!(choose(previous, Meta::inspect(&prefix)), Ok(Some(0))));
        index += 1;
    }
    prefix[511] = marker[3];
    assert!(valid(&prefix, true));
    assert!(matches!(choose(previous, Meta::inspect(&prefix)), Ok(Some(1))));

    let mut corrupted = next;
    corrupted[32] ^= 1;
    assert!(owned(&corrupted));
    assert!(!valid(&corrupted, true));
    assert!(matches!(choose(previous, Meta::inspect(&corrupted)), Ok(Some(0))));

    let mut erase_interrupted = next;
    index = 0;
    while index < 512 {
        erase_interrupted[index] = 255;
        assert!(matches!(choose(previous, Meta::inspect(&erase_interrupted)), Ok(Some(0))));
        index += 1;
    }
    assert!(Meta::inspect(&erase_interrupted).reclaimable());

    let mut last = encode(state, u32::MAX);
    put(&mut last, 508, COMMIT);
    let mut wrapped = encode(state, 0);
    put(&mut wrapped, 508, COMMIT);
    assert!(matches!(choose(Meta::inspect(&last), Meta::inspect(&wrapped)), Ok(Some(1))));

    let a = Meta { valid: true, generation: 1, crc: 10, ..Meta::unknown() };
    let b = Meta { valid: true, generation: 1, crc: 11, ..Meta::unknown() };
    assert!(matches!(choose(a, b), Err(604)));
    let half = Meta { generation: 0x8000_0001, ..b };
    assert!(matches!(choose(a, half), Err(604)));
};

/// Exercise saved DW, compatibility defaults, and reserved-field rejection.
const fn check_saved_dual_watch() {
    let mut version = 1;
    while version <= 14 {
        let mut bytes = if version == 1 {
            legacy_fixture(200 + version)
        } else if version <= 7 {
            historical_fixture(fixture(), version, 200 + version)
        } else {
            let mut packed = encode(fixture(), 200 + version);
            put(&mut packed, 4, version);
            let crc = checksum(&packed);
            put(&mut packed, 504, crc);
            put(&mut packed, 508, COMMIT);
            packed
        };

        assert!(valid(&bytes, true));
        let loaded = snapshot(&bytes);
        assert!(loaded.words[111] == 0);

        // Assigned fields cannot be introduced into a stricter format
        // merely by recalculating its CRC.
        if version >= 6 {
            let selection_at = if version <= 7 { 476 } else { 124 };
            put(&mut bytes, selection_at, 1);
            let crc = checksum(&bytes);
            put(&mut bytes, 504, crc);
            assert!(!valid(&bytes, true));
        }

        version += 1;
    }

    let mut state = fixture();
    state.words[110] = 1;
    state.words[111] = 1;
    state.words[130] = crate::ctcss::pair(0x8013, 0xC015);
    state.words[132] = crate::ctcss::pair(1000, 885);
    assert!(state.valid());

    let mut off = encode(fixture(), 300);
    put(&mut off, 508, COMMIT);
    let previous = Meta::inspect(&off);

    let mut on = encode(state, 301);
    assert!(valid(&on, false));
    assert!(!valid(&on, true));
    assert!(matches!(choose(previous, Meta::inspect(&on)), Ok(Some(0))));

    put(&mut on, 508, COMMIT);
    assert!(valid(&on, true));
    assert!(word(&on, 4) == 19);
    assert!(word(&on, 8) == 448);
    assert!(word(&on, 124) == 1);
    assert!(matches!(choose(previous, Meta::inspect(&on)), Ok(Some(1))));

    let loaded = snapshot(&on);
    let mut index = 0;
    while index < state.words.len() {
        assert!(loaded.words[index] == state.words[index]);
        index += 1;
    }

    let mut disabled = loaded;
    disabled.words[111] = 0;
    let encoded = encode(disabled, 302);
    assert!(valid(&encoded, false));
    assert!(snapshot(&encoded).words[111] == 0);
    assert!(snapshot(&encoded).words[110] == 1);

    state.words[111] = 0x8000_0000;
    assert!(!state.valid());

    let mut malformed = on;
    put(&mut malformed, 124, 0x8000_0000);
    let crc = checksum(&malformed);
    put(&mut malformed, 504, crc);
    assert!(!valid(&malformed, true));
}

const _: () = check_saved_dual_watch();

const _: () = {
    let mut state = fixture();
    state.words[8] = 118_025_000;
    state.words[11] = 3;
    state.words[16] = 118_025_000;
    state.words[18] = 2;
    state.words[130] = crate::ctcss::pair(0x8013, 885);
    assert!(state.valid());
    assert!(!state.legacy());

    let mut bytes = encode(state, 401);
    assert!(valid(&bytes, false));
    put(&mut bytes, 508, COMMIT);
    assert!(valid(&bytes, true));
    let loaded = snapshot(&bytes);
    let mut index = 0;
    while index < state.words.len() {
        assert!(loaded.words[index] == state.words[index]);
        index += 1;
    }

    // Correct CRC does not grant AM or extended RX to a format-15 record.
    put(&mut bytes, 4, 15);
    let crc = checksum(&bytes);
    put(&mut bytes, 504, crc);
    assert!(!valid(&bytes, true));

    let mut old = encode(fixture(), 400);
    put(&mut old, 4, 15);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));
    assert!(snapshot(&old).legacy());

    state.words[11] = 4;
    assert!(!state.valid());
};

const _: () = {
    let mut state = fixture();
    state.words[8] = 50_000_000;
    state.words[11] = 2;
    state.words[12] = 629_999_990;
    state.words[13] = 250_000_000;
    state.words[15] = 3;
    state.words[16] = 350_000_000;
    state.words[18] = 1;
    state.words[100] = 280_000_000;
    state.words[101] = 629_999_990;
    assert!(state.valid());
    assert!(!state.rx16());
    assert!(!state.legacy());

    let mut saved = encode(state, 501);
    assert!(valid(&saved, false));
    put(&mut saved, 508, COMMIT);
    assert!(valid(&saved, true));
    assert!(word(&saved, 4) == 19);

    let restored = snapshot(&saved);
    let mut index = 0;
    while index < state.words.len() {
        assert!(restored.words[index] == state.words[index]);
        index += 1;
    }

    let mut disguised = saved;
    put(&mut disguised, 4, 16);
    let crc = checksum(&disguised);
    put(&mut disguised, 504, crc);
    assert!(!valid(&disguised, true));

    // A genuine format-16 AM record remains readable without a migration write.
    let mut old_state = fixture();
    old_state.words[8] = 118_025_000;
    old_state.words[11] = 3;
    assert!(old_state.rx16());
    let mut old = encode(old_state, 500);
    put(&mut old, 4, 16);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));
    assert!(snapshot(&old).words[11] == 3);

    // Each frequency-bearing region participates in format-16 admission.
    let positions = [8usize, 9, 12, 13, 16, 21, 100, 101];
    index = 0;
    while index < positions.len() {
        let mut altered = old_state;
        altered.words[positions[index]] = 500_000_000;
        assert!(!altered.rx16());
        index += 1;
    }
};

const _: () = {
    let mut state = fixture();
    state.words[8] = 1_000_000_000;
    state.words[9] = 145_500_000;
    state.words[11] = 3;
    state.words[16] = 1_300_000_000;
    state.words[18] = 2;
    state.words[100] = 840_000_000;
    state.words[101] = 1_300_000_000;
    assert!(state.valid());
    assert!(!state.rx17());

    let mut bytes = encode(state, 601);
    assert!(valid(&bytes, false));
    put(&mut bytes, 508, COMMIT);
    assert!(valid(&bytes, true));
    let restored = snapshot(&bytes);
    let mut index = 0;
    while index < state.words.len() {
        assert!(restored.words[index] == state.words[index]);
        index += 1;
    }

    // A correct CRC cannot extend a historical record's RX vocabulary.
    put(&mut bytes, 4, 17);
    let crc = checksum(&bytes);
    put(&mut bytes, 504, crc);
    assert!(!valid(&bytes, true));

    let mut old_state = fixture();
    old_state.words[8] = 250_000_000;
    old_state.words[11] = 2;
    let mut old = encode(old_state, 600);
    put(&mut old, 4, 17);
    let crc = checksum(&old);
    put(&mut old, 504, crc);
    put(&mut old, 508, COMMIT);
    assert!(valid(&old, true));
    assert!(snapshot(&old).words[8] == 250_000_000);
    assert!(snapshot(&old).words[11] == 2);
};

const _: () = {
    let mut settings = crate::slope::Settings::new();
    settings.enabled = true;
    settings.upper = true;
    settings.delta = 40;
    settings.narrow = true;

    let mut state = fixture();
    state.words[11] = 2;
    state.words[111] = 1 | ((settings.word() as u32) << 1);
    assert!(state.valid());

    let mut bytes = encode(state, 801);
    assert!(valid(&bytes, false));
    put(&mut bytes, 508, COMMIT);
    assert!(valid(&bytes, true));
    let loaded = snapshot(&bytes);
    let mut index = 0;
    while index < state.words.len() {
        assert!(state.words[index] == loaded.words[index]);
        index += 1;
    }
    assert!(loaded.words[111] & 1 == 1);

    let restored = match crate::slope::Settings::decode(loaded.words[111] >> 1) {
        Some(value) => value,
        None => panic!("saved SlopeCast rejected"),
    };
    assert!(restored.enabled && restored.upper && restored.narrow);
    assert!(restored.delta == 40);

    put(&mut bytes, 4, 18);
    let crc = checksum(&bytes);
    put(&mut bytes, 504, crc);
    assert!(!valid(&bytes, true));

    // Both historical DW states load with SlopeCast disabled.
    let mut dual = 0;
    while dual < 2 {
        let mut old_state = fixture();
        old_state.words[111] = dual;
        let mut old = encode(old_state, 800);
        put(&mut old, 4, 18);
        let crc = checksum(&old);
        put(&mut old, 504, crc);
        put(&mut old, 508, COMMIT);
        assert!(valid(&old, true));
        let old = snapshot(&old);
        let settings = match crate::slope::Settings::decode(old.words[111] >> 1) {
            Some(value) => value,
            None => panic!("historical SlopeCast defaults rejected"),
        };
        assert!(!settings.enabled && settings.delta == 50);
        assert!(old.words[111] & 1 == dual);
        dual += 1;
    }
};
