//! [`FixedHeap`]: the global allocator over one fixed region, its pages and
//! large blocks drawn from one pool.

#![allow(unsafe_code)]

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::fmt;
use core::ptr;

use crate::ctx::{self, Lifetime};
use crate::raw::{Mem, align_down, align_up};
use crate::scope::{ScopeSummary, SummaryTable};
use crate::slab::{self, CLASS_COUNT, CLASSES, PAGE, Slab};
use crate::tlsf::{self, HEADER, Lines, TAG, Tlsf};

const WORD: usize = size_of::<usize>();

/// The split configuration of a [`FixedHeap`]: the two floors.
///
/// The floors are minimum guarantees. No slab page lies in the top
/// `large_floor` bytes of the region, and no large block starts in the
/// bottom `small_floor` bytes (save in at most their last 31 bytes, past
/// their last whole page, where no page fits), so the large side can always
/// reach `large_floor` contiguous bytes (less two block headers) and the
/// small side every whole page of `small_floor`. Between the floors neither
/// side owns an address range: free pages and free large blocks are one
/// pool both sides draw from.
///
/// Each floor also caps the other side: the small side can hold at most the
/// region less `large_floor` in pages, and the large side about the region
/// less `small_floor`. Set each floor to the least its side must always be
/// able to reach, not to its expected peak; free memory between the floors
/// serves whichever side asks for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Bytes at the bottom of the region that the large side never claims.
    pub small_floor: usize,
    /// Bytes at the top of the region that the small side never claims.
    pub large_floor: usize,
}

impl Config {
    /// No floors: both sides may use the whole region.
    pub const DEFAULT: Self = Self::new(0, 0);

    /// A configuration with the given floors.
    pub const fn new(small_floor: usize, large_floor: usize) -> Self {
        Self {
            small_floor,
            large_floor,
        }
    }
}

/// Why a region or configuration was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    /// The heap already manages a region.
    AlreadyInitialized,
    /// The region cannot hold the page table, one page and the pool's end
    /// marker.
    TooSmall,
    /// The two floors together exceed the usable region.
    Floors {
        /// Usable bytes of the region.
        usable: usize,
        /// `small_floor + large_floor`.
        requested: usize,
    },
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => f.write_str("heap is already initialized"),
            Self::TooSmall => f.write_str("region is too small for a heap"),
            Self::Floors { usable, requested } => {
                write!(
                    f,
                    "floors of {requested} bytes exceed the usable {usable} bytes"
                )
            }
        }
    }
}

impl core::error::Error for InitError {}

/// Counters of one side of the heap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideStats {
    /// Live bytes (slab: at class size; large: whole blocks, headers included).
    pub live: usize,
    /// Highest `live` since creation or [`FixedHeap::reset_peaks`].
    pub peak: usize,
    /// Bytes this side currently holds in the region: the small side its
    /// pages, the large side its blocks and the pool's 16-byte end marker.
    pub capacity: usize,
    /// The most this side could hold right now: `capacity` and the free
    /// memory it could take (the small side whole free pages below the large
    /// floor, the large side free blocks above the small floor).
    pub limit: usize,
    /// Allocations this side refused.
    pub failed: usize,
}

/// Live objects and pages of one slab class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassStats {
    /// Object size of the class.
    pub size: usize,
    /// Live objects.
    pub objects: usize,
    /// Pages held.
    pub pages: usize,
}

/// A snapshot of a [`FixedHeap`]'s counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Usable bytes between the page table and the end of the region.
    pub region: usize,
    /// The small (slab) side.
    pub small: SideStats,
    /// The large (TLSF) side, scope chunks included.
    pub large: SideStats,
    /// Per-class histogram of the small side.
    pub classes: [ClassStats; CLASS_COUNT],
    /// Whole free pages the small side could take now (below the large
    /// floor), wherever they lie.
    pub free_pages: usize,
    /// Free blocks in the region (a fresh heap has one).
    pub free_blocks: usize,
    /// Free bytes in the region, held by neither side (headers of free
    /// blocks included): `region - small.capacity - large.capacity`.
    pub gap: usize,
    /// Payload bytes the large side could hand out now, summed over the free
    /// blocks above the small floor (see [`FixedHeap::free_large`]).
    pub free_large: usize,
    /// The largest single large allocation that would succeed now.
    pub largest_free_large: usize,
    /// Allocations a scope refused at its cap.
    pub scope_cap_refusals: usize,
    /// Scope chunks still held by objects of scopes that have ended.
    pub parked_chunks: usize,
    /// Bytes of those chunks.
    pub parked_bytes: usize,
    /// Highest bytes both sides held together (`small.capacity +
    /// large.capacity`) since creation or reset.
    pub high_water: usize,
}

