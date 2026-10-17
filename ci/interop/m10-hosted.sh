#!/usr/bin/env bash
# Hosted M10 interop: run every M10 interop harness against pinned upstream
# Python peers on a disposable ubuntu-latest runner.
#
#   synapse        rtao-compat-synapse  <-> bittensor==10.5.0 axon/dendrite
#   epistula       rtao-compat-epistula <-> BitMind / Apex reference code
#   quic           rtao-transport-quic  <-> btlightning Python bindings
#   remote-signer  rtao-remote-signer   <-> bittensor-wallet key code
#
# Upstream inputs are fetched at fixed commits and checked against SHA-256
# digests before use. Each harness exits nonzero on any failed check.
set -euo pipefail
umask 022

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
: "${RTAO_TEMP_ROOT:?RTAO_TEMP_ROOT must be set}"
root="$RTAO_TEMP_ROOT/m10-interop"
mkdir -p "$root"
upstream="$root/upstream"
mkdir -p "$upstream/bitmind-subnet/gas/protocol" "$upstream/apex"

fetch() { # url destination sha256
  curl -fsSL --retry 3 "$1" -o "$2"
  echo "$3  $2" | sha256sum -c --quiet -
}
fetch https://raw.githubusercontent.com/BitMind-AI/bitmind-subnet/5fd96727b8f41d8376348f6dd8a11cd5069bce45/gas/protocol/epistula.py \
  "$upstream/bitmind-subnet/gas/protocol/epistula.py" 815617a5c423bd8db8d0e380040fca68a353e9dccd7aa790fd6d6324fd2cdcb5
fetch https://raw.githubusercontent.com/macrocosm-os/apex/bd71ceb070cd72b4bd80562e96884b66e86d908a/shared/common/src/common/utils/epistula.py \
  "$upstream/apex/epistula.py" 634009ac707a13d6af516b0b8c10bd89d366faf9af7fb41368a63a6308f2d074
fetch https://raw.githubusercontent.com/macrocosm-os/apex/bd71ceb070cd72b4bd80562e96884b66e86d908a/shared/common/src/common/settings.py \
  "$upstream/apex/settings.py" 27d7d1bfd6d221d0ab18b462944c79c407055f28b4d63d38b9eb9e2066344f8f

# One Python 3.12 environment per hash-pinned requirement set.
python3 -m venv "$root/synapse-py"
"$root/synapse-py/bin/pip" install --quiet --require-hashes -r "$repo/ci/interop/synapse/requirements.txt"
python3 -m venv "$root/epistula-py"
"$root/epistula-py/bin/pip" install --quiet -r "$repo/ci/interop/epistula/requirements.txt"

# btlightning Python bindings at the reviewed commit, built outside the rtao
# checkout so rtao's .cargo/config.toml (PYO3_NO_PYTHON=1) does not apply.
git clone --quiet https://github.com/inference-labs-inc/btlightning.git "$upstream/btlightning"
git -C "$upstream/btlightning" checkout --quiet 53c090b436238f78a4a0dd30cbbfcba1cbebe2e0
python3 -m venv "$root/quic-py"
"$root/quic-py/bin/pip" install --quiet "maturin>=1.0,<2.0"
(cd "$upstream/btlightning/crates/btlightning-py" &&
  CARGO_TARGET_DIR="$root/btlightning-target" VIRTUAL_ENV="$root/quic-py" \
    PATH="$root/quic-py/bin:$PATH" maturin develop --release --quiet)

cd "$repo"
PYTHON="$root/synapse-py/bin/python" bash ci/interop/synapse/run.sh
EPISTULA_PY="$root/epistula-py/bin/python" EPISTULA_UPSTREAM="$upstream" bash ci/interop/epistula/run.sh
PYTHON="$root/quic-py/bin/python" bash ci/interop/quic/run.sh
RTAO_INTEROP_PYTHON="$root/synapse-py/bin/python" bash ci/interop/remote-signer/run.sh
echo "M10 hosted interop: ok"
