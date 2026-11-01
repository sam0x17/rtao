# rtao-arena

Fixed-capacity memory for wasm32 and other constrained hosts whose linear
memory must never grow.

`FixedHeap` is a global allocator over one region chosen up front. It never
asks the host for more memory; when the region is full, allocation returns
null. Small objects (up to 2 KiB) live in size-class slab pages and larger
blocks in a TLSF pool, and pages are blocks of that same pool: the small side
takes the lowest free page and gives a page back as soon as it is empty,
wherever it lies, large blocks are carved from the top (from memory no page
may use whenever it holds them), and freed memory coalesces at once, so
neither side strands memory the other needs. Two configurable floors reserve
one end of the region for each side; each is a minimum for its side and a
ceiling for the other, so set it to the least its side must always reach.
Allocation and deallocation are O(1) on both sides, and all per-object
metadata lives inside the region.

`Scope` and `Scoped` provide capped, chunked sub-arenas for transient work:
while a scope is active its thread's allocations bump-allocate in it, and its
chunks return as soon as their objects are freed. `Block`, `FixedVec` and
`KeyBuf` are fallible, non-growing building blocks. Queries such as
`can_serve` and `largest_free_large` tell whether a large request would
succeed before it is made, and `stats` reports per-side and per-class
counters.

The crate is `no_std`; the heap needs neither `alloc` nor `std`.

| Feature | Effect |
| --- | --- |
| `std` (default) | Thread-local allocation context, which scopes need on native targets, and `conformance::Strict`, a wrapping allocator for native test binaries that runs a `FixedHeap` of a chosen shape and fails on exhaustion, on scopes that end with live objects and, optionally, on large allocations outside scopes or declared blocks. Implies `alloc`. |
| `alloc` | `Global`, the source that uses the global allocator, as the default source of `Block` and `FixedVec`. |

Scopes are activated per thread. Without `std` they are available only on
wasm32 without the `atomics` target feature, where there is one thread; on
other `no_std` targets `Scope::run` and `Scope::scoped` are not compiled.

On wasm32, `FixedHeap::wasm` manages `[__heap_base, memory.size)` and is set
up at the first allocation; link the module with equal initial and maximum
memory so the size is fixed.

This crate is the workspace's one exception to `unsafe_code = "forbid"`: it
denies unsafe code at crate level and allows it only in the modules that
manage raw memory, each unsafe block with a `SAFETY` comment.
