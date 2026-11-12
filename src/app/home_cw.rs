//! CW operating screen with fixed frequency, text, and live-symbol regions.
//!
//! The header contains VFO identity, selected speed, input/session readiness,
//! and the calibrated battery estimate. Frequency uses the shared native
//! large-digit renderer at y=10, including its ten-hertz suffix. The right
//! column identifies the key mode, reversal, contact source, and raw contact
//! levels. Readiness is an application observation, not an RF measurement.
//!
//! Completed RX and TX text occupy separate eighteen-cell Normal rows at
//! y=27 and y=37. Characters do not shift; display-line replacement belongs
//! to Decoder. Live elements and tentative characters occupy a separate row
//! below both histories. Each direction has six fixed four-pixel element
//! slots and its own inverse tentative-character cell. The active element
//! has an underline. No provisional character enters the completed rows.
//!
//! The bottom row normally shows pitch in Hz, TX hang in observed ticks,
//! open/gated RX audio, and the selected PA profile label. It is replaced
//! by operational warnings or a complete hexadecimal error code when needed.
//! TEST and TRAIN remain explicit in the header; neither denotes RF admission.
//! The PA profile label is not measured transmitter power.
//!
//! Header, frequency, completed text, live symbols, and footer have disjoint
//! bounds. There are no enclosing panels, scrolling labels, blink timers,
//! extra histories, or framebuffer allocations. Rendering changes no keyer,
//! receiver, transmitter, release qualification, or persistent setting.

use super::{
    home::CwView,
    operation::{Bank, Operation},
};
use crate::cw_decode::{Live, DISPLAY_COLUMNS};
use crate::drivers::bk4829::{
    receive::View as RxView,
    transmit::View as TxView,
};
use crate::ui::{font::Size, widgets, Frame};

const TEXT_X: i16 = 12;
const RX_Y: i16 = 27;
const TX_Y: i16 = 37;
const LIVE_Y: i16 = 48;
const ELEMENTS: usize = 6;

/// Compact names distinguish the two straight contacts without changing
/// their mapping. Source and reversal have separate fixed display cells.
fn key_name(state: CwView<'_>) -> &'static [u8] {
    if state.key == b"IAMBIC A" {
        b"IAMB A"
    } else if state.key == b"IAMBIC B" {
        b"IAMB B"
    } else if state.key == b"STRAIGHT PTT" || state.key == b"STRAIGHT TIP" {
        b"KEY 1"
    } else {
        b"KEY 2"
    }
}

/// Six element slots remain stationary as the tentative character changes.
/// Inversion covers only the character's own opaque Normal cell.
fn live_symbol(frame: &mut Frame, x: i16, label: &[u8], live: Live) {
    frame.text_size(x, LIVE_Y, label, Size::Small);

    for index in 0..live.count.min(ELEMENTS) {
        let left = x + 10 + index as i16 * 4;
        match live.elements[index] {
            b'.' => frame.fill(left + 1, LIVE_Y + 2, 2, 2, true),
            b'-' => frame.fill(left, LIVE_Y + 2, 3, 2, true),
            _ => frame.text_size(left, LIVE_Y, b"?", Size::Small),
        }

        if live.down && index + 1 == live.count {
            frame.fill(left, LIVE_Y + 6, 3, 1, true);
        }
    }

    if live.count != 0 {
        frame.text_size(x + 38, LIVE_Y - 1, &[live.character], Size::Normal);
        frame.invert(x + 38, LIVE_Y - 1, 6, 8);
    }
}

fn decoded(frame: &mut Frame, label: &[u8], y: i16, text: &[u8]) {
    frame.text_size(2, y, label, Size::Normal);
    frame.text_size(
        TEXT_X, y, &text[..text.len().min(DISPLAY_COLUMNS)], Size::Normal,
    );
}

fn error(frame: &mut Frame, label: &[u8], code: u32) {
    frame.text_size(2, 58, label, Size::Small);
    widgets::hex(frame, 30, 58, code, Size::Small);
}

/// Readiness does not follow individual mark/space edges. Those belong to
/// the live-symbol row. Source test and local practice remain distinct.
fn readiness(
    state: CwView<'_>,
    receiver_enabled: bool,
    transmitter: TxView,
    receiver: RxView,
) -> &'static [u8] {
    if transmitter.error != 0 {
        b"TX ERR"
    } else if receiver.error != 0 {
        b"RX ERR"
    } else if state.testing {
        b"TEST"
    } else if !receiver_enabled {
        b"RX OFF"
    } else if !state.qualified {
        b"REL KEY"
    } else if !state.qsk {
        b"TRAIN"
    } else if !transmitter.ham {
        b"HAM OFF"
    } else if transmitter.active {
        b"SESSION"
    } else {
        b"READY"
    }
}

