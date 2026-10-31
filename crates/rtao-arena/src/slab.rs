//! The small side: size-class slab pages that grow up from the bottom.
//!
//! Pages are 64 KiB and 64 KiB-aligned, so an object at `page + i * class`
//! is aligned to the largest power of two dividing its class. Page metadata
//! lives in a table at the start of the region, one 16-byte entry per page
//! the region could ever hold. Free objects keep their in-page free list in
//! their own first four bytes. A fully free page is marked in a free-page
//! bitmap (after the table) that any class reuses lowest page first, so live
//! objects settle low and the top drains; when a free page is the topmost
//! page it is handed back to the gap, together with any free pages directly
//! below it.

#![allow(unsafe_code)]

use crate::raw::Mem;

/// Slab page size.
pub(crate) const PAGE: usize = 64 * 1024;
/// Object sizes of the slab classes.
pub(crate) const CLASSES: [usize; 14] = [
    16, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048,
];
/// Number of slab classes.
pub(crate) const CLASS_COUNT: usize = CLASSES.len();
/// The largest object the small side serves.
pub(crate) const MAX_SMALL: usize = 2048;
/// Bytes of one page-table entry.
pub(crate) const ENTRY: usize = 16;

const NONE: u32 = u32::MAX;
const FREE_PAGE: u32 = u32::MAX;

// Page-table entry layout (four u32 words).
const E_CLASS: usize = 0; // class index, or FREE_PAGE
const E_COUNTS: usize = 4; // used (low 16 bits) | carved (high 16 bits)
const E_FREE: usize = 8; // in-page offset + 1 of the first free object, 0 = none
const E_LINK: usize = 12; // next page index (partial lists); the prev index lives in the page table's second half

const WORD: usize = size_of::<usize>();
const WORD_BITS: usize = usize::BITS as usize;

/// Natural alignment of a class: the largest power of two dividing its size.
#[inline]
pub(crate) const fn natural_align(class: usize) -> usize {
    let size = CLASSES[class];
    size & size.wrapping_neg()
}

/// The class for `size` bytes at `align`, or `None` if the small side cannot
/// serve that layout.
#[inline]
pub(crate) fn class_for(size: usize, align: usize) -> Option<usize> {
    if size > MAX_SMALL || align > MAX_SMALL {
        return None;
    }
    let mut class = CLASS_OF_GRANULE[size.max(1).div_ceil(16)] as usize;
    while class < CLASS_COUNT {
        if natural_align(class) >= align {
            return Some(class);
        }
        class += 1;
    }
    None
}

/// Smallest class index whose size is at least `16 * index`.
const CLASS_OF_GRANULE: [u8; MAX_SMALL / 16 + 1] = {
    let mut table = [0u8; MAX_SMALL / 16 + 1];
    let mut i = 0;
    while i < table.len() {
        let mut class = 0;
        while CLASSES[class] < i * 16 {
            class += 1;
        }
        table[i] = class as u8;
        i += 1;
    }
    table
};

/// Objects one page of `class` holds.
#[inline]
pub(crate) const fn per_page(class: usize) -> usize {
    PAGE / CLASSES[class]
}

/// The slab side's control structure; page metadata lives in the region.
pub(crate) struct Slab {
    /// Address of the page table (entries, then prev links, then the
    /// free-page bitmap).
    table: usize,
    /// Address of the free-page bitmap (one bit per page, set = free page
    /// waiting for reuse).
    bitmap: usize,
    /// Index of the lowest bitmap word that may have a bit set.
    bitmap_hint: usize,
    /// Number of table entries (the most pages the region could hold).
    capacity: u32,
    /// Address of page 0.
    pub(crate) lo: usize,
    /// Pages currently claimed from the gap.
    pub(crate) pages: u32,
    partial: [u32; CLASS_COUNT],
    /// Free pages waiting for reuse (not counting returned pages).
    pub(crate) free_pages: u32,
    /// Live bytes, counted at class size.
    pub(crate) live_bytes: usize,
    /// Live objects per class.
    pub(crate) objects: [usize; CLASS_COUNT],
    /// Pages held per class.
    pub(crate) class_pages: [u32; CLASS_COUNT],
}

impl Slab {
    pub(crate) const fn new() -> Self {
        Self {
            table: 0,
            bitmap: 0,
            bitmap_hint: 0,
            capacity: 0,
            lo: 0,
            pages: 0,
            partial: [NONE; CLASS_COUNT],
            free_pages: 0,
            live_bytes: 0,
            objects: [0; CLASS_COUNT],
            class_pages: [0; CLASS_COUNT],
        }
    }

    /// Offset of the free-page bitmap from the table's start.
    const fn bitmap_offset(capacity: usize) -> usize {
        (capacity * (ENTRY + 4)).next_multiple_of(WORD)
    }

