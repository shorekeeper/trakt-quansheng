//! LCD initialization and full-frame transmission.
//!
//! Each page begins at controller column four, followed by 128 bytes.
//! All eight pages are written before display-on and backlight-on.
//! The power sequence uses 2B, 2E, and four 2F commands with fixed delays.
//! No hardware reset pin or display readback is assumed.
//!
//! A pump performs at most 64 transport steps. A frame has a 1000-tick
//! deadline in addition to the transport's byte deadline. Failure releases
//! CS, disables SPI, turns off the backlight, and is not retried.
//!
//! TRAKT_DISPLAY reports completed wire operations and elapsed tick counts.
//! Successful SPI transfers do not prove that the LCD accepted their data.



use crate::app::time::reached;
use crate::mcu::{self, spi::{Progress, Spi}};
use crate::ui::Frame;

const PAGE_BYTES: u16 = 131;
const FRAME_BYTES: u16 = PAGE_BYTES * 8;

#[derive(Clone, Copy)]
enum Instruction {
    Command(u8),
    Delay(u32),
    Frame,
}

const SCRIPT: [Instruction; 23] = [
    Instruction::Command(0xAE),
    Instruction::Command(0xE2),
    Instruction::Delay(120),
    Instruction::Command(0xA2),
    Instruction::Command(0xC0),
    Instruction::Command(0xA1),
    Instruction::Command(0xA6),
    Instruction::Command(0xA4),
    Instruction::Command(0x24),
    Instruction::Command(0x81),
    Instruction::Command(31),
    Instruction::Command(0x2B),
    Instruction::Delay(1),
    Instruction::Command(0x2E),
    Instruction::Delay(1),
    Instruction::Command(0x2F),
    Instruction::Command(0x2F),
    Instruction::Command(0x2F),
    Instruction::Command(0x2F),
    Instruction::Delay(40),
    Instruction::Command(0x40),
    Instruction::Frame,
    Instruction::Command(0xAF),
];

#[repr(u32)]
#[derive(Clone, Copy)]
enum Phase {
    Script = 0,
    Delay = 1,
    Frame = 2,
    Idle = 3,
    Dead = 4,
}

#[derive(Clone, Copy)]
enum Action {
    Command(u8),
    Pixel(usize),
}

struct Sequence {
    phase: Phase,
    instruction: usize,
    cursor: u16,
    deadline: u32,
    initial_frame: bool,
}

impl Sequence {
    const fn new() -> Self {
        Self {
            phase: Phase::Script,
            instruction: 0,
            cursor: 0,
            deadline: 0,
            initial_frame: true,
        }
    }

