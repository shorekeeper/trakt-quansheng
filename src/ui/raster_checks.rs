//! Compiler-evaluated comparisons against direct pixel equations.
//!
//! A patterned initial framebuffer exposes accidental clearing or retention
//! outside a requested region. Every comparison checks all 1024 bytes, not
//! just the pixels expected to change.
//!
//! Shape expectations are calculated at each visible coordinate without
//! using the production rectangle engine or its page masks. Text expectations
//! expand font pixels directly, including opaque spacing and bottom margins.
//! Font tables are shared input; clipping and destination writes are not.
//!
//! Separate constant initializers bound each evaluation. These fixtures have
//! no runtime entry, static framebuffer, peripheral access, or heap storage.

use super::{font, Frame};

const fn patterned() -> Frame {
    let mut frame = Frame::new();
    let mut index = 0;

    while index < frame.bytes.len() {
        frame.bytes[index] = (index as u8).wrapping_mul(73)
            ^ ((index / 128) as u8).wrapping_mul(29)
            ^ 0xA5;
        index += 1;
    }

    frame
}

const fn equal(actual: &Frame, expected: &Frame) {
    let mut index = 0;
    while index < actual.bytes.len() {
        assert!(actual.bytes[index] == expected.bytes[index]);
        index += 1;
    }
}

/// action zero clears, one sets, two inverts. Border selects outline tests;
/// inversion is a filled-rectangle operation and has no outline API.
const fn shape(
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    action: u8,
    border: bool,
) {
    assert!(action < 3 && !(border && action == 2));

    let mut actual = patterned();
    let mut expected = Frame { bytes: actual.bytes };

    if action == 2 {
        actual.invert(x, y, width, height);
    } else if border {
        actual.outline(x, y, width, height, action == 1);
    } else {
        actual.fill(x, y, width, height, action == 1);
    }

    let left = x as i32;
    let top = y as i32;
    let right = left + width as i32;
    let bottom = top + height as i32;

    let mut row = 0i32;
    while row < 64 {
        let mut column = 0i32;
        while column < 128 {
            let inside = column >= left && column < right
                && row >= top && row < bottom;
            let edge = column == left || column == right - 1
                || row == top || row == bottom - 1;

            if inside && (!border || edge) {
                let on = if action == 2 {
                    let at = row as usize / 8 * 128 + column as usize;
                    expected.bytes[at] & (1 << (row as u32 & 7)) == 0
                } else {
                    action == 1
                };
                expected.pixel(column, row, on);
            }
            column += 1;
        }
        row += 1;
    }

    equal(&actual, &expected);
}

const fn rectangle(x: i16, y: i16, width: u16, height: u16) {
    shape(x, y, width, height, 0, false);
    shape(x, y, width, height, 1, false);
    shape(x, y, width, height, 2, false);
}

const fn outline(x: i16, y: i16, width: u16, height: u16) {
    shape(x, y, width, height, 0, true);
    shape(x, y, width, height, 1, true);
}

/// Independent scalar mapping from cell pixels to font pixels. The input
/// traversal limit is part of the renderer contract even for negative x.
const fn text(x: i16, y: i16, value: &[u8], size: font::Size) {
    let mut actual = patterned();
    let mut expected = Frame { bytes: actual.bytes };

    actual.text_size(x, y, value, size);

    let (width, height) = size.cell();
    let (columns, rows, scale) = size.source();
    assert!(width == (columns + 1) * scale);
    assert!(height == (rows + 1) * scale);

    let limit = 128 / width + 1;
    let count = if value.len() < limit { value.len() } else { limit };
    let mut index = 0;

    while index < count {
        let left = x as i32 + (index * width) as i32;
        if left >= 128 {
            break;
        }

        let glyph = font::glyph(value[index], size);
        let mut row = 0;
        while row < height {
            let source_row = row / scale;
            let mut column = 0;
            while column < width {
                let source_column = column / scale;
                let on = source_row < rows && source_column < columns
                    && glyph[source_row] & (1 << (columns - 1 - source_column)) != 0;

                expected.pixel(left + column as i32, y as i32 + row as i32, on);
                column += 1;
            }
            row += 1;
        }
        index += 1;
    }

    equal(&actual, &expected);
}

