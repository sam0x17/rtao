//! [`FixedVec`]: a vector whose capacity is fixed when it is created.

#![allow(unsafe_code)]

use core::alloc::Layout;
use core::fmt;
use core::marker::PhantomData;
use core::mem::needs_drop;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};

use crate::block::{AllocError, Global, Source};
use crate::ctx::Lifetime;

/// The vector is full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityError {
    /// The fixed capacity.
    pub capacity: usize,
}

impl fmt::Display for CapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "capacity of {} exhausted", self.capacity)
    }
}

impl core::error::Error for CapacityError {}

/// A vector with a capacity fixed at creation: one allocation, no growth.
///
/// Pushing past the capacity hands the value back instead of reallocating, so
/// the memory a `FixedVec` holds is known up front.
///
/// ```
/// use rtao_arena::FixedVec;
///
/// let mut v: FixedVec<u32> = FixedVec::try_with_capacity(2)?;
/// v.try_push(1).map_err(|_| "full")?;
/// v.try_push(2).map_err(|_| "full")?;
/// assert_eq!(v.try_push(3), Err(3));
/// assert_eq!(&v[..], &[1, 2]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct FixedVec<T, S: Source = Global> {
    ptr: NonNull<T>,
    len: usize,
    capacity: usize,
    source: S,
    _owns: PhantomData<T>,
}

impl<T> FixedVec<T, Global> {
    /// An empty vector with room for exactly `capacity` elements from the
    /// global allocator.
    pub fn try_with_capacity(capacity: usize) -> Result<Self, AllocError> {
        Self::try_with_capacity_in(capacity, Lifetime::Short, Global)
    }
}

impl<T, S: Source> FixedVec<T, S> {
    /// An empty vector with room for exactly `capacity` elements from `source`.
    ///
    /// An explicit `&FixedHeap` source ignores an active scope; [`Global`]
    /// follows it (see [`Source`](crate::Source)).
    pub fn try_with_capacity_in(
        capacity: usize,
        hint: Lifetime,
        source: S,
    ) -> Result<Self, AllocError> {
        let error = AllocError {
            size: size_of::<T>().saturating_mul(capacity),
            align: align_of::<T>(),
        };
        let layout = Layout::array::<T>(capacity).map_err(|_| error)?;
        let ptr = if layout.size() == 0 {
            NonNull::dangling()
        } else {
            source.allocate(layout, hint).ok_or(error)?.cast::<T>()
        };
        Ok(Self {
            ptr,
            len: 0,
            capacity,
            source,
            _owns: PhantomData,
        })
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the vector is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The fixed capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Free element slots.
    pub fn remaining(&self) -> usize {
        self.capacity - self.len
    }

    /// Whether no slot is free.
    pub fn is_full(&self) -> bool {
        self.len == self.capacity
    }

    /// Appends `value`, or hands it back when the vector is full.
    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.len == self.capacity {
            return Err(value);
        }
        // SAFETY: `len < capacity`, so the slot is inside the allocation and
        // uninitialized.
        unsafe { self.ptr.as_ptr().add(self.len).write(value) };
        self.len += 1;
        Ok(())
    }

    /// Removes and returns the last element.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: the slot at the old last index is initialized and is no
        // longer counted.
        Some(unsafe { self.ptr.as_ptr().add(self.len).read() })
    }

    /// Drops the elements from `len` on.
    pub fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        let tail = ptr::slice_from_raw_parts_mut(
            // SAFETY: `len < self.len`, inside the allocation.
            unsafe { self.ptr.as_ptr().add(len) },
            self.len - len,
        );
        self.len = len;
        if needs_drop::<T>() {
            // SAFETY: the tail was initialized and is no longer counted.
            unsafe { ptr::drop_in_place(tail) };
        }
    }

    /// Drops every element; the capacity stays.
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Appends clones of `items`, all or nothing.
    pub fn try_extend_from_slice(&mut self, items: &[T]) -> Result<(), CapacityError>
    where
        T: Clone,
    {
        if items.len() > self.remaining() {
            return Err(CapacityError {
                capacity: self.capacity,
            });
        }
        for item in items {
            if self.try_push(item.clone()).is_err() {
                return Err(CapacityError {
                    capacity: self.capacity,
                });
            }
        }
        Ok(())
    }

    /// The elements.
    pub fn as_slice(&self) -> &[T] {
        self
    }

    /// The elements, mutably.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self
    }

    fn layout(&self) -> Option<Layout> {
        Layout::array::<T>(self.capacity)
            .ok()
            .filter(|l| l.size() != 0)
    }
}

impl<T, S: Source> Deref for FixedVec<T, S> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        // SAFETY: the first `len` slots are initialized.
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<T, S: Source> DerefMut for FixedVec<T, S> {
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: the first `len` slots are initialized; access is unique.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T, S: Source> Drop for FixedVec<T, S> {
    fn drop(&mut self) {
        self.clear();
        if let Some(layout) = self.layout() {
            // SAFETY: allocated by `source` with exactly this layout.
            unsafe { self.source.release(self.ptr.cast::<u8>(), layout) };
        }
    }
}

impl<T: fmt::Debug, S: Source> fmt::Debug for FixedVec<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a, T, S: Source> IntoIterator for &'a FixedVec<T, S> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T, S: Source> IntoIterator for &'a mut FixedVec<T, S> {
    type Item = &'a mut T;
    type IntoIter = core::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

// SAFETY: the vector uniquely owns its elements.
unsafe impl<T: Send, S: Source + Send> Send for FixedVec<T, S> {}
// SAFETY: shared access only hands out `&T`.
unsafe impl<T: Sync, S: Source + Sync> Sync for FixedVec<T, S> {}
