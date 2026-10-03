//! Checked tuning and transmit-route values without device access.
//!
//! Frequencies and offsets use integer hertz on a ten-hertz grid. RX admits
//! 50 MHz through 629999990 Hz and 840 MHz through 1300 MHz inclusive.
//! The gap from 630 MHz through 839999990 Hz is rejected.
//! TX route resolution retains 136..174 MHz and 400..470 MHz. Band::of checks
//! those TX bounds; Band::receive selects the admitted RX path. Neither
//! operation grants policy, calibration, or resource-ownership permission.
//!
//! Receive paths divide at 280 MHz, following the board reference driver's
//! PickRXFilterPathBasedOnFrequency. Their names denote board paths rather
//! than formal spectrum allocations. Manual stepping clamps within a path;
//! direct entry can select either. The other-path VFO position remains saved.
//!
//! RX limits follow the two intervals in the reference frequencies.c
//! RX_freq_check, with the lower limit intentionally restricted to 50 MHz.
//! This is tuning admission, not a measured sensitivity specification.
//! The upper interval shares the UHF hardware path but has independent
//! stepping and scan bounds, so no operation synthesizes a frequency in the gap.
//!
//! Tx describes intent, not live RF configuration. Simplex follows RX.
//! Plus and Minus retain a positive offset up to 70 MHz as RX changes.
//! An offset must resolve inside the RX band; an inadmissible result blocks
//! transmission without deleting the saved offset. Split stores an explicit
//! frequency in either admitted band. ReceiveOnly has no TX destination.
//!
//! Profile packing uses three explicit words:
//!   Word 0: RX/10 low 26 bits in 0..25, narrow in 26, step in 27..28,
//!           AM in 29, and RX/10 bit 26 in bit 30
//!   Word 1: argument/10 in bits 0..25, TX mode in bits 26..28
//!   Word 2: zero, with directional signaling handled by the enclosing codec
//!
//! Step indices 0..2 select 12500, 25000, and 100000 Hz. TX modes 0..4
//! select Simplex, Plus, Minus, Split, and ReceiveOnly. Simplex and ReceiveOnly
//! require a zero argument. Word 0 reserves bit 31; the route word reserves
//! bits 29..31. Frequency bit 26 is stored separately so lower-range packed
//! profiles retain identical bytes. Enclosing codecs reject this additional
//! frequency bit under contracts that do not admit upper-range reception.
//!
//! Modulation and width are independent saved fields. FM, NFM, AM, and NAM
//! are display names; NAM means the narrower AM preset, not a channel-spacing
//! promise. Frequency entry does not automatically change modulation.
//!
//! Encode validates representation independently of effective TX resolution.
//! Decode checks reserved bits, step, route, and RX-band admission directly.
//! The tuning-only projection succeeds only for a valid simplex profile.
//! Returning an effective frequency never grants TX policy, calibration,
//! battery, carrier, or resource-ownership admission.

#[derive(Clone, Copy)]
pub(crate) enum Band {
    Vhf,
    Uhf,
}

impl Band {
    /// Runtime RX admission and board-path selection. Frequency writes
    /// remain integer ten-hertz words in the established receiver sequence.
    pub(crate) const fn receive(frequency: u32) -> Option<Self> {
        match receive_span(frequency) {
            1 => Some(Self::Vhf),
            2 | 3 => Some(Self::Uhf),
            _ => None,
        }
    }

    /// Exact RX vocabulary of journal format 17 and wire schema 8.
    pub(crate) const fn receive_v17(frequency: u32) -> Option<Self> {
        if frequency >= 630_000_000 {
            None
        } else {
            Self::receive(frequency)
        }
    }

    /// Exact frequency vocabulary of journal format 16 and wire schema 7.
    /// Keeping this separate prevents an older format label from acquiring
    /// extended RX meaning merely because its CRC was recalculated.
    pub(crate) const fn receive_v16(frequency: u32) -> Option<Self> {
        if frequency % 10 == 0
            && frequency >= 108_000_000 && frequency < 136_000_000
        {
            Some(Self::Vhf)
        } else {
            Self::of(frequency)
        }
    }

