//! Command recognition independent of USART, DMA, and RF.
//!
//! Each byte is offered to channel framing first. Active or newly entered
//! binary framing suppresses the screenshot matcher, including the body's
//! final CRC byte. A pending complete import does not prevent screenshots
//! or export requests, but another import body is drained without overwrite.
//!
//! The caller polls expiry using observed ticks and resets partial framing
//! after transport loss. Reset never releases a complete staged import.
//! Events request work; they do not acknowledge application or write storage.
//!
//! One production Requests instance owns the static channel payload.
//! Multiple physical transports must share or arbitrate that owner rather
//! than independently borrowing the same payload.

pub(crate) mod channels;
pub(crate) mod packets;

const REQUEST: &[u8; 5] = b"TKSS\n";

struct Matcher {
    index: usize,
}

impl Matcher {
    const fn new() -> Self {
        Self { index: 0 }
    }

    const fn byte(&mut self, value: u8) -> bool {
        if value == b'\r' {
            return false;
        }
        if value == REQUEST[self.index] {
            self.index += 1;
            if self.index == REQUEST.len() {
                self.index = 0;
                return true;
            }
        } else {
            self.index = if value == REQUEST[0] { 1 } else { 0 };
        }
        false
    }
}

pub(crate) enum Event {
    None,
    Capture,
    Export,
    Import,
}

pub(crate) struct Requests<'a> {
    matcher: Matcher,
    channels: channels::Decoder<'a>,
}

impl Requests<'static> {
    /// Construct once with no other owner of the static import payload.
    /// Initialization changes parser state only and performs no device I/O.
    pub(crate) unsafe fn new() -> Self {
        Self::with_channels(unsafe { channels::Decoder::new() })
    }
}

impl<'a> Requests<'a> {
    const fn with_channels(channels: channels::Decoder<'a>) -> Self {
        Self { matcher: Matcher::new(), channels }
    }

    pub(crate) const fn version(&self) -> u8 {
        self.channels.version()
    }

    pub(crate) const fn pending(&self) -> bool {
        self.channels.pending()
    }

    pub(crate) const fn data(&self) -> Option<&[u8]> {
        self.channels.data()
    }

    pub(crate) const fn finish(&mut self) {
        self.channels.finish();
    }

    pub(crate) const fn reset_partial(&mut self) {
        self.matcher = Matcher::new();
        self.channels.reset_partial();
    }

    pub(crate) const fn expire(&mut self, now: u32) {
        self.channels.expire(now);
    }

    pub(crate) const fn byte(&mut self, value: u8, now: u32) -> Event {
        let was_binary = self.channels.receiving();
        let channel = self.channels.byte(value, now);

        if was_binary || self.channels.receiving() {
            self.matcher = Matcher::new();
        } else if self.matcher.byte(value) {
            return Event::Capture;
        }

        match channel {
            channels::Event::None => Event::None,
            channels::Event::Export => Event::Export,
            channels::Event::Import => Event::Import,
        }
    }
}

const fn quiet(requests: &mut Requests<'_>, bytes: &[u8], now: u32) {
    let mut index = 0;
    while index < bytes.len() {
        assert!(matches!(requests.byte(bytes[index], now), Event::None));
        index += 1;
    }
}

