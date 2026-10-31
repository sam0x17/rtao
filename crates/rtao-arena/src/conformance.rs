//! A conformance allocator for native test binaries.
//!
//! [`Strict`] runs a [`FixedHeap`] of a configurable shape on one buffer taken
//! from the system allocator, and serves the whole test binary from it. Code
//! that passes under `Strict` fits the same shape on a host whose memory never
//! grows.
//!
//! On the calling thread, [`Strict::live_path`] turns on the checks:
//!
//! - an allocation the arena cannot serve is a violation (with the check off
//!   it is served by the system allocator and counted in
//!   [`Strict::growth_requests`], which a conforming run keeps at zero);
//! - a [`crate::Scope`] that ends with objects still live panics;
//! - with [`Shape::unscoped_threshold`], an allocation, or a reallocation
//!   that grows a block, above the threshold that is neither inside a scope
//!   nor a declared block ([`crate::declared`], [`crate::Block`],
//!   [`crate::FixedVec`]) is a violation.
//!
//! A scope that refuses an allocation at its cap is not exhaustion: as on a
//! [`FixedHeap`], the allocation returns null (the caller's fallible limit),
//! on or off the live path, with no violation and no system fallback. The
//! heap counts it in [`Stats::scope_cap_refusals`].
//!
//! A global allocator must not unwind, so a violation inside an allocation
//! cannot panic. With [`OnViolation::Abort`] (the default) it prints the size
//! and a backtrace to standard error and aborts the process; with
//! [`OnViolation::Record`] it is counted (see [`Strict::violations`]), an
//! exhausted allocation returns null, and [`Strict::assert_clean`] panics
//! later from ordinary code.
//!
//! ```no_run
//! use rtao_arena::conformance::{Shape, Strict};
//!
//! #[global_allocator]
//! static ALLOC: Strict = Strict::new(Shape::new(64 << 20, 8 << 20, 32 << 20));
//!
//! fn main() {
//!     ALLOC.live_path(true);
//!     let buffer = vec![0u8; 1 << 20];
//!     ALLOC.live_path(false);
//!     drop(buffer);
//!     ALLOC.assert_clean();
//!     assert!(ALLOC.stats().large.peak <= 64 << 20);
//! }
//! ```

#![allow(unsafe_code)]

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use core::fmt;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::alloc::System;
use std::backtrace::Backtrace;
use std::io::Write as _;
use std::sync::Once;

use crate::ctx;
use crate::heap::{Config, FixedHeap, Stats};
use crate::scope::ScopeSummary;
use crate::slab::PAGE;

/// What a violation inside an allocation does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnViolation {
    /// Print the size and a backtrace to standard error, then abort.
    #[default]
    Abort,
    /// Count it; an exhausted allocation returns null.
    Record,
}

/// The kind of a recorded violation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViolationKind {
    /// The arena could not serve the allocation.
    Exhausted,
    /// An allocation above the unscoped threshold outside any scope or
    /// declared block.
    UnscopedLarge,
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Exhausted => "arena exhausted",
            Self::UnscopedLarge => "unscoped large allocation",
        })
    }
}

/// The last violation [`Strict`] saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Violation {
    /// What went wrong.
    pub kind: ViolationKind,
    /// Requested bytes.
    pub size: usize,
    /// Requested alignment.
    pub align: usize,
}

/// The shape of the arena [`Strict`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    /// Bytes of the buffer the heap manages.
    pub bytes: usize,
    /// The heap's floors.
    pub config: Config,
    /// Allocations above this many bytes on the live path must be inside a
    /// scope or a declared block.
    pub unscoped_threshold: Option<usize>,
    /// What a violation does.
    pub on_violation: OnViolation,
}

impl Shape {
    /// A shape of `bytes` with the given floors, no unscoped threshold and
    /// aborting violations.
    pub const fn new(bytes: usize, small_floor: usize, large_floor: usize) -> Self {
        Self {
            bytes,
            config: Config::new(small_floor, large_floor),
            unscoped_threshold: None,
            on_violation: OnViolation::Abort,
        }
    }

    /// Sets the unscoped threshold.
    pub const fn unscoped_threshold(mut self, bytes: usize) -> Self {
        self.unscoped_threshold = Some(bytes);
        self
    }

    /// Sets what a violation does.
    pub const fn on_violation(mut self, on_violation: OnViolation) -> Self {
        self.on_violation = on_violation;
        self
    }
}

std::thread_local! {
    static LIVE: Cell<bool> = const { Cell::new(false) };
    static BYPASS: Cell<bool> = const { Cell::new(false) };
}

fn live() -> bool {
    LIVE.try_with(Cell::get).unwrap_or(false)
}

fn bypass() -> bool {
    BYPASS.try_with(Cell::get).unwrap_or(false)
}

