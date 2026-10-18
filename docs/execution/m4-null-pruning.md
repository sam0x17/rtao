# M4-12: sequential Null pruning

Status: source implemented and all four recent fixtures passed hosted CI.
Milestone acceptance and the complete pytest
mapping remain separate. No local build,
test, mining operation or chain service was run for this follow-up.

Current ordinary [38865058430](https://github.com/sam0x17/rtao/actions/runs/38865058430)
passes all six jobs at exact `3f1aa67`: 3,040 tests passed, zero failed and four
skipped, plus doctests and documentation. Bounded named-result evidence (not
retained in the repository) binds the four additions below to exact PASS
records and source blobs. Its authenticated
715,611-byte log has SHA256
`55de87df70e9afc863da7175a9dc7ed809ce239c310f8b2e2eef0b84ef46e975`.

Hosted [38858957306](https://github.com/sam0x17/rtao/actions/runs/38858957306)
passed at `50b7239e6ec1160cc482a86be8d2a30fe737ffc1`: all 3,014 executed tests
passed (four skipped), alongside docs, doctests and all six jobs. Bounded
named-result evidence authenticates all 33 SDK pruning fixture identities
and seven CLI pruning fixtures at that source. The main log is 710,339 bytes,
SHA256 `15e295426302f686b8cef1918bf91bc2050f97588eca73028870037c2cd82357`.
These are FakeChain/client results; live runtime execution is unverified.

`hparams trim-null` uses the same `TrimNullSession` as `OwnerOps::trim_null`.
The CLI drives each guarded action through `AppContext::submit_with_options`,
which retains transaction review, proxy dispatch, saved multisig approvals and
the selected signing backend. The session owns state decisions and validates
finalized progress; the CLI does not copy the pruning algorithm.

The command accepts the pinned Python options: netuid, optional target,
`--switch-to-yuma`, wait timeout, poll interval, batch limit and recovery file.
The target defaults to an existing on-chain target or the current Yuma ceiling.
The default journal path uses Python's SHA-256 chain/subnet identity beside the
shared CLI config. An explicit `--state-file` uses the supplied recovery path.
The wait budget starts on the first required wait, matching Python. Polls,
changed wait reasons and cancelled read-only waits share that deadline. Each
accepted finalized batch clears the timer, including an unchanged batch; the
next required wait starts a fresh interval. Three consecutive batches without
progress still stop independently. The last sleep is limited to the remaining
time so the driver can reread state before deciding whether another wait is
required. Failed submissions and pending approvals do not clear the timer.
Each submitted step also receives an executor deadline, unless an explicit
submission deadline overrides it. The executor drains attempted delivery when
that deadline expires; the CLI never cancels a submission future to implement a
polling timeout.

Before private signer acquisition, the journal records the reviewed outer call,
runtime versions, semantic operation and pruning state. Known raw receipts reach
the journal before external signer release, multisig follow-up or output errors.
Saved approvals retain their group, inner call and opening timepoint. A pending
approval does not count as a pruning batch. An ambiguous attempt blocks another
broadcast until its inclusion or expiry is reconciled. Confirmed pending
approvals may continue only for the same guarded operation and opening. After
each approval, the shared session verifies canonical finalized inclusion,
the original opening from the receipt's matching event and extrinsic position,
and equality with pending storage before permitting the next signer. Explicit
reviewed timepoints must also match. Restart repeats this receipt check, so a
same-block cancellation and replacement cannot silently change the journal's
opening. An inconsistent older journal remains for operator reconciliation.
The executor also binds its rebuilt outer call to the exact reviewed bytes and
runtime versions before acquiring the private signer.

A successful batch requires finalized canonical inclusion, exactly one matching
`NullUidsPruningProgress` event, at most 64 removals, and corresponding storage
at the inclusion block. Receipt success alone cannot complete the workflow.
Changed ownership, mechanism count or pruning target stops execution. Three
finalized batches without progress also stop. Finalized administration-window
races can wait and replan for direct submissions; saved multisig failures do not
automatically reopen approvals. The journal remains after uncertain delivery or
unverified progress.

Consensus stays Null unless `--switch-to-yuma` is supplied. The shared session
checks every mechanism's timelocked-commit prefix with bounded probes and sends
further cleanup batches while queues remain. The Yuma setter is reviewed through
the same CLI route and must be observed in finalized storage. A shield carrier
receipt cannot prove that inner progress, so this workflow requires
`--no-mev-shield`.

The focused CLI fixtures cover direct and proxy batches, journal creation before
private signing, exact reviewed call bytes, native counts, an unsigned preview,
ambiguous delivery and missing-progress restart refusal, and an opt-in Yuma
switch. A preview reports a closed administration window without waiting for it
to open. A two-member saved multisig fixture executes both approval rounds in
one command, retains the opening and counts only semantic execution. SDK
fixtures also check confirmation between approval rounds, state changes during
an approval and changed-call refusal before private signer acquisition. All
fixture filesystem output is confined to `RTAO_TEST_TMPDIR` and removed by the
fixture.
The durable user recovery record and its stable sibling lock are intentional
runtime state.

Final source review replaced a fixed 32-attempt journal limit with the validated
multisig group's signatory count, preserving the pinned runtime's support for
100 members. Journal reads reuse the bounded nonblocking wallet reader and
refuse content over two MiB before decoding. The `50b7239` source and fixture
suite passed hosted compilation and execution. No dedicated exact 100-member
approval or oversized-journal boundary fixture is claimed.

The separate Python test row remains pending: exact admin-window table,
cooldown/queue transcripts, recovery cases and `// pyt:` provenance still need
completion. Native D-062 closed-window preview refusal and stronger canonical
receipt/journal requirements are explicit policies, not identical Python tests.
Passing current fixtures does not establish milestone acceptance.

Four subsequent independently reviewed fixtures add actual session coverage for
mechanism2/implicit target128, pending-target priority without a local record,
all32 ordered native key-only probes and empty TWC values at the final legacy
index. The default/resume cases execute one real reviewed signed FakeChain
batch. Probe cases stop before signing and assert no commitment value reads.
The pending128 and final-index vectors are explicit native adaptations; the
cooldown half of the shared T19 provenance remains pending. All four additions
now pass compilation, strict Clippy and execution at exact `3f1aa67`. The exact
admin table, integrated cooldown/recovery and full pytest mapping remain open.
