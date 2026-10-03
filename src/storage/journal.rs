//! Alternating sector records with explicit write admission.
//!
//! Boot and Load read all 4096 bytes of both sectors. Bytes after the record
//! must be erased; otherwise the sector is foreign and cannot be reclaimed.
//! A valid latest record is then read again before it is applied to RAM.
//! No boot path erases or programs memory.
//!
//! Save snapshots RAM once and targets only the inactive sector. It checks
//! protection, enables writes, erases, verifies all erased bytes, programs
//! two body fragments, and compares their complete readback. A separate
//! four-byte commit is programmed only after that comparison.
//!
//! Every mutation verifies WEL before issue and WIP/WEL after completion.
//! Program and commit get 100 ticks; erase gets 1000. A complete request gets
//! 5000 ticks. The foreground executes at most 128 transport steps per pass.
//!
//! TestTorn follows Save through body verification but deliberately omits
//! commit. It requires an existing valid record and preserves its selection.
//! This tests protocol recovery, not the electrical effects of a brownout.
//!
//! Failure disables the shared transport and requires a reset before further
//! I/O. An accepted flash operation may finish after a wire failure; boot
//! rescans instead of trusting the interrupted session's metadata.

use core::ptr::addr_of_mut;

use crate::app::time::reached;
use crate::drivers::py25q16::{self, Flash, Query, Reply, RECORD_SECTORS};


use super::{calibration, record::{self, Meta, Snapshot}, Memory};

#[derive(Clone, Copy)]
pub(crate) enum Action {
    Save,
    Load,
    ResetRam,
    TestTorn,
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum Phase {
    Await,
    Check1,
    Check2,
    Pause,
    Scan,
    Load,
    Enable,
    Wel,
    Send,
    Busy1,
    Busy2,
    Poll,
    Blank,
    Verify,
    CommitVerify,
    CalVerify,
    Disable,
    Finish,
    Idle,
    Dead,
}

#[derive(Clone, Copy)]
enum Outcome {
    Defaults,
    Loaded,
    Saved,
    Torn,
    Foreign,
    Error,
}

#[repr(C)]
struct Workspace {
    record: [u8; 512],
    check: [u8; 512],
}

#[used]
#[no_mangle]
static mut TRAKT_JOURNAL_WORK: Workspace = Workspace {
    record: [0; 512],
    check: [0; 512],
};

#[repr(C)]
struct JournalState {
    signature: u32,
    tick: u32,
    phase: u32,
    error: u32,
    generation: u32,
    active: u32,
    target: u32,
    valid_mask: u32,
    owned_mask: u32,
    erased_mask: u32,
    loads: u32,
    saves: u32,
    torn_tests: u32,
    progress: u32,
    status1: u32,
    status2: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_JOURNAL: JournalState = unsafe { core::mem::zeroed() };

#[derive(Clone, Copy)]
pub(crate) struct View {

    pub(crate) status: &'static [u8],
    pub(crate) error: u32,
    pub(crate) generation: u32,
    pub(crate) valid_copies: u32,
    pub(crate) format: u32,
    pub(crate) diagnostic: [u32; 4],

    pub(crate) search: super::area::View,
}

pub(crate) struct Journal {
    work: &'static mut Workspace,
    phase: Phase,
    outcome: Outcome,
    meta: [Meta; 2],
    active: Option<usize>,
    target: usize,
    scan_slot: usize,
    chunk: u32,
    mutation: u32,
    loading: bool,
    torn: bool,
    loaded: bool,
    deadline: u32,
    operation_deadline: u32,
    wake_at: u32,
    status1: u32,
    status2: u32,
    id: u32,
    calibration_crc: u32,
    error: u32,
    loads: u32,
    saves: u32,
    torn_tests: u32,
    saved_event: bool,
    diagnostic: [u32; 4],

