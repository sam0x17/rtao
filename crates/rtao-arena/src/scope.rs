//! Scopes: chunked, capped sub-arenas carved from the large side.
//!
//! A scope bump-allocates inside chunks it takes from the large side as it
//! needs them. Each object carries one tag word just below it that names its
//! chunk, so deallocation finds the chunk by address from anywhere. A chunk
//! goes back to the large side as soon as its last object is freed (the
//! current chunk is rewound instead while the scope lives). When the scope
//! ends, every empty chunk returns at once; chunks that still hold objects stay
//! parked until their last object is freed. A scope is never freed wholesale
//! while anything inside it is live.
//!
//! The scope's counters live in a small control block inside the region, so a
//! [`Scope`] value can move freely and its parked chunks never point at it.

#![allow(unsafe_code)]

use core::alloc::Layout;
use core::fmt;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use crate::ctx;
use crate::heap::{FixedHeap, Inner};
use crate::raw::align_up;
use crate::tlsf::{GRANULE, TAG};

const WORD: usize = size_of::<usize>();
/// The smallest chunk a scope takes.
pub const MIN_CHUNK: usize = 4096;

/// In-region control block of a bound scope.
#[repr(C)]
struct Core {
    name: &'static str,
    cap: usize,
    chunk_bytes: usize,
    held: usize,
    chunks: usize,
    live: usize,
    peak: usize,
    objects: usize,
    cap_refusals: usize,
    current: usize,
    head: usize,
    ended: bool,
}

/// In-region header at the start of every chunk.
#[repr(C)]
struct Chunk {
    core: usize,
    next: usize,
    prev: usize,
    start: usize,
    bump: usize,
    end: usize,
    objects: usize,
    held: usize,
}

const CHUNK_HEADER: usize = match align_up(size_of::<Chunk>(), GRANULE) {
    Some(v) => v,
    None => 0,
};
const CORE_BYTES: usize = match align_up(size_of::<Core>(), GRANULE) {
    Some(v) => v,
    None => 0,
};

/// Counters of one scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopeStats {
    /// The scope's name.
    pub name: &'static str,
    /// Most chunk bytes the scope may hold.
    pub cap: usize,
    /// Bytes of one regular chunk.
    pub chunk_bytes: usize,
    /// Live bytes of objects in the scope (requested sizes).
    pub live: usize,
    /// Highest `live`.
    pub peak: usize,
    /// Live objects.
    pub objects: usize,
    /// Chunks held.
    pub chunks: usize,
    /// Chunk bytes held (what the cap limits).
    pub held: usize,
    /// Chunks left parked when the scope ended (0 while it runs).
    pub parked: usize,
    /// Allocations refused at the cap.
    pub cap_refusals: usize,
}

/// What a heap remembers about the scopes of one name that have ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopeSummary {
    /// The scopes' name.
    pub name: &'static str,
    /// Scopes of this name that ended.
    pub ended: usize,
    /// Highest live bytes any of them reached.
    pub peak: usize,
    /// Allocations refused at the cap, summed.
    pub cap_refusals: usize,
    /// Scopes that ended with objects still live (and parked chunks).
    pub parked_ends: usize,
}

const SUMMARIES: usize = 16;

/// Fixed table of per-name scope summaries.
#[derive(Clone)]
pub(crate) struct SummaryTable {
    entries: [ScopeSummary; SUMMARIES],
    len: usize,
}

