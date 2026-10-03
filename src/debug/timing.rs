//! Foreground work measured in observed SysTick increments.
//!
//! Start is the tick returned by the foreground wait. Finish is sampled
//! after normal work, stack checking, and watchdog service. The recorder
//! itself and the subsequent sleep are excluded.
//!
//! Differences include interrupts that ran during work. They do not include
//! timer periods lost while interrupts were masked, and zero does not mean
//! zero execution time. These counters are not WCET or CPU-cycle measurements.
//!
//! GAP is the separation between consecutive foreground starts. LONG counts
//! passes spanning at least ten observed ticks. Counts saturate rather than
//! wrapping; elapsed differences retain the counter's modular arithmetic.
//! Only the foreground writes this record. The debug-tools feature controls
//! its storage and finish-time sampling. Without that feature, view returns
//! an unavailable zero state; no timing record is allocated or published.



#[repr(C)]
#[derive(Clone, Copy)]
struct TimingState {
    signature: u32,
    tick: u32,
    passes: u32,
    last_work: u32,
    max_work: u32,
    max_gap: u32,
    long_passes: u32,
    reserved: [u32; 9],
}

impl TimingState {
    const fn empty() -> Self {
        Self {
            signature: 0,
            tick: 0,
            passes: 0,
            last_work: 0,
            max_work: 0,
            max_gap: 0,
            long_passes: 0,
            reserved: [0; 9],
        }
    }

    const fn next(mut self, start: u32, finish: u32) -> Self {
        if self.passes != 0 {
            let gap = start.wrapping_sub(self.tick);
            if gap > self.max_gap {
                self.max_gap = gap;
            }
        }

        self.last_work = finish.wrapping_sub(start);
        if self.last_work > self.max_work {
            self.max_work = self.last_work;
        }
        if self.last_work >= 10 {
            self.long_passes = self.long_passes.saturating_add(1);
        }

        self.tick = start;
        self.passes = self.passes.saturating_add(1);
        self.signature = 0;
        self
    }
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_TIMING: TimingState = TimingState::empty();

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) ready: bool,
    pub(crate) passes: u32,
    pub(crate) last_work: u32,
    pub(crate) max_work: u32,
    pub(crate) max_gap: u32,
    pub(crate) long_passes: u32,
}

pub(crate) fn view() -> View {
    let state = crate::observation::read!(TRAKT_TIMING, TimingState::empty());
    View {
        ready: state.signature == 0x5450_4331,
        passes: state.passes,
        last_work: state.last_work,
        max_work: state.max_work,
        max_gap: state.max_gap,
        long_passes: state.long_passes,
    }
}

#[inline(never)]
pub(crate) fn record(start: u32, finish: u32) {
    if !cfg!(feature = "debug-tools") {
        return;
    }

    let previous = crate::observation::read!(TRAKT_TIMING, TimingState::empty());
    let state = previous.next(start, finish);

    crate::observation::publish!(TRAKT_TIMING, state, 0x5450_4331);
}

const _: () = {
    assert!(core::mem::size_of::<TimingState>() == 64);

    let first = TimingState::empty().next(20, 23);
    assert!(first.passes == 1);
    assert!(first.last_work == 3 && first.max_work == 3);
    assert!(first.max_gap == 0 && first.long_passes == 0);

    let second = first.next(25, 37);
    assert!(second.passes == 2);
    assert!(second.last_work == 12 && second.max_work == 12);
    assert!(second.max_gap == 5 && second.long_passes == 1);

    let third = second.next(40, 40);
    assert!(third.last_work == 0);
    assert!(third.max_work == 12);
    assert!(third.max_gap == 15);

    let wrapped = TimingState::empty().next(u32::MAX - 1, 2);
    assert!(wrapped.last_work == 4);
    let next = wrapped.next(3, 4);
    assert!(next.max_gap == 5 && next.last_work == 1);

    let saturated = TimingState {
        passes: u32::MAX,
        long_passes: u32::MAX,
        ..TimingState::empty()
    }.next(10, 20);
    assert!(saturated.passes == u32::MAX);
    assert!(saturated.long_passes == u32::MAX);
};