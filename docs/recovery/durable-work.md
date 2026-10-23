# Durable native work and transaction recovery

The baseline audit compared local and remote main at `42b2c19`. This change
extends the existing client submission pipeline, executor, weight scheduler
and reveal watcher. It does not replace runtime tracking, transaction receipt
decoding, application checkpoints or owner-operation recovery journals.

| Requirement | Existing baseline | Extension |
| --- | --- | --- |
| Durable work | Application `StateStore` checkpoints and specialized owner/CLI journals | Versioned `OperationJournal` with injected storage and a native atomic-file backend; logical effects and signed attempts remain separate; weight slots, inputs and reveal observations restore from the same journal |
| Uncertain transmission | In-process finalized-history reconciliation, nonce reset and explicit unknown fate | Persist bytes before author dispatch; finite restart reconciliation with a durable cursor; missing history, stale views and receipt decoding failures retain uncertainty |
| Runtime upgrades | Metadata refresh, runtime-stamped calls, one executor replan, signing checks and historical parent-runtime receipt decoding | Keep the logical operation across upgrades; prepare fresh physical attempts only after earlier ones are proven unexecuted; semantic guard and structured incompatibility remain bound to an operation |
| Scheduling | Current-state preflight, actual epoch/inclusion observation, per-mechanism cadence and missed-head replay | Retain queued inputs independently of timers; restore slots; recompute the reveal period from historical parent state; bound each reveal replay call and block new submission until caught up |
| Recovery controls | In-memory uncertainty block | Persist pause policy; inspect all operations; recover without a signer; exclusive native/process and scheduler-scope ownership; atomic checkpoint/reveal handoff |
| Failure testing | Encoded FakeChain, transaction and reveal fixtures | Native-file restart, killed writer, lost acknowledgment, write/migration interruption, duplicate recovery, competing attempts, compatible version changes and durable reveal replay fixtures; executable synthetic restart example |

## Transaction use

Open `OperationJournal::open(path)` once and retain its `Arc`. The native
backend holds a lifetime OS lock on a stable sibling lock file. Applications
using an injected `JournalStore` must provide atomic durable generation CAS
and a `claim_scope` guard shared across every store handle/process; an expiring
lease alone cannot make an old writer harmless. The native backend also
refuses two scheduler handles sharing a scope. Never delete a lock file to
force takeover of a live process.

Create an `OperationIntent` with a stable logical ID, optional application
correlation ID, genesis, signer, scope and intended effects. Reopening the same
ID and effects is idempotent; changed identity or effects is refused. Bind it
through `SubmitOptions::journal` or `ExecuteOptions::journal`. Store required
commit/reveal material before preparing its physical call with
`let binding = binding.material(material)?`. Retain that returned binding for
submission. Each material replacement advances a persisted generation and
invalidates older clones, including A-to-B-to-A replacements. Signed publication
checks the selected generation atomically before author I/O; concurrent
preparation cannot attach newer recovery material to older signed bytes. The
ordinary client still performs its nonce, metadata, signing and policy checks.

The actual signed bytes, hash, nonce, spec/transaction versions, mortality era,
era anchor, expiry, metadata hash and material snapshot are persisted before
any author RPC. A failed durable write cannot authorize transmission. A crash
between that write and dispatch remains unresolved, even if it happened just
before network I/O. It is deliberately not guessed to be unexecuted.

`recover(client, maximum_blocks)` reads the submission endpoint's finalized
history. It returns `Rebuildable`, a decoded `Finalized` receipt, or structured
`Unresolved` evidence needs. A successful body read advances the durable cursor;
a missing block, RPC failure or stale/disagreeing finalized identity does not.
Only complete history excluding the exact bytes through mortal validity,
after finality has passed expiry, permits replacement. An immortal attempt
cannot acquire this expiry proof. Repeated calls continue or return the same
historical result without sending anything.
Negative recovery scans require a present extrinsics array and valid entries;
missing/null/malformed lists retain the same cursor and unresolved attempt.
The legacy public lookup's tolerant Python behavior is unchanged.

A finalized dispatch failure also blocks automatic replacement: execution can
have partial effects. The application must inspect the receipt and its own
postconditions before authorizing different logical work. The SDK cannot infer
business-level idempotency from a transaction hash or nonce alone. Callers must
not bypass the journal with a second unjournaled submission path.

General journaled MEV shield pairs currently fail closed. The existing shield
path remains available without this journal, but a durable carrier/inner adapter
with separately reconciled identities is still required before claiming shield
restart support. No shielded dispatch silently ignores the journal option.

## Paired-evidence storage foundation (schema 3)

