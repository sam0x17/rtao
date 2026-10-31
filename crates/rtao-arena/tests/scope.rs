//! Scopes: bump allocation in chunks, caps, release and parking, futures.

#![cfg(feature = "std")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use core::alloc::GlobalAlloc;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use common::{Rng, check, fill, heap, layout};
use rtao_arena::{FixedHeap, Lifetime, Scope, in_scope, unscoped};

const MIB: usize = 1 << 20;
const CHUNK: usize = 64 << 10;

fn alloc(heap: &FixedHeap, size: usize) -> *mut u8 {
    heap.alloc_with(layout(size, 8), Lifetime::Short)
}

fn free(heap: &FixedHeap, ptr: *mut u8, size: usize) {
    // SAFETY: tests pass back what `alloc` returned with the same layout.
    unsafe { heap.dealloc(ptr, layout(size, 8)) };
}

#[test]
fn allocations_inside_run_bump_in_the_scope() {
    let heap = heap(16 * MIB, 0, 0);
    let scope = Scope::new("bump", MIB, CHUNK);
    let (a, b) = scope.run(|| {
        assert!(in_scope());
        (alloc(heap, 24), alloc(heap, 3000))
    });
    assert!(!in_scope());
    let stats = scope.stats();
    assert_eq!((stats.objects, stats.live, stats.chunks), (2, 3024, 1));
    assert_eq!(stats.held, CHUNK);
    assert!(b > a && b.addr() - a.addr() < 64, "bump order");
    assert_eq!(heap.stats().small.live, 0, "nothing went to the slab side");
    // Outside `run` allocations are general.
    let c = alloc(heap, 24);
    assert_eq!(scope.stats().objects, 2);
    assert_eq!(heap.stats().small.live, 32);
    free(heap, c, 24);
    // Deallocation outside the scope finds the chunk by address.
    free(heap, a, 24);
    free(heap, b, 3000);
    let stats = scope.stats();
    assert_eq!((stats.objects, stats.live, stats.peak), (0, 0, 3024));
    let done = scope.finish();
    assert_eq!((done.chunks, done.parked), (0, 0));
    assert_eq!(
        heap.stats().large.live,
        0,
        "chunks and control block returned"
    );
    let summary = heap.scope_summary("bump").unwrap();
    assert_eq!(
        (summary.ended, summary.peak, summary.parked_ends),
        (1, 3024, 0)
    );
}

#[test]
fn a_scope_with_live_objects_parks_until_they_are_freed() {
    let heap = heap(16 * MIB, 0, 0);
    let scope = Scope::new("park", 4 * MIB, CHUNK);
    let objects: Vec<_> = scope.run(|| (0..100).map(|_| alloc(heap, 1000)).collect());
    assert_eq!(scope.stats().chunks, 2);
    let (&kept, rest) = objects.split_last().unwrap();
    for &p in rest {
        free(heap, p, 1000);
    }
    // The older chunk emptied and returned at once; the current one stays.
    assert_eq!(scope.stats().chunks, 1);
    let done = scope.finish();
    assert_eq!((done.parked, done.objects, done.live), (1, 1, 1000));
    let stats = heap.stats();
    assert_eq!((stats.parked_chunks, stats.parked_bytes), (1, CHUNK));
    assert_eq!(heap.scope_summary("park").unwrap().parked_ends, 1);
    free(heap, kept, 1000);
    let stats = heap.stats();
    assert_eq!((stats.parked_chunks, stats.parked_bytes), (0, 0));
    assert_eq!(stats.large.live, 0);
}

