//! The large side: one two-level segregated fit (TLSF) pool that grows down.
//!
//! Every block starts with a 16-byte header: the physically previous block's
//! address (valid while that block is free) and the block's total size with
//! flag bits in the low four bits. A used block's payload follows the header,
//! so the word just below any payload is the size word. Free blocks keep their
//! free-list links in the payload. A zero-size sentinel header sits at the top
//! of the pool. The pool grows down into the gap by exactly what an allocation
//! needs, and its lowest block is never free: a free block that ends up lowest
//! is handed back to the gap at once, so the gap is the only free space below
//! the lowest used block.

#![allow(unsafe_code)]

use crate::raw::{Mem, align_down, align_up};

/// Allocation granule and the alignment of every block and payload.
pub(crate) const GRANULE: usize = 16;
/// Block header bytes in front of every payload.
pub(crate) const HEADER: usize = 16;
const WORD: usize = size_of::<usize>();
const PREV_OFF: usize = 0;
const SIZE_OFF: usize = HEADER - WORD;
const NEXT_FREE_OFF: usize = HEADER;
const PREV_FREE_OFF: usize = HEADER + WORD;
/// The smallest block: a header and room for the two free-list links.
pub(crate) const MIN_BLOCK: usize = HEADER + GRANULE;

/// The block is free.
const FREE: usize = 1;
/// The physically previous block is free.
const PREV_FREE: usize = 2;
/// Marks a scope object's tag word; never set in a block header.
pub(crate) const TAG: usize = 4;
/// The block was allocated with [`crate::Lifetime::Long`].
const LONG: usize = 8;
/// All flag bits.
pub(crate) const FLAGS: usize = 15;

const SL_LOG: u32 = 5;
const SL_COUNT: usize = 1 << SL_LOG;
const FL_SHIFT: u32 = SL_LOG + GRANULE.trailing_zeros();
const SMALL_BLOCK: usize = 1 << FL_SHIFT;
const FL_COUNT: usize = (usize::BITS - FL_SHIFT + 1) as usize;

/// Index of the most significant set bit (`value` must be non-zero).
#[inline]
const fn fls(value: usize) -> u32 {
    usize::BITS - 1 - value.leading_zeros()
}

/// The list a block of `size` bytes is filed under.
#[inline]
const fn mapping_insert(size: usize) -> (usize, usize) {
    if size < SMALL_BLOCK {
        (0, size / (SMALL_BLOCK / SL_COUNT))
    } else {
        let f = fls(size);
        let sl = (size >> (f - SL_LOG)) ^ SL_COUNT;
        ((f - FL_SHIFT + 1) as usize, sl)
    }
}

/// The first list whose every block holds at least `size` bytes.
#[inline]
const fn mapping_search(size: usize) -> Option<(usize, usize)> {
    if size < SMALL_BLOCK {
        return Some(mapping_insert(size));
    }
    let round = (1usize << (fls(size) - SL_LOG)) - 1;
    match size.checked_add(round) {
        Some(rounded) => Some(mapping_insert(rounded)),
        None => None,
    }
}

/// The TLSF pool's control structure. It lives outside the region and has a
/// fixed size; no per-allocation metadata lives outside the region.
pub(crate) struct Tlsf {
    fl_bitmap: usize,
    sl_bitmap: [u32; FL_COUNT],
    heads: [[usize; SL_COUNT]; FL_COUNT],
    /// Lowest block address (the pool's bottom); equals `top` when empty.
    pub(crate) lo: usize,
    /// Address of the sentinel header (the pool's top).
    pub(crate) top: usize,
    /// Sum of free block sizes, headers included.
    pub(crate) free_bytes: usize,
    /// Number of free blocks.
    pub(crate) free_blocks: usize,
    /// Sum of used block sizes, headers included.
    pub(crate) used_bytes: usize,
    /// Number of used blocks.
    pub(crate) used_blocks: usize,
}

impl Tlsf {
    pub(crate) const fn new() -> Self {
        Self {
            fl_bitmap: 0,
            sl_bitmap: [0; FL_COUNT],
            heads: [[0; SL_COUNT]; FL_COUNT],
            lo: 0,
            top: 0,
            free_bytes: 0,
            free_blocks: 0,
            used_bytes: 0,
            used_blocks: 0,
        }
    }

