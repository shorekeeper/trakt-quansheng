//! Absent VOX comparator owner for CW firmware compositions.
//!
//! CW images retain the checked persistent VOX choices but expose no VOX
//! navigation, comparator sampling, or automatic voice request. This concrete
//! replacement occupies no state and performs no register or event access.
//! Other firmware compositions use the operational BK4829 comparator driver.
//!
//! Zero is the only matching disabled selection. A request to start the
//! comparator is rejected rather than reported as successful configuration.
//! Cleanup and polling are inert; observations are always unavailable.
//! The enclosing RF owners retain their ordinary register initialization,
//! independent carrier checks, and complete RF-off cleanup.
//!
//! MASK is zero because this composition has no VOX event consumer. Receive
//! signaling retains its separate event mask and acknowledgement ownership.
//! No captured comparator register or enabled state can be restored here.

use crate::vox::Sample;

pub(crate) const MASK: u16 = 0;

pub(crate) struct Detector;

impl Detector {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) const fn active(&self) -> bool {
        false
    }

    pub(crate) const fn matches(&self, threshold: u16) -> bool {
        threshold == 0
    }

    pub(crate) fn start(&mut self, _now: u32, _threshold: u16) -> Result<(), u32> {
        Err(0x7C02)
    }

    pub(crate) fn check(&self) -> Result<(), u32> {
        Ok(())
    }

    pub(crate) fn stop(&mut self) {}

    pub(crate) fn observe(&mut self, _now: u32, _events: u16) {}

    pub(crate) fn pump(&mut self, _now: u32) {}

    pub(crate) fn sample(&self, _now: u32) -> Option<Sample> {
        None
    }
}