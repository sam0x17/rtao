//! FixedHeap behaviour: dispatch, alignment, floors, placement, realloc,
//! exhaustion, queries and randomized sequences with content checks.

#![cfg(feature = "std")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use core::alloc::GlobalAlloc;
use std::collections::BTreeMap;

use common::{Rng, check, fill, heap, layout};
use rtao_arena::{BLOCK_HEADER, FixedHeap, Lifetime, MAX_SMALL, PAGE_BYTES, SIZE_CLASSES};

const MIB: usize = 1 << 20;

fn alloc(heap: &FixedHeap, size: usize, align: usize, hint: Lifetime) -> *mut u8 {
    heap.alloc_with(layout(size, align), hint)
}

fn free(heap: &FixedHeap, ptr: *mut u8, size: usize, align: usize) {
    // SAFETY: tests pass back what `alloc` returned with the same layout.
    unsafe { heap.dealloc(ptr, layout(size, align)) };
}

fn realloc(heap: &FixedHeap, ptr: *mut u8, size: usize, align: usize, new: usize) -> *mut u8 {
    // SAFETY: tests pass back what `alloc` returned with the same layout.
    unsafe { heap.realloc(ptr, layout(size, align), new) }
}

/// The largest single payload a fresh heap's large side can hold.
fn fresh_largest(heap: &FixedHeap, small_floor: usize) -> usize {
    (heap.stats().region - small_floor - 2 * BLOCK_HEADER) & !15
}

#[test]
fn small_and_large_go_to_their_sides() {
    let heap = heap(8 * MIB, 0, 0);
    let small = alloc(heap, 100, 8, Lifetime::Short);
    let large = alloc(heap, 4096, 8, Lifetime::Short);
    let aligned_small = alloc(heap, 64, 4096, Lifetime::Short);
    let stats = heap.stats();
    assert_eq!(stats.small.live, 128);
    assert_eq!(stats.classes[5].objects, 1);
    assert_eq!(stats.classes[5].size, 128);
    assert_eq!(stats.small.capacity, PAGE_BYTES);
    assert!(stats.large.live >= 4096 + 64 + 2 * BLOCK_HEADER);
    assert!(small < large && small < aligned_small);
    assert_eq!(aligned_small.addr() % 4096, 0);
    free(heap, small, 100, 8);
    free(heap, large, 4096, 8);
    free(heap, aligned_small, 64, 4096);
    let stats = heap.stats();
    assert_eq!((stats.small.live, stats.large.live), (0, 0));
    assert_eq!(
        stats.small.capacity, 0,
        "the free topmost page returns to the gap"
    );
    assert_eq!(
        stats.large.capacity, BLOCK_HEADER,
        "only the sentinel stays"
    );
}

#[test]
fn every_size_class_is_used_at_its_natural_alignment() {
    let heap = heap(8 * MIB, 0, 0);
    for (i, &class) in SIZE_CLASSES.iter().enumerate() {
        let p = alloc(heap, class, 1, Lifetime::Short);
        assert!(!p.is_null());
        let natural = class & class.wrapping_neg();
        assert_eq!(p.addr() % natural, 0, "class {class}");
        assert_eq!(heap.stats().classes[i].objects, 1);
        free(heap, p, class, 1);
    }
    // A layout that needs a larger class for its alignment.
    let p = alloc(heap, 24, 32, Lifetime::Short);
    assert_eq!(p.addr() % 32, 0);
    assert_eq!(heap.stats().classes[1].objects, 1);
    free(heap, p, 24, 32);
    let p = alloc(heap, 40, 64, Lifetime::Short);
    assert_eq!(p.addr() % 64, 0);
    assert_eq!(heap.stats().classes[3].objects, 1);
    free(heap, p, 40, 64);
}