The native backend migrates supported schema1/2 raw envelopes before typed
conversion. Signed bytes, prior certainty, checkpoints and unknown nested
fields remain retained. New graph-field collisions, unsupported variants and
lossy conversions refuse without replacing the original image. The native byte
parser rejects duplicate decoded object names recursively, including escaped
equivalents and opaque extension fields, before constructing a JSON Value;
otherwise last-key-wins parsing could discard evidence before the migration
check sees it. Literal serde reserved Number/RawValue object keys are also
refused before their special deserializer behavior can change opaque object
shape; ordinary numeric tokens and marker-looking string values remain allowed.
The native writer checks serialized material/extensions before publication too,
so it cannot write a new ambiguous image that the reader would refuse. Migrated
typed decoding uses JSON bytes rather than a Value-to-flattened-Content shortcut
that would reject some supported large integers. The existing opened-file byte
bound, JSON nesting limit and EOF check remain in force. Migration uses
the same temporary-file sync, atomic rename and directory barriers; an
interruption may leave a complete old or complete new image, never permission
to reset. Schema2 readers refuse the new envelope; do not downgrade a journal.
Injected stores must supply an explicitly migrated, validated schema3 image.

An operation with no signatures starts `Unselected`; its route selects once
before signing. Legacy signed records migrate to `Ordinary`, without inventing
shield pairs or per-attempt material generations that were never recorded.
Every signed attempt has a stable ordinal and Ordinary/Inner/Carrier role.
Pairs reference those same attempts and retain selected material generation,
preparation/authorization phase and original pair limit. A stored authorization
phase is evidence of uncertainty, not a transferable send capability. Revision
CAS rejects removal or mutation of existing signed identities, intent, route,
limits or authorizations. Contradictory terminal evidence is retained and blocks
replacement instead of overwriting one claim with another.

Recovery selects the exact unique finalized effect across retained attempts;
it never interprets the last vector entry or a carrier as that effect. The
historical scan allowance is shared across targets/pairs, not renewed for each.
It bounds strict historical body lookups; historical receipt decoding for a
found target is additional, and a cached finalized receipt has its own lookup.
Missing/malformed bodies retain the corresponding target's cursor. A recovery
call rechecks the current mode and all retained attempts before reporting
Rebuildable; an attempt published concurrently keeps the operation unresolved
with its stable attempt ID. This is a
configured-RPC history trust boundary, not independent consensus or body-root
verification.

Shield pairs still cannot authorize dispatch or reseal. Even complete expiry
scans remain `ShieldAdapterRequired` without the actual carrier/inner adapter
and historical protocol qualification. Signed-pair recovery fixtures test this
storage/evidence foundation, not a live shield submission pipeline. Ordinary
default weights remain journaled and do not require enabling shield.

Before durable shield author support can be enabled, the live path must refuse
nonfinal waits before key preparation and gate effect consumers on exact
finalized inner receipts. Native queued frames also need explicit ownership
and write-retirement guarantees: cancellation of an outer RPC future or an
asynchronous queue Cancel does not prove its captured socket write retired.
Injected author transports without that capability must refuse. Those hooks,
writer lifecycle tests, runtime-profile qualification and end-to-end scheduler/
reveal composition remain work beyond this storage foundation.

## Weight scheduler use

`NeuronRuntime` opens `weights-journal.json` under its persistent state directory
before scheduler submission. Embedded schedulers opt in with `with_journal`;
`WeightScheduler::new` by itself retains its previous in-memory behavior.
Use `NeuronRuntime::with_weight_journal` to retain a shared application handle
for inspection, pause and resume while the supervised scheduler owns its scope.
Applications can return `WeightsProposal::Operation { id, correlation_id,
input }` or explicitly `enqueue` work on an embedded durable scheduler. Plain
`Set` proposals receive a journal-generated logical ID. Redelivery of completed
application IDs does not authorize a second transaction.

A prepared row and its ciphertext/round are recorded before submission. If a
process dies after finalized inclusion but before the scheduler locates the
native queue, recovery uses the original attempt material and actual historical
inclusion to reconstruct reveal tracking. The reveal checkpoint and handoff
marker publish together. Failed or interrupted handoff is repeatable.

A missed timer or changed tempo/rate limit preserves the pending logical input.
Future opportunities are recomputed by the existing coherent preflight. The
actual inclusion block and epoch are retained even when the read endpoint lags;
missing inclusion state blocks handoff rather than reopening a cadence slot.
Preflights older than the retained inclusion block cannot become ready, even
with a zero chain rate limit. Once-per-epoch work requires a strictly newer
observed epoch; a counter reset requires explicit reconciliation/migration.
The logical operation's explicit version key remains fixed. A policy update cannot
silently replace that key on pending work. Current surface/version refusals
pause execution without deleting it.

`WeightScheduler::next_action` reports readiness, pause, reconciliation, a
rate-limit height, or an observed-epoch/offset condition from a fresh coherent
preflight. Recompute this after parameter changes. A predicted timer is not a
promise that the native chain will process an epoch at that time.

`recover_finalized` is observation-only and requires no private signer. It
validates the supplied head against the client's finalized history and handles
at most 128 historical transaction blocks per active operation plus 256 reveal
blocks per call. Until retained reveal history catches up, no new weights are
submitted. Missing history can therefore block progress indefinitely rather
than fabricate an outcome. Existing native automatic reveals can occur while
submissions are paused, and the watcher continues to observe them.

