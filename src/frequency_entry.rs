//! Numeric RX-frequency fields with explicit six- or seven-digit length.
//!
//! Values are integer kHz until complete conversion to Hz. The length is an
//! editor choice, not inferred from a prefix: 100000 means 100 MHz in a
//! six-digit field and is incomplete in a seven-digit field. Editors expose
//! the length through the preview and permit F on an empty field to change it.
//!
//! hertz checks completion and arithmetic overflow before multiplication.
//! Range admission belongs to the caller. Even seven decimal digits that
//! exceed u32 Hz return None rather than trapping or wrapping.
//!
//! Preview uses three or four MHz positions followed by three kHz positions.
//! Six-digit previews have one trailing space in the fixed eight-byte result.
//! All formatting is ASCII and allocates no heap storage.

pub(crate) const fn count(wide: bool) -> usize {
    if wide { 7 } else { 6 }
}

pub(crate) const fn required(wide: bool) -> &'static [u8] {
    if wide {
        b"REFUSE: SEVEN DIGITS REQUIRED"
    } else {
        b"REFUSE: SIX DIGITS REQUIRED"
    }
}

pub(crate) const fn hint(wide: bool) -> &'static [u8] {
    if wide {
        b"SEVEN KHZ DIGITS / AUTO ACCEPT"
    } else {
        b"SIX KHZ DIGITS / AUTO ACCEPT"
    }
}

pub(crate) const fn hertz(value: u32, digits: usize, wide: bool) -> Option<u32> {
    if digits != count(wide) {
        return None;
    }
    value.checked_mul(1000)
}

pub(crate) const fn preview(
    mut value: u32, digits: usize, wide: bool,
) -> [u8; 8] {
    assert!(digits <= count(wide));

    let whole = if wide { 4 } else { 3 };
    let mut text = [b'-'; 8];
    text[whole] = b'.';
    if !wide {
        text[7] = b' ';
    }

    let mut index = digits;
    while index > 0 {
        index -= 1;
        let position = if index < whole { index } else { index + 1 };
        text[position] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    text
}

const _: () = {
    assert!(count(false) == 6 && count(true) == 7);
    assert!(matches!(hertz(433500, 6, false), Some(433_500_000)));
    assert!(hertz(100000, 6, true).is_none());
    assert!(matches!(hertz(1000000, 7, true), Some(1_000_000_000)));
    assert!(matches!(hertz(1300000, 7, true), Some(1_300_000_000)));
    assert!(hertz(9999999, 7, true).is_none());
    assert!(hertz(u32::MAX, 7, true).is_none());

    let low = preview(433500, 6, false);
    let high = preview(1300000, 7, true);
    let expected_low = *b"433.500 ";
    let expected_high = *b"1300.000";
    let mut index = 0;
    while index < 8 {
        assert!(low[index] == expected_low[index]);
        assert!(high[index] == expected_high[index]);
        index += 1;
    }

    let partial = preview(100000, 6, true);
    let expected = *b"1000.00-";
    index = 0;
    while index < 8 {
        assert!(partial[index] == expected[index]);
        index += 1;
    }
};