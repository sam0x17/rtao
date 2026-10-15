# M3 miner guide preparation

`book/src/miner.md` documents the implemented miner runtime and signed echo
example. Its start commands use the facade's `miner,prometheus` features and
the actual `NeuronConfig` flags. It covers endpoint forwarding and discovery,
idempotent publication, certificate rotation and the separate HTTP listener,
verified handler identity, Value-first JSON, state recovery, metagraph updates,
pruning estimates, graceful shutdown and refusal statuses.

Source review used `PLAN.md` section 11 and M3-10, the existing validator guide,
`crates/rtao/examples/miner.rs`, neuron `miner.rs`, `config.rs`, `runtime.rs`,
`state.rs`, `publisher.rs`, `prune.rs` and `telemetry.rs`, metagraph `sync.rs`,
and btauth `lib.rs`, `nonce.rs`, `redis.rs`, `server.rs` and server regressions.
The guide requires replay retention strictly greater than freshness plus skew
and distinguishes replay state from application checkpoints. It preserves
native checked `u64` balances and distinct TAO/alpha units, and uses
`StakeWeight` only for admission and optional scheduling.

The guide references the existing compiled example instead of adding a second
Rust implementation in the book. Existing bounded logs under the recorded
task root `target/execute-plan.6Ro0PC` confirm the miner example's current-Rust
check and strict MSRV Clippy (`tmp/neuron-miner-example.log` and
`tmp/neuron-miner-example-msrv.log`), and the signed three-MiB runtime/state
regression (`tmp/neuron-miner-runtime-tests.log`). These earlier runs are
implementation evidence, not a new deployment run for this documentation edit.
Localnet, real endpoint routing, hosted interop and template acceptance remain
required. No screenshot or testnet success is asserted.

Root integration subsequently rendered the complete book, including the miner
and validator chapters, with checksum-pinned mdbook 0.5.4 for Apple ARM.
The archive SHA-256 is
`03e8a6d8b13a2971e0b3280affd03b388373c1485e26f73407c3a76b0b1838df`.
Rendering passed without warnings. Logs and source/tool hashes are retained in
`tmp/root-book-render-evidence` under the recorded task root. The disposable
archive/extraction/HTML root was audited and removed; the pinned mdbook binary
remains a shared task tool for subsequent documentation checks. This render
does not establish a tagged documentation release or deployment acceptance.

The edit adds the miner chapter to `book/src/SUMMARY.md`. Static checks confirmed
its example feature gates, runtime builder names, configuration flags, refusal
statuses, chapter links and strict retention condition against the sources.
The fix-slop pattern pass and manual prose review found no remaining flags;
the whitespace check passes. The original guide edit used source checks only;
the later rendered-book check is recorded above. No fresh Rust build was run
for the documentation edit. Section 11's eventual
docify integration still needs an export annotation on the existing example.

The guide edit and later rendering do not mark a milestone complete, commit
or push files, run Python or Docker, or touch wallets. The original guide edit
created no disposable directory; renderer cleanup is recorded above. Shared
build caches and the pinned mdbook tool remain live.
