//! The pool: one two-level segregated fit (TLSF) pool over the whole region.
//!
//! Every byte from the first page to the top of the region belongs to exactly
//! one block: a free block, a used large block or a slab page. Free and large
//! blocks start with a 16-byte header: the physically previous block's
//! address (valid while that block is free) and the block's total size with
//! flag bits in the low four bits. A large block's payload follows its
//! header, so the word just below any payload is the size word. Free blocks
//! keep their free-list links in their payload. A slab page is a block of
//! exactly one 64 KiB-aligned page whose header lives in a per-page word
//! instead, so the small side keeps whole pages. A zero-size sentinel header
//! sits at the top of the pool.
//!
//! Freeing coalesces at once with free neighbours, whatever they held, so a
//! page the small side gives back and the blocks the large side frees around
//! it become one free block, anywhere in the region. Two lines bound the
//! sides: no large block starts below the small floor's line, and no page
//! ends above the large floor's line. A free block is filed under the part
//! of it a large block may use (the part above the line), so the good-fit
//! search never returns a block the large side cannot use, and a block wholly
//! below the line is on no list at all.
//!
//! The small side takes the lowest free page. A bitmap marks, for every free
//! block that could give a page, the first page it could give, and that
//! page's word names the block; the lowest mark is therefore the lowest free
//! page in the region. A page is given only with nothing, or a whole free
//! block of at least [`MIN_BLOCK`] bytes, left on each side of it.

#![allow(unsafe_code)]

use crate::raw::{Mem, align_down, align_up};
use crate::slab::PAGE;

/// Allocation granule and the alignment of every block and payload.
pub(crate) const GRANULE: usize = 16;
/// Block header bytes in front of every payload.
pub(crate) const HEADER: usize = 16;
const WORD: usize = size_of::<usize>();
const WORD_BITS: usize = usize::BITS as usize;
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

/// Set in a page's word while a slab page block starts there; the rest of
/// the word is then the address of its free lower neighbour, or 0.
const PAGE_BLOCK: usize = 1;

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

/// The two lines that bound the sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lines {
    /// The lowest address a large block may start at (16-aligned).
    pub(crate) floor: usize,
    /// The highest address a page may end at.
    pub(crate) page_limit: usize,
}

