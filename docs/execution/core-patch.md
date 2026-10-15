# Ledger core patch maintenance

PR-U17 is [RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242).
The read-only check on 2026-10-15 found it open and unmerged, with one commit:
`eafe84e8a97376e7902118ebb204b9c53e3df55e`, directly on the pinned v478 commit
`850f2755bb05a3851f948489f0ab27b0645cebce`. This observation does not authorize
adopting a later PR revision without checking it.

The initial patch branch can reference that existing commit. `core-patch rebase
v478` reads the branch and PR, verifies the base, linear review series, permitted
source paths, regular-file modes and exact resulting tree, and prepares the two
Cargo patch tables and `core.patch` pin. It does not need a new commit, clone or
write credential on the laptop. The hosted command never pushes the fork.

For a future release or review change that needs new commits, the command prepares
a candidate under the hosted task root and exits with `core-patch-publish-required`.
The bounded evidence contains a mail patch, proposed identity and JSON hashes.
For an existing branch the mail patch contains only the tail after
`publication_parent`. An authorized publisher can apply it there and publish an
ordinary fast-forward. The proposed commit hash is advisory because applying a
mail patch can change committer metadata. Rerunning maintenance adopts only a
published tree that exactly matches application of the recorded PR diff and
preserves the pinned branch history. The tool never force-pushes or deletes refs.

The exact-tree check preserves whitespace inside Rust strings and byte literals.
It applies the recorded review diff to a separate Git index when the release base
has changed; the checkout index remains untouched. A conflict stops with
`core-patch-conflict`. Successful checks remove their disposable index and
worktree. The hosted task owns any retained conflict directory and candidate
evidence.

`pin --check` checks the patch base, paths, fork-only core lock entry, unchanged
upstream BLS/macro sources and the Clippy bans. Isolated hosted probes must compile
both injected transport API names against the patch, and fail against the
unpatched release specifically with unresolved names in the probe's own source.
A dependency, inference, TLS or resolver failure cannot prove API absence. A
successful unpatched probe reports `PR-U17 shipped: drop the core patch`.

`rtao-testkit::ledger_emu` supplies bounded replay and loopback TCP transports for
the core driver. Replay failures are sticky, every request must match and every
script must be exhausted. TCP reads use one total I/O deadline, include the status
word outside Speculos's data-length prefix, reject excessive lengths and retire
partially consumed streams. Authored fixtures cover chunk boundaries, address
parameters, status errors, malformed replies, a complete signer/proof-vector
path and fragmented, truncated, excessive and stalled TCP responses.

Hosted preparation [38730694470](https://github.com/sam0x17/rtao/actions/runs/38730694470)
passed the exact pin checks and the patched/unpatched API probes at `e775003`.
Its reviewed artifact changes only `Cargo.toml`, `Cargo.lock` and `upstream.toml`;
the sole lock identity change is `bittensor-core` to the reviewed fork commit.
Those candidate files are adopted without changing the base, BLS or macro pins.

The replay module is now gated by `ledger-emu` independently of Docker. The
ordinary all-features hosted suite includes its authored protocol fixtures, while
the live emulator binary remains excluded. The feature job then strips the patch
in its disposable checkout and checks shipped crates with the same build target.
These replay fixtures have not yet passed hosted compilation or execution.
No new emulator transcript was recorded, and no
hardware, HID, firmware, app acceptance or screen evidence is claimed. Relevant
hosted validation must also strip the patch and check shipped crates with all
features while excluding testkit and xtask. No node, Docker, emulator or hardware
run is required by this maintenance work.
