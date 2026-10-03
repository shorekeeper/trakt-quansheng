//! Complete channel value and explicit twenty-byte wire slot.
//!
//! The decoder accepts these wire contracts:
//!   Schema 1: frequency Hz, step Hz, width word, eight-byte name; simplex only
//!   Schema 2: packed profile and name; both signaling halves zero
//!   Schema 3: packed route and TX CTCSS; RX signaling zero
//!   Schema 4: packed route and independent RX/TX CTCSS
//!   Schema 5: TX CTCSS/DCS with RX CTCSS
//!   Schema 6: independent RX/TX CTCSS/DCS
//!
//! Schema 7 adds AM and 108..136 MHz RX. Schema 8 extends RX to 50..<630 MHz.
//! Schema 9 adds the upper RX interval and the extra packed frequency bit.
//! Encoding emits schema 9. All schemas use twenty zero bytes for an empty
//! slot. Populated names contain eight printable ASCII bytes. Reserved fields
//! are checked according to the supplied schema; they are never reinterpreted
//! solely because their bits resemble another supported value.
//!
//! Route validation checks representation, not current TX authorization.
//! An offset can remain stored while its effective TX falls outside the
//! selected band. Transmission must reject that result without erasing it.
//!
//! Decode returns None for malformed data, Some(None) for an empty slot,
//! or Some(Some(channel)) for a populated slot. Exact length and schema
//! admission precede indexing. Encoding validates before changing output.
//! No native Rust object layout is serialized.

use crate::profile::{Profile, Tune, Tx};

#[derive(Clone, Copy)]
pub(crate) struct Channel {
    pub(crate) tune: Tune,
    pub(crate) transmit: Tx,
    pub(crate) tx_signal: u16,
    pub(crate) rx_signal: u16,
    pub(crate) name: [u8; 8],
}

const _: () = {
    let original = Channel {
        tune: Tune {
            frequency: 433_500_000, step: 12_500, narrow: false, am: false,
        },
        transmit: Tx::Simplex,
        tx_signal: 0,
        rx_signal: 0,
        name: *b"CHECKED ",
    };

    let mut field = 0;
    while field < 6 {
        let mut invalid = original;
        match field {
            0 => invalid.tune.frequency = 700_000_000,
            1 => invalid.tune.step = 1,
            2 => invalid.transmit = Tx::Plus(0),
            3 => invalid.tx_signal = 1001,
            4 => invalid.rx_signal = 0x8213,
            _ => invalid.name[7] = 127,
        }

        assert!(!invalid.valid());
        let mut bytes = [0xA5u8; 20];
        assert!(!invalid.encode(&mut bytes));
        let mut index = 0;
        while index < bytes.len() {
            assert!(bytes[index] == 0xA5);
            index += 1;
        }
        field += 1;
    }

    let mut valid = [0u8; 20];
    assert!(original.encode(&mut valid));

    let mut bad_name = valid;
    bad_name[19] = 127;
    assert!(Channel::decode(&bad_name, 9).is_none());

    let mut bad_signal = valid;
    put(&mut bad_signal, 8, 0x8213);
    assert!(Channel::decode(&bad_signal, 9).is_none());

    let mut bad_route = valid;
    put(&mut bad_route, 4, 5 << 26);
    assert!(Channel::decode(&bad_route, 9).is_none());

    let mut bad_tune = valid;
    put(&mut bad_tune, 0, 70_000_000);
    assert!(Channel::decode(&bad_tune, 9).is_none());

    let mut legacy = [b' '; 20];
    put(&mut legacy, 0, 433_500_000);
    put(&mut legacy, 4, 1);
    put(&mut legacy, 8, 0);
    assert!(Channel::decode(&legacy, 1).is_none());
};

const fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

const fn put(bytes: &mut [u8], at: usize, value: u32) {
    let encoded = value.to_le_bytes();
    let mut index = 0;
    while index < 4 {
        bytes[at + index] = encoded[index];
        index += 1;
    }
}

