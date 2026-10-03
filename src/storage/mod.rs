//! Read-only calibration acquisition and exclusive flash transport ownership.
//!
//! The settings journal borrows the transport only after acquisition ends.
//! Its writes are restricted by the driver to the two Trakt record sectors;
//! the calibration block remains read only.
//!
//! The physical block at 010000 is read into one static 512-byte buffer.
//! A second transaction compares every byte against that buffer. No second
//! block-sized stack or RAM buffer is used.
//!
//! Status reads precede acquisition and follow comparison. Busy or suspended
//! memory is not accepted as a stable snapshot. Unknown identification,
//! transport failure, or a differing byte leaves the diagnostic unavailable.
//!
//! CRC32 identifies the acquired bytes for comparison with a saved dump.
//! It is computed here, not read from a factory checksum field. Equal reads
//! do not prove correct calibration. Only a validated summary is exposed.

pub(crate) mod area;
pub(crate) mod calibration;
pub(crate) mod journal;
pub(crate) mod record;
mod profile_layout;
pub(crate) mod squelch;
pub(crate) mod transmit;

use core::ptr::addr_of_mut;

use crate::app::time::reached;
use crate::drivers::py25q16::{self, Flash, Query, Reply};

const ADDRESS: u32 = 0x010000;

#[repr(C, align(4))]
struct Block {
    bytes: [u8; 512],
}

#[used]
#[no_mangle]
static mut TRAKT_CALIBRATION: Block = Block { bytes: [0; 512] };

#[repr(u32)]
#[derive(Clone, Copy)]
enum Phase {
    Wait,
    Status1,
    Status2,
    Id,
    Read,
    Compare,
    Final1,
    Final2,
    Ready,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) id: u32,
    pub(crate) ready: bool,
    pub(crate) error: u32,
    pub(crate) calibration: Option<calibration::Summary>,
}

#[repr(C)]
struct StorageState {
    signature: u32,
    tick: u32,
    phase: u32,
    error: u32,
    id: u32,
    capacity: u32,
    status1: u32,
    status2: u32,
    data_bytes: u32,
    compared_bytes: u32,
    crc32: u32,
    flags: u32,
    battery0: u32,
    battery1: u32,
    crystal: u32,
    gains: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_STORAGE: StorageState = unsafe { core::mem::zeroed() };

pub(crate) struct Memory {
    flash: Flash,
    block: &'static mut Block,
    phase: Phase,
    wake_at: u32,
    startup_deadline: u32,
    error: u32,
    id: u32,
    status1: u32,
    status2: u32,
    data_bytes: u32,
    compared_bytes: u32,
    summary: Option<calibration::Summary>,
}

impl Memory {
    /// Borrows the calibration buffer once and takes exclusive ownership
    /// of the flash transport. No ISR may access either resource.
    ///
    /// startup must have cleared BSS. Call only once, with no
    /// other references to TRAKT_CALIBRATION or users of SPI2.
    pub(crate) unsafe fn initialize(pclk: u32) -> Option<Self> {
        let flash = unsafe { Flash::initialize(pclk)? };
        let block = unsafe { &mut *addr_of_mut!(TRAKT_CALIBRATION) };
        Some(Self {
            flash,
            block,
            phase: Phase::Wait,
            wake_at: 10,
            startup_deadline: 1_000,
            error: 0,
            id: 0,
            status1: 0,
            status2: 0,
            data_bytes: 0,
            compared_bytes: 0,
            summary: None,
        })
    }

    pub(crate) fn view(&self) -> View {
        View {
            id: self.id,
            ready: matches!(self.phase, Phase::Ready),
            error: self.error,
            calibration: self.summary,
        }
    }

    /// Returns thresholds only after both calibration reads agreed.
    /// Level zero bypasses SQL but does not bypass calibration acquisition.
    /// FM uses the VHF factory table through 174 MHz and UHF above it.
    /// Extended RX has no independently calibrated FM table here; table
    /// selection is separate from the board's 280 MHz LNA switch.
    ///
    /// AM selects its independent raw-RSSI scale and does not decode an
    /// unused FM threshold row. This does not bypass the acquired calibration
    /// summary required separately for receiver initialization and audio gain.
    pub(crate) fn squelch(
        &self,
        frequency: u32,
        level: u8,
        am: bool,
    ) -> Result<squelch::Thresholds, u32> {
        if !matches!(self.phase, Phase::Ready) {
            return Err(0x5100);
        }
        squelch::select(&self.block.bytes, frequency, level, am)
    }

    /// Calibration acquisition ends before the journal can own a transaction.
    /// This borrow does not escape a foreground pump; the journal's next
    /// transaction continues through the same Flash instance.
    pub(crate) fn journal_flash(&mut self) -> Option<&mut Flash> {
        if matches!(self.phase, Phase::Ready) {
            Some(&mut self.flash)
        } else {
            None
        }
    }

