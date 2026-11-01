//! One pool for both sides: pages the small side gives back anywhere serve
//! large blocks, free pages coalesce with free large blocks, the small side
//! takes the lowest free page, and the floors stay minimum guarantees.
//!
//! The tests keep an independent model of what the heap holds (live objects
//! per page and live large blocks) and check the heap's answers against the
//! free memory the model sees.

#![cfg(feature = "std")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use core::alloc::GlobalAlloc;
use std::collections::{BTreeMap, BTreeSet};

use common::{Rng, check, fill, heap, layout};
use rtao_arena::{BLOCK_HEADER, Config, FixedHeap, Lifetime, MAX_SMALL, PAGE_BYTES, Scope};

const MIB: usize = 1 << 20;
/// How far the heap's free blocks may fall short of the model's free runs: a
/// large block can absorb a 16-byte tail, and a block may not start within
/// 32 bytes above a free block it splits.
const SLACK: usize = 48;

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

fn round16(size: usize) -> usize {
    size.max(16).next_multiple_of(16)
}

fn is_small(size: usize, align: usize) -> bool {
    size <= MAX_SMALL && align <= MAX_SMALL
}

/// Marks the first and last 32 bytes of an allocation (whole-buffer patterns
/// would make the multi-megabyte cases slow in debug builds).
fn mark(p: *mut u8, size: usize, seed: u64) {
    if size <= 64 {
        fill(p, size, seed);
        return;
    }
    fill(p, 32, seed);
    // SAFETY: the caller owns `size` bytes at `p`.
    fill(unsafe { p.add(size - 32) }, 32, seed ^ 0x55);
}

fn checked(p: *mut u8, size: usize, seed: u64) {
    if size <= 64 {
        check(p, size, seed);
        return;
    }
    check(p, 32, seed);
    // SAFETY: the caller owns `size` initialized bytes at `p`.
    check(unsafe { p.add(size - 32) }, 32, seed ^ 0x55);
}

/// An independent model of what a heap holds.
struct Model {
    /// The region's first page.
    lo: usize,
    /// The region's usable end.
    end: usize,
    /// The lowest address a large block may start at.
    floor: usize,
    /// The highest address a page may end at.
    page_limit: usize,
    /// Live small objects per page.
    objects: Vec<u32>,
    /// Live large blocks: header address to payload end (the size rounded up
    /// to 16; the heap may hold up to 16 bytes more).
    large: BTreeMap<usize, usize>,
}

impl Model {
    /// The model of a fresh heap with these floors.
    fn new(heap: &FixedHeap, small_floor: usize, large_floor: usize) -> Self {
        let stats = heap.stats();
        assert_eq!((stats.small.live, stats.large.live), (0, 0), "a fresh heap");
        // A fresh heap puts its first small object at its first page.
        let p = alloc(heap, 16, 1, Lifetime::Short);
        free(heap, p, 16, 1);
        let lo = p.addr();
        let end = lo + stats.region;
        let mut model = Self {
            lo,
            end,
            floor: lo,
            page_limit: end,
            objects: vec![0; stats.region / PAGE_BYTES],
            large: BTreeMap::new(),
        };
        model.lines(small_floor, large_floor);
        model
    }

    /// Draws the heap's two lines for these floors: the small floor's end
    /// rounded up to 16 but not above the page limit, and down to its page
    /// when it lies less than 32 bytes past one.
    fn lines(&mut self, small_floor: usize, large_floor: usize) {
        self.page_limit = self.end - large_floor;
        let floor = (self.lo + small_floor)
            .next_multiple_of(16)
            .min(self.page_limit & !15);
        let sliver = floor % PAGE_BYTES;
        self.floor = if (1..32).contains(&sliver) {
            floor - sliver
        } else {
            floor
        };
    }

    fn page(&self, addr: usize) -> usize {
        (addr - self.lo) / PAGE_BYTES
    }

    fn add(&mut self, p: *mut u8, size: usize, align: usize) {
        if is_small(size, align) {
            let page = self.page(p.addr());
            self.objects[page] += 1;
            assert!(
                p.addr() < self.page_limit,
                "a small object above the large floor"
            );
        } else {
            let header = p.addr() - BLOCK_HEADER;
            assert!(header >= self.floor, "a large block below the small floor");
            self.large.insert(header, p.addr() + round16(size));
        }
    }

    fn remove(&mut self, p: *mut u8, size: usize, align: usize) {
        if is_small(size, align) {
            let page = self.page(p.addr());
            self.objects[page] -= 1;
        } else {
            self.large.remove(&(p.addr() - BLOCK_HEADER)).unwrap();
        }
    }

