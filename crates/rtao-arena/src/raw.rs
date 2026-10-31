//! Raw word access to the managed region.
//!
//! Every position inside the region is handled as an address (`usize`) and
//! turned back into a pointer through the region's base pointer, so all
//! pointers carry the provenance of the region the heap was given.

#![allow(unsafe_code)]

use core::ptr;

/// The managed region's base pointer, used to materialize addresses.
#[derive(Clone, Copy)]
pub(crate) struct Mem {
    base: *mut u8,
}

impl Mem {
    /// A placeholder used before the heap is initialized; never dereferenced.
    pub(crate) const fn none() -> Self {
        Self {
            base: ptr::null_mut(),
        }
    }

    pub(crate) const fn new(base: *mut u8) -> Self {
        Self { base }
    }

    /// The pointer for `addr`, carrying the region's provenance.
    #[inline]
    pub(crate) fn ptr(self, addr: usize) -> *mut u8 {
        self.base.with_addr(addr)
    }

    /// Reads the word at `addr`.
    ///
    /// # Safety
    ///
    /// `addr` must be word-aligned and inside the region.
    #[inline]
    pub(crate) unsafe fn word(self, addr: usize) -> usize {
        // SAFETY: the caller guarantees an aligned in-region address.
        unsafe { self.ptr(addr).cast::<usize>().read() }
    }

    /// Writes the word at `addr`.
    ///
    /// # Safety
    ///
    /// `addr` must be word-aligned, inside the region and owned by the heap.
    #[inline]
    pub(crate) unsafe fn set_word(self, addr: usize, value: usize) {
        // SAFETY: the caller guarantees an aligned in-region address the heap owns.
        unsafe { self.ptr(addr).cast::<usize>().write(value) }
    }

    /// Reads the `u32` at `addr`.
    ///
    /// # Safety
    ///
    /// `addr` must be 4-byte aligned and inside the region.
    #[inline]
    pub(crate) unsafe fn u32(self, addr: usize) -> u32 {
        // SAFETY: the caller guarantees an aligned in-region address.
        unsafe { self.ptr(addr).cast::<u32>().read() }
    }

    /// Writes the `u32` at `addr`.
    ///
    /// # Safety
    ///
    /// `addr` must be 4-byte aligned, inside the region and owned by the heap.
    #[inline]
    pub(crate) unsafe fn set_u32(self, addr: usize, value: u32) {
        // SAFETY: the caller guarantees an aligned in-region address the heap owns.
        unsafe { self.ptr(addr).cast::<u32>().write(value) }
    }

    /// Copies `len` bytes between two non-overlapping in-region ranges.
    ///
    /// # Safety
    ///
    /// Both ranges must be inside the region and must not overlap.
    #[inline]
    pub(crate) unsafe fn copy(self, from: usize, to: usize, len: usize) {
        // SAFETY: the caller guarantees in-region, non-overlapping ranges.
        unsafe { ptr::copy_nonoverlapping(self.ptr(from), self.ptr(to), len) }
    }

    /// Moves `len` bytes between two in-region ranges that may overlap.
    ///
    /// # Safety
    ///
    /// Both ranges must be inside the region.
    #[inline]
    pub(crate) unsafe fn move_bytes(self, from: usize, to: usize, len: usize) {
        // SAFETY: the caller guarantees in-region ranges; `copy` allows overlap.
        unsafe { ptr::copy(self.ptr(from), self.ptr(to), len) }
    }
}

/// Rounds `value` up to a multiple of `align` (a power of two).
#[inline]
pub(crate) const fn align_up(value: usize, align: usize) -> Option<usize> {
    match value.checked_add(align - 1) {
        Some(v) => Some(v & !(align - 1)),
        None => None,
    }
}

/// Rounds `value` down to a multiple of `align` (a power of two).
#[inline]
pub(crate) const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}
