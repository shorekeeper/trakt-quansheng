//! Read-only search between overlay applications and the reference log.
//!
//! An occupied sector is rejected after the first chunk containing a
//! non-FF byte. A potentially erased sector is read in full twice.
//! Only adjacent sectors that passed both reads form a candidate pair.
//!
//! The result is evidence for selecting a fixed layout, not permission to
//! erase. No write command is issued, and the driver's record addresses
//! are not changed by this search.
//!
//! One pump performs at most 128 transport steps. The entire search has
//! a 60000-tick deadline and continues to permit foreground watchdog service.

use crate::app::time::reached;
use crate::drivers::py25q16::{Flash, Query, Reply};

const BEGIN: u32 = 0x122000;
const END: u32 = 0x1E0000;
const SECTOR: u32 = 4096;

struct Finder {
    previous: u32,
    pair: [u32; 2],
}

impl Finder {
    const fn new() -> Self {
        Self { previous: 0, pair: [0; 2] }
    }

    const fn accept(&mut self, address: u32, erased: bool) -> bool {
        assert!(address >= BEGIN && address < END);
        assert!(address & (SECTOR - 1) == 0);

        if !erased {
            self.previous = 0;
            return false;
        }
        if self.previous != 0 && self.previous + SECTOR == address {
            self.pair = [self.previous, address];
            return true;
        }
        self.previous = address;
        false
    }
}

#[derive(Clone, Copy)]
enum State {
    Idle,
    Reading,
    Found,
    Absent,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) status: &'static [u8],
    pub(crate) pair: [u32; 2],
    pub(crate) inspected: u32,
}

pub(crate) struct Probe {
    state: State,
    finder: Finder,
    address: u32,
    chunk: u32,
    pass: u32,
    started: bool,
    clean: bool,
    deadline: u32,
    inspected: u32,
    error: u32,
}

impl Probe {
    pub(crate) const fn new() -> Self {
        Self {
            state: State::Idle,
            finder: Finder::new(),
            address: BEGIN,
            chunk: 0,
            pass: 0,
            started: false,
            clean: true,
            deadline: 0,
            inspected: 0,
            error: 0,
        }
    }

    pub(crate) fn start(&mut self, now: u32) {
        *self = Self::new();
        self.state = State::Reading;
        self.deadline = now.wrapping_add(60_000);
    }

    pub(crate) fn active(&self) -> bool {
        matches!(self.state, State::Reading)
    }

    pub(crate) fn error(&self) -> u32 {
        self.error
    }

    pub(crate) fn view(&self) -> View {
        View {
            status: match self.state {
                State::Idle => b"NOT STARTED",
                State::Reading => b"SEARCHING",
                State::Found => b"PAIR FOUND",
                State::Absent => b"NO ERASED PAIR",
                State::Failed => b"READ FAILURE",
            },
            pair: self.finder.pair,
            inspected: self.inspected,
        }
    }

    fn fail(&mut self, flash: &mut Flash, error: u32) {
        flash.disable();
        self.started = false;
        self.state = State::Failed;
        self.error = error;
    }

    fn finish_sector(&mut self, erased: bool) {
        self.inspected = self.inspected.saturating_add(1);

        if self.finder.accept(self.address, erased) {
            self.state = State::Found;
            return;
        }

        self.address += SECTOR;
        self.chunk = 0;
        self.pass = 0;
        self.clean = true;

        if self.address == END {
            self.state = State::Absent;
        }
    }

    /// The shared flash must be idle when start is called. Until active
    /// becomes false, no other owner may begin a flash transaction.
    pub(crate) fn pump(&mut self, flash: &mut Flash, now: u32) {
        if !self.active() {
            return;
        }
        if reached(now, self.deadline) {
            self.fail(flash, 624);
            return;
        }

        if !self.started {
            let query = Query::Read {
                address: self.address + self.chunk * 512,
                length: 512,
            };
            match flash.start(query, now) {
                Ok(()) => self.started = true,
                Err(error) => self.fail(flash, error),
            }
            return;
        }

        for _ in 0..128 {
            match flash.poll(now) {
                Err(error) => {
                    self.fail(flash, error);
                    return;
                }
                Ok(Reply::Pending) => {}
                Ok(Reply::Byte { value, .. }) => {
                    self.clean &= value == 255;
                }
                Ok(Reply::Done) => {
                    self.started = false;

                    if !self.clean {
                        self.finish_sector(false);
                        return;
                    }

                    self.chunk += 1;
                    if self.chunk == 8 {
                        self.chunk = 0;
                        self.pass += 1;
                        if self.pass == 2 {
                            self.finish_sector(true);
                        }
                    }
                    return;
                }
            }
        }
    }
}

const _: () = {
    assert!(BEGIN & 4095 == 0);
    assert!(END & 4095 == 0);
    assert!(BEGIN < END);
    assert!(END <= crate::drivers::py25q16::CAPACITY);

    let mut adjacent = Finder::new();
    assert!(!adjacent.accept(BEGIN, true));
    assert!(adjacent.accept(BEGIN + SECTOR, true));
    assert!(adjacent.pair[0] == BEGIN);
    assert!(adjacent.pair[1] == BEGIN + SECTOR);

    let mut occupied = Finder::new();
    assert!(!occupied.accept(BEGIN, true));
    assert!(!occupied.accept(BEGIN + SECTOR, false));
    assert!(!occupied.accept(BEGIN + 2 * SECTOR, true));
    assert!(occupied.accept(BEGIN + 3 * SECTOR, true));

    let mut separated = Finder::new();
    assert!(!separated.accept(BEGIN, true));
    assert!(!separated.accept(BEGIN + 2 * SECTOR, true));
    assert!(separated.accept(BEGIN + 3 * SECTOR, true));

    let mut last = Finder::new();
    assert!(!last.accept(END - 2 * SECTOR, true));
    assert!(last.accept(END - SECTOR, true));
};