    /// Free runs `(start, end, exact)` in address order: the memory between
    /// held pages and large blocks. `exact` says the run starts at the
    /// region's first page or at the end of a held page (so the heap's free
    /// block starts exactly there).
    fn runs(&self) -> Vec<(usize, usize, bool)> {
        let mut held: Vec<(usize, usize, bool)> = self
            .objects
            .iter()
            .enumerate()
            .filter(|&(_, &n)| n > 0)
            .map(|(i, _)| {
                let page = self.lo + i * PAGE_BYTES;
                (page, page + PAGE_BYTES, true)
            })
            .collect();
        held.extend(self.large.iter().map(|(&start, &end)| (start, end, false)));
        held.sort_unstable();
        let mut runs = Vec::new();
        let (mut at, mut exact) = (self.lo, true);
        for (start, end, page) in held {
            if start > at {
                runs.push((at, start, exact));
            }
            if end >= at {
                (at, exact) = (end, page);
            }
        }
        let top = self.end - BLOCK_HEADER;
        if top > at {
            runs.push((at, top, exact));
        }
        runs
    }

    /// The largest payload one large block (alignment up to 16) could take
    /// from the model's free memory.
    fn largest_large(&self) -> usize {
        self.runs()
            .iter()
            .map(|&(start, end, _)| {
                end.saturating_sub(start.max(self.floor))
                    .saturating_sub(BLOCK_HEADER)
            })
            .max()
            .unwrap_or(0)
    }

    /// The most one run of the model's free memory holds above the page
    /// limit, where no page may go.
    fn largest_above_limit(&self) -> usize {
        self.runs()
            .iter()
            .map(|&(start, end, _)| end.saturating_sub(start.max(self.page_limit)))
            .max()
            .unwrap_or(0)
    }

    /// The lowest free page the heap may give (any page wholly in a free run
    /// below the page limit) and the lowest it must give at the latest (one
    /// the heap's free block certainly holds with room on both sides).
    fn lowest_pages(&self) -> (Option<usize>, Option<usize>) {
        let (mut loose, mut strict) = (None, None);
        for (start, end, exact) in self.runs() {
            let mut page = start.next_multiple_of(PAGE_BYTES);
            while page + PAGE_BYTES <= end.min(self.page_limit) {
                loose = loose.or(Some(page));
                let front = page - start;
                let back = end - page - PAGE_BYTES;
                if ((front == 0 && exact) || front >= SLACK) && (back == 0 || back >= 32) {
                    strict = Some(page);
                    return (loose, strict);
                }
                page += PAGE_BYTES;
            }
        }
        (loose, strict)
    }

    /// Checks the heap's large-side queries against the model.
    fn check_large(&self, heap: &FixedHeap, what: &str) {
        let (model, got) = (self.largest_large(), heap.largest_free_large());
        assert!(
            got <= model && got + SLACK >= model,
            "{what}: the heap's largest free block holds {got} B, the model's free memory {model} B"
        );
    }
}

/// One live allocation of a workload.
#[derive(Clone, Copy)]
struct Live {
    ptr: *mut u8,
    size: usize,
    align: usize,
    seed: u64,
}

/// A workload on one heap and its model: every allocation is checked against
/// the model's free memory.
struct Run {
    heap: &'static FixedHeap,
    model: Model,
    small: Vec<Live>,
    large: Vec<Live>,
    seed: u64,
    served: usize,
    refused: usize,
}

impl Run {
    fn new(bytes: usize, small_floor: usize, large_floor: usize) -> Self {
        let heap = heap(bytes, small_floor, large_floor);
        let model = Model::new(heap, small_floor, large_floor);
        Self {
            heap,
            model,
            small: Vec::new(),
            large: Vec::new(),
            seed: 0,
            served: 0,
            refused: 0,
        }
    }

    fn keep(&mut self, ptr: *mut u8, size: usize, align: usize) -> Live {
        assert_eq!(ptr.addr() % align, 0);
        self.seed += 1;
        mark(ptr, size, self.seed);
        self.model.add(ptr, size, align);
        Live {
            ptr,
            size,
            align,
            seed: self.seed,
        }
    }

    /// A small allocation; when it takes a new page, the page must be the
    /// lowest free one (the model has not seen the new object yet).
    fn small(&mut self, size: usize) -> Option<Live> {
        let before = self.heap.stats().small.capacity;
        let p = alloc(self.heap, size, 8, Lifetime::Short);
        if p.is_null() {
            let (_, strict) = self.model.lowest_pages();
            assert!(
                strict.is_none(),
                "no page for a small object with {strict:x?} free"
            );
            return None;
        }
        if self.heap.stats().small.capacity > before {
            let (loose, strict) = self.model.lowest_pages();
            let page = p.addr() & !(PAGE_BYTES - 1);
            assert!(
                loose.is_some_and(|l| page >= l),
                "a new page below every free one"
            );
            if let Some(strict) = strict {
                assert!(
                    page <= strict,
                    "a new page at {page:#x}, above the free {strict:#x}"
                );
            }
        }
        let live = self.keep(p, size, 8);
        self.small.push(live);
        Some(live)
    }

