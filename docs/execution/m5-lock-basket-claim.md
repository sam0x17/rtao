# M5-02 lock, collateral, leasing, EVM, basket and root-claim source

The batch implements seventeen operations: four lock, two
collateral, two leasing, two EVM, five basket/configuration and two root-claim
intents. Verified hosted output is adopted unchanged, bringing declared
implementation coverage to 66/83 intents. Ordinary hosted compilation/tests
and milestone acceptance remain pending.

The reference commit is `850f2755bb05a3851f948489f0ab27b0645cebce`.
GitHub source identities were authenticated against the exact vendored bytes:

| File | Git blob | SHA-256 |
|---|---|---|
| `intents/lock.py` | `f3ff3300f9029885fc1a82f1944f9c70cafbc82a` | `21e0dafeeadded015cff91cbe2a7bb11cdd1cdb7094cae5ddb66792e522e6aba` |
| `intents/collateral.py` | `93f4a53ec22db5cebd2723626b6096646f170ba3` | `83d357515686489d2530e0faca8d744b58dd6bef897462a6e3e0bacfd1479b96` |
| `intents/leasing.py` | `c1bf79b7bb6a5ebcec925010679de8b87f700115` | `234be85f4543df3ae70b1b4dbea2ef4d0d4298468da19f1d586de663b1adaf62` |
| `intents/evm.py` | `62274b85ee0d003ae650099b83bb1e73dba37533` | `ea5f6f57ca9b60ad38c4b4e5c338f25a01ee22e5e11ab192dc103011985830d5` |
| `evm/addresses.py` | `19c36c3d7411944184148a1e5cbd96f03819f178` | `dcbaeaa869cfd6f13f7b571a245257cc2202bed4ee3e28f1cc9c5e6e9eefe4df` |
| `intents/staking.py` | `82e885a8820b62d6d43dcf0ce278c6eea9886121` | `c72469c8877925b5de91d56bd513704da96a09cdb9f8c95952e71241870e7ea9` |
| `intents/registration.py` | `96645c419f9b5e95f9b0407164bdb0578c5efd85` | `3751d31317cc935e44f4205346b3394bb42dcecb46b27137eec809f8eba6b0fc` |
| `intents/_root_claim_fee.py` | `f59999129baaad88b743f2961a75d49fb03ee008` | `ffd560bcc0a68370743560cd29a1073f759f7d8ac365cb7ea3b051a53a74266f` |

Lock `all` uses the effective coldkey's runtime-reported available stake,
preserving root's TAO denomination before conversion to the call's alpha wire
newtype. Collateral `all` subtracts the hotkey's locked collateral from its
stake, then obtains a spot-price limit with Python's floating-point operation
order and checked u64 bounds. Leasing keeps its percent validation, wallet
defaults, unbounded registration spend and chain admission rules.

EVM funding uses the core's BLAKE2b-256 and SS58 codecs for the mirror account.
Its pre-deserialization hook preserves address-before-money validation and
canonical address storage; typed JSON output uses the same normalization.
Withdrawal uses the effective coldkey's truncated
address and reads the mirror's balance only for `all`. Pinned mirror vectors
and independent SCALE layouts are in the fixture source.

The five staking operations cover reward routing, alpha sweeping, basket
deposits and ordered basket swaps. Multi-leg swaps retain Python's leg
normalization, reject empty or oversized batches, and preserve touched-subnet
order. Shared stakeable-TAO arithmetic is enabled for staking only after this
batch supplies actual consumers. Existing reviewed pass-through maps remain
used, avoiding duplicate unused generated builders.

Root claims include admission selection, both work budgets, watermarks and
mandatory fee-payer reserve checks. Exact wrapped-call fee probing has the
pinned fallback; dust-aware and legacy payout previews preserve their distinct
failure behavior. Fee/payout fixture source includes independently recorded
node quotes. The helper is currently compiled under registration; staking-only
exposure awaits the claim-aware staking consumers.

Source review corrected candidate references to nonexistent account/balance
methods, removed unknown-prefix success assumptions and encoded lock fixtures'
native integer map keys through metadata. Ordinary money JSON now follows
`intents/base.py::_encode` and `Balance.decimal`: insignificant fractional
zeroes are omitted from exact amount strings. Display balances retain their
fixed-nine-place presentation. Native u64 bounds and arithmetic are unchanged;
fixtures cover zero, whole units, fractions, tagged amounts and u64::MAX.

No local build, test, Python process, scratch root, node, wallet operation or
upstream filing ran. Source inspection and formatting cannot establish hosted
test success, fresh oracle interoperability or milestone acceptance.

Hosted generation
[38702003174](https://github.com/sam0x17/rtao/actions/runs/38702003174)
at `7d842e95945bab2d5fc0e537c4bd668a79abc6e6` succeeded. Review verified
fourteen exact regular-file archive members, digests/sizes, all 23 dependency
hashes and unchanged role package sets/counts 225/361/365/416. Generated
changes are exactly seventeen pending removals and availability changes,
eleven call-map additions, two constructor hooks and two normalized JSON
interfaces. Only those seventeen intent statuses change in the matrix;
other interface and matrix families remain byte-for-byte unchanged.

The generated ZIP is 91,676 bytes, SHA-256
`775e73b20106c40121719163af4c84061fbf8817b5798c42ac316acd9b2da2ac`.
The report ZIP is 40,301 bytes, SHA-256
`79dc6aa200f6645dbccdb02293363ed2789a124a6e777a905be38a008af1daaf`.
The bounded private proof is
`.local/recovery/2026-10-14/m5-lock-basket-claim-adoption.json`.
No runtime or oracle validation claim is added by these generated artifacts.
