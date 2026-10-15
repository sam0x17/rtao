# M5 final intent and CLI source

Ten remaining intent ports are prepared: RemoveStake, RemoveStakeLimit,
MoveStake, TransferStake, UnstakeAll, StakeBurn and the four raw multisig
operations. Generated adoption and ordinary hosted validation remain pending.
Integrated coverage stays 73/83 intents and 40/102 reads until adoption.

Claim-aware staking includes the complete atomic claim/follow-up construction,
admission and reserve preflight, original wrapped-call fee probes, root money
units, MAX/all distinctions, dust/legacy previews and conviction-transfer
warnings. Staking enables futures and the shared root-claim helper; LockState
joins the client's existing typed decoder registry. Source review corrected
two stale Python test class names in the earlier trade fixtures.

StakeBurn belongs to governance and works without the staking feature. It
rejects all, preserves validation and hotkey-read ordering, always carries a
price limit and retains Python's inherited preview. Raw multisig calls keep
explicit timepoints, full openings versus hash approvals, derived-account
child builds, funding/weight checks and base policy semantics. D-046 bounds
their constructor, typed JSON and nested input carriers. Typed output now
reads Python's multisig_call_hash/multisig_call_data hex fields while retaining
legacy native aliases. Pending inspection decodes against its pinned runtime.

The 20 M5 CLI routes are authored: seven stake aliases, two curated hparams
mutations, six proxy commands and five multisig commands. They reuse ordinary
SDK review/submission and shared persistence. Hidden st/buyback spellings are
covered. Proxy create/kill retains creation coordinates and does not invent a
fund transfer. D-047 keeps live entries after approval-only success. Multisig
pending authenticates cached, supplied or historical opening bytes; discovers
deployment proposals and treats bounded release HTTP enrichment as advisory.
D-048 distinguishes an unknown sudo layer from a verified unopened layer.
No HTTP enrichment or wallet operation was run during implementation.

M5-03 adds source fixtures covering all 21 supported actual core constructors,
481 real constructor/intent pairs, independent SCALE bytes, native boundaries
and public set_take queries through a bounded loopback HTTP fixture. Python's
ordered/duplicate subnet scope and the core's sorted set are asserted separately;
fee/spend/global diagnostics retain their exact order. Review corrected the
missing Delegates ValueQuery default to 11796. The retired constructor remains
excluded. These fixtures have not executed yet.

M5-04 plans all 83 authenticated INTENT_SAMPLES entries, all 12 supplementary
samples, six root denomination branches and a hotkey Batch through the real
executor. It requires complete catalog identity, successful preview, authorized
metadata call indices, Sudo inner calls, the expected public signer and no
private acquisition/submission. PoW uses one deterministic nonce-zero public
proof at difficulty one, with no worker. Positive subnet/hotkey rewriting keeps
required distinct relationships and root zero. The 72 catalog intent-plan
manifest keys point to this ordinary hosted fixture; their qualification still
requires the exact all-feature Cargo test and indexed body. Other submit/e2e
keys are not satisfied by this plan-only test.

Hosted docs tooling now produces an indexed transaction reference from the
catalog and pending register, with schemas, defaults, roles, calls, shield
settings and accurate CLI availability. Book/LLM outputs include it. The
inactive release draft copies the same page if it is activated later.

Source review and formatting are complete for the contributed files; combined
compilation, tests, doctests, rendered docs and milestone acceptance remain
pending. One bounded offline lock resolution adds no package versions: only
the existing reqwest dependency edge for CLI advisory HTTP is recorded.
No local build, test campaign, node, container, service or temporary root ran.
