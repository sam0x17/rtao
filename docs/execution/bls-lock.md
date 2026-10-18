# BLS lockfile requirement

The original M0-09, M0-11 and M0-14 raw-lock requirement failed at v478.
Q26's built-dependency correction was approved on 2026-10-14.

Cargo.lock contains:

- w3f-bls 0.1.3 from RaoFoundation/subtensor tag v478, commit
  850f2755bb05a3851f948489f0ab27b0645cebce.
- w3f-bls 0.1.9 from crates.io. Upstream polkadot-sdk's sp-core declares it as
  an optional dependency used by bls-experimental. Cargo locks optional
  dependencies even when every rtao profile leaves that feature off.

This command succeeds and reports no dependencies:

```sh
cargo tree --locked --workspace --all-features --target all -e normal,build -i w3f-bls@0.1.9
```

The all-feature tree and both no-host trees contain only the pinned 0.1.3 in
normal/build edges. Upstream research reports F §3.5 and B record both locked
versions; the final plan's raw-lock requirement overlooked that distinction.

The implemented correction requires exactly one built BLS implementation, with
source and commit equal to core_tag, in every shipped profile and the workspace
all-feature, all-target normal/build graph. The lock check still requires
exactly one pinned 0.1.3 entry and allow only the explicitly recorded dormant
registry 0.1.9 entry, with checksum
`e6bfb937b3d12077654a9e43e32a4e9c20177dd9fea0f3aba673e7840bb54f32`.
Actual dependency-tree checks fail if any feature activates that
second entry, including sp-core/bls-experimental, or if a new unrelated BLS
entry appears. This preserves the downstream patch and timelock byte rules.

Shared policy checks now cover the core lock test, pin, dependency checks and
the consumer workspace inspected by doctor. All-feature/all-target normal/build
inspection rejects dormant activation; separate shipped profiles are checked
by deps-check. Current Rust and Rust 1.89 pass the shared policy, pin, core lock
and eleven doctor regressions, including a real downstream optional and
cross-target build-dependency fixture. Strict Clippy passes both toolchains.
The actual `pin --check` and doctor commands pass. Raw logs are not retained
in the repository.

The subsequent staged-boundary repair passes thirteen dependency regressions
and strict Clippy on both toolchains. Portable base64 and zeroize copies must
match the actual pinned core graph; benchmark-only dependencies stay in the
dev graph. Extrinsic hashing delegates to the core's existing `hash_param`
API. Unicode formatting uses authenticated local data rather than a new staged
crate dependency. Direct test RPC dependencies move through the dev-only
testkit export. These changes preserve the layer restrictions; the full
dependency command and complete gates still need final integrated verification.