    /// Table bytes needed for `capacity` pages (`table` must be 16-aligned).
    pub(crate) const fn table_bytes(capacity: usize) -> usize {
        Self::bitmap_offset(capacity) + capacity.div_ceil(WORD_BITS) * WORD
    }

    /// Sets up an empty slab side with its table at `table` (16-aligned,
    /// [`Self::table_bytes`] long, inside the region) and page 0 at `lo`.
    pub(crate) fn init(&mut self, mem: Mem, table: usize, capacity: u32, lo: usize) {
        *self = Self::new();
        self.table = table;
        self.bitmap = table + Self::bitmap_offset(capacity as usize);
        self.capacity = capacity;
        self.lo = lo;
        for word in 0..(capacity as usize).div_ceil(WORD_BITS) {
            // SAFETY: the bitmap lies inside the table the caller handed over.
            unsafe { mem.set_word(self.bitmap + word * WORD, 0) };
        }
    }

    /// Marks `page` free (`true`) or not in the free-page bitmap.
    ///
    /// # Safety
    /// `page` must be below `capacity`.
    unsafe fn mark_free(&mut self, mem: Mem, page: u32, free: bool) {
        let index = page as usize / WORD_BITS;
        let bit = 1usize << (page as usize % WORD_BITS);
        let addr = self.bitmap + index * WORD;
        // SAFETY: the word is inside the bitmap.
        unsafe {
            let word = mem.word(addr);
            mem.set_word(addr, if free { word | bit } else { word & !bit });
        }
        if free {
            self.bitmap_hint = self.bitmap_hint.min(index);
        }
    }

    /// End of the claimed pages.
    #[inline]
    pub(crate) fn hi(&self) -> usize {
        self.lo + self.pages as usize * PAGE
    }

    #[inline]
    fn entry(&self, page: u32) -> usize {
        self.table + page as usize * ENTRY
    }

    #[inline]
    fn prev_slot(&self, page: u32) -> usize {
        self.table + self.capacity as usize * ENTRY + page as usize * 4
    }

    #[inline]
    fn page_addr(&self, page: u32) -> usize {
        self.lo + page as usize * PAGE
    }

    // ---- doubly linked partial page lists, one per class ------------------

    /// # Safety
    /// `page` must be a claimed page that is on no list.
    unsafe fn push(&mut self, mem: Mem, class: usize, page: u32) {
        let head = self.partial[class];
        // SAFETY: table entries of claimed pages are inside the table.
        unsafe {
            mem.set_u32(self.entry(page) + E_LINK, head);
            mem.set_u32(self.prev_slot(page), NONE);
            if head != NONE {
                mem.set_u32(self.prev_slot(head), page);
            }
        }
        self.partial[class] = page;
    }

    /// # Safety
    /// `page` must be on the partial list of `class`.
    unsafe fn unlink(&mut self, mem: Mem, class: usize, page: u32) {
        // SAFETY: `page` and its neighbours are claimed pages on `list`.
        unsafe {
            let next = mem.u32(self.entry(page) + E_LINK);
            let prev = mem.u32(self.prev_slot(page));
            if next != NONE {
                mem.set_u32(self.prev_slot(next), prev);
            }
            if prev != NONE {
                mem.set_u32(self.entry(prev) + E_LINK, next);
            } else {
                self.partial[class] = next;
            }
        }
    }

    // ---- allocation -------------------------------------------------------

    /// Allocates one object of `class`, claiming a page below `limit` (the
    /// highest address the small side may reach) when no page has room.
    pub(crate) fn alloc(&mut self, mem: Mem, class: usize, limit: usize) -> Option<usize> {
        let mut page = self.partial[class];
        if page == NONE {
            page = self.acquire(mem, limit)?;
            // SAFETY: `page` was just claimed and is on no list.
            unsafe {
                let entry = self.entry(page);
                mem.set_u32(entry + E_CLASS, class as u32);
                mem.set_u32(entry + E_COUNTS, 0);
                mem.set_u32(entry + E_FREE, 0);
                self.push(mem, class, page);
            }
            self.class_pages[class] += 1;
        }
        let entry = self.entry(page);
        let base = self.page_addr(page);
        // SAFETY: `page` is a claimed page of `class` with a free object.
        let addr = unsafe {
            let counts = mem.u32(entry + E_COUNTS);
            let used = counts & 0xffff;
            let carved = counts >> 16;
            let free = mem.u32(entry + E_FREE);
            let (addr, carved) = if free != 0 {
                let addr = base + (free - 1) as usize;
                mem.set_u32(entry + E_FREE, mem.u32(addr));
                (addr, carved)
            } else {
                (base + carved as usize * CLASSES[class], carved + 1)
            };
            let used = used + 1;
            mem.set_u32(entry + E_COUNTS, used | (carved << 16));
            if used as usize == per_page(class) {
                self.unlink(mem, class, page);
            }
            addr
        };
        self.live_bytes += CLASSES[class];
        self.objects[class] += 1;
        Some(addr)
    }

