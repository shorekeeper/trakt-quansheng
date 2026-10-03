//! USART1 receive buffering through DMA1 channel two.
//!
//! DMA alone writes the 256-byte static ring. Foreground reads use raw pointers
//! and volatile accesses, not Rust references to DMA-owned storage. The request
//! mapping is USART1 RX with byte transfers, memory increment, circular reload,
//! and no DMA interrupt.
//!
//! A pass consumes at most 64 bytes from the producer position sampled through
//! CNDTR. A complete lap is not recoverable from position alone. A service gap
//! over fifty observed ticks discards buffered input and resets partial framing;
//! this is not a lossless bulk-transport guarantee.
//!
//! USART errors reset partial framing and restart the ring. DMA errors disable
//! reception. A complete staged import survives framing reset until released
//! by its application owner.
//!
//! The protocol parser owns command recognition, binary-body isolation, CRC,
//! and staged schema. This driver supplies bytes and transport-loss boundaries.
//! No recognized command directly changes RF, persistent storage, or version state.

use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use crate::mcu;
use crate::protocol::{Event, Requests};
use super::serial_view::View;

const USART: usize = 0x4001_3800;
const DMA: usize = 0x4002_0000;
const CHANNEL: usize = DMA + 0x1C;
const MAPPING: usize = 0x4001_0008;
const ENABLED: u32 = 0x20A1;


#[repr(C, align(4))]
struct Ring {
    bytes: [u8; 256],
}

#[used]
#[no_mangle]
static mut TRAKT_UART_RX: Ring = Ring { bytes: [0; 256] };