    /// A large allocation: it must succeed whenever the model's free memory
    /// has a run that holds it, the heap's own prediction is exact, and it
    /// reaches above the page limit (into memory no page may use) whenever a
    /// run there holds it with room for the size classes' rounding.
    fn large(&mut self, size: usize, align: usize, hint: Lifetime, what: &str) -> Option<Live> {
        let model = self.model.largest_large();
        let above = self.model.largest_above_limit();
        let predicted = size <= self.heap.largest_free_large();
        let p = alloc(self.heap, size, align, hint);
        if align <= 16 {
            let need = round16(size) + BLOCK_HEADER;
            if above >= need + need / 16 + SLACK {
                assert!(
                    p.addr() + round16(size) > self.model.page_limit,
                    "{what}: {size} B placed below the page limit with {above} B free above it"
                );
            }
            assert_eq!(
                !p.is_null(),
                predicted,
                "{what}: largest_free_large is exact for {size}"
            );
            if round16(size) + SLACK <= model {
                assert!(
                    !p.is_null(),
                    "{what}: {size} B refused with a {model} B free run"
                );
            }
            if round16(size) > model {
                assert!(
                    p.is_null(),
                    "{what}: {size} B served beyond the model's {model} B"
                );
            }
        }
        if p.is_null() {
            self.refused += 1;
            return None;
        }
        self.served += 1;
        let live = self.keep(p, size, align);
        self.large.push(live);
        Some(live)
    }

    fn drop_small(&mut self, index: usize) {
        let item = self.small.swap_remove(index);
        self.release(item);
    }

    fn drop_large(&mut self, index: usize) {
        let item = self.large.swap_remove(index);
        self.release(item);
    }

    fn release(&mut self, item: Live) {
        checked(item.ptr, item.size, item.seed);
        self.model.remove(item.ptr, item.size, item.align);
        free(self.heap, item.ptr, item.size, item.align);
    }

    /// Resizes a large block; growth must succeed whenever the model has a
    /// run for the new size beside the old block.
    fn resize(&mut self, index: usize, new: usize) {
        let item = self.large[index];
        let model = self.model.largest_large();
        let p = realloc(self.heap, item.ptr, item.size, item.align, new);
        if p.is_null() {
            assert!(
                item.align > 16 || round16(new) + SLACK > model,
                "{} B -> {new} B refused with a {model} B free run",
                item.size
            );
            checked(item.ptr, item.size, item.seed);
            return;
        }
        let keep = item.size.min(new).min(32);
        check(p, keep, item.seed);
        self.model.remove(item.ptr, item.size, item.align);
        self.large[index] = self.keep(p, new, item.align);
    }

    fn check(&self, what: &str) {
        self.model.check_large(self.heap, what);
        let stats = self.heap.stats();
        assert_eq!(
            stats.small.capacity + stats.large.capacity + stats.gap,
            stats.region,
            "{what}"
        );
        let held = self.model.objects.iter().filter(|&&n| n > 0).count();
        assert_eq!(
            stats.small.capacity,
            held * PAGE_BYTES,
            "{what}: empty pages are given back"
        );
    }

    fn clear(&mut self) {
        while !self.small.is_empty() {
            self.drop_small(self.small.len() - 1);
        }
        while !self.large.is_empty() {
            self.drop_large(self.large.len() - 1);
        }
        let stats = self.heap.stats();
        assert_eq!((stats.small.live, stats.large.live), (0, 0));
        assert_eq!((stats.small.capacity, stats.free_blocks), (0, 1));
    }
}

/// A slab size weighted like a heap of keys, nodes and small vectors: mostly
/// 64–512 bytes.
fn small_size(rng: &mut Rng) -> usize {
    match rng.range(0, 99) {
        0..=4 => rng.range(1, 48),
        5..=44 => rng.range(49, 64),
        45..=69 => rng.range(65, 128),
        70..=84 => rng.range(129, 256),
        85..=94 => rng.range(257, 512),
        _ => rng.range(513, MAX_SMALL),
    }
}

/// A large size spread over 2 KiB to `max`, more small ones than big ones.
fn large_size(rng: &mut Rng, max: usize) -> usize {
    let bits = rng.range(12, max.ilog2() as usize);
    rng.range((1 << bits) / 2 + 1, 1 << bits).max(MAX_SMALL + 1)
}

#[test]
fn a_page_freed_anywhere_serves_large_blocks() {
    let mut run = Run::new(8 * MIB, 0, 0);
    let per_page = PAGE_BYTES / 256;
    // Twenty pages of 256-byte objects, then a block at the top.
    let objects: Vec<Live> = (0..20 * per_page)
        .map(|_| run.small(256).unwrap())
        .collect();
    let top = run.large(1 << 20, 16, Lifetime::Short, "top").unwrap();
    // Free pages 2..=17; pages 0, 1, 18 and 19 stay, so the freed pages are
    // neither the topmost nor next to the large side.
    let first = objects[0].ptr.addr();
    for item in objects {
        let page = (item.ptr.addr() - first) / PAGE_BYTES;
        if (2..18).contains(&page) {
            let index = run.small.iter().position(|l| l.ptr == item.ptr).unwrap();
            run.drop_small(index);
        }
    }
    run.check("after the drain");
    // The sixteen freed pages are one free block a large block can fill.
    let hole = 16 * PAGE_BYTES - BLOCK_HEADER;
    let rest = run.heap.largest_free_large();
    let filler = run.large(rest, 16, Lifetime::Short, "the rest").unwrap();
    run.check("the rest taken");
    assert_eq!(run.heap.largest_free_large(), hole);
    let in_hole = run
        .large(hole, 16, Lifetime::Short, "the freed pages")
        .unwrap();
    assert_eq!(in_hole.ptr.addr(), first + 2 * PAGE_BYTES + BLOCK_HEADER);
    assert_eq!(run.heap.largest_free_large(), 0);
    run.check("the hole taken");
    // Freed again, the pages serve the small side again, lowest first.
    let index = run.large.iter().position(|l| l.ptr == in_hole.ptr).unwrap();
    run.drop_large(index);
    let again = run.small(1024).unwrap();
    assert_eq!(again.ptr.addr(), first + 2 * PAGE_BYTES);
    assert!(top.ptr.addr() > filler.ptr.addr());
    run.clear();
}

