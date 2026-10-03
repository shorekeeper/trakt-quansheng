//! Runtime stack observations and controlled recovery tests.
//!
//! Reset paints reserved stack RAM before the first Rust call. The bottom
//! sixteen bytes of each stack carry a distinct guard. Foreground scanning
//! checks at most sixteen painted words per pass with interrupts masked;
//! it never scans the currently occupied main-stack region.
//!
//! A changed paint word records prior use. Sampled MSP contributes current
//! use, including the observer itself. Equal data and paint can hide prior
//! use, and sampling does not establish a worst-case bound. No MPU or
//! hardware stack limit is implied by the guard.
//!
//! Previous failure data is copied once from noinit into ordinary BSS.
//! Its publication word contains a format-specific CRC over the remaining
//! fifteen words. Incompatible, incomplete, and corrupted records are rejected.
//! The record occupies a fixed 64-byte reservation above both stacks.
//! Application clearing and stack painting exclude that reservation.
//! Loader retention is still tested rather than assumed; an absent record
//! is not proof that the preceding run completed normally.
//!
//! Recovery tests require parked RF and an idle foreground storage owner.
//! They never modify saved settings. Watchdog testing deliberately masks
//! interrupts and stops servicing IWDG; exception testing executes UDF.
//! Neither test restores itself or TX admission after reboot.

pub(crate) mod timing;

use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use crate::{mcu, startup};

#[derive(Clone, Copy)]
pub(crate) struct MemoryView {
    pub(crate) static_bytes: u32,
    pub(crate) reserved_stack: u32,
    pub(crate) unassigned: u32,
}

/// Uses linked section extents. Static includes data, BSS, and retained RAM;
/// the stack value is its full reservation, not the observed watermark.
/// Unassigned is the gap below the emergency stack, not an available heap.
#[inline(never)]
pub(crate) fn memory_view() -> MemoryView {
    let data = (addr_of!(__data_end) as usize)
        - (addr_of!(__data_start) as usize);
    let bss = (addr_of!(__bss_end) as usize)
        - (addr_of!(__bss_start) as usize);
    let retained = (addr_of!(__noinit_end) as usize)
        - (addr_of!(__noinit_start) as usize);
    let stack_low = addr_of!(__stack_limit) as usize;
    let stack_high = addr_of!(__stack_top) as usize;
    let bss_end = addr_of!(__bss_end) as usize;

    MemoryView {
        static_bytes: (data + bss + retained) as u32,
        reserved_stack: (stack_high - stack_low) as u32,
        unassigned: stack_low.saturating_sub(bss_end) as u32,
    }
}

pub(crate) const PAINT: u32 = 0xA55A_C33C;
pub(crate) const GUARD: u32 = 0x6B8D_124F;
const GUARD_BYTES: usize = 16;
const MARGIN: usize = 96;

extern "C" {
    static __stack_top: u8;
    static __stack_limit: u8;
    static __main_stack_limit: u8;
    static __fault_stack_top: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
    static __noinit_start: u8;
    static __noinit_end: u8;
}

#[derive(Clone, Copy)]
pub(crate) enum Test {
    Fault,
    Watchdog,
    Reentry,
}

#[derive(Clone, Copy)]
pub(crate) struct FaultView {
    pub(crate) valid: bool,
    pub(crate) reason: u32,
    pub(crate) exception: u32,
    pub(crate) frame: bool,
    pub(crate) pc: u32,
    pub(crate) lr: u32,
    pub(crate) reset_flags: u32,
    pub(crate) published: u32,
    pub(crate) computed: u32,
    pub(crate) raw_reason: u32,
}

#[used]
#[no_mangle]
static mut TRAKT_LAST_FAULT: [u32; 16] = [0; 16];

