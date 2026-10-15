# M5-02 batch, staking trades and identity source

Seven behavior ports have verified generated integration: Batch, AddStake,
AddStakeLimit, SwapStake, MoveSwapStake, SetIdentity and SwapHotkey.
Integrated coverage reaches 73/83 intents and 40/102 reads. Ordinary
compilation/tests and milestone acceptance remain pending.

The Python reference is `850f2755bb05a3851f948489f0ab27b0645cebce`.
The authenticated Batch source blob is
`efbab058438eba1017daecfd906a623d715f441c`, SHA-256
`01bdea99d5d5864db10f7f74d0fdff46e94ddc30999739b56c265c40a24a49fd`.
Identity uses blob `a4a2167fcbb2db93d649935c32705e5d6612b1e9`.
Registration and staking identities are recorded in
[m5-lock-basket-claim.md](m5-lock-basket-claim.md).

Batch normalizes child constructors, refuses nested batches and mixed signer
roles, builds semantic child calls against one public origin/view/runtime/clock,
and prefixes extras by child index and operation. It aggregates policy and
independent preview hooks. Full preflight passes the composed parent call to
every child, selecting the complete reserve triple from the first strictly
largest requirement without summing duplicate fee requirements.

The instance role is exposed through `Intent::signer_role` and the object-safe
forwarder. The generated associated role remains the catalog's Coldkey.
A hotkey Batch uses the dynamic executor API; typed entry points refuse a role
mismatch before key access. CLI wallet selection, payer lookup and lazy
acquisition follow the selected role. D-046 also covers Batch's virtual-array
depth, node, encoded-byte and retained-copy limits without cloning its child
slice before checking.

Five executor fixtures cover all typed entry points, public previews, policy
refusal and lazy acquisition for ordinary and shielded execution. A CLI fixture
uses a real hotkey Batch through both submission paths, checking the fee payer
in rendered JSON and RPC requests without unlocking or submitting. The manual
multisig adapter keeps Coldkey approval signing while exposing the semantic
role separately; its isolated hooks have a dedicated regression fixture.

Staking trades retain checked u64 money, root denomination, effective-origin
all-funds resolution, Python's floating-point limit calculation order and the
move/swap one-rao dust adjustment. Quote fixtures include an integer division
case that distinguishes correct rounding from premature binary64 conversion.
The pinned typed price API returns u64; malformed or absent data fails typed
decoding rather than reproducing the Python fake's special None diagnostic.

SetIdentity keeps UTF-8 wire fields and empty-field defaults. SwapHotkey uses
the legacy call with a narrowly scoped deprecation allowance and unchanged
warnings/policy. Its unused generated pass-through mapping is removed; the
SetIdentity mapping remains. Success address fixtures use supported SS58
formats 0, 2, 42 and 110.

The leasing generator also copies Percent and optional Copy arguments instead
of cloning them, correcting strict Clippy failures in normal run 38703332560.
No generated file is edited manually. Source inspection, rustfmt and whitespace
checks are local; substantial validation uses free hosted Ubuntu CI. No new
upstream filing, local build, service or temporary root is created.

Hosted generation
[38704675475](https://github.com/sam0x17/rtao/actions/runs/38704675475)
at `429f72787bd46d5304c3209f834c983cc65475aa` succeeded. Review verified
fourteen exact regular-file archive members, all 23 dependency hashes and
unchanged role package sets/counts 225/361/365/416. Output changes only seven
availability/pending entries, the identity helper, two leasing Copy arguments,
the guarded/canonical Batch constructor, normalized Batch JSON and the seven
matching intent matrix statuses plus D-046's additional fixture reference.
All other generated interfaces and matrix families remain unchanged.

The generated archive is 91,607 bytes, SHA-256
`7af5fb55b0d97a9db0063eceebbe645ddd652714434b03dd10cc36213bde73f3`.
The reports archive is 40,321 bytes, SHA-256
`5d05bc7d921266a569ec3c05e02a23656ce20f1acf693794fe34c8e58b0766ec`.
The bounded private proof is
`.local/recovery/2026-10-14/m5-batch-trades-identity-adoption.json`.
This establishes generated integration without runtime or oracle claims.
