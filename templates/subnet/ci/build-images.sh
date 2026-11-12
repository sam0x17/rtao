#!/usr/bin/env bash
set -euo pipefail
: "${RTAO_SOURCE:?Set the resolved SDK checkout from ci/source.sh; for the crates.io release, check out the rtao-v$RTAO_VERSION tag of https://github.com/sam0x17/rtao and run bash ci/patch-source.sh on it first}"
for role in validator miner; do
  docker buildx build --load --build-context "rtao=$RTAO_SOURCE" \
    --build-arg "BUILD_PACKAGE=subnet-$role" \
    --tag "subnet-$role:local" --file Dockerfile .
done
