//! Register access used during board initialization and exception handling.
//!
//! Configuration updates run with interrupts masked. They are ordinary
//! read-modify-write operations, not atomic register transactions.
//! Output levels use BSRR, so unrelated output latch bits are preserved.

pub(crate) mod clock;
pub(crate) mod spi;
pub(crate) mod tick;
pub(crate) mod watchdog;
pub(crate) mod tx_limit;

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};

pub(crate) const RCC: usize = 0x4002_1000;
pub(crate) const GPIO_A: usize = 0x5000_0000;
pub(crate) const GPIO_B: usize = 0x5000_0400;
pub(crate) const GPIO_C: usize = 0x5000_0800;
pub(crate) const GPIO_F: usize = 0x5000_1400;

/// Reads a 32-bit peripheral register.
///
/// the address must identify an aligned, readable register whose
/// clock and access conditions are satisfied. Read side effects belong
/// to the caller's register-specific handling.
#[inline(always)]
pub(crate) unsafe fn read(address: usize) -> u32 {
    unsafe { read_volatile(address as *const u32) }
}

/// Writes a 32-bit peripheral register.
///
/// the address must identify an aligned, writable register.
/// The value must respect reserved bits and register-specific write rules.
#[inline(always)]
pub(crate) unsafe fn write(address: usize, value: u32) {
    unsafe { write_volatile(address as *mut u32, value) }
}

/// Updates selected bits in a normal read/write configuration register.
///
/// read and write must both be valid, and no other execution
/// context may change the register between them. Do not use this on
/// write-one-to-clear registers or registers with destructive reads.
#[inline(always)]
pub(crate) unsafe fn modify(address: usize, mask: u32, value: u32) {
    unsafe {
        write(address, (read(address) & !mask) | (value & mask));
    }
}

/// Enables the four GPIO ports used by the radio.
///
/// the caller owns RCC configuration while this update runs.
pub(crate) unsafe fn enable_gpio() {
    unsafe {
        modify(RCC + 0x34, 0x27, 0x27);
        let _ = read(RCC + 0x34);
        asm!("dsb", options(nostack, preserves_flags));
    }
}

/// Establishes the latch before selecting push-pull output mode.
///
/// `port` must be one of this chip's enabled GPIO ports and
/// `pin` must be below 16. The caller owns configuration of that pin.
/// Low slew rate is sufficient for parked signals; drivers choose their
/// operating speed when the corresponding bus is initialized.
pub(crate) unsafe fn output(port: usize, pin: u32, high: bool) {
    let bit = 1u32 << pin;
    let shift = pin * 2;
    unsafe {
        write(port + 0x18, if high { bit } else { bit << 16 });
        modify(port + 4, bit, 0);
        modify(port + 8, 3 << shift, 0);
        modify(port + 12, 3 << shift, 0);
        modify(port, 3 << shift, 1 << shift);
    }
}

/// Selects input mode with a pull-up.
///
/// the port must be enabled, the pin must be below 16, and the
/// caller must own its configuration.
pub(crate) unsafe fn input_pull_up(port: usize, pin: u32) {
    let shift = pin * 2;
    unsafe {
        modify(port, 3 << shift, 0);
        modify(port + 12, 3 << shift, 1 << shift);
    }
}

/// Checks register configuration and the output latch, not the voltage
/// measured at the pin.
///
/// the port must be enabled and the pin must be below 16.
pub(crate) unsafe fn output_matches(port: usize, pin: u32, high: bool) -> bool {
    let bit = 1u32 << pin;
    let shift = pin * 2;
    unsafe {
        read(port) & (3 << shift) == 1 << shift
            && read(port + 4) & bit == 0
            && read(port + 12) & (3 << shift) == 0
            && (read(port + 0x14) & bit != 0) == high
    }
}

pub(crate) fn control() -> u32 {
    let value: u32;
    unsafe {
        asm!("mrs {}, CONTROL", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub(crate) fn interrupt_mask() -> u32 {
    let value: u32;
    unsafe {
        asm!("mrs {}, PRIMASK", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub(crate) fn stack_pointer() -> u32 {
    let value: u32;
    unsafe {
        asm!("mrs {}, MSP", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Restores the interrupt mask that was present when the guard was created.
/// Nested guards therefore leave the outer mask intact.
pub(crate) struct InterruptMask(u32);

impl InterruptMask {
    pub(crate) fn acquire() -> Self {
        let previous: u32;
        unsafe {
            asm!(
                "mrs {}, PRIMASK",
                "cpsid i",
                out(reg) previous,
                options(nostack, preserves_flags),
            );
        }
        Self(previous)
    }
}

impl Drop for InterruptMask {
    fn drop(&mut self) {
        unsafe {
            asm!(
                "msr PRIMASK, {}",
                "isb",
                in(reg) self.0,
                options(nostack, preserves_flags),
            );
        }
    }
}

/// Allows maskable exceptions after their state and vector entries exist.
///
/// every enabled interrupt must have a valid handler and initialized
/// state. This does not change the NVIC enable registers.
pub(crate) unsafe fn enable_interrupts() {
    unsafe {
        asm!("cpsie i", "isb", options(nostack, preserves_flags));
    }
}

/// Sleeps without changing interrupt admission. A pending exception may
/// wake WFI; the loop does not assume that each wake represents useful work.
pub(crate) fn park() -> ! {
    loop {
        unsafe {
            asm!("dsb", "wfi", options(nostack, preserves_flags));
        }
    }
}