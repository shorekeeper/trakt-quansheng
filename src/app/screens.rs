//! Radio screen composition without device access.
//!
//! Ordinary Home is composed by home and re-exported here. Dual Watch Home
//! belongs to home_dual and is called by Panel directly. This module keeps
//! TX progress, storage status, About, HAM confirmation, and Controls.
//!
//! TX progress uses observed elapsed ticks and the admitted attempt budget.
//! The microphone display consumes a published REG_64 amplitude observation,
//! not PCM or measured deviation. Battery voltage is a calibration-based
//! estimate; RSSI remains a raw indicator.
//!
//! Screens render into the caller-owned framebuffer. They neither retune RF,
//! write settings, acknowledge receive events, nor alter watchdog deadlines.

use crate::drivers::{
    adc::Reading,
    bk4829::transmit::View as TxView,

};
use crate::storage::{journal::View as JournalView, View as MemoryView};
use crate::ui::{font::Size, widgets, Frame};





pub(super) use super::home::render as home;
#[inline(never)]
pub(super) fn transmit(
    frame: &mut Frame,
    state: TxView,
    battery: Reading,
    memory: MemoryView,
    appearance: crate::app::appearance::Settings,
    vox_enabled: bool,
) {
    frame.clear();

    frame.text_size(
        3, 1,
        if state.error != 0 { b"LOCAL ALERT" } else { b"TRANSMIT" },
        Size::Small,
    );

    frame.text_size(
        44, 1, crate::app::tx_policy::power_name(state.power), Size::Small,
    );

    // A preference label does not identify the source of this TX attempt.
    if vox_enabled {
        frame.text_size(66, 1, b"VOX", Size::Small);
    }
    widgets::battery(
        frame,
        super::power::millivolts(battery.raw, memory.calibration),
    );
    widgets::badge(frame, 103, 0, if state.carrier { b"TX" } else { b"WAIT" });
    frame.fill(0, 8, 128, 1, true);

    widgets::frequency(frame, 4, 12, state.frequency, Size::Large);

    frame.text_size(4, 31, if state.slope {
        b"SC"
    } else if state.narrow {
        b"NFM"
    } else {
        b"FM"
    }, Size::Small);

    if state.error != 0 {
        if state.blink {
            frame.text_size(24, 31, b"TX ERROR", Size::Small);
        }

        frame.hex(72, 30, state.error);
        frame.text_size(
            3, 43, crate::app::tx_policy::reason(state.error), Size::Small,
        );
        frame.text_size(
            3, 58, crate::app::tx_policy::service(state.frequency), Size::Small,
        );
        return;
    }

    frame.text_size(24, 31, state.status, Size::Small);

    if appearance.tx_visual == 0 {
        frame.text_size(4, 42, b"ELAPSED", Size::Small);
        widgets::number(frame, 40, 42, state.elapsed, Size::Small);
        frame.text_size(64, 42, b"MS", Size::Small);
        frame.text_size(82, 42, b"BIAS", Size::Small);
        widgets::number(frame, 104, 42, state.bias, Size::Small);
    } else {
        let microphone = crate::drivers::bk4829::meter::view();

        if microphone.active {
            match appearance.tx_visual {
                1 => {
                    frame.text_size(3, 41, b"MIC", Size::Small);
                    widgets::bar(frame, 22, 40, 103, 8, microphone.level, 255);

                    let peak = widgets::extent(microphone.peak, 255, 100);
                    frame.fill(23 + peak as i16, 40, 1, 8, true);
                }
                _ => {
                    let first = if microphone.count == 64 {
                        microphone.index
                    } else {
                        0
                    };

                    for column in 0..microphone.count {
                        let index = (first + column) & 63;
                        let height = widgets::extent(
                            microphone.history[index] as u32, 255, 14,
                        ) as u16;

                        if height == 0 {
                            frame.pixel(column as i32 * 2, 48, true);
                        } else {
                            frame.fill(
                                column as i16 * 2,
                                49 - height as i16,
                                1,
                                height,
                                true,
                            );
                        }
                    }
                }
            }

            if appearance.layout == 2 {
                // Raw register observations for this voice-TX run.
                // LO is an observed minimum, not an estimated noise floor.
                frame.text_size(2, 58, b"R", Size::Small);
                widgets::number(frame, 10, 58, microphone.raw, Size::Small);
                frame.text_size(38, 58, b"LO", Size::Small);
                widgets::number(frame, 50, 58, microphone.minimum, Size::Small);
                frame.text_size(78, 58, b"HI", Size::Small);
                widgets::number(frame, 90, 58, microphone.maximum, Size::Small);
            }
        } else {
            frame.text_size(4, 42, b"MIC METER: VOICE TX ONLY", Size::Small);
        }
    }

    widgets::bar(frame, 3, 52, 122, 4, state.elapsed, state.budget);

    if appearance.layout != 2 || appearance.tx_visual == 0 {
        frame.text_size(3, 58, if state.status == b"VOX TRANSMITTING" {
            b"VOICE HOLD / PTT OR EXIT STOP"
        } else {
            b"RELEASE PTT / EXIT STOP"
        }, Size::Small);
    }
}

