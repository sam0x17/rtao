# rtao-primitives

Shared Bittensor identifiers, amounts and value conversions.

`AccountId` and `Ss58Address` represent accounts. `NetUid`, `Uid`, `MechId`,
`BlockNumber` and `H256` identify chain objects. `Balance<Tao>` and
`Balance<Alpha>` keep currency units distinct; `DynBalance` carries its subnet
and symbol at runtime. Balances store nonnegative `u64` rao, with checked
arithmetic and exact amount parsing.

`TaoBalance`, `AlphaBalance`, `PriceRao` and `StakeWeight` describe wire values.
Fixed-point and proportion types preserve runtime units. `IntoCoreValue`
converts supported values to the core's dynamic carrier.

Helpers in `pyfmt`, `json`, `drand`, `axon`, `evm`, `hyperparams` and `ratio`
cover formatting, exact JSON input, beacon arithmetic, endpoint construction
and metadata-directed unit conversion. These helpers perform no network I/O.

| Feature | Effect |
| --- | --- |
| `serde` (default) | Enables the feature-gated serialization implementations for accounts, balances and other primitive types. |
| `schemars` | Adds JSON Schema implementations for supported primitive types. |

Disabling `serde` does not remove the Serde dependency or serialization used
unconditionally by other helpers.
