# U10 Python wheel release compatibility

Read-only release audit on 2026-10-14. Reviewed
[PR #3250](https://github.com/RaoFoundation/subtensor/pull/3250) at published
HEAD `64f152725dda64c040e8f3c6435c08f450093a59`. This report records a release
coordination gap. The audit established no broken, currently installable
stable SDK/core combination and selected no new version floor.

## Import contract and published versions

The PR adds eager assignments in
[`bittensor/sp_core.py`](https://github.com/RaoFoundation/subtensor/blob/64f152725dda64c040e8f3c6435c08f450093a59/sdk/python/bittensor/sp_core.py#L14):
`verify_strict = _backend.verify_strict` and
`encode_password_for_environment = _backend.encode_password_for_environment`.
A backend lacking either attribute cannot import this module successfully.
Matched-source Rust/PyO3 builds do not test that installation combination.

The committed SDK dependency remains `bittensor-core>=0.1.6,<0.2.0`.
The binding's Python distribution version remains `0.1.6`; its pyproject
explicitly uses a static version so the release train can stamp PEP 440
suffixes. Its Rust Cargo package version is separately `0.1.0`, with
`publish = false`. Changing the Rust package version alone would not select
the Python distribution version.

Public PyPI metadata observed during this audit:

| Package/version | State and native dependency |
| --- | --- |
| `bittensor-core 0.1.5` | Latest stable native release |
| `bittensor-core 0.1.6` | Version JSON endpoint returned HTTP 404 |
| `bittensor-core 0.1.6rc68`, `0.1.6rc69` | Published prereleases |
| `bittensor 11.3.0` | Latest stable SDK; requires `bittensor-core>=0.1.5,<0.2.0` |
| `bittensor 11.3.1rc69` | Published RC; requires exactly `bittensor-core==0.1.6rc69` |

The PR's `>=0.1.6` constraint excludes both `0.1.5` and `0.1.6rc69`.
A prerelease sorts before its final release; allowing prereleases does not
make `0.1.6rc69` satisfy that lower bound. These older published artifacts
therefore do not establish an already allowed broken stable combination.
The current index has no published stable candidate for the PR's floor.

Sources: [core metadata](https://pypi.org/pypi/bittensor-core/json),
[stable SDK metadata](https://pypi.org/pypi/bittensor/json), and
[SDK RC metadata](https://pypi.org/pypi/bittensor/11.3.1rc69/json).
These endpoints are mutable; the table records the 2026-10-14 observation.

## What the release train guarantees

The following source links use the reviewed immutable PR HEAD. The PR does
not change these release scripts or workflows.

| Channel | Version selection and dependency handling |
| --- | --- |
| RC | [`release-train.yml:304-403`](https://github.com/RaoFoundation/subtensor/blob/64f152725dda64c040e8f3c6435c08f450093a59/.github/workflows/release-train.yml#L304) computes one `rcN` suffix, builds native wheels and rewrites the SDK requirement to the exact matching core RC. The published `11.3.1rc69` metadata confirms that pin. |
| Stable | [`watch-mainnet-release.yml`](https://github.com/RaoFoundation/subtensor/blob/64f152725dda64c040e8f3c6435c08f450093a59/.github/workflows/watch-mainnet-release.yml#L161) reads versions from an immutable release SHA and builds both packages from that SHA. [`prepare-sdk-dist.py`](https://github.com/RaoFoundation/subtensor/blob/64f152725dda64c040e8f3c6435c08f450093a59/.github/scripts/prepare-sdk-dist.py#L10) removes `.dev0`, the local uv source mapping and uv lock, but leaves the dependency range unchanged. |
| Dev | [`publish-sdk-dev.yml`](https://github.com/RaoFoundation/subtensor/blob/64f152725dda64c040e8f3c6435c08f450093a59/.github/workflows/publish-sdk-dev.yml#L43) publishes the SDK alone to TestPyPI with a `.devN` version. It does not build matching native wheels or rewrite the core requirement. Its install instructions use real PyPI as the additional dependency index. |

The stable workflow verifies expected artifact versions, platform coverage
and publication attestations. The inspected
[`check-python-release.py`](https://github.com/RaoFoundation/subtensor/blob/64f152725dda64c040e8f3c6435c08f450093a59/.github/scripts/check-python-release.py)
does not install the SDK against its minimum allowed native wheel, inspect
that wheel's callable exports, or validate the SDK wheel's dependency floor.

Same-commit builds establish one matching pair. A broad dependency range can
also permit an older native release to remain installed when the SDK changes.
If that older permitted release lacks the new exports, the eager assignments
fail even though the version resolver accepted the dependency.

The final [v478 release](https://github.com/RaoFoundation/subtensor/releases/tag/v478)
points to `850f2755bb05a3851f948489f0ab27b0645cebce`. That immutable source
already designates native `0.1.6` and SDK `11.3.1.dev0`, without U10's helpers.
This existing allocation matters when choosing the first containing native
release. It does not prove that stable `0.1.6` has been published, or that a
particular later version such as `0.1.7` is already the required floor.

## Artifact inspection without Python

Native curl, Ruby SHA-256/gzip/tar readers and the system archive reader
processed downloaded bytes in memory. No package was installed, no extension
was loaded, no Python interpreter ran, and nothing was extracted to disk.

| Artifact | Bytes | Verified SHA-256 |
| --- | ---: | --- |
| `bittensor_core-0.1.5.tar.gz` | 291225 | `bb6175643ef351e8afc5059cb907b6f13083bec7525ad287c1a6db30be9ee980` |
| `bittensor_core-0.1.6rc69.tar.gz` | 291275 | `3dbc70cfbfc7521b5ca3350a4673448703ab8e072189bb19438fcaf2a1f468dd` |
| `bittensor_core-0.1.6rc69-cp310-abi3-macosx_11_0_arm64.whl` | 4762950 | `5ed3f84a412ce37cb0d24207ab097dcc0c9a2d9586880c2c1d3859ac003e1341` |

Both sdists contain the same binding `sdk/bittensor-core-py/src/keys.rs`,
SHA-256 `51fd96b6cdfaf4f7e5b71f3f894d25a92b1d4be89c84cd84fa1765c4c93ccb1c`.
Neither source defines or registers either new helper. This is source evidence;
the audit performed no native-module runtime export test. The inspected wheel
reports version `0.1.6rc69` in METADATA and uses a wildcard native reexport in
`bittensor_core/__init__.py`. PyPI's provenance response identifies the
`RaoFoundation/subtensor` release-train publisher and `testnet` environment,
with the same wheel subject digest. No independent Sigstore verification was
performed.

Artifact URLs and digests came from the versioned
[0.1.5 metadata](https://pypi.org/pypi/bittensor-core/0.1.5/json) and
[0.1.6rc69 metadata](https://pypi.org/pypi/bittensor-core/0.1.6rc69/json).

## Proposed correction and hosted checks

The release owner needs to select a native version whose actual wheel contains
both exports. Align the binding pyproject version and SDK lower bound with that
first containing release, then regenerate the development lock canonically.
Account for v478's existing allocation; do not guess a floor, rewrite a tag, or
reuse a published version for changed bytes. No version edit is authorized or
implemented by this audit.

Hosted CI should build versioned SDK/core wheels and install them without
editable sources or the monorepo uv override. It should:

1. Install the SDK with the exact minimum supported native wheel, import
   `bittensor` and `bittensor.sp_core`, and exercise both helpers with their
   existing behavioral vectors.
2. Start with an older released core already installed, then install the new
   SDK through the resolver. Assert that dependency metadata requires an
   upgrade to a containing wheel and that the imports succeed. Before the
   new native version reaches PyPI, use the CI-built versioned wheel in the
   test wheelhouse; repeat against the published artifact after release.
3. Inspect the built SDK wheel's `Requires-Dist` and verify the final published
   minimum-version artifacts. Keep RC exact-pin coverage separately.

Strict verification must retain its raw-message contract; substituting the
existing verifier's wrapped-message retry would change behavior.

[PR #3249](https://github.com/RaoFoundation/subtensor/pull/3249), reviewed at
`0c4ed7585ae9d232577e53d45813f383e0ec2eec`, changes bindings and Python tests,
with no production Python eager alias to a new attribute. Its added IR fields
need release coverage, but this audit found no equivalent import failure in
that PR.

The proposed hosted checks have not run. This audit made no production-source,
dependency, version, GitHub, label, review-request or publication change.
This bounded report is its only filesystem output; there are no temporary
extraction or build directories to clean up.

The release prerequisite was subsequently appended to the existing #3250
body; the returned text and unchanged published HEAD were verified. The body
SHA-256 is `47a8e9a91400fde422b982c04f9f1404ffcf642a77df34e8ecb90087c3fd1b61`.
The posted text is retained as
[`u10-release-coordination/body.md`](u10-release-coordination/body.md);
before/after snapshots are not retained in the repository. Labels and review requests remain empty.
No version edit, wheel publication or installed-wheel test is claimed by
this disclosure. The preparation directory was audited and removed after
retaining the bounded public evidence.
