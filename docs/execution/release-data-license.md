# Data-license shipment

The release verifier requires both notices as standalone assets and in every
CLI archive. Each copy must match its source file in `docs/licenses/`:

| Notice | Bytes | SHA-256 |
| --- | ---: | --- |
| `webpki-root-certs-LICENSE` | 2371 | `e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8` |
| `Unicode-DFS-2016.txt` | 2315 | `68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844` |

CLI archives contain exactly `rtao` and those two notices. Binary executable,
archive traversal, target, source tag/tree, toolchain, platform, check-log,
signature, compatibility, and attestation checks remain enforced. Each notice's
extraction is bounded to its pinned size. Missing, changed, oversized, or
symlinked notice files are refused in source, assets and archives.

All release builders stage both files. Existing output is accepted only
when it already matches, so a builder refuses to replace unknown or symlinked
data. The docs build records both hashes and the GitHub provenance job attests
both notices. Complete-draft verification includes both files in the exact asset
inventory and `SHA256SUMS` before upload.

The validator Dockerfile copies both notices to `/usr/share/licenses/rtao/`.
Its build and runtime stages require regular files with pinned sizes and hashes.
No Docker image was built locally.

The release regressions exercise both notices' exact checksums, missing source
and asset copies, same-length tampering, oversize input and symlinks. Archive
regressions omit each notice and exercise altered, oversized and linked members.
Native shell probes also cover dangling links and directory inputs, preserve
rejected destination bytes and link targets, and confirm exact idempotent staging.
Validation and cleanup results are retained in `release-unicode-validation.txt`;
the earlier certificate-only results remain in `release-license-validation.txt`.

This validation exercised synthetic assets and signatures. It made no tagged
release, hosted attestation, image or device claim, ran no Python or Docker, and
made no commits, pushes, tags, labels or review requests. The existing literal
BLS criterion and the raw lru 0.16.4 advisory remain unchanged.