    pub(crate) const fn rx_lower(self) -> u32 {
        match self {
            Self::Vhf => 50_000_000,
            Self::Uhf => 280_000_000,
        }
    }

    pub(crate) const fn rx_upper(self) -> u32 {
        match self {
            Self::Vhf => 279_999_990,
            Self::Uhf => 629_999_990,
        }
    }

    pub(crate) const fn shift_receive(
        self, frequency: u32, step: u32, up: bool,
    ) -> u32 {
        assert!(matches!(
            (self, Self::receive(frequency)),
            (Self::Vhf, Some(Self::Vhf)) | (Self::Uhf, Some(Self::Uhf))
        ));
        assert!(step > 0 && step % 10 == 0);

        let (lower, upper) = receive_bounds(frequency);
        let next = if up {
            frequency.saturating_add(step)
        } else {
            frequency.saturating_sub(step)
        };
        if next < lower {
            lower
        } else if next > upper {
            upper
        } else {
            next
        }
    }

    pub(crate) const fn of(frequency: u32) -> Option<Self> {
        if frequency % 10 != 0 {
            return None;
        }
        if frequency >= 136_000_000 && frequency <= 174_000_000 {
            Some(Self::Vhf)
        } else if frequency >= 400_000_000 && frequency <= 470_000_000 {
            Some(Self::Uhf)
        } else {
            None
        }
    }

    pub(crate) const fn lower(self) -> u32 {
        match self {
            Self::Vhf => 136_000_000,
            Self::Uhf => 400_000_000,
        }
    }

    pub(crate) const fn upper(self) -> u32 {
        match self {
            Self::Vhf => 174_000_000,
            Self::Uhf => 470_000_000,
        }
    }

    pub(crate) const fn initial(self) -> u32 {
        match self {
            Self::Vhf => 145_500_000,
            Self::Uhf => 433_500_000,
        }
    }

    /// Clamp within the selected band. The caller supplies a current
    /// frequency and positive step on the ten-hertz grid.
    pub(crate) const fn shift(self, frequency: u32, step: u32, up: bool) -> u32 {
        assert!(frequency >= self.lower() && frequency <= self.upper());
        assert!(frequency % 10 == 0 && step > 0 && step % 10 == 0);

        let next = if up {
            frequency.saturating_add(step)
        } else {
            frequency.saturating_sub(step)
        };
        if next < self.lower() {
            self.lower()
        } else if next > self.upper() {
            self.upper()
        } else {
            next
        }
    }
}

/// Contiguous tuning intervals: zero is invalid, one uses the low path,
/// two is the lower part of the high path, three is its upper RF interval.
/// These numbers are comparison keys only and are not serialized.
pub(crate) const fn receive_span(frequency: u32) -> u8 {
    if frequency % 10 != 0 {
        return 0;
    }
    match frequency {
        50_000_000..=279_999_990 => 1,
        280_000_000..=629_999_990 => 2,
        840_000_000..=1_300_000_000 => 3,
        _ => 0,
    }
}

