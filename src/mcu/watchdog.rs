//! Independent watchdog owned by the foreground diagnostic.
//!
//! The configured timeout is 65536 LSI cycles: prescaler 64, reload 1023.
//! LSI is independent of SysTick and its actual frequency is not measured.
//! A stopped tick or stalled foreground therefore stops watchdog service.
//!
//! Configuration waits use a fixed polling budget and service an inherited
//! IWDG while its settings are updated. A running window watchdog is rejected.
//! Neither watchdog option bytes nor reset flags are modified.
//!
//! Debug halt freezes IWDG after the debug peripheral clock is enabled.
//! A common-trap stop does not refresh it and is followed by a watchdog reset.

use crate::mcu;

const IWDG: usize = 0x4000_3000;
const WWDG: usize = 0x4000_2C00;
const POLL_LIMIT: u32 = 1_000_000;

pub(crate) struct Watchdog {
    pub(crate) inherited_window: u32,
}

impl Watchdog {
    /// Starts IWDG and verifies its configured prescaler and reload.
    ///
    /// the caller owns watchdog and RCC configuration. Interrupts
    /// must be masked, and no other context may refresh or reconfigure IWDG.
    pub(crate) unsafe fn initialize() -> Result<Self, u32> {
        let inherited_window = unsafe {
            if mcu::read(mcu::RCC + 0x3C) & (1 << 11) != 0 {
                mcu::read(WWDG)
            } else {
                0
            }
        };
        if inherited_window & 0x80 != 0 {
            return Err(0x4001);
        }

        unsafe {
            mcu::write(IWDG, 0xAAAA);
            mcu::write(IWDG, 0xCCCC);
            mcu::write(IWDG, 0x5555);
            mcu::write(IWDG + 4, 4);
            mcu::write(IWDG + 8, 1023);
        }

        let mut ready = false;
        for attempt in 0..POLL_LIMIT {
            if attempt & 255 == 0 {
                unsafe { mcu::write(IWDG, 0xAAAA) };
            }
            if unsafe { mcu::read(IWDG + 12) } & 3 == 0
                && unsafe { mcu::read(mcu::RCC + 0x60) } & 2 != 0
            {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err(0x4002);
        }
        if unsafe { mcu::read(IWDG + 4) } & 7 != 4
            || unsafe { mcu::read(IWDG + 8) } & 0xFFF != 1023
        {
            return Err(0x4003);
        }

        unsafe {
            mcu::write(IWDG, 0xAAAA);
            mcu::write(IWDG, 0);

            mcu::modify(mcu::RCC + 0x40, 1 << 10, 1 << 10);
            let _ = mcu::read(mcu::RCC + 0x40);
            mcu::modify(0x4001_5808, 1 << 12, 1 << 12);
        }

        Ok(Self { inherited_window })
    }

    pub(crate) fn refresh(&mut self) {
        unsafe { mcu::write(IWDG, 0xAAAA) };
    }
}