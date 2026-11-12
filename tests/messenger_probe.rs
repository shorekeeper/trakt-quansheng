#![deny(warnings)]

//! Host execution of the production messenger codec.
//!
//! rustc selects its installed Windows host target explicitly. This harness
//! uses the standard Rust test support, no Cargo dependency, and no device I/O.
//! The firmware module is compiled from its actual source path.

#[path = "../src/messenger/mod.rs"]
mod messenger;

#[test]
fn production_core_check_covers_runtime_seeds() {
    for seed in [
        0u32, 1, u32::MAX, 24_000_000, 48_000_000, 72_000_000,
        0x1357_9BDF, 0x9E37_79B9,
    ] {
        assert!(messenger::check_core(seed));
    }
}