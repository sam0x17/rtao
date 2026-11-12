#!/usr/bin/env bash
set -euo pipefail
sdk=$(cd "${1:?SDK checkout is required}" && pwd -P)
test -f "$sdk/crates/rtao/Cargo.toml"
test -f "$sdk/crates/rtao-testkit/Cargo.toml"
# Build against an SDK checkout instead of the crates.io release: its rtao
# crates replace the registry ones, and their workspace paths bring the rest.
# Only this final source-patch section is owned by the integration tooling.
# The template itself carries no [patch] (D-183).
awk '/^\[patch\.crates-io\]$/ { exit } { print }' Cargo.toml > Cargo.toml.source-patch
mv Cargo.toml.source-patch Cargo.toml
printf '\n[patch.crates-io]\n' >> Cargo.toml
for package in rtao rtao-testkit rtao-runtime; do
  printf '%s = { path = "%s/crates/%s" }\n' "$package" "$sdk" "$package" >> Cargo.toml
done
