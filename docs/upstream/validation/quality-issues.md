# Independent audit of submitted upstream issues

Audit date: 2026-10-14. The audit retrieved the actual posted bodies for
[subtensor#3244](https://github.com/RaoFoundation/subtensor/issues/3244),
[#3245](https://github.com/RaoFoundation/subtensor/issues/3245),
[#3251](https://github.com/RaoFoundation/subtensor/issues/3251), and
[#3252](https://github.com/RaoFoundation/subtensor/issues/3252), then compared
their claims with the upstream v478 source and lockfile at
`850f2755bb05a3851f948489f0ab27b0645cebce`. None of these four filings submits
a code fix. The first two request a supported API or release contract; the
last two report security findings that remain unresolved upstream.

The source findings are supported. One provenance correction is required:
#3251 names Ed25519-Zebra 4.2.0 in its v478 source chain, while the upstream
v478 lockfile actually selects 4.1.0. The original downstream probe used the
same `sp-core` Git revision with compatible Zebra 4.2.0. Both Zebra versions
have the reported secret fields and lack a wiping destructor, so the finding
survives the correction. #3252's final mitigation sentence should also say
that disclosure is blocked through the tested signer, wallet and codec
boundaries: the SDK publicly re-exports upstream `Keypair`, so direct core
API use still retains upstream diagnostics. Both corrected bodies are
prepared locally; this audit has made no GitHub changes.

The final direct core integration reproduction passed all three tests on
Rust 1.99.0 and Rust 1.89.0 with upstream's unchanged v478 lock. Another audit
detected artifact reuse between sibling worktrees sharing the target
directory, so these final runs use the unique wrapper path
`upstream-quality/wrappers/base-v478.sh` to partition workspace artifact
hashes while retaining external dependency caches. The final binaries are
`secret_uri_diagnostics_repro-84243b4dc22c4bae` (current) and
`secret_uri_diagnostics_repro-d7e8514119299998` (MSRV). Both logs show actual
compilation from the base-v478 core source. Earlier shared-cache runs are
retained as provisional evidence and are superseded by these isolated runs.
The new proof invokes the actual core constructor.

| Filing | Independently checked evidence | Remaining validation |
| --- | --- | --- |
| #3244: selector discovery | Exactly 77 source selectors, all literal index/name arms agree with declaration order; direct SCALE decoding verifies both `Vec<u16>` input types and absence of the selector enum from actual V15/V16 registries on both toolchains. | There is no submitted runtime fix to validate. |
| #3245: metadata assets | Live v478 release has exactly the five named assets and no raw metadata asset; the pinned workflow uploads the same inventory; a fresh archive RPC capture at the exact historical block matches both metadata files byte for byte, spec 478, and the quoted code hash. | This requests an export contract. Export from the released WASM has not been implemented or claimed complete. |
| #3251: Ed25519 erasure | Core owns the pair directly without an enclosing wiping destructor; URI construction retains neither optional secret buffer; both Zebra versions have the reported secret fields and no wiping destructor; the actual upstream-lock test confirms the Copy/Drop distinction on both toolchains. | Correct the Zebra version/probe provenance. No actual signing-object erasure fix has been submitted. |
| #3252: URI diagnostics | Actual core construction reproduces phrase/password disclosure for both algorithms in both Display and Debug, with a valid password-URI control, on both toolchains. Existing rtao source and current/MSRV test logs support its owned boundary mitigation. | Limit the final mitigation sentence to those boundaries. |

## Selector discovery

The relevant sources are
[`metagraph.rs:488`](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/rpc_info/metagraph.rs#L488),
its private `from_index` at line 569, and the runtime declarations in
[`runtime-api/src/lib.rs:62`](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/runtime-api/src/lib.rs#L62).
An independent literal-source comparison found precisely indices 0 through
76, from `Netuid` to `CollateralEarned`, without a mismatch. The response
struct already derives `TypeInfo` and names its optional fields; those names
describe output fields, not the numeric input-selector mapping.

The rtao generator actually parses the pinned source with `syn`, rejects
discriminants/non-unit variants, and compares the source arm map with the
enumeration before emitting its own selectors. Its mutation regression is
`selective_source_mapping_must_match_variant_order`. The graph decoding
claims have earlier runtime/metagraph validation evidence in
[m1-metagraph.md](../../execution/m1-metagraph.md); they concern successful
decoding, not live discovery of input selectors.

The live bodies of prior #1457 and #1810 match their description in #3244.
An independent all-state issue search also found #2089 (netuid output
inconsistency), #1589 (efficient validator filtering), and #1979 (adding
commitments). Those do not request metadata-visible input-selector discovery.
This is bounded search evidence, not a proof that no differently worded
duplicate can exist.

## Metadata release assets

The live release API reports target commit
`850f2755bb05a3851f948489f0ab27b0645cebce` and exactly these asset names:
`pending-release.json`, `proxy_proxy_blob.hex`, `subtensor-digest.json`,
`subtensor.wasm`, and `upgrade-manifest.json`. The pinned workflow's
`assets=(...)` upload list agrees.

Independent raw-file checks obtained:

| Version | Bytes | SHA256 |
| --- | --- | --- |
| V15 | 400,688 | `17cdbed812cb59a833cc64d2764d734aee5d08174385868167a1cf284d6ff502` |
| V16 | 404,727 | `723240b9f3761915eed75a5e158484bfdb39f6eea8ddd06d32e77996e67b14c7` |

The audit repeated the historical network capture through
`https://archive.chain.opentensor.ai:443` at the exact quoted block
`0x2bc979722ab7c83b9992900c5463dfd1daebbac77d5e64ae1e45413c69e5401f`.
`state_getRuntimeVersion` reports spec 478; `state_getStorageHash(":code")`
returns the quoted `0x7e722b3aa5c9192c98665a5d65fa91364bf9055e21fc90efce6d59bf4ef414e1`.
The two `Metadata_metadata_at_version` responses were strictly unwrapped
from `Some(Vec<u8>)` and compared byte for byte with the retained captures,
including the entire lengths and metadata version prefixes. Both match.
The audit has not executed the released WASM. The issue expressly
distinguishes its Finney availability observations
from the requested release-build export. The pinned runtime implements
`metadata_at_version(version) -> Option<OpaqueMetadata>`, supporting the
proposed unwrapping semantics.

Prior #384, #910 and #3159 were read directly. They concern error metadata,
polkadot.js type definitions, and a missing compiled signing hash,
respectively. The independent all-state issue search found no other request
for this release asset contract among its seven results.

## Signing-object erasure

The exact upstream lock records `sp-core 37.0.0` at
`cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a`, Zebra **4.1.0**, and Schnorrkel
0.11.5. The pinned Ed25519 `Pair` owns `SigningKey` directly and derives
`Copy, Clone`. Zebra 4.1.0's `SigningKey` holds `seed`, `s`, `prefix`, and
`vk`, derives `Copy, Clone, Zeroize`, and does not implement `Drop` or derive
`ZeroizeOnDrop`. Its expansion code computes the cached scalar and nonce
prefix from the seed. The downstream 4.2.0 source has the same relevant
fields and absence of Drop.

The Sr25519 pair owns Schnorrkel's `Keypair`; its explicit `Drop` invokes
`zeroize`, and its `SecretKey` uses `#[zeroize(drop)]`. This source inspection
establishes the reported distinction independently of `needs_drop`.
`needs_drop == true` alone does not prove wiping, and `needs_drop == false`
does not prove every released byte remains unchanged. The posted issue
already excludes freed-memory reads and optimizer guarantees.

The current-main branch API independently returned the same v478 commit
during this audit. Its fetched keys source has the same SHA256
`46ed45c6926538876af8d45ae099a1f39bff7a54f8ce0cc20ba9e1907591357d`,
supporting the issue's dated current-main observation.

## Secret-URI error disclosure and SDK mitigation

`Keypair::from_uri` calls the real `sp_core::Pair::from_string` and embeds its
Debug error in a `CoreError::Crypto(String)` for both algorithms.
`InvalidCharacterInfo` copies the complete initial URI. The parser excludes
a newline from the password and then constructs an input-bearing error from
that initial URI. CoreError Display writes the Crypto String; its derived
Debug also exposes it.

The reproduction uses only the public BIP39 test phrase beginning
`abandon abandon` and `public-disposable-password`. A valid password URI is
checked against `Keypair::from_mnemonic(..., Some(password))`; adding one
newline supplies the failing input. No wallet, environment secret or freed
memory is read. The actual integration test checks the returned Crypto category plus
phrase/password containment in actual core Display and Debug for both
algorithms. It passed on Rust 1.99.0 and Rust 1.89.0 against upstream's own
unchanged lock.

The direct core proof command, run from the detached v478 checkout, is:

```sh
export RTAO_TEMP_ROOT=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC
export CARGO_TARGET_DIR="$RTAO_TEMP_ROOT/build"
export TMPDIR="$RTAO_TEMP_ROOT/tmp"
export PYO3_NO_PYTHON=1
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export RTAO_QUALITY_METADATA_ROOT=/Users/sam/workspace/rtao/vendor/metadata/finney
export RUSTC_WORKSPACE_WRAPPER="$RTAO_TEMP_ROOT/upstream-quality/wrappers/base-v478.sh"
cargo +1.89.0 test --locked --offline -p bittensor-core \
  --no-default-features --test secret_uri_diagnostics_repro -- --nocapture
```

Repeat the same command with `cargo +stable` for the Rust 1.99.0 run. The
target also checks the actual locked pair type and decodes the actual portable
metadata registries. Both runs report **3 passed, 0 failed, 0 ignored**.
The isolated current run emitted a warning that the local toolchain's
`rust-objcopy` could not strip debug information because `libLLVM.dylib` was
unavailable. Compilation and all three executable tests nevertheless
completed successfully; this is not a warning-free Clippy claim. The MSRV
run completed without that warning. The upstream lock SHA256 remains
`c2008ec9d5e290a65ad4357e4ccdcf7caf2b0b74568a81efddfa9760ded2ca3a`.

The existing rtao mitigation uses `redacted_derivation_error` to wipe the
discarded owned error String and return a fixed Crypto diagnostic. Signer
and wallet constructor source both use it. The independent audit read
successful current/MSRV logs for:

- `constructor_error_chains_redact_uri_phrase_password_and_seed_sentinels`
- `valid_password_uri_derivation_remains_identical_to_the_pinned_core`
- `regeneration_error_chains_redact_disposable_secret_input_sentinels`
- `encrypted_import_error_chains_redact_json_and_passphrase_sentinels`

Those tests substantiate the stated SDK boundary mitigation. Direct use of
`rtao::core::keys::Keypair` bypasses those constructor adapters and retains
upstream diagnostics. They do not
establish erasure of the opaque Ed25519 signing object or all dependency
parser allocations. The issue's final sentence should limit its mitigation
claim to those tested owned boundaries. The two security findings remain
separate.

## Retained evidence and hygiene

Public API JSON, exact source snapshots, source hashes, the Rust integration
test, historical RPC captures and their strict verification script, and
both locally prepared issue corrections are retained under
`target/execute-plan.6Ro0PC/upstream-quality/issues-audit`. The source and API
evidence is bounded to approximately 3 MB. Cargo uses the shared task build
root, `TMPDIR` remains beneath the recorded temporary root, and
`PYO3_NO_PYTHON=1` is mandatory. No local Python, containers, commits,
pushes, new filings, labels, or review requests are part of this audit.

The exact detached `base-v478` checkout was removed with Git's worktree
command after the isolated reruns. Its only untracked change was the
executed proof source, retained byte for byte in the evidence directory.
Cleanup found no matching processes, local open handles, active runtime
configuration references, launchd references, incoming workspace symlinks,
or virtual environments. The one checkout symlink targeted its own tracked
`.agents/skills` directory. `lsof` warned about an unrelated unavailable SMB
mount; the audited checkout is on the local filesystem. Other audit owners
reported no filesystem dependency on this base checkout.

The common bare Git repository, sibling worktrees, recorded build cache and
shared wrappers remain live dependencies of the parent task. No unrelated
user work, wallet, private dataset, model asset, or daemon state was removed.