impl Channel {
    pub(crate) const fn profile(self) -> Profile {
        Profile { tune: self.tune, transmit: self.transmit }
    }

    pub(crate) const fn valid(self) -> bool {
        self.tune.valid()
            && self.transmit.word().is_ok()
            && self.metadata_valid()
    }

    /// Validate the fields outside Profile. Encoding and decoding use this
    /// after their own profile check, without encoding that profile again.
    /// The caller must separately establish valid tuning and route data.
    #[inline(never)]
    const fn metadata_valid(&self) -> bool {
        if !crate::tx_signal::valid(self.tx_signal as u32)
            || !crate::tx_signal::valid(self.rx_signal as u32)
        {
            return false;
        }

        let mut index = 0;
        while index < self.name.len() {
            if self.name[index] < 32 || self.name[index] > 126 {
                return false;
            }
            index += 1;
        }
        true
    }

    pub(crate) const fn named(self) -> bool {
        let mut index = 0;
        while index < self.name.len() {
            if self.name[index] != b' ' { return true; }
            index += 1;
        }
        false
    }

    /// Writes schema nine. Export retains the route and both signaling directions.
    pub(crate) const fn encode(self, bytes: &mut [u8]) -> bool {
        // All fields validate before the first destination write.
        if bytes.len() != 20 || !self.metadata_valid() { return false; }
        let mut words = match self.profile().encode() {
            Ok(words) => words,
            Err(_) => return false,
        };
        words[2] = ((self.tx_signal as u32) << 16) | self.rx_signal as u32;
        let mut index = 0;
        while index < 3 {
            put(bytes, index * 4, words[index]);
            index += 1;
        }
        index = 0;
        while index < 8 {
            bytes[12 + index] = self.name[index];
            index += 1;
        }
        true
    }

    pub(crate) const fn decode(bytes: &[u8], version: u8) -> Option<Option<Self>> {
        if bytes.len() != 20 || !matches!(version, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9) { return None; }

        let mut empty = true;
        let mut index = 0;
        while index < bytes.len() {
            empty = empty && bytes[index] == 0;
            index += 1;
        }
        if empty { return Some(None); }

        let profile = if version == 1 {
            let narrow = word(bytes, 8);
            if narrow > 1 { return None; }
            let tune = Tune {
                frequency: word(bytes, 0),
                step: word(bytes, 4),
                am: false, narrow: narrow != 0,
            };
            if !tune.valid() {
                return None;
            }
            Profile::simplex(tune)
        } else {
            if (version == 2 && word(bytes, 8) != 0)
                || (version == 3 && word(bytes, 8) & 65535 != 0)
                || (version < 5 && !crate::ctcss::valid(word(bytes, 8) >> 16))
                || (version < 6 && !crate::ctcss::valid(word(bytes, 8) & 65535))
            {
                return None;
            }
            match Profile::decode([word(bytes, 0), word(bytes, 4), 0]) {
                Ok(profile) => profile,
                Err(_) => return None,
            }
        };

        if version == 8
            && crate::profile::Band::receive_v17(profile.tune.frequency).is_none()
        {
            return None;
        }

        if version == 7
            && crate::profile::Band::receive_v16(profile.tune.frequency).is_none()
        {
            return None;
        }

        if version < 7
            && (profile.tune.am || crate::profile::Band::of(profile.tune.frequency).is_none())
        {
            return None;
        }

        let mut name = [0; 8];
        index = 0;
        while index < name.len() {
            name[index] = bytes[12 + index];
            index += 1;
        }

        // Schema-specific restrictions were checked above. The complete
        // metadata check below validates both generic signaling selectors.
        let tx_signal = if version >= 3 {
            (word(bytes, 8) >> 16) as u16
        } else {
            0
        };
        let channel = Self {
            tune: profile.tune,
            transmit: profile.transmit,
            tx_signal,
            rx_signal: if version >= 4 { word(bytes, 8) as u16 } else { 0 },
            name,
        };
        // Both profile construction branches established tuning and route
        // validity. Only signaling and printable name bytes remain.
        if channel.metadata_valid() { Some(Some(channel)) } else { None }
    }
}