#[test]
fn alignment_matrix() {
    let heap = heap(64 * MIB, 0, 0);
    let sizes = [
        1,
        7,
        8,
        16,
        24,
        33,
        100,
        500,
        2047,
        2048,
        2049,
        4096,
        10_000,
        100_000,
        1 << 20,
    ];
    let mut live = Vec::new();
    for &size in &sizes {
        for shift in 0..=17 {
            let align = 1usize << shift;
            let p = alloc(
                heap,
                size,
                align,
                if shift % 2 == 0 {
                    Lifetime::Short
                } else {
                    Lifetime::Long
                },
            );
            assert!(!p.is_null(), "size {size} align {align}");
            assert_eq!(p.addr() % align, 0, "size {size} align {align}");
            assert!(heap.contains(p));
            fill(p, size, (size ^ align) as u64);
            live.push((p, size, align));
        }
        for (p, size, align) in live.drain(..) {
            check(p, size, (size ^ align) as u64);
            free(heap, p, size, align);
        }
    }
    let stats = heap.stats();
    assert_eq!(
        (stats.small.live, stats.large.live, stats.free_blocks),
        (0, 0, 1),
        "everything coalesced back into one free block"
    );
}

#[test]
fn floors_bound_each_side() {
    let (small_floor, large_floor) = (2 * MIB, 4 * MIB);
    let heap = heap(8 * MIB, small_floor, large_floor);
    let region = heap.stats().region;

    // Fill the small side alone: it stops below the large floor.
    let mut small = Vec::new();
    loop {
        let p = alloc(heap, 2048, 8, Lifetime::Short);
        if p.is_null() {
            break;
        }
        small.push(p);
    }
    let stats = heap.stats();
    assert!(stats.small.capacity <= region - large_floor);
    assert!(stats.small.capacity + PAGE_BYTES > region - large_floor);
    assert_eq!(stats.small.failed, 1);
    // The large side still has its floor.
    assert!(heap.largest_free_large() >= large_floor - 2 * BLOCK_HEADER);
    for p in small.drain(..) {
        free(heap, p, 2048, 8);
    }
    assert_eq!(heap.stats().small.capacity, 0);

    // Fill the large side alone: it stops above the small floor.
    let mut large = Vec::new();
    loop {
        let p = alloc(heap, 64 << 10, 16, Lifetime::Short);
        if p.is_null() {
            break;
        }
        large.push(p);
    }
    let stats = heap.stats();
    assert!(stats.large.capacity <= region - small_floor);
    assert!(stats.large.capacity + (64 << 10) + 2 * BLOCK_HEADER > region - small_floor);
    assert_eq!(stats.large.failed, 1);
    // The small side still gets its floor.
    let mut pages = 0;
    loop {
        let p = alloc(heap, 2048, 8, Lifetime::Short);
        if p.is_null() {
            break;
        }
        small.push(p);
        pages += 1;
    }
    assert!(pages * 2048 >= small_floor - PAGE_BYTES);
    for p in small {
        free(heap, p, 2048, 8);
    }
    for p in large {
        free(heap, p, 64 << 10, 16);
    }
    assert_eq!(heap.stats().large.capacity, BLOCK_HEADER);
    assert!(heap.high_water() <= region);
}

/// The address of the region's first page: where a fresh heap puts its first
/// small object.
fn first_page(heap: &FixedHeap) -> usize {
    let p = alloc(heap, 16, 1, Lifetime::Short);
    free(heap, p, 16, 1);
    p.addr()
}

