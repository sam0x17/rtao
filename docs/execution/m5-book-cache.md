# M5 multisig book and call-cache source

`books::MultisigBook` binds the existing `Paths` override for
`BTCLI_MULTISIGS_PATH`. Its async `load`, `save`, `add` and `remove` methods
run filesystem work on a blocking worker. Add requires explicit overwrite for
an existing exact name. Replacement removes the prior same-name rows and
appends the new row, as in the pinned Python writer. Other rows, reference
order and unknown fields survive. Names and signatory references must be
nonempty strings and thresholds must fit a positive native u16. The book
retains portable references. Signing code still resolves their distinct
accounts and validates live runtime limits before using a preset.

The existing `FsRawCallCache` already provided off-reactor read/modify/write,
same-directory exclusive temporary files, file fsync, rename, parent fsync and
failure cleanup. The change retains `RawCallCache::save` and shares that file
publication mechanism with the book. New Unix files are 0600 and new
directories are 0700. Atomic replacement preserves an existing file's mode.
Existing directory permissions are preserved. Final symlink and special-file
destinations refuse. Paths and parent directories remain caller-controlled;
these helpers do not provide a cross-process transaction or a hostile-parent
filesystem sandbox. Concurrent updates retain Python's last-writer-wins model.
The pre-existing synchronous contact signer-tag flow also reuses the shared
publication helper; general address/proxy writer APIs remain M8 work.

Mutating readers reuse the existing bounded, zeroizing, Unix nonblocking wallet
reader. Descriptor validation prevents FIFO substitution from blocking open.
Input and serialized output are capped at 16 MiB. A bounded serializer stops
before allocating output beyond the limit, and programmatic JSON receives a
128-level nesting bound before cloning or serialization. Invalid JSON and
wrong top-level shapes become empty working data. On replacement, their exact
bytes are retained at a unique `.corrupt.<timestamp>[.<ordinal>]` path. A hard
link installs the backup without replacing an earlier backup and leaves the
original path present until rename. Rejected replacement data creates neither
a backup nor a temporary file. Read-only loads do not create backups.

The pinned Python book/cache writers use direct `write_text` and do not back
up corrupt input. Atomic publication and corruption retention follow the
architecture's shared-file requirement. Strict size/type/depth limits,
explicit private modes and hash verification strengthen the filesystem
adapter while preserving the btcli list/object schemas and top-level
`0x`-prefixed call-hash keys. D-045 records these reviewed differences and
their six regression sources. Hosted validation remains pending.

`FsRawCallCache::save` now requires a 32-byte hex hash and exact nonempty
`call_data`. It verifies the bytes with the core's `Blake2_256` hasher and
checks any embedded `call_hash` before modifying a file. Hash and data hex
normalize to lowercase. Existing unrelated entries remain intact, including
legacy fields. `load_call(hash, runtime)` checks the same identities and uses
`CallBytes::decode` for complete metadata-directed decoding and byte-identical
re-encoding. It returns call bytes only. Cached target/params, labels and
network names do not authorize an operation or supply its semantics. The
cache identifies call contents, while chain identity, signer membership and
the exact live multisig opening remain the caller's responsibility.

The dedicated `book_cache_contract` fixture source covers exact-name overwrite,
reference/unknown-field preservation, removal, oversized input/output,
programmatic depth refusal, corruption retention, mode preservation,
symlink/FIFO refusal, call-hash/data substitution, presentation-field poisoning
and canonical SCALE import with a trailing-byte rejection. The existing
`raw_call_contract` cache fixture now supplies real hash/data identity. All
fixtures use their own exact children of `RTAO_TEMP_ROOT`, or the bounded
repository test root, and remove them after file handles and worker calls
finish.

Installed rustfmt and `git diff --check` pass for this source batch. No build,
test, Python oracle, wallet operation, network operation or temporary directory
was run or created locally. Compilation, fixture execution, strict Clippy and
M5 acceptance remain pending hosted CI. No command registration, context,
manifest or other command file is changed by this batch.
