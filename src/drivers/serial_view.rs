//! Foreground serial diagnostic view without buffer or peripheral ownership.
//!
//! A compiled UART driver copies its last published observation into this
//! value. An absent transport returns zero counters with live false.
//! This structure is not itself a static diagnostic record.

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) received: u32,
    pub(crate) requests: u32,
    pub(crate) captures: u32,
    pub(crate) errors: u32,
    pub(crate) overruns: u32,
    pub(crate) error_flags: u32,
    pub(crate) tx_failures: u32,
    pub(crate) status: u32,
    pub(crate) remaining: u32,
    pub(crate) live: bool,
    pub(crate) rx_high: bool,
}