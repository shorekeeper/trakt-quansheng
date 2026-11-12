//! Single-profile Home with independently assigned ordinary rows.
//!
//! A selected CW VFO outside frequency entry delegates to home_cw. That
//! screen has fixed Morse, decoded-line, and settings regions. Ordinary
//! presentation choices remain unchanged on modulation transitions.
//!
//! Identity occupies pixels 0..7, the secondary label 10..15, and the
//! primary field 18..33. Three six-pixel data rows start at 36, 43, and 50.
//! The warning footer remains at 58 and cannot be reassigned. Rows keep
//! their positions when hidden or when reception changes.
//!
//! Auto 1/2/3 select TX/detail/meter for ordinary reception and RX text,
//! TX text, and key status for CW. Explicit choices may duplicate, reorder,
//! or hide rows. Detail and meter Auto rows follow their saved preferences.
//! Layout changes framing only; no saved presentation field configures RF.
//!
//! Large frequencies use the shared native digit renderer. Channel names
//! retain Large cell geometry. Identity, rows, and the footer use the fixed
//! Small font. Drawing owns neither another framebuffer nor widget state.
//!
//! Carrier, speaker audio, and CW key state have separate labels. Battery
//! is a factory-calibration estimate; RSSI is an uncalibrated raw indicator.
//! Rendering performs no RF access and changes no input admission.

use super::operation::{Bank, Mode, Operation};
use crate::drivers::{
    adc::Reading,
    bk4829::{receive::{Phase, View}, transmit::View as TxView},
    keyboard::Input,
};
use crate::storage::View as MemoryView;
use crate::ui::{font::Size, widgets, Frame};

#[cfg(feature = "cw")]
#[derive(Clone, Copy)]
pub(super) struct CwView<'a> {
    pub(super) rx_live: crate::cw_decode::Live,
    pub(super) tx_live: crate::cw_decode::Live,
    pub(super) rx_line: &'a [u8],
    pub(super) tx_line: &'a [u8],
    pub(super) input: &'static [u8],
    pub(super) usb: bool,
    pub(super) testing: bool,
    pub(super) wpm: u8,
    pub(super) pitch: u16,
    pub(super) hang: u16,
    pub(super) breakin: u8,
    pub(super) key: &'static [u8],
    pub(super) reverse: bool,
    pub(super) gated: bool,
    pub(super) qsk: bool,
    pub(super) qualified: bool,
    pub(super) contacts: Option<(bool, bool)>,
}

pub(super) const fn row_name(value: u8) -> &'static [u8] {
    match value {
        0 => b"AUTO 1",
        1 => b"AUTO 2",
        2 => b"AUTO 3",
        3 => b"PROFILE",
        4 => b"TX ROUTE",
        5 => b"SIGNAL",
        6 => b"BATTERY",
        7 => b"OTHER VFO",
        8 => b"UPTIME",
        9 => b"METER",
        10 => b"CW RX",
        11 => b"CW TX",
        12 => b"CW KEYS",
        _ => b"OFF",
    }
}

const fn row_kind(
    selected: u8,
    cw: bool,
    appearance: crate::app::appearance::Settings,
) -> u8 {
    match selected {
        0 if cw => 10,
        1 if cw => 11,
        2 if cw => 12,
        0 => 4,
        1 if appearance.layout == 2 || appearance.detail == 1 => 5,
        1 => match appearance.detail {
            0 => 3,
            2 => 6,
            3 => 7,
            4 => 8,
            _ => 13,
        },
        2 => 9,
        value => value,
    }
}

const fn step_name(step: u32) -> &'static [u8] {
    match step {
        100 => b"100HZ",
        12_500 => b"12.5K",
        25_000 => b"25K",
        _ => b"100K",
    }
}

fn meter(frame: &mut Frame, y: i16, signal: u32, selection: u8) {
    match selection {
        1 => widgets::bar(frame, 2, y, 124, 5, signal, 511),
        2 => widgets::signal(frame, y, signal),
        _ => {}
    }
}

