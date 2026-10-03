//! Diagnostic screen composition with separate basic and extended admission.
//!
//! diagnostics retains Stack, Power, Last fault, and available UART screens.
//! They use mandatory observations or normal operational driver data rather
//! than requiring optional debugger snapshots. debug-tools additionally
//! admits input, calibration, receiver, timing, and memory-layout screens.
//!
//! Radio controls, storage status, About, and Help remain outside this module.
//! Drawing neither performs recovery tests nor configures RF. Constant
//! feature guards at Panel dispatch select the runtime call paths while the
//! complete renderer source remains type-checked with warnings denied.
//!
//! Feature selection does not remove stack checking, watchdog service,
//! failure capture, transmitter deadlines, or device-control verification.

use crate::drivers::{
    adc::Reading,
    bk4829::receive::View,
    keyboard::{Event, Input},
};
use crate::storage::View as MemoryView;
use crate::ui::{font::Size, widgets, Frame, Snapshot};

use super::operation::Bank;

pub(super) fn inputs(
    frame: &mut Frame,
    state: Snapshot,
    input: &Input,
    battery: Reading,
    last: Option<Event>,
) {
    frame.clear();
    frame.text(2, 0, b"INPUT DIAGNOSTICS");
    frame.fill(0, 8, 128, 1, true);
    frame.text(2, 10, b"KEY");
    frame.text(42, 10, last.map_or(b"NONE", |event| event.key.name()));
    frame.text(2, 18, b"EVENT");
    frame.text(42, 18, last.map_or(b"NONE", |event| event.edge.name()));
    frame.text(2, 26, b"PTT");
    frame.text(30, 26, if input.ptt() { b"HELD" } else { b"RELEASED" });
    frame.text(2, 34, b"ADC");
    match battery.average {
        Some(value) => frame.decimal(42, 34, value),
        None if battery.error == 0 => frame.text(42, 34, b"WAIT"),
        None => {
            frame.text(42, 34, b"ERR");
            frame.decimal(66, 34, battery.error);
        }
    }
    frame.text(2, 42, b"MULTI");
    frame.decimal(42, 42, input.ambiguous);
    frame.text(2, 50, b"TICK");
    frame.decimal(42, 50, state.tick);
    frame.text_size(2, 58, b"EXIT HOME / MENU SETUP", Size::Small);
}

pub(super) fn memory(frame: &mut Frame, memory: MemoryView) {
    frame.clear();
    frame.text(2, 0, b"FLASH / CALIBRATION");
    frame.fill(0, 8, 128, 1, true);
    frame.text(2, 10, b"ID");
    frame.hex(42, 10, memory.id);
    frame.text(2, 18, b"STATE");
    if memory.error != 0 {
        frame.text(42, 18, b"ERR");
        frame.decimal(66, 18, memory.error);
    } else {
        frame.text(42, 18, if memory.ready { b"STABLE" } else { b"WAIT" });
    }
    if let Some(calibration) = memory.calibration {
        frame.text(2, 26, b"CRC");
        frame.hex(42, 26, calibration.crc32);
        frame.text(2, 34, b"B0");
        frame.decimal(42, 34, calibration.battery[0] as u32);
        frame.text(2, 42, b"B1");
        frame.decimal(42, 42, calibration.battery[1] as u32);
        frame.text(2, 50, b"XTAL");
        frame.signed(42, 50, calibration.crystal as i32);
    }
    frame.text_size(2, 58, b"EXIT HOME / MENU SETUP", Size::Small);
}