#[test]
fn long_takes_the_largest_run_and_short_the_best_fit() {
    let heap = heap(16 * MIB, 0, 0);
    let lo = first_page(heap);
    // A hole of four 64 KiB blocks below a block at the top, and the rest of
    // the region below the hole's lower neighbour.
    let top = alloc(heap, 64 << 10, 16, Lifetime::Short);
    let hole: Vec<_> = (0..4)
        .map(|_| alloc(heap, 64 << 10, 16, Lifetime::Short))
        .collect();
    let below = alloc(heap, 64 << 10, 16, Lifetime::Short);
    assert!(hole.iter().all(|&p| p < top && p > below));
    for &p in &hole {
        free(heap, p, 64 << 10, 16);
    }
    assert_eq!(heap.stats().free_blocks, 2);
    let hole_hi = top.addr() - BLOCK_HEADER;

    // Short: the best fit (the hole), at its top.
    let short = alloc(heap, 8192, 16, Lifetime::Short);
    assert_eq!(short.addr() + 8192, hole_hi);
    // Long: the largest free run, at its top, against `below`.
    let long = alloc(heap, 8192, 16, Lifetime::Long);
    assert_eq!(long.addr() + 8192, below.addr() - BLOCK_HEADER);

    // With the run below `long` beneath the small floor, only the hole is
    // left for large blocks: Long takes its top, against `short`.
    heap.configure(rtao_arena::Config::new(long.addr() - BLOCK_HEADER - lo, 0))
        .unwrap();
    let hole_left = hole_hi - 8192 - BLOCK_HEADER - (below.addr() + (64 << 10));
    assert_eq!(heap.free_large(), hole_left - BLOCK_HEADER);
    let long2 = alloc(heap, 8192, 16, Lifetime::Long);
    assert_eq!(long2.addr() + 8192, short.addr() - BLOCK_HEADER);
    for p in [short, long, long2, below, top] {
        free(
            heap,
            p,
            if p == top || p == below {
                64 << 10
            } else {
                8192
            },
            16,
        );
    }
    let stats = heap.stats();
    assert_eq!((stats.large.live, stats.free_blocks), (0, 1));
}

#[test]
fn freeing_everything_coalesces_back_to_one_region() {
    let heap = heap(16 * MIB, MIB, 0);
    let mut rng = Rng(7);
    // A long block first: the top of the region.
    let pin = alloc(heap, 4096, 16, Lifetime::Long);
    let mut live = Vec::new();
    for _ in 0..80 {
        let size = rng.range(2049, 200_000);
        let hint = if rng.chance(30) {
            Lifetime::Long
        } else {
            Lifetime::Short
        };
        let p = alloc(heap, size, 16, hint);
        if !p.is_null() {
            live.push((p, size));
        }
    }
    // Keep the pin and free the rest in random order.
    assert!(live.iter().all(|&(p, _)| p < pin));
    while !live.is_empty() {
        let (p, size) = live.swap_remove(rng.range(0, live.len() - 1));
        free(heap, p, size, 16);
    }
    let stats = heap.stats();
    assert_eq!(
        stats.free_blocks, 1,
        "everything below the pin is one block"
    );
    free(heap, pin, 4096, 16);
    let stats = heap.stats();
    assert_eq!((stats.free_blocks, stats.large.live), (1, 0));
    assert_eq!(stats.large.capacity, BLOCK_HEADER);
    let largest = fresh_largest(heap, MIB);
    assert_eq!(heap.largest_free_large(), largest);
    assert_eq!(heap.free_large(), largest);
    let all = alloc(heap, largest, 16, Lifetime::Short);
    assert!(!all.is_null());
    assert!(alloc(heap, 4096, 16, Lifetime::Short).is_null());
    free(heap, all, largest, 16);
}