`RevealWatcher::outstanding` exposes redacted pending diagnostics. Success
attribution requires a matching account/mechanism event plus one uniquely
matching parent-queue entry: the native event does not identify ciphertext.
Ambiguous events remain unresolved. An exact commitment still in its current
native queue is not declared lost merely because a predicted window ended.
A missed outcome records `native_queue_absent` only after complete observed
history lacks attributable success, the processing epoch passed, and the exact
entry is absent. It preserves its diagnostic evidence and does not mark the
application work completed or automatically replace it.
Completed reveal diagnostics use the existing 256-outcome ring; original
operation records, signed attempts and commit material remain in the journal.
Applications needing an indefinite reveal-diagnostic history must retain those
outcomes separately. Pending obligations are not subject to this ring limit.

## Pause, upgrades and storage

`OperationJournal::control` pauses new work and dispatch while retaining
observation and reconciliation. Generic transaction obligations can be marked
before pausing and explicitly allowed by `allow_obligations`; this does not
permit creating new operations while paused. Weight submissions stop under
pause; native chain processing continues independently.

A `SemanticGuard` can reject application meaning with an `Incompatibility`
code, descriptor and remedy. `Validator::weight_semantics` installs the weight
adapter. Its contract token is persisted; restart must reinstall that guard
before signing. Metadata shapes establish encoding compatibility, not unchanged
permission, reward, reveal or economic rules. Runtime version equality is not
an application-work lifetime requirement.

The native backend uses bounded JSON images, mode 0600 on Unix, synced
same-directory publication and generation CAS. It requires qualified local
filesystem locks, atomic rename and directory fsync. It is not a multi-host
lease service. Opening also synchronizes the resolved directory ancestry before
any journal publication, including existing directories left by an interrupted
first open. An ancestry-sync failure refuses opening and preserves existing
bytes; process-kill tests do not establish hardware power-loss qualification.
Images are capped at 64 MiB; exhaustion refuses new writes and
requires an explicit storage migration. It never deletes retained operation
idempotency records to regain space. Signed bytes, inputs and ciphertext are
sensitive: Debug omits them, but authorized storage exports still contain them.
No private signing keys or seed phrases belong in the journal.

Storage APIs are synchronous. Signed-attempt and receipt publication run on an
I/O worker, while readiness checks and scheduler checkpoints still do synchronous
I/O. The whole-image backend suits modest native workloads; applications needing
larger histories or nonblocking storage should supply a separately qualified
adapter. Recovery trusts the configured RPC's finalized chain view, as the
existing client does; this journal is not an independent consensus verifier.

The envelope supports atomic v1-to-v2 migration, preserving unresolved attempts
and unknown extension fields. Failed publication leaves the original image
intact; incomplete temporary files are not adopted as a new generation.
Unknown schemas and unsupported checkpoint fields refuse recovery without
resetting evidence. Validate a migration before starting new writes. This
journal cannot reconstruct material never persisted by an earlier in-memory
scheduler: reconcile such pre-journal activity independently before authorizing
new application work.

## Local executable and qualification limits

Run the maintained synthetic example with a new disposable path:

```sh
cargo run -p rtao-client --example journal_restart -- .local/recovery-demo/journal.json
```

It injects an acknowledgment loss after FakeChain inclusion, drops/reopens the
native storage handles and recovers twice with one submission. This simulates
caller restart state loss while preserving the external synthetic chain; it
is not an actual node, process-kill or power-loss test.

One separate fixture kills a child writer after signed-attempt publication,
then confirms OS-lock release and retained unresolved bytes/material. It does
not simulate disk hardware failure or keep an external live chain in that child.

Focused development fixtures exercise actual Rust client/encoder/signature/
receipt paths against the maintained FakeChain and actual local files. They
are not live Finney upgrade, actual target-Mac, disk hardware power-loss or
remote-filesystem evidence. Free ordinary hosted validation remains the
repository's authoritative full validation; none is launched by this task.
The initial `75a764d` checkpoint passed 14 journal tests (including its child-process
helper), 26 weight tests, three runtime tests, the existing 39 submission and
24 executor regressions, selected strict all-target Clippy, and native
CLI/neuron all-feature/all-target compilation. The offline example also ran.
These selected checks do not constitute full workspace or live-chain validation.
Its correction checkpoint passed 18 journal, 26 weight, 39 submission and 15
targeted inner-shield tests, plus strict selected all-target Clippy, native
CLI/neuron all-feature/all-target compilation, the offline restart example,
matrix consistency and formatting checks. An earlier bounded shield run stopped
after 32 checks while
`shield_preparation_precedes_fresh_nonce_anchor_and_finality_guard` was still
running; a separate 25-second run on the initial checkpoint also timed out.
Source investigation identified three sequential 60-second default finality
waits in that fixture. Those observations did not establish a deadlock.
The fixture now uses Tokio's paused clock and asserts the full unchanged
default wait elapsed before each refusal. Its exact test and all 33 shield
tests passed locally, with zero ignored tests. Production timers and checks
are unchanged. Journaled shield-pair recovery still requires a dedicated SDK
adapter; these test results do not complete that implementation requirement.
Native-chain queue deletion, expired cryptographic windows, registration or
permission changes and governance actions remain chain behavior. The SDK
cannot pause emissions, restore a deleted native commitment, guarantee archive
availability or certify unchanged economics from metadata alone.
