# Validator image

Build from the repository root:

```sh
docker buildx build --load --platform linux/amd64 \
  --file ci/validator/Dockerfile --tag rtao-validator:check .
```

The builder is `rust:1.89-slim-bookworm`, pinned to its linux/amd64 manifest.
It builds `-p rtao-neuron --bin rtao-validator` with default features off and
`validator,prometheus` enabled. BuildKit caches Cargo git sources, registry
sources and artifacts. The recipe installs no build packages and rejects the
no-host dependency list before compilation. Both stages have immutable base
digests. The runtime runs as UID 65532 and carries the builder's CA bundle.

The `validator-image` workflow runs on free `ubuntu-latest`. It builds the
image, records its exact image ID, then runs the Rust probe in `probe/`. The
probe starts a real TLS `FakeNode`, requires an untrusted certificate to fail
before any RPC reaches the server, and verifies startup with the explicitly
mounted test certificate. Successful startup must request genesis, runtime
version and metadata. The probe closes its server and removes only containers
whose unique ownership label matches. If removal cannot be verified, it keeps
the certificate mount and reports the retained path.

The sample validator skips weights by default. Its explicit
`--synthetic-weights` option scores registered non-owner destinations equally
for protocol tests. A subnet application supplies its scoring callback. Automated
localnet checks exercise real commits and observed reveals. The build arguments
`BUILD_PACKAGE`, `BUILD_BINARY` and
`BUILD_FEATURES` let a subnet supply its own validator binary under the same
recipe and no-host check.

Mount only the hotkey file and any required public wallet files read-only,
and mount a separate writable state directory. Pass `--wallet-path /wallet`
and `--state-dir /state`. Keep the metrics listener private to the host or
monitoring network. Run the image's `--help` for the binary's current flags.
No 24-hour testnet soak is required. Automated image, TLS, localnet and
interoperability checks remain required.
