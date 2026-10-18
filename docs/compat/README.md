# Recording compatibility validation

Use the manual `pr-gates` workflow with `refresh_compat=true` to regenerate
these source reports on hosted Ubuntu CI. Verify its source commit,
artifact digest and report inputs before adoption; see
[the hosted procedure](../execution/ci-gates.md#regenerating-compatibility-inputs-in-ci).
The command descriptions below explain that hosted job. They do not require a
local build or platform rehearsal. Release publication remains deferred.

`cargo xtask compat` generates the release matrix from the workspace version,
`upstream.toml`, and the immutable metadata index. It verifies the stored metadata
checksums, SCALE decoding, provenance and Python project version before writing
`compat.json` and `COMPAT.md`. `--check` verifies both outputs offline. An optional
`--milestone M0` through `M10` records the release milestone; later invocations
preserve it. `--write` and `--json` select output files within the repository.

`cargo xtask compat --measure-build-weight` measures normal/build dependency
counts for the core without host support and the validator, miner and owner
profiles on the compiler's host target. `--build-target <triple>` selects another
Rust target. Cargo evaluates that target's dependency graph; the recorded host
triple and compiler describe the toolchain performing the measurement. This command does
not build a target artifact or claim artifact provenance.

Each report under `docs/compat/build-weight/<target>.json` records the target,
host, compiler, Cargo version, timestamp, source commit and dirty state,
hashes of the workspace/package manifests, lockfile and upstream pins, selected
profile features, complete deduplicated package inventories, and whether the
core's host feature was enabled. The generated JSON embeds these reports and
their checksums; the Markdown presents their scoped counts. Counts exclude dev
dependencies and identify package name/version/source, including multiple
versions, rather than counting lines in the shared lockfile.

Ordinary generation and `--check` validate stored reports offline. Changed
dependency inputs or release versions, incomplete profiles, duplicate package
inventories, inconsistent counts or host activation in a no-host profile fail.
Remeasure each recorded target after a dependency or release-version change.
An explicit M1 or later release milestone requires a measured report. Historical
measurements retained under `docs/execution/` describe their own snapshots; they
are not substituted as constants in future release records.

A fetched metadata blob provides an input identity. A validated identity requires
passing drift, e2e and interop reports. Until those reports exist, the generated
range is `null`, the validated runtime list is empty, and the client's compatibility
guard warns for every runtime. Its per-call surface checks remain authoritative.

The `metadata_hash` kind `blake2b-256-raw` means BLAKE2b configured for a 32-byte
output over the complete raw metadata blob, including its SCALE prefix and version
byte. Each blob also has a SHA-256 file checksum. The transaction metadata
commitment and structural shape hashes serve separate purposes.

The Python source version is taken from the pinned `pyproject.toml`. A matching
version on PyPI alone does not prove publication from that commit, so
`parity.pypi_release` remains `null`.

## Evidence ledger

After the required CI runs pass, store their result reports under
`docs/compat/reports/` and create `docs/compat/validation.json`. All paths must be
relative, contain no dot components, and resolve through regular directories to
regular files. Symlinks are rejected. The generator verifies every report's stored
bytes against the ledger's SHA-256 checksum.

The ledger has these fields:

| field | value |
|---|---|
| `schema` | `1` |
| `inputs_sha256` | `compat.json`'s `validation.inputs_sha256` |
| `validated_spec_range` | `{"min": first_spec, "max": last_spec}` |
| `runtimes` | Exact runtime identity objects selected from the pinned metadata inputs |
| `reports` | Objects with `path` and lowercase `sha256` fields |

A runtime identity has `network`, `spec_version`, `genesis` and `metadata_hash`.
The hash object has `kind` and `value`. Its fields must exactly match a V15 or V16
blob recorded in `compat.json`. Every integer spec in the range needs at least one
recorded identity. An inclusive range spans at most 4096 specs.

`validation.inputs_sha256` hashes compact JSON with recursively sorted object keys
for the input fields: `rtao_version`, `core`, `parity`, `metadata_pins`, `localnet`,
`btauth` and `interop`. Changing those inputs invalidates previous evidence. A
report must use the same hash. Generated validation fields and the milestone are
excluded from this input hash.

Each report has these common fields:

| field | value |
|---|---|
| `schema` | `1` |
| `kind` | `drift`, `e2e` or `interop` |
| `status` | `passed` |
| `inputs_sha256` | The ledger's input hash |
| `run.url` | `https://github.com/sam0x17/rtao/actions/runs/<numeric-id>` |
| `run.head_commit` | The run's full lowercase Git commit hash |

An `e2e` or `interop` report also lists `runtimes` and `image_digests`. The report
asserts that its test kind passed for every combination in those two lists. Each
validated runtime needs passing e2e and interop coverage on every distinct digest
listed in the generated localnet image matrix. Unknown or changed digests are
rejected. A reporter must split reports when runs cover different combinations.

A `drift` report records `from`, `to` and `classification`. Its endpoints are exact
runtime identities on the same network and genesis. The classification must be
`surface-unchanged`, `additive` or `behavioural`. `breaking` cannot validate a range.
Every runtime below the range's upper bound needs a passing drift report to an
identity at the next spec on the same chain. For a single-spec range, record a
`surface-unchanged` self comparison against the pinned input.

Result reporters are added with the CI lanes that produce these results. Report
checksums preserve the recorded evidence; they do not authenticate a GitHub run or
replace review of its logs. No result report or validated ledger is recorded yet.