pub(super) fn stacks(frame: &mut Frame, state: crate::debug::StackView) {
    frame.clear();
    frame.text(2, 0, b"STACK OBSERVATIONS");
    frame.fill(0, 8, 128, 1, true);

    if !state.ready {
        frame.text(2, 18, b"WAIT");
        return;
    }

    frame.text_size(2, 11, b"MAIN", Size::Small);
    widgets::number(frame, 26, 11, state.main_used, Size::Small);
    frame.text_size(48, 11, b"/", Size::Small);
    widgets::number(frame, 54, 11, state.main_budget, Size::Small);
    frame.text_size(82, 11, b"FREE", Size::Small);
    widgets::number(frame, 104, 11, state.main_free, Size::Small);
    widgets::bar(frame, 2, 19, 124, 5, state.main_used, state.main_budget);

    frame.text_size(2, 29, b"FAULT", Size::Small);
    widgets::number(frame, 30, 29, state.fault_used, Size::Small);
    frame.text_size(52, 29, b"/", Size::Small);
    widgets::number(frame, 58, 29, state.fault_budget, Size::Small);
    widgets::bar(frame, 2, 37, 124, 5, state.fault_used, state.fault_budget);

    frame.text_size(2, 47, b"GUARDS", Size::Small);
    frame.text_size(34, 47, if state.guards == 3 { b"OK" } else { b"FAIL" }, Size::Small);
    frame.text_size(62, 47, b"GAP", Size::Small);
    widgets::number(frame, 82, 47, state.max_tick_gap, Size::Small);
    frame.text_size(2, 58, b"OBSERVED ONLY / EXIT HOME", Size::Small);
}

pub(super) fn last_fault(
    frame: &mut Frame,
    state: crate::debug::FaultView,
    test_refused: bool,
) {
    frame.clear();
    frame.text(2, 0, b"PREVIOUS FAILURE");
    frame.fill(0, 8, 128, 1, true);

    if state.valid {
        frame.text_size(2, 11, b"CODE", Size::Small);
        frame.hex(30, 10, state.reason);
        frame.text_size(2, 21, b"EXC", Size::Small);
        widgets::number(frame, 30, 21, state.exception, Size::Small);
        frame.text_size(62, 21, if state.frame { b"FRAME OK" } else { b"NO FRAME" }, Size::Small);
        frame.text_size(2, 31, b"PC", Size::Small);
        frame.hex(30, 30, state.pc);
        frame.text_size(2, 41, b"LR", Size::Small);
        frame.hex(30, 40, state.lr);
    } else {
        frame.text_size(2, 10, b"INVALID RETAINED RECORD", Size::Small);
        frame.text_size(2, 21, b"PUB", Size::Small);
        frame.hex(30, 20, state.published);
        frame.text_size(2, 31, b"CRC", Size::Small);
        frame.hex(30, 30, state.computed);
        frame.text_size(2, 41, b"RAW", Size::Small);
        frame.hex(30, 40, state.raw_reason);
    }

    frame.text_size(2, 51, b"RST", Size::Small);
    frame.hex(30, 50, state.reset_flags);
    frame.text_size(2, 58, if test_refused {
        b"TEST REFUSED: RELEASE PTT"
    } else if state.valid {
        b"CHECKSUM VERIFIED / EXIT HOME"
    } else {
        b"RAW WORDS ONLY / EXIT HOME"
    }, Size::Small);
}

pub(super) fn power(frame: &mut Frame, battery: Reading, memory: MemoryView) {
    frame.clear();
    frame.text(2, 0, b"BATTERY ESTIMATE");
    frame.fill(0, 8, 128, 1, true);

    frame.text_size(2, 12, b"AVERAGE", Size::Small);
    widgets::voltage(
        frame, 48, 11,
        super::power::millivolts(battery.average, memory.calibration),
        Size::Normal,
    );
    frame.text_size(2, 23, b"RAW NOW", Size::Small);
    widgets::voltage(
        frame, 48, 22,
        super::power::millivolts(battery.raw, memory.calibration),
        Size::Normal,
    );

    frame.text_size(2, 34, b"ADC AVG", Size::Small);
    match battery.average {
        Some(value) => widgets::number(frame, 42, 34, value, Size::Small),
        None => frame.text_size(42, 34, b"NONE", Size::Small),
    }

    frame.text_size(2, 43, b"CAL[3]", Size::Small);
    match super::power::reference(memory.calibration) {
        Some(value) => {
            widgets::number(frame, 38, 43, value, Size::Small);
            frame.text_size(66, 43, b"AT 7.60V", Size::Small);
        }
        None => frame.text_size(38, 43, b"INVALID", Size::Small),
    }

    frame.text_size(2, 51, b"ADC ERR", Size::Small);
    widgets::number(frame, 42, 51, battery.error, Size::Small);
    frame.text_size(2, 58, b"ESTIMATE ONLY / EXIT HOME", Size::Small);
}

