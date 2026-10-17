# Core patch maintenance

The v478 core patch now contains two reviewed changes:

1. PR-U17, [RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242), adds the injectable Ledger APDU transport.
2. The M2-S02 repair tracked by [RaoFoundation/subtensor#3251](https://github.com/RaoFoundation/subtensor/issues/3251) gives the stored Ed25519 signing key a wiping owner.

The maintained fork branch is `rtao-core-patch/v478` at
`5a43348e8dc95311e9750781a28a33380c454409`. It is three commits ahead of the
pinned v478 base `850f2755bb05a3851f948489f0ab27b0645cebce`:

1. `eafe84e8a97376e7902118ebb204b9c53e3df55e`, the exact Ledger PR commit.
2. `774cc3ac78e442bfde7ef399b36f79e4bec4d460`, the Ed25519 ownership repair.
3. `5a43348e8dc95311e9750781a28a33380c454409`, the corrected wiping derivation path.

The combined diff is restricted to the six paths recorded in `upstream.toml`
and has SHA-256
`404c285c98003fb845eed23ec9e48640a25a8e5b570ac1b7312c2df6aa922f6e`.
The authenticated review identity separates `upstream_pr_rev` from the combined
`review_rev`, so a later Ledger PR update cannot silently replace the security
repair.

## Ed25519 ownership repair

The core stores Ed25519 private state as `Zeroizing<ed25519_zebra::SigningKey>`.
That signing object contains the seed, expanded scalar and nonce prefix and
implements `Zeroize`, so its final owner wipes the complete stored object before
the allocation is released. URI and mnemonic derivation use a private
`WipingEd25519Pair`; hard-derivation seeds and derived copies also have wiping
owners. Encrypted JSON import constructs the same stored type. Public key and
signature bytes remain unchanged, with an RFC 8032 seed/public/signature vector
guarding compatibility.

This repair establishes erasure for the core-owned stored Ed25519 signing
object. It does not establish erasure of allocator or OS copies, third-party KDF
internals, or every short-lived value inside cryptographic dependencies.
Static Ed25519 signature verification still uses `sp_core::ed25519::Pair` and
holds no private key.

## Provenance and maintenance

`core-patch rebase v478` verifies the base, linear review series, permitted
regular-file paths, exact resulting tree and combined diff digest. It validates
the Ledger API and the Ed25519 source structure, including the RFC vector. A
rebase must preserve both reviewed components unless each component has an
independently authenticated replacement.

For a future release or review change that needs new commits, the command
prepares a candidate under the hosted task root and exits with
`core-patch-publish-required`. The bounded evidence contains a mail patch,
proposed identity and JSON hashes. An authorized publisher can apply it to the
recorded publication parent and publish an ordinary fast-forward. The proposed
commit hash is advisory because applying a mail patch can change committer
metadata. The tool never force-pushes or deletes refs.

The exact-tree check applies the recorded review diff in a separate Git index
when the release base changes. A conflict stops with `core-patch-conflict`.
Successful checks remove their disposable index and worktree; a hosted task
owns any retained conflict directory and candidate evidence.

`pin --check` checks the base, branch, combined paths and digest, fork-only core
lock entry, unchanged upstream BLS/macro sources and Clippy bans. Isolated
hosted probes compile the injected Ledger transport names against the patch and
must fail against the unpatched release specifically because those names are
absent. A dependency, inference, TLS or resolver failure cannot prove API
absence.

A successful unpatched Ledger API probe now shows only that the Ledger portion
has shipped. It does not authorize dropping the combined patch. Remove the
Ledger delta only after authenticating its upstream replacement, and retain the
Ed25519 delta until its own upstream replacement has been reviewed.

`rtao-testkit::ledger_emu` supplies bounded replay and loopback TCP transports
for the core driver. Replay failures are sticky, every request must match and
every script must be exhausted. TCP reads use one total I/O deadline, include
the status word outside Speculos's data-length prefix, reject excessive lengths
and retire partially consumed streams. The ordinary all-features hosted suite
executes these fake-transport fixtures. No node, Docker service, emulator or
hardware run is required.

Hosted root-lock refresh
[39004028813](https://github.com/sam0x17/rtao/actions/runs/39004028813) and
TLS-fixture lock refresh
[39004030113](https://github.com/sam0x17/rtao/actions/runs/39004030113)
resolved the reviewed core revision. The root lock preserved all other package
identities and the fuzz lock stayed byte-identical. The TLS lock changed only
the core revision and its `ed25519-zebra` dependency edge; unrelated resolver
updates were excluded.

No new emulator transcript was recorded, and no hardware, HID, firmware, app
acceptance or screen evidence is claimed.
