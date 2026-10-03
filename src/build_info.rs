//! Build identity displayed by boot presentation and About.
//!
//! The build command reserves this identity before compilation. Failed
//! builds retain their reserved number. Explicit ignore-increment selects
//! the current identity without allocating another number.

pub(crate) const VERSION: &[u8] = b"0.4.5";
pub(crate) const LABEL: &[u8] = b"Build Tachyon.5";