/// The heap's state; accessed only under the heap's lock.
pub(crate) struct Inner {
    ready: bool,
    /// A lazy heap has asked its [`RegionFn`] for its region (once, whether
    /// or not it got one).
    asked: bool,
    pub(crate) mem: Mem,
    /// Address of the pool's sentinel header; the region's usable end is
    /// `top + HEADER`.
    top: usize,
    /// Address of the region's first page.
    lo: usize,
    config: Config,
    pub(crate) slab: Slab,
    pub(crate) tlsf: Tlsf,
    small_peak: usize,
    large_peak: usize,
    small_failed: usize,
    pub(crate) large_failed: usize,
    pub(crate) cap_refusals: usize,
    /// Set when the current call was refused by a scope's cap (not by
    /// exhaustion); cleared at the start of every allocation and resize.
    pub(crate) cap_refused: bool,
    pub(crate) parked_chunks: usize,
    pub(crate) parked_bytes: usize,
    high_water: usize,
    baseline: usize,
    pub(crate) summaries: SummaryTable,
}

impl Inner {
    const fn new(config: Config) -> Self {
        Self {
            ready: false,
            asked: false,
            mem: Mem::none(),
            top: 0,
            lo: 0,
            config,
            slab: Slab::new(),
            tlsf: Tlsf::new(),
            small_peak: 0,
            large_peak: 0,
            small_failed: 0,
            large_failed: 0,
            cap_refusals: 0,
            cap_refused: false,
            parked_chunks: 0,
            parked_bytes: 0,
            high_water: 0,
            baseline: 0,
            summaries: SummaryTable::new(),
        }
    }

    /// # Safety
    /// `[base, base + len)` must be valid for reads and writes, unused by
    /// anything else, and outlive every use of the heap.
    unsafe fn init(&mut self, base: *mut u8, len: usize) -> Result<(), InitError> {
        if self.ready {
            return Err(InitError::AlreadyInitialized);
        }
        let start = base.addr();
        let end = start.checked_add(len).ok_or(InitError::TooSmall)?;
        let table = align_up(start, 16).ok_or(InitError::TooSmall)?;
        let max_pages = len / PAGE;
        let pool_table = table
            .checked_add(Slab::table_bytes(max_pages))
            .ok_or(InitError::TooSmall)?;
        let table_end = pool_table
            .checked_add(Tlsf::table_bytes(max_pages))
            .ok_or(InitError::TooSmall)?;
        let lo = align_up(table_end, PAGE).ok_or(InitError::TooSmall)?;
        let usable_end = align_down(end, tlsf::GRANULE);
        if usable_end < lo + PAGE + HEADER {
            return Err(InitError::TooSmall);
        }
        let top = usable_end - HEADER;
        let usable = usable_end - lo;
        let requested = self
            .config
            .small_floor
            .saturating_add(self.config.large_floor);
        if requested > usable {
            return Err(InitError::Floors { usable, requested });
        }
        let pages = ((top - lo) / PAGE).min(max_pages);
        self.mem = Mem::new(base);
        self.lo = lo;
        self.top = top;
        self.slab
            .init(table, u32::try_from(pages).unwrap_or(u32::MAX), lo);
        let lines = self.lines(self.config);
        // SAFETY: the pool's table follows the slab's inside the region and
        // ends below `lo`; `[lo, top + HEADER)` is the rest of the region,
        // `top` is 16-aligned and at least a page above `lo`.
        unsafe {
            self.tlsf
                .init(self.mem, pool_table, pages, (lo, top), lines);
        }
        self.ready = true;
        self.note();
        Ok(())
    }

    fn usable_end(&self) -> usize {
        self.top + HEADER
    }

    fn usable(&self) -> usize {
        self.usable_end() - self.lo
    }