const _: () = {
    let mut matcher = Matcher::new();
    assert!(!matcher.byte(b'T'));
    assert!(!matcher.byte(b'K'));
    assert!(!matcher.byte(b'S'));
    assert!(!matcher.byte(b'S'));
    assert!(matcher.byte(b'\n'));
    assert!(matcher.index == 0);

    let mut overlap = Matcher::new();
    assert!(!overlap.byte(b'T'));
    assert!(!overlap.byte(b'T'));
    assert!(!overlap.byte(b'K'));
    assert!(!overlap.byte(b'S'));
    assert!(!overlap.byte(b'S'));
    assert!(!overlap.byte(b'\r'));
    assert!(overlap.byte(b'\n'));

    let mut rejected = Matcher::new();
    assert!(!rejected.byte(b'T'));
    assert!(!rejected.byte(b'K'));
    assert!(!rejected.byte(b'X'));
    assert!(!rejected.byte(b'S'));
    assert!(!rejected.byte(b'S'));
    assert!(!rejected.byte(b'\n'));

    let stream = b"TRAKT tick=00000000\r\nTKSS\nTKSS\n";
    let mut count = 0;
    let mut index = 0;
    let mut parser = Matcher::new();
    while index < stream.len() {
        if parser.byte(stream[index]) {
            count += 1;
        }
        index += 1;
    }
    assert!(count == 2);
};

const _: () = {
    let mut payload = channels::Payload { bytes: [0; 324] };
    let decoder = channels::Decoder::with_payload(&mut payload);
    let mut requests = Requests::with_channels(decoder);
    let packet = channels::fixture();

    quiet(&mut requests, b"TKSS", 0);
    assert!(matches!(requests.byte(b'\n', 0), Event::Capture));
    quiet(&mut requests, b"TKCE", 0);
    assert!(matches!(requests.byte(b'\n', 0), Event::Export));

    // Command-looking bytes inside the fixture remain binary data.
    quiet(&mut requests, b"TKCI\n", 1);
    quiet(&mut requests, packet.split_at(323).0, 1);
    assert!(matches!(requests.byte(packet[323], 1), Event::Import));
    assert!(requests.pending());

    let staged = match requests.data() {
        Some(bytes) => bytes,
        None => panic!("combined parser lost staged payload"),
    };
    let mut index = 0;
    while index < 320 {
        assert!(staged[index] == packet[index]);
        index += 1;
    }

    quiet(&mut requests, b"TKSS\r", 2);
    assert!(matches!(requests.byte(b'\n', 2), Event::Capture));

    // The second import is drained, even if application releases the first.
    quiet(&mut requests, b"TKCI\n", 3);
    quiet(&mut requests, packet.split_at(17).0, 3);
    requests.finish();
    quiet(&mut requests, packet.split_at(17).1, 3);
    assert!(!requests.pending());

    // A command cannot span a transport framing reset.
    quiet(&mut requests, b"TKS", 4);
    requests.reset_partial();
    quiet(&mut requests, b"S\n", 4);

    quiet(&mut requests, b"TKCI\n", 5);
    quiet(&mut requests, packet.split_at(319).0, 5);
    requests.expire(1006);
    assert!(!requests.pending());
    quiet(&mut requests, b"TKSS", 1007);
    assert!(matches!(requests.byte(b'\n', 1007), Event::Capture));

    // Complete payload ownership survives subsequent transport loss.
    quiet(&mut requests, b"TKCI\n", 1010);
    quiet(&mut requests, packet.split_at(323).0, 1010);
    assert!(matches!(requests.byte(packet[323], 1010), Event::Import));
    quiet(&mut requests, b"TK", 1011);
    requests.reset_partial();
    assert!(requests.pending());
    quiet(&mut requests, b"SS\n", 1012);
    assert!(requests.version() == 1);

    requests.finish();
    quiet(&mut requests, b"TKDI\n", 1020);
    quiet(&mut requests, packet.split_at(323).0, 1020);
    assert!(matches!(requests.byte(packet[323], 1020), Event::Import));
    assert!(requests.version() == 2);

    // A compatibility header cannot replace the staged schema-two bank.
    quiet(&mut requests, b"TKCI\n", 1021);
    quiet(&mut requests, &packet, 1021);
    assert!(requests.version() == 2);
    requests.reset_partial();
    assert!(requests.version() == 2);
    requests.finish();
    assert!(requests.version() == 0);

    quiet(&mut requests, b"TKDE\n", 1022);
    quiet(&mut requests, b"TKEI\n", 1023);
    quiet(&mut requests, packet.split_at(323).0, 1023);
    assert!(matches!(requests.byte(packet[323], 1023), Event::Import));
    assert!(requests.version() == 3);
    quiet(&mut requests, b"TKDI\n", 1024);
    quiet(&mut requests, &packet, 1024);
    assert!(requests.version() == 3);

    requests.finish();
    quiet(&mut requests, b"TKFI\n", 1025);
    quiet(&mut requests, packet.split_at(323).0, 1025);
    assert!(matches!(requests.byte(packet[323], 1025), Event::Import));
    assert!(requests.version() == 4);
    quiet(&mut requests, b"TKEI\n", 1026);
    quiet(&mut requests, &packet, 1026);
    assert!(requests.version() == 4);

    requests.finish();
    quiet(&mut requests, b"TKGI\n", 1027);
    quiet(&mut requests, packet.split_at(323).0, 1027);
    assert!(matches!(requests.byte(packet[323], 1027), Event::Import));
    assert!(requests.version() == 5);
    quiet(&mut requests, b"TKFI\n", 1028);
    quiet(&mut requests, &packet, 1028);
    assert!(requests.version() == 5);

    requests.finish();
    quiet(&mut requests, b"TKHI\n", 1029);
    quiet(&mut requests, packet.split_at(323).0, 1029);
    assert!(matches!(requests.byte(packet[323], 1029), Event::Import));
    assert!(requests.version() == 6);
    quiet(&mut requests, b"TKGI\n", 1030);
    quiet(&mut requests, &packet, 1030);
    assert!(requests.version() == 6);
};
const _: () = {
    let mut payload = channels::Payload { bytes: [0; 324] };
    let mut requests = Requests::with_channels(
        channels::Decoder::with_payload(&mut payload),
    );
    let packet = channels::fixture();
    quiet(&mut requests, b"TKII\n", 0);
    quiet(&mut requests, packet.split_at(323).0, 0);
    assert!(matches!(requests.byte(packet[323], 0), Event::Import));
    assert!(requests.version() == 7);

    quiet(&mut requests, b"TKHI\n", 1);
    quiet(&mut requests, &packet, 1);
    assert!(requests.version() == 7);
    requests.finish();
    quiet(&mut requests, b"TKHI\n", 2);
    quiet(&mut requests, packet.split_at(323).0, 2);
    assert!(matches!(requests.byte(packet[323], 2), Event::Import));
    assert!(requests.version() == 6);
};

