# M3 subnet template execution evidence

M3-09 is implemented locally, with hosted image/chain acceptance pending. The
full gate is `cargo xtask template check`; `--native-only` is an explicit local
preflight and does not mark milestone acceptance.

`templates/subnet/` contains a cargo-generate workspace with validator, miner
and integration-test members. The validator uses the real signed fan-out
client, keeps scores by hotkey and proposes only positive scores. The miner
uses the runtime's default validator-permit admission and an authenticated
`VerifiedCaller` / `Json<T>` route. Both install ring first in `main`; the xtask
test rejects a different provider or configuration before provider installation.
The invariant check also checks the authoritative core pin, the single BLS
patch, no member patches and each neuron's exact no-host feature selection.

The generated CI uses only free `ubuntu-latest`, caches Cargo's git database,
checks formatting/strict Clippy, builds both binaries, runs doctor against the
actual generated manifest, builds each image with `-p`, and runs compose. The
Dockerfile uses the same immutable Rust 1.89/bookworm manifests as the prepared
validator image, BuildKit git/registry/target caches and no installed build
packages. It rejects the host/alternate TLS/ledger dependency stacks. The
runtime is non-root with a read-only hotkey mount, separate writable state and
the required webpki-root-certs and Unicode-DFS-2016 licenses. Both stages reject
symlinks and require regular files of 2,371 and 2,315 bytes respectively, then
check their exact SHA256. Explicit runtime/config cache paths live
inside writable state so they work with a read-only filesystem. The systemd
unit uses SIGTERM and a 90-second stop timeout.

The integration test creates the subnet through the real `Scenario`, stakes
the validator, waits for its validator permit, creates only deterministic
development hotkeys under the recorded task root, and starts both actual
images. It observes every finalized height to avoid missing an event on the
250 ms chain. Acceptance requires the exact validator/subnet commitment and
`TimelockedWeightsRevealed`, the exact miner weight row, a successful reply
to the validator's signed request, a positive miner checkpoint request count and
zero exit codes after graceful shutdown.

The root e2e job generates with checksum-pinned cargo-generate 0.23.12. It
patches the rtao git source to this checkout for `rtao`, `rtao-testkit` and
`rtao-runtime` (typed integration setup/storage). The Docker build receives this
same checkout as the `rtao` named context and rewrites only the generated source
overlay to its in-image path. A generated project's own CI resolves the named
context and localnet digest from the SDK checkout Cargo actually selected.
Template paths, including Markdown, count as code; the template job participates
in `e2e-required`.

No rtao release exists yet, so the scaffold currently selects `branch = "main"`
for all SDK dependencies. Its lockfile captures the resolved commit, and its CI
preserves an existing lockfile. The first release should replace these selectors
with the same released `rtao-v*` tag. No generated core pins or catalogs were
edited by this work.

Verification performed on the Mac Studio:

| Check | Result |
| --- | --- |
| cargo-generate 0.23.12 macOS release archive checksum | `d702c3addcc1b7401330d80d74886def25168247b80c8d9af5a8f8fbfa4a64bc`, matches GitHub's asset digest |
| Actual generation and source overlay | Passed; standalone generated workspace resolved a Rust 1.89-compatible lock |
| Locked dependency prefetch before offline doctor | Passed; Cargo fetch covers foreign-target metadata dependencies |
| Template source and actual Bash cleanup regressions | All three passed on current Rust and Rust 1.89 |
| Rust 1.89 generated formatting | Passed |
| actionlint, root e2e and generated CI | Passed |
| Bash syntax, all four generated scripts | Passed |
| Compose configuration parsing | Passed for all three services, both neurons UID/GID 65532 with read-only roots |
| Generated strict workspace/all-target Clippy, Rust 1.89 | Passed |
| Both generated native binaries, Rust 1.89 | Passed |
| Real integration executable linking and native doctor | Passed on the generated consumer workspace |
| Both CLI help commands, no-host feature trees and generated-project cleanup | Passed |
| Both container builds and actual compose-localnet contract | Pending: the local Docker daemon is unavailable |

An independent source review found and corrected three issues: dependency
prefetch was missing before offline doctor; the Bash EXIT trap could hide a
compose cleanup failure; and the README described a signed response even though
only the request is signed. The regression executes the shipped cleanup function
with injected Docker exit statuses and checks all four work/cleanup success and
failure combinations. It preserves a work failure and reports failed cleanup
after successful work. A separate source guard checks the actual license bytes,
both-stage file/size/digest guards and runtime copies.

The final native run is recorded in `template-evidence-28986/result.json`:
`passed` and `cleanup_passed` are true; `native_only` is true and
`milestone_acceptance` is false. Its `project-ci.log` contains the actual Rust
1.89 Clippy/build/link checks and doctor result. The earlier native result did
not include dependency prefetch or doctor and is superseded by this run.
The final three-regression logs are `m3-template/logs/xtask-tests-licenses.log`
and `m3-template/logs/xtask-tests-msrv.log`; both report three passed tests and
no failures. The independent final source review reports no remaining findings.

All retained logs and intermediate results are beneath
`target/execute-plan.6Ro0PC/m3-template/` and the command's bounded
`template-evidence-*` directories. No local Python or container Python ran.
The xtask waits for child commands and verifies the compose ownership label,
open-file dependencies, directory marker and absence of symlinks before removing
its disposable generated project. It retains its bounded lock, result and public
logs. The shared Cargo target/cache is a live dependency of other ongoing work.
The verified generator, both licenses, release metadata and archive/binary
checksums are retained for reuse under `target/execute-plan.6Ro0PC/tools/template/`.
Its original extraction and 5.6 MiB archive were removed after open-file,
launchd, shell-configuration, symlink and dynamic-library checks. Only system
libraries were linked; no virtual environment or global installation was created.

No M3 checkbox, milestone tag, commit or push is asserted by this evidence.
