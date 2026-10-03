//! Battery voltage estimate from the checked factory calibration.
//!
//! Calibration entry three is the reference ADC code for 7.60 V.
//! Conversion uses rounded integer millivolts with no fallback coefficient.
//! All six entries must be nonzero, within the ADC range, and increasing.
//!
//! This is a scale conversion, not an independent voltage measurement.
//! It does not infer chemistry, remaining capacity, or charging state.
//! TX admission continues to use its existing raw ADC limits.

use crate::storage::calibration::Summary;

pub(super) const fn reference(summary: Option<Summary>) -> Option<u32> {
    let summary = match summary {
        Some(summary) if summary.flags & 3 == 3 => summary,
        _ => return None,
    };

    let mut index = 0;
    while index < summary.battery.len() {
        let value = summary.battery[index];
        if value == 0 || value > 4095 {
            return None;
        }
        if index != 0 && value <= summary.battery[index - 1] {
            return None;
        }
        index += 1;
    }

    Some(summary.battery[3] as u32)
}

pub(super) const fn convert(raw: u32, reference: u32) -> Option<u32> {
    if raw > 4095 || reference == 0 || reference > 4095 {
        None
    } else {
        Some((raw * 7600 + reference / 2) / reference)
    }
}

pub(super) const fn millivolts(
    raw: Option<u32>,
    summary: Option<Summary>,
) -> Option<u32> {
    let raw = match raw {
        Some(raw) => raw,
        None => return None,
    };
    let reference = match reference(summary) {
        Some(reference) => reference,
        None => return None,
    };
    convert(raw, reference)
}

const _: () = {
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

    assert!(matches!(reference(Some(summary)), Some(2170)));
    assert!(matches!(millivolts(Some(2170), Some(summary)), Some(7600)));
    assert!(matches!(millivolts(Some(2278), Some(summary)), Some(7978)));
    assert!(matches!(millivolts(Some(2280), Some(summary)), Some(7985)));
    assert!(matches!(millivolts(Some(0), Some(summary)), Some(0)));
    assert!(millivolts(None, Some(summary)).is_none());
    assert!(millivolts(Some(2278), None).is_none());
    assert!(convert(4096, 2170).is_none());
    assert!(convert(2278, 0).is_none());
    assert!(convert(2278, 4096).is_none());

    let mut invalid = summary;
    invalid.battery[3] = 0;
    assert!(reference(Some(invalid)).is_none());
    invalid = summary;
    invalid.battery[4] = invalid.battery[3];
    assert!(reference(Some(invalid)).is_none());
    invalid = summary;
    invalid.battery[5] = 4096;
    assert!(reference(Some(invalid)).is_none());
    invalid = summary;
    invalid.flags = 1;
    assert!(reference(Some(invalid)).is_none());
};