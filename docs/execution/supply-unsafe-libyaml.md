# Reviewed upstream YAML maintenance dependency

Hosted run [38740705929](https://github.com/sam0x17/rtao/actions/runs/38740705929)
stopped on [RUSTSEC-2024-0449](https://rustsec.org/advisories/RUSTSEC-2024-0449.html)
for `unsafe-libyaml 0.2.11`. The actual finding is classified `unmaintained`,
with null CVSS, null affected scope and no patched or unaffected releases. It
reports the repository's archival and the resulting maintenance risk. This is
an active dependency when the optional Subxt adapter builds.

The raw workspace report has 17 maintenance notices, the existing lru unsound
finding and the existing ruzstd vulnerability. This is the only maintenance
finding not already covered by the reviewed policy. The fuzz report adds no
unknown finding. The retained lru and ruzstd rules still require their affected
versions to be absent from every checked build graph; this YAML rule does not
waive a vulnerability or claim dormancy.

## Upstream options and feature checks

The current published versions match the checked lockfile:

```text
rtao-client optional subxt feature
  subxt 0.51.1
    frame-decode 0.18.1, legacy-types enabled
      serde_yaml 0.9.34+deprecated
        unsafe-libyaml 0.2.11
```

The published [Subxt manifest](https://docs.rs/crate/subxt/0.51.1/source/Cargo.toml)
requires frame-decode with `features = ["legacy-types"]` and
`default-features = false`. That dependency is unconditional; Subxt's `native`
feature does not control it. This project already disables Subxt's defaults
and selects only `native`. There is no exposed Subxt feature switch that turns
off the requested frame-decode feature.

The published [frame-decode manifest](https://docs.rs/crate/frame-decode/0.18.1/source/Cargo.toml)
defines `legacy-types = ["legacy", "dep:serde_yaml"]` and requires
`serde_yaml = "0.9"`. The separate subxt-metadata dependency disables
frame-decode defaults, but Cargo's feature unification retains the
`legacy-types` feature requested directly by Subxt. The raw all-features and
adapter-profile graphs both contain frame-decode, serde_yaml and unsafe-libyaml.

The advisory recommends safe Rust parsers such as saphyr/serde-saphyr and
noyalib. These cannot satisfy the existing serde_yaml package requirement by
a version update. The suggested serde_yaml_ng fork still uses unsafe-libyaml,
as the advisory explicitly records. Removing this dependency requires an
upstream parent migration or a maintained parent fork, with validation of its
legacy metadata behavior. There is no compatible released parent update or
supported Subxt feature setting that performs that migration today. No new
upstream issue or PR was filed.

## Exact maintenance policy

The user's authorization permits best-effort remediation while accepting
upstream limitations. [xtask/src/supply.rs](../../xtask/src/supply.rs) accepts
this exact informational notice as retained maintenance evidence. The package
identities must each occur once with the crates.io registry source:

| Package | Version | Registry SHA-256 |
| --- | --- | --- |
| unsafe-libyaml | 0.2.11 | `673aac59facbab8a9007c7f6108d11f63b603f7cabff99fabf650fea5c32b861` |
| serde_yaml | 0.9.34+deprecated | `6a8b1a1a2ebf674015cc02edccce75287f1a0130d394307b36743c2f5d504b47` |
| frame-decode | 0.18.1 | `c9a9e93cab13c8e9a6accf029997d19943cf54fea5f5eb028a1133b7179d06aa` |
| subxt | 0.51.1 | `c5a67bfdccc607684ec2db8dbff6dbfcb72fa0ba10608698d9cc0b3cc452ebe5` |
| subxt-metadata | 0.51.1 | `cc5c32a26366a6eac132535164b4f0034a5f4d56718f34db6814af7fc5e10f35` |

The only direct parent of unsafe-libyaml must remain serde_yaml; the only
direct parent of serde_yaml must remain frame-decode; frame-decode's direct
parents must remain exactly subxt and subxt-metadata. A changed version,
checksum, source, dependency edge or additional parent fails for investigation.
Removing the dependency needs no exception.

Acceptance requires `unmaintained` in both the warning and advisory, the exact
advisory ID, metadata and description hash, null CVSS and affected scope, and
the reviewed empty version ranges, vulnerability categories, aliases, related
advisories and references. A newly reported vulnerability, changed category,
scope, severity, description or advisory fails. The rule does not filter or
edit the raw findings, alter their command statuses, or weaken unrelated
advisory and dormant-activation checks.

## Evidence and validation

Every raw workspace/fuzz finding in hosted artifact `11975709033`,
`supply-evidence`, was inspected, and the actual warning is retained in
[unsafe-libyaml.json](../../xtask/tests/fixtures/supply/unsafe-libyaml.json).
The bounded evidence record is
`.local/recovery/2026-10-15/unsafe-libyaml-warning.json`. The artifact ZIP was
validated and inspected in memory; no archive or extraction directory was
retained. The follow-up graph inspection authenticated these exact entries:

- Artifact ZIP SHA-256: `e315c10997ac4ec9b5f2ab6829f5f73f01d016a6b60893ddce8c463d672a20da`.
- Raw `supply-15777/003-workspace-audit-raw-json.stdout` SHA-256: `dc550f5877c98ef975b5c2e2502cdfe4c4eb8f14d36d33263e60bdd8c194eadb`.
- `supply-15777/007-workspace-all-features.stdout` SHA-256: `d080c3da8aa7844d5304f9e54baa42e75d2c9930b47e93c59b4bb09bbe67d8b4`.
- `supply-15777/035-rtao-profile-23.stdout` SHA-256: `c38cce78af1e766a9e56b9189c1375f7865a375965e992a59eda875e117b36b9`.
- Warning fixture SHA-256: `b3899472a82cdef8aa97807a8c7841fefb11184373de56795d5948b8323f29f1`.
- Advisory description SHA-256: `fbaecdaa813d8fbe71e2864557722e8fe22367aefe39b598d2f77c68486c0ada`.

Three authored regressions use the independent actual-warning fixture. They
cover active graphs, unchanged raw JSON and terminal warning visibility,
command status, vulnerability/category conversion, new advisories, changed
identities even when audited, scope/severity/description drift and changed
parent identities or edges. Local checks cover formatting, whitespace and
bounded source/evidence inspection. Compilation, regressions and the full
supply gate remain for hosted CI; no test success is claimed here.
