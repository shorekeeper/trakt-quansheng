//! Compact compiler memory-copy entries for Cortex-M0+ on PY32F071.
//!
//! Rust aggregate movement and slice copying can emit C or ARM EABI memory
//! calls. All supported copy and move entries use one byte-transfer loop.
//! EABI aligned variants retain their ordinary contract but do not require
//! another implementation: byte accesses also satisfy aligned requests.
//!
//! C memcpy and memmove return the destination pointer. ARM EABI memcpy,
//! memcpy4, memcpy8, memmove, memmove4, and memmove8 return no value. The
//! exported wrappers preserve those distinct calling conventions.
//!
//! Moving a destination above an overlapping source copies backward.
//! Nonoverlapping regions and destinations below the source copy forward.
//! Equal pointers and zero length perform no access. No length rounding,
//! aligned overread, temporary buffer, allocation, or source mutation occurs.
//!
//! Source and destination must identify valid ordinary memory for the
//! complete requested length. Every source byte must be readable and every
//! destination byte writable. Pointer arithmetic must remain within their
//! allocated regions. No concurrent writer may access the copied storage.
//! These entries are not peripheral, DMA synchronization, or atomic-copy APIs.
//!
//! Volatile byte accesses prevent LLVM from converting the implementation
//! into a call to the same memory intrinsic. They do not provide inter-thread
//! synchronization. Copying deliberately favors shared small code rather
//! than word-aligned acceleration. Execution time requires measurement with
//! the firmware's existing foreground timing and stack observations.
//!
//! Test builds do not export libc or ARM memory symbols, so Windows test
//! support retains its own runtime implementation. Tests execute this exact
//! transfer loop directly against independent byte-array expectations.
//! Startup's assembly data initialization and all memset entries are unchanged.

use core::ptr::{read_volatile, write_volatile};

/// Perform one complete movement without constructing overlapping Rust
/// references. The caller establishes the ordinary-memory bounds described
/// by this module before entry. An empty request may use null pointers.
#[inline(never)]
unsafe fn move_bytes(destination: *mut u8, source: *const u8, length: usize) {
    if length == 0 || destination as *const u8 == source {
        return;
    }

    let target = destination as usize;
    let origin = source as usize;
    let backward = target > origin && target.wrapping_sub(origin) < length;

    if backward {
        let mut remaining = length;
        while remaining != 0 {
            remaining -= 1;
            unsafe {
                let value = read_volatile(source.add(remaining));
                write_volatile(destination.add(remaining), value);
            }
        }
    } else {
        let mut index = 0;
        while index < length {
            unsafe {
                let value = read_volatile(source.add(index));
                write_volatile(destination.add(index), value);
            }
            index += 1;
        }
    }
}

/// C move entry permits overlap and returns the original destination.
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn memmove(
    destination: *mut u8,
    source: *const u8,
    length: usize,
) -> *mut u8 {
    unsafe { move_bytes(destination, source, length) };
    destination
}

/// C copy callers provide disjoint regions. Sharing the overlap-capable
/// implementation does not weaken that caller contract.
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn memcpy(
    destination: *mut u8,
    source: *const u8,
    length: usize,
) -> *mut u8 {
    unsafe { move_bytes(destination, source, length) };
    destination
}

// LLVM may select any of these ARM EABI entries for aggregate movement.
// The four/eight suffix promises pointer alignment, not a rounded length.
// All wrappers preserve byte-exact movement and the EABI void return.
#[cfg(not(test))]
macro_rules! eabi_entry {
    ($name:ident) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(
            destination: *mut u8,
            source: *const u8,
            length: usize,
        ) {
            unsafe { move_bytes(destination, source, length) };
        }
    };
}

#[cfg(not(test))]
eabi_entry!(__aeabi_memcpy);
#[cfg(not(test))]
eabi_entry!(__aeabi_memcpy4);
#[cfg(not(test))]
eabi_entry!(__aeabi_memcpy8);
#[cfg(not(test))]
eabi_entry!(__aeabi_memmove);
#[cfg(not(test))]
eabi_entry!(__aeabi_memmove4);
#[cfg(not(test))]
eabi_entry!(__aeabi_memmove8);

#[cfg(test)]
mod tests {
    use super::move_bytes;

    fn pattern<const N: usize>() -> [u8; N] {
        let mut bytes = [0u8; N];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(73)
                ^ ((index / 256) as u8).wrapping_mul(29)
                ^ 0xA5;
        }
        bytes
    }

    #[test]
    fn every_small_overlap_and_alignment_matches_detached_expectation() {
        const LENGTH: usize = 32;
        let original = pattern::<LENGTH>();

        for count in 0..=LENGTH {
            for source in 0..=LENGTH - count {
                for destination in 0..=LENGTH - count {
                    let mut actual = original;
                    let mut expected = original;

                    // Expected bytes come from the untouched source image,
                    // not from another overlap-copy implementation.
                    for index in 0..count {
                        expected[destination + index] = original[source + index];
                    }

                    let pointer = actual.as_mut_ptr();
                    unsafe {
                        move_bytes(
                            pointer.add(destination),
                            pointer.add(source),
                            count,
                        );
                    }

                    assert_eq!(
                        actual, expected,
                        "source={source} destination={destination} count={count}",
                    );
                }
            }
        }
    }

    #[test]
    fn firmware_sized_regions_preserve_every_surrounding_byte() {
        let original = pattern::<2048>();
        let counts = [1usize, 3, 4, 7, 8, 20, 64, 320, 448, 512, 592, 1024];

        for count in counts {
            for (source, destination) in [
                (0usize, 1usize),
                (1, 0),
                (3, 7),
                (7, 3),
                (0, 1024),
                (1024, 0),
                (17, 17),
            ] {
                let mut actual = original;
                let mut expected = original;

                for index in 0..count {
                    expected[destination + index] = original[source + index];
                }

                let pointer = actual.as_mut_ptr();
                unsafe {
                    move_bytes(
                        pointer.add(destination),
                        pointer.add(source),
                        count,
                    );
                }

                assert_eq!(
                    actual, expected,
                    "source={source} destination={destination} count={count}",
                );
            }
        }
    }

    #[test]
    fn copying_between_distinct_arrays_is_byte_exact() {
        let source = pattern::<1024>();
        let mut destination = [0xCCu8; 1024];

        unsafe {
            move_bytes(
                destination.as_mut_ptr(),
                source.as_ptr(),
                source.len(),
            );
        }

        assert_eq!(destination, source);
        assert_eq!(source, pattern::<1024>());
    }

    #[test]
    fn empty_null_request_performs_no_access() {
        unsafe {
            move_bytes(core::ptr::null_mut(), core::ptr::null(), 0);
        }
    }
}