# M4 config and catalog commands

The CLI registers and dispatches all five `config` commands, `tools` and
`explain`. Config uses the injected shared store. Global overrides come from
the selected leaf, so `config get --json` and root `--json config get` both
reach the output renderer. These commands use local files and authenticated
catalogs without opening a chain connection or unlocking a signer.

`tools` retains all 83 upstream records and their nine original fields, then
adds D-029's `rust` object. Tx/query integration now supplies metadata from the
verified hosted generated interfaces and keeps pending CLI invocations null.
Its combined build and tests still need hosted validation.
`explain` preserves semantic explanations, dispatch errors,
pallet-qualified names, duplicate chain-error names and runtime declaration
order. Its output uses the existing single diagnostic renderer.

The recovered patch is authenticated by SHA-256
`01c448cf25b332f6506e0d4b34587c65d51849e98ff03e426e9ee8eb1697cece`
in `.local/recovery/2026-10-14/source-manifest.json`. Its recovery archive member
is `m4-config-catalog.20261015-010143-38375/proof/M4-10-config-catalog.patch`.
Adoption adds binary routing fixtures, honors `RTAO_TEMP_ROOT` in config
fixtures and replaces test-only `expect_err` calls with returned failures.
Existing Apache source attribution and authenticated generated catalogs cover
these ports; no generator output or license declaration changes.

Source inspection and installed `rustfmt` checks are the local validation.
Hosted CI must run the command and binary-routing fixtures with the combined
source. No local build or test ran, and M4-10 remains incomplete. Tx/query
integration is prepared without compilation or test acceptance; see
[m4-generated-review.md](m4-generated-review.md). Other planned command work
remains pending.