impl SummaryTable {
    pub(crate) const fn new() -> Self {
        const EMPTY: ScopeSummary = ScopeSummary {
            name: "",
            ended: 0,
            peak: 0,
            cap_refusals: 0,
            parked_ends: 0,
        };
        Self {
            entries: [EMPTY; SUMMARIES],
            len: 0,
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<ScopeSummary> {
        self.iter().find(|s| s.name == name).copied()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ScopeSummary> {
        self.entries[..self.len].iter()
    }

    fn record(&mut self, stats: &ScopeStats) {
        let index = match self.entries[..self.len]
            .iter()
            .position(|s| s.name == stats.name)
        {
            Some(i) => i,
            None if self.len < SUMMARIES => {
                self.entries[self.len].name = stats.name;
                self.len += 1;
                self.len - 1
            }
            None => return,
        };
        let entry = &mut self.entries[index];
        entry.ended += 1;
        entry.peak = entry.peak.max(stats.peak);
        entry.cap_refusals += stats.cap_refusals;
        entry.parked_ends += usize::from(stats.parked > 0);
    }
}

/// A capped sub-arena carved from a [`FixedHeap`]'s large side in chunks.
///
/// While the scope is active (during [`Scope::run`] or a [`Scoped`] poll),
/// global allocations on the thread bump-allocate from its chunks; an object
/// larger than a chunk gets a block of its own that counts against the
/// scope. Reaching the cap returns null. Objects may be freed anywhere, at any
/// time, also after the scope has ended.
///
/// A scope binds to the first heap that serves it. Allocations through other
/// heaps while it is active are served by those heaps normally.
///
/// ```
/// use core::alloc::{GlobalAlloc, Layout};
/// use rtao_arena::{Config, FixedHeap, Lifetime, Scope};
///
/// let heap: &'static FixedHeap = Box::leak(Box::new(FixedHeap::new(Config::DEFAULT)));
/// heap.init_static(Box::leak(vec![0u8; 8 << 20].into_boxed_slice()))?;
///
/// let scope = Scope::new("decode", 1 << 20, 64 << 10);
/// let layout = Layout::new::<[u64; 32]>();
/// let p = scope.run(|| heap.alloc_with(layout, Lifetime::Short));
/// assert_eq!(scope.stats().objects, 1);
/// // SAFETY: `p` was allocated by `heap` with `layout`.
/// unsafe { heap.dealloc(p, layout) };
/// let done = scope.finish();
/// assert_eq!((done.live, done.parked), (0, 0));
/// assert_eq!(heap.stats().large.live, 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Scope {
    name: &'static str,
    cap: usize,
    chunk_bytes: usize,
    heap: AtomicPtr<FixedHeap>,
    core: AtomicUsize,
}

impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope")
            .field("stats", &self.stats())
            .finish()
    }
}

impl Scope {
    /// A scope that may hold up to `cap` chunk bytes in chunks of
    /// `chunk_bytes` (at least [`MIN_CHUNK`], rounded up to 16).
    pub const fn new(name: &'static str, cap: usize, chunk_bytes: usize) -> Self {
        let chunk = if chunk_bytes < MIN_CHUNK {
            MIN_CHUNK
        } else {
            match align_up(chunk_bytes, GRANULE) {
                Some(v) => v,
                None => usize::MAX & !(GRANULE - 1),
            }
        };
        Self {
            name,
            cap,
            chunk_bytes: chunk,
            heap: AtomicPtr::new(ptr::null_mut()),
            core: AtomicUsize::new(0),
        }
    }

    /// The scope's name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Runs `f` with this scope active on the calling thread.
    ///
    /// The active scope is per thread, so this exists only where the
    /// allocation context is: with the `std` feature, or on wasm32 without
    /// the `atomics` target feature (one thread). Other `no_std` targets have
    /// no per-thread storage, and a process-wide active scope would let one
    /// thread allocate into another thread's scope after it ended.
    #[cfg(any(
        feature = "std",
        test,
        all(target_arch = "wasm32", not(target_feature = "atomics"))
    ))]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            feature = "std",
            all(target_arch = "wasm32", not(target_feature = "atomics"))
        )))
    )]
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = ctx::enter(Some(self));
        f()
    }

    /// Wraps `future` so this scope is active only while it is polled.
    /// Available where [`Scope::run`] is.
    #[cfg(any(
        feature = "std",
        test,
        all(target_arch = "wasm32", not(target_feature = "atomics"))
    ))]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            feature = "std",
            all(target_arch = "wasm32", not(target_feature = "atomics"))
        )))
    )]
    pub fn scoped<F: core::future::Future>(&self, future: F) -> Scoped<'_, F> {
        Scoped {
            scope: self,
            future,
        }
    }

    /// Current counters.
    pub fn stats(&self) -> ScopeStats {
        let heap = self.heap.load(Ordering::Acquire);
        // SAFETY: a heap a scope is bound to outlives the scope (the heap's
        // construction contract).
        match unsafe { heap.as_ref() } {
            Some(heap) => heap.with(|inner| inner.scope_stats(self)),
            None => self.unbound_stats(),
        }
    }

    /// Ends the scope and returns its final counters. Empty chunks return to
    /// the heap; chunks with live objects stay parked until those are freed.
    /// Dropping a scope does the same.
    pub fn finish(mut self) -> ScopeStats {
        self.end()
    }

    fn unbound_stats(&self) -> ScopeStats {
        ScopeStats {
            name: self.name,
            cap: self.cap,
            chunk_bytes: self.chunk_bytes,
            ..ScopeStats::default()
        }
    }

    /// Binds the scope to `heap` unless it is bound elsewhere; returns whether
    /// `heap` serves it.
    pub(crate) fn bind(&self, heap: *const FixedHeap) -> bool {
        let heap = heap.cast_mut();
        match self
            .heap
            .compare_exchange(ptr::null_mut(), heap, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => true,
            Err(bound) => bound == heap,
        }
    }

    fn end(&mut self) -> ScopeStats {
        let heap = self.heap.swap(ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: a heap a scope is bound to outlives the scope.
        let stats = match unsafe { heap.as_ref() } {
            Some(heap) => heap.with(|inner| inner.end_scope(self)),
            None => return self.unbound_stats(),
        };
        #[cfg(feature = "std")]
        if stats.parked > 0 && ctx::strict_scopes() && !std::thread::panicking() {
            #[allow(clippy::panic)] // The conformance check exists to fail the test here.
            {
                panic!(
                    "scope `{}` ended with {} live bytes in {} objects ({} chunks parked)",
                    stats.name, stats.live, stats.objects, stats.parked
                );
            }
        }
        #[cfg(not(feature = "std"))]
        let _ = ctx::strict_scopes();
        stats
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.end();
    }
}