/// The pool's control structure. It lives outside the region and has a fixed
/// size; the per-page words and the bitmap live in the region's table, and
/// no per-allocation metadata lives outside the region.
pub(crate) struct Tlsf {
    fl_bitmap: usize,
    sl_bitmap: [u32; FL_COUNT],
    heads: [[usize; SL_COUNT]; FL_COUNT],
    /// Address of the first page, the pool's lowest block.
    pub(crate) lo: usize,
    /// Address of the sentinel header (the pool's top).
    pub(crate) top: usize,
    /// Address of the per-page words.
    words: usize,
    /// Address of the first-page bitmap (one bit per page).
    bitmap: usize,
    /// Index of the lowest bitmap word that may have a bit set.
    bitmap_hint: usize,
    /// Whole pages between `lo` and `top`.
    pub(crate) pages: usize,
    /// The lowest address a large block may start at (16-aligned).
    floor: usize,
    /// The highest address a page may end at.
    page_limit: usize,
    /// Sum of free block sizes, headers included.
    pub(crate) free_bytes: usize,
    /// Number of free blocks.
    pub(crate) free_blocks: usize,
    /// Sum of the sizes free blocks are filed under (the bytes of them a
    /// large block may use, headers included).
    pub(crate) filed_bytes: usize,
    /// Number of free blocks filed for the large side.
    pub(crate) filed_blocks: usize,
    /// Whole free pages the small side could take (below the page limit).
    pub(crate) free_pages: usize,
    /// Sum of used large block sizes, headers included (pages not counted).
    pub(crate) used_bytes: usize,
    /// Number of used large blocks.
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
            words: 0,
            bitmap: 0,
            bitmap_hint: 0,
            pages: 0,
            floor: 0,
            page_limit: 0,
            free_bytes: 0,
            free_blocks: 0,
            filed_bytes: 0,
            filed_blocks: 0,
            free_pages: 0,
            used_bytes: 0,
            used_blocks: 0,
        }
    }

    /// Table bytes the pool needs for `pages` pages (word-aligned).
    pub(crate) const fn table_bytes(pages: usize) -> usize {
        pages * WORD + pages.div_ceil(WORD_BITS) * WORD
    }

    /// Sets up a pool that is one free block from `lo` (page-aligned) to the
    /// sentinel header at `top`, with its per-page table at `table`
    /// ([`Self::table_bytes`] for `pages`), bounded by `lines`.
    ///
    /// # Safety
    ///
    /// `[lo, top + HEADER)` and the table must be inside the region, owned by
    /// the heap and not overlap; `top` must be 16-aligned, `table`
    /// word-aligned, `top - lo` at least [`MIN_BLOCK`] and `pages` at most
    /// `(top - lo) / PAGE`.
    pub(crate) unsafe fn init(
        &mut self,
        mem: Mem,
        table: usize,
        pages: usize,
        (lo, top): (usize, usize),
        lines: Lines,
    ) {
        *self = Self::new();
        self.lo = lo;
        self.top = top;
        self.words = table;
        self.bitmap = table + pages * WORD;
        self.pages = pages;
        self.floor = lines.floor;
        self.page_limit = lines.page_limit;
        // SAFETY: the caller hands the table, the pool and the sentinel's
        // header to the pool.
        unsafe {
            for page in 0..pages {
                mem.set_word(self.words + page * WORD, 0);
            }
            for word in 0..pages.div_ceil(WORD_BITS) {
                mem.set_word(self.bitmap + word * WORD, 0);
            }
            mem.set_word(top + PREV_OFF, 0);
            mem.set_word(top + SIZE_OFF, 0);
            self.insert(mem, lo, top - lo);
            self.set_prev(mem, top, Some(lo));
        }
    }

    /// Moves the two lines (see [`Self::init`]). Nothing moves in the region:
    /// every free block is filed again under the new lines, which walks the
    /// region once.
    pub(crate) fn reconfigure(&mut self, mem: Mem, lines: Lines) {
        // SAFETY: the walk visits the pool's free blocks; `remove` leaves
        // their headers as they are and `insert` writes the same sizes back.
        unsafe {
            self.walk_free(mem, |pool, block| pool.remove(mem, block));
            self.floor = lines.floor;
            self.page_limit = lines.page_limit;
            self.walk_free(mem, |pool, block| {
                let size = Self::size(mem, block);
                pool.insert(mem, block, size);
            });
        }
    }

    /// Calls `f` with every free block, lowest first.
    ///
    /// # Safety
    /// `f` must not change any block's size or split or merge blocks.
    unsafe fn walk_free(&mut self, mem: Mem, mut f: impl FnMut(&mut Self, usize)) {
        let mut block = self.lo;
        while block < self.top {
            // SAFETY: the pool's blocks tile `[lo, top)`, so `block` is always
            // a page or a block header.
            unsafe {
                if self.is_page(mem, block) {
                    block += PAGE;
                    continue;
                }
                let size = Self::size(mem, block);
                if Self::flags(mem, block) & FREE != 0 {
                    f(self, block);
                }
                block += size;
            }
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

    /// Address of the word of the page at `page`.
    #[inline]
    fn word_of(&self, page: usize) -> usize {
        self.words + (page - self.lo) / PAGE * WORD
    }

    /// Whether a slab page block starts at `addr`.
    #[inline]
    pub(crate) fn is_page(&self, mem: Mem, addr: usize) -> bool {
        addr & (PAGE - 1) == 0
            && addr.wrapping_sub(self.lo) < self.pages * PAGE
            // SAFETY: a page of the pool has its word in the pool's table (an
            // uninitialized pool has no pages).
            && unsafe { mem.word(self.word_of(addr)) } & PAGE_BLOCK != 0
    }

    /// Whether the block at `block` (a block, a page or the sentinel) is free.
    ///
    /// # Safety
    /// `block` must be a block, a page or the sentinel of this pool.
    #[inline]
    unsafe fn is_free(&self, mem: Mem, block: usize) -> bool {
        // SAFETY: a block that is not a page has an in-region header.
        !self.is_page(mem, block) && unsafe { Self::flags(mem, block) & FREE != 0 }
    }

    /// Records on `block` (a block, a page or the sentinel) whether the block
    /// below it is free, and where.
    ///
    /// # Safety
    /// `block` must be a block, a page or the sentinel of this pool.
    #[inline]
    unsafe fn set_prev(&self, mem: Mem, block: usize, prev: Option<usize>) {
        // SAFETY: a page's header is its word in the table; any other block
        // has an in-region header.
        unsafe {
            if self.is_page(mem, block) {
                mem.set_word(self.word_of(block), prev.unwrap_or(0) | PAGE_BLOCK);
                return;
            }
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

    // ---- filing ---------------------------------------------------------

    /// The lowest address a large block may start at in the free block at
    /// `block`: `block` itself above the floor, else the floor, leaving a
    /// front free block of at least [`MIN_BLOCK`] bytes. A block starts there
    /// or at least [`MIN_BLOCK`] bytes higher, so what it leaves below is
    /// nothing or a whole free block (and the floor's pages stay whole).
    #[inline]
    fn usable_start(&self, block: usize) -> usize {
        if block >= self.floor {
            block
        } else {
            self.floor.max(block + MIN_BLOCK)
        }
    }

    /// The size the free block `[block, block + size)` is filed under: the
    /// bytes of it a large block may use, or 0 if it cannot hold one.
    #[inline]
    fn filed(&self, block: usize, size: usize) -> usize {
        let start = self.usable_start(block);
        let end = block + size;
        if end >= start + MIN_BLOCK {
            end - start
        } else {
            0
        }
    }

    /// The pages the free block `[block, block + size)` could give the small
    /// side: the first one's address, and how many end at or below the page
    /// limit.
    fn pages_of(&self, block: usize, size: usize) -> Option<(usize, usize)> {
        let end = block + size;
        let mut first = align_up(block, PAGE)?;
        if first != block && first - block < MIN_BLOCK {
            first = first.checked_add(PAGE)?;
        }
        let mut last = align_down(end.checked_sub(PAGE)?, PAGE);
        let tail = end.checked_sub(last + PAGE)?;
        if tail != 0 && tail < MIN_BLOCK {
            last = last.checked_sub(PAGE)?;
        }
        if last < first {
            return None;
        }
        let below = match self.page_limit.checked_sub(PAGE) {
            Some(limit) if limit >= first => (align_down(limit, PAGE).min(last) - first) / PAGE + 1,
            _ => 0,
        };
        Some((first, below))
    }

    // ---- free lists -----------------------------------------------------

    /// Files `block` as free, `size` bytes long.
    ///
    /// # Safety
    /// `block` must be a free block of `size` bytes, between used blocks (or
    /// the pool's ends), that is on no list.
    unsafe fn insert(&mut self, mem: Mem, block: usize, size: usize) {
        // SAFETY: a free block's header is in the region; its links live in
        // its payload; `head` is a free block; a first page lies inside the
        // block at least `MIN_BLOCK` bytes above its header and links.
        unsafe {
            Self::set_header(mem, block, size, FREE);
            self.free_bytes += size;
            self.free_blocks += 1;
            let filed = self.filed(block, size);
            if filed != 0 {
                let (fl, sl) = mapping_insert(filed);
                let head = self.heads[fl][sl];
                mem.set_word(block + NEXT_FREE_OFF, head);
                mem.set_word(block + PREV_FREE_OFF, 0);
                if head != 0 {
                    mem.set_word(head + PREV_FREE_OFF, block);
                }
                self.heads[fl][sl] = block;
                self.fl_bitmap |= 1 << fl;
                self.sl_bitmap[fl] |= 1 << sl;
                self.filed_bytes += filed;
                self.filed_blocks += 1;
            }
            if let Some((page, below)) = self.pages_of(block, size) {
                mem.set_word(self.word_of(page), block);
                self.mark(mem, page, true);
                self.free_pages += below;
            }
        }
    }

    /// Takes `block` off the free lists; its header stays as it is.
    ///
    /// # Safety
    /// `block` must be a free block filed by [`Self::insert`] under the
    /// current lines.
    unsafe fn remove(&mut self, mem: Mem, block: usize) {
        // SAFETY: `block` is free, so its header and (if filed) its links are
        // valid; neighbours on a free list are free blocks.
        unsafe {
            let size = Self::size(mem, block);
            let filed = self.filed(block, size);
            if filed != 0 {
                let (next, prev) = (
                    mem.word(block + NEXT_FREE_OFF),
                    mem.word(block + PREV_FREE_OFF),
                );
                let (fl, sl) = mapping_insert(filed);
                if next != 0 {
                    mem.set_word(next + PREV_FREE_OFF, prev);
                }
                if prev != 0 {
                    mem.set_word(prev + NEXT_FREE_OFF, next);
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
                self.filed_bytes -= filed;
                self.filed_blocks -= 1;
            }
            self.free_bytes -= size;
            self.free_blocks -= 1;
            if let Some((page, below)) = self.pages_of(block, size) {
                self.mark(mem, page, false);
                self.free_pages -= below;
            }
        }
    }

    /// Sets or clears the first-page mark of the page at `page`.
    ///
    /// # Safety
    /// `page` must be a page of this pool.
    unsafe fn mark(&mut self, mem: Mem, page: usize, on: bool) {
        let index = (page - self.lo) / PAGE;
        let word = index / WORD_BITS;
        let bit = 1usize << (index % WORD_BITS);
        let addr = self.bitmap + word * WORD;
        // SAFETY: the word is inside the bitmap.
        unsafe {
            let value = mem.word(addr);
            mem.set_word(addr, if on { value | bit } else { value & !bit });
        }
        if on {
            self.bitmap_hint = self.bitmap_hint.min(word);
        }
    }

    /// The lowest marked page: the lowest page any free block could give.
    fn lowest_page(&mut self, mem: Mem) -> Option<usize> {
        let words = self.pages.div_ceil(WORD_BITS);
        while self.bitmap_hint < words {
            // SAFETY: the word is inside the bitmap.
            let word = unsafe { mem.word(self.bitmap + self.bitmap_hint * WORD) };
            if word != 0 {
                let index = self.bitmap_hint * WORD_BITS + word.trailing_zeros() as usize;
                return Some(self.lo + index * PAGE);
            }
            self.bitmap_hint += 1;
        }
        None
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

    /// The head of the highest non-empty list, if it holds `need` bytes: a
    /// block of the largest size class, found in O(1).
    fn largest_head(&self, mem: Mem, need: usize) -> Option<usize> {
        if self.fl_bitmap == 0 {
            return None;
        }
        let fl = fls(self.fl_bitmap) as usize;
        let block = self.heads[fl][fls(self.sl_bitmap[fl] as usize) as usize];
        // SAFETY: blocks on a free list are free blocks of this pool.
        let size = unsafe { Self::size(mem, block) };
        (self.filed(block, size) >= need).then_some(block)
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
                if self.filed(block, Self::size(mem, block)) >= need {
                    return Some(block);
                }
                block = mem.word(block + NEXT_FREE_OFF);
            }
        }
        None
    }

    // ---- allocation -----------------------------------------------------

    /// Allocates `size` payload bytes aligned to `align` (a power of two) at
    /// or above the floor. A long-lived block takes the top of a free block
    /// of the largest size class, so it packs against the blocks above the
    /// largest free run and leaves holes to transients; any other block
    /// takes the top of the best-fitting free block. Returns the payload
    /// address.
    pub(crate) fn alloc(
        &mut self,
        mem: Mem,
        size: usize,
        align: usize,
        long: bool,
    ) -> Option<usize> {
        let size = align_up(size.max(GRANULE), GRANULE)?;
        let align = align.max(GRANULE);
        let payload = if align <= GRANULE {
            size
        } else {
            size.checked_add(align + MIN_BLOCK - GRANULE)?
        };
        let need = payload.checked_add(HEADER)?;
        let block = if long {
            self.largest_head(mem, need)
        } else {
            None
        };
        let block = block
            .or_else(|| self.good_fit(need))
            .or_else(|| self.scan(mem, need))?;
        // SAFETY: a filed free block with room for `size` bytes at `align`
        // above the floor.
        Some(unsafe { self.place(mem, block, size, align, long) })
    }

    /// Carves an allocation out of the top of the free `block`, at or above
    /// the floor. Leftovers become free blocks.
    ///
    /// # Safety
    /// `block` must be a filed free block whose filed size holds `size`
    /// bytes at `align`, including the padding that makes a front split
    /// possible.
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
        let lowest = self.usable_start(block);
        let first = lowest + HEADER;
        let low = {
            let p = align_down(first + align - 1, align);
            if p != first && p - first < MIN_BLOCK {
                align_down(first + MIN_BLOCK + align - 1, align)
            } else {
                p
            }
        };
        let high = align_down(end - size, align);
        let above = high - HEADER - lowest;
        let payload = if above != 0 && above < MIN_BLOCK {
            low
        } else {
            high
        };
        let start = payload - HEADER;
        let front = start - block;
        let mut used_end = payload + size;
        if end - used_end < MIN_BLOCK {
            used_end = end;
        }
        let long_flag = if long { LONG } else { 0 };
        // SAFETY: every address written below lies inside the removed free
        // block, or is the header of the block (or page) that follows it.
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
                self.set_prev(mem, end, Some(used_end));
            } else {
                self.set_prev(mem, end, None);
            }
        }
        self.used_bytes += used_end - start;
        self.used_blocks += 1;
        payload
    }

    /// Frees the block whose payload is at `payload`, coalescing at once with
    /// free neighbours.
    ///
    /// # Safety
    /// `payload` must be the payload of a used block of this pool.
    pub(crate) unsafe fn free(&mut self, mem: Mem, payload: usize) {
        let mut block = payload - HEADER;
        // SAFETY: `block` is a used block; neighbours are read through valid
        // headers (the sentinel bounds the top, `PREV_FREE` guards the bottom,
        // a page above is recognized by its word).
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
            if self.is_free(mem, next) {
                size += Self::size(mem, next);
                self.remove(mem, next);
            }
            self.insert(mem, block, size);
            self.set_prev(mem, block + size, Some(block));
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
        // SAFETY: `block` is a used block; its upper neighbour is a block, a
        // page or the sentinel.
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
            if !self.is_free(mem, next) {
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
                self.set_prev(mem, end, Some(tail));
                self.used_bytes += need - current;
            } else {
                Self::set_header(mem, block, total, flags);
                self.set_prev(mem, end, None);
                self.used_bytes += total - current;
            }
            true
        }
    }

    /// Grows a block downward into its free lower neighbour when it cannot
    /// grow in place: that neighbour (above the floor), the block itself and
    /// a free upper neighbour become one block, placed as high as `new_size`
    /// bytes at `align` allow, and the first `keep` payload bytes move down
    /// with it. A buffer that keeps growing this way (such as a doubling
    /// vector at the top of a free block) reuses the room of its earlier
    /// sizes instead of stranding it above itself. Returns the new payload
    /// address, or `None` (nothing changed) if the block has no free lower
    /// neighbour or the room does not suffice.
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
    ) -> Option<usize> {
        let block = payload - HEADER;
        let size = align_up(new_size.max(GRANULE), GRANULE)?;
        let align = align.max(GRANULE);
        // SAFETY: `block` is a used block; its lower neighbour is free (its
        // address is in the header), its upper neighbour is a block, a page
        // or the sentinel, and every address written lies in those blocks.
        unsafe {
            let flags = Self::flags(mem, block);
            if flags & PREV_FREE == 0 {
                return None;
            }
            let prev = mem.word(block + PREV_OFF);
            let current = Self::size(mem, block);
            let next = block + current;
            let next_free = self.is_free(mem, next);
            let end = if next_free {
                next + Self::size(mem, next)
            } else {
                next
            };
            let to = align_down(end.checked_sub(size)?, align);
            let start = to.checked_sub(HEADER)?;
            let lowest = self.usable_start(prev);
            if start < lowest || start > block {
                return None;
            }
            let above = start - lowest;
            if above != 0 && above < MIN_BLOCK {
                return None;
            }
            let front = start - prev;
            self.remove(mem, prev);
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
            if front > 0 {
                self.insert(mem, prev, front);
                Self::set_header(mem, start, used_end - start, PREV_FREE | (flags & LONG));
                mem.set_word(start + PREV_OFF, prev);
            } else {
                Self::set_header(mem, start, used_end - start, flags & LONG);
            }
            if used_end < end {
                self.insert(mem, used_end, end - used_end);
                self.set_prev(mem, end, Some(used_end));
            } else {
                self.set_prev(mem, end, None);
            }
            self.used_bytes = self.used_bytes - current + (used_end - start);
            Some(to)
        }
    }

    // ---- pages ----------------------------------------------------------

    /// Takes the lowest free page that ends at or below the page limit as a
    /// slab page and returns its address.
    pub(crate) fn take_page(&mut self, mem: Mem) -> Option<usize> {
        let page = self.lowest_page(mem)?;
        if page + PAGE > self.page_limit {
            return None;
        }
        // SAFETY: a marked page's word names the free block it lies in, at
        // least `MIN_BLOCK` bytes above that block's start (or at it) and at
        // least `MIN_BLOCK` bytes below its end (or at it); every address
        // written lies in that block, in the page's word or in the header of
        // the block (or page) after it.
        unsafe {
            let block = mem.word(self.word_of(page));
            let end = block + Self::size(mem, block);
            self.remove(mem, block);
            let prev = if page > block {
                self.insert(mem, block, page - block);
                block
            } else {
                0
            };
            mem.set_word(self.word_of(page), prev | PAGE_BLOCK);
            let after = page + PAGE;
            if end > after {
                self.insert(mem, after, end - after);
                self.set_prev(mem, end, Some(after));
            } else {
                self.set_prev(mem, end, None);
            }
        }
        Some(page)
    }

    /// Gives the slab page at `page` back, coalescing it at once with free
    /// neighbours.
    ///
    /// # Safety
    /// `page` must be a slab page taken with [`Self::take_page`] that holds
    /// no live object.
    pub(crate) unsafe fn free_page(&mut self, mem: Mem, page: usize) {
        // SAFETY: the page's word holds its lower neighbour; the block above
        // it is a block, a page or the sentinel; the merged block lies in the
        // page and its free neighbours.
        unsafe {
            let word = self.word_of(page);
            let prev = mem.word(word) & !PAGE_BLOCK;
            mem.set_word(word, 0);
            let (mut block, mut size) = (page, PAGE);
            if prev != 0 {
                self.remove(mem, prev);
                size += page - prev;
                block = prev;
            }
            let next = page + PAGE;
            if self.is_free(mem, next) {
                size += Self::size(mem, next);
                self.remove(mem, next);
            }
            self.insert(mem, block, size);
            self.set_prev(mem, block + size, Some(block));
        }
    }

    // ---- queries --------------------------------------------------------

    /// The largest filed size of a free block (headers included), or 0.
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
                best = best.max(self.filed(block, Self::size(mem, block)));
                block = mem.word(block + NEXT_FREE_OFF);
            }
        }
        best
    }

    /// Payload bytes the filed free blocks could hand out, one allocation
    /// each.
    pub(crate) fn free_payload(&self) -> usize {
        self.filed_bytes - self.filed_blocks * HEADER
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

    #[test]
    fn pages_of_keeps_whole_free_blocks_on_both_sides() {
        let mut pool = Tlsf::new();
        let lo = 16 * PAGE;
        pool.lo = lo;
        pool.pages = 64;
        pool.page_limit = lo + 64 * PAGE;
        // A block that starts on a page and ends on one gives every page.
        assert_eq!(pool.pages_of(lo, 4 * PAGE), Some((lo, 4)));
        // Leftovers below MIN_BLOCK on either side are not split off.
        assert_eq!(pool.pages_of(lo - 16, PAGE + 16), None);
        assert_eq!(pool.pages_of(lo - 16, 2 * PAGE + 16), Some((lo + PAGE, 1)));
        assert_eq!(pool.pages_of(lo, PAGE + 16), None);
        assert_eq!(pool.pages_of(lo, 2 * PAGE + 16), Some((lo, 1)));
        assert_eq!(pool.pages_of(lo - 32, PAGE + 64), Some((lo, 1)));
        assert_eq!(pool.pages_of(lo + 32, PAGE), None);
        // Only pages ending at or below the limit count.
        pool.page_limit = lo + 2 * PAGE + 100;
        assert_eq!(pool.pages_of(lo, 4 * PAGE), Some((lo, 2)));
        pool.page_limit = lo + PAGE - 1;
        assert_eq!(pool.pages_of(lo, 4 * PAGE), Some((lo, 0)));
    }
}
