//! Nominal HCLK and PCLK from the loader's RCC configuration.
//!
//! HSI and HSI-fed PLL are accepted. External and low-speed sources are
//! rejected because their board frequency is not established here.
//! Reserved HSI and PLL encodings are errors, not fallback frequencies.
//!
//! PLL uses HSI before HSIDIV. AHB division applies after the selected
//! system source; APB division then produces the USART peripheral clock.
//! No oscillator, calibration, divider, or Flash timing register is changed.

use crate::mcu;

#[derive(Clone, Copy)]
pub(crate) struct Clocks {
    pub(crate) hclk: u32,
    pub(crate) pclk: u32,
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub(crate) enum Error {
    Switching = 1,
    Source = 2,
    HsiRange = 3,
    NotReady = 4,
    PllSource = 5,
    PllFactor = 6,
    Fractional = 7,
    Frequency = 8,
}

pub(crate) const fn decode(
    control: u32,
    internal: u32,
    config: u32,
    pll: u32,
) -> Result<Clocks, Error> {
    let source = (config >> 3) & 7;
    if config & 7 != source {
        return Err(Error::Switching);
    }
    if source != 0 && source != 2 {
        return Err(Error::Source);
    }

    let hsi = match (internal >> 13) & 7 {
        0 => 4_000_000u32,
        1 => 8_000_000,
        2 => 16_000_000,
        3 => 22_120_000,
        4 => 24_000_000,
        _ => return Err(Error::HsiRange),
    };
    if control & 0x500 != 0x500 {
        return Err(Error::NotReady);
    }

    let system = if source == 0 {
        hsi >> ((control >> 11) & 7)
    } else {
        if control & 0x0300_0000 != 0x0300_0000 {
            return Err(Error::NotReady);
        }
        if pll & 3 != 2 {
            return Err(Error::PllSource);
        }
        let factor = match (pll >> 2) & 3 {
            0 => 2,
            1 => 3,
            _ => return Err(Error::PllFactor),
        };
        hsi * factor
    };

    if system == 0 || system > 72_000_000 {
        return Err(Error::Frequency);
    }

    let ahb_shift = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 6, 7, 8, 9]
        [((config >> 8) & 15) as usize];
    let apb_shift = [0, 0, 0, 0, 1, 2, 3, 4]
        [((config >> 12) & 7) as usize];

    if system % (1 << ahb_shift) != 0 {
        return Err(Error::Fractional);
    }
    let hclk = system >> ahb_shift;
    if hclk % (1 << apb_shift) != 0 {
        return Err(Error::Fractional);
    }

    Ok(Clocks {
        hclk,
        pclk: hclk >> apb_shift,
    })
}

/// Reads the active clock configuration. The application owns RCC and
/// must not change its clock fields while these reads are in progress.
pub(crate) fn inspect() -> Result<Clocks, Error> {
    unsafe {
        decode(
            mcu::read(mcu::RCC),
            mcu::read(mcu::RCC + 4),
            mcu::read(mcu::RCC + 8),
            mcu::read(mcu::RCC + 12),
        )
    }
}

const fn check(
    control: u32,
    internal: u32,
    config: u32,
    pll: u32,
    hclk: u32,
    pclk: u32,
) {
    match decode(control, internal, config, pll) {
        Ok(actual) => {
            assert!(actual.hclk == hclk);
            assert!(actual.pclk == pclk);
        }
        Err(_) => panic!("clock test rejected a valid configuration"),
    }
}

const _: () = {
    check(0x500, 1 << 13, 0, 0, 8_000_000, 8_000_000);
    check(0xD00, 1 << 13, 0, 0, 4_000_000, 4_000_000);
    check(0x0300_0500, 4 << 13, 0x12, 2, 48_000_000, 48_000_000);
    check(0x0300_0500, 2 << 13, 0x12, 6, 48_000_000, 48_000_000);
    check(0x0300_3D00, 4 << 13, 0x12, 6, 72_000_000, 72_000_000);
    check(0x0300_0500, 4 << 13, 0x4912, 2, 12_000_000, 6_000_000);

    assert!(matches!(decode(0x500, 0, 2, 0), Err(Error::Switching)));
    assert!(matches!(decode(0x500, 0, 9, 0), Err(Error::Source)));
    assert!(matches!(decode(0x500, 5 << 13, 0, 0), Err(Error::HsiRange)));
    assert!(matches!(decode(0x100, 0, 0, 0), Err(Error::NotReady)));
    assert!(matches!(decode(0x500, 0, 0x12, 2), Err(Error::NotReady)));
    assert!(matches!(
        decode(0x0300_0500, 0, 0x12, 3),
        Err(Error::PllSource)
    ));
    assert!(matches!(
        decode(0x0300_0500, 0, 0x12, 10),
        Err(Error::PllFactor)
    ));
    assert!(matches!(
        decode(0x500, 3 << 13, 15 << 8, 0),
        Err(Error::Fractional)
    ));
};