const fn position(remaining: u32) -> Option<usize> {
    if remaining > 256 {
        None
    } else {
        Some(((256 - remaining) & 255) as usize)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct State {
    signature: u32,
    tick: u32,
    received: u32,
    requests: u32,
    captures: u32,
    transmitted: u32,
    errors: u32,
    overruns: u32,
    error_flags: u32,
    last_byte: u32,
    resyncs: u32,
    tx_failures: u32,
    status: u32,
    divider: u32,
    remaining: u32,
    flags: u32,
}

#[used]
#[no_mangle]
static mut TRAKT_SERIAL: State = unsafe { core::mem::zeroed() };

pub(super) fn view() -> View {
    let state = unsafe { read_volatile(addr_of!(TRAKT_SERIAL)) };
    View {
        received: state.received,
        requests: state.requests,
        captures: state.captures,
        errors: state.errors,
        overruns: state.overruns,
        error_flags: state.error_flags,
        tx_failures: state.tx_failures,
        status: state.status,
        remaining: state.remaining,
        live: state.flags & 0x21 == 0x21,
        rx_high: state.flags & 16 != 0,
    }
}

pub(super) struct Rx {
    commands: Requests<'static>,
    export_requested: bool,
    tail: usize,
    last_tick: u32,
    live: bool,
    received: u32,
    requests: u32,
    errors: u32,
    overruns: u32,
    error_flags: u32,
    last_byte: u32,
    resyncs: u32,
}

impl Rx {
    /// USART1 must be disabled and the caller must own DMA channel
    /// two and its request mapping. Call once after BSS initialization.
    pub(super) unsafe fn initialize() -> Self {
        unsafe {
            mcu::modify(mcu::RCC + 0x38, 1, 1);
            let _ = mcu::read(mcu::RCC + 0x38);
            mcu::modify(mcu::RCC + 0x40, 1, 1);
            let _ = mcu::read(mcu::RCC + 0x40);

            mcu::write(CHANNEL, 0);
            mcu::modify(MAPPING, 0x3F00, 7 << 8);
            mcu::write(CHANNEL + 4, 256);
            mcu::write(CHANNEL + 8, (USART + 4) as u32);
            mcu::write(
                CHANNEL + 12,
                addr_of_mut!(TRAKT_UART_RX.bytes) as *mut u8 as usize as u32,
            );
            mcu::write(DMA + 4, 0xF0);
            asm!("dmb", options(nostack, preserves_flags));
            mcu::write(CHANNEL, ENABLED);
            mcu::modify(USART + 20, 1 << 6, 1 << 6);
        }

        Self {
            commands: unsafe { super::uart_channels::requests() },
            export_requested: false,
            tail: 0,
            last_tick: 0,
            live: true,
            received: 0,
            requests: 0,
            errors: 0,
            overruns: 0,
            error_flags: 0,
            last_byte: 0,
            resyncs: 0,
        }
    }

    fn recover(&mut self) {
        unsafe {
            mcu::modify(USART + 20, 1 << 6, 0);
            mcu::write(CHANNEL, 0);

            // SR followed by DR clears the USART receive error flags.
            let _ = mcu::read(USART);
            let _ = mcu::read(USART + 4);

            mcu::write(CHANNEL + 4, 256);
            mcu::write(DMA + 4, 0xF0);
            asm!("dmb", options(nostack, preserves_flags));
            mcu::write(CHANNEL, ENABLED);
            mcu::modify(USART + 20, 1 << 6, 1 << 6);
        }
        self.tail = 0;
        self.commands.reset_partial();
        self.resyncs = self.resyncs.saturating_add(1);
    }

    pub(super) fn take_export(&mut self) -> bool {
        let requested = self.export_requested;
        self.export_requested = false;
        requested
    }

    pub(super) fn import_version(&self) -> u8 {
        self.commands.version()
    }

    pub(super) fn import_pending(&self) -> bool {
        self.commands.pending()
    }

    pub(super) fn import_data(&self) -> Option<&[u8]> {
        self.commands.data()
    }

    pub(super) fn finish_import(&mut self) {
        self.commands.finish();
    }

    pub(super) fn stop(&mut self) {
        unsafe {
            mcu::modify(USART + 20, 1 << 6, 0);
            mcu::write(CHANNEL, 0);
        }
        self.live = false;
        self.commands.reset_partial();
    }

    pub(super) fn poll(&mut self, now: u32) -> bool {
        if !self.live {
            return false;
        }

        self.commands.expire(now);

        if unsafe { mcu::read(DMA) } & 0x80 != 0 {
            self.errors = self.errors.saturating_add(1);
            self.error_flags |= 1 << 16;
            self.stop();
            return false;
        }

        let status = unsafe { mcu::read(USART) };
        if status & 15 != 0 {
            self.errors = self.errors.saturating_add(1);
            self.error_flags |= status & 15;
            if status & 8 != 0 {
                self.overruns = self.overruns.saturating_add(1);
            }
            self.recover();
            self.last_tick = now;
            return false;
        }

        let head = match position(unsafe { mcu::read(CHANNEL + 4) }) {
            Some(head) => head,
            None => {
                self.errors = self.errors.saturating_add(1);
                self.error_flags |= 1 << 17;
                self.stop();
                return false;
            }
        };

        if now.wrapping_sub(self.last_tick) > 50 {
            self.tail = head;
            self.commands.reset_partial();
            self.resyncs = self.resyncs.saturating_add(1);
        }
        self.last_tick = now;

        unsafe { asm!("dmb", options(nostack, preserves_flags)) };
        let bytes = unsafe { addr_of!(TRAKT_UART_RX.bytes) as *const u8 };
        let mut requested = false;

        for _ in 0..64 {
            if self.tail == head {
                break;
            }
            let value = unsafe { read_volatile(bytes.add(self.tail)) };
            self.tail = (self.tail + 1) & 255;
            self.received = self.received.saturating_add(1);
            self.last_byte = value as u32;

            match self.commands.byte(value, now) {
                Event::Capture => {
                    requested = true;
                    self.requests = self.requests.saturating_add(1);
                }
                Event::Export => {
                    self.export_requested = true;
                    self.requests = self.requests.saturating_add(1);
                }
                Event::Import => {
                    self.requests = self.requests.saturating_add(1);
                }
                Event::None => {}
            }
        }
        requested
    }

    pub(super) fn publish(
        &self,
        now: u32,
        captures: u32,
        transmitted: u32,
        tx_failures: u32,
        tx_flags: u32,
    ) {
        let rx_high = (unsafe { mcu::read(mcu::GPIO_A + 0x10) }) & (1u32 << 10) != 0;
        let state = State {
            signature: 0,
            tick: now,
            received: self.received,
            requests: self.requests,
            captures,
            transmitted,
            errors: self.errors,
            overruns: self.overruns,
            error_flags: self.error_flags,
            last_byte: self.last_byte,
            resyncs: self.resyncs,
            tx_failures,
            status: unsafe { mcu::read(USART) },
            divider: unsafe { mcu::read(USART + 8) },
            remaining: unsafe { mcu::read(CHANNEL + 4) },
            flags: tx_flags | ((rx_high as u32) << 4) | ((self.live as u32) << 5),
        };

        unsafe {
            write_volatile(addr_of_mut!(TRAKT_SERIAL.signature), 0);
            write_volatile(addr_of_mut!(TRAKT_SERIAL), state);
            asm!("dmb", options(nostack, preserves_flags));
            write_volatile(addr_of_mut!(TRAKT_SERIAL.signature), 0x5455_4331);
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<Ring>() == 256);
    assert!(core::mem::size_of::<State>() == 64);
    assert!(ENABLED & 0x000E == 0);
    assert!(ENABLED & 0x0010 == 0);
    assert!(ENABLED & 0x0020 != 0);
    assert!(ENABLED & 0x0080 != 0);
    assert!(ENABLED & 0x0F00 == 0);

    assert!(matches!(position(256), Some(0)));
    assert!(matches!(position(255), Some(1)));
    assert!(matches!(position(1), Some(255)));
    assert!(matches!(position(0), Some(0)));
    assert!(position(257).is_none());


};