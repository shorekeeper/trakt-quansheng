//! Board-specific device drivers.

pub(crate) mod adc;
pub(crate) mod bk4829;
pub(crate) mod keyboard;
pub(crate) mod py25q16;
pub(crate) mod st7565;
#[cfg(feature = "uart")]
pub(crate) mod uart;
#[cfg(not(feature = "uart"))]
#[path = "uart_off.rs"]
pub(crate) mod uart;

mod serial_view;
#[cfg(feature = "uart")]
mod uart_rx;
#[cfg(feature = "uart")]
mod uart_channels;