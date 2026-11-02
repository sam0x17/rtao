//! Lazy heaps: the region is asked for once, at the first use.

#![cfg(feature = "std")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use rtao_arena::{Config, FixedHeap, InitError};

const MIB: usize = 1 << 20;
const LEN: usize = 4 * MIB;

#[repr(align(64))]
struct Region(UnsafeCell<[u8; LEN]>);
// SAFETY: each region is touched only by the one heap it is handed to.
unsafe impl Sync for Region {}

static SERVED: Region = Region(UnsafeCell::new([0; LEN]));
static SERVED_CALLS: AtomicUsize = AtomicUsize::new(0);
fn served() -> Option<(*mut u8, usize)> {
    SERVED_CALLS.fetch_add(1, Ordering::SeqCst);
    Some((SERVED.0.get().cast(), LEN))
}

static NONE_CALLS: AtomicUsize = AtomicUsize::new(0);
fn none() -> Option<(*mut u8, usize)> {
    NONE_CALLS.fetch_add(1, Ordering::SeqCst);
    None
}

static TIGHT: Region = Region(UnsafeCell::new([0; LEN]));
static TIGHT_CALLS: AtomicUsize = AtomicUsize::new(0);
fn tight() -> Option<(*mut u8, usize)> {
    TIGHT_CALLS.fetch_add(1, Ordering::SeqCst);
    Some((TIGHT.0.get().cast(), LEN))
}

static NEVER_CALLS: AtomicUsize = AtomicUsize::new(0);
fn never() -> Option<(*mut u8, usize)> {
    NEVER_CALLS.fetch_add(1, Ordering::SeqCst);
    None
}

#[test]
fn the_region_is_asked_for_once_at_the_first_use() {
    // SAFETY: `SERVED` belongs to this heap alone; `served` does not
    // allocate from it.
    static HEAP: FixedHeap = unsafe { FixedHeap::lazy(Config::new(MIB, MIB), served) };
    assert_eq!(
        SERVED_CALLS.load(Ordering::SeqCst),
        0,
        "nothing at const init"
    );
    let small = Layout::from_size_align(100, 8).unwrap();
    let large = Layout::from_size_align(256 << 10, 64).unwrap();
    // SAFETY: both are freed below with their layouts.
    let (a, b) = unsafe { (HEAP.alloc(small), HEAP.alloc(large)) };
    assert!(!a.is_null() && !b.is_null());
    assert!(HEAP.contains(a) && HEAP.contains(b));
    let base = SERVED.0.get().cast::<u8>().addr();
    assert!((base..base + LEN).contains(&a.addr()));
    assert!(HEAP.is_ready());
    assert!(HEAP.stats().region > LEN - (128 << 10));
    // SAFETY: allocated above with these layouts.
    unsafe {
        HEAP.dealloc(a, small);
        HEAP.dealloc(b, large);
    }
    assert_eq!(HEAP.stats().large.live, 0);
    assert_eq!(SERVED_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn no_region_means_null_and_one_ask() {
    // SAFETY: `none` hands out no memory.
    static HEAP: FixedHeap = unsafe { FixedHeap::lazy(Config::DEFAULT, none) };
    let layout = Layout::from_size_align(64, 8).unwrap();
    for _ in 0..3 {
        // SAFETY: a null result is never used.
        assert!(unsafe { HEAP.alloc(layout) }.is_null());
    }
    assert!(!HEAP.is_ready());
    assert_eq!(HEAP.stats().region, 0);
    assert_eq!(NONE_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn floors_that_do_not_fit_mean_null_and_one_ask() {
    // SAFETY: `TIGHT` belongs to this heap alone.
    static HEAP: FixedHeap = unsafe { FixedHeap::lazy(Config::new(3 * MIB, 3 * MIB), tight) };
    let layout = Layout::from_size_align(64, 8).unwrap();
    // SAFETY: a null result is never used.
    assert!(unsafe { HEAP.alloc(layout) }.is_null());
    assert!(!HEAP.is_ready());
    // SAFETY: as above.
    assert!(unsafe { HEAP.alloc(layout) }.is_null());
    assert_eq!(TIGHT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn a_heap_handed_a_region_first_never_asks() {
    // SAFETY: `never` hands out no memory.
    static HEAP: FixedHeap = unsafe { FixedHeap::lazy(Config::DEFAULT, never) };
    HEAP.init_static(Box::leak(vec![0u8; LEN].into_boxed_slice()))
        .unwrap();
    let layout = Layout::from_size_align(5000, 16).unwrap();
    // SAFETY: freed below with its layout.
    let p = unsafe { HEAP.alloc(layout) };
    assert!(!p.is_null());
    // SAFETY: allocated above.
    unsafe { HEAP.dealloc(p, layout) };
    assert_eq!(
        HEAP.init_static(Box::leak(vec![0u8; LEN].into_boxed_slice())),
        Err(InitError::AlreadyInitialized)
    );
    assert_eq!(NEVER_CALLS.load(Ordering::SeqCst), 0);
}