#[test]
fn freed_pages_coalesce_with_free_large_blocks() {
    let mut run = Run::new(8 * MIB, 0, 0);
    let per_page = PAGE_BYTES / 512;
    // Pages at the bottom, then large blocks right above them, 64 KiB apart.
    let objects: Vec<Live> = (0..4 * per_page).map(|_| run.small(512).unwrap()).collect();
    let rest = run.heap.largest_free_large();
    let a = rest - 3 * ((100 << 10) + BLOCK_HEADER) - (64 << 10);
    let a = run.large(a, 16, Lifetime::Short, "a").unwrap();
    let b = run.large(100 << 10, 16, Lifetime::Short, "b").unwrap();
    let c = run.large(100 << 10, 16, Lifetime::Short, "c").unwrap();
    let d = run.large(100 << 10, 16, Lifetime::Short, "d").unwrap();
    assert!(d.ptr < c.ptr && c.ptr < b.ptr && b.ptr < a.ptr);
    assert!(objects.iter().all(|o| o.ptr < d.ptr));
    // Free c, then the two upper pages, then d: one free block spans the
    // pages and the large blocks.
    let at = |run: &Run, p: *mut u8| run.large.iter().position(|l| l.ptr == p).unwrap();
    run.drop_large(at(&run, c.ptr));
    for item in &objects[2 * per_page..] {
        let index = run.small.iter().position(|l| l.ptr == item.ptr).unwrap();
        run.drop_small(index);
    }
    run.check("pages and c freed");
    assert_eq!(run.heap.stats().free_blocks, 2);
    run.drop_large(at(&run, d.ptr));
    run.check("d freed");
    let stats = run.heap.stats();
    assert_eq!(stats.free_blocks, 1);
    let span = c.ptr.addr() + (100 << 10) - (objects[2 * per_page].ptr.addr());
    assert_eq!(run.heap.largest_free_large(), span - BLOCK_HEADER);
    run.clear();
}