    /// The two lines `config` draws: the lowest address a large block may
    /// start at (16-aligned) and the highest address a page may end at.
    ///
    /// The first is the small floor's end rounded up to 16 bytes, but never
    /// above the page limit (the floors may fill the region), and the page
    /// boundary below that when it lies less than [`tlsf::MIN_BLOCK`] bytes
    /// above one: a large block starting in such a sliver would leave too
    /// little above the floor's last whole page for a free block, and the
    /// small side could not take that page. A sliver could never be a page,
    /// so the small side keeps every whole page of its floor; the line stays
    /// at or below the page limit, so the large side keeps all of its floor.
    fn lines(&self, config: Config) -> Lines {
        let page_limit = self.usable_end().saturating_sub(config.large_floor);
        let floor = align_up(self.lo.saturating_add(config.small_floor), tlsf::GRANULE)
            .unwrap_or(usize::MAX)
            .min(align_down(page_limit, tlsf::GRANULE));
        let floor = match floor - align_down(floor, PAGE) {
            1..tlsf::MIN_BLOCK => align_down(floor, PAGE),
            _ => floor,
        };
        Lines { floor, page_limit }
    }

    pub(crate) fn note(&mut self) {
        self.small_peak = self.small_peak.max(self.slab.live_bytes);
        self.large_peak = self.large_peak.max(self.tlsf.used_bytes);
        self.high_water = self.high_water.max(self.claimed());
    }

    /// Bytes both sides hold: the slab's pages, the large blocks and the
    /// pool's end marker.
    fn claimed(&self) -> usize {
        self.small_capacity() + self.large_capacity()
    }

    fn small_capacity(&self) -> usize {
        self.slab.pages as usize * PAGE
    }

    fn large_capacity(&self) -> usize {
        self.tlsf.used_bytes + HEADER
    }

    /// Whether `addr` lies in a slab page.
    #[inline]
    fn in_small(&self, addr: usize) -> bool {
        addr >= self.lo && self.tlsf.is_page(self.mem, align_down(addr, PAGE))
    }

    /// Whether `addr` lies in the pool outside any slab page (a large block
    /// or a scope chunk, once [`Self::in_small`] said no).
    #[inline]
    fn in_large(&self, addr: usize) -> bool {
        addr >= self.lo && addr < self.top
    }

    /// The scope chunk of a scope object, or `None` for a plain large block.
    ///
    /// # Safety
    /// `addr` must be a live large-side allocation.
    #[inline]
    pub(crate) unsafe fn tag_of(&self, addr: usize) -> Option<usize> {
        // SAFETY: the word below a large-side allocation is either its block's
        // size word or a scope object's tag word.
        let word = unsafe { self.mem.word(addr - WORD) };
        (word & TAG != 0).then_some(word & !tlsf::FLAGS)
    }

    /// Allocates outside any scope: small layouts from the slab side, the
    /// rest from the large side.
    pub(crate) fn alloc_general(&mut self, layout: Layout, hint: Lifetime) -> Option<usize> {
        if !self.ready {
            return None;
        }
        let addr = if let Some(class) = slab::class_for(layout.size(), layout.align()) {
            let addr = self.slab.alloc(self.mem, &mut self.tlsf, class);
            if addr.is_none() {
                self.small_failed += 1;
            }
            addr
        } else {
            self.alloc_large(layout.size(), layout.align(), hint == Lifetime::Long)
        };
        self.note();
        addr
    }

    /// Allocates from the large side regardless of size.
    pub(crate) fn alloc_large(&mut self, size: usize, align: usize, long: bool) -> Option<usize> {
        let addr = self.tlsf.alloc(self.mem, size, align, long);
        if addr.is_none() {
            self.large_failed += 1;
        }
        addr
    }

    /// Frees an allocation by address, routing it to its owner.
    ///
    /// # Safety
    /// `addr` must be a live allocation of this heap made with `layout`.
    pub(crate) unsafe fn free(&mut self, addr: usize, layout: Layout) {
        if self.in_small(addr) {
            // SAFETY: the address is a live slab object.
            unsafe { self.slab.free(self.mem, &mut self.tlsf, addr) };
        } else if self.in_large(addr) {
            // SAFETY: the address is a live large allocation.
            unsafe {
                match self.tag_of(addr) {
                    Some(chunk) => self.scope_free(chunk, layout.size()),
                    None => self.tlsf.free(self.mem, addr),
                }
            }
        }
    }

