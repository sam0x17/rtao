# M4 subnet command source

The CLI source routes subnet creation, neuron registration and metagraph
inspection through existing SDK intents and reads. Registration selects root,
burned or public CPU PoW behavior, retaining explicit hotkeys and the effective
proxy or saved-multisig owner. CPU worker/time bounds survive into late proof
refresh. GPU support remains an M7 gap with an explicit refusal.

Metagraph machine output retains the complete typed payload. The human view
adds stake ordering, names and IPv4/IPv6 endpoints. Native u128 volume counters
remain exact; balances keep checked u64 rao. Source fixtures cover semantic
intent routing, public PoW preparation/refusal, rendered names/endpoints and
large counters without starting real nodes.

Under D-044, `subnets register --pow --dry-run` refuses before searching for a
proof and points to `tx pow-register` for an existing-proof preview. Other dry
runs use SDK planning. CLI hooks, module routing and tests are prepared; no
local build, test or mining ran, and hosted acceptance remains pending.

The sequential `hparams trim-null` source now uses a shared SDK/CLI session.
It preserves per-approval review, saved multisig openings, ambiguous-delivery
recovery and finalized progress checks before another batch. Its fixtures
still require successful hosted execution; see [m4-null-pruning.md](m4-null-pruning.md).
