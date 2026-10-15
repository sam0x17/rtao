#!/usr/bin/env bash
set -euo pipefail
sdk=$(cd "${1:?SDK checkout is required}" && pwd -P)
test -f "$sdk/crates/rtao/Cargo.toml"
test -f "$sdk/crates/rtao-testkit/Cargo.toml"
# Only this final source-patch section is owned by the integration tooling.
# The canonical BLS patch stays in the preceding crates.io section.
awk '/^\[patch\."https:\/\/github.com\/sam0x17\/rtao"\]$/ { exit } { print }' Cargo.toml > Cargo.toml.source-patch
mv Cargo.toml.source-patch Cargo.toml
printf '\n[patch."https://github.com/sam0x17/rtao"]\n' >> Cargo.toml
for package in rtao rtao-testkit rtao-runtime; do
  printf '%s = { path = "%s/crates/%s" }\n' "$package" "$sdk" "$package" >> Cargo.toml
done
