//! Exact comparison with the accepted logical settings baseline.
//!
//! The baseline contains all 148 Snapshot words, occupying 592 bytes. Journal
//! packing maps those fields into 448 persistent bytes. Comparison includes
//! names, both VFOs, channel routes and signaling, presentation, sound, power,
//! TOT, HAM, and the saved DW enable choice.
//!
//! Runtime RX, monitor, scanning progress, and the temporary DW listening VFO
//! are outside the Snapshot. They cannot create an unsaved-settings marker.
//!
//! One foreground owner borrows the static baseline after BSS initialization.
//! Defaults establish it when no saved record exists; validated Load replaces
//! it. Save replaces it only after verified committed completion. Reset RAM
//! and TestTorn do not accept a new baseline.
//!
//! Comparison is triggered by relevant settings operations, not every
//! foreground pass. This module allocates no storage, computes no checksum,
//! and issues no persistent write.

use core::ptr::addr_of_mut;

use crate::storage::record::Snapshot;

#[used]
#[no_mangle]
static mut TRAKT_CLEAN_SETTINGS: Snapshot = Snapshot { words: [0; 148] };

const fn equal(left: &Snapshot, right: &Snapshot) -> bool {
    let mut index = 0;

    while index < left.words.len() {
        if left.words[index] != right.words[index] {
            return false;
        }

        index += 1;
    }

    true
}

pub(super) struct Changes {
    baseline: &'static mut Snapshot,
    dirty: bool,
}

impl Changes {
    /// construct once after BSS initialization. No other context
    /// may access TRAKT_CLEAN_SETTINGS while this owner exists.
    pub(super) unsafe fn new() -> Self {
        Self {
            baseline: unsafe { &mut *addr_of_mut!(TRAKT_CLEAN_SETTINGS) },
            dirty: false,
        }
    }

    pub(super) fn dirty(&self) -> bool {
        self.dirty
    }

    pub(super) fn compare(&mut self, current: &Snapshot) {
        self.dirty = !equal(self.baseline, current);
    }

    pub(super) fn accept(&mut self, current: &Snapshot) {
        self.baseline.words.copy_from_slice(&current.words);
        self.dirty = false;
    }
}

const _: () = {
    assert!(core::mem::size_of::<Snapshot>() == 592);

    let baseline = Snapshot { words: [0; 148] };
    let mut changed = baseline;

    assert!(equal(&baseline, &changed));

    let mut index = 0;
    while index < changed.words.len() {
        changed.words[index] = 1;
        assert!(!equal(&baseline, &changed));

        changed.words[index] = 0;
        assert!(equal(&baseline, &changed));

        index += 1;
    }

    changed.words[19] = u32::from_le_bytes(*b"NAME");
    assert!(!equal(&baseline, &changed));

    changed.words[19] = 0;
    changed.words[7] = 4;
    assert!(!equal(&baseline, &changed));

    changed.words[7] = 0;
    changed.words[111] = 1;
    assert!(!equal(&baseline, &changed));
    changed.words[111] = 0;
    assert!(equal(&baseline, &changed));
};