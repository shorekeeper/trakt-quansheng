//! CW envelope qualification and bounded Morse text decoding.
//!
//! Envelope consumes raw nine-bit RSSI observations. Opening and closing
//! use separate supplied thresholds and require five observed ticks of
//! persistence. A service gap above twenty ticks discards qualification.
//! Optional speaker admission remains open for fifteen ticks after a
//! qualified mark ends. This is an RSSI gate, not an audio-frequency filter.
//!
//! Decoder consumes qualified mark/space observations. Its speed is the
//! selected 5..35 WPM, shared with the electronic keyer. It does not estimate
//! speed, identify a station, or decode audio-frequency tones carried by a
//! continuous FM carrier. A transmitter with a different speed requires
//! adjusting the selection.
//!
//! Dot duration is round(1200 / WPM) observed ticks. Marks below one quarter
//! of that duration are ignored. Marks below two units are dots; marks from
//! two through five units are dashes. Longer marks invalidate the symbol.
//! Three units of space complete a character; seven insert one word space.
//! Letters, digits, and common punctuation occupy a fixed six-element tree.
//! Unsupported or malformed symbols produce a question mark.
//!
//! The first asserted observation after construction or synchronization loss
//! is not a complete mark and is discarded. A gap above twenty ticks drops
//! the pending symbol rather than treating missing observations as silence.
//! Suspension preserves completed text but discards partial timing.
//!
//! cw-debug and host fixtures additionally retain completed qualified
//! mark/space durations and the maximum observed sample interval. Ordinary
//! firmware stores neither those counters nor their transition timestamps.
//! Contact filtering, stale-sample rejection, and audio-tail timing remain
//! operational independently of these diagnostic observations.
//!
//! Probe combines those optional durations with the receiver's cached raw
//! RSSI and thresholds. Durations use qualified transitions, not measured
//! RF edges. The first partial interval after reset is omitted. A stale
//! interval clears durations but remains visible in the maximum-gap value.
//! Probe changes no detection threshold.
//!
//! live projects the pending symbol without changing decoder state or text.
//! Completed elements appear immediately. A currently observed mark first
//! projects as a dot and changes to a dash at two units; unknown or oversized
//! sequences project a question mark. This is a provisional character, not a
//! prediction of additional elements. It disappears when emit commits the
//! final character. Initial partial and stale marks cannot create a preview.
//!
//! Each decoder additionally owns an eighteen-cell display line containing
//! committed characters only. Cells never shift. After the line fills, the
//! next non-space character clears that line and starts at column zero.
//! Leading spaces and spaces beyond a full line are omitted from this
//! display projection, not from retained history. Suspension preserves the
//! line; explicit clear resets it. RX and TX advance independently.
//!
//! RX and TX use separate display lines and timing instances. Firmware
//! retains the last committed byte for session separation, not another
//! scrolling history. Host fixtures additionally observe the emitted stream
//! through a twenty-eight-character reference history.
//!
//! end separates TX sessions with a space and flushes only fully observed,
//! completed marks. That separator denotes a session boundary, not a measured
//! Morse word gap. A separator omitted at the full display-line boundary
//! still updates the last committed byte and cannot repeat on the next end.
//!
//! These objects perform no peripheral access and cannot authorize RF.
//! All durations refer to observed foreground ticks, not measured RF time.

use crate::app::time::reached;

#[cfg(test)]
const HISTORY: usize = 28;
pub(crate) const DISPLAY_COLUMNS: usize = 18;
const SAMPLE_LIMIT: u32 = 20;
const CONTACT_TICKS: u32 = 5;
const AUDIO_TAIL: u32 = 15;

/// Operational observations from one receiver-owned envelope. Counts are
/// raw nine-bit RSSI units; durations and sample intervals are observed ticks.
#[cfg(any(feature = "cw-debug", test))]
#[derive(Clone, Copy)]
pub(crate) struct Probe {
    pub(crate) rssi: u32,
    pub(crate) open: u32,
    pub(crate) close: u32,
    pub(crate) down: bool,
    pub(crate) mark_ticks: u32,
    pub(crate) space_ticks: u32,
    pub(crate) max_gap: u32,
}