const _: () = {
    let mut payload = channels::Payload { bytes: [0; 324] };
    let mut requests = Requests::with_channels(
        channels::Decoder::with_payload(&mut payload),
    );
    let packet = channels::fixture();

    quiet(&mut requests, b"TKJI\n", 0);
    quiet(&mut requests, packet.split_at(323).0, 0);
    assert!(matches!(requests.byte(packet[323], 0), Event::Import));
    assert!(requests.version() == 8);

    quiet(&mut requests, b"TKII\n", 1);
    quiet(&mut requests, &packet, 1);
    assert!(requests.version() == 8);
    requests.finish();

    quiet(&mut requests, b"TKII\n", 2);
    quiet(&mut requests, packet.split_at(323).0, 2);
    assert!(matches!(requests.byte(packet[323], 2), Event::Import));
    assert!(requests.version() == 7);
};

const _: () = {
    let mut payload = channels::Payload { bytes: [0; 324] };
    let mut requests = Requests::with_channels(
        channels::Decoder::with_payload(&mut payload),
    );
    let packet = channels::fixture();
    quiet(&mut requests, b"TKKI\n", 0);
    quiet(&mut requests, packet.split_at(323).0, 0);
    assert!(matches!(requests.byte(packet[323], 0), Event::Import));
    assert!(requests.version() == 9);
    quiet(&mut requests, b"TKJI\n", 1);
    quiet(&mut requests, &packet, 1);
    assert!(requests.version() == 9);
};