pub(super) use super::diagnostic_screens::{
    inputs, memory, stacks, last_fault, power, radio, runtime, layout, serial,
};
pub(super) fn files(
    frame: &mut Frame,
    journal: JournalView,
    reset: bool,
    dirty: bool,
) {
    frame.clear();
    frame.text(2, 0, b"STORAGE STATUS");

    if dirty {
        frame.text_size(106, 1, b"* RAM", Size::Small);
    }
    frame.fill(0, 8, 128, 1, true);

    if journal.error == 612 {
        frame.text(2, 10, journal.search.status);
        frame.text(2, 20, b"A");
        frame.hex(26, 20, journal.search.pair[0]);
        frame.text(2, 30, b"B");
        frame.hex(26, 30, journal.search.pair[1]);
        frame.text(2, 40, b"CHECKED");
        frame.decimal(56, 40, journal.search.inspected);
        frame.text_size(2, 57, b"NO ERASE / EXIT HOME", Size::Small);
        return;
    }

    if journal.error != 0 && journal.diagnostic[0] != 0 {
        frame.text(2, 10, b"ERR");
        frame.decimal(42, 10, journal.error);
        frame.text(2, 18, b"PHASE");
        frame.text(42, 18, match journal.diagnostic[0] {
            1 => b"TX", 2 => b"RX", 3 => b"DRAIN", _ => b"DEAD",
        });
        frame.text(2, 26, b"BYTE");
        frame.decimal(42, 26, journal.diagnostic[3]);
        frame.text(2, 34, b"SR");
        frame.hex(42, 34, journal.diagnostic[1]);
        frame.text(2, 42, b"CR1");
        frame.hex(42, 42, journal.diagnostic[2]);
        frame.text_size(2, 57, b"RESTART REQUIRED / EXIT HOME", Size::Small);
        return;
    }

    frame.text(2, 12, if reset { b"RAM RESET" } else { journal.status });
    frame.text(2, 24, b"GEN");
    frame.decimal(42, 24, journal.generation);
    frame.text(2, 34, b"COPIES");
    frame.decimal(54, 34, journal.valid_copies);
    frame.text_size(2, 45, b"FMT", Size::Small);
    widgets::number(frame, 22, 45, journal.format, Size::Small);
    frame.text_size(48, 45, b"ERR", Size::Small);
    widgets::number(frame, 68, 45, journal.error, Size::Small);
    frame.text_size(2, 57, b"MENU SETUP / EXIT HOME", Size::Small);
}


