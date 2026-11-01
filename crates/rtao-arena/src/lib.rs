//! Fixed-capacity memory for wasm32 and other constrained hosts whose linear
//! memory must never grow.
//!
//! [`FixedHeap`] is a [`GlobalAlloc`](core::alloc::GlobalAlloc) over one
//! region chosen up front. It never asks the host for more memory: when the
//! region is full, allocation returns null. Pages and large blocks are blocks
//! of one pool over the whole region, and the two sides fill it from its two
//! ends:
//!
//! ```text
//! region start                                                        region end
//! | page table | slab pages (lowest free first) ->  <- large blocks (at the top) |
//! |            |<--- small_floor: no large block --->|                          |
//! |            |                   |<------- large_floor: no slab page -------->|
//! ```
//!
//! - **Small side.** Objects up to 2 KiB live in 64 KiB slab pages of 14
//!   size classes (16 … 2048 bytes), each at its natural alignment. The small
//!   side takes the lowest free page in the region and gives a page back as
//!   soon as its last object is freed, wherever the page lies, so its memory
//!   serves large blocks again at once. Objects never share a page with large
//!   blocks.
//! - **Large side.** Larger (or more strictly aligned) blocks come from the
//!   same two-level segregated fit pool, with O(1) good-fit search and
//!   immediate coalescing: a freed block or page merges with its free
//!   neighbours, whatever they held. Blocks are carved from the top of a free
//!   block, so the large side grows down from the top of the region while the
//!   small side fills it from the bottom. [`Lifetime::Short`] blocks (the
//!   default) take the best-fitting free block, so transients reuse holes;
//!   [`Lifetime::Long`] blocks take a block of the largest size class, packing
//!   against the large blocks above the largest free run and leaving holes to
//!   transients. Alignment up to 64 KiB and beyond works.
//! - **Floors.** [`Config::small_floor`] and [`Config::large_floor`] are the
//!   only split configuration. Each reserves one end of the region for one
//!   side, so that side can always reach its floor; between them, free memory
//!   belongs to whichever side asks for it.
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
