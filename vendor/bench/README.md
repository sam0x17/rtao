# Reproducible performance checks

The SCALE payloads and their hashes are recorded in [index.toml](index.toml).
The implemented suite has 36 Criterion measurements: raw metagraph decoding through the
borrowed view, typed visitor and core dynamic decoder; complete metagraph
projection through each path; V15 parsing; 10,000 live-metadata storage keys;
decoding a 256-row map page; signing and assembling an extrinsic; and btauth
signing, stateless authentication and verification with replay commitment for
both signature algorithms, and conformance at 256 and 2,500 entries. The map page decodes both the free map keys and
values, with 256 encoded weights per row.

`cargo xtask bench` runs all five harnesses with 30 samples, a one-second warmup
and a one-second target measurement time. Criterion may extend measurement time
to collect the requested samples. Results record the mean, median, confidence
interval, sample count, compiler, CPU, fixture hashes, source revision, dirty
state, pin hash and lock hash. `RTAO_TEMP_ROOT` must identify the task's temporary
root. Raw Criterion output stays below that root, and `bench/latest.json` is the
bounded result. A committed local result describes its recorded machine.

To measure a source revision checked out at `REFERENCE` and the current source
on the same machine:

```sh
cargo xtask bench --workspace "$REFERENCE" --save-baseline "$RTAO_TEMP_ROOT/bench/reference.json"
cargo xtask bench --compare-baseline --baseline "$RTAO_TEMP_ROOT/bench/reference.json" --fail-over-percent 30
```

The gate fails when any mean exceeds its reference by more than 30%. It also
fails for missing measurements, mismatched contexts or invalid estimates.
Changes to the fixture set, sample settings or suite require an explicitly
measured reference. Hosted `nightly.yml` measures both revisions sequentially on
one free Ubuntu runner. Its initial reference is `rtao-v0.1.0`; a dispatch may
select the first benchmark-capable implementation commit before that tag exists.
Later reference updates use a benchmark-capable source revision through the
`RTAO_BENCH_REFERENCE` repository variable or a dispatch input.

The initial M1 measurement had 34 paths. The M2 preparation adds conformance
at 256 and 2,500 entries using the pure helper now implemented. Comparisons
require a reference revision with these same measurements; a missing target
fails instead of silently dropping a regression check. The nightly gate cannot
compare the local Mac measurements with an Ubuntu result.

For robustness checks, install the pinned `cargo-fuzz` release under the recorded
temporary root, add its `tools/bin` to `PATH`, and run:

```sh
cargo +nightly install cargo-fuzz --version 0.13.2 --locked --root "$RTAO_TEMP_ROOT/tools"
cargo xtask fuzz --toolchain nightly --seconds-per-target 120
```

The four targets are `btauth_headers`, `btauth_verify`, `json_rpc_frame` and
`metagraph_view`. The driver requires nightly and cargo-fuzz 0.13.2, uses address
sanitizer and coverage instrumentation, copies immutable seeds into temporary
corpora, runs arbitrary mutations of the real metagraph responses, and retains
bounded results plus any crash artifacts. Cargo-fuzz has no `--locked` option;
the driver resolves with `cargo metadata --locked`, executes offline and fails
if the standalone lock changes. Future milestones add their targets to the
driver and manifest together.