    /// Sets up an empty pool whose sentinel header sits at `top`.
    ///
    /// # Safety
    ///
    /// `[top, top + HEADER)` must be inside the region, 16-byte aligned and
    /// owned by the heap.
    pub(crate) unsafe fn init(&mut self, mem: Mem, top: usize) {
        *self = Self::new();
        self.lo = top;
        self.top = top;
        // SAFETY: the caller hands the sentinel's header to the pool.
        unsafe {
            mem.set_word(top + PREV_OFF, 0);
            mem.set_word(top + SIZE_OFF, 0);
        }
    }

    // ---- header helpers -------------------------------------------------

    /// # Safety
    /// `block` must be a block header in this pool.
    #[inline]
    unsafe fn size(mem: Mem, block: usize) -> usize {
        // SAFETY: forwarded from the caller.
        unsafe { mem.word(block + SIZE_OFF) & !FLAGS }
    }

    /// # Safety
    /// `block` must be a block header in this pool.
    #[inline]
    unsafe fn flags(mem: Mem, block: usize) -> usize {
        // SAFETY: forwarded from the caller.
        unsafe { mem.word(block + SIZE_OFF) & FLAGS }
    }

    /// # Safety
    /// `block` must be a block header in this pool.
    #[inline]
    unsafe fn set_header(mem: Mem, block: usize, size: usize, flags: usize) {
        // SAFETY: forwarded from the caller.
        unsafe { mem.set_word(block + SIZE_OFF, size | flags) }
    }

    /// Sets or clears `PREV_FREE` on `block`, recording `prev` when set.
    ///
    /// # Safety
    /// `block` must be a block header (or the sentinel) in this pool.
    #[inline]
    unsafe fn set_prev(mem: Mem, block: usize, prev: Option<usize>) {
        // SAFETY: forwarded from the caller.
        unsafe {
            let word = mem.word(block + SIZE_OFF);
            match prev {
                Some(prev) => {
                    mem.set_word(block + PREV_OFF, prev);
                    mem.set_word(block + SIZE_OFF, word | PREV_FREE);
                }
                None => mem.set_word(block + SIZE_OFF, word & !PREV_FREE),
            }
        }
    }

    // ---- free lists -----------------------------------------------------

    /// # Safety
    /// `block` must be a free block of `size` bytes that is not on a list.
    unsafe fn insert(&mut self, mem: Mem, block: usize, size: usize) {
        let (fl, sl) = mapping_insert(size);
        let head = self.heads[fl][sl];
        // SAFETY: a free block's payload holds its links; `head` is a free block.
        unsafe {
            Self::set_header(mem, block, size, FREE);
            mem.set_word(block + NEXT_FREE_OFF, head);
            mem.set_word(block + PREV_FREE_OFF, 0);
            if head != 0 {
                mem.set_word(head + PREV_FREE_OFF, block);
            }
        }
        self.heads[fl][sl] = block;
        self.fl_bitmap |= 1 << fl;
        self.sl_bitmap[fl] |= 1 << sl;
        self.free_bytes += size;
        self.free_blocks += 1;
    }

    /// # Safety
    /// `block` must be a free block on its list.
    unsafe fn remove(&mut self, mem: Mem, block: usize) {
        // SAFETY: `block` is free, so its header and links are valid.
        let (size, next, prev) = unsafe {
            (
                Self::size(mem, block),
                mem.word(block + NEXT_FREE_OFF),
                mem.word(block + PREV_FREE_OFF),
            )
        };
        let (fl, sl) = mapping_insert(size);
        // SAFETY: neighbours on a free list are free blocks.
        unsafe {
            if next != 0 {
                mem.set_word(next + PREV_FREE_OFF, prev);
            }
            if prev != 0 {
                mem.set_word(prev + NEXT_FREE_OFF, next);
            }
        }
        if self.heads[fl][sl] == block {
            self.heads[fl][sl] = next;
            if next == 0 {
                self.sl_bitmap[fl] &= !(1 << sl);
                if self.sl_bitmap[fl] == 0 {
                    self.fl_bitmap &= !(1 << fl);
                }
            }
        }
        self.free_bytes -= size;
        self.free_blocks -= 1;
    }

