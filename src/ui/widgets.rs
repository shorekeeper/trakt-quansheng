//! Small drawing primitives shared by menus and radio screens.
//!
//! Meter extents are integer fractions with explicit saturation. Their scale
//! is supplied by the caller; the RSSI widget is not a calibrated S-meter.
//! Scrollbar geometry describes a visible window, not a pointer position.
//! None of these functions allocates storage or retains a framebuffer borrow.

use super::{font::Size, Frame};

pub(crate) const fn extent(value: u32, maximum: u32, width: u32) -> u32 {
    if maximum == 0 || width > 128 || maximum > u32::MAX / 128 {
        return 0;
    }
    let value = if value > maximum { maximum } else { value };
    value * width / maximum
}

/// Ten-hertz display precision. Frequencies below 1000 MHz use three MHz
/// digits; larger values use four. No significant MHz digit is discarded.
/// The returned start index omits only the unused leading zero cell.
const fn frequency_text(hertz: u32) -> ([u8; 10], usize) {
    let mhz = hertz / 1_000_000;
    let mut fraction = hertz % 1_000_000 / 10;
    let mut text = [
        b'0' + (mhz / 1000) as u8,
        b'0' + (mhz / 100 % 10) as u8,
        b'0' + (mhz / 10 % 10) as u8,
        b'0' + (mhz % 10) as u8,
        b'.', b'0', b'0', b'0', b'0', b'0',
    ];
    let mut index = 10;
    while index > 5 {
        index -= 1;
        text[index] = b'0' + (fraction % 10) as u8;
        fraction /= 10;
    }
    (text, if mhz < 1000 { 1 } else { 0 })
}

/// Runtime formatting remains behind one call boundary for all screens.
/// Coordinates and glyph size remain arguments rather than separate renderers.
#[inline(never)]
pub(crate) fn frequency(frame: &mut Frame, x: i16, y: i16, hertz: u32, size: Size) {
    let (text, start) = frequency_text(hertz);
    frame.text_size(x, y, &text[start..], size);
}

