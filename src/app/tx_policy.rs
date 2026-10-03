//! Technical TX envelope, separate from receiver tuning and calibration.
//!
//! HAM mode admits occupied frequency margins within 144..146 MHz and
//! 433..440 MHz. The omitted 430..433 MHz segment is a conservative choice
//! because the firmware has no location input for its regional restriction.
//! These intervals do not validate operator credentials, service priority,
//! mode-specific allocations, station identification, or measured emissions.
//!
//! LPD and PMR grids are classified for diagnostics, not enabled as a
//! license-exempt service. LPD frequencies overlap the amateur allocation:
//! passing as amateur operation does not prove 10 mW LPD compliance.
//! PMR metadata uses the conservative eight-channel 446.0..446.1 MHz grid.
//! No PA code is treated as proof of an ERP limit or equipment conformity.
//!
//! Disabling HAM removes this allocation filter only. Frequency remains
//! inside the existing VHF/UHF hardware envelope, with an edge margin.
//! FM margins are conservative tuning guards, not measured occupied bandwidth.
//! RX never consults this module.

const HAM_RANGES: [(u32, u32); 2] = [
    (144_000_000, 146_000_000),
    (433_000_000, 440_000_000),
];

const HARDWARE_RANGES: [(u32, u32); 2] = [
    (136_000_000, 174_000_000),
    (400_000_000, 470_000_000),
];

const LPD_FIRST: u32 = 433_075_000;
const LPD_LAST: u32 = 434_775_000;
const LPD_STEP: u32 = 25_000;

const PMR_FIRST: u32 = 446_006_250;
const PMR_LAST: u32 = 446_093_750;
const PMR_STEP: u32 = 12_500;

const fn contains(frequency: u32, margin: u32, ranges: &[(u32, u32)]) -> bool {
    let mut index = 0;
    while index < ranges.len() {
        let (lower, upper) = ranges[index];
        if frequency >= lower + margin && frequency <= upper - margin {
            return true;
        }
        index += 1;
    }
    false
}

const fn grid(frequency: u32, first: u32, last: u32, step: u32) -> bool {
    frequency >= first && frequency <= last
        && (frequency - first) % step == 0
}

pub(crate) const fn service(frequency: u32) -> &'static [u8] {
    if grid(frequency, PMR_FIRST, PMR_LAST, PMR_STEP) {
        b"PMR GRID / NOT CERTIFIED"
    } else if grid(frequency, LPD_FIRST, LPD_LAST, LPD_STEP) {
        b"LPD GRID / POWER UNMEASURED"
    } else {
        b"VERIFY SERVICE AUTHORIZATION"
    }
}

pub(crate) const fn check(
    frequency: u32,
    narrow: bool,
    ham: bool,
    power: u8,
) -> Result<(), u32> {
    if power >= crate::app::tx_options::POWER_COUNT {
        return Err(0x7604);
    }
    if frequency % 10 != 0 {
        return Err(0x7602);
    }

    let margin = if narrow { 6_250 } else { 12_500 };
    if !contains(frequency, margin, &HARDWARE_RANGES) {
        return Err(0x7602);
    }
    if ham && !contains(frequency, margin, &HAM_RANGES) {
        return Err(0x7601);
    }
    Ok(())
}

pub(crate) const fn power_name(power: u8) -> &'static [u8] {
    crate::app::tx_options::power_name(power)
}

pub(crate) const fn reason(error: u32) -> &'static [u8] {
    match error {
        0x7601 => b"HAM FILTER / RX UNAFFECTED",
        0x7602 => b"OUTSIDE HARDWARE TX ENVELOPE",
        0x7604 => b"INVALID POWER PROFILE",
        0x7001 => b"CHECK RX / MONITOR / SCAN",
        0x7003 => b"WAIT FOR RECEIVER SETTLING",
        0x7004 => b"BATTERY DATA OR LIMIT",
        0x7005 => b"BUSY CHANNEL",
        0x7006 => b"TX TIME LIMIT",
        0x7009 => b"RELEASE PTT BEFORE RETRY",
        0x700A => b"INVALID TX TIME LIMIT",
        0x700B => b"TX ROUTE OUT OF BAND",
        0x700C => b"RX ONLY PROFILE",
        0x700D => b"INVALID TX SIGNAL",
        0x700E => b"SELECTED RX CHECK TIMEOUT",
        0x700F => b"AM PROFILE / RECEIVE ONLY",
        0x7010 => b"INVALID SLOPECAST SETTINGS",
        0x7011 => b"SC-FM REQUIRES SIMPLEX",
        0x7012 => b"SC-FM SHIFT OUT OF TX BAND",
        0x7013 => b"SC-FM REQUEST MISMATCH",
        _ => b"TX CHECK FAILED",
    }
}

const _: () = {
    assert!(check(145_500_000, false, true, 0).is_ok());
    assert!(check(433_500_000, false, true, 4).is_ok());
    assert!(check(144_006_250, true, true, 0).is_ok());
    assert!(matches!(
        check(144_006_250, false, true, 0), Err(0x7601),
    ));
    assert!(matches!(check(146_000_000, true, true, 0), Err(0x7601)));
    assert!(matches!(check(432_500_000, true, true, 0), Err(0x7601)));
    assert!(matches!(check(446_006_250, true, true, 0), Err(0x7601)));
    assert!(matches!(check(150_000_000, true, true, 0), Err(0x7601)));

    assert!(check(150_000_000, true, false, 0).is_ok());
    assert!(check(446_006_250, true, false, 0).is_ok());
    assert!(matches!(check(280_000_000, true, false, 0), Err(0x7602)));
    assert!(matches!(check(136_000_000, true, false, 0), Err(0x7602)));
    assert!(matches!(check(470_000_000, true, false, 0), Err(0x7602)));
    assert!(matches!(check(433_500_001, true, false, 0), Err(0x7602)));
    assert!(check(433_500_000, true, false, 13).is_ok());
    assert!(matches!(check(433_500_000, true, false, 14), Err(0x7604)));

    assert!(grid(433_075_000, LPD_FIRST, LPD_LAST, LPD_STEP));
    assert!(grid(434_775_000, LPD_FIRST, LPD_LAST, LPD_STEP));
    assert!(grid(433_500_000, LPD_FIRST, LPD_LAST, LPD_STEP));
    assert!(!grid(433_512_500, LPD_FIRST, LPD_LAST, LPD_STEP));
    assert!(grid(446_006_250, PMR_FIRST, PMR_LAST, PMR_STEP));
    assert!(grid(446_093_750, PMR_FIRST, PMR_LAST, PMR_STEP));
    assert!(!grid(446_100_000, PMR_FIRST, PMR_LAST, PMR_STEP));
};