#![deny(warnings)]

//! Host execution of the production Link packet and application state.
//!
//! Only the pure Link module is compiled. No RF register code, device,
//! external dependency, Cargo build, or firmware identity is involved.

#[path = "../src/link.rs"]
mod link;