/// A future whose allocations go to a [`Scope`] while it is polled.
///
/// The scope is active only inside `poll`, so code that runs between polls
/// (the async runtime, other tasks, host callbacks) never allocates into it.
/// Available where [`Scope::run`] is.
#[cfg(any(
    feature = "std",
    test,
    all(target_arch = "wasm32", not(target_feature = "atomics"))
))]
#[cfg_attr(
    docsrs,
    doc(cfg(any(
        feature = "std",
        all(target_arch = "wasm32", not(target_feature = "atomics"))
    )))
)]
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Scoped<'s, F> {
    scope: &'s Scope,
    future: F,
}

#[cfg(any(
    feature = "std",
    test,
    all(target_arch = "wasm32", not(target_feature = "atomics"))
))]
impl<F: core::future::Future> core::future::Future for Scoped<'_, F> {
    type Output = F::Output;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> {
        // SAFETY: `future` is structurally pinned: it is never moved out of
        // `self`, and `Scoped` is `Unpin` only when `F` is.
        let this = unsafe { self.get_unchecked_mut() };
        let _guard = ctx::enter(Some(this.scope));
        // SAFETY: see above; `this.future` stays where it is while pinned.
        unsafe { core::pin::Pin::new_unchecked(&mut this.future) }.poll(cx)
    }
}

// ---- heap side -------------------------------------------------------------
//
// Control blocks and chunk headers are reached through raw pointers only, so
// no reference to region memory outlives a call that may touch it again.

impl Inner {
    #[inline]
    fn core_ptr(&self, addr: usize) -> *mut Core {
        self.mem.ptr(addr).cast::<Core>()
    }

    #[inline]
    fn chunk_ptr(&self, addr: usize) -> *mut Chunk {
        self.mem.ptr(addr).cast::<Chunk>()
    }

    /// # Safety
    /// `core` must point at a live control block.
    unsafe fn snapshot(core: *const Core) -> ScopeStats {
        // SAFETY: forwarded.
        unsafe {
            ScopeStats {
                name: (*core).name,
                cap: (*core).cap,
                chunk_bytes: (*core).chunk_bytes,
                live: (*core).live,
                peak: (*core).peak,
                objects: (*core).objects,
                chunks: (*core).chunks,
                held: (*core).held,
                parked: if (*core).ended { (*core).chunks } else { 0 },
                cap_refusals: (*core).cap_refusals,
            }
        }
    }

    pub(crate) fn scope_stats(&self, scope: &Scope) -> ScopeStats {
        match scope.core.load(Ordering::Relaxed) {
            0 => scope.unbound_stats(),
            // SAFETY: a scope's stored control block is live until it ends.
            core => unsafe { Self::snapshot(self.core_ptr(core)) },
        }
    }

