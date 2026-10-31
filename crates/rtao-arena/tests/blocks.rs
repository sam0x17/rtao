//! Block, FixedVec and KeyBuf over a heap and over the global allocator.

#![cfg(feature = "std")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::Cell;
use std::rc::Rc;

use common::heap;
use rtao_arena::{Block, FixedVec, KeyBuf, Lifetime, Scope, is_declared};

const MIB: usize = 1 << 20;

#[test]
fn blocks_are_exact_zeroed_and_released() {
    let heap = heap(16 * MIB, 0, 0);
    let short = Block::try_new_zeroed_in(MIB, 16, Lifetime::Short, heap).unwrap();
    let mut long = Block::try_new_zeroed_in(MIB, 4096, Lifetime::Long, heap).unwrap();
    assert!(long.as_ptr() < short.as_ptr(), "long blocks sit lower");
    assert_eq!(long.as_ptr().addr() % 4096, 0);
    assert!(long.iter().all(|&b| b == 0));
    long[..3].copy_from_slice(b"abc");
    assert_eq!(&long[..3], b"abc");
    assert_eq!((long.len(), long.align()), (MIB, 4096));
    assert!(Block::try_new_zeroed_in(64 * MIB, 16, Lifetime::Short, heap).is_err());
    let empty = Block::try_new_zeroed_in(0, 64, Lifetime::Short, heap).unwrap();
    assert!(empty.is_empty());
    let live = heap.stats().large.live;
    drop(short);
    drop(long);
    drop(empty);
    assert!(heap.stats().large.live < live);
    assert_eq!(heap.stats().large.live, 0);
    // The global source works with any global allocator.
    let global = Block::try_new_zeroed(1000, Lifetime::Short).unwrap();
    assert_eq!(global.len(), 1000);
}

struct Counted(Rc<Cell<usize>>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn fixed_vecs_never_grow_and_drop_their_elements() {
    let heap = heap(8 * MIB, 0, 0);
    let drops = Rc::new(Cell::new(0));
    let mut v = FixedVec::try_with_capacity_in(3, Lifetime::Short, heap).unwrap();
    for _ in 0..3 {
        assert!(v.try_push(Counted(drops.clone())).is_ok());
    }
    assert!(v.is_full());
    let rejected = v.try_push(Counted(drops.clone())).err().unwrap();
    drop(rejected);
    assert_eq!(drops.get(), 1);
    drop(v.pop());
    assert_eq!(drops.get(), 2);
    v.truncate(1);
    assert_eq!(drops.get(), 3);
    drop(v);
    assert_eq!(drops.get(), 4);
    assert_eq!(heap.stats().small.live, 0);

    let mut nums: FixedVec<u32, _> =
        FixedVec::try_with_capacity_in(4, Lifetime::Long, heap).unwrap();
    nums.try_extend_from_slice(&[1, 2, 3]).unwrap();
    assert!(nums.try_extend_from_slice(&[4, 5]).is_err());
    assert_eq!(nums.as_slice(), &[1, 2, 3]);
    nums[0] = 9;
    assert_eq!(nums.iter().sum::<u32>(), 14);
    assert_eq!(format!("{nums:?}"), "[9, 2, 3]");

    let mut units: FixedVec<(), _> =
        FixedVec::try_with_capacity_in(2, Lifetime::Short, heap).unwrap();
    assert!(units.try_push(()).is_ok() && units.try_push(()).is_ok());
    assert!(units.try_push(()).is_err());
    assert!(FixedVec::<u64, _>::try_with_capacity_in(usize::MAX, Lifetime::Short, heap).is_err());

    // The global source marks its allocations as declared.
    let mut seen = FixedVec::<bool>::try_with_capacity(1).unwrap();
    assert!(seen.try_push(is_declared()).is_ok());
    assert!(!seen[0], "the marker is cleared after the allocation");
}

#[test]
fn explicit_heap_blocks_ignore_an_active_scope() {
    let heap = heap(32 * MIB, 0, 0);
    let scope = Scope::new("blocks", MIB, 64 << 10);
    // Admitted against the large side, then taken inside a smaller scope: the
    // scope's cap does not apply, the hint does.
    assert!(heap.can_serve(2 * MIB, 2 * MIB));
    let (big, long, vec) = scope.run(|| {
        (
            Block::try_new_zeroed_in(2 * MIB, 16, Lifetime::Long, heap),
            Block::try_new_zeroed_in(256 << 10, 16, Lifetime::Long, heap).unwrap(),
            FixedVec::<u64, _>::try_with_capacity_in(100, Lifetime::Short, heap).unwrap(),
        )
    });
    let big = big.expect("an explicit-heap block is not refused by the scope cap");
    let stats = scope.stats();
    assert_eq!((stats.objects, stats.cap_refusals), (0, 0));
    assert!(heap.stats().large.live >= 2 * MIB + (256 << 10));
    // Long blocks pack low: the later one sits below the earlier one.
    assert!(long.as_ptr() < big.as_ptr());
    // Blocks that outlive the scope leave nothing parked.
    let ended = scope.finish();
    assert_eq!((ended.parked, ended.live), (0, 0));
    assert_eq!(heap.stats().parked_chunks, 0);
    drop((big, long, vec));
    assert_eq!(heap.stats().large.live, 0);
}

#[test]
fn key_buffers_build_keys_without_allocating() {
    use core::fmt::Write;
    let mut key = KeyBuf::<64>::new();
    write!(key, "table/{}/row/", 7).unwrap();
    key.push_u64(12).unwrap();
    assert_eq!(key.as_str(), Some("table/7/row/12"));
    let copy = key;
    assert_eq!(copy, key);
    key.clear();
    key.push_hex(&[0xde, 0xad]).unwrap();
    assert_eq!(&*key, b"dead");
    assert_eq!(format!("{key:?}"), "\"dead\"");
}
