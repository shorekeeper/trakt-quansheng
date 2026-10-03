//! Foreground keypad scanning and independent PTT filtering.
//!
//! One column is sampled per observed tick. Columns use open-drain outputs:
//! an inactive column is released, not driven against another pressed key.
//! A complete scan includes the all-released state for the two side buttons.
//!
//! Thirty stable ticks accept an edge. Hold is reported after 500 ticks;
//! only Up and Down repeat, starting at 600 ticks with a 100-tick period.
//! Delayed repeats are coalesced into one event. Multiple observed matrix
//! keys are rejected; this is not a multi-key or ghost-key decoder.
//!
//! PTT has its own filter and never enters the matrix selection. All events
//! are diagnostic input. This module cannot enable RF transmission.



use crate::app::time::reached;
use crate::mcu;

const COLS: u32 = 0x78;

#[repr(u32)]
#[derive(Clone, Copy)]
pub(crate) enum Key {
    None,
    Menu,
    Up,
    Down,
    Exit,
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Star,
    Zero,
    F,
    Side1,
    Side2,
    Ptt,
}

impl Key {
    pub(crate) const fn name(self) -> &'static [u8] {
        match self {
            Self::None => b"NONE",
            Self::Menu => b"MENU",
            Self::Up => b"UP",
            Self::Down => b"DOWN",
            Self::Exit => b"EXIT",
            Self::One => b"1",
            Self::Two => b"2",
            Self::Three => b"3",
            Self::Four => b"4",
            Self::Five => b"5",
            Self::Six => b"6",
            Self::Seven => b"7",
            Self::Eight => b"8",
            Self::Nine => b"9",
            Self::Star => b"STAR",
            Self::Zero => b"0",
            Self::F => b"F",
            Self::Side1 => b"SIDE1",
            Self::Side2 => b"SIDE2",
            Self::Ptt => b"PTT",
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub(crate) enum Edge {
    Down = 1,
    Up,
    Hold,
    Repeat,
}

impl Edge {
    pub(crate) const fn name(self) -> &'static [u8] {
        match self {
            Self::Down => b"DOWN",
            Self::Up => b"UP",
            Self::Hold => b"HOLD",
            Self::Repeat => b"REPEAT",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Event {
    pub(crate) key: Key,
    pub(crate) edge: Edge,
}

struct Button {
    candidate: Key,
    stable: Key,
    changed: u32,
    hold_at: u32,
    repeat_at: u32,
    held: bool,
}

impl Button {
    const fn new() -> Self {
        Self {
            candidate: Key::None,
            stable: Key::None,
            changed: 0,
            hold_at: 0,
            repeat_at: 0,
            held: false,
        }
    }

    const fn sample(&mut self, raw: Key, now: u32) -> [Option<Event>; 2] {
        let mut output = [None; 2];

        if raw as u32 != self.candidate as u32 {
            self.candidate = raw;
            self.changed = now;
        }

        if self.candidate as u32 != self.stable as u32 {
            if !reached(now, self.changed.wrapping_add(30)) {
                return output;
            }

            let mut index = 0;
            if !matches!(self.stable, Key::None) {
                output[index] = Some(Event { key: self.stable, edge: Edge::Up });
                index += 1;
            }

            self.stable = self.candidate;
            self.held = false;
            self.hold_at = now.wrapping_add(500);
            self.repeat_at = now.wrapping_add(600);

            if !matches!(self.stable, Key::None) {
                output[index] = Some(Event { key: self.stable, edge: Edge::Down });
            }
            return output;
        }

        if matches!(self.stable, Key::None) {
            return output;
        }

        if !self.held && reached(now, self.hold_at) {
            self.held = true;
            output[0] = Some(Event { key: self.stable, edge: Edge::Hold });
        } else if self.held
            && matches!(self.stable, Key::Up | Key::Down)
            && reached(now, self.repeat_at)
        {
            let count = now.wrapping_sub(self.repeat_at) / 100 + 1;
            self.repeat_at = self.repeat_at.wrapping_add(count.wrapping_mul(100));
            output[0] = Some(Event { key: self.stable, edge: Edge::Repeat });
        }

        output
    }
}

/// Each nibble contains active-low rows normalized to row order:
/// bit zero is PB15, bit three is PB12. Nibble zero has no selected column.
const fn decode(rows: [u8; 5]) -> Option<Key> {
    let matrix = [
        [Key::Menu, Key::One, Key::Four, Key::Seven],
        [Key::Up, Key::Two, Key::Five, Key::Eight],
        [Key::Down, Key::Three, Key::Six, Key::Nine],
        [Key::Exit, Key::Star, Key::Zero, Key::F],
    ];

    let baseline = rows[0];
    if baseline & 12 != 0 {
        return None;
    }

    let mut selected = Key::None;
    let mut count = 0;

    if baseline & 1 != 0 {
        selected = Key::Side1;
        count += 1;
    }
    if baseline & 2 != 0 {
        selected = Key::Side2;
        count += 1;
    }

    let mut column = 0;
    while column < 4 {
        let active = rows[column + 1] & !baseline;
        let mut row = 0;
        while row < 4 {
            if active & (1 << row) != 0 {
                selected = matrix[column][row];
                count += 1;
            }
            row += 1;
        }
        column += 1;
    }

    if count > 1 { None } else { Some(selected) }
}

const fn packed(rows: [u8; 5]) -> u32 {
    let mut value = 0;
    let mut index = 0;
    while index < 5 {
        value |= (rows[index] as u32) << (index * 4);
        index += 1;
    }
    value
}

struct Scanner {
    rows: [u8; 5],
    column: usize,
    next: u32,
    last: u32,
    scans: u32,
    resets: u32,
}

impl Scanner {
    fn select(column: usize) {
        unsafe {
            mcu::write(mcu::GPIO_B + 0x18, COLS);
            if column != 0 {
                mcu::write(mcu::GPIO_B + 0x18, 1 << (23 - column));
            }
        }
    }

    fn step(&mut self, now: u32) -> Option<[u8; 5]> {
        if !reached(now, self.next) {
            return None;
        }

        if now.wrapping_sub(self.last) > 10 {
            Self::select(0);
            self.column = 0;
            self.rows = [0; 5];
            self.next = now.wrapping_add(1);
            self.last = now;
            self.resets = self.resets.saturating_add(1);
            return None;
        }

        self.last = now;
        self.next = now.wrapping_add(1);

        let low = unsafe { (!mcu::read(mcu::GPIO_B + 0x10) >> 12) & 15 };
        self.rows[self.column] = (low.reverse_bits() >> 28) as u8;

        if self.column == 4 {
            self.column = 0;
            Self::select(0);
            self.scans = self.scans.saturating_add(1);
            Some(self.rows)
        } else {
            self.column += 1;
            Self::select(self.column);
            None
        }
    }
}

#[repr(C)]
struct InputState {
    signature: u32,
    tick: u32,
    matrix: u32,
    stable_key: u32,
    ptt: u32,
    events: u32,
    ambiguous: u32,
    presses: u32,
    releases: u32,
    holds: u32,
    repeats: u32,
    last_key: u32,
    last_edge: u32,
    scans: u32,
    scan_resets: u32,
    reserved: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_INPUT: InputState = unsafe { core::mem::zeroed() };

pub(crate) struct Input {
    scanner: Scanner,
    keypad: Button,
    ptt: Button,
    matrix: u32,
    counts: [u32; 4],
    last_event: Option<Event>,
    pub(crate) ambiguous: u32,
}

impl Input {
    /// Owns PB3..PB6, PB12..PB15, and PB10.
    ///
    /// no other execution context may configure or scan these pins.
    /// GPIO clocks must be available and the keypad must not share an active bus.
    pub(crate) unsafe fn initialize() -> Self {
        unsafe {
            mcu::enable_gpio();
            mcu::write(mcu::GPIO_B + 0x18, COLS);
            mcu::modify(mcu::GPIO_B + 4, COLS, COLS);
            mcu::modify(mcu::GPIO_B + 8, 0x3FC0, 0);
            mcu::modify(mcu::GPIO_B + 12, 0x3FC0, 0x1540);
            mcu::modify(mcu::GPIO_B, 0x3FC0, 0x1540);
            for pin in 12..16 {
                mcu::input_pull_up(mcu::GPIO_B, pin);
            }
            mcu::input_pull_up(mcu::GPIO_B, 10);
        }

        Self {
            scanner: Scanner {
                rows: [0; 5],
                column: 0,
                next: 1,
                last: 0,
                scans: 0,
                resets: 0,
            },
            keypad: Button::new(),
            ptt: Button::new(),
            matrix: 0,
            counts: [0; 4],
            last_event: None,
            ambiguous: 0,
        }
    }

    pub(crate) fn stable(&self) -> Key {
        self.keypad.stable
    }

    pub(crate) fn ptt(&self) -> bool {
        matches!(self.ptt.stable, Key::Ptt)
    }

    /// TX admission uses the debounced input; shutdown also consults this
    /// raw level so it does not wait for release debounce.
    pub(crate) fn raw_ptt(&self) -> bool {
        (unsafe { mcu::read(mcu::GPIO_B + 0x10) }) & (1u32 << 10) == 0
    }

    /// Returns at most two transitions per filter. PTT and matrix input
    /// remain independent even when both change in the same pass.
    pub(crate) fn poll(&mut self, now: u32) -> [Option<Event>; 4] {
        let pressed = unsafe { mcu::read(mcu::GPIO_B + 0x10) } & (1 << 10) == 0;
        let ptt = if pressed { Key::Ptt } else { Key::None };
        let mut output = [None; 4];
        let mut count = 0;

        for event in self.ptt.sample(ptt, now).into_iter().flatten() {
            output[count] = Some(event);
            count += 1;
        }

        if let Some(rows) = self.scanner.step(now) {
            self.matrix = packed(rows);
            let raw = match decode(rows) {
                Some(key) => key,
                None => {
                    self.ambiguous = self.ambiguous.saturating_add(1);
                    Key::None
                }
            };
            for event in self.keypad.sample(raw, now).into_iter().flatten() {
                output[count] = Some(event);
                count += 1;
            }
        }

        for event in output.into_iter().flatten() {
            let index = event.edge as usize - 1;
            self.counts[index] = self.counts[index].saturating_add(1);
            self.last_event = Some(event);
        }
        output
    }

    pub(crate) fn publish(&self, now: u32) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let mut total = 0u32;
        for count in self.counts {
            total = total.saturating_add(count);
        }
        let state = InputState {
            signature: 0,
            tick: now,
            matrix: self.matrix,
            stable_key: self.stable() as u32,
            ptt: self.ptt() as u32,
            events: total,
            ambiguous: self.ambiguous,
            presses: self.counts[0],
            releases: self.counts[1],
            holds: self.counts[2],
            repeats: self.counts[3],
            last_key: self.last_event.map_or(0, |event| event.key as u32),
            last_edge: self.last_event.map_or(0, |event| event.edge as u32),
            scans: self.scanner.scans,
            scan_resets: self.scanner.resets,
            reserved: 0,
        };
        crate::observation::publish!(TRAKT_INPUT, state, 0x5449_4354);
    }
}

const fn expect(event: Option<Event>, key: Key, edge: Edge) {
    match event {
        Some(event) => {
            assert!(event.key as u32 == key as u32);
            assert!(event.edge as u32 == edge as u32);
        }
        None => panic!("missing input event"),
    }
}

const _: () = {
    assert!(core::mem::size_of::<InputState>() == 64);
    assert!(matches!(decode([0; 5]), Some(Key::None)));
    assert!(matches!(decode([0, 1, 0, 0, 0]), Some(Key::Menu)));
    assert!(matches!(decode([0, 0, 0, 0, 8]), Some(Key::F)));
    assert!(matches!(decode([1; 5]), Some(Key::Side1)));
    assert!(matches!(decode([2; 5]), Some(Key::Side2)));
    assert!(decode([3; 5]).is_none());
    assert!(decode([0, 3, 0, 0, 0]).is_none());
    assert!(decode([0, 1, 1, 0, 0]).is_none());
    assert!(decode([4; 5]).is_none());
    assert!(packed([1, 2, 3, 4, 5]) == 0x54321);

    let mut button = Button::new();
    assert!(button.sample(Key::Up, 0)[0].is_none());
    assert!(button.sample(Key::Up, 29)[0].is_none());
    expect(button.sample(Key::Up, 30)[0], Key::Up, Edge::Down);
    assert!(button.sample(Key::Up, 529)[0].is_none());
    expect(button.sample(Key::Up, 530)[0], Key::Up, Edge::Hold);
    expect(button.sample(Key::Up, 630)[0], Key::Up, Edge::Repeat);
    expect(button.sample(Key::Up, 990)[0], Key::Up, Edge::Repeat);
    assert!(button.sample(Key::Up, 1_029)[0].is_none());
    expect(button.sample(Key::Up, 1_030)[0], Key::Up, Edge::Repeat);

    assert!(button.sample(Key::Menu, 1_040)[0].is_none());
    let changed = button.sample(Key::Menu, 1_070);
    expect(changed[0], Key::Up, Edge::Up);
    expect(changed[1], Key::Menu, Edge::Down);
    assert!(button.sample(Key::None, 1_080)[0].is_none());
    expect(button.sample(Key::None, 1_110)[0], Key::Menu, Edge::Up);

    let mut bounce = Button::new();
    assert!(bounce.sample(Key::One, 0)[0].is_none());
    assert!(bounce.sample(Key::None, 20)[0].is_none());
    assert!(bounce.sample(Key::One, 25)[0].is_none());
    assert!(bounce.sample(Key::One, 54)[0].is_none());
    expect(bounce.sample(Key::One, 55)[0], Key::One, Edge::Down);

    let mut ptt = Button::new();
    assert!(ptt.sample(Key::Ptt, 0xFFFF_FFF0)[0].is_none());
    expect(ptt.sample(Key::Ptt, 14)[0], Key::Ptt, Edge::Down);
    expect(ptt.sample(Key::Ptt, 514)[0], Key::Ptt, Edge::Hold);
    assert!(ptt.sample(Key::Ptt, 1_000)[0].is_none());
};