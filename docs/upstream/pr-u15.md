# PR-U15: portable decoded-value navigation

Filed 2026-10-14: [RaoFoundation/subtensor#3243](https://github.com/RaoFoundation/subtensor/pull/3243).

| Item | Verified value |
|---|---|
| Base | `850f2755bb05a3851f948489f0ab27b0645cebce` (`main`, `v478`) |
| Fork and branch | `sam0x17/subtensor`, `sdk-value-navigation` |
| Review commit | `1ac10f67ff3dcf8306d18183471158b8a648cdb4` |
| Retained patch | [pr-u15.patch](pr-u15.patch) |
| Scope | Four files under `sdk/bittensor-core`; 474 insertions, 90 deletions |

`codec::value` exports the existing field, variant, scalar and byte helpers
and a `ValueExt` trait. The blocking client uses these functions and preserves
its public helper exports. Byte extraction traverses singleton wrappers
iteratively, with a 256-wrapper bound matching the codec's nesting limit.

Rust 1.89 on macOS ARM64 passed these checks on the committed review SHA,
again through the task-local pre-push hook:

- Nine portable navigation tests with `--no-default-features`.
- All-feature core library tests: 71 passed, one existing ignored test.
- Strict core and Python-binding Clippy, all features and targets.
- Portable `wasm32-unknown-unknown` check.
- Package formatting and `git diff --check`.

Binding compilation used `PYO3_NO_PYTHON=1`; no Python interpreter ran locally.
The upstream instructions require the full monorepo preflight and the
`unarbos` actor. The maintainer's account and the project's CI-only Python
policy take precedence, as for PR-U17. The task-local hook verified a clean committed
HEAD, the exact four-file scope, authenticated `sam0x17` identity and fork URL,
then ran the Rust checks. No force push or hook bypass was used.

The nested checkout initially inherited rtao's Clippy configuration, which
forbids the core's product methods for rtao consumers. Upstream has no Clippy
configuration file. An empty task-local `CLIPPY_CONF_DIR` stopped discovery at
the parent workspace; upstream's own Cargo lint rules remained active.

Remote checks have not validated the code. Ten upstream workflows ended in
`startup_failure`. [Check Rust](https://github.com/RaoFoundation/subtensor/actions/runs/38526348269)
had zero jobs; `gh run view` reported a likely workflow-file issue without
further diagnostics. The PR was open at the recorded SHA with no labels or
review requests. No `CI` label or subscriber review request was applied.

The PR clone, hook and configuration were held under the recorded temporary
root's `pr-u15/` directory. The clone used Git alternates to the shared pin
cache, which remains needed by ongoing rtao work. Before exact removal,
process and open-file checks found no dependency; both launchd domains and
configuration checks passed. Seventy-five symlinks and 58 configuration or
environment files had no external reference. The scoped local-directory `lsof`
query found no open file. Bounded patch and validation evidence are retained here.