/// The conformance allocator; see the [module docs](self).
pub struct Strict {
    shape: Shape,
    once: Once,
    heap: AtomicPtr<FixedHeap>,
    lo: AtomicUsize,
    hi: AtomicUsize,
    record: AtomicBool,
    growth: AtomicUsize,
    violations: AtomicUsize,
    last_kind: AtomicU8,
    last_size: AtomicUsize,
    last_align: AtomicUsize,
}

impl fmt::Debug for Strict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Strict")
            .field("shape", &self.shape)
            .field("growth_requests", &self.growth_requests())
            .field("violations", &self.violations())
            .finish()
    }
}

impl Strict {
    /// A conformance allocator of `shape`; its buffer is taken at the first
    /// allocation.
    pub const fn new(shape: Shape) -> Self {
        Self {
            shape,
            once: Once::new(),
            heap: AtomicPtr::new(ptr::null_mut()),
            lo: AtomicUsize::new(0),
            hi: AtomicUsize::new(0),
            record: AtomicBool::new(matches!(shape.on_violation, OnViolation::Record)),
            growth: AtomicUsize::new(0),
            violations: AtomicUsize::new(0),
            last_kind: AtomicU8::new(0),
            last_size: AtomicUsize::new(0),
            last_align: AtomicUsize::new(0),
        }
    }

    /// The shape.
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// The heap, set up on first use; `None` if the buffer could not be had.
    pub fn heap(&self) -> Option<&'static FixedHeap> {
        self.once.call_once(|| self.init());
        // SAFETY: the pointer is null or a heap leaked for 'static in `init`.
        unsafe { self.heap.load(Relaxed).as_ref() }
    }

    fn init(&self) {
        let Ok(region_layout) = Layout::from_size_align(self.shape.bytes, PAGE) else {
            return;
        };
        if region_layout.size() == 0 {
            return;
        }
        // SAFETY: non-zero sizes; the system allocator does not re-enter us.
        let (region, slot) = unsafe {
            (
                System.alloc(region_layout),
                System.alloc(Layout::new::<FixedHeap>()).cast::<FixedHeap>(),
            )
        };
        if region.is_null() || slot.is_null() {
            return;
        }
        // SAFETY: `slot` is fresh memory for one heap, never freed, so the heap
        // lives (and stays put) for the rest of the process; `region` is a
        // fresh buffer of `bytes` bytes that only the heap uses.
        let heap = unsafe {
            slot.write(FixedHeap::new(self.shape.config));
            let heap = &*slot;
            if heap.init(region, self.shape.bytes).is_err() {
                return;
            }
            heap
        };
        self.lo.store(region.addr(), Relaxed);
        self.hi.store(region.addr() + self.shape.bytes, Relaxed);
        self.heap.store(ptr::from_ref(heap).cast_mut(), Relaxed);
    }

    /// Turns the live-path checks on or off for the calling thread.
    pub fn live_path(&self, on: bool) {
        let _ = LIVE.try_with(|c| c.set(on));
        ctx::set_strict_scopes(on);
    }

    /// Whether the live-path checks are on for the calling thread.
    pub fn is_live_path(&self) -> bool {
        live()
    }

    /// Changes what a violation does.
    pub fn set_on_violation(&self, on_violation: OnViolation) {
        self.record
            .store(matches!(on_violation, OnViolation::Record), Relaxed);
    }

    /// Allocations the arena could not serve off the live path; each was served
    /// by the system allocator instead. A conforming run keeps this at zero.
    pub fn growth_requests(&self) -> usize {
        self.growth.load(Relaxed)
    }

    /// Violations recorded so far.
    pub fn violations(&self) -> usize {
        self.violations.load(Relaxed)
    }

    /// The last recorded violation.
    pub fn last_violation(&self) -> Option<Violation> {
        let kind = match self.last_kind.load(Relaxed) {
            1 => ViolationKind::Exhausted,
            2 => ViolationKind::UnscopedLarge,
            _ => return None,
        };
        Some(Violation {
            kind,
            size: self.last_size.load(Relaxed),
            align: self.last_align.load(Relaxed),
        })
    }

    /// Panics if any violation was recorded or the arena ever had to grow.
    pub fn assert_clean(&self) {
        let (violations, growth) = (self.violations(), self.growth_requests());
        if violations != 0 || growth != 0 {
            #[allow(clippy::panic)] // Conformance failures fail the test.
            {
                panic!(
                    "arena conformance: {violations} violations (last {:?}), {growth} growth requests",
                    self.last_violation()
                );
            }
        }
    }

    /// The heap's counters (per-side live and peak, class histogram, ...).
    pub fn stats(&self) -> Stats {
        self.heap().map(FixedHeap::stats).unwrap_or_default()
    }

    /// Highest bytes the heap ever claimed; never above the arena.
    pub fn high_water(&self) -> usize {
        self.heap().map_or(0, FixedHeap::high_water)
    }

    /// Resets the heap's peaks.
    pub fn reset_peaks(&self) {
        if let Some(heap) = self.heap() {
            heap.reset_peaks();
        }
    }

    /// The recorded summary (peak, cap refusals, parked ends) of the scopes
    /// named `name`.
    pub fn scope_summary(&self, name: &str) -> Option<ScopeSummary> {
        self.heap().and_then(|heap| heap.scope_summary(name))
    }

    /// Whether `ptr` lies in the arena.
    pub fn contains(&self, ptr: *const u8) -> bool {
        let addr = ptr.addr();
        addr >= self.lo.load(Relaxed) && addr < self.hi.load(Relaxed)
    }

    /// Records an unscoped large allocation (or growth to `layout`) on the
    /// live path: above the threshold, outside any scope and declared block.
    fn check_unscoped(&self, layout: Layout) {
        if let Some(threshold) = self.shape.unscoped_threshold
            && layout.size() > threshold
            && !ctx::in_scope()
            && !ctx::is_declared()
        {
            self.violation(ViolationKind::UnscopedLarge, layout);
        }
    }

    #[cold]
    fn violation(&self, kind: ViolationKind, layout: Layout) {
        self.violations.fetch_add(1, Relaxed);
        self.last_kind.store(
            match kind {
                ViolationKind::Exhausted => 1,
                ViolationKind::UnscopedLarge => 2,
            },
            Relaxed,
        );
        self.last_size.store(layout.size(), Relaxed);
        self.last_align.store(layout.align(), Relaxed);
        if self.record.load(Relaxed) {
            return;
        }
        // Everything allocated while reporting goes to the system allocator.
        let _ = BYPASS.try_with(|c| c.set(true));
        let backtrace = Backtrace::force_capture();
        let _ = writeln!(
            std::io::stderr(),
            "rtao-arena conformance: {kind} of {} bytes (align {}) on the live path\n{backtrace}",
            layout.size(),
            layout.align()
        );
        std::process::abort();
    }
}