#[test]
fn the_cap_refuses_and_large_objects_get_their_own_chunk() {
    let heap = heap(16 * MIB, 0, 0);
    let scope = Scope::new("cap", 2 * CHUNK, CHUNK);
    let mut live = Vec::new();
    scope.run(|| {
        loop {
            let p = alloc(heap, 4000);
            if p.is_null() {
                break;
            }
            live.push(p);
        }
    });
    let stats = scope.stats();
    assert_eq!(stats.cap_refusals, 1);
    assert!(stats.held <= 2 * CHUNK);
    assert_eq!(heap.stats().scope_cap_refusals, 1);
    for p in live.drain(..) {
        free(heap, p, 4000);
    }
    drop(scope);

    let scope = Scope::new("big", 4 * MIB, CHUNK);
    let big = scope.run(|| alloc(heap, 3 * CHUNK));
    assert!(!big.is_null());
    let stats = scope.stats();
    assert_eq!(stats.chunks, 1);
    assert!(stats.held > 3 * CHUNK);
    let small = scope.run(|| alloc(heap, 100));
    assert_eq!(scope.stats().chunks, 2);
    free(heap, big, 3 * CHUNK);
    assert_eq!(
        scope.stats().chunks,
        1,
        "the dedicated chunk returned at once"
    );
    // Over the cap even as a dedicated chunk.
    assert!(scope.run(|| alloc(heap, 8 * MIB)).is_null());
    free(heap, small, 100);
    drop(scope);
    assert_eq!(heap.stats().large.live, 0);
}

#[test]
fn realloc_stays_in_the_owning_scope() {
    let heap = heap(16 * MIB, 0, 0);
    let a_scope = Scope::new("a", 4 * MIB, CHUNK);
    let b_scope = Scope::new("b", 4 * MIB, CHUNK);
    let p = a_scope.run(|| alloc(heap, 100));
    fill(p, 100, 1);
    // The last object grows in place.
    // SAFETY: `p` was allocated by `heap` with this layout.
    let grown = unsafe { heap.realloc(p, layout(100, 8), 1000) };
    assert_eq!(grown, p);
    check(grown, 100, 1);
    let q = a_scope.run(|| alloc(heap, 50));
    // Not the last object any more: moves, but inside `a` even with `b` active.
    // SAFETY: as above.
    let moved = b_scope.run(|| unsafe { heap.realloc(grown, layout(1000, 8), 5000) });
    assert_ne!(moved, grown);
    check(moved, 100, 1);
    assert_eq!(a_scope.stats().objects, 2);
    assert_eq!(b_scope.stats().objects, 0);
    assert_eq!(a_scope.stats().live, 5050);
    // After `a` ends, a realloc of its object goes to the general heap.
    let ended = a_scope.finish();
    assert_eq!(ended.parked, 1);
    // SAFETY: as above.
    let general = unsafe { heap.realloc(moved, layout(5000, 8), 9000) };
    check(general, 100, 1);
    assert_eq!(b_scope.stats().objects, 0);
    free(heap, general, 9000);
    free(heap, q, 50);
    drop(b_scope);
    let stats = heap.stats();
    assert_eq!((stats.parked_chunks, stats.large.live), (0, 0));
}

#[test]
fn nested_and_unscoped_regions() {
    let heap = heap(16 * MIB, 0, 0);
    let outer = Scope::new("outer", MIB, CHUNK);
    let inner = Scope::new("inner", MIB, CHUNK);
    let (o, i, g) = outer.run(|| {
        let o = alloc(heap, 64);
        let i = inner.run(|| alloc(heap, 64));
        let g = unscoped(|| alloc(heap, 64));
        (o, i, g)
    });
    assert_eq!(outer.stats().objects, 1);
    assert_eq!(inner.stats().objects, 1);
    assert_eq!(heap.stats().small.live, 64);
    for p in [o, i, g] {
        free(heap, p, 64);
    }
    drop((outer, inner));
    assert_eq!(heap.stats().large.live, 0);
}

/// A future that allocates on every poll and is pending once.
struct TwoPolls<'h> {
    heap: &'h FixedHeap,
    polls: usize,
    out: Vec<*mut u8>,
}

impl Future for TwoPolls<'_> {
    type Output = Vec<*mut u8>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        assert!(in_scope());
        let p = alloc(self.heap, 200);
        self.out.push(p);
        self.polls += 1;
        if self.polls == 1 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(core::mem::take(&mut self.out))
        }
    }
}