const _: () = {
    let channel = Channel {
        tune: Tune { frequency: 433_700_000, step: 12_500, am: false, narrow: true },
        transmit: Tx::Simplex,
        tx_signal: 0xC013,
        rx_signal: 885,
        name: *b"DCS TEST",
    };
    let mut bytes = [0u8; 20];
    assert!(channel.encode(&mut bytes));
    assert!(Channel::decode(&bytes, 4).is_none());
    let restored = match Channel::decode(&bytes, 5) {
        Some(Some(channel)) => channel,
        _ => panic!("DCS channel rejected"),
    };
    assert!(restored.tx_signal == 0xC013 && restored.rx_signal == 885);

    let mut digital_rx = channel;
    digital_rx.rx_signal = 0x8015;
    assert!(digital_rx.encode(&mut bytes));
    assert!(Channel::decode(&bytes, 5).is_none());
    let received = match Channel::decode(&bytes, 6) {
        Some(Some(channel)) => channel,
        _ => panic!("independent digital RX channel rejected"),
    };
    assert!(received.tx_signal == 0xC013 && received.rx_signal == 0x8015);

    let mut invalid = channel;
    invalid.rx_signal = 0x8213;
    assert!(!invalid.valid());
};

const _: () = {
    let tune = Tune { frequency: 145_700_000, step: 12_500, am: false, narrow: true };
    let routes = [
        Tx::Simplex, Tx::Plus(600_000), Tx::Minus(600_000),
        Tx::Split(433_700_000), Tx::ReceiveOnly,
    ];

    let mut index = 0;
    while index < routes.len() {
        let channel = Channel {
            tune, transmit: routes[index], tx_signal: 1000,
            rx_signal: 885, name: *b"PROFILE ",
        };
        let mut bytes = [0u8; 20];
        assert!(channel.encode(&mut bytes));
        assert!(Channel::decode(&bytes, 2).is_none());
        assert!(Channel::decode(&bytes, 3).is_none());
        let restored = match Channel::decode(&bytes, 4) {
            Some(Some(channel)) => channel,
            _ => panic!("channel roundtrip rejected"),
        };
        assert!(restored.tune.frequency == tune.frequency);
        assert!(restored.tune.narrow);
        assert!(restored.tx_signal == 1000);
        assert!(restored.rx_signal == 885);
        let mut repeated = [0u8; 20];
        assert!(restored.encode(&mut repeated));
        let mut byte = 0;
        while byte < 20 {
            assert!(bytes[byte] == repeated[byte]);
            byte += 1;
        }
        index += 1;
    }

    let mut legacy = [b' '; 20];
    put(&mut legacy, 0, tune.frequency);
    put(&mut legacy, 4, tune.step);
    put(&mut legacy, 8, 1);
    let old = match Channel::decode(&legacy, 1) {
        Some(Some(channel)) => channel,
        _ => panic!("legacy channel rejected"),
    };
    assert!(matches!(old.transmit, Tx::Simplex));
    assert!(matches!(Channel::decode(&[0; 20], 1), Some(None)));
    assert!(matches!(Channel::decode(&[0; 20], 2), Some(None)));
    assert!(Channel::decode(&[0; 19], 2).is_none());
    assert!(Channel::decode(&[0; 21], 2).is_none());
    assert!(Channel::decode(&[0; 20], 10).is_none());

    let mut malformed = [0u8; 20];
    malformed[19] = b' ';
    assert!(Channel::decode(&malformed, 2).is_none());
    legacy[12] = 0;
    assert!(Channel::decode(&legacy, 1).is_none());

    let bad = Channel {
        tune, transmit: Tx::Plus(0), tx_signal: 0, rx_signal: 0, name: *b"INVALID ",
    };
    let mut untouched = [0xA5; 20];
    assert!(!bad.encode(&mut untouched));
    index = 0;
    while index < 20 {
        assert!(untouched[index] == 0xA5);
        index += 1;
    }
};
const _: () = {
    let mut channel = Channel {
        tune: Tune {
            frequency: 118_025_000, step: 25_000, narrow: true, am: true,
        },
        transmit: Tx::ReceiveOnly,
        tx_signal: 0x8013,
        rx_signal: 885,
        name: *b"AIR TEST",
    };
    let mut bytes = [0u8; 20];
    assert!(channel.encode(&mut bytes));
    let loaded = match Channel::decode(&bytes, 7) {
        Some(Some(channel)) => channel,
        _ => panic!("AM channel rejected"),
    };
    assert!(loaded.tune.am && loaded.tune.narrow);
    assert!(loaded.tune.frequency == 118_025_000);
    assert!(loaded.rx_signal == 885 && loaded.tx_signal == 0x8013);
    assert!(matches!(loaded.profile().transmit_frequency(), Ok(None)));

    let mut schema = 1;
    while schema < 7 {
        assert!(Channel::decode(&bytes, schema).is_none());
        schema += 1;
    }

    channel.tune.frequency = 433_500_000;
    assert!(channel.encode(&mut bytes));
    assert!(Channel::decode(&bytes, 6).is_none());

    channel.tune.am = false;
    assert!(channel.encode(&mut bytes));
    assert!(Channel::decode(&bytes, 6).is_some());

    channel.tune.frequency = 118_025_000;
    assert!(channel.encode(&mut bytes));
    assert!(Channel::decode(&bytes, 6).is_none());
    assert!(Channel::decode(&bytes, 7).is_some());
};

