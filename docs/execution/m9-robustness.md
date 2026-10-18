# M9 robustness source review

Reviewed source at `e4322e6ca2ecdae25ec68a294c56b38550b4c063` on
2026-10-15. M9-01 now requires source review and ordinary hosted regression
coverage under Q30. No new fuzz campaign, local build, node or corpus replay
was run for this review. Earlier campaign reports describe their own source
snapshots and do not validate the current code.

A follow-up source review at `2f9a30d` read all nine target files, the complete
driver, committed seeds, actual production entry points and selected failure
regressions. The manifest, driver and target inventories agree. It identified
the unsigned seed gap and the historical mnemonic-KDF wording corrected below.
The seed and guard changes received a separate independent source review and
are committed. The ordinary seed regression now has authenticated hosted
execution below; standalone fuzz-build coverage remains unverified.

## Existing targets

The standalone `fuzz/Cargo.toml` declares nine targets. None needs a live chain,
wallet or external service. Their signing keys are public development keys.

| Target | Production boundary and asserted property |
| --- | --- |
| `metagraph_view` | Parses arbitrary SCALE against the committed metadata; parser errors are allowed and panics remain failures. |
| `json_rpc_frame` | Seeds pending reads/submits/subscriptions in the production offline dispatcher; binary and valid UTF-8 text frames must have identical observable results. |
| `btauth_headers` | Parses six independently length-prefixed headers with a fixed clock and arbitrary body. Exercises authentication without committing a nonce. |
| `btauth_verify` | Mutates real Sr25519/Ed25519 raw and wrapped signed requests; unchanged requests must satisfy their wrapping policy, and every accepted request must reject replay. |
| `btauth_server` | Runs the actual Tower admission layer over an in-memory service; requires caller insertion, policy refusal, body-size refusal and replay rejection. |
| `keyfile_detection` | Checks combined encryption detection against the three format detectors and exercises method classification. |
| `keyfile_json` | Bounds inputs to 64 KiB; accepted keyfiles must serialize and restore the same public key and crypto type. |
| `event_splitting` | Verifies a nonempty committed seed, then compares successful raw/owned event projections, phases, topics and record counts. |
| `unsigned_extrinsic_json` | Parses untrusted JSON and reconstructs it against fixed runtime, genesis and era context; accepted exports must preserve call, payload and signed/extrinsic inclusion fields on reimport. |

The RPC harness calls `Supervisor::inbound`, rather than a second JSON-only
parser. Its ordinary unit fixtures cover batch correlation, early notification
draining and malformed/error inputs. The btauth harnesses use real signature
verification and the ordinary nonce store and admission service. The event and
offline-payload harnesses authenticate against the retained v478 metadata;
inputs cannot select a different trusted context.

## Ordinary regression coverage

The normal hosted `check` job runs workspace all-feature tests. Its `ci` profile
excludes the four live e2e/interop/oracle/emulator binaries, while retaining the
following client tests:

- `rtao-metagraph/tests/decode.rs` compares all three decoders, checks truncated
  and trailing input, rejects native balance overflow and exercises arbitrary
  malformed bytes through a bounded property test.
- `rtao-rpc/src/session/frame_fuzz.rs` contains ordinary tests for the production
  dispatcher and its seeded state. The extra binary/text probe test is enabled
  by the workspace all-features selection.
- `rtao-btauth/tests/conformance.rs` and `tests/server.rs` cover real signatures,
  nonce identity/freshness, replay, policy/commit order, request-target bytes,
  body limits and limiter behavior.
- `rtao-core/tests/keyfile_json_diff.rs` covers format-prefix truncations,
  malformed and Unicode JSON, field precedence and host/portable differences.
  `rtao-wallet/tests/keyfile.rs` adds production file-size, regular-file,
  symlink-write and encryption-feature refusal tests.
- `rtao-client/tests/offline.rs` covers valid import/signing, JSON injection,
  bad external signatures, runtime/genesis changes, era expiry and native fee
  boundaries. Event regression assertions also run in the client test suite.

The current ordinary hosted checkpoint is recorded below. It validates its
workspace source selection, with the standalone fuzz-workspace limit retained.

## Hosted seed regression, 2026-10-16

[Run 38897437100](https://github.com/sam0x17/rtao/actions/runs/38897437100)
at `3772699612278d4592648d07232241430e96791b` passes all six jobs and 3,080
tests, with four skipped and nine doctests. Its main log explicitly records
`fuzz_unsigned_seed_reconstructs_signs_and_attaches_under_authenticated_context`
as PASS. That test imports the committed positive seed through authenticated
production reconstruction, signs and attaches it, decodes the remark, rejects
tampering and leaves nonce/submission state untouched. The current fixture
body is byte-identical to that executed source.

The bounded hosted-results proof, not retained in the repository, has SHA256
`fadc0d5af652d64707c474093ac6cf8c0bf2b60b647f51aab1f00360000c1eb4`.
No raw log is retained. This resolves the ordinary positive-seed witness gap;
it does not execute the standalone target, its initialization guard or a
sanitizer/mutation campaign, and does not accept M9.

## Coverage limits

The fuzz workspace is separate from ordinary nextest. A green workspace suite
does not establish a current fuzz build, sanitizer run or mutation coverage.
The unsigned JSON seed directory includes a hand-authored public immortal
remark candidate alongside the rejecting empty-object and overflow seeds. The
target initialization now requires that candidate to reconstruct under its
unchanged trusted runtime, context and anchor. An ordinary client regression
imports the same bytes, signs and attaches them, decodes the remark, and checks
tampering refusal, unchanged nonce cache and zero submissions. The candidate
and guards are committed source. The ordinary fixture can establish acceptance
of the same seed through production reconstruction, signing and attachment.
It does not execute the standalone target's initialization guard. The ordinary
fixture passed at the exact hosted checkpoint above.
No Python or interoperability result is implied.

The keyfile harness's 64 KiB input cap does not exercise the wallet's 16 MiB
file boundary; dedicated wallet fixtures cover that boundary. Public mnemonic
fixtures can run PBKDF2 seed derivation; no wallet-envelope decryption is
performed. No new campaign is required by the revised plan.
