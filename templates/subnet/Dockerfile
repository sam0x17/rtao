# syntax=docker/dockerfile:1.7
# Supply the resolved SDK checkout as the named context `rtao` (ci/build-images.sh).
FROM rust:1.89-slim-bookworm@sha256:c9ac3fa8945b61dede1e4500d25028aa8fd8a8fe46365fcf9c0422f8d999b9b0 AS build
WORKDIR /workspace
COPY --from=rtao / /rtao-src
COPY . .
ENV CARGO_TARGET_DIR=/cache/target CARGO_INCREMENTAL=0 PYO3_NO_PYTHON=1
ARG BUILD_PACKAGE
# The image already supplies the C compiler used by ring/blake3. No Perl/make.
RUN --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/cache/target \
    test -f /rtao-src/LICENSE-MIT \
    && test ! -L /rtao-src/LICENSE-MIT \
    && test "$(stat -c %s /rtao-src/LICENSE-MIT)" = 1064 \
    && printf '%s  %s\n' 'ff96227eea95d57ded50c95da31b338bc70cb8880aa34f9062ce33cffa2c7ac7' /rtao-src/LICENSE-MIT | sha256sum --check --strict \
    && test -f /rtao-src/LICENSE-APACHE \
    && test ! -L /rtao-src/LICENSE-APACHE \
    && test "$(stat -c %s /rtao-src/LICENSE-APACHE)" = 11358 \
    && printf '%s  %s\n' 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30' /rtao-src/LICENSE-APACHE | sha256sum --check --strict \
    && test -f /rtao-src/crates/rtao-primitives/LICENSE-PSF \
    && test ! -L /rtao-src/crates/rtao-primitives/LICENSE-PSF \
    && test "$(stat -c %s /rtao-src/crates/rtao-primitives/LICENSE-PSF)" = 13936 \
    && printf '%s  %s\n' '3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf' /rtao-src/crates/rtao-primitives/LICENSE-PSF | sha256sum --check --strict \
    && test -f /rtao-src/NOTICE \
    && test ! -L /rtao-src/NOTICE \
    && notice_size="$(stat -c %s /rtao-src/NOTICE)" \
    && test "$notice_size" -gt 0 && test "$notice_size" -le 1048576 \
    && mkdir -p /out \
    && printf '%s\n' "$notice_size" > /out/NOTICE.size \
    && printf '%s  %s\n' "$(sha256sum /rtao-src/NOTICE | cut -d' ' -f1)" /usr/share/licenses/subnet/NOTICE > /out/NOTICE.sha256 \
    && test -f /rtao-src/docs/licenses/webpki-root-certs-LICENSE \
    && test ! -L /rtao-src/docs/licenses/webpki-root-certs-LICENSE \
    && test "$(stat -c %s /rtao-src/docs/licenses/webpki-root-certs-LICENSE)" = 2371 \
    && test -f /rtao-src/docs/licenses/Unicode-DFS-2016.txt \
    && test ! -L /rtao-src/docs/licenses/Unicode-DFS-2016.txt \
    && test "$(stat -c %s /rtao-src/docs/licenses/Unicode-DFS-2016.txt)" = 2315 \
    && printf '%s  %s\n' \
        'e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8' /rtao-src/docs/licenses/webpki-root-certs-LICENSE \
        '68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844' /rtao-src/docs/licenses/Unicode-DFS-2016.txt | sha256sum --check --strict \
    && bash ci/patch-source.sh /rtao-src \
    && cargo tree --locked -p "$BUILD_PACKAGE" --edges normal,build --prefix none --format '{p}' > /tmp/deps.txt \
    && ! grep -E '^(sodiumoxide|libsodium-sys|openssl|openssl-sys|openssl-src|native-tls|fernet|ansible-vault|secp256k1-sys|aws-lc-rs|aws-lc-sys|cmake|hidapi|libloading|rayon) ' /tmp/deps.txt \
    && cargo build --locked --release -p "$BUILD_PACKAGE" \
    && install -Dm755 "$CARGO_TARGET_DIR/release/$BUILD_PACKAGE" /out/neuron \
    && install -Dm644 /rtao-src/LICENSE-MIT /out/LICENSE-MIT \
    && install -Dm644 /rtao-src/LICENSE-APACHE /out/LICENSE-APACHE \
    && install -Dm644 /rtao-src/crates/rtao-primitives/LICENSE-PSF /out/LICENSE-PSF \
    && install -Dm644 /rtao-src/NOTICE /out/NOTICE \
    && cmp /rtao-src/NOTICE /out/NOTICE \
    && install -Dm644 /rtao-src/docs/licenses/webpki-root-certs-LICENSE /out/webpki-root-certs-LICENSE \
    && install -Dm644 /rtao-src/docs/licenses/Unicode-DFS-2016.txt /out/Unicode-DFS-2016.txt