    /// Resizes an allocation on its owner's side; `None` leaves it intact.
    ///
    /// A large block grows in place into a free upper neighbour; failing
    /// that, it grows down into a free lower neighbour (moving its bytes);
    /// otherwise it moves to a new block.
    ///
    /// # Safety
    /// `addr` must be a live allocation of this heap made with `layout`.
    unsafe fn realloc(&mut self, addr: usize, layout: Layout, new_size: usize) -> Option<usize> {
        let new_layout = Layout::from_size_align(new_size, layout.align()).ok()?;
        let keep = layout.size().min(new_size);
        if self.in_small(addr) {
            // SAFETY: the address is a live slab object.
            let class = unsafe { self.slab.class_of(self.mem, addr) };
            if new_size <= CLASSES[class] {
                return Some(addr);
            }
            let to = self.alloc_general(new_layout, ctx::lifetime())?;
            // SAFETY: both are live, distinct allocations of at least `keep` bytes.
            unsafe {
                self.mem.copy(addr, to, keep);
                self.slab.free(self.mem, &mut self.tlsf, addr);
            }
            return Some(to);
        }
        if !self.in_large(addr) {
            return None;
        }
        // SAFETY: the address is a live large allocation.
        if let Some(chunk) = unsafe { self.tag_of(addr) } {
            // SAFETY: forwarded; `chunk` is the object's scope chunk.
            return unsafe { self.scope_realloc(chunk, addr, layout, new_layout) };
        }
        // SAFETY: the address is a live TLSF block.
        unsafe {
            if self.tlsf.resize_in_place(self.mem, addr, new_size) {
                self.note();
                return Some(addr);
            }
            if let Some(to) = self
                .tlsf
                .grow_down(self.mem, addr, new_size, layout.align(), keep)
            {
                self.note();
                return Some(to);
            }
            let long = Tlsf::is_long(self.mem, addr);
            let to = self.alloc_large(new_size, layout.align(), long)?;
            self.mem.copy(addr, to, keep);
            self.tlsf.free(self.mem, addr);
            self.note();
            Some(to)
        }
    }

    fn free_large(&self) -> usize {
        self.tlsf.free_payload()
    }

    fn largest_free_large(&self) -> usize {
        self.tlsf.largest_free(self.mem).saturating_sub(HEADER)
    }

    fn stats(&self) -> Stats {
        if !self.ready {
            return Stats::default();
        }
        let mut classes = [ClassStats::default(); CLASS_COUNT];
        for (i, class) in classes.iter_mut().enumerate() {
            *class = ClassStats {
                size: CLASSES[i],
                objects: self.slab.objects[i],
                pages: self.slab.class_pages[i] as usize,
            };
        }
        let (small, large) = (self.small_capacity(), self.large_capacity());
        Stats {
            region: self.usable(),
            small: SideStats {
                live: self.slab.live_bytes,
                peak: self.small_peak,
                capacity: small,
                limit: small + self.tlsf.free_pages * PAGE,
                failed: self.small_failed,
            },
            large: SideStats {
                live: self.tlsf.used_bytes,
                peak: self.large_peak,
                capacity: large,
                limit: large + self.tlsf.filed_bytes,
                failed: self.large_failed,
            },
            classes,
            free_pages: self.tlsf.free_pages,
            free_blocks: self.tlsf.free_blocks,
            gap: self.tlsf.free_bytes,
            free_large: self.free_large(),
            largest_free_large: self.largest_free_large(),
            scope_cap_refusals: self.cap_refusals,
            parked_chunks: self.parked_chunks,
            parked_bytes: self.parked_bytes,
            high_water: self.high_water,
        }
    }

    fn live(&self) -> usize {
        self.slab.live_bytes + self.tlsf.used_bytes
    }
}

#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
mod lock {
    use core::marker::PhantomData;

    /// No lock: single-threaded wasm32 has no other thread to exclude.
    pub(super) struct Lock;

    pub(super) struct Guard<'a>(PhantomData<&'a Lock>);

    impl Lock {
        pub(super) const fn new() -> Self {
            Self
        }

        #[inline]
        pub(super) fn lock(&self) -> Guard<'_> {
            Guard(PhantomData)
        }
    }
}

#[cfg(not(all(target_arch = "wasm32", not(target_feature = "atomics"))))]
mod lock {
    use core::hint::spin_loop;
    use core::sync::atomic::{AtomicBool, Ordering};

    /// A spin lock; the critical sections are O(1) and never allocate.
    pub(super) struct Lock(AtomicBool);

    pub(super) struct Guard<'a>(&'a Lock);

    impl Lock {
        pub(super) const fn new() -> Self {
            Self(AtomicBool::new(false))
        }

        #[inline]
        pub(super) fn lock(&self) -> Guard<'_> {
            while self
                .0
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                while self.0.load(Ordering::Relaxed) {
                    spin_loop();
                }
            }
            Guard(self)
        }
    }

    impl Drop for Guard<'_> {
        #[inline]
        fn drop(&mut self) {
            self.0.0.store(false, Ordering::Release);
        }
    }
}

