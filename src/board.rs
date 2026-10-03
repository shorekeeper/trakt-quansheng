//! Initial output levels for UV-K5 V3.
//!
//! External flash, display, and RF chip selects are released before their
//! other signals are changed. The audio amplifier, backlight, and lamp
//! are driven low. PTT is an input with a pull-up.
//!
//! PA13 and PA14 are not modified: their loader-provided SWD configuration
//! is retained. Keypad scanning and alternate-function buses are not
//! configured here.
//!
//! Releasing RF chip select alone does not stop transmission. The RF
//! inhibition driver subsequently clears the chip's link and PA controls.

use crate::mcu;

/// Parks the board without communicating with an external device.
///
/// interrupts must be masked and the caller must own these GPIOs.
/// This is also used by exception handling, so it does not access mutable
/// static data or depend on prior board initialization.
pub(crate) unsafe fn park_outputs() {
    unsafe {
        mcu::enable_gpio();

        mcu::output(mcu::GPIO_F, 9, true);  // BK4829 CS.
        mcu::output(mcu::GPIO_A, 3, true);  // External flash CS.
        mcu::output(mcu::GPIO_B, 2, true);  // Display CS.

        mcu::output(mcu::GPIO_A, 8, false); // Audio amplifier.
        mcu::output(mcu::GPIO_F, 8, false); // Backlight.
        mcu::output(mcu::GPIO_C, 13, false);// Lamp.

        mcu::output(mcu::GPIO_A, 6, true);  // Display A0.
        mcu::output(mcu::GPIO_B, 8, true);  // RF serial clock.
        mcu::output(mcu::GPIO_B, 9, true);  // RF serial data.
    }
}

/// Returns bit 0 for the expected GPIO configuration and bit 1 when the
/// SWD mode and alternate-function fields were preserved.
///
/// the caller owns GPIO configuration with interrupts masked.
pub(crate) unsafe fn initialize() -> u32 {
    const SWD_MODES: u32 = (3 << 26) | (3 << 28);
    const SWD_FUNCTIONS: u32 = (15 << 20) | (15 << 24);

    unsafe {
        mcu::enable_gpio();
        let swd_modes = mcu::read(mcu::GPIO_A) & SWD_MODES;
        let swd_functions = mcu::read(mcu::GPIO_A + 0x24) & SWD_FUNCTIONS;

        park_outputs();
        mcu::input_pull_up(mcu::GPIO_B, 10);

        let outputs = mcu::output_matches(mcu::GPIO_F, 9, true)
            && mcu::output_matches(mcu::GPIO_A, 3, true)
            && mcu::output_matches(mcu::GPIO_B, 2, true)
            && mcu::output_matches(mcu::GPIO_A, 8, false)
            && mcu::output_matches(mcu::GPIO_F, 8, false)
            && mcu::output_matches(mcu::GPIO_C, 13, false)
            && mcu::output_matches(mcu::GPIO_A, 6, true)
            && mcu::output_matches(mcu::GPIO_B, 8, true)
            && mcu::output_matches(mcu::GPIO_B, 9, true);

        let ptt = mcu::read(mcu::GPIO_B) & (3 << 20) == 0
            && mcu::read(mcu::GPIO_B + 12) & (3 << 20) == 1 << 20;

        let swd = mcu::read(mcu::GPIO_A) & SWD_MODES == swd_modes
            && mcu::read(mcu::GPIO_A + 0x24) & SWD_FUNCTIONS == swd_functions;

        (outputs && ptt) as u32 | ((swd as u32) << 1)
    }
}