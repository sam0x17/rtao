# rtao-catalog

Generated intent, read and error catalogs from the pinned Bittensor sources.
These tables describe the upstream interfaces used by the client and CLI.

`INTENTS` and `READS` expose parameter schemas, Python defaults and provenance
through `IntentSpec`, `ReadSpec` and `ParamSpec`. `SEMANTIC_ERRORS` and
`CHAIN_ERRORS` retain descriptions and classifications; `ErrorCode` is the
shared semantic error vocabulary.

`py` holds network presets and extracted Python tables. `config` records
configuration paths and defaults, `hyperparams` records parameter metadata,
and `owner_map` records owner setter and cooldown rules. `rt` contains
runtime-derived constants; `runtime_py` contains metadata-fetch and cache
format constants.

The crate declares no Cargo features and performs no network I/O. Catalog
membership describes an interface; implementation coverage is tracked
separately in the workspace parity records. Regenerate derived tables through
the workspace extraction and generation tools when the upstream pin changes.
