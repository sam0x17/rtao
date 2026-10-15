# Fixed ruint with Rust 1.89 compatibility

The EVM dependency graph activated `ruint 1.17.2`, which is affected by
[RUSTSEC-2026-0220](https://rustsec.org/advisories/RUSTSEC-2026-0220.html).
Incorrect shift overflow flags can make checked shifts accept discarded bits.
The no-allocation `to_base_be` implementation can then loop while finding the
largest power of a radix. Separately, large shift amounts can truncate in
primitive fast paths. This is an active dependency, so the dormant-package
exceptions for lru and ruzstd do not apply.

The proposed replacement is the complete published `ruint 1.20.1` source with
one compatibility backport. It retains the fixes from
[upstream PR 603](https://github.com/alloy-rs/ruint/pull/603), including masked
top bits, discarded limbs, large shift amounts, pointer-width-aware big shifts,
and the big-endian `byte()` bounds check. It also retains the other upstream
correctness and safety fixes shipped through 1.20.1. The audit advisory is not
suppressed.

Upstream requires Rust 1.90. Its
[MSRV change, PR 570](https://github.com/alloy-rs/ruint/pull/570), replaced a
manual byte-reversal loop inside the constant `to_be_bytes` method with
`bytes.reverse()`. Reversal became usable in constant evaluation in Rust 1.90.
Our copy restores that previous loop and declares Rust 1.89 in the normalized
crate manifest. The optimized full-limb byte conversion and every other source
file remain identical to the published release.

## Source identity

The source is retained under [patches/ruint](../../patches/ruint). Its
[UPSTREAM.json](../../patches/ruint/UPSTREAM.json) records the original and local
SHA-256 of each retained file and the only two modified files.

| Input | Identity |
| --- | --- |
| Registry archive | `https://static.crates.io/crates/ruint/ruint-1.20.1.crate` |
| Archive bytes | `146696` |
| Registry/archive SHA-256 | `2973657b5127d510e230f5c63d2d106af9c8f79393d8b9f4647323e8196bdde5` |
| Upstream commit from `.cargo_vcs_info.json` | `8a242035914783682b9e6a4bc87e0d65221bff77` |
| Upstream MIT license blob | `62ed7829c605a9207b8388c3e45b3ecc31d6b987` |
| Published normalized `Cargo.toml` SHA-256 | `a3243fbb622adaf8fe670a1368863280dff1ec7d23e002e2c4916aedf30ad3c3` |
| Local `Cargo.toml` SHA-256 | `cf634bd6338f4aee9840a8e3845db571b7ddb63af65080dc451e6669816c895d` |
| Published `src/bytes.rs` SHA-256 | `eb5a317f703262134a332fe7bb38e131b25596cf0a1d6a6bd3a920756199f9c9` |
| Local `src/bytes.rs` SHA-256 | `644fc79b4df9abdf4465465a2c9b24b8ef0ff25f3d3e913e54f4af2cbcdb58c8` |

The registry index checksum and downloaded archive matched before source was
retained. The archive was parsed in memory with size, entry-type and path
checks. All 67 Rust source files are retained. The upstream README, normalized
manifest, original manifest, documentation header and VCS identity are also
retained. The MIT license was read from the pinned upstream tree and added
because the published archive omits that file. The upstream build aliases and
94 KB dependency lockfile are omitted. No archive, clone or build cache was
written locally. The retained upstream files occupy about 599 KB.

## Compatibility and regression checks

Source inspection checked the complete release's stable implementation,
including the arithmetic helpers, byte conversion, string parser, macros and
optional support modules. The newer integer `unbounded_shl`/`unbounded_shr`,
`is_multiple_of`, checked string splitting, mutable raw slices and const pointer
operations predate Rust 1.90. Big-integer intrinsics remain behind upstream's
existing nightly feature. The x86 subtraction intrinsic retains its upstream
unsafe wrapper for older compilers. This inspection is evidence for the narrow
backport; hosted compilation is required to establish Rust 1.89 compatibility.

The Rust 1.89.0 core sources confirm the relevant stability declarations:

| API | Stable const use |
| --- | --- |
| [`slice::as_ptr_range` and `as_mut_ptr_range`](https://github.com/rust-lang/rust/blob/1.89.0/library/core/src/slice/mod.rs) | Rust 1.61 |
| [`slice::as_flattened_mut`](https://github.com/rust-lang/rust/blob/1.89.0/library/core/src/slice/mod.rs) | Rust 1.87 |
| [`u64::is_multiple_of`, `unbounded_shl` and `unbounded_shr`](https://github.com/rust-lang/rust/blob/1.89.0/library/core/src/num/uint_macros.rs) | Rust 1.87 |
| [`usize::unchecked_add`](https://github.com/rust-lang/rust/blob/1.89.0/library/core/src/num/uint_macros.rs) | Rust 1.79 |
| [`str::split_at_checked`](https://github.com/rust-lang/rust/blob/1.89.0/library/core/src/str/mod.rs) | Rust 1.86 |

The same slice source marks const `reverse` as unstable in 1.89, matching the
specific compatibility change.

[ci/ruint-probe](../../ci/ruint-probe) is a separate workspace whose direct
dependency disables both `std` and `alloc`. Its source contains:

- Constant byte-order fixtures for odd/even partial limbs and zero/one bytes.
- Masked-top-bit and whole-limb shift overflow fixtures.
- Binary digit conversion at 65, 160, 161 and 255 bits, exercising the
  no-allocation constructor that could hang in the affected release.
- Decimal formatting into a bounded stack buffer.
- Public operators with integer shift amounts beyond 32-bit pointers and
  beyond `u64`, including shifts of zero.

The probe must run under an external CI timeout because the vulnerable
constructor can loop before returning an iterator. The timeout bounds a
regression failure; it does not make the assertion pass.

[The EVM integration tests](../../crates/rtao-evm/tests/ruint_security.rs) exercise
Alloy's actual integer dependency. An independent bit-by-bit shift oracle
checks zero, one, maximum and alternating-bit values at 12 widths, including
all primitive fast paths and non-limb-aligned widths. It compares shifted
values, overflow flags, checked results and saturating left shifts across every
in-range shift, one out-of-range shift and `usize::MAX`. Other fixtures cover
strict shifts, big shifts and shift amounts at least `2^32`.

No Rust build or test was run locally. At authoring time, the compatibility
probe, EVM regression tests, integrated MSRV build and full supply gate still
require hosted results. Source completeness and authored fixtures do not prove
those checks passed.

An independent source review fetched the archive, pinned license and commit
again, checked all retained identities and both local changes, and reviewed the
fixtures. It found calls to crate-private big-shift helpers in the initial
fixtures; those calls were replaced with public shift operators. Its final
review found no remaining source issue. The reviewed probe SHA-256 is
`0081bd873f95415786058d1a6dea29533f2b6ec11174e0c6ea587bea954c89cf`;
the EVM fixture SHA-256 is
`6d9a0161c73a81361ed7b8ab687e882569a7a26a927552ee86ab3ab8d5fb6695`.

The root workspace patch, lockfile and supply provenance guard are integrated
separately. They must select exactly this copy and reject an unexpected source
or file change. Future removal of the patch requires a compatible fixed
upstream release, or an explicit change to the project's Rust 1.89 requirement.