pub(crate) struct Envelope {
    sampled: bool,
    last: u32,
    candidate: bool,
    since: u32,
    mark: bool,
    audio_tail: bool,
    audio_until: u32,
    #[cfg(any(feature = "cw-debug", test))]
    edge_at: u32,
    #[cfg(any(feature = "cw-debug", test))]
    edge_known: bool,
    #[cfg(any(feature = "cw-debug", test))]
    mark_ticks: u32,
    #[cfg(any(feature = "cw-debug", test))]
    space_ticks: u32,
    #[cfg(any(feature = "cw-debug", test))]
    max_gap: u32,
}

impl Envelope {
    pub(crate) const fn new() -> Self {
        Self {
            sampled: false,
            last: 0,
            candidate: false,
            since: 0,
            mark: false,
            audio_tail: false,
            audio_until: 0,
            #[cfg(any(feature = "cw-debug", test))]
            edge_at: 0,
            #[cfg(any(feature = "cw-debug", test))]
            edge_known: false,
            #[cfg(any(feature = "cw-debug", test))]
            mark_ticks: 0,
            #[cfg(any(feature = "cw-debug", test))]
            space_ticks: 0,
            #[cfg(any(feature = "cw-debug", test))]
            max_gap: 0,
        }
    }

    pub(crate) const fn reset(&mut self) {
        *self = Self::new();
    }

    /// The caller supplies one fresh RSSI observation and checked hysteresis
    /// thresholds. Repeated observations at one tick cannot complete a filter.
    pub(crate) const fn poll(
        &mut self,
        now: u32,
        rssi: u32,
        open: u32,
        close: u32,
    ) {
        assert!(rssi <= 511 && open <= 511 && close <= open);

        let gap = if self.sampled { now.wrapping_sub(self.last) } else { 0 };

        #[cfg(any(feature = "cw-debug", test))]
        let maximum = if gap > self.max_gap { gap } else { self.max_gap };

        // Continuity is required in every composition. Only the optional
        // maximum-gap observation survives this reset.
        if gap > SAMPLE_LIMIT {
            self.reset();
        }

        #[cfg(any(feature = "cw-debug", test))]
        {
            self.max_gap = maximum;
        }

        self.sampled = true;
        self.last = now;

        let wanted = rssi >= if self.mark { close } else { open };
        if wanted != self.candidate {
            self.candidate = wanted;
            self.since = now;
        }

        if wanted != self.mark
            && now.wrapping_sub(self.since) >= CONTACT_TICKS
        {
            #[cfg(any(feature = "cw-debug", test))]
            {
                if self.edge_known {
                    let elapsed = now.wrapping_sub(self.edge_at);
                    if self.mark {
                        self.mark_ticks = elapsed;
                    } else {
                        self.space_ticks = elapsed;
                    }
                }
                self.edge_at = now;
                self.edge_known = true;
            }

            self.mark = wanted;
            if !wanted {
                self.audio_tail = true;
                self.audio_until = now.wrapping_add(AUDIO_TAIL);
            }
        }

        if self.mark || reached(now, self.audio_until) {
            self.audio_tail = false;
        }
    }

    pub(crate) const fn audible(&self, now: u32) -> bool {
        self.sampled
            && now.wrapping_sub(self.last) <= SAMPLE_LIMIT
            && (self.mark
                || (self.audio_tail && !reached(now, self.audio_until)))
    }

    /// Timestamp and qualified state of the last accepted observation.
    /// Receiver phase and input-mute eligibility remain with the caller.
    pub(crate) const fn observation(&self) -> Option<(u32, bool)> {
        if self.sampled { Some((self.last, self.mark)) } else { None }
    }