/// Build identity with an inverse title panel and variant strip.
/// Text is drawn before inversion so glyph cells and surrounding background
/// share the same contrast. No additional font, bitmap, or frame is stored.
/// The build label uses its actual ASCII cell width for horizontal centering.
pub(super) fn about(frame: &mut Frame) {
    frame.clear();
    frame.outline(1, 1, 126, 23, true);
    frame.text_size(34, 5, b"TRAKT", Size::Large);
    frame.invert(3, 3, 122, 19);

    let label_x = (128 - crate::build_info::LABEL.len() as i16 * 4) / 2;
    frame.text_size(label_x, 27, crate::build_info::LABEL, Size::Small);
    frame.fill(4, 34, 120, 1, true);

    frame.text_size(8, 36, b"VERSION", Size::Small);
    frame.text_size(44, 36, crate::build_info::VERSION, Size::Small);
    frame.text_size(8, 44, b"UV-K5 V3 / PY32F071", Size::Small);
    frame.text_size(8, 51, if cfg!(feature = "cw") {
        b"VARIANT: CW"
    } else if cfg!(feature = "fsk") {
        b"VARIANT: FSK PROBE"
    } else if cfg!(feature = "fam") {
        if cfg!(feature = "diagnostics") && cfg!(feature = "spectrum")
            && cfg!(feature = "uart") && !cfg!(feature = "debug-tools")
        {
            b"VARIANT: FULL+FAM"
        } else {
            b"VARIANT: CUSTOM"
        }
    } else if cfg!(feature = "debug-tools") {
        if cfg!(feature = "uart") && !cfg!(feature = "spectrum") {
            b"VARIANT: DEBUG"
        } else {
            b"VARIANT: CUSTOM"
        }
    } else if !cfg!(feature = "uart") {
        if cfg!(feature = "diagnostics") || cfg!(feature = "spectrum") {
            b"VARIANT: CUSTOM"
        } else {
            b"VARIANT: STANDALONE"
        }
    } else if cfg!(feature = "spectrum") {
        if cfg!(feature = "diagnostics") {
            b"VARIANT: FULL"
        } else {
            b"VARIANT: RADIO"
        }
    } else if cfg!(feature = "diagnostics") {
        b"VARIANT: CUSTOM"
    } else {
        b"VARIANT: COMPACT"
    }, Size::Small);
    frame.invert(4, 50, 120, 7);
    frame.text_size(2, 58, b"MENU SETUP / EXIT HOME", Size::Small);
}

pub(super) fn ham_warning(frame: &mut Frame, stage: u8) {
    frame.clear();

    // Draw the warning symbol directly; no font-specific punctuation
    // support or additional bitmap storage is required.
    for row in 0..11 {
        frame.pixel(64 - row, 1 + row, true);
        frame.pixel(64 + row, 1 + row, true);
    }
    frame.fill(54, 11, 21, 1, true);
    frame.fill(64, 5, 1, 3, true);
    frame.pixel(64, 9, true);

    let title: &[u8] = if stage == 1 {
        b"DISABLE HAM FILTER?"
    } else {
        b"CONFIRM HAM OFF"
    };
    frame.text((128 - title.len() as i16 * 6) / 2, 16, title);
    frame.fill(0, 25, 128, 1, true);

    let lines: [&[u8]; 3] = if stage == 1 {
        [
            b"DISABLE TX ALLOCATION FILTER",
            b"RX COVERAGE STAYS UNCHANGED",
            b"CHECK YOUR TX AUTHORIZATION",
        ]
    } else {
        [
            b"SAVE / AUTO KEEPS THIS SETTING",
            b"LOAD RESTORES SAVED HAM MODE",
            b"RESET RAM ENABLES HAM FILTER",
        ]
    };

    for (index, line) in lines.iter().enumerate() {
        let x = (128 - line.len() as i16 * 4) / 2;
        frame.text_size(x, 31 + index as i16 * 8, line, Size::Small);
    }

    frame.text_size(2, 58, if stage == 1 {
        b"MENU CONTINUE / EXIT CANCEL"
    } else {
        b"MENU DISABLE / EXIT CANCEL"
    }, Size::Small);
}

pub(super) fn help(frame: &mut Frame) {
    frame.clear();
    frame.text(2, 0, b"MAIN CONTROLS");
    frame.fill(0, 8, 128, 1, true);
    let lines: [&[u8]; 7] = [
        b"MENU: OPEN SETTINGS",
        b"UP/DOWN: TUNE OR CHANNEL",
        b"DIGITS: KHZ / AUTO APPLY",
        b"F TAP: A/B / F HOLD: MONITOR",
        b"STAR: VFO/MR / SIDE2: RX",
        b"SIDE1: LIGHT / EXIT: RX OFF",
        b"PTT: FILTERED TX / SAVED TOT",
    ];
    for (index, line) in lines.iter().enumerate() {
        frame.text_size(2, 11 + index as i16 * 7, line, Size::Small);
    }
    frame.text_size(2, 58, b"EXIT HOME", Size::Small);
}
