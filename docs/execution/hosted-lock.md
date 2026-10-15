# Hosted dependency edge resolution

The optional `refresh_lock` input on `pr.yml` runs a five-minute free
`ubuntu-latest` job. It does not compile source or fetch dependencies locally.
`ci/resolve-existing-lock.mjs` refuses execution outside hosted Ubuntu and gives
`cargo update --workspace` 170 seconds inside a 180-second command limit.

This candidate permits only the newly declared `rtao-cli -> nix` and
`rtao-wallet -> rustix` edges. Both packages already occur in the committed
lockfile. All package names, versions, sources, checksums and other package
metadata must remain identical. Removed edges or unrelated additions fail.
The `multisig` feature also activates its existing Tokio dependency for a bounded
advisory read; that package and dependency edge are already locked.

The job returns only `Cargo.lock`. Review its exact artifact provenance and diff,
then adopt it before regenerating compatibility reports. Report generation uses
a separate clean commit with matching manifest and lock hashes. The candidate
job cannot push or make test, runtime or platform acceptance claims.