/// Decimal digits without field padding. The runtime boundary favors sharing
/// the conversion loop across menus, measurements, and diagnostic screens.
#[inline(never)]
pub(crate) fn number(frame: &mut Frame, x: i16, y: i16, mut value: u32, size: Size) {
    let mut bytes = [b'0'; 10];
    let mut start = bytes.len();
    loop {
        start -= 1;
        bytes[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 { break; }
    }
    frame.text_size(x, y, &bytes[start..], size);
}

#[inline(never)]
pub(crate) fn bar(
    frame: &mut Frame,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    value: u32,
    maximum: u32,
) {
    if width < 3 || width > 128 || height < 3 { return; }
    frame.outline(x, y, width, height, true);
    let filled = extent(value, maximum, width as u32 - 2);
    frame.fill(x + 1, y + 1, filled as u16, height - 2, true);
}

pub(crate) fn signal(frame: &mut Frame, value: u32) {
    let filled = extent(value, 511, 12);
    for index in 0..12 {
        let x = 2 + index * 10;
        if index < filled {
            frame.fill(x as i16, 51, 8, 5, true);
        } else {
            frame.fill(x as i16, 55, 8, 1, true);
        }
    }
}

pub(crate) const fn thumb(total: usize, first: usize, visible: usize) -> (u16, u16) {
    if total <= visible || total == 0 {
        return (0, 39);
    }
    let height = 39 * visible / total;
    let height = if height < 3 { 3 } else { height };
    let maximum = total - visible;
    let first = if first > maximum { maximum } else { first };
    ((first * (39 - height) / maximum) as u16, height as u16)
}

/// Draws the track and checked window thumb through a shared runtime entry.
/// Geometry remains a function of the supplied list and visible window.
#[inline(never)]
pub(crate) fn scrollbar(frame: &mut Frame, total: usize, first: usize, visible: usize) {
    let (offset, height) = thumb(total, first, visible);
    frame.fill(125, 11, 1, 39, true);
    frame.fill(124, 11 + offset as i16, 3, height, true);
}

/// A four-segment voltage-range indicator, not remaining-capacity percent.
/// The displayed span is 6.30..8.40 V; unavailable data shows a question mark.
pub(crate) fn battery(frame: &mut Frame, millivolts: Option<u32>) {
    frame.outline(80, 0, 14, 8, true);
    frame.fill(94, 2, 2, 4, true);

    let millivolts = match millivolts {
        Some(value) => value,
        None => {
            frame.text_size(85, 1, b"?", Size::Small);
            return;
        }
    };

    let count = extent(millivolts.saturating_sub(6300), 2100, 4);
    for index in 0..count {
        frame.fill(82 + index as i16 * 3, 2, 2, 4, true);
    }
}

const fn voltage_text(millivolts: u32) -> Option<([u8; 6], usize)> {
    if millivolts > 99_990 {
        return None;
    }
    let value = (millivolts + 5) / 10;
    let volts = value / 100;
    let fraction = value % 100;

    if volts < 10 {
        Some(([
            b'0' + volts as u8,
            b'.',
            b'0' + (fraction / 10) as u8,
            b'0' + (fraction % 10) as u8,
            b'V',
            b' ',
        ], 5))
    } else {
        Some(([
            b'0' + (volts / 10) as u8,
            b'0' + (volts % 10) as u8,
            b'.',
            b'0' + (fraction / 10) as u8,
            b'0' + (fraction % 10) as u8,
            b'V',
        ], 6))
    }
}

#[inline(never)]
pub(crate) fn voltage(
    frame: &mut Frame,
    x: i16,
    y: i16,
    millivolts: Option<u32>,
    size: Size,
) {
    let encoded = millivolts.and_then(voltage_text);
    match encoded {
        Some((bytes, length)) => frame.text_size(x, y, &bytes[..length], size),
        None => frame.text_size(x, y, b"--.--V", size),
    }
}

#[inline(never)]
pub(crate) fn badge(frame: &mut Frame, x: i16, y: i16, label: &[u8]) {
    let cells = label.len().min(31);
    let width = cells as u16 * 4 + 2;
    frame.text_size(x + 1, y + 1, &label[..cells], Size::Small);
    frame.invert(x, y, width, 8);
}

const _: () = {
    assert!(extent(0, 511, 12) == 0);
    assert!(extent(511, 511, 12) == 12);
    assert!(extent(999, 511, 12) == 12);
    assert!(extent(2500, 5000, 124) == 62);
    assert!(extent(1, 0, 12) == 0);
    assert!(extent(u32::MAX, u32::MAX, 12) == 0);
    assert!(thumb(4, 0, 4).1 == 39);
    assert!(thumb(10, 0, 4).0 == 0);
    assert!(thumb(10, 6, 4).0 + thumb(10, 6, 4).1 == 39);
    assert!(thumb(10, 99, 4).0 == thumb(10, 6, 4).0);

    let (voltage, length) = match voltage_text(7978) {
        Some(encoded) => encoded,
        None => panic!("voltage formatting rejected"),
    };
    assert!(length == 5);
    assert!(voltage[0] == b'7' && voltage[1] == b'.');
    assert!(voltage[2] == b'9' && voltage[3] == b'8' && voltage[4] == b'V');

    let (rounded, length) = match voltage_text(9995) {
        Some(encoded) => encoded,
        None => panic!("voltage rounding rejected"),
    };
    assert!(length == 6);
    assert!(rounded[0] == b'1' && rounded[1] == b'0');
    assert!(rounded[2] == b'.' && rounded[3] == b'0' && rounded[4] == b'0');
    assert!(voltage_text(100_000).is_none());
    assert!(voltage_text(u32::MAX).is_none());
};
const _: () = {
    let (low, start) = frequency_text(433_506_250);
    assert!(start == 1);
    let expected = *b"0433.50625";
    let mut index = 0;
    while index < low.len() {
        assert!(low[index] == expected[index]);
        index += 1;
    }

    let (high, start) = frequency_text(1_300_000_000);
    assert!(start == 0);
    let expected = *b"1300.00000";
    index = 0;
    while index < high.len() {
        assert!(high[index] == expected[index]);
        index += 1;
    }
};