    const fn ready(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    const fn in_frame(&self) -> bool {
        matches!(self.phase, Phase::Frame)
    }

    const fn frame_action(&self) -> Action {
        let page = self.cursor / PAGE_BYTES;
        let column = self.cursor % PAGE_BYTES;
        match column {
            0 => Action::Command(0xB0 | page as u8),
            1 => Action::Command(0x10),
            2 => Action::Command(0x04),
            _ => Action::Pixel(page as usize * 128 + column as usize - 3),
        }
    }

    const fn action(&mut self, now: u32) -> Option<Action> {
        match self.phase {
            Phase::Script => match SCRIPT[self.instruction] {
                Instruction::Command(value) => Some(Action::Command(value)),
                Instruction::Delay(ticks) => {
                    self.deadline = now.wrapping_add(ticks);
                    self.instruction += 1;
                    self.phase = Phase::Delay;
                    None
                }
                Instruction::Frame => {
                    self.cursor = 0;
                    self.instruction += 1;
                    self.phase = Phase::Frame;
                    Some(self.frame_action())
                }
            },
            Phase::Delay => {
                if reached(now, self.deadline) {
                    self.phase = Phase::Script;
                }
                None
            }
            Phase::Frame => Some(self.frame_action()),
            Phase::Idle | Phase::Dead => None,
        }
    }

    /// Bit zero reports a completed frame; bit one reports completion of
    /// initialization, including the final display-on command.
    const fn complete(&mut self) -> u32 {
        match self.phase {
            Phase::Script => {
                self.instruction += 1;
                if self.instruction == SCRIPT.len() {
                    self.phase = Phase::Idle;
                    2
                } else {
                    0
                }
            }
            Phase::Frame => {
                self.cursor += 1;
                if self.cursor == FRAME_BYTES {
                    self.phase = if self.initial_frame { Phase::Script } else { Phase::Idle };
                    1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    const fn request(&mut self) -> bool {
        if !self.ready() {
            return false;
        }
        self.initial_frame = false;
        self.cursor = 0;
        self.phase = Phase::Frame;
        true
    }
}

#[repr(C)]
struct DisplayState {
    signature: u32,
    tick: u32,
    phase: u32,
    frames: u32,
    bytes: u32,
    failures: u32,
    last_error: u32,
    frame_ticks: u32,
    max_frame_ticks: u32,
    spi_hz: u32,
    rendered_tick: u32,
    frame_active: u32,
    reserved: [u32; 4],
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_DISPLAY: DisplayState = unsafe { core::mem::zeroed() };

pub(crate) struct Display {
    spi: Spi,
    sequence: Sequence,
    spi_hz: u32,
    frame_active: bool,
    frame_start: u32,
    frame_deadline: u32,
    frames: u32,
    bytes: u32,
    failures: u32,
    last_error: u32,
    frame_ticks: u32,
    max_frame_ticks: u32,
}

impl Display {
    /// Takes ownership of SPI1, PA5, PA7, PA6, PB2, and PF8.
    ///
    /// these pins must belong to this foreground driver and their
    /// GPIO clocks must be available. No second display may use SPI1.
    pub(crate) unsafe fn initialize(pclk: u32) -> Option<Self> {
        unsafe {
            mcu::output(mcu::GPIO_B, 2, true);
            mcu::output(mcu::GPIO_A, 6, false);
            mcu::output(mcu::GPIO_F, 8, false);
        }
        let spi = unsafe { Spi::initialize(pclk)? };
        Some(Self {
            spi,
            sequence: Sequence::new(),
            spi_hz: pclk / 64,
            frame_active: false,
            frame_start: 0,
            frame_deadline: 0,
            frames: 0,
            bytes: 0,
            failures: 0,
            last_error: 0,
            frame_ticks: 0,
            max_frame_ticks: 0,
        })
    }

    pub(crate) fn ready(&self) -> bool {
        self.sequence.ready()
    }

    pub(crate) fn phase(&self) -> u32 {
        self.sequence.phase as u32
    }

    pub(crate) fn frames(&self) -> u32 {
        self.frames
    }

    /// Backlight remains off during initialization and after transport failure.
    pub(crate) fn backlight(&self, requested: bool) {
        let initialized = self.sequence.ready() || !self.sequence.initial_frame;
        let on = requested && initialized && !matches!(self.sequence.phase, Phase::Dead);
        unsafe {
            mcu::write(mcu::GPIO_F + 0x18, if on { 1 << 8 } else { 1 << 24 });
        }
    }

    /// Requests transmission of an already rendered, stable framebuffer.
    pub(crate) fn request(&mut self) -> bool {
        self.sequence.request()
    }

    fn release(&self) {
        unsafe { mcu::write(mcu::GPIO_B + 0x18, 1 << 2) };
    }

    fn fail(&mut self, error: u32) {
        self.spi.disable();
        self.release();
        unsafe { mcu::write(mcu::GPIO_F + 0x18, 1 << 24) };
        self.sequence.phase = Phase::Dead;
        self.frame_active = false;
        self.failures = self.failures.saturating_add(1);
        self.last_error = error;
    }

    /// The caller must keep the frame unchanged until ready becomes true.
    /// A failed display remains unavailable and accepts no further frames.
    pub(crate) fn pump(&mut self, now: u32, frame: &Frame) {
        if matches!(self.sequence.phase, Phase::Dead) {
            return;
        }

        for _ in 0..64 {
            if self.frame_active && reached(now, self.frame_deadline) {
                self.fail(3);
                return;
            }

            let action = match self.sequence.action(now) {
                Some(action) => action,
                None => {
                    self.release();
                    return;
                }
            };

            if self.sequence.in_frame() && !self.frame_active {
                self.frame_active = true;
                self.frame_start = now;
                self.frame_deadline = now.wrapping_add(1_000);
            }

            let (data, value) = match action {
                Action::Command(value) => (false, value),
                Action::Pixel(index) => (true, frame.byte(index)),
            };

            if self.spi.idle() {
                unsafe {
                    mcu::write(mcu::GPIO_A + 0x18, if data { 1 << 6 } else { 1 << 22 });
                    mcu::write(mcu::GPIO_B + 0x18, 1 << 18);
                }
            }

            match self.spi.poll(value, now) {
                Progress::Pending => {}
                Progress::Failed(error) => {
                    self.fail(error);
                    return;
                }
                Progress::Complete(_) => {
                    self.bytes = self.bytes.saturating_add(1);
                    let completion = self.sequence.complete();
                    if completion & 1 != 0 {
                        self.frames = self.frames.saturating_add(1);
                        self.frame_ticks = now.wrapping_sub(self.frame_start);
                        self.max_frame_ticks = self.max_frame_ticks.max(self.frame_ticks);
                        self.frame_active = false;
                    }
                    if completion & 2 != 0 {
                        self.release();
                        unsafe { mcu::write(mcu::GPIO_F + 0x18, 1 << 8) };
                    }
                }
            }
        }
    }

    pub(crate) fn publish(&self, now: u32, rendered_tick: u32) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let state = DisplayState {
            signature: 0,
            tick: now,
            phase: self.phase(),
            frames: self.frames,
            bytes: self.bytes,
            failures: self.failures,
            last_error: self.last_error,
            frame_ticks: self.frame_ticks,
            max_frame_ticks: self.max_frame_ticks,
            spi_hz: self.spi_hz,
            rendered_tick,
            frame_active: self.frame_active as u32,
            reserved: [0; 4],
        };
        crate::observation::publish!(TRAKT_DISPLAY, state, 0x5444_4354);
    }
}

const _: () = {
    assert!(core::mem::size_of::<DisplayState>() == 64);
    assert!(FRAME_BYTES == 1_048);

    let mut sequence = Sequence::new();
    assert!(!sequence.request());
    assert!(matches!(sequence.action(0), Some(Action::Command(0xAE))));
    assert!(sequence.complete() == 0);
    assert!(matches!(sequence.action(0), Some(Action::Command(0xE2))));
    assert!(sequence.complete() == 0);
    assert!(sequence.action(0xFFFF_FFF0).is_none());
    assert!(sequence.action(0xFFFF_FFFF).is_none());
    assert!(sequence.action(103).is_none());
    assert!(matches!(sequence.phase, Phase::Delay));
    assert!(sequence.action(104).is_none());
    assert!(matches!(sequence.phase, Phase::Script));
    assert!(matches!(sequence.action(104), Some(Action::Command(0xA2))));

    let mut full = Sequence::new();
    let mut now = 0u32;
    let mut steps = 0;
    let mut pixels = 0;
    let mut frame_completions = 0;
    let mut initialized = false;

    while !full.ready() {
        assert!(steps < 2_000);
        match full.action(now) {
            Some(Action::Command(value)) => {
                if value == 0xAF {
                    assert!(pixels == 1_024);
                    assert!(frame_completions == 1);
                }
                let result = full.complete();
                if result & 1 != 0 {
                    frame_completions += 1;
                }
                if result & 2 != 0 {
                    initialized = true;
                }
            }
            Some(Action::Pixel(index)) => {
                assert!(index == pixels);
                pixels += 1;
                let result = full.complete();
                if result & 1 != 0 {
                    frame_completions += 1;
                }
            }
            None => now = now.wrapping_add(1),
        }
        steps += 1;
    }
    assert!(initialized);
    assert!(pixels == 1_024);
    assert!(frame_completions == 1);
    assert!(full.request());
    assert!(!full.request());

    full.cursor = 130;
    assert!(matches!(full.action(0), Some(Action::Pixel(127))));
    assert!(full.complete() == 0);
    assert!(matches!(full.action(0), Some(Action::Command(0xB1))));
    assert!(full.complete() == 0);
    assert!(matches!(full.action(0), Some(Action::Command(0x10))));
    assert!(full.complete() == 0);
    assert!(matches!(full.action(0), Some(Action::Command(0x04))));
    assert!(full.complete() == 0);
    assert!(matches!(full.action(0), Some(Action::Pixel(128))));

    full.cursor = FRAME_BYTES - 1;
    assert!(matches!(full.action(0), Some(Action::Pixel(1_023))));
    assert!(full.complete() == 1);
    assert!(full.ready());
};