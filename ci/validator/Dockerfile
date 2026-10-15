# Rust 1.89 slim-bookworm, exact linux/amd64 manifest.
FROM rust:1.89-slim-bookworm@sha256:c9ac3fa8945b61dede1e4500d25028aa8fd8a8fe46365fcf9c0422f8d999b9b0 AS build
WORKDIR /workspace
COPY . .
ARG BUILD_PACKAGE=rtao-neuron
ARG BUILD_BINARY=rtao-validator
ARG BUILD_FEATURES=validator,prometheus
ENV CARGO_TARGET_DIR=/cache/target CARGO_INCREMENTAL=0
# The base supplies rustc's C linker. No build packages are installed here.
RUN --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/cache/target \
    test -f LICENSE-MIT \
    && test ! -L LICENSE-MIT \
    && test "$(stat -c %s LICENSE-MIT)" = 1064 \
    && printf '%s  %s\n' 'ff96227eea95d57ded50c95da31b338bc70cb8880aa34f9062ce33cffa2c7ac7' LICENSE-MIT | sha256sum --check --strict \
    && test -f LICENSE-APACHE \
    && test ! -L LICENSE-APACHE \
    && test "$(stat -c %s LICENSE-APACHE)" = 11358 \
    && printf '%s  %s\n' 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30' LICENSE-APACHE | sha256sum --check --strict \
    && test -f crates/rtao-primitives/LICENSE-PSF \
    && test ! -L crates/rtao-primitives/LICENSE-PSF \
    && test "$(stat -c %s crates/rtao-primitives/LICENSE-PSF)" = 13936 \
    && printf '%s  %s\n' '3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf' crates/rtao-primitives/LICENSE-PSF | sha256sum --check --strict \
    && test -f NOTICE \
    && test ! -L NOTICE \
    && notice_size="$(stat -c %s NOTICE)" \
    && test "$notice_size" -gt 0 && test "$notice_size" -le 1048576 \
    && mkdir -p /out \
    && printf '%s\n' "$notice_size" > /out/NOTICE.size \
    && printf '%s  %s\n' "$(sha256sum NOTICE | cut -d' ' -f1)" /usr/share/licenses/rtao/NOTICE > /out/NOTICE.sha256 \
    && test -f docs/licenses/webpki-root-certs-LICENSE \
    && test ! -L docs/licenses/webpki-root-certs-LICENSE \
    && test "$(stat -c %s docs/licenses/webpki-root-certs-LICENSE)" = 2371 \
    && test -f docs/licenses/Unicode-DFS-2016.txt \
    && test ! -L docs/licenses/Unicode-DFS-2016.txt \
    && test "$(stat -c %s docs/licenses/Unicode-DFS-2016.txt)" = 2315 \
    && printf '%s  %s\n' 'e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8' \
         docs/licenses/webpki-root-certs-LICENSE | sha256sum --check --strict \
    && printf '%s  %s\n' '68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844' \
         docs/licenses/Unicode-DFS-2016.txt | sha256sum --check --strict \
    && rustc --version | grep -E '^rustc 1\.89\.0 ' \
    && cargo tree --locked -p "$BUILD_PACKAGE" --no-default-features \
         --features "$BUILD_FEATURES" --edges normal,build --prefix none \
         --format '{p}' > /tmp/neuron-dependencies.txt \
    && ! grep -E '^(sodiumoxide|libsodium-sys|openssl|openssl-sys|openssl-src|native-tls|fernet|ansible-vault|secp256k1-sys|aws-lc-rs|aws-lc-sys|cmake|hidapi|libloading|rayon) ' /tmp/neuron-dependencies.txt \
    && cargo build --release --locked -p "$BUILD_PACKAGE" --bin "$BUILD_BINARY" \
         --no-default-features --features "$BUILD_FEATURES" \
    && install -Dm755 "$CARGO_TARGET_DIR/release/$BUILD_BINARY" /out/rtao-validator

