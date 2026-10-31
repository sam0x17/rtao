//! Shared helpers: leaked heaps, a deterministic PRNG and content patterns.

#![allow(dead_code, unsafe_code, clippy::unwrap_used)]

use core::alloc::Layout;

use rtao_arena::{Config, FixedHeap};

/// A heap of `bytes` with the given floors, leaked for the test's lifetime.
pub fn heap(bytes: usize, small_floor: usize, large_floor: usize) -> &'static FixedHeap {
    let heap: &'static FixedHeap = Box::leak(Box::new(FixedHeap::new(Config::new(
        small_floor,
        large_floor,
    ))));
    heap.init_static(Box::leak(vec![0u8; bytes].into_boxed_slice()))
        .unwrap();
    heap
}

/// SplitMix64: small, fast and deterministic.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() % (hi - lo + 1) as u64) as usize
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// Fills `len` bytes at `ptr` with a pattern derived from `seed`.
pub fn fill(ptr: *mut u8, len: usize, seed: u64) {
    for i in 0..len {
        // SAFETY: the caller owns `len` bytes at `ptr`.
        unsafe { ptr.add(i).write(pattern(seed, i)) };
    }
}

/// Checks the pattern written by [`fill`] over the first `len` bytes.
pub fn check(ptr: *const u8, len: usize, seed: u64) {
    for i in 0..len {
        // SAFETY: the caller owns `len` initialized bytes at `ptr`.
        let got = unsafe { ptr.add(i).read() };
        assert_eq!(got, pattern(seed, i), "byte {i} of {len} (seed {seed})");
    }
}

fn pattern(seed: u64, i: usize) -> u8 {
    (seed as u8)
        .wrapping_mul(31)
        .wrapping_add(i as u8)
        .wrapping_add((i >> 8) as u8)
}

pub fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}
