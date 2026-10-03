//! Board SPI byte transfers: mode 3, MSB first, PCLK divided by 64.
//!
//! The two constructors configure only the LCD and external-flash wiring.
//! Register bases remain private; this is not a general SPI interface.
//!
//! Each byte passes through TX availability, RX completion, and bus drain.
//! RX is consumed even though the display returns no useful data. A0 and
//! chip select may change only after the final drain reports completion.
//!
//! Polling performs one status read and at most one data-register access.
//! Each blocked stage has a five-tick deadline. Ready flags are checked
//! before timeout, so delayed foreground service can consume an already
//! completed operation. Progress starts a fresh deadline for the next stage.
//! Request deadlines remain separate. All bounds require a running SysTick.

use core::ptr::{read_volatile, write_volatile};

use crate::app::time::reached;
use crate::mcu;

const SPI: usize = 0x4001_3000;

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Tx,
    Rx,
    Drain,
    Dead,
}

enum Operation {
    Wait,
    Write,
    Read,
    Done,
    Fault(u32),
}

struct Transfer {
    phase: Phase,
    deadline: u32,
}

impl Transfer {
    const fn new() -> Self {
        Self { phase: Phase::Idle, deadline: 0 }
    }

    const fn poll(&mut self, status: u32, now: u32) -> Operation {
        if matches!(self.phase, Phase::Dead) {
            return Operation::Fault(1);
        }
        if matches!(self.phase, Phase::Idle) {
            self.phase = Phase::Tx;
            self.deadline = now.wrapping_add(5);
        }
        if status & 0x70 != 0 {
            self.phase = Phase::Dead;
            return Operation::Fault(1);
        }
        match self.phase {
            Phase::Tx if status & 2 != 0 => {
                self.phase = Phase::Rx;
                self.deadline = now.wrapping_add(5);
                Operation::Write
            }
            Phase::Rx if status & 1 != 0 => {
                self.phase = Phase::Drain;
                self.deadline = now.wrapping_add(5);
                Operation::Read
            }
            Phase::Drain if status & 0x82 == 2 => {
                self.phase = Phase::Idle;
                Operation::Done
            }
            _ if reached(now, self.deadline) => {
                self.phase = Phase::Dead;
                Operation::Fault(2)
            }
            _ => Operation::Wait,
        }
    }
}

pub(crate) enum Progress {
    Pending,
    Complete(u8),
    Failed(u32),
}

pub(crate) struct Spi {
    transfer: Transfer,
    base: usize,
    received: u8,
    /// Failed stage, SR, and CR1 before disabling the peripheral.
    failure: [u32; 3],
}

impl Spi {
    /// Configures PA5 and PA7 and resets SPI1.
    ///
    /// the caller owns SPI1 and these pins, has released LCD CS,
    /// and will keep PCLK unchanged while this transport is used.
    pub(crate) unsafe fn initialize(pclk: u32) -> Option<Self> {
        if !(1_000_000..=72_000_000).contains(&pclk) {
            return None;
        }

        unsafe {
            mcu::enable_gpio();
            mcu::modify(mcu::RCC + 0x40, 1 << 12, 1 << 12);
            let _ = mcu::read(mcu::RCC + 0x40);
            mcu::modify(mcu::RCC + 0x30, 1 << 12, 1 << 12);
            mcu::modify(mcu::RCC + 0x30, 1 << 12, 0);

            mcu::write(SPI, 0);
            mcu::write(SPI + 4, 0);

            mcu::write(mcu::GPIO_A + 0x18, (1 << 5) | (1 << 7));
            mcu::modify(mcu::GPIO_A + 4, (1 << 5) | (1 << 7), 0);
            mcu::modify(
                mcu::GPIO_A + 8,
                (3 << 10) | (3 << 14),
                (3 << 10) | (3 << 14),
            );
            mcu::modify(mcu::GPIO_A + 12, (3 << 10) | (3 << 14), 1 << 10);
            mcu::modify(mcu::GPIO_A + 0x20, (15 << 20) | (15 << 28), 0);
            mcu::modify(
                mcu::GPIO_A,
                (3 << 10) | (3 << 14),
                (2 << 10) | (2 << 14),
            );

            // SSM, SSI, BR=/64, master, CPOL=1, CPHA=1, SPE.
            mcu::write(SPI, 0x036F);
        }

        Some(Self {
            transfer: Transfer::new(),
            base: SPI,
            received: 0,
            failure: [0; 3],
        })
    }

