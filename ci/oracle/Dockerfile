# Uses the frontend bundled with the workflow's digest-pinned BuildKit.
FROM ghcr.io/astral-sh/uv@sha256:aa49a668339c61efa77057114bf6d58c3d69b4a6bdd2a4f1aacac70b408d6187 AS uv
FROM rust@sha256:c9ac3fa8945b61dede1e4500d25028aa8fd8a8fe46365fcf9c0422f8d999b9b0

ARG PARITY_COMMIT
ARG PARITY_TAG
ARG ORACLE_INPUT_SHA256
LABEL org.opencontainers.image.source="https://github.com/sam0x17/rtao" \
      org.opencontainers.image.revision="$PARITY_COMMIT" \
      io.rtao.oracle.parity-tag="$PARITY_TAG" \
      io.rtao.oracle.input-sha256="$ORACLE_INPUT_SHA256"
ENV UV_PYTHON=3.12.11 UV_PYTHON_DOWNLOADS=automatic UV_LINK_MODE=copy \
    UV_CACHE_DIR=/cache/uv CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/build \
    BITTENSOR_RUNTIME_CACHE_DIR=/scratch/runtime-cache PYTHONHASHSEED=0 \
    RTAO_ORACLE_PYTHON=3.12.11 RTAO_ORACLE_UV=0.8.24 \
    PYTHONPATH=/upstream/sdk/python:/oracle
COPY --from=uv /uv /uvx /usr/local/bin/
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates git pkg-config libssl-dev perl clang libclang-dev libudev-dev \
    && rm -rf /var/lib/apt/lists/*
RUN printf '%s' "$PARITY_COMMIT" | grep -Eq '^[0-9a-f]{40}$' \
    && printf '%s' "$PARITY_TAG" | grep -Eq '^v[0-9]+$' \
    && git init /upstream && git -C /upstream remote add origin https://github.com/RaoFoundation/subtensor.git \
    && git -C /upstream fetch --depth=1 origin "refs/tags/$PARITY_TAG" \
    && git -C /upstream checkout --detach FETCH_HEAD \
    && test "$(git -C /upstream rev-parse HEAD)" = "$PARITY_COMMIT"
WORKDIR /upstream/sdk/python
COPY build-requirements.txt /oracle/build-requirements.txt
# Frozen uv resolves the checked-out path dependency, never a PyPI core wheel.
# Maturin receives --locked so upstream Cargo.lock defines the wire/crypto graph.
# Preinstall hashed build backends and disable isolation, which otherwise resolves
# floating pyproject build requirements outside the frozen upstream uv.lock.
RUN --mount=type=cache,target=/cache/uv --mount=type=cache,target=/cache/cargo --mount=type=cache,target=/cache/build \
    uv python install 3.12.11 \
    && uv sync --frozen --group dev --no-install-local --no-build \
    && uv pip install --python .venv/bin/python --no-deps --require-hashes --only-binary :all: \
         -r /oracle/build-requirements.txt \
    && uv sync --frozen --group dev --inexact --no-build-isolation --config-settings=build-args=--locked
COPY . /oracle/
RUN .venv/bin/python /oracle/image_identity.py \
         --commit "$PARITY_COMMIT" --tag "$PARITY_TAG" --input-sha256 "$ORACLE_INPUT_SHA256"
ENTRYPOINT ["/upstream/sdk/python/.venv/bin/python", "/oracle/run.py"]
