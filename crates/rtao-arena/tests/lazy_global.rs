//! A lazy heap as a native test binary's global allocator: the region (a
//! `static` array) is set up at the first allocation, which the runtime
//! makes before `main`, and every allocation after it (the test harness,
//! its threads, these tests) lands in it.

#![cfg(feature = "std")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use core::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use rtao_arena::{Config, FixedHeap};

const LEN: usize = 64 << 20;

#[repr(align(4096))]
struct Region(UnsafeCell<[u8; LEN]>);
// SAFETY: touched only by `HEAP`, under its lock.
unsafe impl Sync for Region {}
static REGION: Region = Region(UnsafeCell::new([0; LEN]));
static CALLS: AtomicUsize = AtomicUsize::new(0);

fn region() -> Option<(*mut u8, usize)> {
    CALLS.fetch_add(1, Ordering::SeqCst);
    Some((REGION.0.get().cast(), LEN))
}

// SAFETY: `REGION` is this heap's alone, `region` does not allocate, and the
// heap is a static.
#[global_allocator]
static HEAP: FixedHeap = unsafe { FixedHeap::lazy(Config::new(8 << 20, 8 << 20), region) };

#[test]
fn every_allocation_lands_in_the_static_region() {
    assert_eq!(CALLS.load(Ordering::SeqCst), 1, "asked once, before main");
    let before = HEAP.stats();
    assert!(
        before.small.live + before.large.live > 0,
        "pre-main allocations"
    );
    let v: Vec<u64> = (0..1 << 16).collect();
    let s = String::from("rtao-arena");
    let joined = std::thread::spawn(|| vec![7u8; 1 << 20]).join().unwrap();
    for p in [v.as_ptr().cast::<u8>(), s.as_ptr(), joined.as_ptr()] {
        assert!(HEAP.contains(p));
    }
    assert!(HEAP.high_water() >= (1 << 19) + (1 << 20));
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}
