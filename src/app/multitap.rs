//! Traditional multi-tap ASCII input without text-buffer ownership.
//!
//! Repeated presses of one digit within 800 observed ticks select the next
//! character of that key. Another digit or an expired interval requests the
//! next text position. The first press after clear uses the current position.
//! The caller owns cursor bounds, text mutation, deletion, and RF admission.
//!
//! Characters are uppercase because the display font folds lowercase.
//! Digits are included in each key cycle. Zero selects space or zero; One
//! provides punctuation. No dictionary, allocation, or periodic task exists.
//! clear terminates the current cycle without changing committed text.

pub(super) struct Input {
    digit: u8,
    selection: usize,
    last: u32,
    active: bool,
}

const fn alphabet(digit: u8) -> &'static [u8] {
    match digit {
        0 => b" 0",
        1 => b".,?!-/1",
        2 => b"ABC2",
        3 => b"DEF3",
        4 => b"GHI4",
        5 => b"JKL5",
        6 => b"MNO6",
        7 => b"PQRS7",
        8 => b"TUV8",
        9 => b"WXYZ9",
        _ => b"",
    }
}

impl Input {
    pub(super) const fn new() -> Self {
        Self { digit: 0, selection: 0, last: 0, active: false }
    }

    pub(super) const fn clear(&mut self) {
        self.active = false;
    }

    /// Returns (advance cursor, selected byte). Invalid digits change no
    /// state. A selected byte is immediately part of the caller's draft;
    /// no later timeout callback is required to commit it.
    pub(super) const fn press(&mut self, digit: u8, now: u32) -> Option<(bool, u8)> {
        let characters = alphabet(digit);
        if characters.is_empty() {
            return None;
        }
        let repeat = self.active && self.digit == digit
            && now.wrapping_sub(self.last) < 800;
        let advance = self.active && !repeat;
        self.selection = if repeat {
            (self.selection + 1) % characters.len()
        } else {
            0
        };
        self.digit = digit;
        self.last = now;
        self.active = true;
        Some((advance, characters[self.selection]))
    }
}

const _: () = {
    let mut input = Input::new();
    assert!(matches!(input.press(2, 0), Some((false, b'A'))));
    assert!(matches!(input.press(2, 100), Some((false, b'B'))));
    assert!(matches!(input.press(2, 200), Some((false, b'C'))));
    assert!(matches!(input.press(2, 300), Some((false, b'2'))));
    assert!(matches!(input.press(2, 400), Some((false, b'A'))));
    assert!(matches!(input.press(3, 500), Some((true, b'D'))));
    assert!(matches!(input.press(3, 1299), Some((false, b'E'))));
    assert!(matches!(input.press(3, 2099), Some((true, b'D'))));
    input.clear();
    assert!(matches!(input.press(0, 2100), Some((false, b' '))));
    assert!(matches!(input.press(0, 2101), Some((false, b'0'))));
    assert!(input.press(10, 2102).is_none());

    input.clear();
    assert!(matches!(
        input.press(9, u32::MAX - 100), Some((false, b'W')),
    ));
    assert!(matches!(input.press(9, 100), Some((false, b'X'))));

    let mut digit = 0;
    while digit < 10 {
        let characters = alphabet(digit);
        let mut index = 0;
        while index < characters.len() {
            assert!(characters[index] >= 32 && characters[index] <= 126);
            index += 1;
        }
        digit += 1;
    }
};