    search: super::area::Probe,
}

impl Journal {
    /// construct once after BSS initialization. No other context
    /// may access the static workspace or journal record.
    pub(crate) unsafe fn new() -> Self {
        Self {
            work: unsafe { &mut *addr_of_mut!(TRAKT_JOURNAL_WORK) },
            phase: Phase::Await,
            outcome: Outcome::Defaults,
            meta: [Meta::unknown(); 2],
            active: None,
            target: 0,
            scan_slot: 0,
            chunk: 0,
            mutation: 0,
            loading: true,
            torn: false,
            loaded: false,
            deadline: 5_000,
            operation_deadline: 0,
            wake_at: 0,
            status1: 0,
            status2: 0,
            id: 0,
            calibration_crc: 0,
            error: 0,
            loads: 0,
            saves: 0,
            torn_tests: 0,
            saved_event: false,
            diagnostic: [0; 4],

            search: super::area::Probe::new(),
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.search.active() || !matches!(self.phase, Phase::Idle | Phase::Dead)
    }

    pub(crate) fn view(&self) -> View {
        let status: &'static [u8] = if self.search.active() {
            b"READ ONLY SEARCH"
        } else if self.busy() {
            if self.loading { b"LOADING" as &[u8] } else { b"SAVING" }
        } else {
            match self.outcome {
                Outcome::Defaults => b"NO SAVED RECORD",
                Outcome::Loaded => b"LOADED",
                Outcome::Saved => b"SAVED",
                Outcome::Torn => b"TORN TEST COMPLETE",
                Outcome::Foreign => b"FOREIGN AREA",
                Outcome::Error => b"ERROR",
            }
        };
        View {

            status,
            error: self.error,
            generation: self.active.map_or(0, |slot| self.meta[slot].generation),
            valid_copies: self.meta.iter().filter(|meta| meta.valid).count() as u32,
            diagnostic: self.diagnostic,

            search: self.search.view(),
            format: self.active.map_or(0, |slot| self.meta[slot].version),

        }
    }

    fn reject(&mut self, error: u32) {
        self.error = error;
        self.outcome = Outcome::Error;
    }

    pub(crate) fn load(&mut self, now: u32) {
        if matches!(self.phase, Phase::Dead) {
            return;
        }
        if !matches!(self.phase, Phase::Idle) {
            self.reject(610);
            return;
        }
        self.error = 0;
        self.loading = true;
        self.loaded = false;
        self.phase = Phase::Await;
        self.deadline = now.wrapping_add(5_000);
    }

    #[inline(never)]
    pub(crate) fn save(&mut self, state: Snapshot, torn: bool, now: u32) {
        if matches!(self.phase, Phase::Dead) {
            return;
        }
        if !matches!(self.phase, Phase::Idle) {
            self.reject(610);
            return;
        }
        if !state.valid() {
            self.reject(611);
            return;
        }
        if !self.meta[0].reclaimable() || !self.meta[1].reclaimable() {
            self.reject(612);
            self.search.start(now);
            return;
        }
        if self.id != 0x684015 {
            self.reject(613);
            return;
        }
        if torn && self.active.is_none() {
            self.reject(614);
            return;
        }

        self.target = self.active.map_or(0, |slot| slot ^ 1);
        let generation = self.active.map_or(1, |slot| {
            self.meta[slot].generation.wrapping_add(1)
        });
        self.saved_event = false;
        record::encode_into(&mut self.work.record, &state, generation);
        self.loading = false;
        self.torn = torn;
        self.loaded = false;
        self.error = 0;
        self.mutation = 0;
        self.phase = Phase::Await;
        self.deadline = now.wrapping_add(5_000);
    }

    /// Published only after committed readback, calibration CRC comparison,
    /// and write-disable verification. TestTorn never sets this event.
    pub(crate) fn take_saved(&mut self) -> bool {
        let completed = self.saved_event;
        self.saved_event = false;
        completed
    }

    #[inline(never)]
    pub(crate) fn take_loaded(&mut self) -> Option<Snapshot> {
        if self.loaded {
            self.loaded = false;
            Some(record::snapshot(&self.work.record))
        } else {
            None
        }
    }

    fn fail(&mut self, flash: &mut Flash, error: u32) {
        self.diagnostic = flash.diagnostic();
        flash.disable();
        self.phase = Phase::Dead;
        self.loaded = false;
        self.reject(error);
    }