    /// The O(1) good-fit search: the head of the first non-empty list whose
    /// every block holds at least `need` bytes.
    fn good_fit(&self, need: usize) -> Option<usize> {
        let (fl, sl) = mapping_search(need)?;
        if fl >= FL_COUNT {
            return None;
        }
        let mut sl_map = self.sl_bitmap[fl] & (!0u32).checked_shl(sl as u32).unwrap_or(0);
        let mut fl = fl;
        if sl_map == 0 {
            let fl_map = self.fl_bitmap & (!0usize).checked_shl(fl as u32 + 1).unwrap_or(0);
            if fl_map == 0 {
                return None;
            }
            fl = fl_map.trailing_zeros() as usize;
            sl_map = self.sl_bitmap[fl];
        }
        Some(self.heads[fl][sl_map.trailing_zeros() as usize])
    }

    /// Scans the list `need` falls in for a block the good-fit search skips
    /// (one in the same list, at least `need` bytes). Only an allocation that
    /// would otherwise fail runs it; it keeps [`Self::largest_free`] exact.
    fn scan(&self, mem: Mem, need: usize) -> Option<usize> {
        let (fl, sl) = mapping_insert(need);
        let mut block = self.heads[fl][sl];
        while block != 0 {
            // SAFETY: blocks on a free list are free blocks of this pool.
            unsafe {
                if Self::size(mem, block) >= need {
                    return Some(block);
                }
                block = mem.word(block + NEXT_FREE_OFF);
            }
        }
        None
    }

    // ---- allocation -----------------------------------------------------

    /// Allocates `size` payload bytes aligned to `align` (a power of two),
    /// never placing a block below `floor`. Returns the payload address.
    pub(crate) fn alloc(
        &mut self,
        mem: Mem,
        size: usize,
        align: usize,
        long: bool,
        floor: usize,
    ) -> Option<usize> {
        let size = align_up(size.max(GRANULE), GRANULE)?;
        let align = align.max(GRANULE);
        let payload = if align <= GRANULE {
            size
        } else {
            size.checked_add(align + MIN_BLOCK - GRANULE)?
        };
        let need = payload.checked_add(HEADER)?;
        // Long: the gap first (the lowest address), then a free block.
        // Short: a free block first (placed at its top), then the gap.
        if long && let Some(addr) = self.extend(mem, size, align, long, floor) {
            return Some(addr);
        }
        if let Some(block) = self.good_fit(need) {
            // SAFETY: a free block of this pool with room for `size` bytes at
            // `align`.
            return Some(unsafe { self.place(mem, block, size, align, long) });
        }
        if !long && let Some(addr) = self.extend(mem, size, align, long, floor) {
            return Some(addr);
        }
        let block = self.scan(mem, need)?;
        // SAFETY: as above.
        Some(unsafe { self.place(mem, block, size, align, long) })
    }

    /// Carves an allocation out of the free `block`: at its low end for a long
    /// lifetime, at its high end otherwise. Leftovers become free blocks.
    ///
    /// # Safety
    /// `block` must be a free block on its list with room for `size` bytes at
    /// `align`, including the padding that makes a front split possible.
    unsafe fn place(
        &mut self,
        mem: Mem,
        block: usize,
        size: usize,
        align: usize,
        long: bool,
    ) -> usize {
        // SAFETY: `block` is a free block on its list.
        let block_size = unsafe {
            self.remove(mem, block);
            Self::size(mem, block)
        };
        let end = block + block_size;
        let first = block + HEADER;
        let low = {
            let p = align_down(first + align - 1, align);
            if p != first && p - first < MIN_BLOCK {
                align_down(first + MIN_BLOCK + align - 1, align)
            } else {
                p
            }
        };
        let payload = if long {
            low
        } else {
            let high = align_down(end - size, align);
            let front = high - HEADER - block;
            if front != 0 && front < MIN_BLOCK {
                low
            } else {
                high
            }
        };
        let start = payload - HEADER;
        let front = start - block;
        let mut used_end = payload + size;
        if end - used_end < MIN_BLOCK {
            used_end = end;
        }
        let long_flag = if long { LONG } else { 0 };
        // SAFETY: every address written below lies inside the removed free
        // block or is the header of the block that follows it.
        unsafe {
            if front > 0 {
                self.insert(mem, block, front);
                Self::set_header(mem, start, used_end - start, PREV_FREE | long_flag);
                mem.set_word(start + PREV_OFF, block);
            } else {
                Self::set_header(mem, start, used_end - start, long_flag);
            }
            if used_end < end {
                self.insert(mem, used_end, end - used_end);
                Self::set_prev(mem, end, Some(used_end));
            } else {
                Self::set_prev(mem, end, None);
            }
        }
        self.used_bytes += used_end - start;
        self.used_blocks += 1;
        payload
    }

