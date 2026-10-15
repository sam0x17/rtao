# Release verification

Releases include CLI archives for Linux x86_64, Linux ARM64, Apple ARM macOS
and Intel macOS. Linux builds target glibc 2.35. ARM Linux checks use QEMU;
Apple ARM checks are native, and Intel macOS checks use Rosetta.

Check `SHA256SUMS` after downloading assets. Checksums establish integrity only
after the provenance has been authenticated. Linux and docs have GitHub build
attestations from this repository's release workflow. Mac binaries have a
separate signed local build record containing the source tag, toolchain,
artifact hashes and check logs. The workflow does not claim to build Mac assets.

From the exact clean source tag, run:

```sh
cargo xtask release verify --tag rtao-v0.1.0 --dir /path/to/downloaded/assets
```

Verification fails if any target or required provenance is missing. Local Mac
verification also requires the repository's approved Ed25519 trust anchor.
`COMPAT.md` and `compat.json` record runtime validation. The book and rustdoc
archives can be extracted and viewed offline.
