//! Absent erased-pair search for compositions without debug-tools.
//!
//! Save still refuses unrecognized record sectors with error 612. No flash
//! read starts, and the Storage screen reports that the search is not part
//! of this image. Journal admission, record selection, and write limits are
//! unchanged; only the read-only layout investigation is omitted.

use crate::drivers::py25q16::Flash;

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) status: &'static [u8],
    pub(crate) pair: [u32; 2],
    pub(crate) inspected: u32,
}

pub(crate) struct Probe;

impl Probe {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) fn start(&mut self, _now: u32) {}

    pub(crate) fn active(&self) -> bool {
        false
    }

    pub(crate) fn error(&self) -> u32 {
        0
    }

    pub(crate) fn view(&self) -> View {
        View { status: b"SEARCH NOT IN IMAGE", pair: [0; 2], inspected: 0 }
    }

    pub(crate) fn pump(&mut self, _flash: &mut Flash, _now: u32) {}
}