/// The caller supplies an admitted receive frequency. Limits are inclusive
/// ten-hertz points. Physical-path equality alone is insufficient because
/// the high path contains two disjoint tuning intervals.
pub(crate) const fn receive_bounds(frequency: u32) -> (u32, u32) {
    match receive_span(frequency) {
        1 => (Band::Vhf.rx_lower(), Band::Vhf.rx_upper()),
        2 => (Band::Uhf.rx_lower(), Band::Uhf.rx_upper()),
        3 => (840_000_000, 1_300_000_000),
        _ => panic!("invalid receive interval"),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Tune {
    pub(crate) frequency: u32,
    pub(crate) step: u32,
    pub(crate) narrow: bool,
    pub(crate) am: bool,
}

impl Tune {
    /// Encode the complete checked tuning word without constructing a route
    /// or signaling word. This is shared by channel profiles and the journal.
    /// Receive admission precedes step admission, matching Profile::encode.
    #[inline(never)]
    pub(crate) const fn word(self) -> Result<u32, Error> {
        if Band::receive(self.frequency).is_none() {
            return Err(Error::Receive);
        }

        let step = match self.step {
            12_500 => 0,
            25_000 => 1,
            100_000 => 2,
            _ => return Err(Error::Step),
        };
        let frequency = self.frequency / 10;

        Ok((frequency & ARGUMENT_MASK)
            | ((frequency & (1 << 26)) << 4)
            | ((self.narrow as u32) << 26)
            | (step << 27)
            | ((self.am as u32) << 29))
    }

    /// Decode field representation, not receive admission. Reserved bit 31
    /// and the unknown step index are rejected before constructing the Tune.
    /// The decoded frequency is on the ten-hertz grid but may be outside
    /// the admitted RX intervals.
    ///
    /// Profile::decode checks RX after route representation to preserve its
    /// error precedence. Journal unpacking checks RX as part of the complete
    /// detached Snapshot before returning it. Neither caller may expose this
    /// intermediate Tune as an admitted hardware request.
    #[inline(never)]
    pub(crate) const fn decode_fields(word: u32) -> Result<Self, Error> {
        if word & 0x8000_0000 != 0 {
            return Err(Error::Reserved);
        }

        let step = match (word >> 27) & 3 {
            0 => 12_500,
            1 => 25_000,
            2 => 100_000,
            _ => return Err(Error::Step),
        };

        Ok(Self {
            frequency: ((word & ARGUMENT_MASK)
                | ((word >> 4) & (1 << 26))) * 10,
            step,
            narrow: word & (1 << 26) != 0,
            am: word & (1 << 29) != 0,
        })
    }

    /// Snapshot width word: bit zero is narrow, bit one is AM.
    /// No additional Snapshot word or channel-slot byte is allocated.
    pub(crate) const fn flags(self) -> u32 {
        self.narrow as u32 | ((self.am as u32) << 1)
    }

    pub(crate) const fn name(self) -> &'static [u8] {
        if self.am {
            if self.narrow { b"NAM" } else { b"AM" }
        } else if self.narrow {
            b"NFM"
        } else {
            b"FM"
        }
    }

    pub(crate) const fn initial(band: Band) -> Self {
        Self { frequency: band.initial(), step: 12_500, narrow: false, am: false }
    }

    pub(crate) const fn valid(self) -> bool {
        Band::receive(self.frequency).is_some()
            && matches!(self.step, 12_500 | 25_000 | 100_000)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Tx {
    Simplex,
    Plus(u32),
    Minus(u32),
    Split(u32),
    ReceiveOnly,
}

#[derive(Clone, Copy)]
pub(crate) enum Error {
    Receive,
    Step,
    Route,
    Reserved,
}

impl Tx {
    /// Encode intent without requiring the current RX frequency to resolve
    /// to an admitted TX frequency. The maximum offset equals the widest
    /// supported band span; zero offset must be expressed as Simplex.
    pub(crate) const fn word(self) -> Result<u32, Error> {
        let (mode, argument) = match self {
            Self::Simplex => (0, 0),
            Self::ReceiveOnly => (4, 0),
            Self::Plus(offset) | Self::Minus(offset) => {
                if offset == 0 || offset > 70_000_000 || offset % 10 != 0 {
                    return Err(Error::Route);
                }
                (if matches!(self, Self::Plus(_)) { 1 } else { 2 }, offset)
            }
            Self::Split(frequency) => {
                if Band::of(frequency).is_none() {
                    return Err(Error::Route);
                }
                (3, frequency)
            }
        };

        Ok(argument / 10 | (mode << 26))
    }

    pub(crate) const fn decode(word: u32) -> Result<Self, Error> {
        if word & RESERVED_MASK != 0 {
            return Err(Error::Reserved);
        }

        let argument = (word & ARGUMENT_MASK) * 10;
        let route = match (word >> 26) & 7 {
            0 if argument == 0 => Self::Simplex,
            1 => Self::Plus(argument),
            2 => Self::Minus(argument),
            3 => Self::Split(argument),
            4 if argument == 0 => Self::ReceiveOnly,
            _ => return Err(Error::Route),
        };

        match route.word() {
            Ok(_) => Ok(route),
            Err(error) => Err(error),
        }
    }

    pub(crate) const fn name(self) -> &'static [u8] {
        match self {
            Self::Simplex => b"SIMPLEX",
            Self::Plus(_) => b"+",
            Self::Minus(_) => b"-",
            Self::Split(_) => b"SPLIT",
            Self::ReceiveOnly => b"RX ONLY",
        }
    }

    /// Resolve one route without rounding, clamping, or wrapping.
    /// Ok(None) explicitly means RX only. The caller must still check its
    /// policy, calibrated profile, RF ownership, battery, and TX deadline.
    pub(crate) const fn frequency(self, receive: u32) -> Result<Option<u32>, Error> {
        if let Err(error) = self.word() {
            return Err(error);
        }

        let band = match Band::receive(receive) {
            Some(band) => band,
            None => return Err(Error::Receive),
        };

        let offset = match self {
            Self::Simplex => {
                return if Band::of(receive).is_some() {
                    Ok(Some(receive))
                } else {
                    Err(Error::Route)
                };
            }
            Self::ReceiveOnly => return Ok(None),
            Self::Split(frequency) => {
                return if Band::of(frequency).is_some() {
                    Ok(Some(frequency))
                } else {
                    Err(Error::Route)
                };
            }
            Self::Plus(offset) | Self::Minus(offset) => offset,
        };

        if offset == 0 || offset % 10 != 0 {
            return Err(Error::Route);
        }

        let frequency = if matches!(self, Self::Plus(_)) {
            receive.checked_add(offset)
        } else {
            receive.checked_sub(offset)
        };

        match frequency {
            Some(value) if value >= band.lower() && value <= band.upper() => {
                Ok(Some(value))
            }
            _ => Err(Error::Route),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Profile {
    pub(crate) tune: Tune,
    pub(crate) transmit: Tx,
}

const ARGUMENT_MASK: u32 = 0x03FF_FFFF;
const RESERVED_MASK: u32 = 0xE000_0000;

impl Profile {
    /// Display resolution includes the modulation-level TX prohibition.
    /// Stored route intent remains available for an explicit return to FM.
    pub(crate) const fn transmit_frequency(self) -> Result<Option<u32>, Error> {
        if self.tune.am { Ok(None) } else { self.transmit.frequency(self.tune.frequency) }
    }

    pub(crate) const fn simplex(tune: Tune) -> Self {
        Self { tune, transmit: Tx::Simplex }
    }

    #[inline(never)]
    pub(crate) const fn encode(self) -> Result<[u32; 3], Error> {
        let tune = match self.tune.word() {
            Ok(word) => word,
            Err(error) => return Err(error),
        };

        let route = match self.transmit.word() {
            Ok(route) => route,
            Err(error) => return Err(error),
        };

        Ok([tune, route, 0])
    }

    #[inline(never)]
    pub(crate) const fn decode(words: [u32; 3]) -> Result<Self, Error> {
        if words[1] & RESERVED_MASK != 0 || words[2] != 0 {
            return Err(Error::Reserved);
        }

        let tune = match Tune::decode_fields(words[0]) {
            Ok(tune) => tune,
            Err(error) => return Err(error),
        };
        let transmit = match Tx::decode(words[1]) {
            Ok(transmit) => transmit,
            Err(error) => return Err(error),
        };

        let profile = Self { tune, transmit };

        // Reserved fields, the step encoding, and route representation
        // were checked above. A decoded frequency is already on the
        // ten-hertz grid. Only receive-band admission remains.
        //
        // Do not require the route to resolve here: an offset retained
        // after manual RX tuning may be valid data but must block TX.
        if Band::receive(profile.tune.frequency).is_none() {
            Err(Error::Receive)
        } else {
            Ok(profile)
        }
    }

    /// Lossless projection into the existing v7 tuning representation.
    /// A syntactically valid extended route is still not representable.
    pub(crate) const fn v7_tune(self) -> Option<Tune> {
        if self.tune.valid() && !self.tune.am && Band::of(self.tune.frequency).is_some() && matches!(self.transmit, Tx::Simplex) {
            Some(self.tune)
        } else {
            None
        }
    }
}

const _: () = {
    let frequencies = [
        50_000_000, 118_025_000, 433_506_250,
        629_999_990, 840_000_000, 1_300_000_000,
    ];
    let steps = [12_500, 25_000, 100_000];
    let mut frequency = 0;

    while frequency < frequencies.len() {
        let mut step = 0;
        while step < steps.len() {
            let mut flags = 0;
            while flags < 4 {
                let tune = Tune {
                    frequency: frequencies[frequency],
                    step: steps[step],
                    narrow: flags & 1 != 0,
                    am: flags & 2 != 0,
                };
                let word = match tune.word() {
                    Ok(word) => word,
                    Err(_) => panic!("checked tune word rejected"),
                };
                let decoded = match Tune::decode_fields(word) {
                    Ok(value) => value,
                    Err(_) => panic!("tune field decoding failed"),
                };
                assert!(decoded.frequency == tune.frequency);
                assert!(decoded.step == tune.step);
                assert!(decoded.flags() == tune.flags());
                assert!(decoded.valid());

                let profile = Profile {
                    tune, transmit: Tx::ReceiveOnly,
                };
                let words = checked_words(profile);
                assert!(words[0] == word);
                assert!(words[1] == 4 << 26 && words[2] == 0);

                flags += 1;
            }
            step += 1;
        }
        frequency += 1;
    }

    // Field decoding cannot by itself admit a receive frequency.
    let zero = match Tune::decode_fields(0) {
        Ok(value) => value,
        Err(_) => panic!("zero has valid field representation"),
    };
    assert!(zero.frequency == 0 && !zero.valid());
    assert!(matches!(Profile::decode([0, 0, 0]), Err(Error::Receive)));

    assert!(matches!(
        Tune::decode_fields(1 << 31), Err(Error::Reserved),
    ));
    assert!(matches!(
        Tune::decode_fields(3 << 27), Err(Error::Step),
    ));

    // Complete-profile errors retain their established precedence.
    assert!(matches!(
        Profile::decode([3 << 27, 1 << 31, 0]),
        Err(Error::Reserved),
    ));
    assert!(matches!(
        Profile::decode([3 << 27, 5 << 26, 0]),
        Err(Error::Step),
    ));
    assert!(matches!(
        Profile::decode([0, 5 << 26, 0]),
        Err(Error::Route),
    ));

    let malformed = Profile {
        tune: Tune { frequency: 0, step: 1, narrow: false, am: false },
        transmit: Tx::Plus(0),
    };
    assert!(matches!(malformed.encode(), Err(Error::Receive)));
};

const fn checked_words(profile: Profile) -> [u32; 3] {
    match profile.encode() {
        Ok(words) => words,
        Err(_) => panic!("profile fixture rejected"),
    }
}

const fn roundtrip(profile: Profile) {
    let words = checked_words(profile);
    let decoded = match Profile::decode(words) {
        Ok(value) => value,
        Err(_) => panic!("packed profile rejected"),
    };
    let again = checked_words(decoded);
    let mut index = 0;
    while index < words.len() {
        assert!(words[index] == again[index]);
        index += 1;
    }
    assert!(decoded.tune.frequency == profile.tune.frequency);
    assert!(decoded.tune.step == profile.tune.step);
    assert!(decoded.tune.narrow == profile.tune.narrow);
}

const _: () = {
    let tune = Tune::initial(Band::Vhf);
    let original = checked_words(Profile::simplex(tune));
    let invalid_frequencies = [
        0, 49_999_990, 630_000_000, 640_000_000, 671_088_630,
    ];

    let mut index = 0;
    while index < invalid_frequencies.len() {
        let mut words = original;
        words[0] = (words[0] & !ARGUMENT_MASK)
            | (invalid_frequencies[index] / 10);
        assert!(matches!(Profile::decode(words), Err(Error::Receive)));
        index += 1;
    }

    // The decoder rejects earlier malformed fields before considering RX.
    let mut words = original;
    words[0] &= !ARGUMENT_MASK;
    words[0] |= 3 << 27;
    assert!(matches!(Profile::decode(words), Err(Error::Step)));

    words = original;
    words[0] &= !ARGUMENT_MASK;
    words[1] = 5 << 26;
    assert!(matches!(Profile::decode(words), Err(Error::Route)));

    words = original;
    words[0] &= !ARGUMENT_MASK;
    words[2] = 1;
    assert!(matches!(Profile::decode(words), Err(Error::Reserved)));

    // An offset can remain representable while its effective TX frequency
    // lies outside the current RX band. Decoding must preserve that intent.
    let blocked = Profile {
        tune: Tune { frequency: 174_000_000, ..tune },
        transmit: Tx::Plus(600_000),
    };
    let restored = match Profile::decode(checked_words(blocked)) {
        Ok(profile) => profile,
        Err(_) => panic!("retained blocked route was rejected"),
    };
    assert!(matches!(restored.transmit, Tx::Plus(600_000)));
    assert!(restored.transmit.frequency(restored.tune.frequency).is_err());
};

const _: () = {
    assert!(core::mem::size_of::<Tune>() == 12);
    assert!(core::mem::size_of::<[u32; 3]>() == 12);

    let frequencies = [
        136_000_000, 145_500_000, 174_000_000,
        400_000_000, 433_506_250, 470_000_000,
    ];
    let steps = [12_500, 25_000, 100_000];

    let mut frequency = 0;
    while frequency < frequencies.len() {
        let mut step = 0;
        while step < steps.len() {
            let mut width = 0;
            while width < 2 {
                let profile = Profile::simplex(Tune {
                    frequency: frequencies[frequency],
                    step: steps[step],
                    am: false, narrow: width != 0,
                });
                roundtrip(profile);
                assert!(profile.v7_tune().is_some());
                width += 1;
            }
            step += 1;
        }
        frequency += 1;
    }

    let tune = Tune::initial(Band::Vhf);
    let routes = [
        Tx::Simplex,
        Tx::Plus(600_000),
        Tx::Minus(600_000),
        Tx::Split(433_500_000),
        Tx::ReceiveOnly,
    ];
    let mut route = 0;
    while route < routes.len() {
        let profile = Profile { tune, transmit: routes[route] };
        roundtrip(profile);
        assert!(profile.v7_tune().is_some() == (route == 0));
        route += 1;
    }

    assert!(matches!(Tx::Simplex.frequency(145_500_000), Ok(Some(145_500_000))));
    assert!(matches!(Tx::Plus(600_000).frequency(145_500_000), Ok(Some(146_100_000))));
    assert!(matches!(Tx::Minus(600_000).frequency(145_500_000), Ok(Some(144_900_000))));
    assert!(matches!(Tx::Split(433_500_000).frequency(145_500_000), Ok(Some(433_500_000))));
    assert!(matches!(Tx::ReceiveOnly.frequency(145_500_000), Ok(None)));

    assert!(matches!(Tx::Plus(0).frequency(145_500_000), Err(Error::Route)));
    assert!(matches!(Tx::Minus(1).frequency(145_500_000), Err(Error::Route)));
    assert!(matches!(Tx::Plus(10).frequency(174_000_000), Err(Error::Route)));
    assert!(matches!(Tx::Minus(10).frequency(400_000_000), Err(Error::Route)));
    assert!(matches!(
        Tx::Plus(4_000_000_000).frequency(470_000_000),
        Err(Error::Route),
    ));
    assert!(matches!(
        Tx::Minus(4_000_000_000).frequency(136_000_000),
        Err(Error::Route),
    ));
    assert!(matches!(
        Tx::Plus(288_000_000).frequency(145_500_000),
        Err(Error::Route),
    ));
    assert!(matches!(Tx::Split(280_000_000).frequency(145_500_000), Err(Error::Route)));
    assert!(matches!(Tx::Split(433_500_001).frequency(145_500_000), Err(Error::Route)));
    assert!(matches!(Tx::ReceiveOnly.frequency(0), Err(Error::Receive)));

    let valid = checked_words(Profile::simplex(tune));

    let mut bit = 29;
    while bit < 32 {
        let mut corrupted = valid;
        if bit == 31 {
            corrupted[0] |= 1 << bit;
            assert!(matches!(Profile::decode(corrupted), Err(Error::Reserved)));
        }
        corrupted = valid;
        corrupted[1] |= 1 << bit;
        assert!(matches!(Profile::decode(corrupted), Err(Error::Reserved)));
        bit += 1;
    }

    bit = 0;
    while bit < 32 {
        let mut corrupted = valid;
        corrupted[2] = 1 << bit;
        assert!(matches!(Profile::decode(corrupted), Err(Error::Reserved)));
        bit += 1;
    }

    let mut corrupted = valid;
    corrupted[0] |= 3 << 27;
    assert!(matches!(Profile::decode(corrupted), Err(Error::Step)));
    corrupted = valid;
    corrupted[1] = 1;
    assert!(matches!(Profile::decode(corrupted), Err(Error::Route)));
    corrupted[1] = (4 << 26) | 1;
    assert!(matches!(Profile::decode(corrupted), Err(Error::Route)));
    corrupted[1] = 5 << 26;
    assert!(matches!(Profile::decode(corrupted), Err(Error::Route)));
    assert!(Profile::decode([0; 3]).is_err());

    let blocked = Profile {
        tune: Tune { frequency: 174_000_000, ..tune },
        transmit: Tx::Plus(600_000),
    };
    assert!(blocked.encode().is_ok());
    assert!(blocked.transmit.frequency(blocked.tune.frequency).is_err());
    roundtrip(blocked);

    let route = match Tx::Plus(600_000).word() {
        Ok(value) => value,
        Err(_) => panic!("offset encoding rejected"),
    };
    assert!(matches!(Tx::decode(route), Ok(Tx::Plus(600_000))));
    assert!(Tx::Plus(70_000_000).word().is_ok());
    assert!(Tx::Plus(70_000_010).word().is_err());
    assert!(Tx::Minus(0).word().is_err());
    assert!(Tx::decode(7 << 26).is_err());

    let bad = Profile::simplex(Tune { step: 1, ..tune });
    assert!(matches!(bad.encode(), Err(Error::Step)));
    assert!(bad.v7_tune().is_none());
};
const _: () = {
    assert!(Band::of(118_000_000).is_none());
    assert!(matches!(Band::receive(108_000_000), Some(Band::Vhf)));
    assert!(matches!(Band::receive(135_999_990), Some(Band::Vhf)));
    assert!(Band::receive(49_999_990).is_none());
    assert!(Band::receive(118_000_001).is_none());
    assert!(Band::Vhf.shift_receive(50_000_000, 12_500, false) == 50_000_000);
    assert!(Band::Vhf.shift_receive(136_000_000, 12_500, false) == 135_987_500);
    assert!(Tx::Simplex.frequency(118_000_000).is_err());
    assert!(matches!(Tx::ReceiveOnly.frequency(118_000_000), Ok(None)));
    assert!(matches!(
        Tx::Split(145_500_000).frequency(118_000_000), Ok(Some(145_500_000)),
    ));
    assert!(Tx::Split(118_000_000).word().is_err());

    let frequencies = [108_000_000, 118_025_000, 145_500_000, 433_500_000];
    let mut index = 0;
    while index < frequencies.len() {
        let mut flags = 0;
        while flags < 4 {
            let profile = Profile::simplex(Tune {
                frequency: frequencies[index], step: 25_000,
                narrow: flags & 1 != 0, am: flags & 2 != 0,
            });
            assert!(profile.tune.flags() == flags);
            roundtrip(profile);
            if profile.tune.am {
                assert!(matches!(profile.transmit_frequency(), Ok(None)));
                assert!(profile.v7_tune().is_none());
            }
            flags += 1;
        }
        index += 1;
    }
};

const _: () = {
    let frequencies = [
        50_000_000, 107_999_990, 174_000_010, 250_000_000,
        279_999_990, 280_000_000, 350_000_000, 399_999_990,
        470_000_010, 500_000_000, 629_999_990,
    ];
    let mut index = 0;
    while index < frequencies.len() {
        let frequency = frequencies[index];
        let band = match Band::receive(frequency) {
            Some(band) => band,
            None => panic!("extended receive frequency rejected"),
        };
        assert!(frequency >= band.rx_lower() && frequency <= band.rx_upper());
        assert!(Band::of(frequency).is_none());
        assert!(Band::receive_v16(frequency).is_none());
        assert!(Tx::Simplex.frequency(frequency).is_err());
        assert!(Tx::Split(frequency).word().is_err());

        let mut flags = 0;
        while flags < 4 {
            let profile = Profile {
                tune: Tune {
                    frequency, step: 12_500,
                    narrow: flags & 1 != 0, am: flags & 2 != 0,
                },
                transmit: Tx::ReceiveOnly,
            };
            roundtrip(profile);
            assert!(matches!(profile.transmit_frequency(), Ok(None)));
            flags += 1;
        }
        index += 1;
    }

    assert!(Band::receive(630_000_000).is_none());
    assert!(Band::receive(839_999_990).is_none());
    assert!(Band::receive(u32::MAX).is_none());
    assert!(Band::receive(500_000_001).is_none());

    assert!(matches!(Band::receive(279_999_990), Some(Band::Vhf)));
    assert!(matches!(Band::receive(280_000_000), Some(Band::Uhf)));
    assert!(Band::Vhf.shift_receive(279_999_990, 12_500, true) == 279_999_990);
    assert!(Band::Uhf.shift_receive(280_000_000, 12_500, false) == 280_000_000);
    assert!(Band::Uhf.shift_receive(629_999_990, 12_500, true) == 629_999_990);
    assert!(Band::Uhf.shift_receive(500_000_000, 12_500, true) == 500_012_500);

    // Receive extension must not change the bounds used by TX offsets.
    assert!(Band::Vhf.lower() == 136_000_000);
    assert!(Band::Vhf.upper() == 174_000_000);
    assert!(Band::Uhf.lower() == 400_000_000);
    assert!(Band::Uhf.upper() == 470_000_000);
    assert!(Tx::Plus(10).frequency(174_000_000).is_err());
    assert!(Tx::Minus(10).frequency(400_000_000).is_err());
};

const _: () = {
    let frequencies = [840_000_000, 999_999_990, 1_000_000_000, 1_300_000_000];
    let mut index = 0;
    while index < frequencies.len() {
        let frequency = frequencies[index];
        assert!(matches!(Band::receive(frequency), Some(Band::Uhf)));
        assert!(Band::receive_v17(frequency).is_none());
        assert!(Band::of(frequency).is_none());
        assert!(Tx::Simplex.frequency(frequency).is_err());
        assert!(Tx::Split(frequency).word().is_err());
        assert!(receive_span(frequency) == 3);

        let mut flags = 0;
        while flags < 4 {
            let profile = Profile {
                tune: Tune {
                    frequency, step: 12_500,
                    narrow: flags & 1 != 0, am: flags & 2 != 0,
                },
                transmit: Tx::ReceiveOnly,
            };
            roundtrip(profile);
            let words = checked_words(profile);
            assert!(words[0] & (1 << 30) != 0);
            assert!(words[0] & (1 << 31) == 0);
            assert!(words[1] == 4 << 26);
            flags += 1;
        }
        index += 1;
    }

    assert!(Band::receive(630_000_000).is_none());
    assert!(Band::receive(839_999_990).is_none());
    assert!(Band::receive(1_300_000_010).is_none());
    assert!(Band::receive(1_000_000_001).is_none());
    assert!(Band::Uhf.shift_receive(629_999_990, 100_000, true) == 629_999_990);
    assert!(Band::Uhf.shift_receive(840_000_000, 100_000, false) == 840_000_000);
    assert!(Band::Uhf.shift_receive(1_300_000_000, 100_000, true) == 1_300_000_000);

    let lower = Profile::simplex(Tune::initial(Band::Vhf));
    assert!(checked_words(lower)[0] == 14_550_000);
    assert!(checked_words(lower)[0] & (1 << 30) == 0);
};