    /// The receiver supplies its cached RSSI and current threshold pair.
    /// This projection performs no sampling or detection-state transition.
    #[cfg(any(feature = "cw-debug", test))]
    pub(crate) const fn probe(&self, rssi: u32, open: u32, close: u32) -> Probe {
        Probe {
            rssi, open, close,
            down: self.mark,
            mark_ticks: self.mark_ticks,
            space_ticks: self.space_ticks,
            max_gap: self.max_gap,
        }
    }
}

const fn unit(wpm: u8) -> u32 {
    assert!(wpm >= 5 && wpm <= 35);
    (1200 + wpm as u32 / 2) / wpm as u32
}

const fn pattern(elements: &[u8]) -> usize {
    assert!(!elements.is_empty() && elements.len() <= 6);
    let mut value = 1usize;
    let mut index = 0;

    while index < elements.len() {
        assert!(elements[index] == b'.' || elements[index] == b'-');
        value = (value << 1) | (elements[index] == b'-') as usize;
        index += 1;
    }

    value
}

const fn catalog() -> [u8; 128] {
    let entries: &[(u8, &[u8])] = &[
        (b'A', b".-"),     (b'B', b"-..."),   (b'C', b"-.-."),
        (b'D', b"-.."),    (b'E', b"."),      (b'F', b"..-."),
        (b'G', b"--."),    (b'H', b"...."),   (b'I', b".."),
        (b'J', b".---"),   (b'K', b"-.-"),    (b'L', b".-.."),
        (b'M', b"--"),     (b'N', b"-."),     (b'O', b"---"),
        (b'P', b".--."),   (b'Q', b"--.-"),   (b'R', b".-."),
        (b'S', b"..."),    (b'T', b"-"),      (b'U', b"..-"),
        (b'V', b"...-"),   (b'W', b".--"),    (b'X', b"-..-"),
        (b'Y', b"-.--"),   (b'Z', b"--.."),
        (b'0', b"-----"),  (b'1', b".----"),  (b'2', b"..---"),
        (b'3', b"...--"),  (b'4', b"....-"),  (b'5', b"....."),
        (b'6', b"-...."),  (b'7', b"--..."),  (b'8', b"---.."),
        (b'9', b"----."),
        (b'+', b".-.-."),  (b'=', b"-...-"),  (b'/', b"-..-."),
        (b'?', b"..--.."), (b'.', b".-.-.-"), (b',', b"--..--"),
        (b'-', b"-....-"), (b':', b"---..."), (b'@', b".--.-."),
        (b'(', b"-.--."),  (b')', b"-.--.-"), (b'\'', b".----."),
    ];
    let mut tree = [0u8; 128];
    let mut index = 0;

    while index < entries.len() {
        let (character, elements) = entries[index];
        let at = pattern(elements);
        assert!(tree[at] == 0);
        tree[at] = character;
        index += 1;
    }

    tree
}

static TREE: [u8; 128] = catalog();

/// At most six elements and one tentative character. count zero means no
/// pending symbol. down marks the final displayed element as still active.
/// The final character belongs only to Decoder's committed text history.
#[derive(Clone, Copy)]
pub(crate) struct Live {
    pub(crate) elements: [u8; 6],
    pub(crate) count: usize,
    pub(crate) character: u8,
    pub(crate) down: bool,
}

impl Live {
    const fn empty() -> Self {
        Self { elements: [b' '; 6], count: 0, character: b' ', down: false }
    }
}

pub(crate) struct Decoder {
    #[cfg(test)]
    text: [u8; HISTORY],
    #[cfg(test)]
    used: usize,
    line: [u8; DISPLAY_COLUMNS],
    line_used: usize,
    last_character: u8,
    sampled: bool,
    last: u32,
    down: bool,
    edge_at: u32,
    collecting: bool,
    pattern: usize,
    elements: u8,
    malformed: bool,
    letter_done: bool,
    word_done: bool,
}

impl Decoder {
    pub(crate) const fn new() -> Self {
        Self {
            #[cfg(test)]
            text: [b' '; HISTORY],
            #[cfg(test)]
            used: 0,
            line: [b' '; DISPLAY_COLUMNS],
            line_used: 0,
            last_character: 0,
            sampled: false,
            last: 0,
            down: false,
            edge_at: 0,
            collecting: false,
            pattern: 1,
            elements: 0,
            malformed: false,
            letter_done: false,
            word_done: false,
        }
    }

