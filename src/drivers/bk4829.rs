//! BK4829 inhibition over the board's three-wire interface.
//!
//! PF9 selects the chip; PB8 clocks MSB-first data on PB9. Reads release
//! PB9 after the address and sample each returned bit before raising SCK.
//! Every transaction has a fixed number of edges and no acknowledgement wait.
//!
//! The delay contains 64 NOP instructions. It is not a calibrated timer;
//! it provides conservative edge spacing within the admitted MCU clock range.
//! Interrupts are masked for one transaction, not for the complete check.
//!
//! Register 30 disables the RF links, register 36 disables PA control,
//! and register 33 parks the RF-controlled board outputs. Readback checks
//! register retention, not actual PA voltage or absence of RF emission.

pub(crate) mod receive;
pub(crate) mod transmit;
pub(crate) mod sound;
pub(crate) mod meter;
mod subtone;
mod demod;
#[path = "bk4829/rx_ctcss.rs"]
mod rx_signal;

use core::arch::asm;

use crate::mcu;

const REGISTERS: [u8; 5] = [0x30, 0x33, 0x36, 0x37, 0x47];
const EXPECTED: [u16; 5] = [0, 0x9000, 0, 0x9D1F, 0x6042];

#[inline(never)]
fn spacing() {
    unsafe {
        asm!(
            ".rept 64",
            "nop",
            ".endr",
            options(nostack, preserves_flags),
        );
    }
}

fn level(port: usize, pin: u32, high: bool) {
    unsafe {
        mcu::write(port + 0x18, 1 << (pin + if high { 0 } else { 16 }));
    }
}

fn begin() {
    level(mcu::GPIO_F, 9, true);
    level(mcu::GPIO_B, 8, false);
    spacing();
    level(mcu::GPIO_F, 9, false);
    spacing();
}

fn finish() {
    level(mcu::GPIO_F, 9, true);
    spacing();
    level(mcu::GPIO_B, 8, true);
    level(mcu::GPIO_B, 9, true);
}

const fn write_word(register: u8, value: u16) -> u32 {
    ((register as u32) << 16) | value as u32
}

fn send(value: u32, bits: u32) {
    for index in (0..bits).rev() {
        level(mcu::GPIO_B, 9, value & (1 << index) != 0);
        spacing();
        level(mcu::GPIO_B, 8, true);
        spacing();
        level(mcu::GPIO_B, 8, false);
        spacing();
    }
}

fn write(register: u8, value: u16) {
    let _mask = mcu::InterruptMask::acquire();
    begin();
    send(write_word(register, value), 24);
    spacing();
    finish();
}

fn read(register: u8) -> u16 {
    let _mask = mcu::InterruptMask::acquire();
    begin();
    send((register | 0x80) as u32, 8);

    unsafe {
        mcu::modify(mcu::GPIO_B, 3 << 18, 0);
    }
    spacing();

    let mut value = 0u16;
    for _ in 0..16 {
        let bit = unsafe { mcu::read(mcu::GPIO_B + 0x10) >> 9 } & 1;
        value = (value << 1) | bit as u16;
        level(mcu::GPIO_B, 8, true);
        spacing();
        level(mcu::GPIO_B, 8, false);
        spacing();
    }

    // Release the chip before driving the shared data wire again.
    level(mcu::GPIO_F, 9, true);
    spacing();
    unsafe {
        mcu::write(mcu::GPIO_B + 0x18, 1 << 9);
        mcu::modify(mcu::GPIO_B, 3 << 18, 1 << 18);
    }
    finish();
    value
}

const fn mismatch(values: [u16; 5]) -> Option<u32> {
    let mut index = 0;
    while index < values.len() {
        if values[index] != EXPECTED[index] {
            return Some(0x3000 + REGISTERS[index] as u32);
        }
        index += 1;
    }
    None
}

/// Reestablishes the transport pins and issues only disabling writes.
///
/// the caller owns PF9, PB8, and PB9. This can run from the common
/// trap after a foreground transaction was interrupted. It does not read
/// static state, wait for a timer, or assume the previous wire phase.
pub(crate) unsafe fn inhibit() {
    unsafe {
        mcu::enable_gpio();
        mcu::output(mcu::GPIO_F, 9, true);
        mcu::output(mcu::GPIO_B, 8, true);
        mcu::output(mcu::GPIO_B, 9, true);
    }

    write(0x36, 0);
    write(0x30, 0);
    write(0x33, 0x9000);
    write(0x51, 0);
    write(0x3F, 0);
    write(0x70, 0);
    write(0x59, 0);
    write(0x58, 0);
    write(0x50, 0xBB18);
}

