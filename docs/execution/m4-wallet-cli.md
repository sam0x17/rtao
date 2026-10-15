# M4 wallet command source

`commands/wallet.rs` implements the eleven M4 wallet commands: create,
new-coldkey, new-hotkey, regen-coldkey, regen-hotkey, regen-coldkeypub,
regen-hotkeypub, show, list, balance and inspect, plus the rtao-only
`save-password-env`. Binary/module routing and dependency resolution are
prepared for hosted validation.

Key writes use the existing wallet library, explicit overwrite options and
injected password callbacks. Coldkeys are encrypted by default; hotkeys are
unencrypted. Creation validates both requested crypto schemes before writing
and checks all four destinations before a combined create without overwrite.
Generated mnemonics appear in the documented creation payload only. Recovery
accepts one mnemonic, 32-byte seed, 64-byte private key or coldkey JSON export;
hidden input is never added to replay history. JSON imports retain encrypted
bytes and passphrases in wiping storage, enforce a 16 MiB input limit and use
the library's redacted derivation errors. Public regeneration checks the ss58
address against the supplied public bytes before writing a companion.

Show and list inspect public identities without unlocking. List projects only
public fields from bounded file reads; encrypted or malformed hotkeys remain
listed with a null address. Lock artifacts and public hotkey companions are
excluded. Saved multisigs retain precedence in show and appear alongside the
address and proxy books in list. Public-unavailability diagnostics use fixed
messages rather than copying keyfile parsing errors into output.

Balance and inspect compose existing reads with the generated stake-availability
runtime API and dynamic root-basket positions. One snapshot supplies every
read. JSON preserves the distinct single-balance, all-wallet and inspect row
shapes, Python balance strings and root-yield fields. Locked stake remains part
of the alpha valuation. All-wallet totals count each coldkey once. Checked
native u64 arithmetic refuses overflow. Dust and empty-row filtering affect
human output only.

Natural sorting supports arbitrary-length Unicode decimal runs and Python
casefold behavior. It needs `caseless = "=0.2.2"` and
`unicode-general-category.workspace = true` in the CLI manifest. The source
checks Unicode 16 folding and Unicode 15 category versions at compile time,
then leaves characters unassigned in Unicode 15 unchanged. A read-only
comparison of Unicode's 15.0.0 and 16.0.0 CaseFolding data found no changed
mapping for a character assigned in 15.0.0. No Unicode table is copied into
the repository.

Integration hooks are `pub mod wallet` in `commands/mod.rs`,
`wallet::command()` in the root builder and
`wallet::run(&mut app, matches, &mut invocation.history).await` in dispatch.
Resolve root and deepest-leaf globals before calling the handler. The existing
empty reserved-field list suits these commands. Tests can inject a FakeChain
client through `wallet::run_with_client` without opening network connections.

The new command and read fixture files cover real task-owned keyfiles,
mnemonic-derived identities, overwrite refusal, secret-safe regeneration,
watch-only companions, partial wallets, Unicode order, alpha/beta/locked
accounting, root yield, snapshot pinning, duplicate coldkeys, empty JSON rows,
dust and overflow. They clean their exact fixture directories and never use
native password providers.

The positive encrypted-JSON CLI fixture uses the exact public version-3,
base64 sr25519 export from `polkadot-js/common`
`packages/keyring/src/index.spec.ts` at commit
`9281daa14c2c2a41708c3489de8770a064525577` (Apache-2.0, copyright 2017-2026
@polkadot/keyring authors and contributors). Its source SHA-256 is
`03512b88212344c3f7fd4fbe847fbf1b55c99d41ab749d5fd07090dcbeafbe43`;
the unchanged JSON fixture SHA-256 is
`eeab2b83e4cf25ec651a7645ce705e6066adaa12787717b3934b68ac090fbf74`.
The fixture writes that export only beneath its task-owned root and runs the
real wallet handler with an injected hidden import passphrase and a separate
storage password. It checks the independently recorded identity, the export's
scheme despite a conflicting CLI hint, default NaCl encryption, decryption
through a fresh wallet handle, secret-free output/replay and overwrite refusal
without changing either keyfile or the export.

Installed rustfmt and source inspection are the local checks; no build or test
ran. Hosted CI must compile and execute this batch after the integration hooks
and dependency lock update land. M4-11 and the complete CLI parity gate remain
unverified.

Independent source review found that the two new CLI file readers could block
while opening a FIFO. Both now use `rtao::wallet::read_keyfile_data`, a public
reexport of the existing bounded, zeroizing, nonblocking wallet reader. It
checks the opened descriptor and preserves regular symlink targets. A Unix
real-binary fixture creates task-owned FIFOs without writers, bounds each
child to five seconds, and kills/reaps a regressed child before cleanup.
The all-wallet balance fixture also checks that empty address strings are
skipped, as in Python. These fixtures have not run locally.

`save-password-env` resolves the explicit CLI password sources and prints the
existing core-compatible encoded coldkey export. It never changes process
environment variables, opens a chain connection or unlocks a signing key.
Shell-invalid environment names refuse before password resolution; JSON mode
returns the same name, encoded value and export text. A fixture checks both
renderings, decoder compatibility, unchanged process state and invalid-name
refusal. Encoded output is intentionally secret-bearing and reversible.

Lock resolution added only `caseless 0.2.2`; the 29,309-byte crate archive
matched lock SHA-256
`8b6fd507454086c8edfd769ca6ada439193cdb209c7681712ef6275cccbfe5d8`.
Source confirms the required API and Unicode 16 table. Independent comparison
of Unicode 15/16 CaseFolding and Unicode 15 DerivedAge verifies all 1,530
Unicode 15 common/full mappings are unchanged, while the 27 new Unicode 16
mappings belong to characters unassigned in 15. The category filter therefore
preserves Python 3.12 casefold behavior. Only bounded hashes/results are retained
under the private recovery directory; no Unicode tables or archives were
extracted into the workspace. The changed lock/manifest still need hosted
compatibility generation and all applicable checks before acceptance.
