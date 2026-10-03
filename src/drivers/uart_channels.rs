//! Construction of the command owner used by USART1 RX.
//!
//! Call once after BSS initialization. The returned parser owns the static
//! import payload; no second transport may construct another parser over it.
//! Transport framing and application confirmation remain separate.

pub(super) unsafe fn requests() -> crate::protocol::Requests<'static> {
    unsafe { crate::protocol::Requests::new() }
}