    /// Configures SPI2: PA0 AF8 clock, PA1 AF9 MOSI, PA2 AF9 MISO.
    ///
    /// the caller owns SPI2 and PA0..PA3. Flash CS must remain
    /// released during configuration, and PCLK must remain unchanged.
    pub(crate) unsafe fn initialize_flash(pclk: u32) -> Option<Self> {
        if !(1_000_000..=72_000_000).contains(&pclk) {
            return None;
        }
        const FLASH_SPI: usize = 0x4000_3800;

        unsafe {
            mcu::enable_gpio();
            mcu::output(mcu::GPIO_A, 3, true);

            mcu::modify(mcu::RCC + 0x3C, 1 << 14, 1 << 14);
            let _ = mcu::read(mcu::RCC + 0x3C);
            mcu::modify(mcu::RCC + 0x2C, 1 << 14, 1 << 14);
            mcu::modify(mcu::RCC + 0x2C, 1 << 14, 0);

            mcu::write(FLASH_SPI, 0);
            mcu::write(FLASH_SPI + 4, 0);

            mcu::write(mcu::GPIO_A + 0x18, 7);
            mcu::modify(mcu::GPIO_A + 4, 7, 0);
            mcu::modify(mcu::GPIO_A + 8, 0x3F, 0x3F);
            mcu::modify(mcu::GPIO_A + 12, 0x3F, 0x15);
            mcu::modify(mcu::GPIO_A + 0x20, 0xFFF, 0x998);
            mcu::modify(mcu::GPIO_A, 0x3F, 0x2A);

            mcu::write(FLASH_SPI, 0x036F);
        }

        Some(Self {
            transfer: Transfer::new(),
            base: FLASH_SPI,
            received: 0,
            failure: [0; 3],
        })
    }

    pub(crate) fn idle(&self) -> bool {
        matches!(self.transfer.phase, Phase::Idle)
    }

    /// The caller supplies the same byte until Complete or Failed.
    pub(crate) fn poll(&mut self, value: u8, now: u32) -> Progress {
        let status = unsafe { mcu::read(self.base + 8) };
        let stage = match self.transfer.phase {
            Phase::Idle | Phase::Tx => 1,
            Phase::Rx => 2,
            Phase::Drain => 3,
            Phase::Dead => 4,
        };
        match self.transfer.poll(status, now) {
            Operation::Wait => Progress::Pending,
            Operation::Write => {
                unsafe { write_volatile((self.base + 12) as *mut u8, value) };
                Progress::Pending
            }
            Operation::Read => {
                self.received = unsafe {
                    read_volatile((self.base + 12) as *const u8)
                };
                Progress::Pending
            }
            Operation::Done => Progress::Complete(self.received),
            Operation::Fault(error) => {
                if self.failure[0] == 0 {
                    self.failure = [
                        stage,
                        status,
                        unsafe { mcu::read(self.base) },
                    ];
                }
                self.disable();
                Progress::Failed(error)
            }
        }
    }

    pub(crate) fn failure(&self) -> [u32; 3] {
        self.failure
    }

    pub(crate) fn disable(&mut self) {
        unsafe { mcu::write(self.base, 0) };
        self.transfer.phase = Phase::Dead;
    }
}

const _: () = {
    let mut transfer = Transfer::new();
    assert!(matches!(transfer.poll(2, 0), Operation::Write));
    assert!(matches!(transfer.poll(0, 0), Operation::Wait));
    assert!(matches!(transfer.poll(1, 0), Operation::Read));
    assert!(matches!(transfer.poll(0x82, 0), Operation::Wait));
    assert!(matches!(transfer.poll(2, 0), Operation::Done));

    let mut timeout = Transfer::new();
    assert!(matches!(timeout.poll(0, 10), Operation::Wait));
    assert!(matches!(timeout.poll(0, 14), Operation::Wait));
    assert!(matches!(timeout.poll(0, 15), Operation::Fault(2)));

    let mut wrapped = Transfer::new();
    assert!(matches!(wrapped.poll(0, 0xFFFF_FFFD), Operation::Wait));
    assert!(matches!(wrapped.poll(0, 1), Operation::Wait));
    assert!(matches!(wrapped.poll(0, 2), Operation::Fault(2)));

    // Delayed polling must consume ready data instead of discarding it.
    let mut delayed = Transfer::new();
    assert!(matches!(delayed.poll(2, 0), Operation::Write));
    assert!(matches!(delayed.poll(1, 20), Operation::Read));
    assert!(matches!(delayed.poll(2, 40), Operation::Done));

    let mut receive_timeout = Transfer::new();
    assert!(matches!(receive_timeout.poll(2, 0), Operation::Write));
    assert!(matches!(receive_timeout.poll(0, 4), Operation::Wait));
    assert!(matches!(receive_timeout.poll(0, 5), Operation::Fault(2)));

    let mut drain_timeout = Transfer::new();
    assert!(matches!(drain_timeout.poll(2, 0), Operation::Write));
    assert!(matches!(drain_timeout.poll(1, 1), Operation::Read));
    assert!(matches!(drain_timeout.poll(0x82, 5), Operation::Wait));
    assert!(matches!(drain_timeout.poll(0x82, 6), Operation::Fault(2)));

    // Hardware error flags take precedence even when RXNE is set.
    let mut receive_fault = Transfer::new();
    assert!(matches!(receive_fault.poll(2, 0), Operation::Write));
    assert!(matches!(receive_fault.poll(0x41, 1), Operation::Fault(1)));

    let mut fault = Transfer::new();
    assert!(matches!(fault.poll(0x40, 0), Operation::Fault(1)));
    assert!(matches!(fault.poll(2, 0), Operation::Fault(1)));
};