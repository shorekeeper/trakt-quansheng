//! RF readback checks and watchdog service for the diagnostic loop.
//!
//! RF is inhibited before the watchdog is configured. An AF selection
//! challenge then checks that the bus can read two distinct written values.
//! The boot check enables neither RX nor TX. A foreground receiver may
//! subsequently request the receive-aware control check; TX stays excluded.
//!
//! A successful foreground pass refreshes IWDG. Once per 1000 observed ticks,
//! the RF register set is read again before that refresh. Any mismatch enters
//! the common trap, which issues disabling writes and stops watchdog service.
//!
//! TRAKT_BENCH contains the most recent register snapshot and refresh count.
//! Its signature does not establish hardware safety; readback and absence of
//! unexpected RF must both be checked during the bench test.



use crate::drivers::bk4829;
use crate::mcu::{self, watchdog::Watchdog};
use crate::startup;

#[repr(C)]
struct BenchState {
    signature: u32,
    tick: u32,
    status: u32,
    reg30: u32,
    reg33: u32,
    reg36: u32,
    reg37: u32,
    reg47: u32,
    reset_flags: u32,
    watchdog_prescaler: u32,
    watchdog_reload: u32,
    watchdog_refreshes: u32,
    inherited_window: u32,
    last_check: u32,
    last_error: u32,
    reserved: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_BENCH: BenchState = unsafe { core::mem::zeroed() };

pub(crate) struct Guard {
    watchdog: Watchdog,
    registers: [u16; 5],
    reset_flags: u32,
    last_check: u32,
    refreshes: u32,
    receiver_owned: bool,
    transmitter_controls: Option<[u16; 5]>,
}

impl Guard {
    /// Requires the board pins to be initialized and interrupts masked.
    /// Only HCLK from 24 to 72 MHz is admitted for this diagnostic transport.
    /// This frequency bound does not replace physical timing measurements.
    pub(crate) fn initialize(hclk: u32) -> Self {
        if !(24_000_000..=72_000_000).contains(&hclk) {
            startup::stop(0x4004);
        }

        let reset_flags = unsafe { mcu::read(mcu::RCC + 0x60) };
        let registers = match unsafe { bk4829::initialize() } {
            Ok(registers) => registers,
            Err(error) => startup::stop(error),
        };
        let watchdog = match unsafe { Watchdog::initialize() } {
            Ok(watchdog) => watchdog,
            Err(error) => startup::stop(error),
        };
        let guard = Self {
            watchdog,
            registers,
            reset_flags,
            last_check: 0,
            refreshes: 0,
            receiver_owned: false,
            transmitter_controls: None,
        };
        guard.publish(0);
        guard
    }

    fn publish(&self, now: u32) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let state = BenchState {
            signature: 0,
            tick: now,
            status: if self.transmitter_controls.is_some() {
                3
            } else if self.receiver_owned {
                2
            } else {
                1
            },
            reg30: self.registers[0] as u32,
            reg33: self.registers[1] as u32,
            reg36: self.registers[2] as u32,
            reg37: self.registers[3] as u32,
            reg47: self.registers[4] as u32,
            reset_flags: self.reset_flags,
            watchdog_prescaler: 64,
            watchdog_reload: 1023,
            watchdog_refreshes: self.refreshes,
            inherited_window: self.watchdog.inherited_window,
            last_check: self.last_check,
            last_error: 0,
            reserved: 0,
        };
        crate::observation::publish!(TRAKT_BENCH, state, 0x5442_4354);
    }

    /// Selects only between inhibited and receive-only control checks.
    /// The receiver performs its own frequency and profile readback.
    pub(crate) fn receiver_owned(&mut self, owned: bool) {
        self.receiver_owned = owned;
    }

    pub(crate) fn transmitter_controls(&mut self, expected: Option<[u16; 5]>) {
        self.transmitter_controls = expected;
    }

    /// Call after foreground work, never from SysTick or another interrupt.
    pub(crate) fn service(&mut self, now: u32) {
        if now.wrapping_sub(self.last_check) >= 1_000 {
            let result = if let Some(expected) = self.transmitter_controls {
                bk4829::verify_transmit(expected)
            } else if self.receiver_owned {
                bk4829::verify_no_tx()
            } else {
                bk4829::verify()
            };
            self.registers = match result {
                Ok(registers) => registers,
                Err(error) => {
                    #[cfg(feature = "debug-tools")]
                    unsafe {
                        core::ptr::write_volatile(
                            core::ptr::addr_of_mut!(TRAKT_BENCH.status), 0,
                        );
                        core::ptr::write_volatile(
                            core::ptr::addr_of_mut!(TRAKT_BENCH.last_error), error,
                        );
                    }
                    startup::stop(error)
                }
            };
            self.last_check = now;
            self.publish(now);
        }
        self.watchdog.refresh();
        self.refreshes = self.refreshes.saturating_add(1);
    }
}

const _: () = {
    assert!(core::mem::size_of::<BenchState>() == 64);
};