/// The failure this pool exists to prevent: a spike of small objects, then
/// most of them freed with a few survivors scattered over the spike's pages
/// (one on its topmost page), then a large phase that needs more than the
/// memory above the spike's topmost page. Shaped like an 80 MiB module
/// (77.5 MiB region, floors 16 and 40 MiB): a long-lived 3.5 MB, 9 MB of
/// resident small objects, a spike to 35 MB of small objects, then large
/// blocks up to 40.5 MB live, a vector doubled from 3.5 to 7 MiB and 5 MB
/// buffers. While free pages returned to the large side only from the top of
/// the slab, the large side could never reach below the slab's topmost page
/// and the doubling failed. The large phase fits above the spike's pages, so
/// it stays there and leaves the drained pages to the small side; the
/// doubling and the buffers then take the drained pages.
#[test]
fn a_spike_of_small_objects_leaves_its_pages_to_large_blocks() {
    const MB: usize = 1_000_000;
    let mut run = Run::new(80 * MIB - (5 << 19), 16 * MIB, 40 * MIB);
    let mut rng = Rng(2026);
    // Long-lived blocks: two fixed buffers and a few caches.
    run.large(MIB, 16, Lifetime::Long, "io").unwrap();
    run.large(64 << 10, 16, Lifetime::Long, "status").unwrap();
    for _ in 0..12 {
        run.large(200_000, 16, Lifetime::Long, "cache").unwrap();
    }
    // Resident small objects, kept throughout.
    while run.heap.stats().small.live < 9 * MB {
        run.small(small_size(&mut rng)).unwrap();
    }
    let resident: BTreeSet<usize> = run
        .small
        .iter()
        .map(|l| l.ptr.addr() & !(PAGE_BYTES - 1))
        .collect();
    let resident_objects = run.small.len();
    // The spike.
    while run.heap.stats().small.live < 35 * MB {
        run.small(small_size(&mut rng)).unwrap();
    }
    run.check("the spike");
    let spike_stats = run.heap.stats();
    // Survivors: one object on every third page of the spike's lower third,
    // none in its middle third and the first 6 MiB of its upper third, one on
    // every fourth page above, and one on the topmost page.
    let mut by_page: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, item) in run.small.iter().enumerate().skip(resident_objects) {
        let page = item.ptr.addr() & !(PAGE_BYTES - 1);
        if !resident.contains(&page) {
            by_page.entry(page).or_default().push(i);
        }
    }
    let pages: Vec<usize> = by_page.keys().copied().collect();
    let (third, n) = (pages.len() / 3, pages.len());
    let stretch = 6 * MIB / PAGE_BYTES;
    let mut doomed = Vec::new();
    for (k, page) in pages.iter().enumerate() {
        let survivor = if k == n - 1 {
            true
        } else if k < third {
            k % 3 == 0
        } else if k < 2 * third + stretch {
            false
        } else {
            k % 4 == 0
        };
        let objects = &by_page[page];
        doomed.extend_from_slice(if survivor { &objects[1..] } else { objects });
    }
    doomed.sort_unstable_by(|a, b| b.cmp(a));
    for index in doomed {
        run.drop_small(index);
    }
    let spike_top = pages[n - 1] + PAGE_BYTES;
    run.check("the drain");
    let drained = run.heap.stats();
    eprintln!(
        "spike: small live {} B in {} B of pages; drained: small live {} B in {} B of pages, \
         {} free pages, largest free {} B, free large {} B, {} B above the spike's top page",
        spike_stats.small.live,
        spike_stats.small.capacity,
        drained.small.live,
        drained.small.capacity,
        drained.free_pages,
        drained.largest_free_large,
        drained.free_large,
        run.model.end - spike_top,
    );
    assert!(drained.largest_free_large >= 15 * MIB);

    // The large phase: blocks of 64 KiB to 1.5 MiB until 40.5 MB are live.
    while run.heap.stats().large.live < 40_500_000 {
        let size = rng.range(64 << 10, 3 << 19);
        run.large(size, 16, Lifetime::Short, "phase").unwrap();
    }
    run.check("the large phase");
    assert!(
        run.large
            .iter()
            .all(|l| l.ptr.addr() - BLOCK_HEADER >= spike_top),
        "a block of the large phase among the drained pages"
    );
    // A vector doubling from 56 KiB through 3.5 MiB to 7 MiB, as `Vec` grows.
    let mut v = run.large(56 << 10, 1, Lifetime::Short, "vector").unwrap();
    let index = run.large.len() - 1;
    while v.size < 7 * MIB {
        run.resize(index, v.size * 2);
        let grown = run.large[index];
        assert_eq!(grown.size, v.size * 2, "{} -> {} B", v.size, v.size * 2);
        v = grown;
    }
    run.check("the doubling");
    // Then 5 MB buffers while the model's free memory holds one.
    let mut buffers = 0;
    while run.model.largest_large() >= 5 * MB + SLACK {
        run.large(5 * MB, 16, Lifetime::Short, "5 MB").unwrap();
        buffers += 1;
    }
    let _ = run.large(5 * MB, 16, Lifetime::Short, "one more");
    run.check("the 5 MB buffers");
    let full = run.heap.stats();
    eprintln!(
        "large phase: large live {} B with a 7 MiB vector and {buffers} buffers of 5 MB; \
         largest free {} B; {} large blocks served, {} refused",
        full.large.live, full.largest_free_large, run.served, run.refused,
    );
    assert!(buffers >= 1);
    // The large side holds more than the memory above the spike's top page:
    // it reuses pages the small side gave back below it.
    assert!(full.large.live > run.model.end - spike_top);
    assert!(run.large.iter().any(|l| l.ptr.addr() < spike_top));
    assert_eq!(full.small.failed, 0);

    // The large phase ends; the next small spike reuses the freed pages from
    // the bottom up, and the large side keeps its floor.
    let long = 14;
    while run.large.len() > long {
        run.drop_large(run.large.len() - 1);
    }
    while run.heap.stats().small.live < 35 * MB {
        run.small(small_size(&mut rng)).unwrap();
    }
    run.check("the second spike");
    let second = run.heap.stats();
    assert_eq!(second.small.failed, 0);
    assert!(second.largest_free_large >= 40 * MIB - 4 * MB);
    eprintln!(
        "second spike: small live {} B in {} B of pages, largest free {} B",
        second.small.live, second.small.capacity, second.largest_free_large,
    );
    run.clear();
}

/// Large blocks take memory no page may use before memory the small side
/// could use: after the small side filled everything below the page limit
/// and was drained to one object on every eighth page, large blocks held
/// through the next spike stay above the limit while it holds them, so the
/// next spike gets every page back. Placed by best fit, they took the holes
/// among the drained pages instead, and the next spike fell short by those
/// pages with the memory above the limit still free.
#[test]
fn large_blocks_leave_drained_pages_to_the_next_spike() {
    for hint in [Lifetime::Short, Lifetime::Long] {
        for (bytes, large_floor, size) in
            [(8 * MIB, 4 * MIB, 300_000), (16 * MIB, 6 * MIB, 700_000)]
        {
            let what = format!("{hint:?} {size} B blocks on {bytes} B");
            let mut run = Run::new(bytes, 0, large_floor);
            while run.small(MAX_SMALL).is_some() {}
            let first = run.heap.stats().small.capacity;
            assert!(
                first + PAGE_BYTES > run.model.page_limit - run.model.lo,
                "{what}"
            );
            let mut kept = BTreeSet::new();
            let mut i = 0;
            while i < run.small.len() {
                let page = (run.small[i].ptr.addr() - run.model.lo) / PAGE_BYTES;
                if page.is_multiple_of(8) && kept.insert(page) {
                    i += 1;
                } else {
                    run.drop_small(i);
                }
            }
            run.check(&what);
            for _ in 0..8 {
                let block = run.large(size, 16, hint, &what).unwrap();
                assert!(
                    block.ptr.addr() - BLOCK_HEADER >= run.model.page_limit,
                    "{what}: a block among the drained pages"
                );
            }
            while run.small(MAX_SMALL).is_some() {}
            run.check(&what);
            assert_eq!(run.heap.stats().small.capacity, first, "{what}");
            run.clear();
        }
    }
}

