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

A runtime is validated only when the compiled evidence lists its exact
genesis, spec version and raw metadata hash within the recorded range. The
client identifies the chain by its genesis hash, not by the network label, so
an explicit endpoint for a public network is checked like its named preset.
A runtime outside the evidence produces a compatibility warning on Finney,
testnet and devnet. Local and custom chains have their own genesis and can
never be validated, so the client logs that message at debug level there.
Downloaded metadata records an input; a successful hosted client build records
its own scope. Neither establishes live-chain interoperability.

Validation evidence comes from the manual `pr-gates` workflow dispatched with
`validate_compat=true`. It checks that the pinned network's finalized runtime
is the pinned input, runs the e2e suite and core oracle on one localnet lane
per distinct pinned image digest, and attests each passed report. A maintainer
adopts a successful run with `ci/compat/adopt.sh <run-id>`, which verifies the
attestations before recording the reports and regenerating `compat.json`. See
[recording compatibility validation](https://github.com/sam0x17/rtao/blob/main/docs/compat/README.md)
for the report requirements.

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

Each weight attempt composes only the call its mechanism uses, so a change
to the other weight call never blocks it. When that call's compiled shape no
longer matches the live runtime, the scheduler composes it by name against
the live metadata if every field still fits (for example a field moved into
a single-field newtype, `Vec` replaced by `BoundedVec`, a compact switch, a
widened integer or reordered fields). It logs a warning once per runtime and
counts `rtao_weights_resolved_composition_total`; the call is decoded back
and compared with the prepared values before anything is signed. A validator
can also install a `WeightCallEncoder` for calls that name resolution
refuses. The attempt pauses (`PausedSurface`) only when no composer can
produce the call: an added, removed or renamed field, or a field that became
an `Option`, an enum, a narrower or signed integer, an array or a nested
call. Review the refusal, update rtao or the encoder, and the next head
resumes weights once the call composes again. A subnet's weight
version key is also an application contract: configure
`--weights-version-key` only for a scoring implementation that supports that
version. Raising the number alone does not upgrade the implementation.

Commit-reveal commits carry a version tag that root sets in
`CommitRevealWeightsVersion`. The chain only checks that the tag matches; the
runtime code decides which payload a reveal can decode. rtao encodes the
version 4 payload. When the stored tag changes, the scheduler follows it
without a rebuild if the live runtime code is a reviewed pinned runtime or has
already revealed one of the validator's own commitments while that same tag
was stored. Otherwise timelocked commits pause for that mechanism, with one
warning, until the tag returns to 4 or the code is proven; other duties keep
running. A commitment carrying a followed tag that then misses its reveal for
no visible reason pauses that tag under that code again. If the chain rejects
a commit's tag at dispatch, the scheduler re-reads the tag and retries, at
most 10 times with a growing wait. A runtime pin must review
the reveal path and add the new code hash to `REVIEWED_CR_RUNTIME_CODE` when
the payload format is unchanged; a test keeps the newest pin in that list.
Follow the [validator guide](validator.md) for reveal observations,
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