    /// Allocates `layout` inside `scope`, creating its control block first.
    pub(crate) fn scope_alloc(&mut self, scope: &Scope, layout: Layout) -> Option<usize> {
        let mut core = scope.core.load(Ordering::Relaxed);
        if core == 0 {
            core = self.alloc_large(CORE_BYTES, GRANULE, false)?;
            // SAFETY: a fresh large block of `CORE_BYTES` bytes.
            unsafe {
                self.core_ptr(core).write(Core {
                    name: scope.name,
                    cap: scope.cap,
                    chunk_bytes: scope.chunk_bytes,
                    held: 0,
                    chunks: 0,
                    live: 0,
                    peak: 0,
                    objects: 0,
                    cap_refusals: 0,
                    current: 0,
                    head: 0,
                    ended: false,
                });
            }
            scope.core.store(core, Ordering::Relaxed);
        }
        // SAFETY: `core` is the scope's live, not ended control block.
        let addr = unsafe { self.core_alloc(core, layout) };
        self.note();
        addr
    }

    /// Bump-allocates inside a chunk, writing the object's tag word.
    ///
    /// # Safety
    /// `chunk` must be a live chunk of this heap.
    unsafe fn bump(&mut self, chunk: usize, size: usize, align: usize) -> Option<usize> {
        let ch = self.chunk_ptr(chunk);
        // SAFETY: forwarded; `[addr - WORD, addr + size)` lies inside the
        // chunk's unused tail once the bounds check passes.
        unsafe {
            let addr = align_up((*ch).bump.checked_add(WORD)?, align)?;
            if addr.checked_add(size)? > (*ch).end {
                return None;
            }
            self.mem.set_word(addr - WORD, chunk | TAG);
            (*ch).bump = addr + size;
            (*ch).objects += 1;
            Some(addr)
        }
    }

    /// # Safety
    /// `core` must be a live, not ended control block of this heap.
    unsafe fn core_alloc(&mut self, core: usize, layout: Layout) -> Option<usize> {
        let size = layout.size().max(1);
        let align = layout.align().max(WORD);
        let c = self.core_ptr(core);
        // SAFETY: `c` is live; the current chunk, when set, is a live chunk of
        // this scope; a new chunk is a fresh large block that becomes a chunk
        // of this scope, linked at the head of its list.
        unsafe {
            let mut addr = match (*c).current {
                0 => None,
                current => self.bump(current, size, align),
            };
            if addr.is_none() {
                let worst = CHUNK_HEADER
                    .checked_add(WORD)?
                    .checked_add(size)?
                    .checked_add(align)?;
                let regular = worst <= (*c).chunk_bytes;
                let bytes = if regular {
                    (*c).chunk_bytes
                } else {
                    align_up(worst, GRANULE)?
                };
                if (*c).held.saturating_add(bytes) > (*c).cap {
                    (*c).cap_refusals += 1;
                    self.cap_refusals += 1;
                    self.cap_refused = true;
                    return None;
                }
                let chunk = self.alloc_large(bytes, GRANULE, false)?;
                let head = (*c).head;
                self.chunk_ptr(chunk).write(Chunk {
                    core,
                    next: head,
                    prev: 0,
                    start: chunk + CHUNK_HEADER,
                    bump: chunk + CHUNK_HEADER,
                    end: chunk + bytes,
                    objects: 0,
                    held: bytes,
                });
                if head != 0 {
                    (*self.chunk_ptr(head)).prev = chunk;
                }
                (*c).head = chunk;
                (*c).held += bytes;
                (*c).chunks += 1;
                if regular {
                    let old = (*c).current;
                    (*c).current = chunk;
                    if old != 0 && (*self.chunk_ptr(old)).objects == 0 {
                        self.release_chunk(core, old);
                    }
                }
                addr = self.bump(chunk, size, align);
            }
            let addr = addr?;
            (*c).objects += 1;
            (*c).live += layout.size();
            (*c).peak = (*c).peak.max((*c).live);
            Some(addr)
        }
    }

