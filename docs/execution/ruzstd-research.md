# Ruzstd advisory investigation

Investigated on 2026-10-14 without modifying either lockfile, manifests, or
advisory policy. No Python execution or dependency compilation was needed.

## Finding and package identity

The fresh raw workspace audit reports **RUSTSEC-2026-0344** as a vulnerability,
with `informational: null`, category `denial-of-service`, and patched versions
`>=0.9.1`. It is separate from the approved dormant `lru` unsoundness exception.
The [RustSec source](https://github.com/RustSec/advisory-db/blob/main/crates/ruzstd/RUSTSEC-2026-0344.md)
matches the local JSON: a crafted compressed input under 128 KB can cause
multi-gigabyte allocation and process termination. The advisory describes no
memory corruption or remote code execution. Its fixed commit is
[`d3907c960ca4a1aa44cc38340137c5679dd98f5a`](https://github.com/KillingSpark/zstd-rs/commit/d3907c960ca4a1aa44cc38340137c5679dd98f5a).

Both `Cargo.lock` and `fuzz/Cargo.lock` contain the same affected identity:

```text
name: ruzstd
version: 0.8.3
source: registry+https://github.com/rust-lang/crates.io-index
checksum: a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8
```

The locked parent chain is:

```text
subxt-rpcs 0.51.1
  optional light-client feature -> subxt-lightclient 0.51.1
  -> smoldot-light 1.3.2
  -> smoldot 2.2.0
  -> ruzstd 0.8.3
```

`subxt-rpcs` has `light-client = ["dep:subxt-lightclient"]`. The workspace selects
`default-features = false, features = ["native"]`; its `native` feature forwards
`subxt-lightclient?/native`, which does not activate that optional dependency.
The workspace's `rtao-client/subxt` feature is empty and does not activate it
either. The affected package is currently locked for the disabled light client.

## Graph evidence

All commands below succeeded with `--locked --offline --target all --edges
normal,build,dev --format '{p}' --prefix none`. The all-platform graph includes
normal, build, and development dependencies. Cargo tree deduplication was kept
enabled. No graph contained `ruzstd`, `lru`, `smoldot`, `smoldot-light`, or
`subxt-lightclient`.

| Selection | Graph lines |
| --- | ---: |
| Workspace, `--workspace --all-features` | 1,889 |
| Fuzz, `--manifest-path fuzz/Cargo.toml --all-features` | 1,078 |
| `rtao`, default / no default features / all features | 1,073 / 996 / 1,534 |
| `rtao-cli`, default / no default features / all features | 902 / 902 / 983 |
| `rtao-neuron`, default / no default features / all features | 1,284 / 1,279 / 1,330 |

Line counts describe these evidence snapshots, not an acceptance criterion.
Individual shipped feature selections remain the supply guard's responsibility;
these observations establish absence from the inspected broad and default
profiles. They do not permit future activation.

Evidence is bounded text under the recorded task root:

```text
target/execute-plan.6Ro0PC/tmp/ruzstd-research-*-tree.txt
target/execute-plan.6Ro0PC/tmp/ruzstd-research-audit-summary.json
target/execute-plan.6Ro0PC/tmp/supply-raw-schema.json
```

The raw schema snapshot reports one vulnerability, fourteen unmaintained
warnings, and the separately approved `lru` unsound warning. It does not contain
a fuzz raw audit; the supply guard must collect that independently.

## Compatible fixes and upstream constraint

The live crates.io sparse index was checked for `ruzstd`, `smoldot`,
`smoldot-light`, `subxt-lightclient`, and `subxt-rpcs`:

* `ruzstd 0.8.3` is the latest published 0.8 release. The fixed `0.9.1` was
  published on 2026-10-11. There is no published fixed 0.8 backport.
* `smoldot 2.2.0` requires `ruzstd ^0.8.0`. Every published later major through
  the latest `5.0.0` retains that requirement. The upstream `main`
  [`lib/Cargo.toml`](https://github.com/paritytech/smoldot/blob/main/lib/Cargo.toml)
  also retains `ruzstd = "0.8.0"` at investigation time.
* The latest `subxt-rpcs` and `subxt-lightclient` remain `0.51.1`.
  `subxt-lightclient` requires `smoldot-light ^1.3.1`, whose compatible latest
  release is the already locked `1.3.2`. That package requires `smoldot ^2.0.0`.
* Later incompatible `smoldot-light` releases fix their `lru` requirement but
  still transitively select `ruzstd ^0.8.0`; a broad parent upgrade would not
  clear this advisory.

Because `^0.8.0` excludes `0.9.1`, `cargo update -p ruzstd --precise 0.9.1`
cannot satisfy the existing parent's constraint. Adding an unused direct 0.9.1
dependency would leave the vulnerable 0.8.3 lock entry. A Cargo patch does not
override the parent's semver requirement. Those changes would not fix the gate.

An upstream compatible release that upgrades the decoder, or a verified fixed
0.8 backport, would let us clear the lock finding without changing our public
behavior. Maintaining a local fork and a source exception would add a new
dependency policy decision and was not attempted here.

Upstream already has an open automated issue:
[`paritytech/smoldot#3405`](https://github.com/paritytech/smoldot/issues/3405),
opened by `github-actions[bot]` on 2026-10-14. It is **not an issue filed by this
task**. No duplicate issue or comment was sent.

If light-client support is activated later, Smoldot's
`src/executor/host/zstd.rs` calls `ruzstd::decoding::FrameDecoder` for compressed
runtime blobs. Its current `UptoBytes(max_allowed)` strategy is not a substitute
for the decoder's missing per-block bound described by the advisory. This
investigation does not claim safety for an activated decoder and did not run an
OOM reproducer.

The current supply guard correctly rejects this new finding. Any temporary
acceptance needs its own recorded decision, exact identity checks, raw audit
retention, and activation checks across workspace, fuzz, and shipped features.
The existing approval covers `lru` only.

## Scratch cleanup

Only the bounded evidence files above were created. No private state was read,
and no temporary directory, service, or virtual environment was created. An
initial undeduplicated read-only tree command was stopped after its output grew;
its exact Cargo process was checked and terminated, then the bounded commands
were rerun successfully. Final process inspection found no surviving research
command. The recorded shared task root and its live build cache remain owned by
the overall plan execution and require its final dependency audit and cleanup.