    fn start(&mut self, flash: &mut Flash, phase: Phase, query: Query, now: u32) {
        match flash.start(query, now) {
            Ok(()) => self.phase = phase,
            Err(error) => self.fail(flash, error),
        }
    }

    fn read_chunk(&mut self, flash: &mut Flash, phase: Phase, slot: usize, now: u32) {
        self.start(flash, phase, Query::Read {
            address: RECORD_SECTORS[slot] + self.chunk * 512,
            length: 512,
        }, now);
    }

    fn enable(&mut self, flash: &mut Flash, now: u32) {
        self.start(flash, Phase::Enable, Query::Enable, now);
    }

    fn issue(&mut self, flash: &mut Flash, now: u32) {
        let query = match self.mutation {
            0 => Query::Erase { slot: self.target as u8 },
            1 => Query::Program { slot: self.target as u8, offset: 0, length: 256 },
            2 => Query::Program { slot: self.target as u8, offset: 256, length: 252 },
            3 => Query::Program { slot: self.target as u8, offset: 508, length: 4 },
            _ => {
                self.fail(flash, 615);
                return;
            }
        };

        if self.active == Some(self.target) {
            self.fail(flash, 616);
            return;
        }
        self.start(flash, Phase::Send, query, now);
    }

    fn mutation_complete(&mut self, flash: &mut Flash, now: u32) {
        match self.mutation {
            0 => {
                self.meta[self.target] = Meta::unknown();
                self.chunk = 0;
                self.read_chunk(flash, Phase::Blank, self.target, now);
            }
            1 => {
                self.mutation = 2;
                self.enable(flash, now);
            }
            2 => {
                self.chunk = 0;
                self.read_chunk(flash, Phase::Verify, self.target, now);
            }
            3 => {
                record::put(&mut self.work.record, 508, record::COMMIT);
                self.chunk = 0;
                self.read_chunk(flash, Phase::CommitVerify, self.target, now);
            }
            _ => self.fail(flash, 615),
        }
    }

