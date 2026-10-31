//! Fixed-capacity memory for wasm32 and other constrained hosts whose linear
//! memory must never grow.
//!
//! [`FixedHeap`] is a [`GlobalAlloc`](core::alloc::GlobalAlloc) over one
//! region chosen up front. It never asks the host for more memory: when the
//! region is full, allocation returns null. The region is two-ended:
//!
//! ```text
//! region start                                                       region end
//! | page table | slab pages (≤ 2 KiB, grow up) -->  gap  <-- TLSF pool (> 2 KiB, grow down) |
//! |<------- small_floor: never claimed by the pool ------>|
//!                           |<------ large_floor: never claimed by slab pages ------>|
//! ```
//!
//! - **Small side.** Objects up to 2 KiB live in 64 KiB slab pages of 14
//!   size classes (16 … 2048 bytes), each at its natural alignment. A fully
//!   free page is reused by any class and returns to the gap when it is the
//!   topmost page. Small objects never interleave with large blocks.
//! - **Large side.** Larger (or more strictly aligned) blocks live in one
//!   two-level segregated fit pool with O(1) good-fit search and immediate
//!   coalescing. The pool grows down into the gap by exactly what an
//!   allocation needs, and a free block at its bottom returns to the gap.
//!   [`Lifetime::Long`] blocks take the lowest fitting address, packing against
//!   the boundary; [`Lifetime::Short`] blocks (the default) take the highest,
//!   so transients churn at the top. Alignment up to 64 KiB and beyond works.
//! - **Floors.** [`Config::small_floor`] and [`Config::large_floor`] are the
//!   only split configuration; the boundary floats between them.
//! - **Scopes.** A [`Scope`] is a capped sub-arena carved from the large side
//!   in chunks. While it is active ([`Scope::run`], [`Scoped`]) global
//!   allocations bump-allocate in it; dealloc finds the chunk by address, and
//!   a scope's memory returns when its last object is freed.
//! - **Queries.** [`FixedHeap::free_large`], [`FixedHeap::largest_free_large`]
//!   and [`FixedHeap::can_serve`] tell whether a large request would succeed
//!   before making it; [`FixedHeap::stats`] reports per-side live, peak and
//!   capacity, the class histogram and failure counts.
//! - **Building blocks.** [`Block`] (an exact-size byte block),
//!   [`FixedVec`] (a vector that never grows) and [`KeyBuf`] (a stack buffer
//!   for keys) let callers allocate fallibly and exactly.
//!
//! All heap metadata lives inside the region except the fixed control
//! structure of the [`FixedHeap`] value itself; nothing is allocated per
//! object outside the region.
//!
//! # Features
//!
//! - `std` (default): thread-local allocation context, which scopes need on
//!   native targets, and `conformance::Strict`, a wrapping allocator for
//!   native test binaries. Implies `alloc`.
//! - `alloc`: [`Global`], the [`Source`] that uses the global allocator, as
//!   the default source of [`Block`] and [`FixedVec`].
//!
//! The crate is `no_std`; the heap itself needs neither `alloc` nor `std`.
//!
//! # Threads
//!
//! On wasm32 without the `atomics` target feature the heap and the allocation
//! context (active scope, lifetime hint, declared marker) are plain cells:
//! there is only one thread. Elsewhere the heap is behind a spin lock and,
//! with `std`, the context is thread-local, so a scope is active only on the
//! thread that runs it.
//!
//! Without `std` on any other target (a native target, or wasm32 with
//! `atomics`) there is no per-thread storage. The lifetime hint and the
//! declared marker are then process-wide, which can only change placement
//! between threads. Scopes cannot be activated there: [`Scope::run`],
//! [`Scope::scoped`] and `Scoped` are not compiled, because a process-wide
//! active scope would let one thread allocate into a scope that another
//! thread has already ended.
//!
//! # Example
//!
//! ```
//! use core::alloc::Layout;
//! use rtao_arena::{Config, FixedHeap, FixedVec, Lifetime, Scope};
//!
//! let heap: &'static FixedHeap = Box::leak(Box::new(FixedHeap::new(Config::new(1 << 20, 2 << 20))));
//! heap.init_static(Box::leak(vec![0u8; 8 << 20].into_boxed_slice()))?;
//!
//! // Ask before taking a large buffer, then take it fallibly.
//! assert!(heap.can_serve(1 << 20, 1 << 20));
//! let mut rows: FixedVec<u64, _> = FixedVec::try_with_capacity_in(1 << 17, Lifetime::Long, heap)?;
//! rows.try_push(7).map_err(|_| "full")?;
//!
//! // Transient work in a capped scope: while it runs, the heap's (global)
//! // allocations bump-allocate in the scope, and its memory returns when it
//! // ends. An explicit `heap` source, as for `rows`, ignores the scope.
//! let scope = Scope::new("scratch", 1 << 20, 64 << 10);
//! let layout = Layout::array::<u8>(1000)?;
//! scope.run(|| {
//!     let tmp = heap.alloc_with(layout, Lifetime::Short);
//!     assert!(!tmp.is_null());
//!     // SAFETY: `tmp` was allocated by `heap` with `layout`.
//!     unsafe { core::alloc::GlobalAlloc::dealloc(heap, tmp, layout) };
//! });
//! assert_eq!(scope.finish().parked, 0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod block;
mod ctx;
mod heap;
mod key;
mod raw;
mod scope;
mod slab;
mod tlsf;
mod vec;

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub mod conformance;

pub use block::{AllocError, BLOCK_ALIGN, Block, Global, Source};
pub use ctx::{
    DeclaredGuard, Lifetime, LifetimeGuard, declared, in_scope, is_declared, lifetime, unscoped,
    with_lifetime,
};
pub use heap::{ClassStats, Config, FixedHeap, InitError, SideStats, Stats};
pub use key::KeyBuf;
#[cfg(any(
    feature = "std",
    test,
    all(target_arch = "wasm32", not(target_feature = "atomics"))
))]
pub use scope::Scoped;
pub use scope::{MIN_CHUNK, Scope, ScopeStats, ScopeSummary};
pub use vec::{CapacityError, FixedVec};

/// Bytes of one slab page.
pub const PAGE_BYTES: usize = slab::PAGE;
/// The largest object the small side serves.
pub const MAX_SMALL: usize = slab::MAX_SMALL;
/// Object sizes of the slab classes.
pub const SIZE_CLASSES: [usize; 14] = slab::CLASSES;
/// Header bytes every large block carries in front of its payload.
pub const BLOCK_HEADER: usize = tlsf::HEADER;