# Debian bookworm-slim, exact linux/amd64 manifest resolved 2026-10-14.
FROM debian:bookworm-slim@sha256:a4672c0cb26fbdde88e38fa2dfb6c681942306680e41e4378b28770b6e79ee91
LABEL org.opencontainers.image.source="https://github.com/sam0x17/rtao"
COPY --from=build /out/rtao-validator /usr/local/bin/rtao-validator
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build /workspace/LICENSE-MIT /usr/share/licenses/rtao/LICENSE-MIT
COPY --from=build /workspace/LICENSE-APACHE /usr/share/licenses/rtao/LICENSE-APACHE
COPY --from=build /workspace/crates/rtao-primitives/LICENSE-PSF /usr/share/licenses/rtao/LICENSE-PSF
COPY --from=build /workspace/NOTICE /usr/share/licenses/rtao/NOTICE
COPY --from=build /out/NOTICE.size /usr/share/licenses/rtao/NOTICE.size
COPY --from=build /out/NOTICE.sha256 /usr/share/licenses/rtao/NOTICE.sha256
COPY --from=build /workspace/docs/licenses/webpki-root-certs-LICENSE /usr/share/licenses/rtao/webpki-root-certs-LICENSE
COPY --from=build /workspace/docs/licenses/Unicode-DFS-2016.txt /usr/share/licenses/rtao/Unicode-DFS-2016.txt
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
RUN test -f /usr/share/licenses/rtao/LICENSE-MIT \
    && test ! -L /usr/share/licenses/rtao/LICENSE-MIT \
    && test "$(stat -c %s /usr/share/licenses/rtao/LICENSE-MIT)" = 1064 \
    && printf '%s  %s\n' 'ff96227eea95d57ded50c95da31b338bc70cb8880aa34f9062ce33cffa2c7ac7' /usr/share/licenses/rtao/LICENSE-MIT | sha256sum --check --strict \
    && test -f /usr/share/licenses/rtao/LICENSE-APACHE \
    && test ! -L /usr/share/licenses/rtao/LICENSE-APACHE \
    && test "$(stat -c %s /usr/share/licenses/rtao/LICENSE-APACHE)" = 11358 \
    && printf '%s  %s\n' 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30' /usr/share/licenses/rtao/LICENSE-APACHE | sha256sum --check --strict \
    && test -f /usr/share/licenses/rtao/LICENSE-PSF \
    && test ! -L /usr/share/licenses/rtao/LICENSE-PSF \
    && test "$(stat -c %s /usr/share/licenses/rtao/LICENSE-PSF)" = 13936 \
    && printf '%s  %s\n' '3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf' /usr/share/licenses/rtao/LICENSE-PSF | sha256sum --check --strict \
    && test -f /usr/share/licenses/rtao/NOTICE \
    && test ! -L /usr/share/licenses/rtao/NOTICE \
    && test -f /usr/share/licenses/rtao/NOTICE.size && test ! -L /usr/share/licenses/rtao/NOTICE.size \
    && test -f /usr/share/licenses/rtao/NOTICE.sha256 && test ! -L /usr/share/licenses/rtao/NOTICE.sha256 \
    && notice_size="$(stat -c %s /usr/share/licenses/rtao/NOTICE)" \
    && test "$notice_size" -gt 0 && test "$notice_size" -le 1048576 \
    && test "$notice_size" = "$(cat /usr/share/licenses/rtao/NOTICE.size)" \
    && sha256sum --check --strict /usr/share/licenses/rtao/NOTICE.sha256 \
    && test -f /usr/share/licenses/rtao/webpki-root-certs-LICENSE \
    && test ! -L /usr/share/licenses/rtao/webpki-root-certs-LICENSE \
    && test "$(stat -c %s /usr/share/licenses/rtao/webpki-root-certs-LICENSE)" = 2371 \
    && test -f /usr/share/licenses/rtao/Unicode-DFS-2016.txt \
    && test ! -L /usr/share/licenses/rtao/Unicode-DFS-2016.txt \
    && test "$(stat -c %s /usr/share/licenses/rtao/Unicode-DFS-2016.txt)" = 2315 \
    && printf '%s  %s\n' 'e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8' \
        /usr/share/licenses/rtao/webpki-root-certs-LICENSE | sha256sum --check --strict \
    && printf '%s  %s\n' '68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844' \
        /usr/share/licenses/rtao/Unicode-DFS-2016.txt | sha256sum --check --strict \
    && mkdir -p /state /wallet /app \
    && chown 65532:65532 /state /wallet /app \
    && printf 'rtao:x:65532:65532:validator:/app:/usr/sbin/nologin\n' >> /etc/passwd \
    && printf 'rtao:x:65532:\n' >> /etc/group
USER rtao:rtao
WORKDIR /app
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/rtao-validator"]
CMD ["--help"]