#[test]
fn realloc_stays_on_the_owners_side() {
    let heap = heap(16 * MIB, 0, 0);
    // Large: grow in place into a free upper neighbour.
    let upper = alloc(heap, 8192, 16, Lifetime::Short);
    let a = alloc(heap, 8192, 16, Lifetime::Short);
    fill(a, 8192, 1);
    free(heap, upper, 8192, 16);
    let grown = realloc(heap, a, 8192, 16, 12_000);
    assert_eq!(grown, a, "grown in place");
    check(grown, 8192, 1);
    // Shrink in place.
    let shrunk = realloc(heap, grown, 12_000, 16, 3000);
    assert_eq!(shrunk, a);
    check(shrunk, 3000, 1);
    // Upper neighbour used: moves, still on the large side.
    let b = alloc(heap, 8192, 16, Lifetime::Short);
    fill(b, 8192, 2);
    let moved = realloc(heap, b, 8192, 16, 100_000);
    assert_ne!(moved, b);
    check(moved, 8192, 2);
    // Shrinking a large block keeps it on the large side.
    let small_again = realloc(heap, moved, 100_000, 16, 100);
    check(small_again, 100, 2);
    assert_eq!(heap.stats().small.live, 0);
    free(heap, small_again, 100, 16);
    free(heap, shrunk, 3000, 16);

    // Small: stays within the class, then moves to a larger class.
    let s = alloc(heap, 20, 8, Lifetime::Short);
    fill(s, 20, 3);
    let same = realloc(heap, s, 20, 8, 32);
    assert_eq!(same, s);
    let bigger = realloc(heap, same, 32, 8, 100);
    assert_ne!(bigger, s);
    check(bigger, 20, 3);
    assert_eq!(heap.stats().classes[5].objects, 1);
    let large = realloc(heap, bigger, 100, 8, 5000);
    check(large, 20, 3);
    assert_eq!(heap.stats().small.live, 0);
    free(heap, large, 5000, 8);
    assert_eq!(heap.stats().large.live, 0);
}

#[test]
fn exhaustion_returns_null() {
    let heap = heap(MIB, 0, 0);
    let mut large = Vec::new();
    loop {
        let p = alloc(heap, 32 << 10, 16, Lifetime::Short);
        if p.is_null() {
            break;
        }
        large.push(p);
    }
    let mut small = Vec::new();
    loop {
        let p = alloc(heap, 16, 1, Lifetime::Short);
        if p.is_null() {
            break;
        }
        small.push(p);
    }
    let stats = heap.stats();
    assert!(stats.large.failed >= 1 && stats.small.failed >= 1);
    assert!(stats.high_water <= stats.region);
    assert!(alloc(heap, usize::MAX / 4, 16, Lifetime::Short).is_null());
    assert!(alloc(heap, 64, 1 << 30, Lifetime::Long).is_null());
    // A failed realloc leaves the block intact.
    let p = large[0];
    fill(p, 32 << 10, 9);
    assert!(realloc(heap, p, 32 << 10, 16, 8 * MIB).is_null());
    check(p, 32 << 10, 9);
    for p in large {
        free(heap, p, 32 << 10, 16);
    }
    for p in small {
        free(heap, p, 16, 1);
    }
    let stats = heap.stats();
    assert_eq!(
        (stats.small.live, stats.large.live, stats.small.capacity),
        (0, 0, 0)
    );
}

#[test]
fn queries_track_peaks_baselines_and_resets() {
    let heap = heap(8 * MIB, 0, 0);
    heap.reinit_baseline();
    let a = alloc(heap, MIB, 16, Lifetime::Short);
    let b = alloc(heap, 100, 8, Lifetime::Short);
    assert_eq!(heap.leaked_bytes(), MIB + BLOCK_HEADER + 128);
    assert!(heap.can_serve(4 * MIB, 4 * MIB));
    assert!(!heap.can_serve(8 * MIB, MIB));
    free(heap, a, MIB, 16);
    assert_eq!(heap.leaked_bytes(), 128);
    let stats = heap.stats();
    assert_eq!(stats.large.peak, MIB + BLOCK_HEADER);
    assert!(stats.high_water >= MIB);
    heap.reset_peaks();
    let stats = heap.stats();
    assert_eq!(stats.large.peak, 0);
    assert_eq!(stats.high_water, PAGE_BYTES + BLOCK_HEADER);
    free(heap, b, 100, 8);
    assert_eq!(heap.leaked_bytes(), 0);
}

