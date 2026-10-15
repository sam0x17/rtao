# M4 CLI machinery

The recovered CLI source adds command builders, flag tiers, context, prompts,
output, logging, read-only books and raw-call execution. The committed entrypoint
registers `call`, `config`, `tools` and `explain`, and retains the offline `doctor`
checks. Tx/query and context integration now includes verified hosted generated
APIs; weights, axon and implemented hyperparameter routes await normal hosted
checks. Other command families remain
incomplete. This source does not complete M4 or establish passing combined checks.

The ports identify upstream commit
`850f2755bb05a3851f948489f0ab27b0645cebce` in their attribution headers. Book name
suggestions also derive from CPython 3.12.12 `Lib/difflib.py` at
`4a5632fbf9bf59477c540e3f53fa7cdbeea3e3f5`, source SHA-256
`3eb2d371b4a171b063921de6bbf115108bc39de8bf72866315868ce2599213b7`.
The CLI retains the full PSF license and its package license clause. The parity
gate checks the exact source attribution, license bytes and root NOTICE section.

`Globals` separates root defaults from command overrides and retains the
tri-state shield choice. Command tiers expose options the handler can use;
raw `call --signer` and `--key` belong to that command's parser. Aliases attach
recursively after registration, including future generated commands. Required
input prompts use clap's value parser. Missing input in piped, JSON or quiet
sessions returns status 2; mutation confirmation without `--yes` in piped or
JSON sessions returns status 1. Cancellation returns status 130.

Wallet and account resolution reads public companions, address/proxy books and
saved multisig descriptions without unlocking. Destination fields never default
to a local key. CLI key sources unlock lazily, retain a successfully loaded
signer and retry a wrong prompted coldkey password twice. Environment, file,
Keychain and dialog passwords retain their provenance and fail once. The
library defaults to `PromptPolicy::Never`.

Hidden entry uses `rpassword` through an injected terminal interface, with
`IfTty` eligibility and retries for empty or mismatched passwords. Keychain and
native dialogs require explicit flags and a caller-supplied provider. Process
execution uses argv without a shell and bounds captured secret buffers.
`security` save uses bare `-w` with password and retype on stdin. Captured native
errors are omitted from diagnostics because they can contain a secret.
Development checks use injected process runners; Q30 defers native Keychain
acceptance and prohibits changing host Keychain settings for rehearsals.

Raw calls validate generated descriptors against metadata and retain bytes
stamped with the runtime version. Wrappers compose inside out: call, Sudo,
Proxy, Multisig, then shield. Dry runs validate without acquiring a private
signer. Automatic later approval rounds retain the original inner call and
opening timepoint. A pinned pending-state read refuses absent, replaced or
already-approved state and encodes `Some(expected)`, preventing a new opening
after the operation disappears (D-041).

D-041 is registered as active source in `parity/divergences.toml`, with regression
names covering client composition, raw approval rounds, saved semantic approvals
and completed owner multisig execution. Hosted validation of the combined source
remains pending; no upstream filing is claimed.

Saved semantic multisig submissions explicitly disable owner-rate replanning.
The final approval can consume pending state even when its inner owner call
fails; retrying that approval could open another operation. Included matching
approval events authenticate the opening for later rounds. Inferring a new
opening from receipt height assumes the pinned runtime uses
`BlockNumberProvider = System`. An automated source guard for that assumption
has not been adopted; future runtime pins need source review.

Recovery hooks run after source acquisition and before signing, then after
release and receipt enrichment, before rendering. Previews skip the hooks.
A release error stops later semantic approvals. A retained receipt supplies
the call hash and recovery command if a later step fails. External signer
factories are injectable; unavailable backends refuse explicitly. Their command
integration remains later work, and Q30 defers hardware/browser rehearsals.

`Output` preserves parsed JSON values and existing plan, receipt and error
shapes. The prepared context uses `plan_and_preflight_dyn` to obtain the plan
and structural facts from the same snapshot and call, then includes those facts
in human review. This integration has not compiled or run. Native balances use
explicit display adapters where read JSON expects a Python Balance string.
Diagnostics and tracing go to stderr. `BITTENSOR_LOG` overrides the verbosity
ladder; ordinary debug verbosity omits raw frames.

Book files have a 16 MiB limit; programmatic JSON and multisig reference
recursion have a depth limit of 128. Oversized inputs refuse rather than change
the signing identity. D-042 registers these input budgets and their named
regressions, including direct programmatic raw-call nesting refusal. Registration
does not claim a passing hosted run.
Replay hints omit known secret flags, including separate and attached
`--json-password` values. Public password-file paths remain visible. Secret
warnings list flag names only.

The adopted test sources exercise clients and the executor over `FakeChain`,
committed SCALE metadata and injected terminals, processes, caches and signers.
Entrypoint fixtures redirect all seven shared-file paths and avoid network or
native password access. Applicable tests and builds run on free hosted
`ubuntu-latest` CI. No local build or test ran during adoption, and historical
isolated results do not prove the current combined source passes. Python
oracle, interop and node scenarios are optional targeted hosted evidence under
Q30; native Keychain and device acceptance remain deferred and unverified.

Recovery member hashes and adoption paths are recorded under
`.local/recovery/2026-10-14/cli-adoption.json`. Command-family integration and
applicable hosted checks remain pending. D-041 and D-042 are registered and
included in the checked-in parity matrix. The prepared tx/query corrections and
remaining gaps are recorded in [m4-generated-review.md](m4-generated-review.md).
Replay strings use `rtao`; comparison with Python's embedded `btcli` name requires
the recorded replacement rule.