    pub(crate) fn tx_profile(
        &self,
        frequency: u32,
        power: u8,
    ) -> Result<transmit::Profile, u32> {
        if !matches!(self.phase, Phase::Ready) {
            return Err(0x7300);
        }
        let summary = match self.summary {
            Some(summary) if summary.flags == 7 => summary,
            _ => return Err(0x7301),
        };
        transmit::decode(&self.block.bytes, summary, frequency, power)
    }

    fn fail(&mut self, error: u32) {
        self.flash.disable();
        self.phase = Phase::Failed;
        self.error = error;
        self.summary = None;
    }

    fn start(&mut self, phase: Phase, query: Query, now: u32) {
        match self.flash.start(query, now) {
            Ok(()) => self.phase = phase,
            Err(error) => self.fail(error),
        }
    }

    fn complete(&mut self, now: u32) {
        match self.phase {
            Phase::Status1 => {
                if self.status1 == 0xFF {
                    self.fail(102);
                } else if self.status1 & 1 != 0 {
                    self.wake_at = now.wrapping_add(10);
                    self.phase = Phase::Wait;
                } else {
                    self.start(Phase::Status2, Query::Status2, now);
                }
            }
            Phase::Status2 => {
                if self.status2 & 0x80 != 0 {
                    self.fail(104);
                } else {
                    self.start(Phase::Id, Query::Id, now);
                }
            }
            Phase::Id => {
                if !py25q16::supported(self.id) {
                    self.fail(103);
                } else if py25q16::suspended(self.id, self.status2) {
                    self.fail(104);
                } else {
                    self.start(
                        Phase::Read,
                        Query::Read { address: ADDRESS, length: 512 },
                        now,
                    );
                }
            }
            Phase::Read => {
                self.start(
                    Phase::Compare,
                    Query::Read { address: ADDRESS, length: 512 },
                    now,
                );
            }
            Phase::Compare => {
                self.start(Phase::Final1, Query::Status1, now);
            }
            Phase::Final1 => {
                if self.status1 & 1 != 0 {
                    self.fail(106);
                } else {
                    self.start(Phase::Final2, Query::Status2, now);
                }
            }
            Phase::Final2 => {
                if py25q16::suspended(self.id, self.status2) {
                    self.fail(104);
                } else if self.data_bytes != 1_024 || self.compared_bytes != 512 {
                    self.fail(107);
                } else {
                    self.summary = Some(calibration::decode(&self.block.bytes));
                    self.phase = Phase::Ready;
                }
            }
            _ => {}
        }
    }

    /// Performs at most 128 byte-transport steps. Neither a busy chip nor
    /// a failed acquisition blocks the rest of the foreground loop.
    pub(crate) fn pump(&mut self, now: u32) {
        if matches!(self.phase, Phase::Ready | Phase::Failed) {
            return;
        }
        if reached(now, self.startup_deadline) {
            self.fail(101);
            return;
        }
        if matches!(self.phase, Phase::Wait) {
            if reached(now, self.wake_at) {
                self.start(Phase::Status1, Query::Status1, now);
            }
            return;
        }

        for _ in 0..128 {
            let reply = match self.flash.poll(now) {
                Ok(reply) => reply,
                Err(error) => {
                    self.fail(error);
                    return;
                }
            };
            match reply {
                Reply::Pending => {}
                Reply::Done => {
                    self.complete(now);
                    return;
                }
                Reply::Byte { index, value } => match self.phase {
                    Phase::Status1 | Phase::Final1 => self.status1 = value as u32,
                    Phase::Status2 | Phase::Final2 => self.status2 = value as u32,
                    Phase::Id => self.id = (self.id << 8) | value as u32,
                    Phase::Read => {
                        self.block.bytes[index] = value;
                        self.data_bytes += 1;
                    }
                    Phase::Compare => {
                        self.data_bytes += 1;
                        if self.block.bytes[index] != value {
                            self.fail(105);
                            return;
                        }
                        self.compared_bytes += 1;
                    }
                    _ => {}
                },
            }
        }
    }

    pub(crate) fn publish(&self, now: u32) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let summary = self.summary.unwrap_or(calibration::Summary::empty());
        let state = StorageState {
            signature: 0,
            tick: now,
            phase: self.phase as u32,
            error: self.error,
            id: self.id,
            capacity: if py25q16::supported(self.id) { py25q16::CAPACITY } else { 0 },
            status1: self.status1,
            status2: self.status2,
            data_bytes: self.data_bytes,
            compared_bytes: self.compared_bytes,
            crc32: summary.crc32,
            flags: summary.flags,
            battery0: summary.battery[0] as u32,
            battery1: summary.battery[1] as u32,
            crystal: summary.crystal as i32 as u32,
            gains: summary.volume as u32 | ((summary.dac as u32) << 8),
        };
        crate::observation::publish!(TRAKT_STORAGE, state, 0x5453_4354);
    }
}

const _: () = {
    assert!(core::mem::size_of::<Block>() == 512);
    assert!(core::mem::size_of::<StorageState>() == 64);
};