/// A global allocator over one fixed region that never grows.
///
/// Small layouts (up to 2 KiB at their natural alignment) live in size-class
/// slab pages, larger ones in large blocks, and both are blocks of one TLSF
/// pool over the whole region. The small side takes the lowest free page and
/// gives a page back as soon as it is empty, wherever it lies; large blocks
/// are carved from the top of free blocks, from memory no page may use
/// whenever a free block there fits, so the pages a spike of small objects
/// gave back stay free for the next one; freed memory coalesces at once, so
/// either side can use what the other gave back. The [`Config`] floors
/// reserve one end of the region for each side. Allocation and deallocation
/// are O(1) on both sides (a large request that the good-fit search cannot
/// serve scans up to three free lists before it fails, which keeps
/// [`largest_free_large`](Self::largest_free_large) exact; taking a page
/// reads a bitmap of one bit per page from the lowest free page up, and
/// giving back the topmost page reads one from that page down); exhaustion
/// returns null; nothing ever asks the host for more memory.
///
/// While a [`Scope`](crate::Scope) is active on the calling thread, allocations go to that
/// scope's chunks instead (see [`Scope::run`](crate::Scope::run) and [`crate::Scoped`]).
/// Deallocation always routes by address to the owner.
///
/// # Example
///
/// ```
/// use core::alloc::{GlobalAlloc, Layout};
/// use rtao_arena::{Config, FixedHeap, Lifetime};
///
/// let heap: &'static FixedHeap = Box::leak(Box::new(FixedHeap::new(Config::DEFAULT)));
/// heap.init_static(Box::leak(vec![0u8; 4 << 20].into_boxed_slice()))?;
///
/// let small = Layout::from_size_align(24, 8)?;
/// let large = Layout::from_size_align(256 << 10, 64)?;
/// let a = heap.alloc_with(small, Lifetime::Short);
/// let b = heap.alloc_with(large, Lifetime::Long);
/// assert!(!a.is_null() && !b.is_null());
/// assert!(heap.stats().small.live >= 24);
/// // SAFETY: both were allocated by `heap` with these layouts.
/// unsafe {
///     heap.dealloc(a, small);
///     heap.dealloc(b, large);
/// }
/// assert_eq!(heap.stats().large.live, 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// On wasm32 the heap is usually the module's global allocator:
///
/// ```no_run
/// # #[cfg(target_arch = "wasm32")]
/// # mod wasm {
/// use rtao_arena::{Config, FixedHeap};
///
/// // SAFETY: this is the module's only allocator over its linear memory.
/// #[global_allocator]
/// static HEAP: FixedHeap = unsafe { FixedHeap::wasm(Config::new(16 << 20, 40 << 20)) };
/// # }
/// ```
pub struct FixedHeap {
    lock: lock::Lock,
    inner: UnsafeCell<Inner>,
    /// Where a lazy heap gets its region at its first use.
    lazy: Option<RegionFn>,
}

/// Supplies a lazy heap's region: `Some((base, len))` for the region
/// `[base, base + len)`, or `None` when there is none to be had (every
/// allocation then returns null). See [`FixedHeap::lazy`].
pub type RegionFn = fn() -> Option<(*mut u8, usize)>;

// SAFETY: all access to `inner` happens under `lock` (or, on single-threaded
// wasm32, on the only thread), and the region it manages is owned by the heap.
unsafe impl Sync for FixedHeap {}
// SAFETY: the heap owns its region; moving it between threads moves no
// thread-bound state.
unsafe impl Send for FixedHeap {}

impl fmt::Debug for FixedHeap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FixedHeap")
            .field("stats", &self.stats())
            .finish()
    }
}

impl FixedHeap {
    /// A heap with no region yet; every allocation returns null until
    /// [`Self::init_static`] or [`Self::init`] hands it one.
    pub const fn new(config: Config) -> Self {
        Self {
            lock: lock::Lock::new(),
            inner: UnsafeCell::new(Inner::new(config)),
            lazy: None,
        }
    }