/// Leave digital control available with RF links and PA disabled. Nonzero
/// readback fields reject a stuck-low bus that could pass zero-only checks.
/// The caller exclusively owns the RF transport, and the external audio
/// amplifier must be off before this sequence runs.
pub(crate) unsafe fn initialize() -> Result<[u16; 5], u32> {
    unsafe { inhibit() };
    write(0x37, 0x9D1F);
    write(0x47, 0x6042);
    let _ = verify()?;

    // Switch only AF selection, with RF links and AF DAC disabled.
    write(0x47, 0x6142);
    let challenge = read(0x47);
    write(0x47, 0x6042);
    if challenge != 0x6142 {
        return Err(0x3147);
    }

    verify()
}

/// Checks the inhibited register set without rewriting it. Failure is
/// reported to the caller; it is not concealed by a corrective retry.
///
/// The caller must retain exclusive ownership established by initialize.
pub(crate) fn verify() -> Result<[u16; 5], u32> {
    let mut values = [0u16; 5];
    for index in 0..values.len() {
        values[index] = read(REGISTERS[index]);
    }
    match mismatch(values) {
        Some(error) => Err(error),
        None => Ok(values),
    }
}

/// Register 33: RX enable is bit six, VHF LNA bit two, UHF LNA bit three.
/// Both LNA paths, PA enable, and the red LED are excluded.
pub(super) const fn rx_gpio_allowed(value: u16) -> bool {
    matches!(value, 0x9000 | 0x9044 | 0x9048)
}

/// Checks a TX owner's exact control snapshot. The expected state is also
/// validated, so an arbitrary PA or simultaneous RX/TX state cannot be
/// admitted merely by supplying matching expected values.
pub(crate) fn verify_transmit(expected: [u16; 5]) -> Result<[u16; 5], u32> {
    let gpio = expected[1];
    let pa = expected[2];
    let low = pa & 0xFF;
    let bias = pa >> 8;

    let pa_valid = pa == 0
        || (gpio == 0x9025 && low == 0x88 && bias > 0 && bias < 255)
        || (gpio == 0x9029 && low == 0xA2 && bias > 0 && bias < 255);

    let local = expected[0] == 0x0302;
    if (!local && !matches!(expected[0], 0 | 0xC1FE | 0xC1FA))
        || !matches!(gpio, 0x9000 | 0x9025 | 0x9029)
        || !pa_valid || expected[3] != 0x9D1F
        || (local && (gpio != 0x9000 || pa != 0 || expected[4] != 0x6342))
        || (!local && expected[4] != 0x6042)
    {
        return Err(0x7200);
    }

    let mut actual = [0u16; 5];
    for index in 0..5 {
        actual[index] = read(REGISTERS[index]);
        let mask = if index == 0 { 0x7FFF } else { 0xFFFF };
        if actual[index] & mask != expected[index] & mask {
            return Err(0x7200 + REGISTERS[index] as u32);
        }
    }
    Ok(actual)
}

/// Checks the control states admitted by the receive diagnostic.
/// VCO calibration bit 15 may clear without changing the RX links.
/// No PA gain, microphone ADC, TX DSP, PA-enable GPIO, or red LED is admitted.
pub(crate) fn verify_no_tx() -> Result<[u16; 5], u32> {
    let mut values = [0u16; 5];
    for index in 0..values.len() {
        values[index] = read(REGISTERS[index]);
    }

    let controls = [
        values[0] == 0 || values[0] & 0x7FFF == 0x3FF1,
        rx_gpio_allowed(values[1]),
        values[2] == 0,
        matches!(values[3], 0x9D1F | 0x9F1F),
        matches!(values[4], 0x6042 | 0x6142),
    ];
    for index in 0..controls.len() {
        if !controls[index] {
            return Err(0x3200 + REGISTERS[index] as u32);
        }
    }
    Ok(values)
}

const _: () = {
    assert!(0xBFF1u16 & 0x000E == 0);
    assert!(0x9044u16 & 0x0021 == 0);
    assert!(0x9048u16 & 0x0021 == 0);
    assert!(rx_gpio_allowed(0x9000));
    assert!(rx_gpio_allowed(0x9044));
    assert!(rx_gpio_allowed(0x9048));
    assert!(!rx_gpio_allowed(0x904C));
    assert!(!rx_gpio_allowed(0x9064));
    assert!(!rx_gpio_allowed(0x9068));
    assert!(!rx_gpio_allowed(0x9049));
    assert!(write_word(0x36, 0x1234) == 0x361234);
    assert!(write_word(0x30, 0) == 0x300000);
    assert!(mismatch(EXPECTED).is_none());
    assert!(matches!(mismatch([0; 5]), Some(0x3033)));
    assert!(matches!(mismatch([u16::MAX; 5]), Some(0x3030)));

    let mut index = 0;
    while index < 5 {
        let mut changed = EXPECTED;
        changed[index] ^= 1;
        assert!(matches!(mismatch(changed), Some(_)));
        index += 1;
    }
};