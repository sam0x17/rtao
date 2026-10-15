# M4-12: sequential Null pruning

Status: source implemented. Hosted compilation, tests and acceptance
remain pending. No local build, test, mining operation or chain service was run.

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
Each submitted step also receives an executor deadline. The executor drains
attempted delivery when that deadline expires; the CLI never cancels a
submission future to implement a polling timeout.

Before private signer acquisition, the journal records the reviewed outer call,
runtime versions, semantic operation and pruning state. Known raw receipts reach
the journal before external signer release, multisig follow-up or output errors.
Saved approvals retain their group, inner call and opening timepoint. A pending
approval does not count as a pruning batch. An ambiguous attempt blocks another
broadcast until its inclusion or expiry is reconciled. Confirmed pending
approvals may continue only for the same guarded operation and opening. After
each approval, the shared session verifies canonical finalized inclusion,
matching approval events and pending storage before permitting the next signer.
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
refuse content over two MiB before decoding. These source corrections also
await hosted validation.

Formatting and source review do not establish milestone acceptance; the new
fixtures require hosted compilation and test results.
