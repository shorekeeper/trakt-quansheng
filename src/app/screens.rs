//! Radio screen composition without device access.
//!
//! The normal Home view separates profile identification, receive frequency,
//! effective transmit destination, route, and selected TX signaling.
//! Presentation choices control the primary field, detail row, battery,
//! meter style, and optional hints without changing RF admission.
//!
//! The DW view distinguishes the selected transmit VFO from the temporary
//! settled receive destination. Carrier indication remains independent of
//! whether CTCSS/DCS opens the speaker.
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
    bk4829::{receive::{Phase, View}, transmit::View as TxView},
    keyboard::Input,
};
use crate::storage::{journal::View as JournalView, View as MemoryView};
use crate::ui::{font::Size, widgets, Frame};

use super::operation::{Bank, Mode, Operation};



/// The header distinguishes selected TX from the current settled receiver.
/// Each row keeps RX large and shows that VFO's effective TX below it.
/// Carrier hold is independent of the selected row and of signal decoding.
#[inline(never)]
pub(super) fn dual_home(
    frame: &mut Frame,
    watch: &super::dual::Watch,
    operation: &Operation,
    receiver: View,
    transmitter: TxView,
    battery: Reading,
    memory: MemoryView,
    dirty: bool,
    slope: crate::slope::Settings,
) {
    frame.clear();
    frame.text_size(2, 1, b"DW", Size::Small);
    frame.text_size(
        18, 1, &[b'T', b'X', b':', b'A' + operation.active as u8], Size::Small,
    );

    let listening = watch.listening();
    let request = operation.vfo_request(listening);
    let settled = matches!(receiver.phase, Phase::Receive)
        && receiver.frequency == request.frequency
        && receiver.narrow == request.narrow
        && receiver.am == request.am
        && receiver.rx_signal == request.rx_signal;

    frame.text_size(
        42, 1,
        &[b'R', b'X', b':', if settled { b'A' + listening as u8 } else { b'-' }],
        Size::Small,
    );
    if dirty { frame.text_size(64, 1, b"*", Size::Small); }

    widgets::battery(frame, super::power::millivolts(
        battery.average, memory.calibration,
    ));
    widgets::badge(frame, 103, 0, if receiver.signal { b"SIG" } else { b"DW" });
    frame.fill(0, 8, 128, 1, true);

    for index in 0..2 {
        let profile = operation.vfo_profile(index);
        let request = operation.vfo_request(index);
        let y = 11 + index as i16 * 24;

        frame.text_size(2, y, &[b'A' + index as u8], Size::Small);
        if index == operation.active {
            frame.text_size(2, y + 8, b">", Size::Small);
        }
        widgets::frequency(frame, 8, y, profile.tune.frequency, Size::Large);
        frame.text_size(2, y + 17, b"TX", Size::Small);

        match crate::slope::resolve(profile, slope) {
            Ok(frequency) => widgets::frequency(
                frame, 14, y + 17,
                if profile.tune.am { profile.tune.frequency } else { frequency },
                Size::Small,
            ),
            Err(0x700C | 0x700F) => frame.text_size(14, y + 17, b"OFF", Size::Small),
            Err(_) => frame.text_size(14, y + 17, b"OUT OF BAND", Size::Small),
        }

        let route: &[u8] = match profile.transmit {
            crate::profile::Tx::Simplex => b"=",
            crate::profile::Tx::Plus(_) => b"+",
            crate::profile::Tx::Minus(_) => b"-",
            crate::profile::Tx::Split(_) => b"SPL",
            crate::profile::Tx::ReceiveOnly => b"OFF",
        };
        frame.text_size(60, y + 17, &route[..1], Size::Small);
        frame.text_size(66, y + 17, if cfg!(feature = "fam")
            && profile.tune.am && slope.enabled
        {
            b"SC"
        } else {
            profile.tune.name()
        }, Size::Small);
        if request.rx_signal != 0 {
            frame.text_size(82, y + 17, b"R", Size::Small);
            frame.text_size(
                90, y + 17, &crate::tx_signal::text(request.rx_signal), Size::Small,
            );
        }
    }

    let status: &[u8] = if transmitter.error != 0 {
        crate::app::tx_policy::reason(transmitter.error)
    } else if !transmitter.ham {
        b"HAM OFF / CHECK AUTHORIZATION"
    } else if transmitter.blocked {
        b"RELEASE PTT"
    } else {
        watch.status()
    };
    frame.text_size(2, 58, status, Size::Small);
}