/// The same with random object sizes, one survivor on a random part of the
/// pages, a small floor, and random large blocks of either lifetime that
/// together take up to three quarters of the memory above the limit.
#[test]
fn large_blocks_of_random_sizes_leave_drained_pages_to_the_next_spike() {
    for seed in 1..=4 {
        let what = format!("seed {seed}");
        let mut rng = Rng(seed);
        let large_floor = 5 * MIB;
        let mut run = Run::new(12 * MIB, MIB, large_floor);
        while run.small(small_size(&mut rng)).is_some() {}
        let first = run.heap.stats().small.capacity;
        let mut kept = BTreeSet::new();
        let mut i = 0;
        while i < run.small.len() {
            let page = (run.small[i].ptr.addr() - run.model.lo) / PAGE_BYTES;
            if page.is_multiple_of(rng.range(2, 8)) && kept.insert(page) {
                i += 1;
            } else {
                run.drop_small(i);
            }
        }
        run.check(&what);
        let mut total = 0;
        loop {
            let size = large_size(&mut rng, MIB);
            if total + size + BLOCK_HEADER > large_floor * 3 / 4 {
                break;
            }
            let hint = if rng.chance(30) {
                Lifetime::Long
            } else {
                Lifetime::Short
            };
            let block = run.large(size, 16, hint, &what).unwrap();
            assert!(
                block.ptr.addr() - BLOCK_HEADER >= run.model.page_limit,
                "{what}: a block among the drained pages"
            );
            total += size + BLOCK_HEADER;
        }
        while run.small(small_size(&mut rng)).is_some() {}
        run.check(&what);
        assert_eq!(run.heap.stats().small.capacity, first, "{what}");
        run.clear();
    }
}

/// A long-lived block packs against the top: with no floors, it takes free
/// memory above the slab's topmost page before a larger hole among the
/// pages, and a hole only once nothing above holds it. Taking the largest
/// free block, it settled among the pages, where it would split the free
/// memory once they were given back.
#[test]
fn long_lived_blocks_pack_against_the_top_before_holes() {
    let mut run = Run::new(8 * MIB, 0, 0);
    run.large(3 * MIB, 16, Lifetime::Long, "top").unwrap();
    while run.heap.largest_free_large() > 600 << 10 {
        run.small(MAX_SMALL).unwrap();
    }
    let lo = run.model.lo;
    let spike_top = run
        .small
        .iter()
        .map(|l| (l.ptr.addr() & !(PAGE_BYTES - 1)) + PAGE_BYTES)
        .max()
        .unwrap();
    // A 2 MiB hole in the middle of the pages, larger than the memory left
    // above them.
    let hole = lo + 16 * PAGE_BYTES..lo + 48 * PAGE_BYTES;
    let mut i = 0;
    while i < run.small.len() {
        if hole.contains(&run.small[i].ptr.addr()) {
            run.drop_small(i);
        } else {
            i += 1;
        }
    }
    run.check("the hole");
    assert_eq!(
        run.heap.largest_free_large(),
        32 * PAGE_BYTES - BLOCK_HEADER
    );
    let above = run
        .model
        .runs()
        .iter()
        .find(|r| r.0 == spike_top)
        .map_or(0, |r| r.1 - r.0);
    assert!(
        above < 600 << 10 && above > 300_000 + 2 * BLOCK_HEADER,
        "{above}"
    );
    let long = run.large(300_000, 16, Lifetime::Long, "long").unwrap();
    assert!(
        long.ptr.addr() > spike_top,
        "a long-lived block among the pages"
    );
    // Nothing above holds a second one: it takes the hole.
    let second = run.large(300_000, 16, Lifetime::Long, "second").unwrap();
    assert!(hole.contains(&second.ptr.addr()));
    run.check("both placed");
    run.clear();
}