    /// Unlinks a chunk from its scope and gives it back to the large side.
    ///
    /// # Safety
    /// `chunk` must be a live chunk of the live control block `core`.
    unsafe fn release_chunk(&mut self, core: usize, chunk: usize) {
        let c = self.core_ptr(core);
        let ch = self.chunk_ptr(chunk);
        // SAFETY: forwarded; neighbours on the list are live chunks.
        unsafe {
            let (next, prev, held) = ((*ch).next, (*ch).prev, (*ch).held);
            if next != 0 {
                (*self.chunk_ptr(next)).prev = prev;
            }
            if prev != 0 {
                (*self.chunk_ptr(prev)).next = next;
            } else {
                (*c).head = next;
            }
            (*c).held -= held;
            (*c).chunks -= 1;
            if (*c).current == chunk {
                (*c).current = 0;
            }
            self.tlsf.free(self.mem, chunk);
        }
    }

    /// Frees a scope object of `size` bytes in `chunk`.
    ///
    /// # Safety
    /// `chunk` must be the chunk of a live scope object of `size` bytes.
    pub(crate) unsafe fn scope_free(&mut self, chunk: usize, size: usize) {
        let ch = self.chunk_ptr(chunk);
        // SAFETY: forwarded; a chunk's control block lives as long as it does.
        unsafe {
            let core = (*ch).core;
            let c = self.core_ptr(core);
            (*ch).objects -= 1;
            (*c).objects -= 1;
            (*c).live -= size;
            if (*ch).objects != 0 {
                return;
            }
            if (*c).ended {
                let held = (*ch).held;
                self.release_chunk(core, chunk);
                self.parked_chunks -= 1;
                self.parked_bytes -= held;
                if (*c).chunks == 0 {
                    self.tlsf.free(self.mem, core);
                }
            } else if (*c).current != chunk {
                self.release_chunk(core, chunk);
            } else {
                (*ch).bump = (*ch).start;
            }
        }
    }

    /// Resizes a scope object: in place when it shrinks or is the last object
    /// of its chunk, else inside its own scope (never the active one), or in
    /// the general heap once its scope has ended.
    ///
    /// # Safety
    /// `addr` must be a live scope object in `chunk` made with `layout`.
    pub(crate) unsafe fn scope_realloc(
        &mut self,
        chunk: usize,
        addr: usize,
        layout: Layout,
        new_layout: Layout,
    ) -> Option<usize> {
        let (old, new) = (layout.size(), new_layout.size());
        let ch = self.chunk_ptr(chunk);
        // SAFETY: forwarded; a chunk's control block lives as long as it does;
        // the copy is between two live, distinct allocations.
        unsafe {
            let core = (*ch).core;
            let c = self.core_ptr(core);
            let last = addr + old.max(1) == (*ch).bump;
            if new <= old {
                if last {
                    (*ch).bump = addr + new.max(1);
                }
                (*c).live -= old - new;
                return Some(addr);
            }
            if last && !(*c).ended && addr.checked_add(new)? <= (*ch).end {
                (*ch).bump = addr + new;
                (*c).live += new - old;
                (*c).peak = (*c).peak.max((*c).live);
                self.note();
                return Some(addr);
            }
            let to = if (*c).ended {
                self.alloc_general(new_layout, ctx::lifetime())?
            } else {
                let to = self.core_alloc(core, new_layout)?;
                self.note();
                to
            };
            self.mem.copy(addr, to, old);
            self.scope_free(chunk, old);
            Some(to)
        }
    }

    /// Ends `scope`: returns empty chunks, parks the rest.
    pub(crate) fn end_scope(&mut self, scope: &Scope) -> ScopeStats {
        let core = scope.core.swap(0, Ordering::Relaxed);
        if core == 0 {
            return scope.unbound_stats();
        }
        let c = self.core_ptr(core);
        // SAFETY: the scope's control block is live; its list holds live chunks.
        unsafe {
            (*c).ended = true;
            (*c).current = 0;
            let mut chunk = (*c).head;
            while chunk != 0 {
                let ch = self.chunk_ptr(chunk);
                let next = (*ch).next;
                if (*ch).objects == 0 {
                    self.release_chunk(core, chunk);
                } else {
                    self.parked_chunks += 1;
                    self.parked_bytes += (*ch).held;
                }
                chunk = next;
            }
            let stats = Self::snapshot(c);
            self.summaries.record(&stats);
            if (*c).chunks == 0 {
                self.tlsf.free(self.mem, core);
            }
            stats
        }
    }
}