    #[cfg(test)]
    pub(crate) const fn text(&self) -> &[u8] {
        self.text.split_at(self.used).0
    }

    /// Committed cells of the current display line. Provisional elements
    /// and characters belong to live and never consume these columns.
    pub(crate) const fn display_line(&self) -> &[u8] {
        self.line.split_at(self.line_used).0
    }

    /// Project the current symbol at the caller's observation time without
    /// accepting an edge, advancing a deadline, or appending text.
    #[inline(never)]
    pub(crate) const fn live(&self, now: u32, wpm: u8) -> Live {
        let dot = unit(wpm);
        if !self.sampled || now.wrapping_sub(self.last) > SAMPLE_LIMIT {
            return Live::empty();
        }

        let mut code = self.pattern;
        let mut count = self.elements as usize;
        let mut bad = self.malformed;
        let mut active = false;

        if self.down && self.collecting {
            let elapsed = now.wrapping_sub(self.edge_at);
            if elapsed >= dot / 4 {
                if elapsed > dot * 5 || count == 6 {
                    bad = true;
                } else {
                    code = (code << 1) | (elapsed >= dot * 2) as usize;
                    count += 1;
                    active = true;
                }
            }
        }

        if count == 0 && !bad {
            return Live::empty();
        }

        let mut result = Live::empty();
        let mut index = 0;
        while index < count {
            result.elements[index] = if code & (1 << (count - 1 - index)) != 0 {
                b'-'
            } else {
                b'.'
            };
            index += 1;
        }
        result.count = count;
        result.down = active;
        result.character = if bad || TREE[code] == 0 { b'?' } else { TREE[code] };

        if bad && count == 0 {
            result.elements[0] = b'?';
            result.count = 1;
        }
        result
    }

    pub(crate) const fn clear(&mut self) {
        *self = Self::new();
    }

    /// Preserve completed text without retaining an interrupted symbol,
    /// an asserted mark, or a pending word separator.
    #[inline(never)]
    pub(crate) const fn suspend(&mut self) {
        self.sampled = false;
        self.last = 0;
        self.down = false;
        self.edge_at = 0;
        self.collecting = false;
        self.pattern = 1;
        self.elements = 0;
        self.malformed = false;
        self.letter_done = false;
        self.word_done = false;
    }

    #[inline(never)]
    const fn push(&mut self, value: u8) {
        if self.line_used == DISPLAY_COLUMNS && value != b' ' {
            self.line = [b' '; DISPLAY_COLUMNS];
            self.line_used = 0;
        }

        // Do not replace a full line with an empty word separator.
        // The next actual character starts the new line at a fixed origin.
        if self.line_used < DISPLAY_COLUMNS
            && (value != b' ' || self.line_used != 0)
        {
            self.line[self.line_used] = value;
            self.line_used += 1;
        }

        // Session separation needs the last committed byte even when the
        // fixed display line omits a trailing space at its right boundary.
        self.last_character = value;

        #[cfg(test)]
        {
            // Fixture history observes the emitted byte stream independently
            // of display-line wrapping. Firmware retains no second text ring.
            if self.used == self.text.len() {
                let mut index = 1;
                while index < self.text.len() {
                    self.text[index - 1] = self.text[index];
                    index += 1;
                }
                self.used -= 1;
            }

            self.text[self.used] = value;
            self.used += 1;
        }
    }

    const fn emit(&mut self) {
        if self.elements == 0 && !self.malformed {
            return;
        }

        let character = if self.malformed {
            b'?'
        } else {
            let value = TREE[self.pattern];
            if value == 0 { b'?' } else { value }
        };
        self.push(character);
        self.pattern = 1;
        self.elements = 0;
        self.malformed = false;
        self.letter_done = true;
        self.word_done = false;
    }

