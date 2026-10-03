//! SysTick backstop following the foreground TX timeout.
//!
//! Foreground receives one checked timeout from the saved menu selection.
//! The backstop uses the same duration plus 1000 observed ticks. Both include
//! setup, local permit audio, speech, and the optional Roger tail.
//!
//! A backstop violation enters the common trap and never resumes interrupted
//! TX work. The guard is cleared only after RF-off readback succeeds.
//! IWDG remains independent of SysTick and foreground service.
//!
//! This is not a thermal sensor or a measured RF-duration guarantee.
//! SysTick periods lost while interrupts are masked remain unobserved.

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use crate::{mcu, startup};
use crate::app::{time::reached, tx_options};

#[repr(C)]
struct Limit {
    enabled: u32,
    deadline: u32,
    trips: u32,
    reserved: u32,
}

#[used]
#[no_mangle]
static mut TRAKT_TX_LIMIT: Limit = unsafe { core::mem::zeroed() };

const fn deadline(now: u32, budget: u32) -> Option<u32> {
    if !tx_options::valid_ticks(budget) {
        None
    } else {
        Some(now.wrapping_add(budget + 1000))
    }
}

/// Arm before any register can admit TX. The duration is fixed for the
/// attempt; this function is not called again to refresh its deadline.
pub(crate) fn arm(now: u32, budget: u32) {
    let deadline = match deadline(now, budget) {
        Some(deadline) => deadline,
        None => startup::stop(0x700A),
    };

    let _mask = mcu::InterruptMask::acquire();

    unsafe {
        write_volatile(addr_of_mut!(TRAKT_TX_LIMIT.deadline), deadline);
        write_volatile(addr_of_mut!(TRAKT_TX_LIMIT.enabled), 1);
    }
}
/// Call only after PA and RF links were disabled and verified.
pub(crate) fn clear() {
    let _mask = mcu::InterruptMask::acquire();

    unsafe {
        write_volatile(addr_of_mut!(TRAKT_TX_LIMIT.enabled), 0);
    }
}

pub(crate) fn enforce(now: u32) {
    let enabled = unsafe {
        read_volatile(addr_of!(TRAKT_TX_LIMIT.enabled))
    };

    if enabled == 0 {
        return;
    }

    let deadline = unsafe {
        read_volatile(addr_of!(TRAKT_TX_LIMIT.deadline))
    };

    if reached(now, deadline) {
        unsafe {
            let trips = read_volatile(addr_of!(TRAKT_TX_LIMIT.trips));
            write_volatile(
                addr_of_mut!(TRAKT_TX_LIMIT.trips),
                trips.saturating_add(1),
            );
        }

        startup::stop(0x7100);
    }
}

const _: () = {
    assert!(core::mem::size_of::<Limit>() == 16);
    assert!(matches!(deadline(0, 30000), Some(31000)));
    assert!(matches!(deadline(100, 180000), Some(181100)));
    assert!(deadline(0, 0).is_none());
    assert!(deadline(0, 5000).is_none());
    assert!(deadline(0, 180001).is_none());

    let wrapped = match deadline(u32::MAX - 100, 15000) {
        Some(value) => value,
        None => panic!("valid wrapped TX deadline rejected"),
    };

    assert!(!reached(wrapped.wrapping_sub(1), wrapped));
    assert!(reached(wrapped, wrapped));
};