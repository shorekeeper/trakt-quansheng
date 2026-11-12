//! Passive USB-C paddle input on the UV-K5 V3.
//!
//! PA12 is USB D+ and the paddle tip; PA11 is USB D- and the ring.
//! Both contacts close to USB GND, connected to the paddle sleeve.
//! VBUS, CC, and shield are not paddle conductors. This adapter must not
//! be connected to a USB host or charger.
//!
//! Wiring follows armel/uv-k1-k5v3-firmware-custom, App-CW-Keyer,
//! and the NR7Y USB-C to TRS paddle documentation. PY32F071 register
//! locations follow the Puya device header: GPIO MODER/PUPDR/IDR at
//! 00/0C/10, RCC APBENR1 at 3C, USBEN at bit 23.
//!
//! One Input owner constructs this driver after GPIO clocks are enabled.
//! CW compositions reserve PA11/PA12 for the entire application lifetime.
//! Initialization disables the inherited USB peripheral clock and selects
//! unpulled GPIO inputs. Selecting USB adds pull-ups; deselection removes
//! them. Neither state drives either data line.
//!
//! Only these two GPIO fields and the USB clock-enable bit are changed.
//! PA9/PA10 UART, PA13/PA14 SWD, GPIO output latches, alternate-function
//! fields, and all other clocks are preserved. No USB peripheral, IRQ,
//! reset, descriptor, DMA channel, or endpoint is started or restored.
//! This firmware has no USB protocol owner to resume on deselection.
//!
//! Activation waits two observed ticks before supplying a sample. Each
//! sample checks the USB clock and GPIO configuration before one IDR read
//! obtains both contacts together. Configuration mismatch returns None;
//! it cannot silently fall back to local PTT or repair a live configuration.
//!
//! Contact filtering, release qualification, keyer memory, and TX admission
//! remain with their existing owners. An unplugged passive adapter normally
//! looks released, not like a detectable cable-disconnect event. Pull-ups
//! and software filtering cannot distinguish every RF-induced assertion
//! from an intentional contact closure.

use crate::mcu;

const USB_CLOCK: u32 = 1 << 23;
const FIELDS: u32 = (3 << 22) | (3 << 24);
const PULL_UP: u32 = (1 << 22) | (1 << 24);

const fn contacts(levels: u32) -> (bool, bool) {
    (levels & (1 << 12) == 0, levels & (1 << 11) == 0)
}

pub(crate) struct Port {
    selected: bool,
    since: u32,
}

impl Port {
    /// Construct once with exclusive PA11/PA12 ownership and GPIOA enabled.
    /// This takes the pins away from any loader-provided USB configuration.
    pub(crate) unsafe fn initialize() -> Self {
        Self::configure(false);
        Self { selected: false, since: 0 }
    }

    fn configure(selected: bool) {
        let _mask = mcu::InterruptMask::acquire();
        unsafe {
            mcu::modify(mcu::RCC + 0x3C, USB_CLOCK, 0);
            let _ = mcu::read(mcu::RCC + 0x3C);
            mcu::modify(mcu::GPIO_A, FIELDS, 0);
            mcu::modify(
                mcu::GPIO_A + 0x0C,
                FIELDS,
                if selected { PULL_UP } else { 0 },
            );
        }
    }

    /// True reports a physical source transition to the caller, which
    /// invalidates ordinary transmitter release qualification.
    pub(crate) fn select(&mut self, now: u32, selected: bool) -> bool {
        if selected == self.selected {
            return false;
        }

        Self::configure(selected);
        self.selected = selected;
        self.since = now;
        true
    }

    /// Raw active-low contacts after activation settling. No contact
    /// history is cached here, and both levels come from one register read.
    pub(crate) fn sample(&self, now: u32) -> Option<(bool, bool)> {
        if !self.selected || now.wrapping_sub(self.since) < 2 {
            return None;
        }

        unsafe {
            if mcu::read(mcu::RCC + 0x3C) & USB_CLOCK != 0
                || mcu::read(mcu::GPIO_A) & FIELDS != 0
                || mcu::read(mcu::GPIO_A + 0x0C) & FIELDS != PULL_UP
            {
                return None;
            }

            Some(contacts(mcu::read(mcu::GPIO_A + 0x10)))
        }
    }
}

const _: () = {
    assert!(FIELDS == 0x03C0_0000);
    assert!(PULL_UP == 0x0140_0000);
    assert!(FIELDS & ((3 << 18) | (3 << 20) | (3 << 26) | (3 << 28)) == 0);
    assert!(!contacts(u32::MAX).0 && !contacts(u32::MAX).1);
    assert!(contacts(u32::MAX & !(1 << 12)).0);
    assert!(!contacts(u32::MAX & !(1 << 12)).1);
    assert!(!contacts(u32::MAX & !(1 << 11)).0);
    assert!(contacts(u32::MAX & !(1 << 11)).1);
    assert!(contacts(0).0 && contacts(0).1);
};