/// The floors' guarantees hold to the byte when the two floors fill the
/// region and the small floor ends a few bytes past a page boundary: the
/// large side reaches `large_floor` less two headers, and the small side
/// every whole page of `small_floor`. (A line moved 16 bytes up for the
/// sliver left the large side 16 bytes short.)
#[test]
fn floors_that_fill_the_region_keep_both_guarantees_to_the_byte() {
    let heap = heap(4 * MIB, 0, 0);
    let region = heap.stats().region;
    for sliver in [0, 8, 16, 17, 24, 31, 32, 40, 48] {
        let small_floor = 3 * PAGE_BYTES + sliver;
        let large_floor = region - small_floor;
        heap.configure(Config::new(small_floor, large_floor))
            .unwrap();
        let largest = heap.largest_free_large();
        assert!(
            largest >= large_floor - 2 * BLOCK_HEADER,
            "sliver {sliver}: the large side reaches {largest} B of its {large_floor} B floor"
        );
        let big = alloc(heap, large_floor - 2 * BLOCK_HEADER, 16, Lifetime::Short);
        assert!(!big.is_null(), "sliver {sliver}");
        let small: Vec<*mut u8> = (0..)
            .map(|_| alloc(heap, MAX_SMALL, 8, Lifetime::Short))
            .take_while(|p| !p.is_null())
            .collect();
        assert_eq!(
            small.len(),
            3 * PAGE_BYTES / MAX_SMALL,
            "sliver {sliver}: the small side's whole pages"
        );
        for p in small {
            free(heap, p, MAX_SMALL, 8);
        }
        free(heap, big, large_floor - 2 * BLOCK_HEADER, 16);
        let stats = heap.stats();
        assert_eq!(
            (stats.small.live, stats.large.live, stats.free_blocks),
            (0, 0, 1)
        );
    }
}

/// Phases of small spikes and drains and large spikes and drains, with
/// reallocations, on heaps with and without floors: every large request
/// succeeds when the model's free memory (whole free pages and the space
/// between large blocks) has a run that holds it, and the heap's own
/// prediction is exact.
fn mixed(seed: u64, bytes: usize, small_floor: usize, large_floor: usize, rounds: usize) {
    let mut run = Run::new(bytes, small_floor, large_floor);
    let mut rng = Rng(seed);
    let budget = bytes / 2;
    for round in 0..rounds {
        let what = format!("seed {seed} round {round}");
        match round % 4 {
            0 => {
                // A small spike with some churn.
                for _ in 0..3000 {
                    if rng.chance(85) || run.small.is_empty() {
                        if run.heap.stats().small.live < budget {
                            let _ = run.small(small_size(&mut rng));
                        }
                    } else {
                        run.drop_small(rng.range(0, run.small.len() - 1));
                    }
                }
            }
            1 => {
                // Drain most small objects, scattered.
                let keep = run.small.len() / 10;
                while run.small.len() > keep {
                    run.drop_small(rng.range(0, run.small.len() - 1));
                }
            }
            2 => {
                // A large spike: allocations, frees and reallocations.
                for _ in 0..1500 {
                    match rng.range(0, 99) {
                        0..=49 => {
                            let size = large_size(&mut rng, bytes / 8);
                            let align = if rng.chance(90) {
                                16
                            } else {
                                1 << rng.range(5, 16)
                            };
                            let hint = if rng.chance(25) {
                                Lifetime::Long
                            } else {
                                Lifetime::Short
                            };
                            let _ = run.large(size, align, hint, &what);
                        }
                        50..=79 if !run.large.is_empty() => {
                            run.drop_large(rng.range(0, run.large.len() - 1));
                        }
                        80..=94 if !run.large.is_empty() => {
                            let index = rng.range(0, run.large.len() - 1);
                            let size = run.large[index].size;
                            let new = if rng.chance(70) {
                                size * 2
                            } else {
                                size / 2 + 1
                            };
                            run.resize(index, new.max(MAX_SMALL + 1));
                        }
                        _ => {
                            let _ = run.small(small_size(&mut rng));
                        }
                    }
                }
            }
            _ => {
                // Drain most large blocks.
                let keep = run.large.len() / 5;
                while run.large.len() > keep {
                    run.drop_large(rng.range(0, run.large.len() - 1));
                }
            }
        }
        run.check(&what);
    }
    assert!(
        run.served > 1000 && run.refused > 0,
        "{} served, {} refused",
        run.served,
        run.refused
    );
    run.clear();
}

#[test]
fn mixed_phases_without_floors() {
    for seed in 1..=3 {
        mixed(seed, 16 * MIB, 0, 0, 16);
    }
}

#[test]
fn mixed_phases_with_floors() {
    for seed in 11..=13 {
        mixed(seed, 16 * MIB, 2 * MIB, 4 * MIB, 16);
    }
}

#[test]
fn mixed_phases_with_a_large_floor() {
    for seed in 21..=23 {
        mixed(seed, 24 * MIB, 0, 8 * MIB, 20);
    }
}