    /// A heap that takes its region from `region` at its first use (the
    /// first allocation or query), so it can be a `static` global allocator
    /// on any target: the region is found when the program first allocates,
    /// before `main` on native targets. `region` is called at most once,
    /// under the heap's lock; if it returns `None`, or a region the floors
    /// do not fit, every allocation returns null. A heap handed a region
    /// first ([`Self::init_static`], [`Self::init`]) never calls it.
    ///
    /// Typical sources are a mapping made by the platform's own call (an
    /// anonymous `mmap`, sized from the environment read without
    /// allocating), a `static` byte array, or, on wasm32, the module's
    /// memory (`FixedHeap::wasm`).
    ///
    /// ```
    /// use core::alloc::{GlobalAlloc, Layout};
    /// use core::cell::UnsafeCell;
    /// use rtao_arena::{Config, FixedHeap};
    ///
    /// #[repr(align(64))]
    /// struct Region(UnsafeCell<[u8; 4 << 20]>);
    /// // SAFETY: only the heap below touches the bytes, under its lock.
    /// unsafe impl Sync for Region {}
    /// static REGION: Region = Region(UnsafeCell::new([0; 4 << 20]));
    ///
    /// fn region() -> Option<(*mut u8, usize)> {
    ///     Some((REGION.0.get().cast(), 4 << 20))
    /// }
    ///
    /// // SAFETY: `REGION` is used by this heap only, `region` does not
    /// // allocate, and the heap is a static.
    /// static HEAP: FixedHeap = unsafe { FixedHeap::lazy(Config::DEFAULT, region) };
    ///
    /// let layout = Layout::from_size_align(1000, 8)?;
    /// // SAFETY: freed below with the same layout.
    /// let p = unsafe { HEAP.alloc(layout) };
    /// assert!(!p.is_null() && HEAP.contains(p));
    /// // SAFETY: `p` came from `HEAP` with `layout`.
    /// unsafe { HEAP.dealloc(p, layout) };
    /// # Ok::<(), core::alloc::LayoutError>(())
    /// ```
    ///
    /// # Safety
    ///
    /// The region `region` returns must be valid for reads and writes, used
    /// by nothing else, and outlive the heap's use. `region` must not
    /// allocate from this heap (it runs under the heap's lock, so that would
    /// deadlock); on a global allocator, that means not allocating at all.
    /// The heap must not be moved or dropped while anything it allocated or
    /// any [`Scope`](crate::Scope) bound to it is live (a `static` satisfies
    /// both).
    pub const unsafe fn lazy(config: Config, region: RegionFn) -> Self {
        Self {
            lock: lock::Lock::new(),
            inner: UnsafeCell::new(Inner::new(config)),
            lazy: Some(region),
        }
    }

    /// A heap over the module's linear memory, `[__heap_base, memory.size)`,
    /// set up at the first allocation. It never calls `memory.grow`; size the
    /// memory at link time (for example with equal initial and maximum
    /// memory).
    ///
    /// If the floors do not fit the memory, every allocation returns null.
    ///
    /// # Safety
    ///
    /// The heap must be the only allocator using the module's memory above
    /// `__heap_base`, and must not be moved or dropped while anything it
    /// allocated or any [`Scope`](crate::Scope) bound to it is live (a `static` satisfies
    /// both).
    #[cfg(target_arch = "wasm32")]
    pub const unsafe fn wasm(config: Config) -> Self {
        // SAFETY: the caller vouches that the heap owns the memory above
        // `__heap_base`; `wasm_region` does not allocate.
        unsafe { Self::lazy(config, wasm_region) }
    }

    /// Hands the heap a region it owns for the rest of the program.
    ///
    /// ```
    /// use rtao_arena::{Config, FixedHeap};
    ///
    /// static HEAP: FixedHeap = FixedHeap::new(Config::new(1 << 20, 1 << 20));
    /// HEAP.init_static(Box::leak(vec![0u8; 8 << 20].into_boxed_slice()))?;
    /// assert!(HEAP.stats().region > 7 << 20);
    /// # Ok::<(), rtao_arena::InitError>(())
    /// ```
    pub fn init_static(&'static self, region: &'static mut [u8]) -> Result<(), InitError> {
        let len = region.len();
        // SAFETY: the region is exclusively borrowed for 'static and the heap
        // itself lives for 'static, so it is never moved or dropped.
        unsafe { self.init(region.as_mut_ptr(), len) }
    }