/// CRC32 covers fifteen payload words in little-endian byte order. The
/// format salt participates in the publication checksum; zero publication
/// is invalid. Readers also validate the payload's field constraints.
#[inline(never)]
pub(crate) const fn seal(words: &[u32; 16]) -> u32 {
    let mut crc = u32::MAX;
    let mut index = 1;
    while index < words.len() {
        let mut value = words[index];
        let mut byte = 0;
        while byte < 4 {
            crc ^= value & 255;
            let mut bit = 0;
            while bit < 8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
                bit += 1;
            }
            value >>= 8;
            byte += 1;
        }
        index += 1;
    }
    !crc ^ 0x5446_4331
}

const fn valid(words: &[u32; 16]) -> bool {
    words[0] != 0 && words[0] == seal(words)
        && words[1] < 48
        && words[4] <= 1
        && words[13] & !3 == 0
        && words[14] <= 1
        && (words[4] == 0
            || matches!(words[2], 0xFFFF_FFF1 | 0xFFFF_FFF9 | 0xFFFF_FFFD))
}

/// Runs once after BSS initialization. Copying does not validate the
/// retained data; consumers check its publication word separately.
#[inline(never)]
pub(crate) fn capture_previous() {
    let words = startup::take_previous_fault();
    unsafe { write_volatile(addr_of_mut!(TRAKT_LAST_FAULT), words) };
}

#[inline(never)]
pub(crate) fn last_fault() -> FaultView {
    let words = unsafe { read_volatile(addr_of!(TRAKT_LAST_FAULT)) };
    let accepted = valid(&words);
    FaultView {
        valid: accepted,
        reason: if accepted { words[15] } else { 0 },
        exception: if accepted { words[1] } else { 0 },
        frame: accepted && words[4] == 1,
        pc: if accepted && words[4] == 1 { words[11] } else { 0 },
        lr: if accepted && words[4] == 1 { words[10] } else { 0 },
        reset_flags: unsafe { mcu::read(mcu::RCC + 0x60) },
        published: words[0],
        computed: seal(&words),
        raw_reason: words[15],
    }
}

struct Mark {
    start: usize,
    top: usize,
    cursor: usize,
    lowest: usize,
    sweeps: u32,
}

impl Mark {
    const fn new(start: usize, top: usize) -> Self {
        Self { start, top, cursor: start, lowest: top, sweeps: 0 }
    }

    const fn sample_pointer(&mut self, pointer: usize) {
        if pointer < self.lowest {
            self.lowest = pointer;
        }
    }

    const fn finish(&mut self) {
        self.cursor = self.start;
        self.sweeps = self.sweeps.saturating_add(1);
    }

    const fn observe(&mut self, value: u32) {
        if value != PAINT {
            self.sample_pointer(self.cursor);
            self.finish();
        } else {
            self.cursor += 4;
        }
    }

    const fn used(&self) -> u32 {
        self.top.saturating_sub(self.lowest) as u32
    }

