//! Serial flash commands constrained to the board and Trakt record area.
//!
//! Reads use physical addresses. Erase accepts only one of the two record
//! sectors; program accepts only their first 512 bytes and cannot cross a
//! 256-byte page. Chip erase, status writes, and arbitrary write addresses
//! are not exposed.
//!
//! Program data is supplied on each poll and must remain unchanged until
//! Done. The driver owns no page buffer. A request has a 250-tick wire
//! deadline; program and erase completion are checked by the journal.
//!
//! A failed transaction disables SPI2 and releases CS. It cannot undo an
//! operation already accepted by the flash.

use crate::app::time::reached;
use crate::mcu::{self, spi::{Progress, Spi}};

pub(crate) const CAPACITY: u32 = 2 * 1024 * 1024;
// Fixed allocation for this board. Boot rescans both complete sectors;
// an unknown header or occupied tail still prevents reclamation.
pub(crate) const RECORD_SECTORS: [u32; 2] = [0x197000, 0x198000];

#[derive(Clone, Copy)]
pub(crate) enum Query {
    Status1,
    Status2,
    Id,
    Read { address: u32, length: u16 },
    Enable,
    Disable,
    Erase { slot: u8 },
    Program { slot: u8, offset: u16, length: u16 },
}

struct Header {
    bytes: [u8; 4],
    length: usize,
    data: usize,
    program: bool,
}

const fn command(opcode: u8, data: usize) -> Header {
    Header {
        bytes: [opcode, 0, 0, 0],
        length: 1,
        data,
        program: false,
    }
}

const fn addressed(opcode: u8, address: u32, data: usize, program: bool) -> Header {
    Header {
        bytes: [
            opcode,
            (address >> 16) as u8,
            (address >> 8) as u8,
            address as u8,
        ],
        length: 4,
        data,
        program,
    }
}

const fn program_address(slot: u8, offset: u16, length: u16) -> Option<u32> {
    if slot > 1 || length == 0 || length > 256 || offset >= 512
        || length as u32 > 512 - offset as u32
        || offset as u32 / 256 != (offset as u32 + length as u32 - 1) / 256
    {
        None
    } else {
        Some(RECORD_SECTORS[slot as usize] + offset as u32)
    }
}

const fn header(query: Query) -> Option<Header> {
    match query {
        Query::Status1 => Some(command(0x05, 1)),
        Query::Status2 => Some(command(0x35, 1)),
        Query::Id => Some(command(0x9F, 3)),
        Query::Enable => Some(command(0x06, 0)),
        Query::Disable => Some(command(0x04, 0)),
        Query::Read { address, length } => {
            if length == 0 || length > 512 || address >= CAPACITY
                || length as u32 > CAPACITY - address
            {
                None
            } else {
                Some(addressed(0x03, address, length as usize, false))
            }
        }
        Query::Erase { slot } => {
            if slot > 1 {
                None
            } else {
                Some(addressed(0x20, RECORD_SECTORS[slot as usize], 0, false))
            }
        }
        Query::Program { slot, offset, length } => {
            match program_address(slot, offset, length) {
                Some(address) => Some(addressed(0x02, address, length as usize, true)),
                None => None,
            }
        }
    }
}

pub(crate) const fn supported(id: u32) -> bool {
    matches!(id, 0x852015 | 0x856015 | 0x684015)
}

pub(crate) const fn suspended(id: u32, status2: u32) -> bool {
    let mask = if id == 0x684015 { 0x84 } else { 0x80 };
    status2 & mask != 0
}

/// Write admission is deliberately narrower than read admission.
/// All Boya BP bits, CMP, suspend, WIP, and an inherited WEL must be clear.
/// SRP and security-register lock bits do not authorize a status change.
pub(crate) const fn writable(id: u32, status1: u32, status2: u32) -> bool {
    id == 0x684015 && status1 & 0x7F == 0 && status2 & 0xC4 == 0
}

struct Job {
    header: Header,
    position: usize,
    deadline: u32,
}

pub(crate) enum Reply {
    Pending,
    Byte { index: usize, value: u8 },
    Done,
}

pub(crate) struct Flash {
    spi: Spi,
    job: Option<Job>,
    failure_position: u32,
}

impl Flash {
    /// the caller exclusively owns SPI2 and PA0..PA3.
    pub(crate) unsafe fn initialize(pclk: u32) -> Option<Self> {
        Some(Self {
            spi: unsafe { Spi::initialize_flash(pclk)? },
            job: None,
            failure_position: 0,
        })
    }

    pub(crate) fn start(&mut self, query: Query, now: u32) -> Result<(), u32> {
        if self.job.is_some() || !self.spi.idle() {
            return Err(302);
        }
        let header = match header(query) {
            Some(header) => header,
            None => return Err(301),
        };
        self.job = Some(Job {
            header,
            position: 0,
            deadline: now.wrapping_add(250),
        });
        unsafe { mcu::write(mcu::GPIO_A + 0x18, 1 << 19) };
        Ok(())
    }

