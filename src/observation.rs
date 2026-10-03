//! Compile-time publication of optional debugger records.
//!
//! Each optional static is declared only with the debug-tools feature.
//! Publishers return before constructing a snapshot when that feature is
//! absent. Their snapshot expressions remain type-checked in both variants;
//! publication then consumes the value without requiring a static object.
//! An unavailable optional reader returns its explicitly supplied empty view.
//!
//! Publication clears the signature, writes the fifteen payload words,
//! executes DSB, and publishes the signature last. All optional records use
//! one runtime writer instead of a separate aggregate-write implementation
//! for each record type. Their debugger representation remains unchanged.
//!
//! Admitted records are repr(C) structures containing exactly sixteen u32
//! words, including fixed u32 arrays. Signature is their first field. These
//! concrete layouts contain no padding, references, enums, or drop state.
//! The macro checks matching source/destination types and equal representation
//! size during compilation. Adding another field type requires reviewing
//! initialization, padding, alignment, and word order before using this path.
//!
//! The writer borrows a local word array only for the call and retains no
//! pointer. No static staging buffer or interrupt masking is introduced.
//! Publication is not atomic and is not a concurrent Rust snapshot protocol.
//!
//! Callers retain their record's execution-context ownership. The macros
//! create raw pointers, not references to mutable statics. Normal records
//! have one foreground writer; startup publication runs before interrupts.
//!
//! Fault capture, stack guards, the stack watermark, TX cutoff state, and
//! operational buffers do not use this optional publication path.

/// Publish one optional diagnostic record through aligned word stores.
///
/// destination must address the complete sixteen-word writable record owned
/// exclusively by the caller. The local source must not overlap it. Neither
/// an ISR nor another Rust context may read or modify the destination while
/// publication is in progress. Debugger observation is not synchronization.
///
/// Word zero is controlled here rather than copied from the source. Clearing
/// it precedes every payload store. DSB completes those stores before the
/// final signature is published. The function does not access RF, refresh a
/// watchdog, change interrupt admission, or retain either pointer.
#[cfg(feature = "debug-tools")]
#[inline(never)]
pub(crate) unsafe fn write_record(
    destination: *mut u32,
    words: &[u32; 16],
    signature: u32,
) {
    unsafe {
        core::ptr::write_volatile(destination, 0);

        let mut index = 1;
        while index < words.len() {
            core::ptr::write_volatile(destination.add(index), words[index]);
            index += 1;
        }

        core::arch::asm!("dsb", options(nostack, preserves_flags));
        core::ptr::write_volatile(destination, signature);
    }
}

macro_rules! publish {
    ($record:ident, $state:expr, $signature:expr) => {{
        #[cfg(feature = "debug-tools")]
        {
            let state = $state;
            unsafe {
                // Both pointers must have the same pointee type. This
                // discarded array checks the call-site pairing without
                // dereferencing or creating a reference to the static.
                let _: [*const _; 2] = [
                    core::ptr::addr_of!(state),
                    core::ptr::addr_of!($record),
                ];

                // Every admitted concrete record contains only initialized
                // u32 fields and arrays in repr(C) order. Transmute checks
                // its exact size; the field contract excludes padding.
                let words = core::mem::transmute::<_, [u32; 16]>(state);

                crate::observation::write_record(
                    core::ptr::addr_of_mut!($record).cast::<u32>(),
                    &words,
                    $signature,
                );
            }
        }

        #[cfg(not(feature = "debug-tools"))]
        {
            let _ = $state;
        }
    }};
}

macro_rules! read {
    ($record:ident, $empty:expr) => {{
        #[cfg(feature = "debug-tools")]
        {
            unsafe {
                core::ptr::read_volatile(core::ptr::addr_of!($record))
            }
        }

        #[cfg(not(feature = "debug-tools"))]
        {
            $empty
        }
    }};
}

pub(crate) use publish;
pub(crate) use read;