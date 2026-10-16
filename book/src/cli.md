# Command line use

Run `rtao --help` for the groups available in your build. Each command has its
own `--help`. Optional EVM, Ledger, Vault and extension support appears only
when the corresponding Cargo feature is enabled.

## Output and approval

Use `--json` for machine output. JSON uses two-space indentation, preserves
Unicode and keeps integer values exact. Finite floating-point values use
Python's notation. Human output escapes terminal control characters in names
and other supplied text.

`--quiet` suppresses chatter while retaining primary output and errors.
Transaction commands review their calls before signing. `--yes` approves the
review; unattended JSON or piped sessions must supply it for a mutation.
`--dry-run` prepares a preview without submitting. A successful multisig
approval can leave an operation pending, so inspect the result before treating
the underlying call as executed.

Balances use checked native `u64` rao. Amounts below one rao, negative amounts
and values outside that range are refused. EVM contract integers and public
book recovery coordinates have their own domains.

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

`rtao stake --help` includes add/remove/move/transfer/swap, whole-position exits,
position views, auto-stake and child-key settings. Interactive stake commands
can select a source position and fill both its hotkey and subnet. Destination
selection happens before an amount prompt. `rtao stake wizard` prints the
movement guide without connecting to a node. An exit across multiple hotkeys
uses one atomic batch and the ordinary shielding policy.

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
