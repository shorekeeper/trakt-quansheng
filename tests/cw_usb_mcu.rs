//! Host-only register model for the production USB paddle owner.
//!
//! The four modeled words cover only the registers that driver accesses.
//! Every test uses its own thread-local register array. This is a check of
//! addressing, masks, contact mapping, and ownership transitions, not a model
//! of the USB PHY, GPIO electrical behavior, pull-up strength, or RF coupling.

use std::cell::RefCell;

pub(crate) const RCC: usize = 0x4002_1000;
pub(crate) const GPIO_A: usize = 0x5000_0000;

std::thread_local! {
    static REGISTERS: RefCell<[u32; 4]> = const { RefCell::new([0; 4]) };
}

fn index(address: usize) -> usize {
    match address {
        value if value == RCC + 0x3C => 0,
        value if value == GPIO_A => 1,
        value if value == GPIO_A + 0x0C => 2,
        value if value == GPIO_A + 0x10 => 3,
        _ => panic!("unexpected register address"),
    }
}

pub(crate) unsafe fn read(address: usize) -> u32 {
    REGISTERS.with(|words| words.borrow()[index(address)])
}

pub(crate) unsafe fn modify(address: usize, mask: u32, value: u32) {
    REGISTERS.with(|words| {
        let mut words = words.borrow_mut();
        let at = index(address);
        words[at] = (words[at] & !mask) | (value & mask);
    });
}

pub(crate) struct InterruptMask;

impl InterruptMask {
    pub(crate) fn acquire() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cw_usb::Port;

    const FIELDS: u32 = 0x03C0_0000;
    const PULL_UP: u32 = 0x0140_0000;
    const USB_CLOCK: u32 = 1 << 23;

    fn seed() {
        REGISTERS.with(|words| {
            *words.borrow_mut() = [u32::MAX; 4];
        });
    }

    fn snapshot() -> [u32; 4] {
        REGISTERS.with(|words| *words.borrow())
    }

    fn levels(value: u32) {
        REGISTERS.with(|words| words.borrow_mut()[3] = value);
    }

    #[test]
    fn initialization_changes_only_owned_fields_and_disables_usb_clock() {
        seed();
        let port = unsafe { Port::initialize() };
        let words = snapshot();
        assert_eq!(words[0], u32::MAX & !USB_CLOCK);
        assert_eq!(words[1], u32::MAX & !FIELDS);
        assert_eq!(words[2], u32::MAX & !FIELDS);
        assert_eq!(words[3], u32::MAX);
        assert!(port.sample(100).is_none());
    }

    #[test]
    fn activation_waits_and_obtains_both_contacts_from_gpioa() {
        seed();
        let mut port = unsafe { Port::initialize() };
        assert!(port.select(10, true));
        assert!(port.sample(10).is_none());
        assert!(port.sample(11).is_none());
        assert_eq!(port.sample(12), Some((false, false)));

        levels(u32::MAX & !(1 << 12));
        assert_eq!(port.sample(13), Some((true, false)));
        levels(u32::MAX & !(1 << 11));
        assert_eq!(port.sample(14), Some((false, true)));
        levels(u32::MAX & !((1 << 11) | (1 << 12)));
        assert_eq!(port.sample(15), Some((true, true)));
        levels(u32::MAX);
        assert_eq!(port.sample(16), Some((false, false)));
    }

    #[test]
    fn repeated_selection_does_not_restart_settling_or_rewrite_registers() {
        seed();
        let mut port = unsafe { Port::initialize() };
        assert!(port.select(10, true));
        let before = snapshot();
        assert!(!port.select(11, true));
        assert_eq!(snapshot(), before);
        assert_eq!(port.sample(12), Some((false, false)));
    }

    #[test]
    fn deselection_removes_pulls_and_cannot_return_a_contact_sample() {
        seed();
        let mut port = unsafe { Port::initialize() };
        assert!(port.select(10, true));
        assert_eq!(snapshot()[2] & FIELDS, PULL_UP);
        assert!(port.select(20, false));
        let words = snapshot();
        assert_eq!(words[0] & USB_CLOCK, 0);
        assert_eq!(words[1] & FIELDS, 0);
        assert_eq!(words[2] & FIELDS, 0);
        assert_eq!(words[1] & !FIELDS, u32::MAX & !FIELDS);
        assert_eq!(words[2] & !FIELDS, u32::MAX & !FIELDS);
        assert!(port.sample(100).is_none());
        assert!(!port.select(101, false));
    }

    #[test]
    fn configuration_fault_returns_none_without_corrective_writes() {
        for field in 0..3 {
            seed();
            let mut port = unsafe { Port::initialize() };
            assert!(port.select(10, true));

            REGISTERS.with(|words| {
                let mut words = words.borrow_mut();
                match field {
                    0 => words[0] |= USB_CLOCK,
                    1 => words[1] |= 1 << 22,
                    _ => words[2] &= !(1 << 24),
                }
            });

            let before = snapshot();
            assert!(port.sample(12).is_none());
            assert_eq!(snapshot(), before);
        }
    }

    #[test]
    fn activation_settling_crosses_tick_wrap() {
        seed();
        let mut port = unsafe { Port::initialize() };
        assert!(port.select(u32::MAX, true));
        assert!(port.sample(0).is_none());
        assert_eq!(port.sample(1), Some((false, false)));
        assert!(port.select(2, false));
        assert!(port.select(3, true));
        assert!(port.sample(4).is_none());
        assert_eq!(port.sample(5), Some((false, false)));
    }
}