#[inline(never)]
pub(super) fn home(
    frame: &mut Frame,
    operation: &Operation,
    bank: &Bank,
    input: &Input,
    receiver: View,
    transmitter: TxView,
    battery: Reading,
    memory: MemoryView,
    dirty: bool,
    appearance: crate::app::appearance::Settings,
    slope: crate::slope::Settings,
) {
    frame.clear();

    let channel = if matches!(operation.mode, Mode::Channel) {
        bank.channel(operation.slot)
    } else {
        None
    };

    let tune = operation.tune(bank);
    let matched = tune.is_some_and(|tune| {
        tune.frequency == receiver.frequency && tune.narrow == receiver.narrow && tune.am == receiver.am
    });

    if matches!(operation.mode, Mode::Channel) {
        frame.text_size(3, 1, b"CH", Size::Small);
        widgets::number(frame, 15, 1, operation.slot as u32 + 1, Size::Small);
    } else {
        frame.text_size(
            3, 1,
            if operation.active == 0 { b"VFO A" } else { b"VFO B" },
            Size::Small,
        );
    }

    if let Some(tune) = tune {
        frame.text_size(
            35, 1,
            if tune.frequency < 280_000_000 { b"LOW" } else { b"HIGH" },
            Size::Small,
        );
        frame.text_size(53, 1, tune.name(), Size::Small);
    }

    if dirty {
        frame.text_size(69, 1, b"*", Size::Small);
    }

    let voltage = super::power::millivolts(battery.average, memory.calibration);

    match appearance.battery {
        0 => widgets::battery(frame, voltage),
        1 => widgets::voltage(frame, 76, 1, voltage, Size::Small),
        _ => {}
    }

    let badge: &[u8] = if transmitter.blocked {
        b"WAIT"
    } else if receiver.signal {
        b"SIG"
    } else if matches!(receiver.phase, Phase::Receive) && matched {
        b"RX"
    } else {
        b"OFF"
    };

    if appearance.layout == 1 {
        frame.text_size(104, 1, badge, Size::Small);
        frame.invert(0, 0, 128, 8);
    } else {
        widgets::badge(frame, 103, 0, badge);
        frame.fill(0, 8, 128, 1, true);
    }

    if let Some(preview) = operation.preview() {
        frame.text_size(4, 10, b"ENTER RX FREQUENCY", Size::Small);
        frame.text_size(10, 18, &preview, Size::Large);
        frame.text_size(4, 36, if operation.entry_refused() {
            operation.notice()
        } else {
            b"6/7 KHZ DIGITS / AUTO APPLY"
        }, Size::Small);
    } else if let Some(tune) = tune {
        let show_name = appearance.primary == 1
            && channel.is_some_and(|channel| channel.named());

        if show_name {
            frame.text_size(4, 10, b"RX", Size::Small);
            widgets::frequency(frame, 18, 10, tune.frequency, Size::Small);
            frame.text_size(16, 18, &channel.unwrap().name, Size::Large);
        } else {
            let label: &[u8] = match channel.as_ref() {
                Some(channel) if channel.named() => &channel.name,
                Some(_) => b"UNNAMED CHANNEL",
                None if operation.active == 0 => b"VFO A",
                None => b"VFO B",
            };
            frame.text_size(4, 10, label, Size::Small);
            widgets::frequency(frame, 4, 18, tune.frequency, Size::Large);
        }

        let status: &[u8] = if operation.scanning() {
            operation.scan_name()
        } else if !matched && matches!(receiver.phase, Phase::Receive) {
            b"TUNING"
        } else if receiver.monitor && receiver.audio {
            b"MONITOR"
        } else if receiver.signal && !receiver.audio && operation.rx_signal(bank) != 0 {
            b"SIG WAIT"
        } else if receiver.signal {
            b"OPEN"
        } else {
            receiver.phase.name()
        };
        frame.text_size(88, 10, status, Size::Small);

        frame.text_size(4, 36, b"TX", Size::Small);
        match operation.transmit_with(bank, slope) {
            Ok(frequency) => widgets::frequency(
                frame, 18, 36,
                if tune.am { tune.frequency } else { frequency },
                Size::Small,
            ),
            Err(0x700C) => frame.text_size(18, 36, b"DISABLED", Size::Small),
            Err(0x700F) => frame.text_size(18, 36, b"AM RX ONLY", Size::Small),
            Err(_) => frame.text_size(18, 36, b"OUT OF BAND", Size::Small),
        }

        if let Some(route) = operation.displayed_route(bank) {
            let label: &[u8] = match route {
                crate::profile::Tx::Simplex => b"=",
                crate::profile::Tx::Plus(_) => b"+",
                crate::profile::Tx::Minus(_) => b"-",
                crate::profile::Tx::Split(_) => b"SPL",
                crate::profile::Tx::ReceiveOnly => b"OFF",
            };
            frame.text_size(66, 36, if cfg!(feature = "fam") && tune.am && slope.enabled {
                b"SC-FM"
            } else {
                label
            }, Size::Small);
        }

        let tone = if cfg!(feature = "fam") && tune.am && slope.enabled {
            0
        } else {
            operation.tx_signal(bank)
        };
        if tone != 0 {
            frame.text_size(94, 36, &crate::tx_signal::text(tone), Size::Small);
        }
    } else {
        frame.text_size(4, 10, b"CHANNEL", Size::Small);
        frame.text_size(16, 18, b"EMPTY", Size::Large);
        frame.text_size(4, 36, b"CHOOSE A POPULATED SLOT", Size::Small);
    }

    if appearance.layout == 2 || appearance.detail == 1 {
        frame.text_size(4, 44, b"R", Size::Small);
        widgets::number(frame, 12, 44, receiver.rssi, Size::Small);
        frame.text_size(38, 44, b"N", Size::Small);
        widgets::number(frame, 46, 44, receiver.noise, Size::Small);
        frame.text_size(72, 44, b"G", Size::Small);
        widgets::number(frame, 80, 44, receiver.glitch, Size::Small);
        frame.text_size(108, 44, b"RAW", Size::Small);
    } else {
        match appearance.detail {
            0 => {
                frame.text_size(4, 44, b"SQL", Size::Small);
                widgets::number(frame, 20, 44, operation.squelch as u32, Size::Small);

                if let Some(tune) = tune {
                    frame.text_size(35, 44, b"STEP", Size::Small);
                    widgets::number(frame, 55, 44, tune.step / 100, Size::Small);
                    frame.text_size(76, 44, b"X100HZ", Size::Small);
                }

                frame.text_size(
                    103, 44, crate::app::tx_policy::power_name(transmitter.power),
                    Size::Small,
                );
            }
            2 => {
                frame.text_size(4, 44, b"BAT", Size::Small);
                widgets::voltage(frame, 22, 44, voltage, Size::Small);
                frame.text_size(58, 44, b"ADC", Size::Small);

                match battery.average {
                    Some(value) => widgets::number(frame, 76, 44, value, Size::Small),
                    None => frame.text_size(76, 44, b"NONE", Size::Small),
                }
            }
            3 => {
                let other = operation.other_vfo();
                frame.text_size(
                    4, 44,
                    if operation.active == 0 { b"B" } else { b"A" },
                    Size::Small,
                );
                widgets::frequency(frame, 14, 44, other.frequency, Size::Small);
                frame.text_size(58, 44, other.name(), Size::Small);
                frame.text_size(80, 44, b"INACTIVE", Size::Small);
            }
            4 => {
                frame.text_size(4, 44, b"UPTIME", Size::Small);
                widgets::number(
                    frame, 38, 44, crate::mcu::tick::snapshot() / 1000, Size::Small,
                );
                frame.text_size(82, 44, b"SECONDS", Size::Small);
            }
            _ => {}
        }
    }

    let signal = if matched { receiver.rssi } else { 0 };

    match appearance.meter {
        1 => {
            widgets::bar(frame, 3, 51, 122, 5, signal, 511);
        }
        2 => {
            widgets::signal(frame, signal);
        }
        _ => {}
    }

    if transmitter.error != 0 {
        if transmitter.blink {
            frame.text_size(3, 58, b"TX ERROR", Size::Small);
            frame.hex(43, 56, transmitter.error);
        } else {
            frame.text_size(
                3, 58, crate::app::tx_policy::reason(transmitter.error), Size::Small,
            );
        }
    } else if receiver.error != 0 {
        frame.text_size(3, 58, b"RX ERROR", Size::Small);
        frame.hex(43, 56, receiver.error);
    } else {
        let warning: &[u8] = if operation.editing() {
            b"F DEL / EMPTY F:6/7 / EXIT"
        } else if !transmitter.ham {
            b"HAM OFF / CHECK AUTHORIZATION"
        } else if input.ptt() || transmitter.blocked {
            b"RELEASE PTT"
        } else if !operation.notice().is_empty() {
            operation.notice()
        } else if appearance.hints {
            b"MENU SETUP / SIDE2 RX"
        } else {
            b""
        };

        frame.text_size(3, 58, warning, Size::Small);
    }
}

#[inline(never)]
pub(super) fn transmit(
    frame: &mut Frame,
    state: TxView,
    battery: Reading,
    memory: MemoryView,
    appearance: crate::app::appearance::Settings,
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
        frame.text_size(3, 58, b"RELEASE PTT / EXIT STOP", Size::Small);
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


pub(super) fn about(frame: &mut Frame) {
    frame.clear();
    frame.text_size(34, 5, b"TRAKT", Size::Large);
    frame.text_size(8, 27, crate::build_info::LABEL, Size::Small);
    frame.text_size(8, 36, b"VERSION", Size::Small);
    frame.text_size(44, 36, crate::build_info::VERSION, Size::Small);
    frame.text_size(8, 44, b"UV-K5 V3 / PY32F071", Size::Small);
    frame.text_size(8, 51, if cfg!(feature = "fam") {
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