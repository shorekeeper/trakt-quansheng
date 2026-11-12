#![deny(warnings)]

//! Execute production CW timing code on the installed Windows host target.
//!
//! The two small vocabulary modules provide the exact contracts consumed
//! by cw.rs. Radio register code is not compiled or executed by this harness.
//! No dependency, device, audio, RF, or firmware version is involved.

mod app {
    pub(crate) mod time {
        pub(crate) const fn reached(now: u32, deadline: u32) -> bool {
            now.wrapping_sub(deadline) < 0x8000_0000
        }
    }
}

mod profile {
    pub(crate) const fn receive_span(frequency: u32) -> u8 {
        if frequency % 10 != 0 {
            return 0;
        }
        match frequency {
            50_000_000..=279_999_990 => 1,
            280_000_000..=629_999_990 => 2,
            840_000_000..=1_300_000_000 => 3,
            _ => 0,
        }
    }
}

#[path = "../src/cw.rs"]
mod cw;

#[path = "../src/cw_controls.rs"]
mod cw_controls;

#[path = "../src/cw_tail.rs"]
mod cw_tail;

#[path = "../src/cw_decode.rs"]
mod cw_decode;

#[path = "../src/cw_start.rs"]
mod cw_start;

#[path = "cw_usb_mcu.rs"]
mod mcu;

#[path = "../src/drivers/cw_usb.rs"]
mod cw_usb;

#[path = "../src/memory_copy.rs"]
mod memory_copy;