    /// Hands the heap the region `[base, base + len)`.
    ///
    /// # Safety
    ///
    /// The region must be valid for reads and writes, used by nothing else,
    /// and outlive the heap's use. The heap must not be moved or dropped while
    /// anything it allocated or any [`Scope`](crate::Scope) bound to it is live.
    pub unsafe fn init(&self, base: *mut u8, len: usize) -> Result<(), InitError> {
        let _guard = self.lock.lock();
        // SAFETY: the lock is held; the caller vouches for the region.
        unsafe { (*self.inner.get()).init(base, len) }
    }

    /// Replaces the floors. Nothing moves: the floors only bound future
    /// growth, so a side already beyond its new limit keeps what it has.
    /// On an initialized heap this walks the region once (every block) to
    /// file its free memory under the new floors.
    pub fn configure(&self, config: Config) -> Result<(), InitError> {
        self.with(|inner| {
            if inner.ready {
                let usable = inner.usable();
                let requested = config.small_floor.saturating_add(config.large_floor);
                if requested > usable {
                    return Err(InitError::Floors { usable, requested });
                }
                let lines = inner.lines(config);
                inner.tlsf.reconfigure(inner.mem, lines);
            }
            inner.config = config;
            Ok(())
        })
    }

    /// The current floors.
    pub fn config(&self) -> Config {
        self.with(|inner| inner.config)
    }

    /// Whether the heap manages a region. A lazy heap ([`Self::lazy`])
    /// asks for its region first if it has not yet.
    pub fn is_ready(&self) -> bool {
        self.with(|inner| inner.ready)
    }