    fn advance(&mut self, flash: &mut Flash, now: u32) {
        match self.phase {
            Phase::Check1 => {
                if self.status1 == 255 {
                    self.fail(flash, 601);
                } else if self.status1 & 1 != 0 {
                    self.phase = Phase::Pause;
                    self.wake_at = now.wrapping_add(10);
                } else {
                    self.start(flash, Phase::Check2, Query::Status2, now);
                }
            }
            Phase::Check2 => {
                if py25q16::suspended(self.id, self.status2) {
                    self.fail(flash, 602);
                } else if self.loading {
                    self.meta = [Meta::unknown(); 2];

                    self.active = None;
                    self.scan_slot = 0;
                    self.chunk = 0;
                    self.read_chunk(flash, Phase::Scan, 0, now);
                } else if !py25q16::writable(self.id, self.status1, self.status2) {
                    self.fail(flash, 603);
                } else {
                    self.enable(flash, now);
                }
            }
            Phase::Scan => {
                if self.chunk == 0 {
                    self.meta[self.scan_slot] = Meta::inspect(&self.work.check);
                } else if !record::erased(&self.work.check) {
                    self.meta[self.scan_slot] = Meta::unknown();
                }

                self.chunk += 1;
                if self.chunk < 8 {
                    self.read_chunk(flash, Phase::Scan, self.scan_slot, now);
                    return;
                }
                if self.scan_slot == 0 {
                    self.scan_slot = 1;
                    self.chunk = 0;
                    self.read_chunk(flash, Phase::Scan, 1, now);
                    return;
                }

                self.active = match record::choose(self.meta[0], self.meta[1]) {
                    Ok(active) => active,
                    Err(error) => {
                        self.fail(flash, error);
                        return;
                    }
                };
                if let Some(slot) = self.active {
                    self.chunk = 0;
                    self.read_chunk(flash, Phase::Load, slot, now);
                } else {
                    self.phase = Phase::Idle;
                    self.outcome = if self.meta.iter().all(|meta| meta.reclaimable()) {
                        Outcome::Defaults
                    } else {
                        Outcome::Foreign
                    };
                }
            }
            Phase::Load => {
                let slot = self.active.unwrap();
                if !record::valid(&self.work.check, true)
                    || record::word(&self.work.check, 12) != self.meta[slot].generation
                    || record::word(&self.work.check, 504) != self.meta[slot].crc
                {
                    self.fail(flash, 605);
                    return;
                }
                self.work.record.copy_from_slice(&self.work.check);
                self.loaded = true;
                self.loads = self.loads.saturating_add(1);
                self.phase = Phase::Idle;
                self.outcome = Outcome::Loaded;
            }
            Phase::Enable => self.start(flash, Phase::Wel, Query::Status1, now),
            Phase::Wel => {
                if self.status1 & 0x7F != 2 {
                    self.fail(flash, 606);
                } else {
                    self.issue(flash, now);
                }
            }
            Phase::Send => {
                let budget = if self.mutation == 0 { 1_000 } else { 100 };
                self.operation_deadline = now.wrapping_add(budget);
                self.start(flash, Phase::Busy1, Query::Status1, now);
            }
            Phase::Busy1 => {
                if reached(now, self.operation_deadline) {
                    self.fail(flash, 607);
                } else if self.status1 & 1 != 0 {
                    self.phase = Phase::Poll;
                    self.wake_at = now.wrapping_add(2);
                } else if self.status1 & 0x7F != 0 {
                    self.fail(flash, 608);
                } else {
                    self.start(flash, Phase::Busy2, Query::Status2, now);
                }
            }
            Phase::Busy2 => {
                if !py25q16::writable(self.id, self.status1, self.status2) {
                    self.fail(flash, 603);
                } else {
                    self.mutation_complete(flash, now);
                }
            }
            Phase::Blank => {
                if !record::erased(&self.work.check) {
                    self.fail(flash, 617);
                    return;
                }
                self.chunk += 1;
                if self.chunk < 8 {
                    self.read_chunk(flash, Phase::Blank, self.target, now);
                } else {
                    self.meta[self.target] = Meta {
                        erased: true, ..Meta::unknown()
                    };
                    self.mutation = 1;
                    self.enable(flash, now);
                }
            }
            Phase::Verify => {
                if self.work.check != self.work.record
                    || !record::valid(&self.work.check, false)
                {
                    self.fail(flash, 618);
                    return;
                }
                self.meta[self.target] = Meta::inspect(&self.work.check);

                if self.torn {
                    self.start(flash, Phase::CalVerify, Query::Read {
                        address: 0x010000, length: 512,
                    }, now);
                } else {
                    self.mutation = 3;
                    self.enable(flash, now);
                }
            }
            Phase::CommitVerify => {
                if self.work.check != self.work.record
                    || !record::valid(&self.work.check, true)
                {
                    self.fail(flash, 619);
                    return;
                }
                self.start(flash, Phase::CalVerify, Query::Read {
                    address: 0x010000, length: 512,
                }, now);
            }
            Phase::CalVerify => {
                if calibration::crc32(&self.work.check) != self.calibration_crc {
                    self.fail(flash, 620);
                } else {
                    self.start(flash, Phase::Disable, Query::Disable, now);
                }
            }
            Phase::Disable => self.start(flash, Phase::Finish, Query::Status1, now),
            Phase::Finish => {
                if self.status1 & 3 != 0 {
                    self.fail(flash, 621);
                    return;
                }
                if self.torn {
                    self.torn_tests = self.torn_tests.saturating_add(1);
                    self.outcome = Outcome::Torn;
                } else {
                    self.meta[self.target] = Meta::inspect(&self.work.record);
                    self.active = Some(self.target);
                    self.saves = self.saves.saturating_add(1);
                    self.saved_event = true;
                    self.outcome = Outcome::Saved;
                }
                self.phase = Phase::Idle;
            }
            _ => {}
        }
    }