    /// A free page: the lowest recycled one first, else a fresh one from the
    /// gap. Taking the lowest keeps live objects low, so pages near the top
    /// drain and return to the gap.
    fn acquire(&mut self, mem: Mem, limit: usize) -> Option<u32> {
        if self.free_pages > 0 {
            let words = (self.capacity as usize).div_ceil(WORD_BITS);
            let mut index = self.bitmap_hint;
            while index < words {
                // SAFETY: the word is inside the bitmap.
                let word = unsafe { mem.word(self.bitmap + index * WORD) };
                if word != 0 {
                    let page = (index * WORD_BITS + word.trailing_zeros() as usize) as u32;
                    self.bitmap_hint = index;
                    // SAFETY: a set bit names a page below `capacity`.
                    unsafe { self.mark_free(mem, page, false) };
                    self.free_pages -= 1;
                    return Some(page);
                }
                index += 1;
            }
            // Unreachable while `free_pages` counts the set bits.
            self.bitmap_hint = words;
        }
        if self.pages >= self.capacity || self.hi() + PAGE > limit {
            return None;
        }
        self.pages += 1;
        Some(self.pages - 1)
    }

    /// The class of the object at `addr`.
    ///
    /// # Safety
    /// `addr` must be a live object of this slab side.
    #[inline]
    pub(crate) unsafe fn class_of(&self, mem: Mem, addr: usize) -> usize {
        let page = ((addr - self.lo) / PAGE) as u32;
        // SAFETY: a live object's page is a claimed page with a class entry.
        unsafe { mem.u32(self.entry(page) + E_CLASS) as usize }
    }

    /// Frees the object at `addr`.
    ///
    /// # Safety
    /// `addr` must be a live object of this slab side.
    pub(crate) unsafe fn free(&mut self, mem: Mem, addr: usize) {
        let page = ((addr - self.lo) / PAGE) as u32;
        let entry = self.entry(page);
        let base = self.page_addr(page);
        // SAFETY: `addr` is a live object of a claimed page.
        unsafe {
            let class = mem.u32(entry + E_CLASS) as usize;
            mem.set_u32(addr, mem.u32(entry + E_FREE));
            mem.set_u32(entry + E_FREE, (addr - base + 1) as u32);
            let counts = mem.u32(entry + E_COUNTS);
            let used = counts & 0xffff;
            if used as usize == per_page(class) {
                self.push(mem, class, page);
            }
            let used = used - 1;
            mem.set_u32(entry + E_COUNTS, used | (counts & 0xffff_0000));
            self.live_bytes -= CLASSES[class];
            self.objects[class] -= 1;
            if used == 0 {
                self.unlink(mem, class, page);
                self.class_pages[class] -= 1;
                self.release(mem, page);
            }
        }
    }

    /// Recycles a fully free page, returning it to the gap when topmost.
    ///
    /// # Safety
    /// `page` must be a claimed page with no live object, on no list.
    unsafe fn release(&mut self, mem: Mem, page: u32) {
        // SAFETY: `page` and the pages below the top are claimed pages.
        unsafe {
            mem.set_u32(self.entry(page) + E_CLASS, FREE_PAGE);
            if page + 1 != self.pages {
                self.mark_free(mem, page, true);
                self.free_pages += 1;
                return;
            }
            self.pages -= 1;
            while self.pages > 0 && mem.u32(self.entry(self.pages - 1) + E_CLASS) == FREE_PAGE {
                self.mark_free(mem, self.pages - 1, false);
                self.free_pages -= 1;
                self.pages -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn classes_cover_every_small_layout() {
        assert_eq!(class_for(24, 32), Some(1));
        assert_eq!(class_for(40, 32), Some(3));
        assert_eq!(class_for(1, 1), Some(0));
        assert_eq!(class_for(2048, 2048), Some(13));
        assert_eq!(class_for(2049, 8), None);
        assert_eq!(class_for(8, 4096), None);
        for size in 1..=MAX_SMALL {
            for shift in 0..=11 {
                let align = 1 << shift;
                if let Some(class) = class_for(size, align) {
                    assert!(CLASSES[class] >= size);
                    assert!(natural_align(class) >= align);
                    if class > 0 {
                        let smaller = class - 1;
                        assert!(CLASSES[smaller] < size || natural_align(smaller) < align);
                    }
                } else {
                    assert!(align > 2048 || size.next_multiple_of(align) > 2048);
                }
            }
        }
        assert!(per_page(0) <= 0xffff);
    }
}