pub(super) fn radio(frame: &mut Frame, receiver: View, sql: u8) {
    frame.clear();
    frame.text(2, 0, b"RECEIVE INDICATORS");
    frame.fill(0, 8, 128, 1, true);

    if receiver.rx_signal != 0 {
        frame.text_size(2, 11, receiver.phase.name(), Size::Small);
        frame.text_size(88, 11, b"SQL", Size::Small);
        widgets::number(frame, 108, 11, sql as u32, Size::Small);

        frame.text_size(2, 20, b"RX SIG", Size::Small);
        frame.text_size(
            30, 20, &crate::tx_signal::text(receiver.rx_signal), Size::Small,
        );
        frame.text_size(66, 20, b"SIG", Size::Small);
        frame.text_size(
            82, 20, if receiver.signal { b"ON" } else { b"OFF" }, Size::Small,
        );

        // Hexadecimal register values. 02 retains the last nonzero event;
        // SEEN accumulates all event bits since the last gate reset.
        frame.text_size(2, 29, b"0C", Size::Small);
        frame.hex(14, 28, receiver.tone_debug[0] as u32);
        frame.text_size(68, 29, b"02", Size::Small);
        frame.hex(80, 28, receiver.tone_debug[1] as u32);

        frame.text_size(2, 38, b"SEEN", Size::Small);
        frame.hex(20, 37, receiver.tone_debug[2] as u32);
        frame.text_size(70, 38, if crate::dcs::decode(receiver.rx_signal).is_some() {
            b"TY"
        } else {
            b"68"
        }, Size::Small);
        frame.hex(80, 37, receiver.tone_debug[3] as u32);

        frame.text_size(2, 47, b"RSSI", Size::Small);
        widgets::number(frame, 24, 47, receiver.rssi, Size::Small);
        frame.text_size(66, 47, b"AUDIO", Size::Small);
        frame.text_size(
            90, 47, if receiver.audio { b"ON" } else { b"OFF" }, Size::Small,
        );

        frame.text_size(2, 58, b"RAW HEX / EXIT HOME", Size::Small);
        return;
    }
    frame.text_size(2, 11, receiver.phase.name(), Size::Small);
    frame.text_size(88, 11, b"SQL", Size::Small);
    widgets::number(frame, 108, 11, sql as u32, Size::Small);

    frame.text_size(2, 21, b"RSSI", Size::Small);
    widgets::number(frame, 42, 21, receiver.rssi, Size::Small);
    frame.text_size(2, 30, b"NOISE", Size::Small);
    widgets::number(frame, 42, 30, receiver.noise, Size::Small);
    frame.text_size(2, 39, b"GLITCH", Size::Small);
    widgets::number(frame, 42, 39, receiver.glitch, Size::Small);

    frame.text_size(2, 48, if receiver.signal {
        b"SQL OPEN"
    } else {
        b"SQL CLOSED"
    }, Size::Small);
    frame.text_size(70, 48, if receiver.audio {
        if receiver.monitor { b"MONITOR" } else { b"AUDIO ON" }
    } else {
        b"AUDIO OFF"
    }, Size::Small);
    frame.text_size(2, 58, if receiver.am {
        b"AM RSSI SQL / EXIT HOME"
    } else {
        b"RAW INDICATORS / EXIT HOME"
    }, Size::Small);
}