    /// the bounds must refer to reset-painted stack RAM. `limit`
    /// must exclude live stack allocations. Interrupts must be masked.
    unsafe fn scan(&mut self, limit: usize) {
        for _ in 0..8 {
            let end = limit.min(self.lowest).min(self.top);
            if self.cursor >= end {
                self.finish();
                break;
            }

            let address = self.cursor;
            let value = unsafe { read_volatile(address as *const u32) };
            self.observe(value);
            if value != PAINT {
                break;
            }
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct StackState {
    signature: u32,
    tick: u32,
    main_used: u32,
    main_budget: u32,
    fault_used: u32,
    fault_budget: u32,
    main_free: u32,
    minimum_msp: u32,
    guards: u32,
    sweeps: u32,
    max_tick_gap: u32,
    fault_valid: u32,
    reset_flags: u32,
    last_reason: u32,
    guard_bytes: u32,
    reserved: u32,
}

#[used]
#[no_mangle]
static mut TRAKT_STACK: StackState = unsafe { core::mem::zeroed() };

#[derive(Clone, Copy)]
pub(crate) struct StackView {
    pub(crate) ready: bool,
    pub(crate) main_used: u32,
    pub(crate) main_budget: u32,
    pub(crate) main_free: u32,
    pub(crate) fault_used: u32,
    pub(crate) fault_budget: u32,
    pub(crate) guards: u32,
    pub(crate) max_tick_gap: u32,
}

pub(crate) fn stack_view() -> StackView {
    let state = unsafe { read_volatile(addr_of!(TRAKT_STACK)) };
    StackView {
        ready: state.signature == 0x5453_4D31,
        main_used: state.main_used,
        main_budget: state.main_budget,
        main_free: state.main_free,
        fault_used: state.fault_used,
        fault_budget: state.fault_budget,
        guards: state.guards,
        max_tick_gap: state.max_tick_gap,
    }
}

pub(crate) struct Monitor {
    main_base: usize,
    fault_base: usize,
    main: Mark,
    fault: Mark,
    minimum_msp: usize,
    last_tick: u32,
    max_tick_gap: u32,
    previous: FaultView,
}

impl Monitor {
    #[inline(never)]
    pub(crate) fn new() -> Self {
        let main_base = addr_of!(__main_stack_limit) as usize;
        let fault_base = addr_of!(__stack_limit) as usize;
        let main_top = addr_of!(__stack_top) as usize;
        let fault_top = addr_of!(__fault_stack_top) as usize;

        Self {
            main_base,
            fault_base,
            main: Mark::new(main_base + GUARD_BYTES, main_top),
            fault: Mark::new(fault_base + GUARD_BYTES, fault_top),
            minimum_msp: main_top,
            last_tick: 0,
            max_tick_gap: 0,
            previous: last_fault(),
        }
    }

    /// Runs before watchdog service. Damaged guards or a sampled stack
    /// below the reserved margin enter the common RF-off trap.
    #[inline(never)]
    pub(crate) fn poll(&mut self, now: u32) {
        let mask = mcu::InterruptMask::acquire();
        let pointer = mcu::stack_pointer() as usize;
        let main_guard = unsafe { guard_matches(self.main_base) };
        let fault_guard = unsafe { guard_matches(self.fault_base) };

        if !main_guard || !fault_guard
            || pointer < self.main_base + GUARD_BYTES + MARGIN
            || pointer > self.main.top
        {
            drop(mask);
            startup::stop(0x6102);
        }

        self.minimum_msp = self.minimum_msp.min(pointer);
        self.main.sample_pointer(pointer);

        // Stay below the observer's current allocation. The scan helpers
        // may use their own stack, so leave additional space below MSP.
        let scan_limit = pointer.saturating_sub(128);
        unsafe {
            self.main.scan(scan_limit);
            self.fault.scan(self.fault.top);
        }
        drop(mask);

        self.max_tick_gap = self.max_tick_gap.max(now.wrapping_sub(self.last_tick));
        self.last_tick = now;
        self.publish(now);
    }

    #[inline(never)]
    fn publish(&self, now: u32) {
        let main_budget = (self.main.top - self.main_base) as u32;
        let used = self.main.used();
        let state = StackState {
            signature: 0,
            tick: now,
            main_used: used,
            main_budget,
            fault_used: self.fault.used(),
            fault_budget: (self.fault.top - self.fault_base) as u32,
            main_free: main_budget.saturating_sub(used + GUARD_BYTES as u32),
            minimum_msp: self.minimum_msp as u32,
            guards: 3,
            sweeps: self.main.sweeps.saturating_add(self.fault.sweeps),
            max_tick_gap: self.max_tick_gap,
            fault_valid: self.previous.valid as u32,
            reset_flags: self.previous.reset_flags,
            last_reason: self.previous.reason,
            guard_bytes: GUARD_BYTES as u32,
            reserved: 0,
        };
        unsafe {
            write_volatile(addr_of_mut!(TRAKT_STACK.signature), 0);
            write_volatile(addr_of_mut!(TRAKT_STACK), state);
            asm!("dsb", options(nostack, preserves_flags));
            write_volatile(addr_of_mut!(TRAKT_STACK.signature), 0x5453_4D31);
        }
    }
}

/// the address must refer to the painted sixteen-byte stack guard.
unsafe fn guard_matches(address: usize) -> bool {
    for index in 0..4 {
        if unsafe { read_volatile((address as *const u32).add(index)) } != GUARD {
            return false;
        }
    }
    true
}

/// A sampled ISR stack check, not hardware overflow prevention.
/// The hardware frame and Rust prologue already exist when this runs.
pub(crate) fn interrupt_stack_check() {
    let lower = addr_of!(__main_stack_limit) as usize;
    let upper = addr_of!(__stack_top) as usize;
    let pointer = mcu::stack_pointer() as usize;

    if pointer < lower + GUARD_BYTES + MARGIN || pointer > upper
        || unsafe { read_volatile(lower as *const u32) } != GUARD
    {
        startup::stop(0x6102);
    }
}

/// The caller must first stop RX, disarm TX, and ensure storage is idle.
/// Each invocation terminates the current run and performs no Flash write.
#[cold]
#[inline(never)]
pub(crate) fn execute(test: Test) -> ! {
    unsafe {
        crate::board::park_outputs();
        crate::drivers::bk4829::inhibit();
    }
    if let Err(error) = crate::drivers::bk4829::verify() {
        startup::stop(error);
    }

    match test {
        Test::Fault => unsafe {
            asm!("udf #0", options(noreturn));
        },
        Test::Watchdog => {
            unsafe {
                asm!("cpsid i", options(nostack, preserves_flags));
                mcu::write(0xE000_E010, 0);
            }
            startup::remember_software(0x6101);
            mcu::park()
        }
        Test::Reentry => {
            unsafe {
                asm!("cpsid i", options(nostack, preserves_flags));
                mcu::write(0xE000_E010, 0);
                mcu::write(0x4000_3000, 0xAAAA);
            }
            startup::remember_software(0x6104);

            // Restart through the existing entry without an MCU reset.
            // Reset_Handler replaces MSP and never returns to these frames.
            // Peripheral reset flags remain unchanged; this is not an
            // alternative watchdog or an exception recovery mechanism.
            unsafe {
                asm!(
                    "dsb",
                    "isb",
                    "bl Reset_Handler",
                    options(noreturn),
                );
            }
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<StackState>() == 64);
    assert!(core::mem::size_of::<[u32; 16]>() == 64);

    let mut words = [0u32; 16];
    words[15] = 0x6101;
    words[14] = 1;
    words[0] = seal(&words);
    assert!(valid(&words));

    let mut index = 1;
    while index < 16 {
        let mut corrupted = words;
        corrupted[index] ^= 1;
        assert!(!valid(&corrupted));
        index += 1;
    }
    let mut incomplete = words;
    incomplete[0] = 0;
    assert!(!valid(&incomplete));
    incomplete[0] = 0x544C_4654;
    assert!(!valid(&incomplete));

    let mut frame = words;
    frame[1] = 3;
    frame[2] = 0xFFFF_FFF9;
    frame[4] = 1;
    frame[11] = 0x080028C0;
    frame[0] = seal(&frame);
    assert!(valid(&frame));
    frame[2] = 0;
    frame[0] = seal(&frame);
    assert!(!valid(&frame));

    let mut mark = Mark::new(16, 128);
    assert!(mark.used() == 0);
    mark.observe(PAINT);
    assert!(mark.cursor == 20);
    mark.observe(0);
    assert!(mark.used() == 108);
    assert!(mark.cursor == 16);
    mark.sample_pointer(64);
    assert!(mark.used() == 108);
    mark.sample_pointer(18);
    assert!(mark.used() == 110);

    let mut sampled = Mark::new(16, 128);
    sampled.sample_pointer(80);
    assert!(sampled.used() == 48);
    sampled.sample_pointer(100);
    assert!(sampled.used() == 48);
};