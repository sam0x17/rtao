# Command line use

Run `rtao --help` for the groups available in your build. Each command has its
own `--help`. Optional EVM, Ledger, Vault and extension support appears only
when the corresponding Cargo feature is enabled.

## Output and approval

Use `--json` for machine output. JSON uses two-space indentation, preserves
Unicode and keeps integer values exact. Finite floating-point values use
Python's notation. Human output escapes terminal control characters in names
and other supplied text.

Connection errors name the selected network. For a custom RPC URL, the label
omits credentials, path, query and fragment; the connection still uses the
configured endpoint. This protects endpoint tokens in human and JSON errors.
For custom endpoints, shared co-signer and upgrade hints require `BT_NETWORK`.
Set it privately to the intended RPC endpoint; an unset or empty value stops
the command before invocation.

MEV-shield carrier fees require free TAO; alpha cannot pay them. A failed
carrier message follows the transport actually used after fallback. Required
shielding cannot be disabled. An unknown outcome requires transaction
inspection before another submission. Canonical receipt JSON retains the
upstream Payment fields; supplemental carrier guidance is human output.

`--quiet` suppresses chatter while retaining primary output and errors.
Transaction commands review their calls before signing. `--yes` approves the
review; unattended JSON or piped sessions must supply it for a mutation.
`--dry-run` prepares a preview without submitting. A successful multisig
approval can leave an operation pending, so inspect the result before treating
the underlying call as executed.

JSON transaction receipts stay on stdout. If a submitted transaction's outcome
is unknown, stderr also carries a JSON diagnostic with its hash and inspection
guidance. Check that transaction on chain before resubmitting. Quiet mode
retains this recovery information.

`--signer extension` still uses terminal transaction review and confirmation
before requesting a signature in the wallet popup. `--yes` skips that terminal
prompt; it does not approve the wallet signature request. JSON or piped
mutations need `--yes` as well. This native confirmation rule is stricter than
btcli's popup-only transaction approval (D-068).

Balances use checked native `u64` rao. Amounts below one rao, negative amounts
and values outside that range are refused. EVM contract integers and public
book recovery coordinates have their own domains.
TAO/alpha amounts under `rtao tx` and amounts for `root allocate` and `root swap`
accept decimal exponents, underscores and Unicode decimal digits, such as
`1e-9`, `1_0` and `١.٥`. Input is limited to 4,096 UTF-8 bytes and exponent
magnitude 4,096. Normalization preserves exact rao values.

## Discovery and account names

`rtao tools` prints the full semantic transaction catalog. Check each entry's
`rust.cli` field for a command available in this build; a null value means no
executable generated command is available. `rtao tx <operation> --help` and
`rtao query <read> --help` expose their arguments.
`rtao explain <code>` describes known semantic and chain errors. The
[transaction reference](tx-reference.md) and [generated reference](reference.md)
describe the SDK operations behind these commands.

The [agent guide](agents.md) connects catalog discovery, schemas, planning,
policy, approval and execution. The [migration guide](migration.md) explains
the v11, v10 and blocking-core API changes.

Use `rtao addr --help` to maintain named public contacts. `rtao proxy book
--help` manages saved pure-proxy recovery details; `list --verify` checks live
state without changing the book. Keep these files with your wallet backups.
Address-book names, local wallet/hotkey names and explicit addresses resolve
through the shared account lookup. External-signer contact import requires an
explicit source selection.

## Wallets and stake

`rtao wallet --help` includes key creation and recovery, public overview,
registrations and history, message signing and verification, sealed message
encryption and decryption, transfers, identity and key swaps. Message decryption
prints plaintext unless a requested clipboard destination succeeds. Protect its
output as you would the original message.

Wrong-password errors identify the selected password source and explain how
to correct it. Prompted coldkeys allow three attempts. Hotkeys and explicit,
environment, file, Keychain or macOS-dialog passwords attempt once. JSON and
quiet mode suppress intermediate retry errors and retain the final diagnostic.

`rtao stake --help` includes add/remove/move/transfer/swap, whole-position exits,
position views, auto-stake and child-key settings. Interactive stake commands
can select a source position and fill both its hotkey and subnet. Destination
selection happens before an amount prompt. `rtao stake wizard` prints the
movement guide without connecting to a node. An exit across multiple hotkeys
uses one atomic batch and the ordinary shielding policy.

After a successful root unstake with an explicit hotkey and without `--claim`,
rtao checks for remaining basket yield. A claim pays that validator's whole
entitlement. Interactive sessions can offer a separate claim, which still
requires its ordinary transaction review. `--yes` and unattended sessions
never claim automatically; visible human output gives guidance. The lookup
and guidance use the account that dispatched the unstake, including proxy
routing (D-073).

## Other groups

`rtao misc --help` includes denomination conversion, latency to existing
endpoints, crowdloan views, weights, axon announcements, collateral, timelocks,
upgrade inspection and the optional extension bridge. Latency connects to the
chosen endpoints. Upgrade checks validate supplied artifacts before opening a
chain connection; signing rechecks the live proposal after confirmation.

The [signer guide](signers.md), [Ledger guide](ledger.md), [EVM guide](evm.md),
[multisig guide](multisig.md) and [shield guide](mev-shield.md) cover their
respective transaction flows.

## Shell completion and compatibility spellings

Generate completion source for the commands in your installed build:

```sh
rtao completions bash > rtao.bash
rtao completions zsh > _rtao
rtao completions fish > rtao.fish
rtao completions powershell > rtao.ps1
rtao completions elvish > rtao.elv
```

Load the resulting file using your shell's completion setup. Generation is
offline. Hyphenated commands also accept hidden snake_case spellings. Historical
group aliases remain available, including `wallets`, `st`, `subnet`, `sudo`,
`addresses` and `utils`; help displays canonical names.

Hosted Rust fixtures check the implemented tree against the pinned Python
source registrations and replay committed vectors. Fresh Python/Click dumps,
device runs, native macOS/ARM execution and live-chain interoperability remain
separate, unexecuted coverage under the current development scope.