pub(super) fn runtime(frame: &mut Frame, state: crate::debug::timing::View) {
    frame.clear();
    frame.text(2, 0, b"FOREGROUND TIMING");
    frame.fill(0, 8, 128, 1, true);
    if !state.ready {
        frame.text(2, 18, b"WAIT");
        return;
    }

    let rows: [(&[u8], u32); 5] = [
        (b"LAST", state.last_work),
        (b"MAX", state.max_work),
        (b"GAP MAX", state.max_gap),
        (b"LONG", state.long_passes),
        (b"PASSES", state.passes),
    ];
    for (index, (label, value)) in rows.iter().enumerate() {
        let y = 11 + index as i16 * 9;
        frame.text_size(2, y, label, Size::Small);
        widgets::number(frame, 44, y, *value, Size::Small);
        if index < 3 {
            frame.text_size(106, y, b"MS", Size::Small);
        }
    }
    frame.text_size(2, 58, b"TICKS, NOT WCET / EXIT HOME", Size::Small);
}

pub(super) fn layout(frame: &mut Frame, memory: crate::debug::MemoryView, bank: &Bank) {
    frame.clear();
    frame.text(2, 0, b"RAM / CHANNEL BANK");
    frame.fill(0, 8, 128, 1, true);

    let rows: [(&[u8], u32); 3] = [
        (b"STATIC", memory.static_bytes),
        (b"STACK RESERVED", memory.reserved_stack),
        (b"UNASSIGNED", memory.unassigned),
    ];
    for (index, (label, bytes)) in rows.iter().enumerate() {
        let y = 11 + index as i16 * 9;
        frame.text_size(2, y, label, Size::Small);
        widgets::number(frame, 72, y, *bytes, Size::Small);
        frame.text_size(112, y, b"B", Size::Small);
    }

    frame.text_size(2, 38, b"CHANNELS", Size::Small);
    widgets::number(frame, 42, 38, bank.count(), Size::Small);
    frame.text_size(52, 38, b"/", Size::Small);
    widgets::number(frame, 60, 38, Bank::capacity() as u32, Size::Small);
    widgets::number(frame, 84, 38, core::mem::size_of::<Bank>() as u32, Size::Small);
    frame.text_size(112, 38, b"B", Size::Small);

    frame.text_size(2, 47, b"EACH SLOT", Size::Small);
    widgets::number(frame, 72, 47, Bank::slot_bytes() as u32, Size::Small);
    frame.text_size(92, 47, b"B", Size::Small);

    frame.text_size(2, 58, b"FIXED BANK / NO HEAP", Size::Small);
}

pub(super) fn serial(frame: &mut Frame, state: crate::drivers::uart::SerialView) {
    frame.clear();
    frame.text(2, 0, b"UART 38400 / DMA");
    frame.fill(0, 8, 128, 1, true);

    frame.text_size(2, 11, b"RX", Size::Small);
    widgets::number(frame, 18, 11, state.received, Size::Small);
    frame.text_size(68, 11, b"REQ", Size::Small);
    widgets::number(frame, 88, 11, state.requests, Size::Small);

    frame.text_size(2, 20, b"SHOT", Size::Small);
    widgets::number(frame, 26, 20, state.captures, Size::Small);
    frame.text_size(68, 20, b"RXERR", Size::Small);
    widgets::number(frame, 96, 20, state.errors, Size::Small);

    frame.text_size(2, 29, b"ORE", Size::Small);
    widgets::number(frame, 22, 29, state.overruns, Size::Small);
    frame.text_size(68, 29, b"TXERR", Size::Small);
    widgets::number(frame, 96, 29, state.tx_failures, Size::Small);

    frame.text_size(2, 38, b"FLAGS", Size::Small);
    frame.hex(34, 37, state.error_flags);
    frame.text_size(2, 47, b"SR", Size::Small);
    frame.hex(34, 46, state.status);

    frame.text_size(2, 58, b"N", Size::Small);
    widgets::number(frame, 10, 58, state.remaining, Size::Small);
    frame.text_size(34, 58, if state.rx_high { b"RX HIGH" } else { b"RX LOW" }, Size::Small);
    frame.text_size(80, 58, if state.live { b"RUNNING" } else { b"STOPPED" }, Size::Small);
}

