//! The small side: size-class slab pages.
//!
//! Pages are 64 KiB and 64 KiB-aligned, so an object at `page + i * class`
//! is aligned to the largest power of two dividing its class. A page is a
//! block of the pool (`tlsf.rs`): the slab takes the lowest free page
//! in the region when a class has no page with room, and gives a page back
//! to the pool as soon as its last object is freed, wherever it lies, where
//! it coalesces with the free blocks around it. Page metadata lives in a
//! table at the start of the region, one 16-byte entry per page the region
//! could ever hold. Free objects keep their in-page free list in their own
//! first four bytes.

#![allow(unsafe_code)]

use crate::raw::Mem;
use crate::tlsf::Tlsf;

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

// Page-table entry layout (four u32 words), valid while the page is a slab
// page.
const E_CLASS: usize = 0; // class index
const E_COUNTS: usize = 4; // used (low 16 bits) | carved (high 16 bits)
const E_FREE: usize = 8; // in-page offset + 1 of the first free object, 0 = none
const E_LINK: usize = 12; // next page index (partial lists); the prev index lives in the page table's second half

const WORD: usize = size_of::<usize>();

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
    /// Address of the page table (entries, then prev links).
    table: usize,
    /// Number of table entries (the most pages the region could hold).
    capacity: u32,
    /// Address of page 0.
    pub(crate) lo: usize,
    /// Pages the slab holds.
    pub(crate) pages: u32,
    partial: [u32; CLASS_COUNT],
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
            capacity: 0,
            lo: 0,
            pages: 0,
            partial: [NONE; CLASS_COUNT],
            live_bytes: 0,
            objects: [0; CLASS_COUNT],
            class_pages: [0; CLASS_COUNT],
        }
    }

    /// Table bytes needed for `capacity` pages (`table` must be 16-aligned);
    /// a multiple of the word size.
    pub(crate) const fn table_bytes(capacity: usize) -> usize {
        (capacity * (ENTRY + 4)).next_multiple_of(WORD)
    }

    /// Sets up an empty slab side with its table at `table` (16-aligned,
    /// [`Self::table_bytes`] long, inside the region) and page 0 at `lo`.
    pub(crate) fn init(&mut self, table: usize, capacity: u32, lo: usize) {
        *self = Self::new();
        self.table = table;
        self.capacity = capacity;
        self.lo = lo;
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

    /// Allocates one object of `class`, taking the pool's lowest free page
    /// when no page of the class has room.
    pub(crate) fn alloc(&mut self, mem: Mem, pool: &mut Tlsf, class: usize) -> Option<usize> {
        let mut page = self.partial[class];
        if page == NONE {
            page = ((pool.take_page(mem)? - self.lo) / PAGE) as u32;
            // SAFETY: `page` was just taken and is on no list.
            unsafe {
                let entry = self.entry(page);
                mem.set_u32(entry + E_CLASS, class as u32);
                mem.set_u32(entry + E_COUNTS, 0);
                mem.set_u32(entry + E_FREE, 0);
                self.push(mem, class, page);
            }
            self.pages += 1;
            self.class_pages[class] += 1;
        }
        let entry = self.entry(page);
        let base = self.page_addr(page);
        // SAFETY: `page` is a slab page of `class` with a free object.
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

    /// Frees the object at `addr`, giving its page back to the pool when it
    /// was the page's last object.
    ///
    /// # Safety
    /// `addr` must be a live object of this slab side.
    pub(crate) unsafe fn free(&mut self, mem: Mem, pool: &mut Tlsf, addr: usize) {
        let page = ((addr - self.lo) / PAGE) as u32;
        let entry = self.entry(page);
        let base = self.page_addr(page);
        // SAFETY: `addr` is a live object of a slab page; a page with no live
        // object left goes back to the pool it came from.
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
                self.pages -= 1;
                pool.free_page(mem, base);
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