    const fn space(&mut self, elapsed: u32, dot: u32) {
        if elapsed >= dot * 3 {
            self.emit();
        }

        if elapsed >= dot * 7 && self.letter_done && !self.word_done {
            self.push(b' ');
            self.word_done = true;
        }
    }

    const fn mark(&mut self, elapsed: u32, dot: u32) {
        if elapsed < dot / 4 {
            return;
        }
        if elapsed > dot * 5 || self.elements == 6 {
            self.malformed = true;
            return;
        }

        self.pattern = (self.pattern << 1) | (elapsed >= dot * 2) as usize;
        self.elements += 1;
    }

    /// Supply continuous observations, including spaces. The first high
    /// state cannot establish where its mark began and is not collected.
    /// A stale interval drops partial timing before accepting a new baseline.
    // RX and TX use identical timing code with independently borrowed state.
    // Constant fixtures evaluate the same implementation without a runtime call.
    #[inline(never)]
    pub(crate) const fn poll(&mut self, now: u32, down: bool, wpm: u8) {
        let dot = unit(wpm);
        if self.sampled && now.wrapping_sub(self.last) > SAMPLE_LIMIT {
            self.suspend();
        }

        if !self.sampled {
            self.sampled = true;
            self.last = now;
            self.down = down;
            self.edge_at = now;
            self.collecting = false;
            return;
        }
        self.last = now;

        if down != self.down {
            let elapsed = now.wrapping_sub(self.edge_at);

            if self.down {
                if self.collecting {
                    self.mark(elapsed, dot);
                }
                self.collecting = false;
            } else {
                self.space(elapsed, dot);
                self.collecting = true;
                self.letter_done = false;
                self.word_done = false;
            }

            self.down = down;
            self.edge_at = now;
        }

        if !self.down {
            self.space(now.wrapping_sub(self.edge_at), dot);
        }
    }

    /// End one TX observation session. A live mark is incomplete and cannot
    /// be converted into a dot or dash by this operation. Repeated end calls
    /// cannot append repeated separators.
    // Cancellation and unavailable TX observations share bounded completion.
    #[inline(never)]
    pub(crate) const fn end(&mut self) {
        if self.sampled {
            if !self.down {
                self.emit();
            }
            if self.last_character != 0 && self.last_character != b' ' {
                self.push(b' ');
            }
        }
        self.suspend();
    }
}

const _: () = {
    assert!(unit(5) == 240);
    assert!(unit(15) == 80);
    assert!(unit(20) == 60);
    assert!(unit(25) == 48);
    assert!(unit(30) == 40);
    assert!(unit(35) == 34);
    assert!(TREE[pattern(b"...")] == b'S');
    assert!(TREE[pattern(b"---")] == b'O');
    assert!(TREE[pattern(b"..--..")] == b'?');
    assert!(TREE[pattern(b"------")] == 0);
};

#[cfg(test)]
mod tests {
    use super::*;

    struct Feed {
        decoder: Decoder,
        now: u32,
        wpm: u8,
    }

    impl Feed {
        fn new(wpm: u8, now: u32) -> Self {
            let mut value = Self { decoder: Decoder::new(), now, wpm };
            value.decoder.poll(now, false, wpm);
            value
        }

        fn hold(&mut self, down: bool, ticks: u32) {
            self.decoder.poll(self.now, down, self.wpm);
            for _ in 0..ticks {
                self.now = self.now.wrapping_add(1);
                self.decoder.poll(self.now, down, self.wpm);
            }
        }

        fn symbol(&mut self, elements: &[u8]) {
            let dot = unit(self.wpm);
            for element in elements {
                self.hold(true, dot * if *element == b'-' { 3 } else { 1 });
                self.hold(false, dot);
            }
            self.hold(false, dot * 2);
        }
    }