#[test]
fn an_uninitialized_heap_refuses_everything() {
    let heap = FixedHeap::new(rtao_arena::Config::DEFAULT);
    assert!(heap.alloc_with(layout(8, 8), Lifetime::Short).is_null());
    assert!(!heap.can_serve(1, 1));
    assert_eq!(heap.stats(), rtao_arena::Stats::default());
    let tiny: &'static mut [u8] = Box::leak(vec![0u8; 1000].into_boxed_slice());
    let leaked: &'static FixedHeap =
        Box::leak(Box::new(FixedHeap::new(rtao_arena::Config::DEFAULT)));
    assert_eq!(
        leaked.init_static(tiny),
        Err(rtao_arena::InitError::TooSmall)
    );
    let floors: &'static FixedHeap = Box::leak(Box::new(FixedHeap::new(rtao_arena::Config::new(
        4 * MIB,
        4 * MIB,
    ))));
    assert!(matches!(
        floors.init_static(Box::leak(vec![0u8; 4 * MIB].into_boxed_slice())),
        Err(rtao_arena::InitError::Floors { .. })
    ));
}

struct Live {
    ptr: *mut u8,
    size: usize,
    align: usize,
    seed: u64,
}

fn random_layout(rng: &mut Rng) -> (usize, usize) {
    let size = match rng.range(0, 99) {
        0..=59 => rng.range(1, MAX_SMALL),
        60..=89 => rng.range(MAX_SMALL + 1, 64 << 10),
        _ => rng.range(64 << 10, MIB),
    };
    let align = match rng.range(0, 99) {
        0..=69 => 1 << rng.range(0, 4),
        70..=94 => 1 << rng.range(5, 12),
        _ => 1 << rng.range(13, 16),
    };
    (size, align)
}

/// Random alloc/free/realloc against a model; checks contents, overlap,
/// alignment, region bounds, floors and `largest_free_large` exactness.
fn run_random(seed: u64, ops: usize, bytes: usize, small_floor: usize, large_floor: usize) {
    let heap = heap(bytes, small_floor, large_floor);
    let region = heap.stats().region;
    let fresh_pages = heap.stats().free_pages;
    let mut rng = Rng(seed);
    let mut live: Vec<Live> = Vec::new();
    let mut spans: BTreeMap<usize, usize> = BTreeMap::new();
    let mut next_seed = seed;
    let insert = |spans: &mut BTreeMap<usize, usize>, p: usize, size: usize| {
        if let Some((&lo, &hi)) = spans.range(..=p).next_back() {
            assert!(hi <= p, "overlap below {p:#x}: {lo:#x}..{hi:#x}");
        }
        if let Some((&lo, _)) = spans.range(p..).next() {
            assert!(lo >= p + size.max(1), "overlap above {p:#x}");
        }
        spans.insert(p, p + size.max(1));
    };
    for op in 0..ops {
        match rng.range(0, 99) {
            0..=49 => {
                let (size, align) = random_layout(&mut rng);
                let hint = if rng.chance(30) {
                    Lifetime::Long
                } else {
                    Lifetime::Short
                };
                let large = size > MAX_SMALL || align > MAX_SMALL;
                let predicted = (large && align <= 16).then(|| size <= heap.largest_free_large());
                let p = alloc(heap, size, align, hint);
                if let Some(predicted) = predicted {
                    assert_eq!(
                        !p.is_null(),
                        predicted,
                        "op {op}: largest_free_large is exact for {size}"
                    );
                }
                if p.is_null() {
                    continue;
                }
                assert_eq!(p.addr() % align, 0);
                assert!(heap.contains(p));
                insert(&mut spans, p.addr(), size);
                next_seed += 1;
                fill(p, size, next_seed);
                live.push(Live {
                    ptr: p,
                    size,
                    align,
                    seed: next_seed,
                });
            }
            50..=79 if !live.is_empty() => {
                let item = live.swap_remove(rng.range(0, live.len() - 1));
                check(item.ptr, item.size, item.seed);
                spans.remove(&item.ptr.addr());
                free(heap, item.ptr, item.size, item.align);
            }
            80..=99 if !live.is_empty() => {
                let index = rng.range(0, live.len() - 1);
                let (new, _) = random_layout(&mut rng);
                let item = &mut live[index];
                let p = realloc(heap, item.ptr, item.size, item.align, new);
                if p.is_null() {
                    check(item.ptr, item.size, item.seed);
                    continue;
                }
                assert_eq!(p.addr() % item.align, 0);
                check(p, item.size.min(new), item.seed);
                spans.remove(&item.ptr.addr());
                insert(&mut spans, p.addr(), new);
                next_seed += 1;
                fill(p, new, next_seed);
                *item = Live {
                    ptr: p,
                    size: new,
                    align: item.align,
                    seed: next_seed,
                };
            }
            _ => {}
        }
        if op % 256 == 0 {
            let stats = heap.stats();
            assert!(stats.small.capacity <= region - large_floor);
            assert!(stats.large.capacity <= region - small_floor);
            assert_eq!(
                stats.small.capacity + stats.large.capacity + stats.gap,
                region
            );
            assert!(stats.high_water <= region);
            let objects: usize = stats.classes.iter().map(|c| c.objects).sum();
            assert!(objects <= live.len());
        }
    }
    for item in live.drain(..) {
        check(item.ptr, item.size, item.seed);
        free(heap, item.ptr, item.size, item.align);
    }
    let stats = heap.stats();
    assert_eq!((stats.small.live, stats.large.live), (0, 0));
    assert_eq!((stats.small.capacity, stats.free_pages), (0, fresh_pages));
    assert_eq!((stats.free_blocks, stats.large.capacity), (1, BLOCK_HEADER));
    assert_eq!(heap.largest_free_large(), fresh_largest(heap, small_floor));
}