const _: () = {
    let frequencies = [50_000_000, 250_000_000, 350_000_000, 629_999_990];
    let mut index = 0;
    while index < frequencies.len() {
        let channel = Channel {
            tune: Tune {
                frequency: frequencies[index], step: 25_000,
                narrow: index & 1 != 0, am: index & 2 != 0,
            },
            transmit: Tx::ReceiveOnly,
            tx_signal: 0x8013,
            rx_signal: 885,
            name: *b"WIDE RX ",
        };
        let mut bytes = [0u8; 20];
        assert!(channel.encode(&mut bytes));
        assert!(Channel::decode(&bytes, 7).is_none());
        let loaded = match Channel::decode(&bytes, 8) {
            Some(Some(value)) => value,
            _ => panic!("extended channel rejected"),
        };
        assert!(loaded.tune.frequency == frequencies[index]);
        assert!(loaded.tune.flags() == channel.tune.flags());
        assert!(loaded.rx_signal == 885 && loaded.tx_signal == 0x8013);

        let mut repeated = [0u8; 20];
        assert!(loaded.encode(&mut repeated));
        let mut byte = 0;
        while byte < bytes.len() {
            assert!(bytes[byte] == repeated[byte]);
            byte += 1;
        }
        index += 1;
    }
};

const _: () = {
    let channel = Channel {
        tune: Tune {
            frequency: 1_300_000_000, step: 25_000, narrow: true, am: true,
        },
        transmit: Tx::ReceiveOnly,
        tx_signal: 0x8013, rx_signal: 885,
        name: *b"UPPER RX",
    };
    let mut bytes = [0u8; 20];
    assert!(channel.encode(&mut bytes));
    let mut version = 1;
    while version < 9 {
        assert!(Channel::decode(&bytes, version).is_none());
        version += 1;
    }
    let decoded = match Channel::decode(&bytes, 9) {
        Some(Some(value)) => value,
        _ => panic!("upper receive channel rejected"),
    };
    assert!(decoded.tune.frequency == 1_300_000_000);
    assert!(decoded.tune.flags() == 3);
    assert!(decoded.tx_signal == 0x8013 && decoded.rx_signal == 885);

    let mut repeated = [0u8; 20];
    assert!(decoded.encode(&mut repeated));
    let mut index = 0;
    while index < bytes.len() {
        assert!(bytes[index] == repeated[index]);
        index += 1;
    }
};
