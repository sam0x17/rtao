# M1 runtime cache evidence

`rtao-client::RuntimeCache` ports the pinned Python
`sdk/python/bittensor/_transport/runtime.py` at
`850f2755bb05a3851f948489f0ab27b0645cebce`. Network metadata uses the V15
runtime API first. Only the extracted missing-method needle, a null response,
or an unavailable metadata option permits the legacy RPC. Malformed wrappers
and other RPC errors propagate. Historical blocks select their parent's
runtime; genesis selects itself.

Clones share a four-entry runtime LRU keyed by genesis, spec and transaction
version, a 512-entry block-to-runtime LRU, and `Shared` metadata fetch futures.
Cancelled callers leave the fetch resumable; futures hold a weak cache reference
to avoid a reference cycle. Head refreshes use the Python twelve-second TTL,
and invalidation prevents an older pending refresh from revalidating the head.
Parsing, extension inspection, surface verification and disk work run on
blocking workers.
Advertised runtime API IDs resolve to metadata trait names using Blake2b-64.

Disk names, headers, and pruning match Python: `md1-<chain16>-<spec>.bin`,
`STMD` followed by little-endian `u32 transaction_version`, one flag byte, and
the raw metadata. The flag records the V15 or legacy fetch path, as Python
writes it; the parsed body determines `MetadataLevel`. Atomic replacement
keeps eight `md*.bin` entries. Corrupt entries are discarded and downloaded
again. The caller injects the cache directory, with `None` disabling disk I/O;
`default_disk_dir()` resolves Python's override and home-directory default.

Validation on 2026-10-14 used the actual committed Finney V15 metadata and a
V14 conversion of that metadata. All 22 cache tests passed, with no ignored
tests. They cover parent selection, cache bounds and eviction, sharing,
cancellation, upgrade invalidation, retryable failures, disk byte equality,
pruning and corrupt recovery, narrow legacy fallback, V14 signing refusal,
and runtime API version lookup. These historical commands passed against the
product crate. Current substantial checks belong in free `ubuntu-latest` CI
under Q30, without a local rebuild:

```text
cargo test -p rtao-client --lib cache:: --no-default-features --locked
cargo clippy -p rtao-client --all-targets --all-features --locked -- -D warnings
cargo +1.89.0 check -p rtao-client --all-features --locked
cargo +1.89.0 check -p rtao-client --no-default-features --locked
```

Builds and tests used the recorded
`target/execute-plan.6Ro0PC/{build,tmp}` root. Fixture directories were removed
after their RPC and blocking workers completed; the final directory check
found no `runtime-cache-test-*` directories. That execution root is retired
and absent; no current build/cache dependency remains. Shared global Cargo and
toolchain caches are preserved. No Python was run locally.

## M3 offline persistence support

`RuntimeCache::genesis()` returns the immutable chain identity.
`cached_runtime(spec)` resolves an existing runtime without RPC, file writes or
deletion. The most recently used matching in-memory handle wins, including its
transaction version. A disk-only handle retains the stored transaction version
and has no verified runtime API versions. It is returned for decoding without
insertion into live or historical cache slots or replacement of the head.

Disk lookup runs on a blocking worker. Missing and corrupt files return typed
errors and remain untouched. The offline path validates the STMD header and
flag, rejects nonregular files and limits input to 64 MiB. On Unix, the file is
opened with `O_NOFOLLOW | O_NONBLOCK` and bounds/type checks use that descriptor.
This refuses final symlinks and checks FIFOs without waiting for a writer, even
if the cache path changes during lookup. The normal cache fetch path retains
its existing corrupt-entry deletion and redownload behavior.

On 2026-10-14 all 31 cache tests passed on Rust 1.99 and Rust 1.89, including seven
offline tests covering same-spec transaction versions, no-RPC operation,
preservation of malformed and missing entries, empty disk API provenance and
head/cache isolation. Symlinks, FIFOs, directories and oversized files are
rejected without mutation; the FIFO check returns within its one-second budget
with no writer present. Strict all-target Clippy for primitives, client, intents
and TX passed on both toolchains. Those follow-up logs were recorded under
`target/execute-plan.6Ro0PC/tmp/m2-cache-persistence-bounds-{current,msrv,clippy,clippy-msrv}.log`;
that path describes historical logs in the retired root.
All fixture workers completed before their RAII owners removed the fixtures;
the final directory audit found no `runtime-cache-test-*` directories.
