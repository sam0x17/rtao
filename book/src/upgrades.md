# Runtime upgrades and compatibility

A runtime upgrade can change call encodings, storage shapes and transaction
versions while a client is connected. rtao uses runtime metadata and per-call
surface checks to decide whether a typed operation can proceed. Review those
checks alongside the recorded validation evidence when updating a client or
responding to a chain upgrade.

## Check the client against the runtime

1. Read the checkout's `COMPAT.md`, `compat.json` and `upstream.toml`. These
   identify the core and Python source pins, metadata inputs and any validated
   runtime range. Check the exact source commit's hosted CI results and the
   features enabled in your build.
2. Connect to an existing endpoint with the intended network and inspect the
   reads your application needs. A named network's genesis mismatch disables
   signing. Embedded callers can inspect `Client::genesis()` and pin a
   `Snapshot` with `Client::at(...)`; use `Client::refresh_runtime()` for the
   current head runtime before preparing work.
3. Preview each intended mutation with `--dry-run`, or use `Executor::plan`
   with the application's `Policy`. Review the resulting call, signer,
   estimated fee and violations against current state. Resolve any runtime
   surface refusal by reviewing and updating the affected client code and
   descriptors before submitting again.
4. After a runtime change, prepare a fresh plan and review any changed call.
   Preserve receipts and recovery records for work already submitted. An
   unknown transaction outcome needs reconciliation before a replacement is
   sent.

The current compatibility record has no validated runtime range. Every
connected runtime therefore produces a compatibility warning. Downloaded
metadata records an input; a successful hosted client build records its own
scope. Neither establishes live-chain interoperability. Runtime validation
requires the exact network, genesis, spec version and raw metadata hash in
the compiled evidence, as well as its recorded range.

Typed composition checks the required call surfaces, and reads check their
own descriptors. `CallBytes` records both specification and transaction
versions. Signature preparation and submission reject stale stamps. Semantic
execution can refresh and replan once after a version mismatch, reapplying
its policy. An `ExecuteOptions::expected_call` constraint remains enforced,
so a changed approved call requires a new review.

For offline or external signing, prepare a new payload after a runtime
mismatch and obtain a new signature. `Executor::submit_signature` validates
the runtime stamps, chain, era and signature; it cannot replan or retry the
immutable signed call. See [multisig and external signing](multisig.md) for
the preparation and result-handling flow.

## Keep validator behavior compatible

The validator scheduler pauses a weight attempt when its required call
surface is incompatible. Review the refusal, update rtao as needed, and
verify the affected surface before resuming weights. A subnet's weight
version key is also an application contract: configure
`--weights-version-key` only for a scoring implementation that supports that
version. Raising the number alone does not upgrade the implementation.

Commit-reveal has a separate client version check. The scheduler supports
version 4 and refuses a different chain requirement while commit-reveal is
enabled. Follow the [validator guide](validator.md) for reveal observations,
shutdown and saved-state compatibility. Retain application checkpoints when
updating the binary; a changed state format needs an explicit schema version
and recovery plan.

## Inspect a proposed chain upgrade

The CLI also supports approval of the chain's existing runtime-upgrade
proposals. This is an operator action using an existing endpoint and an
already-prepared proposal. Inspect the available routes with:

```sh
rtao misc upgrade pending --help
rtao misc upgrade check --help
rtao misc upgrade sign --help
```

`pending` correlates on-chain proposals with release records. `check` takes
exactly one of `--url`, `--hex-url` or `--hex-file`; `--url` accepts a GitHub
release page or `OWNER/REPO@TAG`. Optional `--wasm` compares an already-built
runtime file with the proposal's embedded bytes. Authenticate the source of
the proposal and comparison artifact independently.

Artifact parsing precedes the chain connection. Live checks cover the
expected proxy/Sudo/set-code structure, byte-for-byte re-encoding against
current metadata, the chain's sudo key and a matching pending proposal hash.
Available manifest and digest data receive additional checks. Inspect every
check's status: the command can succeed with the WASM comparison skipped,
and matching a release's WASM only establishes agreement with that artifact.
Use independently verified hosted build output when that comparison is
needed. Client development does not require a local runtime build.

`sign` resolves the sudo multisig membership and threshold, verifies that
they derive the live sudo account, and checks the member before asking for
approval. `--dry-run` reports the proposed signing action. Actual submission
rechecks the proposal and multisig state after confirmation. Inspect the
receipt and pending approvals to distinguish an approval from completed
execution. Do not infer a chain upgrade from a successful approval alone.

Substantial client builds and tests run on free hosted `ubuntu-latest` CI.
This guide does not establish a passing runtime-upgrade rehearsal, live-chain
compatibility range or deployment result. Client binary distribution and
release verification are covered separately in [release verification](releases.md).
