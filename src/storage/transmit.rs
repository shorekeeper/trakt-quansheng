//! Factory PA-code interpolation for Low, Medium, High, and MAX.
//!
//! VHF calibration occupies nine bytes at 0F0; UHF occupies nine at 120.
//! Each region contains three frequency anchors for Low, Medium, and High.
//! The selected family determines which rows must pass validation.
//!
//! Low1..Low5 scale the factory Low anchors using band-specific divisors.
//! Medium1..Medium5 divide the Low-to-Medium interval into five code steps,
//! ending at factory Medium. High1..High3 use quarters one through three of
//! the Medium-to-High interval. MAX uses the factory High row directly.
//!
//! Zero and FF are invalid calibrated PA codes. A subdivided interval rejects
//! descending endpoint rows rather than reordering them. An unrelated row
//! does not invalidate a family that does not use it.
//!
//! Interpolation converts frequency distances to ten-hertz units before
//! multiplying the signed code difference, keeping the arithmetic inside i32.
//! The resulting code is checked before constructing the fixed board PA/GPIO
//! fields. Codes and subdivisions are not measured watts or equal RF steps.
//!
//! Battery limits come from the checked ordered calibration entries. The
//! profile also carries crystal correction and the band-specific PA fields.
//! Decoding performs no RF writes and does not grant transmit authorization.

use super::calibration::Summary;
use crate::app::tx_options::POWER_COUNT;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Profile {
    pub(crate) bias: u16,
    pub(crate) pa: u16,
    pub(crate) gpio: u16,
    pub(crate) crystal: u16,
    pub(crate) battery_min: u32,
    pub(crate) battery_max: u32,
}

impl Profile {
    pub(crate) const fn empty() -> Self {
        Self {
            bias: 0,
            pa: 0,
            gpio: 0,
            crystal: 0,
            battery_min: 0,
            battery_max: 0,
        }
    }
}

const fn calibrated(value: u16) -> bool {
    value > 0 && value < 255
}

const fn anchor(
    block: &[u8; 512],
    base: usize,
    index: usize,
    power: u8,
    vhf: bool,
) -> Result<u16, u32> {
    let low = block[base + index] as u16;

    if power < 5 {
        if !calibrated(low) {
            return Err(0x7304);
        }

        let divisors = if vhf {
            [20, 15, 10, 8, 6]
        } else {
            [25, 19, 13, 9, 6]
        };

        let reduced = low * 4 / divisors[power as usize];

        return if calibrated(reduced) {
            Ok(reduced)
        } else {
            Err(0x7304)
        };
    }

    let medium = block[base + 3 + index] as u16;

    if power <= 9 {
        if !calibrated(low) || !calibrated(medium) || medium < low {
            return Err(0x7304);
        }

        let subdivision = power as u16 - 4;
        return Ok(low + (medium - low) * subdivision / 5);
    }

    let high = block[base + 6 + index] as u16;

    if !calibrated(high) {
        return Err(0x7304);
    }

    if power == 13 {
        return Ok(high);
    }

    if !calibrated(medium) || high < medium {
        return Err(0x7304);
    }

    let subdivision = power as u16 - 9;
    Ok(medium + (high - medium) * subdivision / 4)
}

const fn interpolate(
    frequency: u32,
    lower: u32,
    middle: u32,
    upper: u32,
    values: [u16; 3],
) -> u16 {
    if frequency <= lower {
        return values[0];
    }

    if frequency >= upper {
        return values[2];
    }

    let (left, right, start, end) = if frequency <= middle {
        (values[0], values[1], lower, middle)
    } else {
        (values[1], values[2], middle, upper)
    };

    let delta = right as i32 - left as i32;
    let distance = ((frequency - start) / 10) as i32;
    let span = ((end - start) / 10) as i32;

    (left as i32 + delta * distance / span) as u16
}

