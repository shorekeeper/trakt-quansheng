//! Transmit power labels and checked timeout selections.
//!
//! Power indices 0..4 identify Low1..Low5; 5..9 identify Medium1..Medium5;
//! 10..12 identify High1..High3. Index 13 selects factory High without an
//! additive boost. Labels identify PA-code profiles, not measured watts.
//!
//! TOT includes the admitted attempt's preparation, any selected-VFO receive
//! qualification, local permit audio, speech, and Roger. No Off value exists.
//! Choices are 15, 30, 60, 90, 120, and 180 seconds, with 30 seconds as default.
//!
//! The foreground retains one fixed attempt start and duration. The SysTick
//! guard uses that start with an extra 1000 observed ticks and is armed before
//! TX setup. Watchdog service remains independent of SysTick progress.
//! Changing settings cannot extend an already active attempt.

pub(crate) const POWER_COUNT: u8 = 14;
pub(crate) const DEFAULT_TOT: u8 = 1;

const SECONDS: [u32; 6] = [15, 30, 60, 90, 120, 180];

pub(crate) const fn power_name(power: u8) -> &'static [u8] {
    match power {
        0 => b"LOW1",
        1 => b"LOW2",
        2 => b"LOW3",
        3 => b"LOW4",
        4 => b"LOW5",
        5 => b"MED1",
        6 => b"MED2",
        7 => b"MED3",
        8 => b"MED4",
        9 => b"MED5",
        10 => b"HIGH1",
        11 => b"HIGH2",
        12 => b"HIGH3",
        13 => b"MAX",
        _ => b"INVALID",
    }
}

pub(crate) const fn timeout_count() -> u32 {
    SECONDS.len() as u32
}

pub(crate) const fn timeout_ticks(selection: u8) -> Option<u32> {
    if selection as usize >= SECONDS.len() {
        None
    } else {
        Some(SECONDS[selection as usize] * 1000)
    }
}

pub(crate) const fn timeout_name(selection: u32) -> &'static [u8] {
    match selection {
        0 => b"15S",
        1 => b"30S",
        2 => b"60S",
        3 => b"90S",
        4 => b"120S",
        5 => b"180S",
        _ => b"INVALID",
    }
}

pub(crate) const fn valid_ticks(ticks: u32) -> bool {
    let mut index = 0;

    while index < SECONDS.len() {
        if ticks == SECONDS[index] * 1000 {
            return true;
        }

        index += 1;
    }

    false
}

const _: () = {
    assert!(POWER_COUNT == 14);
    assert!(timeout_count() == 6);
    assert!(matches!(timeout_ticks(DEFAULT_TOT), Some(30000)));
    assert!(matches!(timeout_ticks(0), Some(15000)));
    assert!(matches!(timeout_ticks(5), Some(180000)));
    assert!(timeout_ticks(6).is_none());

    assert!(!valid_ticks(0));
    assert!(!valid_ticks(5000));
    assert!(!valid_ticks(30001));
    assert!(!valid_ticks(u32::MAX));

    let mut index = 0;

    while index < SECONDS.len() {
        assert!(valid_ticks(SECONDS[index] * 1000));
        index += 1;
    }
};