FROM debian:bookworm-slim@sha256:a4672c0cb26fbdde88e38fa2dfb6c681942306680e41e4378b28770b6e79ee91 AS runtime
COPY --from=build /out/neuron /usr/local/bin/neuron
COPY --from=build /out/LICENSE-MIT /usr/share/licenses/subnet/LICENSE-MIT
COPY --from=build /out/LICENSE-APACHE /usr/share/licenses/subnet/LICENSE-APACHE
COPY --from=build /out/LICENSE-PSF /usr/share/licenses/subnet/LICENSE-PSF
COPY --from=build /out/NOTICE /usr/share/licenses/subnet/NOTICE
COPY --from=build /out/NOTICE.size /usr/share/licenses/subnet/NOTICE.size
COPY --from=build /out/NOTICE.sha256 /usr/share/licenses/subnet/NOTICE.sha256
COPY --from=build /out/webpki-root-certs-LICENSE /usr/share/licenses/subnet/webpki-root-certs-LICENSE
COPY --from=build /out/Unicode-DFS-2016.txt /usr/share/licenses/subnet/Unicode-DFS-2016.txt
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN test -f /usr/share/licenses/subnet/LICENSE-MIT \
    && test ! -L /usr/share/licenses/subnet/LICENSE-MIT \
    && test "$(stat -c %s /usr/share/licenses/subnet/LICENSE-MIT)" = 1064 \
    && printf '%s  %s\n' 'ff96227eea95d57ded50c95da31b338bc70cb8880aa34f9062ce33cffa2c7ac7' /usr/share/licenses/subnet/LICENSE-MIT | sha256sum --check --strict \
    && test -f /usr/share/licenses/subnet/LICENSE-APACHE \
    && test ! -L /usr/share/licenses/subnet/LICENSE-APACHE \
    && test "$(stat -c %s /usr/share/licenses/subnet/LICENSE-APACHE)" = 11358 \
    && printf '%s  %s\n' 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30' /usr/share/licenses/subnet/LICENSE-APACHE | sha256sum --check --strict \
    && test -f /usr/share/licenses/subnet/LICENSE-PSF \
    && test ! -L /usr/share/licenses/subnet/LICENSE-PSF \
    && test "$(stat -c %s /usr/share/licenses/subnet/LICENSE-PSF)" = 13936 \
    && printf '%s  %s\n' '3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf' /usr/share/licenses/subnet/LICENSE-PSF | sha256sum --check --strict \
    && test -f /usr/share/licenses/subnet/NOTICE \
    && test ! -L /usr/share/licenses/subnet/NOTICE \
    && test -f /usr/share/licenses/subnet/NOTICE.size && test ! -L /usr/share/licenses/subnet/NOTICE.size \
    && test -f /usr/share/licenses/subnet/NOTICE.sha256 && test ! -L /usr/share/licenses/subnet/NOTICE.sha256 \
    && notice_size="$(stat -c %s /usr/share/licenses/subnet/NOTICE)" \
    && test "$notice_size" -gt 0 && test "$notice_size" -le 1048576 \
    && test "$notice_size" = "$(cat /usr/share/licenses/subnet/NOTICE.size)" \
    && sha256sum --check --strict /usr/share/licenses/subnet/NOTICE.sha256 \
    && test -f /usr/share/licenses/subnet/webpki-root-certs-LICENSE \
    && test ! -L /usr/share/licenses/subnet/webpki-root-certs-LICENSE \
    && test "$(stat -c %s /usr/share/licenses/subnet/webpki-root-certs-LICENSE)" = 2371 \
    && test -f /usr/share/licenses/subnet/Unicode-DFS-2016.txt \
    && test ! -L /usr/share/licenses/subnet/Unicode-DFS-2016.txt \
    && test "$(stat -c %s /usr/share/licenses/subnet/Unicode-DFS-2016.txt)" = 2315 \
    && printf '%s  %s\n' \
        'e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8' /usr/share/licenses/subnet/webpki-root-certs-LICENSE \
        '68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844' /usr/share/licenses/subnet/Unicode-DFS-2016.txt | sha256sum --check --strict \
    && mkdir -p /state /wallet /app \
    && chown 65532:65532 /state /wallet /app \
    && printf 'subnet:x:65532:65532:subnet neuron:/app:/usr/sbin/nologin\n' >> /etc/passwd \
    && printf 'subnet:x:65532:\n' >> /etc/group
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    BITTENSOR_RUNTIME_CACHE_DIR=/state/runtime-cache \
    BTCLI_CONFIG=/state/btcli.json \
    BTCLI_TOKEN_SYMBOLS_CACHE=/state/token-symbols.json
USER 65532:65532
WORKDIR /app
ENTRYPOINT ["/usr/local/bin/neuron"]
CMD ["--help"]
