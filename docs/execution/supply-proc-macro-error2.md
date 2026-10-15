# Reviewed upstream maintenance dependency

`proc-macro-error2 2.0.1` is active when the optional Subxt adapter builds.
[RUSTSEC-2026-0173](https://rustsec.org/advisories/RUSTSEC-2026-0173.html)
reports that its maintainer has stopped maintaining it. The advisory is
`informational = "unmaintained"`, has no patched release, and identifies no
affected function or CVSS severity. This finding remains visible in both raw
audit formats. The lru/ruzstd dormant vulnerability rules do not cover it.

The reviewed dependency chain is:

```text
rtao-client optional subxt feature
  subxt 0.51.1
    subxt-macro 0.51.1
      proc-macro-error2 2.0.1
```

The adapter uses `OnlineClient`, `SubstrateConfig` and `LegacyBackend`; it does
not invoke Subxt's attribute macro. Nevertheless, the published Subxt manifest
depends on `subxt-macro` unconditionally and its public library reexports that
macro. The macro crate unconditionally requires `proc-macro-error2 ^2.0.0`.
Disabling default features or selecting only `native` therefore keeps this
build dependency active.

## Upstream options checked on 2026-10-15

- The registry's latest Subxt and subxt-macro releases are both 0.51.1. Their
  Rust requirement is 1.88, compatible with this project's Rust 1.89. The latest
  proc-macro-error2 remains 2.0.1, with no maintained release in its major.
- [Subxt issue 2238](https://github.com/paritytech/subxt/issues/2238) already
  requests migration from this abandoned helper. It remains open with no
  comments. The current upstream macro manifest still requires error2.
- [The maintainer's statement](https://github.com/GnomedDev/proc-macro-error-2/issues/17#issuecomment-4643215473)
  confirms abandonment. This is the statement linked by RustSec.
- `manyhow`, `proc-macro2-diagnostics` and the maintained `proc-macro-error3`
  fork are migration options. The latest proc-macro-error3 is 3.1.1 with Rust
  1.61 support. Version 3 cannot satisfy Subxt's version-2 requirement. A simple
  renamed dependency also needs care: its attribute expands to the literal
  `::proc_macro_error3::entry_point` path, and its current defaults select Syn 3
  integration. Migration requires changing and validating Subxt's macro crate.

There is no compatible released Subxt update or feature switch that removes
this dependency. Maintaining a local Subxt macro fork solely for the
informational notice would add a compiler plugin maintenance obligation. The
chosen treatment records the exact upstream maintenance dependency and
revisits it when a maintained Subxt release removes the helper.

## Exact policy

[xtask/src/supply.rs](../../xtask/src/supply.rs) accepts this one notice as a
reviewed maintenance finding. It checks the advisory ID, package name, version,
registry source and checksum. It also checks both parent package identities
and the two lockfile dependency edges, rejecting another parent of either the
helper or Subxt macro crate.

| Package | Version | Registry SHA-256 |
| --- | --- | --- |
| proc-macro-error2 | 2.0.1 | `11ec05c52be0a07b08061f7dd003e7d7092e0472bc731b4af7bb1ef876109802` |
| subxt-macro | 0.51.1 | `77d22d8350ef5f6b455f09b211be9468c47499778e8d18615a00c12143f7f400` |
| subxt | 0.51.1 | `c5a67bfdccc607684ec2db8dbff6dbfcb72fa0ba10608698d9cc0b3cc452ebe5` |

Acceptance requires the `unmaintained` classification in both the raw warning
and its advisory. It requires the reviewed empty affected/patched/unaffected
scope, null CVSS, empty vulnerability categories/aliases/references, exact
advisory metadata, and the recorded description hash. A new vulnerability,
classification, version, source, checksum, upstream parent, advisory ID,
severity or scope fails for investigation. Removal of the dependency needs no
exception.

The full unfiltered raw audits, command statuses and graph inspections remain
unchanged. The new rule increments the retained-maintenance count and leaves
the raw result intact, including unrelated vulnerability failures. It does
not weaken cargo-deny, the other advisory checks, the dormant guards, or the
existing 14 maintenance notices.

## Evidence and validation

The bounded fixture
[proc-macro-error2.json](../../xtask/tests/fixtures/supply/proc-macro-error2.json)
is the actual warning from hosted run
[38730653363](https://github.com/sam0x17/rtao/actions/runs/38730653363), artifact
`11970315354` (`supply-evidence`), entry
`supply-3559/003-workspace-audit-raw-json.stdout`. The artifact was inspected in
memory; no archive or extraction directory was retained.

The same run's `007-workspace-all-features.stdout` and
`035-rtao-profile-23.stdout` contain `proc-macro-error2 v2.0.1`. Its active
status is observed in the hosted graph output as well as the published
dependency manifests.

- Artifact ZIP SHA-256: `543b8413c6d78b3bf51d00ca4a9f755e4cb0604e3a0a1e99e192e6fcee6dae4c`.
- Raw workspace JSON SHA-256: `dc550f5877c98ef975b5c2e2502cdfe4c4eb8f14d36d33263e60bdd8c194eadb`.
- Retained warning fixture SHA-256: `cb8124a38e0209a8e01b1cb0f0ca3d2cbe9f2bb2fb22775de0bda60fb79faa87`.
- Advisory description SHA-256: `2f735d4ab46291cc8340d0ab166530ea0ea94959736ef7dc13cc932d6f6913c9`.

Five authored classifier regressions cover the actual notice and active graph,
raw warning visibility, vulnerability conversion, changed warning category,
new advisory, changed identity even when present in the audited lock,
severity/scope/description changes, and changed upstream parent edges. These
tests use the independent raw fixture. They have not run locally; hosted test
execution and the full supply gate remain required.

An independent source investigation confirmed the advisory classification,
unconditional macro dependency, absence of a compatible upstream release and
the maintained-fork migration constraints. No new upstream issue or PR was
filed.