#[test]
fn configure_refiles_free_memory_under_new_floors() {
    let mut run = Run::new(8 * MIB, 0, 0);
    let mut rng = Rng(5);
    for _ in 0..4000 {
        run.small(small_size(&mut rng)).unwrap();
    }
    for _ in 0..20 {
        run.large(large_size(&mut rng, 256 << 10), 16, Lifetime::Short, "fill")
            .unwrap();
    }
    while run.small.len() > 400 {
        run.drop_small(rng.range(0, run.small.len() - 1));
    }
    run.check("before");
    let before = run.heap.stats();
    // Raise both floors past live memory: nothing moves, but large blocks
    // may no longer start below the small floor nor pages end above the
    // large floor.
    let (small_floor, large_floor) = (3 * MIB, 3 * MIB);
    run.heap
        .configure(Config::new(small_floor, large_floor))
        .unwrap();
    run.model.lines(small_floor, large_floor);
    run.check("raised");
    let raised = run.heap.stats();
    assert!(raised.free_large < before.free_large);
    assert!(raised.free_pages < before.free_pages);
    for _ in 0..200 {
        let _ = run.large(large_size(&mut rng, MIB), 16, Lifetime::Short, "raised");
        let _ = run.small(small_size(&mut rng));
    }
    run.check("used under the raised floors");
    // Back to no floors: the same free memory is filed as before.
    while run.large.len() > 20 {
        run.drop_large(run.large.len() - 1);
    }
    while run.small.len() > 400 {
        run.drop_small(run.small.len() - 1);
    }
    run.heap.configure(Config::DEFAULT).unwrap();
    run.model.lines(0, 0);
    run.check("lowered");
    run.clear();
}

/// Each floor stays a guarantee however the sides interleave: with the small
/// side's pages scattered over the region and the large side filling the
/// rest, the large side can still reach `large_floor` contiguous bytes once
/// its own blocks are gone, and the small side `small_floor` bytes of pages.
#[test]
fn floors_stay_minimum_guarantees_under_interleaving() {
    let (small_floor, large_floor) = (2 * MIB, 3 * MIB);
    let mut run = Run::new(12 * MIB, small_floor, large_floor);
    let mut rng = Rng(77);
    // The small side as far as it goes, then scattered survivors.
    while run.small(small_size(&mut rng)).is_some() {}
    let stats = run.heap.stats();
    assert!(stats.small.capacity <= stats.region - large_floor);
    assert!(stats.small.capacity + PAGE_BYTES > stats.region - large_floor);
    while run.small.len() > 300 {
        run.drop_small(rng.range(0, run.small.len() - 1));
    }
    // The large side as far as it goes.
    while run
        .large(large_size(&mut rng, 512 << 10), 16, Lifetime::Short, "fill")
        .is_some()
    {}
    while run
        .large(MAX_SMALL + 1, 16, Lifetime::Short, "fill")
        .is_some()
    {}
    run.check("both sides full");
    // Without its own blocks the large side reaches its floor contiguously.
    while !run.large.is_empty() {
        run.drop_large(run.large.len() - 1);
    }
    run.check("large side empty");
    assert!(run.heap.largest_free_large() >= large_floor - 2 * BLOCK_HEADER);
    // With the large side full and the small side empty, the small side
    // still gets its floor in pages.
    while !run.small.is_empty() {
        run.drop_small(run.small.len() - 1);
    }
    while run
        .large(
            large_size(&mut rng, 512 << 10),
            16,
            Lifetime::Short,
            "refill",
        )
        .is_some()
    {}
    while run
        .large(MAX_SMALL + 1, 16, Lifetime::Short, "refill")
        .is_some()
    {}
    let mut pages = BTreeSet::new();
    while let Some(live) = run.small(MAX_SMALL) {
        pages.insert(live.ptr.addr() & !(PAGE_BYTES - 1));
    }
    assert!(pages.len() * PAGE_BYTES >= small_floor);
    run.check("small side refilled");
    run.clear();
}

#[test]
fn scope_chunks_reuse_freed_pages() {
    let mut run = Run::new(16 * MIB, 0, 0);
    let mut rng = Rng(3);
    while run.heap.stats().small.live < 10 * MIB {
        run.small(small_size(&mut rng)).unwrap();
    }
    // A block at the top of what is left, so only the freed pages remain.
    let rest = run.heap.largest_free_large();
    run.large(rest, 16, Lifetime::Short, "rest").unwrap();
    let topmost = run
        .small
        .iter()
        .max_by_key(|l| l.ptr.addr())
        .map(|l| l.ptr)
        .unwrap();
    // Free every small object but the topmost one.
    let mut i = 0;
    while i < run.small.len() {
        if run.small[i].ptr == topmost {
            i += 1;
        } else {
            run.drop_small(i);
        }
    }
    run.check("drained");
    let before = run.heap.stats();
    assert!(before.largest_free_large > 9 * MIB);
    // A scope of 1 MiB chunks takes them from the freed pages and gives them
    // back when it ends.
    let scope = Scope::new("pages", 8 * MIB, MIB);
    let objects: Vec<*mut u8> = scope.run(|| {
        (0..7)
            .map(|_| alloc(run.heap, 900 << 10, 16, Lifetime::Short))
            .collect()
    });
    assert!(
        objects
            .iter()
            .all(|p| !p.is_null() && p.addr() < topmost.addr())
    );
    for p in objects {
        free(run.heap, p, 900 << 10, 16);
    }
    let done = scope.finish();
    assert_eq!(done.parked, 0);
    let after = run.heap.stats();
    assert_eq!(after.large.live, before.large.live);
    assert_eq!(after.largest_free_large, before.largest_free_large);
    run.clear();
}
