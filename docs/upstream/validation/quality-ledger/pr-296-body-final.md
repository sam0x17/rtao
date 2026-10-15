Closes #295

Bittensor passes native TAO amounts to calls as `TaoBalance`, a `u64` newtype. The generic app currently displays staking 1 TAO as `amount_staked 1000000000`. This adds `TaoBalance` to the native balance types, so that amount displays as `amount_staked TAO 1.0` using the decimals and symbol from the metadata proof.

Subtensor also uses `TaoBalance` for `limit_price`. The formatter can inspect both the field name and type name, so this PR keeps the exact `TaoBalance` / `limit_price` pair raw:

- `add_stake_limit`, `remove_stake_limit` and `add_collateral` use a price in rao per alpha.
- `swap_stake_limit` and `move_stake_limit` use the destination-alpha per origin-alpha exchange ratio, scaled by 1e9. Both reach `get_max_amount_move`, which compares the input divided by 1e9 with `price(origin) / price(destination)`.

Applying the native `TAO` symbol to those values would omit or misstate their units. A 0.95 exchange ratio therefore remains `limit_price 950000000`. Native amount fields still receive TAO formatting. This conservative field/type check also works inside nested calls and avoids adding call-context state to the parser.

`AlphaBalance` stays raw because it represents a subnet's alpha token. `Option<TaoBalance>` stays raw under the existing parser rules; its outer type name differs and the `Some` field is unnamed.

## Tests

`tests/supported_chains/bittensor-477.json` contains five finney spec 477 vectors generated with bittensor-core v477 against V15 metadata at block #9276924, with mortal era period 64, nonce 7 and zero tip:

| Call | Expected display |
|---|---|
| `Balances.transfer_keep_alive` | `value : TAO 1.25` |
| `SubtensorModule.add_stake` | `amount_staked : TAO 1.0` |
| `SubtensorModule.add_stake_limit` | `amount_staked : TAO 1.5`, `limit_price : 50000000` |
| `SubtensorModule.remove_stake` | `amount_unstaked : 2000000000` |
| `Utility.batch` of add/remove stake | formatted `TAO 0.5` and raw `250000000` |

The transaction, extension, proof and digest bytes of those vectors are unchanged by the price correction.

`tests/quality/relative-price-v478.json` adds direct swap, direct move and a batch containing both. The self-contained native regression checks normal and expert mode, verifies each metadata digest, and requires all eight price displays to show the exact raw integer with no TAO symbol. The accompanying README records the generator API, metadata hash and mock account/genesis values. These are parser fixtures, not signed extrinsics or evidence of chain acceptance.

Validation on macOS arm64 with Clang 21:

- The original amount-only change at `9edd5fa` reproduced 5603 passing C++ tests; removing only its `TaoBalance` entry reproduced 5597 passes and 6 failures.
- The new ratio regression fails on `9edd5fa`: both calls, including the batch, display `limit_price : TAO 0.95`.
- Final committed head `8645ebdc31d188a722e22396d26fb52144246da4`: **5604 of 5604 C++ tests pass**. The direct native build uses the repository's app source list and definitions, fmt 7.1.2, GTest 1.14.0 and nlohmann_json 3.12.0, with archive hashes checked against the repository pins. No CMake, Hunter, Python or container was run for this committed-head check.
- Native clang-format 21.1.8 passes with the repository's `.clang-format`; `git diff --check` passes.

At publication, seven upstream workflow runs for this head report `action_required` without job results. Remote checks have not validated this commit yet.

The prior Speculos runs for nanosp/flex and historical finney signature validation described when this PR was opened tested the original amount-only commit. They were not rerun for this price correction. No fresh emulator or physical-device coverage is claimed for the final commit.

## Version and snapshots

`APPVERSION_P` remains 29, and `v100.0.29` already exists. The version check remains outstanding; the release version and corresponding device snapshots need a validated update before merge. This change adds native parser coverage and does not regenerate device snapshots.
