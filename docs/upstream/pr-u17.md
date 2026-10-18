# PR-U17: injectable Ledger APDU transport

Filed 2026-10-14: [RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242).

| Item | Verified value |
|---|---|
| Upstream base | `850f2755bb05a3851f948489f0ab27b0645cebce` (`main`, also `v478`) |
| Fork | `sam0x17/subtensor`, confirmed fork of `RaoFoundation/subtensor` |
| Review branch | `ledger-apdu-transport` |
| Review commit | `eafe84e8a97376e7902118ebb204b9c53e3df55e` |
| Patch | [`pr-u17.patch`](pr-u17.patch), `git format-patch` of the review commit |
| Diff | Only `sdk/bittensor-core/src/signers/{hid,ledger,mod}.rs`: 107 insertions, 5 deletions |

The change follows research J §8.1: a public, ungated `ApduTransport: Send + Sync`; the existing HID exchange body implements that trait; `LedgerDevice` boxes the trait and gains `with_transport`. `open()` keeps its API. The prototype's short-answer wording is corrected to "APDU response was too short" for injected channels. No dependency or lockfile changes.

Current validation used Rust 1.89.0 on macOS arm64. Cargo build and test commands used `--locked`; all build artifacts and temporary files stayed under the task root. Binding builds used `PYO3_NO_PYTHON=1`, so PyO3 selected its abi3 configuration without running a Python interpreter.

| Command | Result |
|---|---|
| `cargo +1.89 test --locked -p bittensor-core --all-features --lib` | 74 passed, 0 failed, 1 ignored |
| `cargo +1.89 test --locked -p bittensor-core --all-features --lib signers::` | 5 passed, including all three new tests |
| `cargo +1.89 clippy --locked -p bittensor-core -p bittensor-core-py --all-targets --all-features -- -D warnings` | Passed under upstream workspace lints |
| `cargo +1.89 check --locked -p bittensor-core --no-default-features` | Passed |
| Same check with `--features ledger` | Passed |
| Same portable check with `--target wasm32-unknown-unknown` | Passed |
| `cargo +1.89 check --locked -p bittensor-core-py` | Passed |
| `cargo +1.89 fmt --check -p bittensor-core -p bittensor-core-py` | Passed |
| `.github/scripts/test-rust-ci-paths.sh` | Passed: 32 packages, 13 path prefixes |
| `git diff --check upstream/main` | Passed |

The new tests are `ledger_device_is_send_and_sync`, `with_transport_carries_the_chunked_sign`, and `with_transport_maps_user_refusal`. The chunking test checks literal INIT/ADD/LAST framing, nonzero account/index path bytes, the payload length, the boundary between a 300-byte payload and a 200-byte proof, and complete consumption of the script. An independent static reviewer found no actionable issue.

Retained output from the committed-HEAD pre-push gate:

```text
running 5 tests
test signers::ledger::tests::ledger_device_is_send_and_sync ... ok
test signers::ledger::tests::bip44_path_matches_generic_app_layout ... ok
test signers::ledger::tests::device_error_formats ... ok
test signers::ledger::tests::with_transport_maps_user_refusal ... ok
test signers::ledger::tests::with_transport_carries_the_chunked_sign ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 70 filtered out; finished in 0.00s
```

The full library run's result was:

```text
test result: ok. 74 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 5.32s
```

Research J §8.7 records a prior emulator run of the prototype at `v477`: core-built version/address/signing calls against Speculos 0.27.1 and Polkadot app 100.0.29, including user rejection, followed by replay. That evidence remains planning evidence. This execution ran Rust checks only; no Python, local emulator or physical-device run occurred.

Upstream `AGENTS.md` says, "Pushes must go out as `unarbos`" and requires `scripts/preflight.sh` before every push. The project plan instead uses the maintainer's `sam0x17` fork and requires Python to run only in CI; that plan takes precedence. We used the authorized fork, accessed no 1Password/PAT secret, and installed a task-local pre-push hook that checked the clean committed HEAD, the authenticated `sam0x17` identity, the exact fork URL, the three-file diff boundary, and the Rust checks above. The push passed that hook; neither `--no-verify` nor a force push was used. The upstream full preflight was not run: it runs Python for this SDK change and requires its other monorepo checks.

`cargo fmt --all --check` was attempted and failed before formatting: the excluded vendored `w3f-bls` package found the enclosing rtao workspace because the clone was intentionally kept under rtao's task temporary root. Package-scoped formatting passed without changing any manifest or vendored file. We do not claim a full monorepo formatting or preflight pass.

The PR is open at the exact review commit. Its labels and review requests were empty at filing.

Remote checks have not validated the code. Ten workflows for the review SHA ended in `startup_failure`, including [Check Rust](https://github.com/RaoFoundation/subtensor/actions/runs/38511706224); its check suite contained no check runs. `gh run view` reports a likely workflow-file issue, but the API provides no specific diagnostic. Vercel reported "Authorization required to deploy." These remain visible upstream failures; no retry, label change or authorization setting was attempted.

The task root's clone, build cache, logs, hook and gate script were disposable once no live dependencies remained. The bounded patch and this record are the retained outputs.

Cleanup completed 2026-10-14 at 04:35 UTC. Before removal, `ps` found no task process and `lsof +D` found no open file under the directory. Service-manager, configuration, symlink and virtual-environment checks found no task-path reference or external dependency. The validated exact directory was removed, reclaiming its 2.4 GB clone/build/temp tree. The shared task root and unrelated work were retained. This record and `pr-u17.patch` remain; no live dependency was retained for PR-U17.
