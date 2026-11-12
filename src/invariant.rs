//! Shared failure entry for explicit firmware invariants.
//!
//! The firmware panic handler ignores PanicInfo and enters the common trap
//! with reason 1003. Per-call message strings and source locations therefore
//! carry no operational information. Explicit assertion and panic macros
//! route here without constructing separate messages at their call sites.
//!
//! This function deliberately has no track_caller attribute and is not
//! inlined. Its single core panic site supplies the same terminal behavior
//! to runtime callers and remains evaluable by compiler constant fixtures.
//! A failed constant invariant still rejects compilation.
//!
//! Assertions retain their complete condition and evaluate it exactly once.
//! No successful check invokes this function. Compiler-generated overflow,
//! bounds, division, and other core checks retain their ordinary handling;
//! this module neither suppresses them nor changes compilation flags.
//!
//! Standalone Windows test harnesses do not declare the firmware macros.
//! They compile the production modules with standard assertion diagnostics.
//! Runtime fault records retain the existing generic panic reason rather
//! than pretending to identify which invariant failed.

/// Non-returning explicit invariant failure. The common panic handler owns
/// RF shutdown, retained fault recording, and stopped watchdog service.
#[cold]
#[inline(never)]
pub(crate) const fn fail() -> ! {
    core::panic!("firmware invariant failed");
}