//! Millisecond counter driven by SysTick at decoded nominal HCLK.
//!
//! The handler checks its sampled stack position, increments one aligned word,
//! and enforces the armed TX cutoff. Foreground snapshots preserve and restore
//! PRIMASK around the volatile counter read. No shared Rust reference to the
//! counter is constructed.
//!
//! SysTick has one pending flag. Interrupt masking across several timer periods
//! loses elapsed increments. This counter therefore does not measure time spent
//! with interrupts disabled and is not an independent foreground-stall detector.
//!
//! wait closes the check-to-sleep window under PRIMASK, waits for an exception,
//! then restores the mask. A stopped counter provides no local timeout;
//! foreground watchdog service stops when the wait cannot make progress.

use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use crate::mcu;

const SYSTICK: usize = 0xE000_E010;

#[used]
#[no_mangle]
static mut TRAKT_TICKS: u32 = 0;

/// Exact nominal cycles per millisecond are required. No fractional reload
/// correction is performed.
pub(crate) const fn reload(hclk: u32) -> Option<u32> {
    if hclk < 1_000 || hclk % 1_000 != 0 {
        return None;
    }
    let cycles = hclk / 1_000;
    if cycles > 0x0100_0000 {
        None
    } else {
        Some(cycles - 1)
    }
}

/// Starts SysTick with its exception still masked by PRIMASK.
///
/// startup must have initialized RAM and installed SysTick_Handler.
/// The caller must own SysTick with interrupts masked.
pub(crate) unsafe fn initialize(hclk: u32) -> Option<u32> {
    let load = reload(hclk)?;
    unsafe {
        mcu::write(SYSTICK, 0);
        write_volatile(addr_of_mut!(TRAKT_TICKS), 0);
        mcu::write(0xE000_ED04, 1 << 25);
        mcu::modify(0xE000_ED20, 0xFF00_0000, 0xC000_0000);
        mcu::write(SYSTICK + 4, load);
        mcu::write(SYSTICK + 8, 0);
        mcu::write(SYSTICK, 7);
        asm!("dsb", "isb", options(nostack, preserves_flags));
    }
    Some(load)
}

#[no_mangle]
pub extern "C" fn SysTick_Handler() {
    crate::debug::interrupt_stack_check();

    unsafe {
        let pointer = addr_of_mut!(TRAKT_TICKS);
        let value = read_volatile(pointer).wrapping_add(1);
        write_volatile(pointer, value);
        mcu::tx_limit::enforce(value);
    }
}

/// Returns one foreground snapshot with the handler excluded during the
/// read. The prior interrupt mask is restored before returning.
pub(crate) fn snapshot() -> u32 {
    let _mask = mcu::InterruptMask::acquire();
    unsafe { read_volatile(addr_of!(TRAKT_TICKS)) }
}

/// Waits for a counter change without leaving a check-to-sleep window.
/// PRIMASK prevents handler execution during the check. A pending exception
/// wakes WFI, and restoring the mask lets its handler update the counter.
///
/// Call only from foreground code with interrupts enabled. The counter
/// must already be running; no independent timeout exists if it stops.
pub(crate) fn wait(previous: u32) -> u32 {
    loop {
        let mask = mcu::InterruptMask::acquire();
        let current = unsafe { read_volatile(addr_of!(TRAKT_TICKS)) };
        if current != previous {
            drop(mask);
            return current;
        }
        unsafe {
            asm!("dsb", "wfi", options(nostack, preserves_flags));
        }
        drop(mask);
    }
}

const _: () = {
    assert!(matches!(reload(48_000_000), Some(47_999)));
    assert!(matches!(reload(72_000_000), Some(71_999)));
    assert!(matches!(reload(1_000), Some(0)));
    assert!(reload(999).is_none());
    assert!(reload(1_001).is_none());
    assert!(reload(0xFFFF_FFFF).is_none());
};