    #[test]
    fn sos_and_word_spacing_at_every_selected_speed() {
        for wpm in 5..=35 {
            let mut feed = Feed::new(wpm, 0);
            feed.symbol(b"...");
            feed.symbol(b"---");
            feed.symbol(b"...");
            assert_eq!(feed.decoder.text(), b"SOS");

            feed.hold(false, unit(wpm) * 4);
            assert_eq!(feed.decoder.text(), b"SOS ");
            feed.hold(false, unit(wpm) * 20);
            assert_eq!(feed.decoder.text(), b"SOS ");
            feed.symbol(b".");
            assert_eq!(feed.decoder.text(), b"SOS E");
        }
    }

    #[test]
    fn display_cells_do_not_move_when_the_line_fills() {
        let mut feed = Feed::new(20, 0);

        for count in 1..=DISPLAY_COLUMNS {
            feed.symbol(b".");
            assert_eq!(feed.decoder.display_line().len(), count);
            assert_eq!(feed.decoder.display_line(), &vec![b'E'; count]);
        }

        let full = [b'E'; DISPLAY_COLUMNS];
        assert_eq!(feed.decoder.display_line(), &full);

        // A word separator cannot clear a full displayed line.
        feed.hold(false, unit(feed.wpm) * 7);
        assert_eq!(feed.decoder.display_line(), &full);

        // A pending mark does not consume a committed display cell.
        feed.hold(true, unit(feed.wpm) * 3);
        assert_eq!(feed.decoder.display_line(), &full);
        feed.hold(false, unit(feed.wpm) * 3);

        assert_eq!(feed.decoder.display_line(), b"T");
        assert_eq!(feed.decoder.text().last(), Some(&b'T'));
    }

    #[test]
    fn display_lines_survive_suspension_and_clear_independently() {
        let mut rx = Feed::new(20, 0);
        let mut tx = Feed::new(20, 0);
        rx.symbol(b".-.");
        tx.symbol(b"-");

        assert_eq!(rx.decoder.display_line(), b"R");
        assert_eq!(tx.decoder.display_line(), b"T");
        rx.decoder.suspend();
        assert_eq!(rx.decoder.display_line(), b"R");

        rx.decoder.clear();
        assert!(rx.decoder.display_line().is_empty());
        assert_eq!(tx.decoder.display_line(), b"T");
    }

    #[test]
    fn provisional_symbol_changes_without_speculative_history() {
        let mut feed = Feed::new(20, 0);
        feed.hold(true, 30);
        let first = feed.decoder.live(feed.now, feed.wpm);
        assert_eq!(first.count, 1);
        assert_eq!(first.elements[0], b'.');
        assert_eq!(first.character, b'E');
        assert!(first.down);
        assert!(feed.decoder.text().is_empty());

        feed.hold(true, 30);
        feed.hold(false, 60);
        let ended = feed.decoder.live(feed.now, feed.wpm);
        assert_eq!(ended.character, b'E');
        assert!(!ended.down);

        feed.hold(true, 100);
        let second = feed.decoder.live(feed.now, feed.wpm);
        assert_eq!(&second.elements[..second.count], b"..");
        assert_eq!(second.character, b'I');
        assert!(second.down);

        feed.hold(true, 21);
        let corrected = feed.decoder.live(feed.now, feed.wpm);
        assert_eq!(&corrected.elements[..corrected.count], b".-");
        assert_eq!(corrected.character, b'A');
        assert!(feed.decoder.text().is_empty());

        feed.hold(false, 180);
        assert_eq!(feed.decoder.text(), b"A");
        assert_eq!(feed.decoder.live(feed.now, feed.wpm).count, 0);
    }

    #[test]
    fn preview_does_not_accept_initial_partial_or_stale_marks() {
        let mut decoder = Decoder::new();
        decoder.poll(0, true, 20);
        decoder.poll(20, true, 20);
        assert_eq!(decoder.live(20, 20).count, 0);

        let mut feed = Feed::new(20, 0);
        feed.hold(true, 60);
        assert_eq!(feed.decoder.live(feed.now, feed.wpm).count, 1);
        assert_eq!(feed.decoder.live(feed.now + 21, feed.wpm).count, 0);
        feed.decoder.suspend();
        assert_eq!(feed.decoder.live(feed.now, feed.wpm).count, 0);
    }

