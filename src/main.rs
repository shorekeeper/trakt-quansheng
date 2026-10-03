#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(warnings)]

//! UV-K5 V3 application entry on PY32F071.
//!
//! Startup memory checks and board GPIO initialization precede clock decoding,
//! RF inhibition/readback, watchdog setup, UART endpoint selection, and SysTick.
//! The application enables interrupts only after its foreground owners exist.
//!
//! Default RX begins after boot acquisition. TX uses selected-profile policy,
//! calibration, battery, carrier, release-history, and deadline checks.
//! Saved HAM and DW choices are application settings, not retained live permits.
//!
//! Settings writes use the journal's two admitted sectors. Explicit Save and
//! eligible Auto scheduling use the same commit path; calibration remains read
//! only. The entry point itself performs no settings write.
//!
//! The optional boot record publishes initialization observations. Removing
//! that record does not remove startup checks, RF inhibition, watchdog setup,
//! fault capture, or the main application.

mod app;
mod board;
mod build_info;
mod drivers;
mod debug;
mod observation;
#[cfg(feature = "uart")]
mod protocol;
mod mcu;
mod startup;
mod storage;
mod profile;
mod slope;
mod frequency_entry;
mod channel;
mod ctcss;
mod dcs;
mod tx_signal;
mod ui;

const _: &[u8] = include_bytes!("../link/firmware.ld");

#[repr(C)]
struct BootState {
    signature: u32,
    status: u32,
    initialized_word: u32,
    cleared_word: u32,
    vector_base: u32,
    control: u32,
    interrupt_mask: u32,
    stack_pointer: u32,
    clock_control: u32,
    internal_clock: u32,
    clock_config: u32,
    pll_config: u32,
    reset_flags: u32,
    gpio_checks: u32,
    stack_limit: u32,
    systick_reload: u32,
}

#[used]
#[no_mangle]
static mut TRAKT_DATA_CHECK: u32 = 0x1357_9BDF;

#[used]
#[no_mangle]
static mut TRAKT_BSS_CHECK: u32 = 0;

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_BOOT: BootState = unsafe { core::mem::zeroed() };

#[no_mangle]
pub extern "C" fn __trakt_main() -> ! {
    debug::capture_previous();

    let initialized = unsafe {
        core::ptr::read_volatile(core::ptr::addr_of!(TRAKT_DATA_CHECK))
    };
    let cleared = unsafe {
        core::ptr::read_volatile(core::ptr::addr_of!(TRAKT_BSS_CHECK))
    };
    if initialized != 0x1357_9BDF || cleared != 0 {
        startup::stop(0x1001);
    }
    if mcu::interrupt_mask() != 1 {
        startup::stop(0x1007);
    }

    let gpio_checks = unsafe { board::initialize() };
    if gpio_checks != 3 {
        startup::stop(0x1002);
    }

    let clocks = match mcu::clock::inspect() {
        Ok(clocks) => clocks,
        Err(error) => startup::stop(0x2000 + error as u32),
    };
    let guard = app::bench::Guard::initialize(clocks.hclk);

    let uart = match unsafe { drivers::uart::Tx::initialize(clocks.pclk) } {
        Some(uart) => uart,
        None => startup::stop(0x1005),
    };
    let systick_reload = match unsafe { mcu::tick::initialize(clocks.hclk) } {
        Some(load) => load,
        None => startup::stop(0x1004),
    };

    if cfg!(feature = "debug-tools") {
        let state = BootState {
            signature: 0,
            status: 4,
            initialized_word: initialized,
            cleared_word: cleared,
            vector_base: unsafe { mcu::read(0xE000_ED08) },
            control: mcu::control(),
            interrupt_mask: mcu::interrupt_mask(),
            stack_pointer: mcu::stack_pointer(),
            clock_control: unsafe { mcu::read(mcu::RCC) },
            internal_clock: unsafe { mcu::read(mcu::RCC + 4) },
            clock_config: unsafe { mcu::read(mcu::RCC + 8) },
            pll_config: unsafe { mcu::read(mcu::RCC + 12) },
            reset_flags: unsafe { mcu::read(mcu::RCC + 0x60) },
            gpio_checks,
            stack_limit: startup::main_stack_limit(),
            systick_reload,
        };
    
        crate::observation::publish!(TRAKT_BOOT, state, 0x544B_5254);
    }

    app::run(clocks, uart, guard)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    startup::stop(0x1003)
}