#[test]
fn random_sequences_without_floors() {
    for seed in 1..=4 {
        run_random(seed, 6000, 32 * MIB, 0, 0);
    }
}

#[test]
fn random_sequences_with_floors_and_pressure() {
    for seed in 10..=13 {
        run_random(seed, 6000, 8 * MIB, MIB, 2 * MIB);
    }
}

#[test]
fn threads_share_one_heap() {
    let heap = heap(64 * MIB, 0, 0);
    std::thread::scope(|s| {
        for t in 0..4u64 {
            s.spawn(move || {
                let mut rng = Rng(100 + t);
                let mut live: Vec<(usize, usize, u64)> = Vec::new();
                for i in 0..4000u64 {
                    if rng.chance(55) || live.is_empty() {
                        let size = if rng.chance(80) {
                            rng.range(1, 2048)
                        } else {
                            rng.range(2049, 100_000)
                        };
                        let p = alloc(heap, size, 8, Lifetime::Short);
                        if !p.is_null() {
                            let seed = t << 32 | i;
                            fill(p, size, seed);
                            live.push((p.expose_provenance(), size, seed));
                        }
                    } else {
                        let (p, size, seed) = live.swap_remove(rng.range(0, live.len() - 1));
                        let p = core::ptr::with_exposed_provenance_mut::<u8>(p);
                        check(p, size, seed);
                        free(heap, p, size, 8);
                    }
                }
                for (p, size, seed) in live {
                    let p = core::ptr::with_exposed_provenance_mut::<u8>(p);
                    check(p, size, seed);
                    free(heap, p, size, 8);
                }
            });
        }
    });
    let stats = heap.stats();
    assert_eq!((stats.small.live, stats.large.live), (0, 0));
}

#[test]
fn huge_alignment_on_the_large_side() {
    let heap = heap(16 * MIB, 0, 0);
    for align in [1 << 16, 1 << 17, 1 << 20] {
        for hint in [Lifetime::Short, Lifetime::Long] {
            let p = alloc(heap, 5000, align, hint);
            assert_eq!(p.addr() % align, 0);
            fill(p, 5000, 4);
            let q = alloc(heap, 3000, align, hint);
            assert_eq!(q.addr() % align, 0);
            free(heap, p, 5000, align);
            free(heap, q, 3000, align);
        }
    }
    assert_eq!(heap.stats().large.live, 0);
}

