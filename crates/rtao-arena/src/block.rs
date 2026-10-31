//! Exact-size byte blocks and the allocation sources behind them.

#![allow(unsafe_code)]

use core::alloc::{GlobalAlloc, Layout};
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};

use crate::ctx::{self, Lifetime};
use crate::heap::FixedHeap;

/// An allocation was refused (exhaustion, a scope cap or an invalid layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocError {
    /// Bytes requested.
    pub size: usize,
    /// Alignment requested.
    pub align: usize,
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not allocate {} bytes (align {})",
            self.size, self.align
        )
    }
}

impl core::error::Error for AllocError {}

/// Where [`Block`] and [`crate::FixedVec`] get their memory.
///
/// [`Global`] goes through the global allocator, so while a [`crate::Scope`]
/// is active on the thread and the global allocator is a [`FixedHeap`], the
/// scope serves the allocation and the lifetime hint does not apply (wrap the
/// call in [`crate::unscoped`] to opt out). An explicit `&FixedHeap` always
/// allocates from that heap's general sides with the hint
/// ([`FixedHeap::alloc_unscoped`]), whether or not a scope is active.
///
/// # Safety
///
/// `allocate` must return either `None` or memory valid for `layout` that no
/// other live allocation overlaps, and `release` must accept exactly what
/// `allocate` returned with the same layout.
pub unsafe trait Source: Copy {
    /// Allocates `layout` (non-zero size) with a lifetime hint.
    fn allocate(self, layout: Layout, hint: Lifetime) -> Option<NonNull<u8>>;

    /// Releases memory from [`Source::allocate`].
    ///
    /// # Safety
    ///
    /// `ptr` must come from `allocate` on an equal source with `layout`, and
    /// must not be used afterwards.
    unsafe fn release(self, ptr: NonNull<u8>, layout: Layout);
}

/// The global allocator, with the hint and the declared-block marker set for
/// the duration of each allocation. An active scope serves it (see
/// [`Source`]). Without the `alloc` feature it refuses every allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Global;

// SAFETY: forwards to the global allocator, which upholds the contract.
unsafe impl Source for Global {
    #[cfg(feature = "alloc")]
    fn allocate(self, layout: Layout, hint: Lifetime) -> Option<NonNull<u8>> {
        let _hint = ctx::with_lifetime(hint);
        let _declared = ctx::declared();
        // SAFETY: callers pass non-zero-size layouts (the trait's contract).
        NonNull::new(unsafe { alloc::alloc::alloc(layout) })
    }

    #[cfg(not(feature = "alloc"))]
    fn allocate(self, _layout: Layout, _hint: Lifetime) -> Option<NonNull<u8>> {
        None
    }

    #[cfg(feature = "alloc")]
    unsafe fn release(self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded from the caller.
        unsafe { alloc::alloc::dealloc(ptr.as_ptr(), layout) }
    }

    #[cfg(not(feature = "alloc"))]
    unsafe fn release(self, _ptr: NonNull<u8>, _layout: Layout) {}
}

// SAFETY: forwards to the heap, which upholds the contract.
unsafe impl Source for &FixedHeap {
    fn allocate(self, layout: Layout, hint: Lifetime) -> Option<NonNull<u8>> {
        let _declared = ctx::declared();
        NonNull::new(self.alloc_unscoped(layout, hint))
    }

    unsafe fn release(self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded from the caller.
        unsafe { self.dealloc(ptr.as_ptr(), layout) }
    }
}

/// An exact-size, zero-initialized byte block.
///
/// A block is one allocation of exactly the requested length; it never grows
/// and never reallocates, so it is a natural unit for callers that admit
/// memory before using it and want the admitted bytes and the memory to be one
/// object. Wrap it in your own type to attach accounting.
///
/// ```
/// use rtao_arena::{Block, Lifetime};
///
/// let mut block = Block::try_new_zeroed(64 << 10, Lifetime::Long)?;
/// block[..5].copy_from_slice(b"hello");
/// assert_eq!(block.len(), 64 << 10);
/// assert_eq!(&block[..5], b"hello");
/// # Ok::<(), rtao_arena::AllocError>(())
/// ```
pub struct Block<S: Source = Global> {
    ptr: NonNull<u8>,
    len: usize,
    align: usize,
    source: S,
}

/// Default alignment of a [`Block`].
pub const BLOCK_ALIGN: usize = 16;

impl Block<Global> {
    /// A zeroed block of `len` bytes from the global allocator.
    pub fn try_new_zeroed(len: usize, hint: Lifetime) -> Result<Self, AllocError> {
        Self::try_new_zeroed_in(len, BLOCK_ALIGN, hint, Global)
    }
}

impl<S: Source> Block<S> {
    /// A zeroed block of `len` bytes at `align` (a power of two) from `source`.
    ///
    /// An explicit `&FixedHeap` source ignores an active scope; [`Global`]
    /// follows it (see [`Source`]).
    pub fn try_new_zeroed_in(
        len: usize,
        align: usize,
        hint: Lifetime,
        source: S,
    ) -> Result<Self, AllocError> {
        let error = AllocError { size: len, align };
        let layout = Layout::from_size_align(len, align).map_err(|_| error)?;
        let ptr = if len == 0 {
            // A dangling, aligned pointer for an empty block.
            NonNull::new(ptr::without_provenance_mut(align)).ok_or(error)?
        } else {
            let ptr = source.allocate(layout, hint).ok_or(error)?;
            // SAFETY: `ptr` is valid for `len` bytes.
            unsafe { ptr.as_ptr().write_bytes(0, len) };
            ptr
        };
        Ok(Self {
            ptr,
            len,
            align: layout.align(),
            source,
        })
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the block is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The block's alignment.
    pub fn align(&self) -> usize {
        self.align
    }

    /// The block's bytes.
    pub fn as_slice(&self) -> &[u8] {
        self
    }

    /// The block's bytes, mutably.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self
    }
}

impl<S: Source> Deref for Block<S> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // SAFETY: `ptr` is valid and initialized for `len` bytes (or dangling
        // and aligned with `len == 0`).
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<S: Source> DerefMut for Block<S> {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `deref`, and `&mut self` makes the access unique.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<S: Source> AsRef<[u8]> for Block<S> {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl<S: Source> AsMut<[u8]> for Block<S> {
    fn as_mut(&mut self) -> &mut [u8] {
        self
    }
}

impl<S: Source> Drop for Block<S> {
    fn drop(&mut self) {
        if self.len != 0 {
            // SAFETY: allocated by `source` with exactly this layout.
            unsafe {
                let layout = Layout::from_size_align_unchecked(self.len, self.align);
                self.source.release(self.ptr, layout);
            }
        }
    }
}

impl<S: Source> fmt::Debug for Block<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("len", &self.len)
            .field("align", &self.align)
            .finish()
    }
}

// SAFETY: a block uniquely owns its bytes; sending it sends the source, which
// must itself be `Send`.
unsafe impl<S: Source + Send> Send for Block<S> {}
// SAFETY: shared access only reads the bytes.
unsafe impl<S: Source + Sync> Sync for Block<S> {}