    pub(crate) fn poll(&mut self, now: u32) -> Result<Reply, u32> {
        self.exchange(now, &[])
    }

    pub(crate) fn poll_program(&mut self, now: u32, data: &[u8]) -> Result<Reply, u32> {
        self.exchange(now, data)
    }

    fn exchange(&mut self, now: u32, data: &[u8]) -> Result<Reply, u32> {
        let job = match self.job.as_ref() {
            Some(job) => job,
            None => return Ok(Reply::Done),
        };

        let position = job.position;
        let prefix = job.header.length;
        let total = prefix + job.header.data;
        let program = job.header.program;

        if reached(now, job.deadline) {
            self.disable();
            return Err(203);
        }
        if program && data.len() != job.header.data {
            self.disable();
            return Err(303);
        }

        let output = if position < prefix {
            job.header.bytes[position]
        } else if program {
            data[position - prefix]
        } else {
            0xFF
        };

        match self.spi.poll(output, now) {
            Progress::Pending => Ok(Reply::Pending),
            Progress::Failed(error) => {
                self.failure_position = position as u32;
                self.disable();
                Err(200 + error)
            }
            Progress::Complete(value) => {
                self.job.as_mut().unwrap().position += 1;
                if position + 1 == total {
                    unsafe { mcu::write(mcu::GPIO_A + 0x18, 1 << 3) };
                    self.job = None;
                }
                if position >= prefix && !program {
                    Ok(Reply::Byte { index: position - prefix, value })
                } else {
                    Ok(Reply::Pending)
                }
            }
        }
    }

    /// Stage, SR, CR1, and zero-based wire byte position including the header.
    pub(crate) fn diagnostic(&self) -> [u32; 4] {
        let spi = self.spi.failure();
        [spi[0], spi[1], spi[2], self.failure_position]
    }

    pub(crate) fn disable(&mut self) {
        self.spi.disable();
        unsafe { mcu::write(mcu::GPIO_A + 0x18, 1 << 3) };
        self.job = None;
    }
}

const _: () = {
    assert!(supported(0x852015));
    assert!(supported(0x856015));
    assert!(supported(0x684015));
    assert!(!supported(0x684014));
    assert!(!supported(0x684016));
    assert!(!supported(0));
    assert!(!supported(0xFFFFFF));

    assert!(suspended(0x684015, 0x80));
    assert!(suspended(0x684015, 0x04));
    assert!(!suspended(0x684015, 2));
    assert!(suspended(0x852015, 0x80));
    assert!(!suspended(0x852015, 4));

    assert!(writable(0x684015, 0, 0));
    assert!(writable(0x684015, 0x80, 2));
    assert!(!writable(0x852015, 0, 0));
    assert!(!writable(0x684015, 1, 0));
    assert!(!writable(0x684015, 2, 0));
    assert!(!writable(0x684015, 4, 0));
    assert!(!writable(0x684015, 0, 0x40));
    assert!(!writable(0x684015, 0, 0x84));

    assert!(RECORD_SECTORS[0] == 0x197000);
    assert!(RECORD_SECTORS[1] == 0x198000);
    assert!(RECORD_SECTORS[0] & 4095 == 0);
    assert!(RECORD_SECTORS[1] == RECORD_SECTORS[0] + 4096);
    assert!(RECORD_SECTORS[1] + 4096 <= 0x1E0000);

    assert!(matches!(program_address(0, 0, 256), Some(0x197000)));
    assert!(matches!(program_address(1, 256, 252), Some(0x198100)));
    assert!(matches!(program_address(0, 508, 4), Some(0x1971FC)));
    assert!(program_address(2, 0, 1).is_none());
    assert!(program_address(0, 255, 2).is_none());
    assert!(program_address(0, 511, 2).is_none());
    assert!(program_address(0, 512, 1).is_none());
    assert!(program_address(0, 0, 0).is_none());

    assert!(header(Query::Erase { slot: 2 }).is_none());
    assert!(header(Query::Read { address: CAPACITY - 1, length: 1 }).is_some());
    assert!(header(Query::Read { address: CAPACITY - 1, length: 2 }).is_none());
    assert!(header(Query::Read { address: 0, length: 513 }).is_none());

    let page = match header(Query::Program { slot: 1, offset: 256, length: 252 }) {
        Some(page) => page,
        None => panic!("record page rejected"),
    };
    assert!(page.bytes[0] == 2);
    assert!(page.bytes[1] == 0x19);
    assert!(page.bytes[2] == 0x81);
    assert!(page.bytes[3] == 0);
    assert!(page.program && page.data == 252);
};