#[inline(never)]
pub(super) fn render(
    frame: &mut Frame,
    operation: &Operation,
    bank: &Bank,
    receiver: RxView,
    transmitter: TxView,
    dirty: bool,
    state: CwView<'_>,
    voltage: Option<u32>,
    vox_enabled: bool,
) {
    frame.clear();
    let request = operation.request(bank);

    let identity = [b'A' + operation.active as u8, b' ', b'C', b'W'];
    frame.text_size(2, 1, &identity, Size::Small);
    widgets::number(frame, 26, 1, state.wpm as u32, Size::Small);
    frame.text_size(36, 1, b"WPM", Size::Small);
    frame.text_size(
        54, 1, readiness(state, request.enabled, transmitter, receiver),
        Size::Small,
    );
    // Saved VOX has no keying role in CW; readiness still belongs to Lab.
    if vox_enabled {
        frame.text_size(86, 1, b"VOX", Size::Small);
    }
    if dirty {
        frame.text_size(102, 1, b"*", Size::Small);
    }
    widgets::battery_at(frame, 110, voltage);

    widgets::frequency(frame, 2, 10, request.frequency, Size::Large);
    frame.text_size(90, 10, key_name(state), Size::Small);
    if state.reverse {
        frame.text_size(120, 10, b"R", Size::Small);
    }
    frame.text_size(
        90, 18,
        if state.usb { b"USB" } else { state.input },
        Size::Small,
    );
    let contacts = match state.contacts {
        Some((first, second)) => [b'0' + first as u8, b'0' + second as u8],
        None => [b'?', b'?'],
    };
    frame.text_size(116, 18, &contacts, Size::Small);

    frame.fill(2, 25, 124, 1, true);
    decoded(frame, b"R", RX_Y, state.rx_line);
    decoded(frame, b"T", TX_Y, state.tx_line);
    frame.fill(2, 45, 124, 1, true);

    live_symbol(frame, 2, b"R", state.rx_live);
    live_symbol(frame, 66, b"T", state.tx_live);

    if transmitter.error != 0 {
        error(frame, b"TX ERR", transmitter.error);
    } else if receiver.error != 0 {
        error(frame, b"RX ERR", receiver.error);
    } else if !request.enabled {
        frame.text_size(2, 58, b"RX OFF / SIDE2 ENABLE", Size::Small);
    } else if !state.qualified && !state.testing {
        frame.text_size(2, 58, b"RELEASE BOTH KEYS", Size::Small);
    } else if !transmitter.ham && state.qsk && !state.testing {
        frame.text_size(2, 58, b"HAM OFF / CHECK AUTHORIZATION", Size::Small);
    } else {
        widgets::number(frame, 2, 58, state.pitch as u32, Size::Small);
        frame.text_size(18, 58, b"HZ", Size::Small);
        let rx_wait = crate::cw_controls::breakin_ticks(state.breakin);
        widgets::number(
            frame, 34, 58, rx_wait.unwrap_or(state.hang as u32), Size::Small,
        );
        frame.text_size(
            50, 58, if rx_wait.is_some() { b"RX" } else { b"MS" }, Size::Small,
        );
        frame.text_size(
            66, 58, if state.gated { b"GAT" } else { b"OPN" },
            Size::Small,
        );
        frame.text_size(
            90, 58, super::tx_policy::power_name(transmitter.power),
            Size::Small,
        );
    }
}

const _: () = {
    assert!(DISPLAY_COLUMNS == 18);
    assert!(TEXT_X as usize + DISPLAY_COLUMNS * 6 <= 126);
    assert!(RX_Y + 8 <= TX_Y);
    assert!(TX_Y + 8 <= 45);
    assert!(LIVE_Y - 1 > 45);
    assert!(LIVE_Y + 7 <= 58);
    assert!(10 + ELEMENTS * 4 <= 38);
    assert!(66 + 38 + 6 <= 128);
    assert!(90 + 6 * 4 <= 116);
    assert!(116 + 2 * 4 <= 128);
};