pub(crate) const fn decode(
    block: &[u8; 512],
    summary: Summary,
    frequency: u32,
    power: u8,
) -> Result<Profile, u32> {
    let vhf = frequency >= 136_000_000 && frequency <= 174_000_000;
    let uhf = frequency >= 400_000_000 && frequency <= 470_000_000;

    if (!vhf && !uhf) || frequency % 10 != 0 {
        return Err(0x7302);
    }

    if power >= POWER_COUNT {
        return Err(0x7604);
    }

    if summary.flags != 7 {
        return Err(0x7301);
    }

    let mut index = 0;

    while index < summary.battery.len() {
        if summary.battery[index] == 0
            || summary.battery[index] > 4095
            || (index != 0
                && summary.battery[index] <= summary.battery[index - 1])
        {
            return Err(0x7303);
        }

        index += 1;
    }

    let base = if vhf { 0xF0 } else { 0x120 };
    let mut values = [0u16; 3];

    index = 0;

    while index < values.len() {
        values[index] = match anchor(block, base, index, power, vhf) {
            Ok(value) => value,
            Err(error) => return Err(error),
        };

        index += 1;
    }

    let bias = if vhf {
        interpolate(
            frequency,
            137_000_000,
            155_500_000,
            174_000_000,
            values,
        )
    } else {
        interpolate(
            frequency,
            400_000_000,
            435_000_000,
            470_000_000,
            values,
        )
    };

    if !calibrated(bias) {
        return Err(0x7304);
    }

    let maximum = summary.battery[5] as u32 * 110 / 100;

    Ok(Profile {
        bias,
        pa: (bias << 8) | if vhf { 0x88 } else { 0xA2 },
        gpio: if vhf { 0x9025 } else { 0x9029 },
        crystal: (22_656i32 + summary.crystal as i32) as u16,
        battery_min: summary.battery[1] as u32,
        battery_max: if maximum > 4095 { 4095 } else { maximum },
    })
}

const _: () = {
    let mut block = [0u8; 512];
    let mut index = 0;

    while index < 3 {
        block[0xF0 + index] = 75;
        block[0xF3 + index] = 120;
        block[0xF6 + index] = 150;

        block[0x120 + index] = 75;
        block[0x123 + index] = 120;
        block[0x126 + index] = 150;

        index += 1;
    }

    let summary = Summary {
        crc32: 0,
        flags: 7,
        battery: [1436, 1993, 2141, 2170, 2289, 2600],
        crystal: -2,
        filter_low: 70,
        filter_high: 80,
        volume: 55,
        dac: 14,
    };

    let expected_vhf = [
        15, 20, 30, 37, 50,
        84, 93, 102, 111, 120,
        127, 135, 142, 150,
    ];

    let expected_uhf = [
        12, 15, 23, 33, 50,
        84, 93, 102, 111, 120,
        127, 135, 142, 150,
    ];

    let mut power = 0;

    while power < POWER_COUNT {
        let vhf = match decode(&block, summary, 145_500_000, power) {
            Ok(profile) => profile,
            Err(_) => panic!("VHF power profile rejected"),
        };

        let uhf = match decode(&block, summary, 433_500_000, power) {
            Ok(profile) => profile,
            Err(_) => panic!("UHF power profile rejected"),
        };

        assert!(vhf.bias == expected_vhf[power as usize]);
        assert!(uhf.bias == expected_uhf[power as usize]);
        assert!(vhf.gpio == 0x9025 && uhf.gpio == 0x9029);
        assert!(vhf.battery_min == 1993 && vhf.battery_max == 2860);
        assert!(vhf.crystal == 22654);

        power += 1;
    }

    assert!(matches!(
        decode(&block, summary, 433_500_000, 14),
        Err(0x7604),
    ));

    block[0xF6] = 255;
    assert!(matches!(
        decode(&block, summary, 145_500_000, 13),
        Err(0x7304),
    ));

    // An unrelated High row does not invalidate the existing Low profile.
    assert!(decode(&block, summary, 145_500_000, 0).is_ok());

    block[0xF3] = 50;
    assert!(matches!(
        decode(&block, summary, 145_500_000, 5),
        Err(0x7304),
    ));

    assert!(interpolate(
        417_500_000,
        400_000_000,
        435_000_000,
        470_000_000,
        [254, 1, 254],
    ) == 128);

    assert!(interpolate(
        452_500_000,
        400_000_000,
        435_000_000,
        470_000_000,
        [1, 254, 1],
    ) == 128);
};