    /// Grows the pool down into the gap by what one allocation needs.
    fn extend(
        &mut self,
        mem: Mem,
        size: usize,
        align: usize,
        long: bool,
        floor: usize,
    ) -> Option<usize> {
        let payload = align_down(self.lo.checked_sub(size)?, align);
        let start = payload.checked_sub(HEADER)?;
        if start < floor {
            return None;
        }
        let mut used_end = payload + size;
        let old_lo = self.lo;
        let long_flag = if long { LONG } else { 0 };
        // SAFETY: `[start, old_lo)` is gap memory above the floor that the pool
        // now claims; `old_lo` is the old lowest block or the sentinel.
        unsafe {
            if old_lo - used_end >= MIN_BLOCK {
                self.insert(mem, used_end, old_lo - used_end);
                Self::set_prev(mem, old_lo, Some(used_end));
            } else {
                used_end = old_lo;
            }
            mem.set_word(start + PREV_OFF, 0);
            Self::set_header(mem, start, used_end - start, long_flag);
        }
        self.lo = start;
        self.used_bytes += used_end - start;
        self.used_blocks += 1;
        Some(payload)
    }

    /// Frees the block whose payload is at `payload`, coalescing at once with
    /// free neighbours and handing a free lowest block back to the gap.
    ///
    /// # Safety
    /// `payload` must be the payload of a used block of this pool.
    pub(crate) unsafe fn free(&mut self, mem: Mem, payload: usize) {
        let mut block = payload - HEADER;
        // SAFETY: `block` is a used block; neighbours are read through valid
        // headers (the sentinel bounds the top, `PREV_FREE` guards the bottom).
        unsafe {
            let mut size = Self::size(mem, block);
            let flags = Self::flags(mem, block);
            self.used_bytes -= size;
            self.used_blocks -= 1;
            let next = block + size;
            if flags & PREV_FREE != 0 {
                let prev = mem.word(block + PREV_OFF);
                self.remove(mem, prev);
                size += Self::size(mem, prev);
                block = prev;
            }
            if Self::flags(mem, next) & FREE != 0 {
                size += Self::size(mem, next);
                self.remove(mem, next);
            }
            let after = block + size;
            if block == self.lo {
                self.lo = after;
                Self::set_prev(mem, after, None);
            } else {
                self.insert(mem, block, size);
                Self::set_prev(mem, after, Some(block));
            }
        }
    }

    /// Whether the used block at `payload` was allocated long-lived.
    ///
    /// # Safety
    /// `payload` must be the payload of a used block of this pool.
    pub(crate) unsafe fn is_long(mem: Mem, payload: usize) -> bool {
        // SAFETY: forwarded from the caller.
        unsafe { Self::flags(mem, payload - HEADER) & LONG != 0 }
    }

    /// Resizes the block at `payload` in place: shrinking splits off the tail,
    /// growing absorbs a free upper neighbour. Returns whether it worked.
    ///
    /// # Safety
    /// `payload` must be the payload of a used block of this pool.
    pub(crate) unsafe fn resize_in_place(
        &mut self,
        mem: Mem,
        payload: usize,
        new_size: usize,
    ) -> bool {
        let Some(size) = align_up(new_size.max(GRANULE), GRANULE) else {
            return false;
        };
        let Some(need) = size.checked_add(HEADER) else {
            return false;
        };
        let block = payload - HEADER;
        // SAFETY: `block` is a used block; its upper neighbour is a valid header.
        unsafe {
            let current = Self::size(mem, block);
            let flags = Self::flags(mem, block);
            if need <= current {
                if current - need >= MIN_BLOCK {
                    let tail = block + need;
                    Self::set_header(mem, block, need, flags);
                    Self::set_header(mem, tail, current - need, 0);
                    self.used_blocks += 1;
                    self.free(mem, tail + HEADER);
                }
                return true;
            }
            let next = block + current;
            if Self::flags(mem, next) & FREE == 0 {
                return false;
            }
            let total = current + Self::size(mem, next);
            if total < need {
                return false;
            }
            self.remove(mem, next);
            let end = block + total;
            if total - need >= MIN_BLOCK {
                let tail = block + need;
                Self::set_header(mem, block, need, flags);
                self.insert(mem, tail, total - need);
                Self::set_prev(mem, end, Some(tail));
                self.used_bytes += need - current;
            } else {
                Self::set_header(mem, block, total, flags);
                Self::set_prev(mem, end, None);
                self.used_bytes += total - current;
            }
            true
        }
    }