    #[inline(never)]
    pub(crate) fn pump(&mut self, memory: &mut Memory, now: u32) {
        if self.search.active() {
            let flash = match memory.journal_flash() {
                Some(flash) => flash,
                None => {
                    self.phase = Phase::Dead;
                    self.reject(622);
                    return;
                }
            };

            self.search.pump(flash, now);
            let error = self.search.error();
            if error != 0 {
                self.fail(flash, error);
            }
            return;
        }

        if !self.busy() { return; }

        let memory_view = memory.view();
        if memory_view.error != 0 {
            self.phase = Phase::Dead;
            self.reject(622);
            return;
        }
        if !memory_view.ready {
            if reached(now, self.deadline) {
                self.phase = Phase::Dead;
                self.reject(623);
            }
            return;
        }

        self.id = memory_view.id;
        self.calibration_crc = memory_view.calibration.unwrap().crc32;
        let flash = match memory.journal_flash() {
            Some(flash) => flash,
            None => return,
        };

        if reached(now, self.deadline) {
            self.fail(flash, 623);
            return;
        }

        match self.phase {
            Phase::Await => {
                self.start(flash, Phase::Check1, Query::Status1, now);
                return;
            }
            Phase::Pause if reached(now, self.wake_at) => {
                self.start(flash, Phase::Check1, Query::Status1, now);
                return;
            }
            Phase::Poll if reached(now, self.wake_at) => {
                self.start(flash, Phase::Busy1, Query::Status1, now);
                return;
            }
            Phase::Pause | Phase::Poll => return,
            _ => {}
        }

        for _ in 0..128 {
            let reply = if matches!(self.phase, Phase::Send) && self.mutation != 0 {
                match self.mutation {
                    1 => flash.poll_program(now, &self.work.record[..256]),
                    2 => flash.poll_program(now, &self.work.record[256..508]),
                    3 => flash.poll_program(now, &record::COMMIT.to_le_bytes()),
                    _ => unreachable!(),
                }
            } else {
                flash.poll(now)
            };

            match reply {
                Err(error) => {
                    self.fail(flash, error);
                    return;
                }
                Ok(Reply::Pending) => {}
                Ok(Reply::Done) => {
                    self.advance(flash, now);
                    return;
                }
                Ok(Reply::Byte { index, value }) => match self.phase {
                    Phase::Check1 | Phase::Wel | Phase::Busy1 | Phase::Finish => {
                        self.status1 = value as u32;
                    }
                    Phase::Check2 | Phase::Busy2 => self.status2 = value as u32,
                    Phase::Scan | Phase::Load | Phase::Blank | Phase::Verify
                        | Phase::CommitVerify | Phase::CalVerify =>
                    {
                        self.work.check[index] = value;
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

        let mut valid = 0;
        let mut owned = 0;
        let mut erased = 0;
        for index in 0..2 {
            valid |= (self.meta[index].valid as u32) << index;
            owned |= (self.meta[index].owned as u32) << index;
            erased |= (self.meta[index].erased as u32) << index;
        }
        let state = JournalState {
            signature: 0,
            tick: now,
            phase: self.phase as u32,
            error: self.error,
            generation: self.view().generation,
            active: self.active.map_or(u32::MAX, |slot| slot as u32),
            target: self.target as u32,
            valid_mask: valid,
            owned_mask: owned,
            erased_mask: erased,
            loads: self.loads,
            saves: self.saves,
            torn_tests: self.torn_tests,
            progress: self.chunk,
            status1: self.status1,
            status2: self.status2,
        };
        crate::observation::publish!(TRAKT_JOURNAL, state, 0x544A_4354);
    }
}

const _: () = {
    assert!(core::mem::size_of::<Workspace>() == 1024);
    assert!(core::mem::size_of::<JournalState>() == 64);
    assert!(RECORD_SECTORS[0] & 4095 == 0);
    assert!(RECORD_SECTORS[1] == RECORD_SECTORS[0] + 4096);
    assert!(RECORD_SECTORS[0] >= 0x122000);
    assert!(RECORD_SECTORS[1] + 4096 <= 0x1E0000);
};