    /// Runs `f` on the state under the lock, setting up a lazy heap.
    #[inline]
    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let _guard = self.lock.lock();
        // SAFETY: the lock is held for the whole closure and nothing inside
        // the heap re-enters it, so this is the only reference to the state.
        let inner = unsafe { &mut *self.inner.get() };
        if !inner.ready
            && !inner.asked
            && let Some(region) = self.lazy
        {
            inner.asked = true;
            if let Some((base, len)) = region() {
                // SAFETY: `lazy` made the caller vouch that the region is the
                // heap's alone and outlives its use.
                let _ = unsafe { inner.init(base, len) };
            }
        }
        f(inner)
    }

    /// Whether `ptr` lies inside the heap's region.
    pub fn contains(&self, ptr: *const u8) -> bool {
        let addr = ptr.addr();
        self.with(|inner| inner.ready && addr >= inner.lo && addr < inner.usable_end())
    }

    /// Allocates `layout` with an explicit lifetime hint (ignored by the slab
    /// side). Inside an active scope the scope serves it and the hint does not
    /// apply; [`Self::alloc_unscoped`] ignores the scope. Returns null on
    /// exhaustion or at the scope's cap; a zero-size layout gets a one-byte
    /// allocation.
    pub fn alloc_with(&self, layout: Layout, hint: Lifetime) -> *mut u8 {
        self.alloc_traced(layout, hint, true).0
    }

    /// Allocates `layout` from the general heap even while a scope is active:
    /// small layouts from the slab side, larger ones from the large side
    /// placed by `hint`. This is what an explicit `&FixedHeap`
    /// [`Source`](crate::Source) uses, so a [`Block`](crate::Block) or
    /// [`FixedVec`](crate::FixedVec) taken from the heap is admitted against
    /// the large side ([`Self::can_serve`]) and never counts against, or
    /// parks, a scope. Returns null on exhaustion.
    pub fn alloc_unscoped(&self, layout: Layout, hint: Lifetime) -> *mut u8 {
        self.alloc_traced(layout, hint, false).0
    }

    /// Allocates and reports whether a null came from a scope's cap.
    pub(crate) fn alloc_traced(
        &self,
        layout: Layout,
        hint: Lifetime,
        scoped: bool,
    ) -> (*mut u8, bool) {
        let scope = if scoped {
            ctx::active_scope()
        } else {
            ptr::null()
        };
        let me = ptr::from_ref(self);
        self.with(|inner| {
            inner.cap_refused = false;
            if !inner.ready {
                return (ptr::null_mut(), false);
            }
            // SAFETY: an active scope is borrowed by its guard on this thread
            // for as long as it is active (the context is per thread, or there
            // is only one thread), so the pointer is valid here.
            let addr = if let Some(scope) = unsafe { scope.as_ref() }
                && scope.bind(me)
            {
                inner.scope_alloc(scope, layout)
            } else {
                inner.alloc_general(layout, hint)
            };
            (
                addr.map_or(ptr::null_mut(), |a| inner.mem.ptr(a)),
                inner.cap_refused,
            )
        })
    }

    /// [`GlobalAlloc::realloc`] that also reports whether a null came from a
    /// scope's cap.
    ///
    /// # Safety
    /// As for [`GlobalAlloc::realloc`].
    pub(crate) unsafe fn realloc_traced(
        &self,
        ptr: *mut u8,
        layout: Layout,
        new_size: usize,
    ) -> (*mut u8, bool) {
        let addr = ptr.addr();
        self.with(|inner| {
            inner.cap_refused = false;
            // SAFETY: the caller passes a live allocation of this heap.
            let to = unsafe { inner.realloc(addr, layout, new_size) };
            (
                to.map_or(ptr::null_mut(), |a| inner.mem.ptr(a)),
                inner.cap_refused,
            )
        })
    }

    /// Payload bytes the large side could hand out now: the free blocks above
    /// the small floor, pages the small side gave back included. Several
    /// allocations each pay a 16-byte header and cannot span free blocks.
    pub fn free_large(&self) -> usize {
        self.with(|inner| if inner.ready { inner.free_large() } else { 0 })
    }

    /// The largest single large allocation (alignment up to 16) that would
    /// succeed now. This is exact.
    pub fn largest_free_large(&self) -> usize {
        self.with(|inner| {
            if inner.ready {
                inner.largest_free_large()
            } else {
                0
            }
        })
    }

    /// Whether the large side can serve `total_bytes` now with no single
    /// buffer above `largest_single`. Exact for one buffer; for several it
    /// checks the total and the largest buffer, not how the rest pack.
    pub fn can_serve(&self, total_bytes: usize, largest_single: usize) -> bool {
        self.with(|inner| {
            inner.ready
                && largest_single <= inner.largest_free_large()
                && total_bytes <= inner.free_large()
        })
    }

    /// A snapshot of the counters.
    pub fn stats(&self) -> Stats {
        self.with(|inner| inner.stats())
    }

    /// Highest bytes both sides ever held together (never above the
    /// region).
    pub fn high_water(&self) -> usize {
        self.with(|inner| inner.high_water)
    }

    /// Resets the live peaks and the high-water mark to the current values.
    pub fn reset_peaks(&self) {
        self.with(|inner| {
            inner.small_peak = inner.slab.live_bytes;
            inner.large_peak = inner.tlsf.used_bytes;
            inner.high_water = if inner.ready { inner.claimed() } else { 0 };
        });
    }

    /// Records the current live bytes as the baseline for [`Self::leaked_bytes`],
    /// typically right after start-up or a rebuild.
    pub fn reinit_baseline(&self) {
        self.with(|inner| inner.baseline = inner.live());
    }

    /// Live bytes above the baseline set by [`Self::reinit_baseline`].
    pub fn leaked_bytes(&self) -> usize {
        self.with(|inner| inner.live().saturating_sub(inner.baseline))
    }

    /// The summary recorded for scopes named `name` when they ended.
    pub fn scope_summary(&self, name: &str) -> Option<ScopeSummary> {
        self.with(|inner| inner.summaries.get(name))
    }

    /// Calls `f` with every recorded scope summary (at most 16 names).
    pub fn scope_summaries(&self, f: impl FnMut(&ScopeSummary)) {
        let table = self.with(|inner| inner.summaries.clone());
        table.iter().for_each(f);
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_region() -> Option<(*mut u8, usize)> {
    unsafe extern "C" {
        static __heap_base: u8;
    }
    // SAFETY: only the address of the linker-provided symbol is taken.
    #[allow(unused_unsafe)]
    let base = unsafe { &raw const __heap_base }.addr();
    let end = core::arch::wasm32::memory_size(0)
        .checked_mul(65536)
        .unwrap_or(usize::MAX & !0xffff);
    Some((
        ptr::with_exposed_provenance_mut(base),
        end.saturating_sub(base),
    ))
}

// SAFETY: `alloc` returns null or a block of at least `layout.size()` bytes at
// `layout.align()` that no other live allocation overlaps; `dealloc` and
// `realloc` route by address to the owner; nothing here unwinds.
unsafe impl GlobalAlloc for FixedHeap {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc_with(layout, ctx::lifetime())
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let addr = ptr.addr();
        // SAFETY: the caller passes a live allocation of this heap.
        self.with(|inner| unsafe { inner.free(addr, layout) });
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded from the caller.
        unsafe { self.realloc_traced(ptr, layout, new_size) }.0
    }
}