    #[test]
    fn digits_and_punctuation_have_explicit_patterns() {
        let cases: &[(u8, &[u8])] = &[
            (b'A', b".-"), (b'Z', b"--.."),
            (b'0', b"-----"), (b'1', b".----"),
            (b'2', b"..---"), (b'3', b"...--"),
            (b'4', b"....-"), (b'5', b"....."),
            (b'6', b"-...."), (b'7', b"--..."),
            (b'8', b"---.."), (b'9', b"----."),
            (b'+', b".-.-."), (b'=', b"-...-"),
            (b'/', b"-..-."), (b'?', b"..--.."),
            (b'.', b".-.-.-"), (b',', b"--..--"),
            (b'-', b"-....-"), (b':', b"---..."),
            (b'@', b".--.-."), (b'(', b"-.--."),
            (b')', b"-.--.-"), (b'\'', b".----."),
        ];

        for &(character, elements) in cases {
            let mut feed = Feed::new(20, 0);
            feed.symbol(elements);
            assert_eq!(feed.decoder.text(), &[character]);
        }
    }

    #[test]
    fn short_pulses_unknown_patterns_and_oversized_marks() {
        let mut feed = Feed::new(20, 0);
        feed.hold(true, 5);
        feed.hold(false, 180);
        assert!(feed.decoder.text().is_empty());

        feed.symbol(b"------");
        assert_eq!(feed.decoder.text(), b"?");
        feed.symbol(b".......");
        assert_eq!(feed.decoder.text(), b"??");

        feed.hold(true, 360);
        feed.hold(false, 180);
        assert_eq!(feed.decoder.text(), b"???");
    }

    #[test]
    fn initial_high_and_missing_observations_do_not_invent_marks() {
        let mut feed = Feed::new(20, 0);
        feed.decoder.suspend();
        feed.decoder.poll(feed.now, true, feed.wpm);
        feed.hold(true, 60);
        feed.hold(false, 180);
        assert!(feed.decoder.text().is_empty());

        feed.hold(true, 60);
        feed.hold(false, 60);
        feed.now += 21;
        feed.decoder.poll(feed.now, false, feed.wpm);
        feed.hold(false, 180);
        assert!(feed.decoder.text().is_empty());

        feed.symbol(b"-");
        assert_eq!(feed.decoder.text(), b"T");
        feed.decoder.suspend();
        assert_eq!(feed.decoder.text(), b"T");
    }

    #[test]
    fn session_end_flushes_completed_marks_but_not_an_active_mark() {
        let mut feed = Feed::new(5, 0);
        feed.hold(true, 240);
        feed.hold(false, 50);
        assert!(feed.decoder.text().is_empty());
        feed.decoder.end();
        assert_eq!(feed.decoder.text(), b"E ");
        feed.decoder.end();
        assert_eq!(feed.decoder.text(), b"E ");

        feed.decoder.poll(feed.now, false, feed.wpm);
        feed.hold(true, 240);
        feed.decoder.end();
        assert_eq!(feed.decoder.text(), b"E ");
        feed.decoder.clear();
        assert!(feed.decoder.text().is_empty());
    }

    #[test]
    fn history_is_bounded_and_retains_the_newest_characters() {
        let mut feed = Feed::new(25, 0);
        for _ in 0..40 {
            feed.symbol(b".");
        }
        assert_eq!(feed.decoder.text(), &[b'E'; HISTORY]);
        feed.symbol(b"-");
        assert_eq!(feed.decoder.text().len(), HISTORY);
        assert_eq!(feed.decoder.text()[HISTORY - 1], b'T');
    }

    #[test]
    fn receive_and_transmit_histories_are_independent() {
        let mut rx = Feed::new(15, 0);
        let mut tx = Feed::new(15, 0);
        rx.symbol(b".-.");
        tx.symbol(b"-");
        assert_eq!(rx.decoder.text(), b"R");
        assert_eq!(tx.decoder.text(), b"T");
        rx.decoder.clear();
        assert!(rx.decoder.text().is_empty());
        assert_eq!(tx.decoder.text(), b"T");
    }

