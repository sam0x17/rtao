# Using rtao from an agent

Use the catalog to choose a semantic operation, plan it with explicit limits,
and request approval before acquiring a signing key. This guide follows the
upstream eight-step navigation and execution flow. The generated
[agent card](agent-card.md) lists commands from the current CLI tree.

## 1. Search the documentation

Start with [llms.txt](llms.txt) for navigation or [llms-full.txt](llms-full.txt)
for the combined reference. Use [concept links](concepts.md) for chain concepts
and [compatibility](compat.md) for the selected source and metadata pins.
Read a generated operation page before constructing its arguments; it links
the upstream description, native type and chain sources.

## 2. Discover tools and reads

`rtao tools` prints JSON even without `--json`. It retains the authenticated
operation catalog and adds a `rust` object. Check `rust.cli`: a null value
means that operation has no executable generated command in this build.
`Executor::tools()` exposes the complete upstream tool schema, so catalog
membership alone does not establish that a feature-enabled parser can execute
an operation.

The [intent](catalog/intents.json), [read](catalog/reads.json) and
[error](catalog/errors.json) catalogs supply machine-readable fields. Use
`rtao tx <operation> --help` or `rtao query <read> --help` for the installed
binary's arguments. Operation names use snake_case in the SDK and canonical
hyphenated spelling in the CLI.

## 3. Select name-based execution

For JSON dispatch, use the facade `Executor` with `intents` enabled, or attach
`rtao_intents::Registry` to the lower-layer executor. The execution entry point
is `execute_tool(op, args, keys, opts)`. The registry parses JSON into an
enabled intent and checks its argument shape. Supply the operation separately
from its argument object.

For a preview, parse the same invocation with `AnyIntent::from_tool` and call
`plan_dyn(intent.as_dyn(), keys, &opts)` on that result. Keep the normalized
arguments for the later invocation.
The [SDK overview](sdk.md) describes typed execution when your application
already knows the operation at compile time.

## 4. Preview before approval

Inspect `Plan::summary`, `args`, signer identity, fee, effects, warnings and
violations. Require `Plan::ok()` before proceeding. CLI mutations support
`--dry-run`, which prepares a preview without submitting or unlocking a
private signing key. Planning can make public chain and wallet reads.

Do not treat a preview as a reservation of balances or state. Bind a later
SDK execution to `Plan::call()` through `ExecuteOptions::expected_call` when
the approval must cover exact bytes. Re-review any changed call.

## 5. Bound the operation with Policy

Configure `max_spend`, `max_fee` and `allowed_netuids` in native units. An
unbounded operation fails a spend cap; an absent fee estimate fails a fee cap.
An operation spanning all subnets fails a subnet allowlist. Under an active
policy, raw calls require the separate `allow_raw_calls` opt-in.

Apply the same policy to planning and execution. A per-operation policy
replaces the default rather than adding limits to it. Policy checks precede
private signer acquisition. They constrain the intent's declared safety
contract; raw calls cannot supply an inspectable spend contract.

## 6. Branch on structured outcomes

After approval and execution, check both the Rust `Result` and the returned
`ExtrinsicResult::success`. Branch on `TxError::code()` or
`result.error.code`, and retain the chain name and remediation for the user.
Use `rtao explain <code>` or the [error pages](errors/index.md) for guidance.
Parsing diagnostic sentences couples the caller to presentation text.

Record the transaction hash and inclusion state. Reconcile an unknown fate
before retrying. A successful pending multisig approval does not mean the
underlying operation ran; see [multisig completion](multisig.md).

## 7. Make noninteractive approval explicit

Use `--json` for machine output and `--dry-run` for the preview. Submit the
reviewed mutation with `--yes` only after the caller's approval decision.
Unattended JSON and piped input require that explicit approval flag. Keep
network, account selection and arguments consistent between preview and
execution. The [CLI guide](cli.md) covers output, selectors and contacts.

In an SDK service, place approval in your own application flow. Restrict the
key source and signing capabilities to the task, retain the policy, and set
execution deadlines. Calling `execute_tool` allows it to request that
source's signer once validation succeeds.

## 8. Use lower-level access deliberately

For a missing named read, use `ViewExt` and runtime/storage descriptors on a
pinned view. For a call outside the semantic catalog, the executor exposes
`submit_call` and `submit_call_dyn`; these require the raw-call policy opt-in
when a policy is configured. Review the composed call and origin yourself.

Use the generated [reference](reference.md) before choosing that route. A raw
call does not acquire the semantic intent's fee/spend accounting or remediation
contract merely because its pallet and call name look familiar.
