# M2 validator image preparation

The no-host image recipe, free Ubuntu workflow and standalone Rust TLS probe
are authored. They use `rtao-neuron`'s wallet-free `--check-connection` path,
normal TLS validation, Finney genesis checking and required runtime metadata
verification. The probe checks both trusted startup and rejection of the
self-signed certificate without its explicit trust root.

The standalone probe compiles on Rust 1.99 and Rust 1.89, and strict Clippy
passes. Actionlint 1.7.7 passes the workflow. These are local preparation
results. No local Docker daemon is available, so no image build or container
TLS run is claimed. M2-23 and its numbered exits still require those actual
Ubuntu results. No Python was executed locally or in a local container.

The Rust builder digest is the existing authenticated oracle base for
`rust:1.89-slim-bookworm` on linux/amd64:
`sha256:c9ac3fa8945b61dede1e4500d25028aa8fd8a8fe46365fcf9c0422f8d999b9b0`.
The runtime uses the linux/amd64 `debian:bookworm-slim` manifest resolved from
Docker Hub on 2026-10-14:
`sha256:a4672c0cb26fbdde88e38fa2dfb6c681942306680e41e4378b28770b6e79ee91`.
The Dockerfile installs no build packages. Native compilation uses the C
linker supplied by the Rust base; the recipe rejects every current no-host
ban over its normal/build dependency tree.

The built-in sample skips weights by default. The explicit
`--synthetic-weights` flag produces equal scores for registered non-owner
destinations for protocol testing; a subnet-specific scoring application is
supplied separately. Q29 waives the 24-hour testnet soak and its dedicated
funding, registration and host prerequisites. Actual image execution, TLS,
localnet commits/reveals and interoperability checks remain pending. No exit
checkbox, rtao commit, push or tag follows from this record.

The probe journals its exact owned container name before starting Docker and
checks its ownership label before removal. A failed cleanup retains the mount
directory and reports the live dependency. The current local checks created
no containers or certificates. Shared build/source caches under
`target/execute-plan.6Ro0PC` remain live for integrated verification.

The final probe's Rust 1.89 strict Clippy check passes after tightening its
metadata assertion: trusted startup must call `Metadata_metadata_at_version`
with the V15 argument, in addition to genesis and runtime-version requests.
The exact unused Debian manifest download was removed after process,
open-file, launchd, source/configuration, symlink and virtual-environment
audits. Bounded cleanup evidence is `tmp/root-image-input-cleanup.txt` under
the shared execution root; no image, container or certificate was created.
