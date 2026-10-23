# v482 source reconciliation candidate

This record covers source identities, extraction producers and declared native
homes for parity v482 (`8cf229f8ef969389d054a4c161b53ffd164a041f`), compared
with v481 (`6a972693ae9580541225bd37696ac2fee26b8ae9`). It does not approve
the inherited production port at `320a4d8`. Independent semantic review of
that port remains **OPEN**. The core stays at v478 with its existing patch.

## Evidence and method

The base SDK is `42b2c19ebb4207dc7fa537e890d6dc51d21a7f35`; the input
candidate is `69ba38df8361dda00ba5cee17451dbd522f64faa`. Both pinned
upstream Git trees are compared with the watched listings, then every
vendored watched artifact is checked by Git blob and the SHA256 in its
corresponding `upstream.toml`. This uses already retained sources only.

The per-file inventory records old/new blob and SHA256 identities, native
homes and extraction-fragment comparisons. A fragment is an existing normal
extractor output object with source provenance. Its semantic comparison drops
only `provenance` objects, retains other fields and compares the sorted
multiset of file-qualified fragment hashes. This limited comparison does not
normalize arbitrary changed values or treat absent projections as evidence
of equivalence. Whole output hashes are also retained; pin/provenance changes
mean the complete extraction is not byte-identical to v481.

[`parity/v482-sources.json`](../../parity/v482-sources.json) records 332
exactly unchanged and 30 changed watched sources, all 388 pinned artifacts,
the metadata hashes and 41 producer/configuration inputs. The 30 changed
sources also carry hashes of their actual v481-to-v482 diffs.
[`parity/v482-decisions.json`](../../parity/v482-decisions.json) records each
changed-path mapping decision.
Unchanged source identity carries forward only the previous declared source
scope. It does not certify a modified native consumer. Metadata projections
are checked by normal regeneration from the pinned V15/V16 inputs; description
producers additionally require normal leaf/assembly extraction and catalogs.
Neither producer check is bypassed or replaced with the source inventory.

Of the unchanged sources, 72 have exactly equal recorded extraction fragments;
256 have no such fragments and carry only their exact unchanged source identity
plus the applicable normal checks. Four conservative whole-fragment comparisons
differ and require these explicit, narrower decisions:

| Unchanged source | Exact comparison and remaining difference |
| --- | --- |
| `common/src/transaction_error.rs` | All 29 native custom-error values and the Python mirror's values match exactly; the mirror's source line offsets move after unrelated new error-map entries. |
| `pallets/subtensor/src/coinbase/reveal_commits.rs` | `reveal_contract.values` matches exactly. The separately named core provenance changes tag/commit while retaining the same core source blob. |
| `sdk/python/bittensor/intents/base.py` | All 85 existing intent inherited-field maps match exactly. The new release intent adds five uses of existing identical declarations: origin, verify, amount fields and both shield defaults. Thus aggregate uses increase from 379 to 384 without changing those defaults. |
| `sdk/python/bittensor/intents/hyperparameters.py` | All `owner_map.values` match exactly. The complete output retains changed source-list and metadata provenance, including its new finalized-head identity. |

Source and value comparisons are recorded separately. For example, the changed
account read's extracted parameters/storage hits remain equal even though its
unextracted return-value behavior changes. No equal-fragment result is used as
a general proof of behavioral equivalence.

## Semantic boundaries for independent review

- `run_coinbase.rs` changes Null-epoch slot ordering. The mapped
  `blocks_until_next_auto_epoch` client projection is unchanged; no local
  scheduler or runtime emission execution is certified.
- V2 announcements send the cleartext destination account. Python intent
  prose still describes a simple stored hash while the core e2e fixture
  computes a destination-and-policy commitment. The client computes neither
  commitment. The runtime owns it, and live swap/recovery/reservation lifecycle
  validation stays deferred under Q30.
- Dynamic Validate/Weights signer selection and direct-only hotkey cleanup,
  recovery-lock optional fields, failed included receipt fees/events and all
  eight omitted identity fields belong to the inherited client port. Source
  mappings identify the existing consumers; independent port review remains
  necessary even if source parity checks pass.
- Metadata/catalog coverage alone does not prove dynamic signer selection,
  chain permission filters, charging/refunds or runtime storage effects.

## Validation limits

Normal offline `parity bless`, matrix generation and `parity check --strict`
pass. The normal producer guards recompute extraction/catalog outputs and
metadata/interface generation; no checker or producer logic changes in this
follow-up. Probe-pin and compatibility-output checks also pass. Exact record
and log hashes are in `parity/v482-validation.json`; these are local source
checks, not new runtime tests. No runtime range, attested runtime
catalog row or validation report is added: `validated_spec_range` remains null,
`validated_runtimes` stays empty and the oracle remains unverified. No new
full-suite, target-Mac trial, remote CI, live probe, localnet or milestone
acceptance is claimed. The earlier missing-environment failures are not rerun.