#[test]
fn scoped_futures_are_scoped_only_while_polled() {
    let heap = heap(16 * MIB, 0, 0);
    let scope = Scope::new("future", MIB, CHUNK);
    let mut future = Box::pin(scope.scoped(TwoPolls {
        heap,
        polls: 0,
        out: Vec::new(),
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    assert!(!in_scope());
    let between = alloc(heap, 200);
    assert_eq!(scope.stats().objects, 1);
    let Poll::Ready(out) = future.as_mut().poll(&mut cx) else {
        panic!("second poll completes");
    };
    assert_eq!(scope.stats().objects, 2);
    assert_eq!(heap.stats().small.live, 256);
    for p in out {
        free(heap, p, 200);
    }
    free(heap, between, 200);
    drop(future);
    assert_eq!(scope.finish().parked, 0);
    assert_eq!(heap.stats().large.live, 0);
}

#[test]
fn random_scope_work_with_content_checks() {
    let heap = heap(32 * MIB, 0, 0);
    let mut rng = Rng(42);
    for round in 0..20u64 {
        let scope = Scope::new("random", 8 * MIB, 256 << 10);
        let mut live: Vec<(*mut u8, usize, usize, u64)> = Vec::new();
        scope.run(|| {
            for i in 0..500u64 {
                if rng.chance(70) || live.is_empty() {
                    let size = if rng.chance(90) {
                        rng.range(1, 4000)
                    } else {
                        rng.range(4000, 600_000)
                    };
                    let align = 1 << rng.range(0, 7);
                    let p = heap.alloc_with(layout(size, align), Lifetime::Short);
                    if p.is_null() {
                        continue;
                    }
                    assert_eq!(p.addr() % align, 0);
                    let seed = round << 32 | i;
                    fill(p, size, seed);
                    live.push((p, size, align, seed));
                } else {
                    let (p, size, align, seed) = live.swap_remove(rng.range(0, live.len() - 1));
                    check(p, size, seed);
                    free_aligned(heap, p, size, align);
                }
            }
        });
        let stats = scope.stats();
        assert_eq!(stats.objects, live.len());
        assert!(stats.held <= 8 * MIB);
        // Keep some objects past the end of the scope.
        let keep = live.split_off(live.len() / 2);
        for (p, size, align, seed) in live {
            check(p, size, seed);
            free_aligned(heap, p, size, align);
        }
        drop(scope);
        for (p, size, align, seed) in keep {
            check(p, size, seed);
            free_aligned(heap, p, size, align);
        }
        let stats = heap.stats();
        assert_eq!(
            (stats.parked_chunks, stats.large.live, stats.small.live),
            (0, 0, 0)
        );
    }
}

fn free_aligned(heap: &FixedHeap, p: *mut u8, size: usize, align: usize) {
    // SAFETY: `p` was allocated by `heap` with this layout.
    unsafe { heap.dealloc(p, layout(size, align)) };
}

/// The active scope is per thread: two threads entering and leaving scopes
/// out of order never see each other's scope, and nothing stays active once
/// every `run` has returned.
#[test]
fn scopes_on_two_threads_stay_apart_in_any_order() {
    use std::sync::Barrier;

    let heap = heap(16 * MIB, 0, 0);
    let a = Scope::new("thread-a", MIB, 4096);
    let b = Scope::new("thread-b", MIB, 4096);
    let bar = Barrier::new(2);
    let (pa, pb) = std::thread::scope(|s| {
        let ta = s.spawn(|| {
            let p = a.run(|| {
                bar.wait(); // 1: A active
                bar.wait(); // 2: B active too
                alloc(heap, 64)
            });
            bar.wait(); // 3: A left, B still active
            assert!(!in_scope());
            bar.wait(); // 4: B left
            p.expose_provenance()
        });
        let tb = s.spawn(|| {
            bar.wait(); // 1
            let p = b.run(|| {
                bar.wait(); // 2
                bar.wait(); // 3
                assert!(in_scope());
                alloc(heap, 64)
            });
            bar.wait(); // 4
            assert!(!in_scope());
            p.expose_provenance()
        });
        (ta.join().unwrap(), tb.join().unwrap())
    });
    assert_eq!((a.stats().objects, b.stats().objects), (1, 1));
    // A fresh thread sees no scope; its allocation goes to the general heap.
    let (active, p) = std::thread::spawn(move || (in_scope(), alloc(heap, 64).expose_provenance()))
        .join()
        .unwrap();
    assert!(!active);
    assert_eq!((a.stats().objects, b.stats().objects), (1, 1));
    for addr in [pa, pb, p] {
        free(heap, std::ptr::with_exposed_provenance_mut(addr), 64);
    }
    assert_eq!(a.finish().parked + b.finish().parked, 0);
}
