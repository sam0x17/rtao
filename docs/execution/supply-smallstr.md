# Remaining Subxt maintenance notice

Hosted run [38734177643](https://github.com/sam0x17/rtao/actions/runs/38734177643)
reaches the full raw supply audit and stops on
[RUSTSEC-2026-0215](https://rustsec.org/advisories/RUSTSEC-2026-0215.html):
`smallstr 0.3.1` is unmaintained. RustSec identifies no vulnerability in this
notice and lists no patched version. The existing dormant vulnerability rules
and reviewed proc-macro-error2 notice do not cover it.

The checked lockfile chain is `smallstr 0.3.1 <- scale-info-legacy 0.4.2`.
The legacy crate has exactly three direct parents: `frame-decode 0.18.1`,
`subxt 0.51.1` and `subxt-metadata 0.51.1`. The optional Subxt adapter and its
metadata/codegen stack use it. The hosted all-features and adapter-profile
graphs include smallstr; this is an active dependency, not a dormant exception.

Read-only upstream checks on 2026-10-15 found:

- The latest registry releases remain smallstr 0.3.1 and scale-info-legacy 0.4.2.
- The upstream scale-info-legacy manifest still requires `smallstr = "0.3.0"`.
  The reviewed manifest blob is `5cc8556f504515fe1aefa5a17c953640779963a7`.
- Subxt still requires scale-info-legacy unconditionally. Its reviewed manifest
  blob is `fffdd0ed1f9a36625ee9735df87b0f91691db617`.
- RustSec suggests compact_str or smol_str. Those require migration in the
  parent crate; neither can satisfy the existing smallstr dependency by a
  version update.

The full raw evidence is retained in hosted artifact `11973500903`,
`supply-evidence`, ZIP SHA-256
`d44d8f5137a5c0f41f5d12b682b668e09680f9e38f77b2521ee60793981caaa4`.

## Exact maintenance policy

The maintainer authorized best-effort crate remediation while accepting upstream
limitations. No compatible released update removes this dependency: the latest
parent still requires smallstr, and the suggested replacements require changing
and validating that parent. Maintaining a private fork for an informational
maintenance notice would add an ongoing upstream maintenance obligation.

[xtask/src/supply.rs](../../xtask/src/supply.rs) retains this exact warning as a
reviewed maintenance notice. It requires the `unmaintained` category in both
the raw warning and the advisory, the advisory ID above, exact package identity,
and the reviewed parent identities and direct edges:

| Package | Version | Registry SHA-256 |
| --- | --- | --- |
| smallstr | 0.3.1 | `862077b1e764f04c251fe82a2ef562fd78d7cadaeb072ca7c2bcaf7217b1ff3b` |
| scale-info-legacy | 0.4.2 | `d972ce93a4f81efc40fce251e992faf99ecb090c02bcbd3614e213991038c181` |
| frame-decode | 0.18.1 | `c9a9e93cab13c8e9a6accf029997d19943cf54fea5f5eb028a1133b7179d06aa` |
| subxt | 0.51.1 | `c5a67bfdccc607684ec2db8dbff6dbfcb72fa0ba10608698d9cc0b3cc452ebe5` |
| subxt-metadata | 0.51.1 | `cc5c32a26366a6eac132535164b4f0034a5f4d56718f34db6814af7fc5e10f35` |

Each identity must occur once with the crates.io registry source. The only
direct parent of smallstr must remain scale-info-legacy, whose direct parents
must remain exactly the three listed above. Version, checksum, source or edge
changes fail for investigation; dependency removal does not need an exception.

Acceptance also requires the exact advisory metadata, description hash, null
CVSS, null affected scope, and empty patched/unaffected ranges, categories,
aliases, related advisories and references. A new vulnerability, reclassified
warning, affected function, severity, advisory or changed description fails.
The original warning is never filtered or edited. Raw audit text/JSON, command
statuses and graph evidence remain retained, and unrelated vulnerabilities and
the existing dormant-activation guards remain blocking.

## Evidence and validation

The actual warning is retained in
[smallstr.json](../../xtask/tests/fixtures/supply/smallstr.json), taken from
`supply-15778/003-workspace-audit-raw-json.stdout` in the artifact above. The ZIP
was validated and inspected in memory; no archive or extraction directory was
retained.

- Raw workspace JSON SHA-256: `dc550f5877c98ef975b5c2e2502cdfe4c4eb8f14d36d33263e60bdd8c194eadb`.
- Warning fixture SHA-256: `5039cb6618759f0a6e3b9903023c31b612cff75e39433d7953d4110a02103777`.
- Advisory description SHA-256: `1e732bf83d21445cd724ccdd77a21839169c4b8b0a5ccea14c6c8187e56a3f80`.
- `007-workspace-all-features.stdout` and `035-rtao-profile-23.stdout` both
  contain `smallstr v0.3.1`.

Five authored regressions cover the actual active warning, unchanged raw JSON,
terminal warning visibility and exit status, vulnerability conversion, changed
warning categories, new advisories, identities even when present in the audit
lock set, changed severity/scope/description, and changed parent identities or
edges. The existing proc-macro-error2 regressions also cover the shared exact
maintenance classifier. Local checks are limited to formatting, whitespace and
bounded source/evidence inspection. The regressions and full supply gate must
run in hosted CI; no test success is claimed here. No new issue or PR was filed.
