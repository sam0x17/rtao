#!/usr/bin/env bash
set -euo pipefail
: "${RTAO_SOURCE:?Set the resolved SDK source from ci/source.sh}"
for role in validator miner; do
  docker buildx build --load --build-context "rtao=$RTAO_SOURCE" \
    --build-arg "BUILD_PACKAGE=subnet-$role" \
    --tag "subnet-$role:local" --file Dockerfile .
done
