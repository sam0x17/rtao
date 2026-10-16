# M1 metagraph decoder evidence

Current whole-source review traces all 32 functions and 54 dataclass fields.
It finds one direct `from_dynamic` adapter gap: hex text accepted by Python's
`bytes.fromhex` may contain ASCII separators between complete byte pairs. The
repair shares the existing exact commitment parser through `pyfmt`, retaining
commitment diagnostics and correcting three source spans. One new public
adapter fixture covers both text fields, all six accepted separators and exact
split-nibble/non-ASCII error positions. It remains unexecuted pending hosted CI.
Canonical chain text uses byte lists; this case demonstrates no wire-path
failure. Typed row access, `Option` and Rust `Debug` are explicit native API
choices; the whole source row remains pending final disposition and results.

The graph has one row projector shared by the borrowed parser, by-name typed
models and the core's dynamic values. Fixed-width key, axon and boolean columns
remain ranges into retained response bytes. Compact columns decode once into
native u64 values. The borrowed parser checks the live exact layout, option
tags, lengths, bounds, score widths and trailing bytes. The typed fallback
checks the generated model projection. Dynamic decoding remains the reference.
Fixed-point hashes include validated signedness, storage width and fractional
bits. The curated dynamic fallback also checks that `moving_price` is I96F32
before interpreting it. A Frac32-to-Frac64 change is rejected, while the core's
raw dynamic reads remain available.

Finney subnets 1, 3 and 64 were recorded with `state_call` at finalized block
`0x5069877ce8f7b95a12fa346ba3d2bf578819f4cd1e7bc89dc22c335f798be3cb`
(height 9279852, spec 478). The same block's runtime code hash is
`0x7e722b3aa5c9192c98665a5d65fa91364bf9055e21fc90efce6d59bf4ef414e1`.
[vendor/bench/index.toml](../../vendor/bench/index.toml) records endpoint,
arguments, sizes and SHA-256 checksums. Historical 2,500-UID fixture generation
used `cargo run -p rtao-metagraph --example synthetic_fixture -- vendor/bench/synthetic-2500.scale`.
Any required regeneration belongs in bounded free `ubuntu-latest` CI under Q30.

All three decode paths produced equal full dataclass JSON for these four
payloads. Tests also cover appended fields selecting the typed fallback,
renamed fields selecting the dynamic fallback, short-column padding,
pre-collateral records, mechanism indices, UTF-8 replacement, first-row hotkey
lookup, truncated responses and native balance range errors. Reserve prices
divide exact integers before rounding to f64. IPv4-mapped IPv6 endpoints retain
Python's hexadecimal spelling. Curated fetch tests use an actual client and
encoded fake-chain state: graph and commitment scans share one hash, snapshots
retain their hash after head advancement, and commitment opt-out performs one
runtime call without storage scans.

`ClientConfig::metagraph_view`, the per-read `view` option and
`RTAO_METAGRAPH_VIEW=0` all disable the borrowed decoder. Auto then selects the
typed fallback on the pinned metadata. An explicit View request returns an
error before graph RPC or commitment scans. Tests cover clients, snapshots and
direct decoding. Normal tests launch separate child processes for the
environment checks, so CI runs them without mutating the test process's
environment.

The registry generator checks 29 model adapters at 256 seeds against spec 478,
plus five metagraph/serving adapters against the committed spec 461 metadata.
It uses all compact boundaries, lengths 0-3 and depth at most eight. Separate
checks exercise all 138 commitment, eight hyperparameter, 19 proxy and three
bounded-call variants. Typed values and core values agree, with complete input
consumption and no skipped failures.
The generator validates all 31 named structures and four exact enums against
their Rust declarations and the pinned metadata. Commitment Data's Raw arm
expands to all Raw0 through Raw128 metadata variants; every other enum requires
the complete variant order, indices and payloads. All eleven semantic field
overrides are enforced.

The historical validation commands passed. Current substantial checks use
hosted CI; no local build, benchmark or fuzz campaign is required under Q30:

- `cargo +1.89.0 test --locked -p rtao-metagraph -p rtao-runtime --all-targets`:
  14 graph decode tests, six fetch tests, 49 runtime unit tests and seven runtime
  integration tests. The two environment checks also pass inside the child
  processes launched by their normal test wrappers. Benchmark entry points
  completed their smoke checks.
- `cargo clippy --locked -p rtao-metagraph -p rtao-runtime -p rtao-primitives --all-targets -- -D warnings`.
- `cargo xtask regen --check`: 31 structures, four enums, 18 scoped pallets,
  25 runtime APIs and 14 namespace accessors match the pinned inputs.
- The chain endpoint helper's IPv4, IPv6, mapped IPv6 and zero-IP checks.
- `cargo check --manifest-path fuzz/Cargo.toml --bin metagraph_view`.

`cargo run -p rtao-metagraph --example finney_metagraph -- 1` also completed
through the public Client and curated namespace at block 9280206, spec 478:
View decoding, 256 neurons, nine validators and nine unregistered commitments.
It disables disk caches and closes the client after the read.

The standalone fuzz target completed a direct 512-run entry-point smoke check
with all four payloads as seeds. That command lacked coverage instrumentation
and is not an instrumented nightly fuzz result. Q30 makes further fuzz/oracle
campaigns optional targeted hosted work and defers localnet infrastructure;
none is a current M1 prerequisite. This evidence does not mark M1 complete.
The capture and smoke-corpus temporary directories were removed after checking
processes, open files, configuration, launchd, symlinks and virtual-environment
dependencies. The former task build root is retired and absent; no current
live-cache dependency remains. Preserve shared global Cargo/toolchain caches.