/// Grows a large buffer by doubling, as `Vec` does, checking its contents at
/// every step. Returns the final pointer.
fn double_until(heap: &FixedHeap, from: usize, to: usize, align: usize) -> *mut u8 {
    let mut size = from;
    let mut p = alloc(heap, size, align, Lifetime::Short);
    assert!(!p.is_null());
    fill(p, size, 7);
    while size < to {
        p = realloc(heap, p, size, align, size * 2);
        assert!(!p.is_null(), "{size} -> {} failed", size * 2);
        assert_eq!(p.addr() % align, 0);
        check(p, size, 7);
        fill(p, size * 2, 7);
        size *= 2;
    }
    p
}

#[test]
fn doubling_growth_costs_no_more_than_old_plus_new() {
    let heap = heap(32 * MIB, 0, 0);
    let region = heap.stats().region;
    // Leave the large side 6.5 MiB of room: enough for 2 MiB + 4 MiB.
    heap.configure(rtao_arena::Config::new(region - (6 * MIB + MIB / 2), 0))
        .unwrap();
    let room = heap.largest_free_large();
    assert!(room >= 6 * MIB + 256 * 1024);

    // The final 2 MiB -> 4 MiB step reached by doubling from 64 bytes must
    // succeed like the same step taken in one shot: earlier copies are not
    // stranded above the block.
    let p = double_until(heap, 64, 4 * MIB, 8);
    let s = heap.stats();
    assert_eq!(s.large.live, 4 * MIB + BLOCK_HEADER);
    assert_eq!(
        heap.largest_free_large(),
        room - 4 * MIB - BLOCK_HEADER,
        "the rest of the room stays one free block"
    );
    free(heap, p, 4 * MIB, 8);
    assert_eq!(heap.stats().large.live, 0);
    assert_eq!(heap.largest_free_large(), room);

    // With a live block above it, the growing block still reuses its own
    // earlier room, and strictly aligned blocks keep their alignment.
    let pinned = alloc(heap, 64 << 10, 8, Lifetime::Short);
    let p = double_until(heap, 8 << 10, 2 * MIB, 4096);
    assert!(
        heap.largest_free_large() + 2 * MIB + (64 << 10) + 4096 + 4 * BLOCK_HEADER >= room,
        "earlier sizes are not stranded"
    );
    free(heap, p, 2 * MIB, 4096);
    free(heap, pinned, 64 << 10, 8);
    assert_eq!(heap.largest_free_large(), room);
}

#[test]
fn recycled_slab_pages_are_taken_lowest_first() {
    let heap = heap(16 * MIB, 0, 0);
    let fresh = heap.stats().free_pages;
    let per_page = PAGE_BYTES / 64;
    let pages: Vec<Vec<*mut u8>> = (0..10)
        .map(|_| {
            (0..per_page)
                .map(|_| alloc(heap, 64, 8, Lifetime::Short))
                .collect()
        })
        .collect();
    let s = heap.stats();
    assert_eq!(
        (s.small.capacity, s.free_pages),
        (10 * PAGE_BYTES, fresh - 10)
    );
    // Drain pages 0..=8 in address order; page 9 stays in use. The drained
    // pages are free memory at once, one block below page 9.
    for page in &pages[..9] {
        for &p in page {
            free(heap, p, 64, 8);
        }
    }
    let s = heap.stats();
    assert_eq!((s.small.capacity, s.free_pages), (PAGE_BYTES, fresh - 1));
    assert_eq!(s.free_blocks, 2);
    // An object of another class takes the lowest free page ...
    let x = alloc(heap, 128, 8, Lifetime::Short);
    assert_eq!(x, pages[0][0]);
    // ... and a drained page merges with the free memory around it.
    for &p in &pages[9] {
        free(heap, p, 64, 8);
    }
    let s = heap.stats();
    assert_eq!((s.small.capacity, s.free_pages), (PAGE_BYTES, fresh - 1));
    assert_eq!(s.free_blocks, 1);
    free(heap, x, 128, 8);
    let s = heap.stats();
    assert_eq!(
        (s.small.capacity, s.free_pages, s.free_blocks),
        (0, fresh, 1)
    );
}
