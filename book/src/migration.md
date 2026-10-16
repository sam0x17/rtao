# Migrating to rtao

Match the chain and Python pins in [compatibility](compat.md) before translating
an application. The [Python and Rust API index](api-index.md) maps every watched
source file, while the [alias index](aliases.md) maps mirrored item spellings.
File reconciliation records source review; it does not certify that every
operation is available or that an oracle or runtime test ran.

## v11 Python

| Python pattern | rtao pattern | Migration decision |
|---|---|---|
| Client with named read namespaces | `Client` plus `Namespaces` | Import the namespace trait and await the read. |
| `client.at(block)` | `Client::at(BlockRef)` and `Snapshot` | Retain one snapshot for all reads in a calculation. |
| `Wallet` used by a mutation | `Wallet`, `KeySource<R>` or `DynKeySource` | Preserve the operation's hotkey/coldkey role and defer unlock. |
| Unit-tagged `Balance` | `Balance<Tao>`, `Balance<Alpha>` or `DynBalance` | Enforce checked native `u64` rao and explicit units. |
| Intent dataclass | Typed intent struct or `AnyIntent::from_tool` | Check the generated page's feature and source status. |
| `plan` and `Policy` | `Executor::plan`/`plan_dyn` and `Policy` | Inspect violations and preserve all limits during execution. |
| `execute_tool(op, args, wallet)` | `Executor::execute_tool(op, args, keys, opts)` | Use the facade registry and provide application approval. |
| Result object and `raise_for_failure` | `ExtrinsicResult` and `raise_for_failure()` | Check chain failure inside `Ok`, inclusion and unknown fate. |

Use the generated [transaction](tx/index.md) and [query](query/index.md)
references for individual parameter and output mappings. Catalog `rust`
metadata identifies the native type or read method; Rust API feature gates
determine what your build exposes. Dynamic JSON retains catalog shapes where
documented, while typed APIs use native identifiers, enums and errors.

## v10 Python

| v10 responsibility | rtao home | Application work |
|---|---|---|
| `Subtensor`/`AsyncSubtensor` chain access | Async `Client`, or optional `blocking::SyncClient` | Choose execution mode and own connection shutdown. |
| `Axon.attach` handlers and serving | `Miner` and `MinerRuntime` | Define axum routes, explicit application state and caller admission. |
| `Dendrite` requests/fanout | `btauth::client::SignedClient` | Select axon targets, receiver binding, deadlines and response schema. |
| `Synapse` model and body hash | Application request/response types plus btauth | Serialize and sign the exact transmitted bytes; define schema validation yourself. |
| Metagraph arrays and periodic sync | `Metagraph`, `MetagraphSync` and UID-indexed scores | Handle changed hotkey identities and removed/reused UIDs. |
| Validator loop and `set_weights` | `Validator`, `NeuronRuntime` and weight intents | Separate scoring from cadence, mechanism and commit-reveal handling. |
| Extrinsic method kwargs | Intent plus `ExecuteOptions` | Select policy, wait conditions, deadlines and failure handling explicitly. |
| Unbounded Python balance arithmetic | Native balance types | Reject negative, sub-rao and overflowing chain amounts before encoding. |

The [miner guide](miner.md) explains routing, replay protection and service
limits. The [validator guide](validator.md) covers framework state, metagraph
refresh and weight scheduling. Replacing a Synapse with a Rust struct does not
establish wire compatibility with an existing subnet protocol: compare exact
request bodies, routes, authentication and response handling in focused
[protocol fixtures](testing.md).

Btauth/1 signs the method, request target, raw body, nonce and identities. The
optional `legacy-v10` verifier accepts a schema-specific body hash supplied by
the application. It is a verification bridge, with no automatic Synapse
serialization or body-hash implementation. Review the peer protocol before
enabling it. Existing v10 weight preprocessing and defaults also need review;
use the current intent contract rather than carrying old fallback behavior
into a new validator.

## The core's blocking Rust client

| `bittensor-core` pattern | rtao pattern | Behavior to review |
|---|---|---|
| `Client::connect(endpoint)` blocks immediately | Async `Client::connect(ClientConfig)`, or cold `SyncClient::new` | Async connection awaits; the blocking facade connects on first use or `connect()`. |
| Direct typed helper methods | Namespace methods and generated query reference | Parameter and return types can differ. |
| Borrowed core snapshot | Owned `Snapshot` or `SyncSnapshot` | Retain its block/runtime context and shared connection lifetime. |
| Core runtime/value codec | `rtao::core` plus `ViewExt` descriptors | Reused codec primitives do not replace client metadata gates. |
| Manual call construction and signing | Intent plus `Executor` | Preserve signing role, origin, policy and runtime-stamped call review. |
| Core transaction outcome | `ExtrinsicResult` and `TxError` | Inspect semantic failure, wait state, nested completion and fate reconciliation. |
| Blocking loop around HTTP reads | Async tasks, or the facade's private worker | Avoid blocking an application's async executor with synchronous calls. |

With the `blocking` feature, `SyncClient` runs the same async implementation on
a lazy private runtime thread. Values crossing that thread are owned. Closing
a started shared facade invalidates its clones and snapshots. `Subtensor`
can select blocking mode on first blocking use or async mode when awaited;
all clones share that choice. Construct a fresh instance to change modes.

## Deliberate differences

The CLI shares current JSON configuration with btcli. It ignores the old
`~/.bittensor/config.yml` file and prints a migration warning once when that
file is present, including during help, version, JSON and quiet invocations.
Move the settings you need with `rtao config set`, then rename or remove the
old YAML file. The warning never reads its contents. Deleting
`~/.bittensor/.btcli_legacy_config_warned` lets it appear again. The warning
skips symlinked legacy directories and files. Only a regular, unsymlinked
marker suppresses it; failure to create the marker leaves the warning enabled.

For saved public contacts, `rtao addr NAME SS58` and
`rtao addresses NAME SS58` retain the old shorthand for `addr add NAME SS58`.
The installed executable is named `rtao`, so Python distributions competing
for the `btcli` console script do not affect its command entry point.

Read the generated [divergence register](divergences.md) before treating a
source correspondence as behavioral equivalence. It records the affected
contract, reason, status and pinning fixtures. Native amounts, typed errors,
transport limits and external-signer capabilities can impose boundaries that
Python did not express in its type system.

For example, D-035 records checked native `u64` chain balances. D-042 bounds
root-command decimal normalization input and exponent size; shared money
constructors retain their stricter grammar. Those policies affect valid input
domains even when an accepted call encodes the same chain bytes. Use the
[parity guide](parity.md) to distinguish source mapping, authored fixtures,
executed Rust checks and independent oracle evidence.