// SAFETY: every pointer comes from the arena or from the system allocator and
// is released by its owner (routed by address); violations abort or record,
// never unwind.
unsafe impl GlobalAlloc for Strict {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if bypass() {
            // SAFETY: forwarded from the caller.
            return unsafe { System.alloc(layout) };
        }
        let Some(heap) = self.heap() else {
            self.growth.fetch_add(1, Relaxed);
            // SAFETY: forwarded from the caller.
            return unsafe { System.alloc(layout) };
        };
        let live = live();
        if live {
            self.check_unscoped(layout);
        }
        let (ptr, cap_refused) = heap.alloc_traced(layout, ctx::lifetime(), true);
        if !ptr.is_null() || cap_refused {
            // A scope's cap refusal is the caller's fallible limit, exactly as
            // on a `FixedHeap`: null, no violation and no system fallback.
            return ptr;
        }
        if live {
            self.violation(ViolationKind::Exhausted, layout);
            return ptr::null_mut();
        }
        self.growth.fetch_add(1, Relaxed);
        // SAFETY: forwarded from the caller.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded; the address tells which allocator owns it.
        unsafe {
            match self.heap() {
                Some(heap) if self.contains(ptr) => heap.dealloc(ptr, layout),
                _ => System.dealloc(ptr, layout),
            }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = Layout::from_size_align(new_size, layout.align()).unwrap_or(layout);
        let live = live() && !bypass();
        // Growth is how `Vec`, `String` and the like reach a large size, so the
        // unscoped check applies to it as it does to a fresh allocation.
        if live && new_size > layout.size() {
            self.check_unscoped(new_layout);
        }
        let heap = match self.heap() {
            Some(heap) if self.contains(ptr) => heap,
            // SAFETY: forwarded; the system allocator owns `ptr`.
            _ => return unsafe { System.realloc(ptr, layout, new_size) },
        };
        // SAFETY: forwarded; the arena owns `ptr`.
        let (moved, cap_refused) = unsafe { heap.realloc_traced(ptr, layout, new_size) };
        if !moved.is_null() || cap_refused {
            // See `alloc`: a cap refusal leaves the block where it is.
            return moved;
        }
        if live {
            self.violation(ViolationKind::Exhausted, new_layout);
            return ptr::null_mut();
        }
        self.growth.fetch_add(1, Relaxed);
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return ptr::null_mut();
        };
        // SAFETY: a fresh system block of `new_size` bytes receives the
        // smaller of the two sizes; the old block goes back to the arena.
        unsafe {
            let to = System.alloc(new_layout);
            if !to.is_null() {
                ptr::copy_nonoverlapping(ptr, to, layout.size().min(new_size));
                heap.dealloc(ptr, layout);
            }
            to
        }
    }
}
