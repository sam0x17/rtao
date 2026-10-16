# SDK overview

The `rtao` facade connects the asynchronous client, typed reads, transaction
executor and optional operator framework. Select the features your application
uses; the [Rust API docs](api/rtao/index.html) describe the enabled modules.
The [getting-started guide](getting-started.md) contains source-backed examples.

## Connections and consistent reads

`Client::connect(ClientConfig)` opens the RPC connection and loads its runtime
context. Reuse a client across readers and executors so they share connection
state and the nonce manager. `Client::close` closes the shared connection;
clones do not create independent sessions.

`View` is the transport-independent read contract implemented by `Client` and
`Snapshot`. The `Namespaces` trait adds named groups such as `balances()`,
`staking()` and `epochs()`. The [query reference](query/index.md) records each
read's arguments, result and underlying storage or runtime API.

A client read resolves its head when needed. For calculations combining several
reads, use `Client::at(BlockRef)` and retain the resulting `Snapshot`. Its block
hash and runtime context stay fixed across reads. Choose finalized state when
your calculation needs finality; a best-head snapshot can belong to a fork.
Historical queries also depend on the endpoint retaining that state.

`ViewExt` exposes typed storage descriptors, map iteration and runtime calls
when the named reads do not cover an operation. Keep the same snapshot through
those calls. Raw transport access does not supply the semantic checks of a
named read or intent.

## Wallets, sources and signing

`WalletRef` describes the public wallet layout. `Wallet` binds its hotkey and
coldkey files without unlocking them at construction. Public key access and
planning can still read public wallet information from disk.

`KeySource<R>` associates a source with the compile-time `Coldkey` or `Hotkey`
role. `DynKeySource` selects the required role at runtime for JSON tools and
CLI operations. A `Signer` supplies the public identity, signing algorithm and
supported signing capabilities. It may hold a software key or delegate to an
external device. A capability refusal is an error, even if the account matches.

The executor requests a signer lazily after planning and policy checks. Keep
the coldkey off an unattended miner or validator where the operation only
needs its hotkey. The [signer](signers.md), [Ledger](ledger.md) and
[multisig](multisig.md) guides cover external signing and recovery records.

## Units and intent arguments

`Balance<Tao>` and `Balance<Alpha>` distinguish token units and store checked
native `u64` rao. Alpha balances also carry their subnet identity.
`DynBalance` carries a runtime-selected unit within the same native range.
Use checked arithmetic and exact decimal input at application
boundaries. A floating-point display value is unsuitable as a lossless balance
record. `NetUid`, `MechId`, `Uid` and `AccountId` retain separate identities.

An `Intent` is a semantic operation with a signing role, composed call and
operation-specific output. Typed intent structs provide Rust field checking.
`AnyIntent::from_tool` accepts a catalog operation name and JSON arguments;
its parser is limited to implemented operations enabled in the build. The
generated [transaction reference](tx/index.md) records the native type, feature,
role and schema for each operation.

## Plan, approve and execute

Construct `rtao::Executor` from the shared client. With the `intents` feature,
the facade attaches the generated registry for `execute_tool`. A bare
`rtao_tx::Executor` needs a registry attached explicitly for JSON dispatch.

Call `plan` for a typed intent or `plan_dyn` for a dynamic one. A `Plan` records
the signer, exact runtime-stamped call, arguments, estimated fee, effects,
warnings and violations. Planning does not unlock a private key or submit the
call. `Plan::ok()` means the recorded policy and preflight permit signing;
state and fees can change before submission.

Set `Policy` limits before asking for approval: `max_fee`, `max_spend`,
`allowed_netuids` and the explicit `allow_raw_calls` opt-in. An unavailable fee
estimate fails a configured fee cap, and unbounded spend fails a spend cap.
A per-operation policy replaces the executor's default policy, so include
every limit you intend to retain in that override.

Review the plan, then call `execute`, `execute_dyn` or `execute_tool` with the
same arguments and policy. SDK callers provide their own approval boundary;
the executor does not open an interactive confirmation prompt. Set
`ExecuteOptions::expected_call` from the approved `Plan::call()` when approval
must bind the exact composed bytes and runtime stamp. A changed call requires
a new review. Configure deadlines and the required inclusion/finality wait
explicitly for your application.

The [agent guide](agents.md) connects discovery, schema selection and JSON
execution. The [transfer reference](tx/transfer.md#use-with-executor) links this
flow to one concrete operation. Shielded and multisig operations have additional
transport and completion semantics in their respective guides.

## Outcomes and errors

`execute` returns a `Result<ExtrinsicResult<_>, TxError>`. A Rust `Ok` can contain
a failed chain outcome. Check `success` or call `raise_for_failure()`, then
inspect `inclusion`, `error`, operation-specific `output` and `data`.
Pool acceptance alone does not establish successful dispatch. A multisig
approval can succeed while its inner call is still pending.

Branch on `TxError::code()` or `ChainError::code`, using the semantic
`ErrorCode`; retain the exact chain name and `remediation()` for diagnostics.
The [error reference](errors/index.md) and `rtao explain` describe these codes.
When `fate_unknown` contains a transmitted transaction hash, reconcile that
attempt before deciding whether to send another one. A timeout does not prove
that the chain rejected the call.