const _: () = rectangle(0, 0, 128, 64);
const _: () = rectangle(3, 1, 9, 5);
const _: () = rectangle(3, 7, 9, 2);
const _: () = rectangle(4, 8, 9, 8);
const _: () = rectangle(5, 15, 11, 18);
const _: () = rectangle(-2, -3, 9, 11);
const _: () = rectangle(121, 59, u16::MAX, u16::MAX);
const _: () = rectangle(i16::MIN, i16::MIN, u16::MAX, u16::MAX);
const _: () = rectangle(127, 63, 1, 1);
const _: () = rectangle(0, 0, 0, 64);
const _: () = rectangle(64, 32, u16::MAX, 0);
const _: () = rectangle(128, 0, 1, 64);
const _: () = rectangle(0, 64, 128, 1);
const _: () = rectangle(i16::MAX, i16::MAX, u16::MAX, u16::MAX);
const _: () = rectangle(i16::MIN, 0, 1, u16::MAX);

const _: () = outline(0, 0, 128, 64);
const _: () = outline(-2, -3, 9, 11);
const _: () = outline(127, 63, 1, 1);
const _: () = outline(127, 0, 1, 64);
const _: () = outline(0, 63, 128, 1);
const _: () = outline(i16::MIN, i16::MIN, u16::MAX, u16::MAX);
const _: () = outline(0, 0, 0, 64);
const _: () = outline(0, 0, 128, 0);

const _: () = text(0, 0, b"A0a?/+-[]:.", font::Size::Small);
const _: () = text(0, 0, b"A0a?/+-[]:.", font::Size::Normal);
const _: () = text(0, 0, b"A0a?/+-[]:.", font::Size::Large);

const _: () = text(3, 7, b"AZ09 gq", font::Size::Small);
const _: () = text(3, 7, b"AZ09 gq", font::Size::Normal);
const _: () = text(3, 7, b"AZ09 gq", font::Size::Large);

const _: () = text(-3, -1, b"Ab 7", font::Size::Small);
const _: () = text(-5, -7, b"Ab 7", font::Size::Normal);
const _: () = text(-11, -15, b"Ab 7", font::Size::Large);

const _: () = text(125, 61, b"W 0", font::Size::Small);
const _: () = text(123, 57, b"W 0", font::Size::Normal);
const _: () = text(117, 49, b"W 0", font::Size::Large);

const _: () = text(0, 63, b"   ", font::Size::Small);
const _: () = text(0, 63, b"   ", font::Size::Normal);
const _: () = text(0, 63, b"   ", font::Size::Large);

const _: () = text(0, -6, b"A", font::Size::Small);
const _: () = text(0, -8, b"A", font::Size::Normal);
const _: () = text(0, -16, b"A", font::Size::Large);

const _: () = text(0, 8, b"\x00\x7F\xFF ", font::Size::Small);
const _: () = text(0, 8, b"\x00\x7F\xFF ", font::Size::Normal);
const _: () = text(0, 8, b"\x00\x7F\xFF ", font::Size::Large);

const _: () = text(0, 0, b"", font::Size::Large);
const _: () = text(128, 0, b"A", font::Size::Normal);
const _: () = text(0, 64, b"A", font::Size::Small);
const _: () = text(i16::MIN, 0, b"A", font::Size::Large);
const _: () = text(0, i16::MIN, b"A", font::Size::Large);
const _: () = text(i16::MAX, i16::MAX, b"A", font::Size::Large);

// Negative origins do not permit unbounded traversal of long input.
const _: () = text(
    -100, 3,
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz",
    font::Size::Small,
);
const _: () = text(
    -100, 3,
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz",
    font::Size::Normal,
);
const _: () = text(
    -100, 3,
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz",
    font::Size::Large,
);

// Ordinary inversion is involutive, including partial page masks.
const _: () = {
    let mut actual = patterned();
    let expected = Frame { bytes: actual.bytes };
    actual.invert(-7, 3, 133, 57);
    actual.invert(-7, 3, 133, 57);
    equal(&actual, &expected);
};