    #[test]
    fn envelope_hysteresis_filter_and_audio_tail_are_separate() {
        let mut envelope = Envelope::new();
        assert!(envelope.observation().is_none());
        assert!(!envelope.audible(0));

        envelope.poll(0, 100, 200, 192);
        envelope.poll(5, 210, 200, 192);
        assert_eq!(envelope.observation(), Some((5, false)));
        envelope.poll(10, 210, 200, 192);
        assert_eq!(envelope.observation(), Some((10, true)));
        assert!(envelope.audible(10));

        envelope.poll(15, 195, 200, 192);
        assert_eq!(envelope.observation(), Some((15, true)));
        envelope.poll(20, 191, 200, 192);
        assert_eq!(envelope.observation(), Some((20, true)));
        envelope.poll(25, 191, 200, 192);
        assert_eq!(envelope.observation(), Some((25, false)));
        assert!(envelope.audible(39));
        assert!(!envelope.audible(40));
    }

    #[test]
    fn stale_envelope_cannot_retain_audio_or_an_asserted_mark() {
        let mut envelope = Envelope::new();
        envelope.poll(0, 220, 200, 192);
        envelope.poll(5, 220, 200, 192);
        assert!(envelope.audible(5));
        assert!(!envelope.audible(26));

        envelope.poll(26, 220, 200, 192);
        assert_eq!(envelope.observation(), Some((26, false)));
        assert!(!envelope.audible(26));
        envelope.poll(31, 220, 200, 192);
        assert!(envelope.audible(31));
        envelope.reset();
        assert!(envelope.observation().is_none());
    }

    #[test]
    fn repeated_timestamp_cannot_complete_envelope_assertion() {
        let mut envelope = Envelope::new();
        for _ in 0..100 {
            envelope.poll(100, 220, 200, 192);
        }
        assert_eq!(envelope.observation(), Some((100, false)));
        envelope.poll(105, 220, 200, 192);
        assert_eq!(envelope.observation(), Some((105, true)));
    }

    #[test]
    fn probe_reports_completed_intervals_and_sampling_loss() {
        let mut envelope = Envelope::new();
        envelope.poll(0, 100, 200, 192);
        envelope.poll(5, 220, 200, 192);
        envelope.poll(10, 220, 200, 192);
        envelope.poll(15, 100, 200, 192);
        envelope.poll(20, 100, 200, 192);
        envelope.poll(25, 100, 200, 192);
        envelope.poll(30, 220, 200, 192);
        envelope.poll(35, 220, 200, 192);

        let probe = envelope.probe(220, 200, 192);
        assert_eq!(probe.rssi, 220);
        assert_eq!(probe.open, 200);
        assert_eq!(probe.close, 192);
        assert!(probe.down);
        assert_eq!(probe.mark_ticks, 10);
        assert_eq!(probe.space_ticks, 15);
        assert_eq!(probe.max_gap, 5);

        envelope.poll(70, 220, 200, 192);
        let stale = envelope.probe(220, 200, 192);
        assert!(!stale.down);
        assert_eq!(stale.mark_ticks, 0);
        assert_eq!(stale.space_ticks, 0);
        assert_eq!(stale.max_gap, 35);

        envelope.reset();
        assert_eq!(envelope.probe(100, 200, 192).max_gap, 0);
        assert!(envelope.observation().is_none());
    }

    #[test]
    fn decoding_and_envelope_deadlines_cross_counter_wrap() {
        let mut feed = Feed::new(20, u32::MAX - 40);
        feed.symbol(b".-");
        assert_eq!(feed.decoder.text(), b"A");

        let mut envelope = Envelope::new();
        envelope.poll(u32::MAX - 2, 220, 200, 192);
        envelope.poll(2, 220, 200, 192);
        assert!(envelope.audible(2));
        envelope.poll(7, 100, 200, 192);
        envelope.poll(12, 100, 200, 192);
        assert!(envelope.audible(26));
        assert!(!envelope.audible(27));
    }
}