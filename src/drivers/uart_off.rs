//! Absent UART endpoint for builds without the uart feature.
//!
//! Tx has no state, buffers, counters, DMA ownership, or peripheral access.
//! Requests are always absent. Output requests are discarded without
//! invoking an export callback or claiming that anything was transmitted.
//! Keeping the concrete foreground interface avoids a second application
//! loop and introduces no dynamic dispatch or platform abstraction.
//!
//! Initialization performs no hardware setup. Startup and common failure
//! handling still retain their independent cleanup of inherited peripherals.
//! Selecting this module does not weaken that mandatory cleanup.

use crate::ui::Frame;

pub(crate) use super::serial_view::View as SerialView;

pub(crate) fn diagnostic() -> SerialView {
    SerialView {
        received: 0,
        requests: 0,
        captures: 0,
        errors: 0,
        overruns: 0,
        error_flags: 0,
        tx_failures: 0,
        status: 0,
        remaining: 0,
        live: false,
        rx_high: false,
    }
}

pub(crate) struct Tx;

impl Tx {
    /// Matches the concrete transport constructor used by startup.
    /// No clock validation or device access is needed for an absent endpoint.
    pub(crate) unsafe fn initialize(_pclk: u32) -> Option<Self> {
        Some(Self)
    }

    pub(crate) fn queue(&mut self, _now: u32, _values: [u32; 5]) {}

    pub(crate) fn pump(&mut self, _now: u32) {}

    pub(crate) fn publish(&self, _now: u32) {}

    pub(crate) const fn counters(&self) -> [u32; 4] {
        [0; 4]
    }

    pub(crate) const fn capture_requested(&self) -> bool {
        false
    }

    pub(crate) fn capture(&mut self, _now: u32, _frame: &Frame) {}

    pub(crate) const fn export_requested(&self) -> bool {
        false
    }

    pub(crate) fn export_channels(
        &mut self,
        _now: u32,
        _fill: impl FnOnce(&mut [u8]),
    ) {}

    pub(crate) const fn import_version(&self) -> u8 {
        0
    }

    pub(crate) fn import_request(&mut self) -> Option<&[u8]> {
        None
    }

    pub(crate) const fn import_data(&self) -> Option<&[u8]> {
        None
    }

    pub(crate) fn acknowledge_import(&mut self, _accepted: bool) {}

    pub(crate) fn finish_import(&mut self) {}
}

const _: () = {
    let endpoint = Tx;
    assert!(core::mem::size_of::<Tx>() == 0);
    assert!(!endpoint.capture_requested());
    assert!(!endpoint.export_requested());
    assert!(endpoint.import_data().is_none());
    assert!(endpoint.counters()[0] == 0);
    assert!(endpoint.counters()[3] == 0);
};