    /// Grows the pool's lowest block downward when it cannot grow in place:
    /// the gap down to `floor`, the block itself and a free upper neighbour
    /// become one block, placed as high as `new_size` bytes at `align` allow,
    /// and the first `keep` payload bytes move down with it. A buffer that
    /// keeps growing this way (such as a doubling vector) reuses the room of
    /// its earlier sizes instead of stranding it above itself. Returns the new
    /// payload address, or `None` (nothing changed) if the block is not the
    /// lowest or the room does not suffice.
    ///
    /// # Safety
    /// `payload` must be the payload of a used block of this pool, made with
    /// an alignment of at most `align`, holding at least `keep` bytes.
    pub(crate) unsafe fn grow_down(
        &mut self,
        mem: Mem,
        payload: usize,
        new_size: usize,
        align: usize,
        keep: usize,
        floor: usize,
    ) -> Option<usize> {
        let block = payload - HEADER;
        if block != self.lo {
            return None;
        }
        let size = align_up(new_size.max(GRANULE), GRANULE)?;
        let align = align.max(GRANULE);
        // SAFETY: `block` is a used block; its upper neighbour is a valid
        // header (a block or the sentinel), and every address written lies
        // in the gap above `floor`, in `block` or in its free neighbour.
        unsafe {
            let current = Self::size(mem, block);
            let flags = Self::flags(mem, block);
            let next = block + current;
            let next_free = Self::flags(mem, next) & FREE != 0;
            let end = if next_free {
                next + Self::size(mem, next)
            } else {
                next
            };
            let to = align_down(end.checked_sub(size)?, align);
            let start = to.checked_sub(HEADER)?;
            if start < floor || start > block {
                return None;
            }
            if next_free {
                self.remove(mem, next);
            }
            // Move first: the source is dead afterwards, and no header written
            // below lies inside the destination `[to, to + keep)`.
            mem.move_bytes(payload, to, keep);
            let used_end = if end - (to + size) >= MIN_BLOCK {
                to + size
            } else {
                end
            };
            mem.set_word(start + PREV_OFF, 0);
            Self::set_header(mem, start, used_end - start, flags & LONG);
            if used_end < end {
                self.insert(mem, used_end, end - used_end);
                Self::set_prev(mem, end, Some(used_end));
            } else {
                Self::set_prev(mem, end, None);
            }
            self.lo = start;
            self.used_bytes = self.used_bytes - current + (used_end - start);
            Some(to)
        }
    }

    // ---- queries --------------------------------------------------------

    /// Size of the largest free block (headers included), or 0.
    pub(crate) fn largest_free(&self, mem: Mem) -> usize {
        if self.fl_bitmap == 0 {
            return 0;
        }
        let fl = fls(self.fl_bitmap) as usize;
        let sl = fls(self.sl_bitmap[fl] as usize) as usize;
        let mut best = 0;
        let mut block = self.heads[fl][sl];
        while block != 0 {
            // SAFETY: blocks on a free list are free blocks of this pool.
            unsafe {
                best = best.max(Self::size(mem, block));
                block = mem.word(block + NEXT_FREE_OFF);
            }
        }
        best
    }

    /// The largest payload one extension into the gap above `floor` can hold.
    pub(crate) fn gap_payload(&self, floor: usize) -> usize {
        if self.lo <= floor {
            return 0;
        }
        align_down((self.lo - floor).saturating_sub(HEADER), GRANULE)
    }

    /// Pool bytes claimed from the region (blocks and sentinel header).
    pub(crate) fn extent(&self) -> usize {
        self.top - self.lo + HEADER
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn mapping_is_monotonic_and_search_rounds_up() {
        let mut last = (0, 0);
        for size in (GRANULE..1 << 22).step_by(GRANULE) {
            let class = mapping_insert(size);
            assert!(class >= last, "{size}");
            last = class;
            let (fl, sl) = mapping_search(size).unwrap();
            // Every block filed under the searched list is at least `size`.
            let lower = if fl == 0 {
                sl * (SMALL_BLOCK / SL_COUNT)
            } else {
                let f = fl as u32 + FL_SHIFT - 1;
                (1 << f) + (sl << (f - SL_LOG))
            };
            assert!(lower >= size, "{size}: {fl}/{sl} starts at {lower}");
        }
        assert!(mapping_insert(usize::MAX).0 < FL_COUNT);
        assert!(mapping_search(usize::MAX).is_none());
    }
}