#[inline(never)]
pub(super) fn render(
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
    vox_enabled: bool,
    vox_status: &[u8],
    #[cfg(feature = "cw")] cw: Option<CwView<'_>>,
) {
    #[cfg(feature = "cw")]
    if operation.cw_selected() && !operation.editing() {
        if let Some(state) = cw {
            super::home_cw::render(
                frame, operation, bank, receiver, transmitter, dirty, state,
                super::power::millivolts(battery.average, memory.calibration),
                vox_enabled,
            );
            return;
        }
    }

    frame.clear();

    let now = crate::mcu::tick::snapshot();
    let cw_mode = operation.cw_selected();
    let request = operation.request(bank);
    let tune = operation.tune(bank);
    let channel = if matches!(operation.mode, Mode::Channel) {
        bank.channel(operation.slot)
    } else {
        None
    };
    let matched = tune.is_some_and(|value| {
        receiver.frequency == value.frequency
            && receiver.am == value.am
            && receiver.narrow == if cw_mode { true } else { value.narrow }
    });
    let voltage = super::power::millivolts(battery.average, memory.calibration);

    if matches!(operation.mode, Mode::Channel) {
        frame.text_size(2, 1, b"CH", Size::Small);
        widgets::number(frame, 14, 1, operation.slot as u32 + 1, Size::Small);
    } else {
        frame.text_size(
            2, 1, if operation.active == 0 { b"VFO A" } else { b"VFO B" },
            Size::Small,
        );
    }
    frame.text_size(34, 1, operation.modulation_name(bank), Size::Small);
    // This label reflects the saved choice, not microphone readiness.
    if vox_enabled {
        frame.text_size(46, 1, b"VOX", Size::Small);
    }
    if dirty {
        frame.text_size(62, 1, b"*", Size::Small);
    }

    match appearance.battery {
        0 => widgets::battery(frame, voltage),
        1 => widgets::voltage(frame, 76, 1, voltage, Size::Small),
        _ => {}
    }

    let badge: &[u8] = if transmitter.active && cw_mode {
        if transmitter.carrier { b"TX" } else { b"GAP" }
    } else if transmitter.blocked && request.enabled {
        b"WAIT"
    } else if receiver.signal && matched {
        b"SIG"
    } else if request.enabled && matched && matches!(receiver.phase, Phase::Receive) {
        b"RX"
    } else {
        b"OFF"
    };
    widgets::badge(frame, 103, 0, badge);

    if appearance.layout == 1 {
        frame.invert(0, 0, 128, 8);
    } else {
        frame.fill(0, 8, 128, 1, true);
    }
    if appearance.layout == 2 {
        frame.fill(2, 34, 124, 1, true);
        frame.fill(2, 56, 124, 1, true);
    }

    if let Some(preview) = operation.preview() {
        frame.text_size(2, 10, b"ENTER RX FREQUENCY", Size::Small);
        frame.text_size(10, 18, &preview, Size::Large);
        frame.text_size(2, 38, if operation.entry_refused() {
            operation.notice()
        } else {
            b"6/7 KHZ DIGITS / AUTO APPLY"
        }, Size::Small);
        frame.text_size(2, 49, b"F DELETE / EMPTY F: LENGTH", Size::Small);
    } else if let Some(value) = tune {
        let named = channel.is_some_and(|value| value.named());
        let large_name = appearance.primary == 1 && named;

        if large_name {
            frame.text_size(2, 10, b"RX", Size::Small);
            widgets::frequency(frame, 16, 10, value.frequency, Size::Small);
            frame.text_size(16, 18, &channel.unwrap().name, Size::Large);
        } else {
            let label: &[u8] = match channel.as_ref() {
                Some(value) if value.named() => &value.name,
                Some(_) => b"UNNAMED CHANNEL",
                None if cw_mode => b"CARRIER MORSE",
                None if value.frequency < 280_000_000 => b"LOW RX PATH",
                None => b"HIGH RX PATH",
            };
            frame.text_size(2, 10, label, Size::Small);
            widgets::frequency(
                frame,
                if value.frequency >= 1_000_000_000 { 4 } else { 10 },
                18, value.frequency, Size::Large,
            );
        }

        let status: &[u8] = if transmitter.active && cw_mode {
            if transmitter.carrier { b"MARK" } else { b"SPACE" }
        } else if operation.scanning() {
            operation.scan_name()
        } else if !request.enabled {
            b"OFF"
        } else if !matched || !matches!(receiver.phase, Phase::Receive) {
            b"TUNING"
        } else if receiver.monitor && receiver.audio {
            b"MON"
        } else if receiver.signal && !receiver.audio && operation.rx_signal(bank) != 0 {
            b"SIG WAIT"
        } else if receiver.audio {
            b"AUDIO"
        } else {
            b"QUIET"
        };
        frame.text_size(
            126 - status.len() as i16 * 4, 10, status, Size::Small,
        );

        for row in 0..3 {
            let y = 36 + row as i16 * 7;
            let kind = row_kind(appearance.rows[row], cw_mode, appearance);

            match kind {
                3 => {
                    frame.text_size(2, y, b"SQL", Size::Small);
                    widgets::number(frame, 18, y, operation.squelch as u32, Size::Small);
                    frame.text_size(32, y, b"STEP", Size::Small);
                    frame.text_size(
                        52, y, step_name(if cw_mode { 100 } else { value.step }),
                        Size::Small,
                    );
                    frame.text_size(
                        104, y, super::tx_policy::power_name(transmitter.power),
                        Size::Small,
                    );
                }
                4 => {
                    frame.text_size(2, y, b"TX", Size::Small);
                    match operation.transmit_with(bank, slope) {
                        Ok(frequency) => {
                            widgets::frequency(
                                frame, 14, y,
                                if value.am { value.frequency } else { frequency },
                                Size::Small,
                            );

                            if let Some(route) = operation.displayed_route(bank) {
                                frame.text_size(60, y, if cw_mode {
                                    b"CW"
                                } else if cfg!(feature = "fam") && value.am && slope.enabled {
                                    b"SC-FM"
                                } else {
                                    route.name()
                                }, Size::Small);
                            }

                            let signal = operation.tx_signal(bank);
                            if signal != 0 && !value.am {
                                frame.text_size(
                                    102, y, &crate::tx_signal::text(signal), Size::Small,
                                );
                            }
                        }
                        Err(0x700C) => frame.text_size(14, y, b"DISABLED", Size::Small),
                        Err(0x700F) => frame.text_size(14, y, b"AM RX ONLY", Size::Small),
                        Err(0x7A05) => frame.text_size(14, y, b"CW NOT IN IMAGE", Size::Small),
                        Err(0x7A01) => frame.text_size(14, y, b"CW NEEDS SIMPLEX", Size::Small),
                        Err(_) => frame.text_size(14, y, b"ROUTE OUT OF BAND", Size::Small),
                    }
                }
                5 => {
                    frame.text_size(2, y, b"R", Size::Small);
                    widgets::number(frame, 10, y, receiver.rssi, Size::Small);
                    frame.text_size(34, y, b"N", Size::Small);
                    widgets::number(frame, 42, y, receiver.noise, Size::Small);
                    frame.text_size(66, y, b"G", Size::Small);
                    widgets::number(frame, 74, y, receiver.glitch, Size::Small);
                    frame.text_size(104, y, b"RAW", Size::Small);
                }
                6 => {
                    frame.text_size(2, y, b"BAT", Size::Small);
                    widgets::voltage(frame, 20, y, voltage, Size::Small);
                    frame.text_size(58, y, b"ADC", Size::Small);
                    match battery.average {
                        Some(value) => widgets::number(frame, 78, y, value, Size::Small),
                        None => frame.text_size(78, y, b"NONE", Size::Small),
                    }
                }
                7 => {
                    let other = operation.other_vfo();
                    frame.text_size(
                        2, y, if operation.active == 0 { b"B" } else { b"A" },
                        Size::Small,
                    );
                    widgets::frequency(frame, 12, y, other.frequency, Size::Small);
                    frame.text_size(58, y, if operation.vfo_cw(operation.active ^ 1) {
                        b"CW"
                    } else {
                        other.name()
                    }, Size::Small);
                    frame.text_size(84, y, b"INACTIVE", Size::Small);
                }
                8 => {
                    frame.text_size(2, y, b"UPTIME", Size::Small);
                    widgets::number(frame, 34, y, now / 1000, Size::Small);
                    frame.text_size(92, y, b"SECONDS", Size::Small);
                }
                9 => meter(
                    frame, y, if matched { receiver.rssi } else { 0 },
                    appearance.meter,
                ),
                10..=12 => {
                    if cw_mode && !cfg!(feature = "cw") {
                        frame.text_size(2, y, b"CW NOT IN THIS IMAGE", Size::Small);
                    }
                }
                _ => {}
            }
        }
    } else {
        frame.text_size(2, 10, b"CHANNEL", Size::Small);
        frame.text_size(34, 18, b"EMPTY", Size::Large);
        frame.text_size(2, 38, b"CHOOSE A POPULATED SLOT", Size::Small);
    }

    if transmitter.error != 0 {
        if transmitter.blink {
            frame.text_size(2, 58, b"TX ERROR", Size::Small);
            frame.hex(42, 56, transmitter.error);
        } else {
            frame.text_size(
                2, 58, super::tx_policy::reason(transmitter.error), Size::Small,
            );
        }
        return;
    }
    if receiver.error != 0 {
        frame.text_size(2, 58, b"RX ERROR", Size::Small);
        frame.hex(42, 56, receiver.error);
        return;
    }

    let warning: &[u8] = if operation.editing() {
        b"F DEL / EMPTY F:6/7 / EXIT"
    } else if !transmitter.ham {
        b"HAM OFF / CHECK AUTHORIZATION"
    } else if !operation.notice().is_empty() {
        operation.notice()
    } else if cw_mode && !cfg!(feature = "cw") {
        b"CW MODULE NOT INCLUDED"
    } else if !request.enabled {
        b"RX OFF / SIDE2 ENABLE"
    } else if cw_mode {
        #[cfg(feature = "cw")]
        {
            match cw {
                Some(state) if state.testing => b"USB INPUT TEST / NO RF",
                Some(_) if transmitter.active => transmitter.status,
                Some(state) if !state.qualified => b"RELEASE BOTH KEYS",
                _ if appearance.hints => b"MENU: CW SETTINGS / 100HZ",
                _ => b"",
            }
        }

        #[cfg(not(feature = "cw"))]
        {
            b"CW MODULE NOT INCLUDED"
        }
    } else if input.ptt() || transmitter.blocked {
        b"RELEASE PTT"
    } else if vox_enabled {
        vox_status
    } else if appearance.hints {
        b"MENU SETUP / SIDE2 RX"
    } else {
        b""
    };
    frame.text_size(2, 58, warning, Size::Small);
}

const _: () = {
    let defaults = crate::app::appearance::Settings::new();
    assert!(row_kind(0, false, defaults) == 4);
    assert!(row_kind(1, false, defaults) == 3);
    assert!(row_kind(2, false, defaults) == 9);
    assert!(row_kind(0, true, defaults) == 10);
    assert!(row_kind(1, true, defaults) == 11);
    assert!(row_kind(2, true, defaults) == 12);

    let mut explicit = 3;
    while explicit < 14 {
        assert!(row_kind(explicit, false, defaults) == explicit);
        assert!(row_kind(explicit, true, defaults) == explicit);
        assert!(!row_name(explicit).is_empty());
        explicit += 1;
    }

    assert!(36 + 6 <= 43);
    assert!(43 + 6 <= 50);
    assert!(50 + 6 <= 58);
};