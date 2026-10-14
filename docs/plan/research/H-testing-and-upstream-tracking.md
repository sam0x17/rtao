# H — Keeping rtao correct and in sync with upstream: testing and upstream-tracking strategy

Date: 2026-10-13. Evidence base: RaoFoundation/subtensor monorepo at commit `850f275` (main, runtime `spec_version: 478`), live finney/testnet metadata V16 at spec 477, GitHub releases/ghcr/PyPI state as of today, and the Rust ecosystem clones.

Path abbreviations used in citations:

- `ST/` = `SP/upstream/subtensor/`
- `META/` = `SP/meta/`
- `DOCS/` = `SP/docs/`
- `RT/` = `SP/research/tmp/` (artifacts I produced while verifying: `golden_461.scale`, `golden_461.json`, `diff_461_477.txt`, `call_indices.py`, `err_ev_indices.py`)
- `ECO/` = `SP/upstream/` (ecosystem clones)

Every claim below is either cited to a file:line, to a command I ran (output summarized), or explicitly marked **inferred**.

---

## 0. TL;DR (what matters for the plan)

1. **Upstream publishes no metadata artifact.** GitHub release `v<spec>` assets are exactly: `subtensor.wasm`, `subtensor-digest.json` (srtool), `proxy_proxy_blob.hex`, `pending-release.json`, `upgrade-manifest.json` (`gh release view v477/v475/v466`, confirmed identical asset sets; publisher logic at `ST/.github/workflows/release-train.yml:664-698` and `ST/.github/workflows/watch-mainnet-release.yml:283-341`). rtao must derive metadata itself: (a) from a live node (`state_call Metadata_metadata_at_version`), (b) from the `:v<spec>` localnet image, or (c) by executing the released wasm. The srtool digest does embed the runtime-API version table (`apis`), `blake2_256` of the wasm and `transactionVersion` (verified by downloading v477's `subtensor-digest.json`), which is a free pre-check.
2. **Immutable per-spec pins exist for images, not metadata:** `ghcr.io/raofoundation/subtensor-localnet:{v432,v473,v475,v477,devnet,testnet,mainnet,main,latest,sha-<40hex>}` and `ghcr.io/raofoundation/subtensor:{v432,v448,v473,v475,v477,devnet,testnet,mainnet,main,latest,...}` (ghcr `tags/list`, today). Only releases since v473 have per-spec localnet images (plus v432).
3. **Release cadence is fast**: v466 (2026-09-18) → v477 (2026-10-08) = 9 finalized mainnet runtimes in 20 days; `spec_version` is already 478 on `main` (`ST/runtime/src/lib.rs:241`). Trains run on every push to `main` touching runtime paths (`release-train.yml:37-47`); devnet deploys automatically, testnet needs one click, mainnet needs a 2-of-3 multisig (`ST/docs/internals/release-process.mdx:44-72`).
4. **What is stable across runtimes (measured 461→477, 16 runtimes):** pallet indices unchanged; call indices append-only (SubtensorModule +151/+152, AdminUtils +110..113; nothing moved); error indices unchanged; transaction-extension list unchanged (13 identifiers); `transaction_version` = 1 throughout. **What is not stable:** event indices (`SubtensorModule` event 149 changed meaning: `BasketSwapped`→`SubnetLeaseDividendSkipped`; `BasketSwapped` moved to 151), storage key/value types and default values (e.g. `LastRateLimitedBlock` key type, `System.Events` value type), storage items removed (`Alpha`, `AlphaMapLastKey`, `TotalHotkeyShares`) (`RT/diff_461_477.txt`; `RT/call_indices.py`, `RT/err_ev_indices.py` outputs in §2.3). `subxt diff` does **not** report events at all — rtao's drift tool must.
5. **Upstream's own Python SDK does not signal runtime compatibility by version**; it fetches metadata per `spec_version` at runtime, caches it on disk keyed by `(genesis, spec_version)` (`ST/sdk/python/bittensor/_transport/runtime.py:64-66`), and relies on a CI *drift gate* (`codegen.check --drift`) to keep committed descriptors equal to a node's metadata (`ST/sdk/python/codegen/check.py:32-53`). SDK versions are bumped by hand alongside spec bumps when needed (release v477 body: "Bump runtime spec to 477 and Python release versions", PR #3221); rc versions are `X.Y.ZrcN` with `N = GITHUB_RUN_NUMBER` of the release train (`release-train.yml:348-349`), and stable publication is gated on the mainnet upgrade having executed (`watch-mainnet-release.yml:343-480`). PyPI today: `bittensor` 11.3.0 stable (2026-10-07), 11.3.1rc68 (2026-10-08); `bittensor-core` 0.1.5 stable, 0.1.6rc68.
6. **Golden fixtures that rtao can import today** (all Apache-2.0 in-repo, no Python needed to *read* them): `ST/sdk/python/tests/fixtures/golden.json` (1.5 MB; storage keys, call encodings, signing payloads, fake-signed extrinsics, multisig derivations, ss58 vectors, decoded constants, pinned storage/event/runtime-API/block vectors, plus the raw V15 and V14 metadata they were recorded against — spec 461), `ST/sdk/python/tests/fixtures/shape_corpus/corpus.json` (1.1 MB; 797/802 portable-registry types × 3+ samples of `(type id, SCALE bytes, decoded JSON shape)`, spec 461), btauth/1 vectors (inline in `ST/sdk/python/tests/unit/test_http_auth.py:20-36` and normative in `ST/docs/guides/signed-requests.mdx:252-289`), RFC-0078 digest vector `affbcc25…11c0` for the 461 metadata (`ST/sdk/bittensor-core/src/digest/mod.rs:184-201`), the Ledger proof vector (`ST/sdk/bittensor-core/fixtures/ledger_proof_vector.json`), and keyfile/legacy-Fernet parameters (`ST/sdk/python/tests/unit/test_keyfiles_golden.py:66-84`).
7. **Recommended core loop for rtao:** pin metadata per network in-repo (never fetch at build time — two ecosystem crates do and their builds are non-reproducible, see §6), run a nightly drift job that fetches finney/testnet/devnet + `:main` localnet metadata, diffs semantically (calls/storage/events/errors/constants/runtime-API versions/tx-extensions, with index-level checks), and opens a PR with regenerated bindings; run e2e in docker against `:v<pinned-spec>`, `:testnet`, `:devnet` (and `:main` as allowed-failure); import the upstream golden/shape-corpus/btauth vectors as conformance tests; record rtao's own fixtures from the localnet image with a Rust `xtask`, mirroring upstream's recorder shapes so the upstream corpus remains a drop-in oracle.

---

## 1. Map of upstream artifacts rtao can consume mechanically

### 1.1 Runtime metadata

**Live endpoints (verified in CI config):**

| Network | WS (used by release train / smoke) | HTTP JSON-RPC (used by deploy guards) | Archive WS (try-runtime) | Genesis hash |
|---|---|---|---|---|
| finney (mainnet) | `wss://entrypoint-finney.opentensor.ai:443` (`release-train.yml:59`); smoke uses `wss://openrpc.taostats.io` (`ST/ts-tests/moonwall.config.json`, env `smoke_mainnet`) | `https://entrypoint-finney.opentensor.ai` (`release-train.yml:431`) | `wss://archive.dev.opentensor.ai:443` (`ST/.github/workflows/runtime-checks.yml:324-336`) | `0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03` (`runtime-checks.yml:336`) |
| testnet | `wss://test.finney.opentensor.ai:443` (`release-train.yml:58`) | `https://test.finney.opentensor.ai` (`release-train.yml:235`) | `wss://archive.dev.opentensor.ai:8443` (`runtime-checks.yml:333`) | `0x8f9cf856bf558a14440e75569c9e58594757048d7b3a84b5d25f6bd978263105` |
| devnet | `wss://dev.chain.opentensor.ai:443` (`release-train.yml:57`) | `https://dev.chain.opentensor.ai` (`release-train.yml:136`) | same as WS | `0x077899043eb684c5277b6814a39161f4ce072b45e782e12c81a521c63fb4f3e5` |

The genesis hashes are what `runtime-checks.yml:351` uses to assert endpoint identity; rtao's network table should pin the same three and refuse to sign against an endpoint whose genesis mismatches.

**How upstream fetches metadata (what to mirror):**

- Python SDK: `state_call("Metadata_metadata_at_version", "0x0f000000", at_hash)` → V15, falling back to `state_getMetadata` (V14) when the runtime API method is missing (`ST/sdk/python/bittensor/_transport/runtime.py:9-11,33-34,293-307`). The golden recorder stores both V15 and V14 blobs (`ST/sdk/python/scripts/record_golden.py:162-167`).
- Rust core: same runtime API with a requested version, retaining V14 as compatibility (`ST/sdk/bittensor-core/src/client.rs:1230-1253`).
- subxt 0.51 fetches the **latest** version, which is V16 on the current chain (`META/finney.json` top-level `{"V16": …}`; subxt supports V16 since 0.42.0 per its CHANGELOG, "Support v16 metadata and use it by default if it's available", PR #1999).
- V16 adds data V15 lacks and that rtao should exploit: per-call `deprecation_info` (e.g. SubtensorModule calls 70, 73, 139, 140 carry deprecation notes — `jq '.[1].V16.pallets[]|select(.name=="SubtensorModule").calls.deprecation_info' META/finney.json`), supported extrinsic versions `[4, 5]`, and `transaction_extensions_by_version` (`{"0": [0..12]}`) (`jq '.[1].V16.extrinsic' META/finney.json`).
- The RFC-0078 metadata digest (`CheckMetadataHash`) must be computed over **V15** — upstream computes it with `merkleized-metadata` 0.5.1 against `frame-metadata` 23 (`ST/sdk/bittensor-core/Cargo.toml:69-73`) and the runtime bakes it with `enable_metadata_hash("TAO", 9)` (`ST/runtime/build.rs:10-16`). So rtao needs V15 for digests even if it uses V16 for codegen; fetch both and store both.

**Is metadata published per release? No.** Confirmed by listing assets of three finalized releases (v477, v475, v466): only the five files listed in §0 item 1. The srtool digest (`subtensor-digest.json`) is the closest thing to a manifest: it contains `runtimes.{compact,compressed}.subwasm.core_version` (specName, specVersion, implVersion, `apis` as `[[hex id, version]…]`, `transactionVersion`, `stateVersion`), `blake2_256` of the wasm, `proposal_hash`, size, and the `rustc` used (downloaded v477 digest; `gen: "srtool v0.18.3"`, rustc 1.89.0). `upgrade-manifest.json` adds `spec_version`, `commit`, `wasm_sha256`, `call_hash`, multisig addresses and asset URLs (`release-train.yml:533-570`; verified content for v477).

**How to pin metadata per `spec_version` (three mechanical options, in order of preference):**

1. **From the `:v<spec>` localnet image** — start `ghcr.io/raofoundation/subtensor-localnet:v477`, wait for the healthcheck, call `Metadata_metadata_at_version` for 15 and 16. Caveat (verified): the localnet runtime is built with features `pow-faucet metadata-hash fast-runtime` (`ST/scripts/localnet.sh:40,47`) after `scripts/localnet_patch.sh` edits two rate-limit constants (`ST/scripts/localnet_patch.sh:50-62`). `pow-faucet` adds `SubtensorModule::faucet` at `call_index(60)` (`ST/pallets/subtensor/src/macros/dispatches.rs:1056-1061`) which is absent from finney metadata (`grep -c '"name": "faucet"' META/finney.json` → 0), and `fast-runtime` changes `prod_or_fast!` constants (`MILLISECS_PER_BLOCK` 12000→250, `ST/common/src/lib.rs:183`; tempo 360→10 `ST/runtime/src/lib.rs:904`; `SubtensorMaxEpochsPerBlock` 2→32 `:987`; etc.). The image also ships a `non-fast-runtime` binary (`pow-faucet metadata-hash` only) at `/target/non-fast-runtime/release/node-subtensor` (`ST/Dockerfile-localnet:99-107`; run the image with `False` to use it, `Dockerfile-localnet:95-96`). **Therefore localnet metadata ≠ finney metadata** (one extra call, different constant defaults). Use the image for *behavioral* e2e and for *structural* diffs that ignore constant values and the faucet call; use the live network (option 2) for the canonical per-spec pin.
2. **From the live network at the first block of the new runtime** — the watcher resolves the finalized head and `state_getStorageHash(":code")` via plain JSON-RPC (`ST/.github/scripts/mainnet-finalized-runtime.sh:19-26`); rtao can do the same, then `state_call Metadata_metadata_at_version` at that hash. Testnet and finney run byte-identical runtimes once promoted (release-process.mdx:49-51; verified today: `META/finney.scale` and `META/test.scale` are byte-identical, sha256 `af0870c1…acba`, both spec 477), so testnet is a ~1-day-early preview of the finney metadata, and devnet earlier still.
3. **From the released `subtensor.wasm`** by executing `Metadata_metadata_at_version` in a wasm executor. `subwasm` (chevdor; latest GitHub release v0.21.3, 2024-05-31; crates.io entry is a 0.0.0 placeholder) does this for `Metadata_metadata`; whether its current release can request V15/V16 specifically is **not verified** — treat as optional tooling. A small rtao `xtask` using `sc-executor` from the RaoFoundation polkadot-sdk fork rev `cacb431…` (`ST/Cargo.toml:150,227`) would be fully deterministic and offline; cost is a heavy dependency tree (inferred).

**Pinning policy recommendation:** commit `metadata/<network>/<spec_version>.v15.scale` and `.v16.scale` plus a `metadata/index.toml` row `{network, spec_version, block_hash, code_hash (blake2_256 of :code), sha256(v15), sha256(v16), recorded_at}`. Key by `(genesis_hash[..16], spec_version)` exactly like upstream's disk cache (`runtime.py:64-66`), so rtao's runtime cache and its fixture index agree.

### 1.2 GitHub releases: tags, assets, cadence

Tags since the train switched to immutable `v<spec>` lightweight tags (v430+; earlier tags were `v3.4.x-<spec>` and releases ≤ v432 are "historical state" handled manually — `watch-mainnet-release.yml:87-105`):

| Tag | Published (UTC) | State |
|---|---|---|
| v466 | 2026-09-18 01:00 | final |
| v467 | 2026-09-18 16:33 | final |
| v468 | 2026-09-21 18:18 | final |
| v469 | 2026-09-22 09:35 | final |
| v470 | 2026-09-24 08:18 | pre-release ("proposed"), never finalized |
| v471 | 2026-09-25 17:14 | pre-release, never finalized |
| v473 | 2026-10-05 13:52 | final |
| v475 | 2026-10-07 20:40 | final |
| v476 | 2026-10-08 19:00 | pre-release, superseded same day |
| v477 | 2026-10-08 21:55 | final, `latest` |
| (478) | merged to main 2026-10-13 01:50 (PR #3237 "Release 478") | in train |

(`gh release list --repo RaoFoundation/subtensor`; PR #3237 via `gh pr view`.) Observations: specs are not contiguous on mainnet (464, 465?, 472, 474 never finalized), "proposed" pre-releases can be abandoned, and the gap between consecutive finalized mainnet runtimes ranged from ~10 hours to ~13 days in this window. rtao's compat matrix must therefore be expressed as **ranges of spec_version** validated against metadata, not as a list of tags.

Assets per final release (verified v477/v475/v466): `subtensor.wasm` (~2.5–2.6 MB compact.compressed), `subtensor-digest.json` (4969 B), `proxy_proxy_blob.hex` (~5.1–5.3 MB), `pending-release.json`, `upgrade-manifest.json`. Pre-release bodies carry the signer instructions and a table with spec, commit, call hash, timepoint, CI key, deployment multisig, wasm sha256 (`release-train.yml:616-662`). Release notes are GitHub auto-generated PR lists (`watch-mainnet-release.yml:332-333`) — useful as a changelog feed for the drift PR.

Mapping a release to source: the tag is a lightweight tag at the exact train commit (`release-train.yml:472-505`); the `mainnet` branch mirror is force-moved to it after finalization (`watch-mainnet-release.yml:266-273`), `devnet`/`testnet` mirrors are moved at deploy time (`release-train.yml:182-192,270-280`). So `git fetch origin mainnet testnet devnet` gives the exact code running on each network at any time.

### 1.3 Docker images

**Packages and tag policy (verified against ghcr today and the publisher scripts):**

- `ghcr.io/raofoundation/subtensor-localnet` — the chain-in-a-box. Tags: moving `main`, `devnet`, `testnet`, `mainnet`, `latest` (= `main`; verified identical digests `c08b3f05…/801a74c6…`), immutable `v<spec>` only for mainnet builds (`v432`, `v473`, `v475`, `v477`; `mainnet` == `v477` today, digests `5dd1dc94…/f0342ddc…`), and `sha-<40-hex>` per published commit (57 today). Policy: `ST/.github/scripts/resolve-localnet-image.sh:34-41` (tag = branch name; `latest` only from `main`), `ST/.github/scripts/publish-localnet-manifest.sh:41-63` (adds `sha-$SHA`, `v<N>` when `RELEASE_TAG` set, `latest` when `PUBLISH_LATEST`). Multi-arch index `linux/amd64` + `linux/arm64` with OCI annotations description/source/licenses (verified for `v477`, `testnet`, `devnet`, `main`, `latest`). Every tag is validated by an anonymous pull + `--help` run before the workflow is green (`ST/.github/workflows/docker-localnet.yml:319-378`). Stale: `ci-<sha>` tags also exist in the public package from an older flow; current PR images go to `ghcr.io/raofoundation/subtensor-localnet-ci:ci-<sha>` (`ST/.github/workflows/check-bittensor-e2e-tests.yml:42`, 277 tags today, pruned after 30 days by `cleanup-localnet-ci-images.yml:23`).
- `ghcr.io/raofoundation/subtensor` — production node image (`Dockerfile.prebuilt`), tags `main`, `devnet`, `testnet`, `mainnet`, `latest` (= mainnet), `v<spec>` for mainnet builds (`v432`, `v448`, `v473`, `v475`, `v477`), plus a few manual refs (`devnet-ready`, `fix-finney-light-sync-checkpoint`). Policy: `ST/.github/scripts/resolve-node-image.sh:2-5,23-33`; `ST/.github/workflows/docker.yml:3-9`.

**Image contents and contract (`ST/Dockerfile-localnet`):**

- Binaries: `/target/fast-runtime/release/node-subtensor` and `/target/non-fast-runtime/release/node-subtensor` plus each runtime's `node_subtensor_runtime.compact.compressed.wasm` (`:99-107`). `snapshot.json` is copied in (`:81`) but it is only read by the finney/testfinney chain specs (`ST/node/src/chain_spec/finney.rs:8`, `testnet.rs:9`), not by `local`.
- Entrypoint `/scripts/localnet.sh`, default CMD `True` (fast blocks); pass `False` for 12 s blocks (`:94-96`). Env `BUILD_BINARY=0`, `RUN_IN_DOCKER=1` (`:86-87`); `RUN_IN_DOCKER` adds `--unsafe-rpc-external` to all three nodes (`ST/scripts/localnet.sh:274-278`).
- Ports: 30334/30335/30336 p2p and **9944/9945/9946 RPC** for authorities One/Two/Three (`localnet.sh:217-271`, `Dockerfile-localnet:89`). The chain spec is generated at container start with `build-spec --chain local --raw` (`localnet.sh:157`), so genesis hash differs per container run (**inferred** from `--chain local` + fresh key generation; the Python golden fixture's genesis `0x1dc00e76…` is one such run).
- Healthcheck: `/scripts/localnet_healthcheck.sh` requires `chain_getHeader` to return a non-zero block number (`ST/scripts/localnet_healthcheck.sh:13-16`), `--interval=5s --timeout=3s --start-period=90s --retries=6` (`Dockerfile-localnet:91-92`). The Rust e2e harness instead polls `docker logs` for the string `Imported #1` with a 180 s deadline (`ST/sdk/bittensor-core/tests/e2e_support/mod.rs:24,101-120`).
- If any authority process exits, the container exits non-zero (`localnet.sh:291-300`); CI proves this plus `--no-purge` persistence through a `/tmp` volume (`ST/.github/scripts/test-localnet-image.sh:101-157`; documented at `ST/docs/guides/local-development.mdx:37-45`).
- Runtime override: the `local` and `dev` specs load `SUBTENSOR_LOCALNET_RUNTIME_WASM` if set (`ST/node/src/chain_spec/localnet.rs:6-26`); the CI image sets it (`ST/sdk/bittensor-core/tests/Dockerfile.localnet-fast:33-37`). rtao can reuse this to boot **any released `subtensor.wasm` on the latest node binary** — i.e. a per-spec localnet without a per-spec image, as long as node-level APIs are unchanged (what `verify-base-node-runtime.py:10-15` checks upstream).

**Funded genesis accounts (`ST/node/src/chain_spec/localnet.rs:78-126,152-157`):** Alice, Bob, Charlie: 1,000,000 TAO each (1e15 rao); Dave, Eve, Ferdie, One, Two, Three: 2,000 TAO each; EVM dev accounts Alith (`5Fghzk1AJt88PeFEzuRfXzbPchiBbsVGTTXcdx599VdZzkTA`) and Baltathar (`5GeqNhKWj1KG78VHzbmo3ZjZgUTrCuWeamdgiA114XHGdaEr`): 2,000 TAO; `BT_DEFAULT_TOKEN_WALLET` env adds/overrides one address to 1,000,000 TAO (`:128-139`). Sudo = Alice; `evmChainId` 42; ss58 prefix 42, token `TAO`/9 decimals (`:31-35`). Alice's seed is published at `local-development.mdx:102-103`. A fresh localnet has subnet 0 and subnet 1 pre-created (`local-development.mdx:48`).

**Fast runtime semantics:** `prod_or_fast!` picks the second value under `fast-runtime` — 250 ms blocks (`ST/common/src/lib.rs:183`), epoch 10 s (`runtime/src/lib.rs:391`), tempo 10 (`:904`), childkey-take rate limit 5 (`:907`), coldkey-swap delays 50/10 blocks (`:973-974`), up to 32 epochs per block (`:987`). Docs: "elapses 48× faster than mainnet wall-clock time" (`local-development.mdx:154-156`). Use `False` (12 s) for timing-sensitive tests.

**Starting it (verified patterns):**

```bash
# upstream docs (local-development.mdx:13-15)
docker run --rm --name local_chain -p 9944:9944 -p 9945:9945 ghcr.io/raofoundation/subtensor-localnet:devnet
# Rust e2e harness (e2e_support/mod.rs:85-99): ephemeral host ports, then `docker port <name> 9944/tcp`
docker run --rm -d --name bittensor-rust-e2e-$$ -p 127.0.0.1::9944 -p 127.0.0.1::9945 ghcr.io/raofoundation/subtensor-localnet:v477
```

With `testcontainers` 0.28.0 (crates.io, 2026-08-06): `GenericImage::new("ghcr.io/raofoundation/subtensor-localnet", "v477").with_exposed_port(9944.tcp()).with_wait_for(WaitFor::message_on_stderr("Imported #1"))` then `ImageExt::with_cmd(["True"])`; docs.rs confirms `GenericImage::new/with_exposed_port/with_wait_for/with_entrypoint` and `ImageExt::with_cmd/with_env_var/with_mapped_port` (fetched docs.rs page); the exact `WaitFor` constructor set and `get_host_port_ipv4` were not confirmed from docs.rs in this session — verify at implementation time. Substrate logs go to stderr; the upstream harness greps both streams (`e2e_support/mod.rs:103-109`). A plain `docker` CLI harness (what upstream does) is ~80 lines and has zero extra dependencies; testcontainers buys Ryuk-based cleanup and parallel-safe port mapping. Either is fine; see §7.

### 1.4 Chainspecs and snapshot.json

- `ST/chainspecs/`: `raw_spec_finney.json` (12.1 MB), `raw_spec_testfinney.json` (0.58 MB), `raw_spec_devnet.json` (8.7 MB) and the three `plain_spec_*.json`. All six are "genesis anchors": the genesis wasm is not reproducible across build architectures, so they are never regenerated from scratch (`ST/docs/internals/scripts.mdx:31-51`; `update-chainspec.yml` splices new content around the stored genesis). `raw_spec_finney.json` is what `clone-mainnet.sh` warp-syncs from (`ST/clones/scripts/clone-mainnet.sh:30-36`, bootnode `/dns/bootnode.finney.chain.opentensor.ai/tcp/30333/ws/p2p/12D3KooWRwbMb85RWnT8DSXSYMWQtuDwh4LJzndoRrTDotTR5gDC`).
- `ST/snapshot.json` (1.47 MB, keys `balances`, `stakes`): genesis state for the embedded finney/testfinney specs only.
- Relevance to rtao: (a) the raw specs give rtao an offline source of each network's genesis hash and properties for unit tests; (b) `node-subtensor build-patched-spec` (`ST/node/src/cli.rs:92-136`) can clone **live mainnet state** into a local chain with Alice as sudo/authority (`ST/docs/internals/mainnet-clone.mdx:17-32`) — the gold standard for "does rtao's metagraph decoding work on real state"; sync takes up to ~35 min per attempt (`clone-mainnet.sh:12-16`) and needs the node binary from the production image. Upstream caches a snapshot artifact in CI (`runtime-checks.yml:670-734`); rtao could do the same weekly rather than per-run.

### 1.5 Golden fixtures and vectors (paths, formats, what each pins)

**`ST/sdk/python/tests/fixtures/golden.json`** (1,508,346 bytes; recorded by `ST/sdk/python/scripts/record_golden.py` against a localnet; recorded at `spec_version` **461**, `transaction_version` 1, ss58 42, genesis `0x1dc00e76…ce452`). Top-level keys and what each pins (`record_golden.py` line refs):

| Key | Contents | Pins |
|---|---|---|
| `network` | genesis_hash, spec_version, transaction_version, ss58_format (`:153-160`) | chain identity used for signing |
| `metadata` | `block_hash`, `v15_hex` (raw `Metadata_metadata_at_version(15)` response, i.e. SCALE `Option<OpaqueMetadata>` — 773,478 hex chars), `v14_hex` (`state_getMetadata`) (`:162-167`) | lets every other vector be replayed offline with the exact registry |
| `storage_keys[10]` | pallet, storage_function, params, `key_hex`, `value_scale_type` (`:77-88,169-183`) — covers plain, map, double map, triple map (`Alpha`), `Multisig.Multisigs` with 32-byte key | twox128/blake2_128concat/identity hashing + key encoding |
| `calls[10]` | module, function, params, `data_hex` incl. nested `Sudo.sudo(System.remark)`, `Utility.batch([2 transfers])`, `Proxy.proxy(real, Transfer, call)` (`:90-110,185-237`) | call index + argument SCALE encoding, nested `RuntimeCall` |
| `signature_payloads[2]` | immortal (nonce 7) and mortal (period 64) payloads for `Balances.transfer_keep_alive(Bob, 12345)` (`:239-284`) | extension layout: `call ‖ era ‖ nonce ‖ tip ‖ mode(0x00) ‖ spec_version ‖ tx_version ‖ genesis ‖ era_hash ‖ metadata_hash(None=0x00)` — decoded from the vector hex; custom extensions contribute zero bytes |
| `extrinsics[2]` | fake fixed signature `bytes(1..=64)` with `signature_version` 1 (sr25519), immortal nonce 0 and mortal nonce 11; `extrinsic_hex`, `extrinsic_hash` (`:286-311`) | v4 signed extrinsic framing (`0x84`, `MultiAddress::Id`, sig type byte, extras, call) + blake2 hash |
| `multisig[3]` | signatories, threshold, derived ss58 (`:313-328`) | multisig account derivation incl. sorting |
| `ss58[4]` | pubkey → address for Alice/Bob/Charlie/Alice//hot (`:330-334`) | ss58 encode, `//hot` derivation |
| `constants[3]` | ExistentialDeposit=500, Aura.SlotDuration, SS58Prefix decoded (`:336-340`) | constant decoding |
| `receipt` | a real `transfer_keep_alive(Bob, 1 TAO)` submission: block_hash, extrinsic_idx, total_fee_amount, triggered_events (`:342-358`) | event→extrinsic attribution, fee from events |
| `storage_values[8]` | raw + decoded at pinned block, incl. a `None` map miss (`:118-127,360-378`) | storage value decoding and Option semantics |
| `events` | raw `System.Events` bytes + decoded list at the pinned block (`:380-387`) | `Vec<EventRecord>` decoding shape |
| `query_maps[4]` | prefix, `state_getKeysPaged` keys, `state_queryStorageAt` changes, decoded pairs (`:389-417`) | map iteration + key decoding (incl. double-map partial key) |
| `runtime_calls[4]` | `NeuronInfoRuntimeApi.get_neurons_lite(1)`, `SubnetInfoRuntimeApi.get_subnet_hyperparams(1)`, `StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid`, `AccountNonceApi.account_nonce`: `encoded_params_hex`, `raw_hex`, decoded (`:136-145,419-436`) | runtime-API param encoding and output decoding |
| `block` | `chain_getBlock` raw + decoded header/extrinsics (`:438-446`) | block/extrinsic decoding |
| `payment_info` | keys `class, partial_fee, weight`; weight `{ref_time, proof_size}` (`:448-450`) | `TransactionPaymentApi` shape |

Rust-side consumers prove these are language-neutral: `ST/sdk/bittensor-core/src/codec/mod.rs:80-99` loads `golden.json`, strips the `Option` wrapper, and builds the `Runtime` from `network.{spec_version,transaction_version,ss58_format}`; `digest/mod.rs:162-181` does the same for the RFC-0078 digest. I replayed this myself: `RT/golden_461.scale` (386,733 bytes, magic `meta`, version byte 15) extracted from `metadata.v15_hex` and consumed by `subxt diff`/`subxt metadata` (§2.3).

**`ST/sdk/python/tests/fixtures/shape_corpus/corpus.json`** (1,076,611 bytes; `scripts/record_shape_corpus.py`). Schema: `{spec_version: 461, codec: "bittensor-core", types_total: 802, types_covered: 797, skipped: [{id, name, reason}], types: [{id, name, samples: [{scale_hex, decoded}]}]}` (`record_shape_corpus.py:186-195`). Bytes are *generated* from the portable registry with a per-type seeded RNG (`random.Random(type_id*1000+i)`), ≥3 samples per type, every enum variant visited (`:38-40,151-184`), then decoded once with the production codec; `decoded` uses upstream's JSON rendering conventions (bytes as `0x…`, AccountId32 as ss58, dict keys stringified; `:133-143`, and `ST/sdk/bittensor-core/src/codec/value.rs:58-90 to_corpus_json`). Tests: Python requires >95 % coverage and exact equality (`tests/unit/test_shape_corpus.py:39-74`); Rust asserts corpus spec == golden spec and replays every sample (`codec/mod.rs:115-160`). The spec calls this "the codec's definition of done" (`ST/sdk/bittensor-core-spec.md:105-127,423-436`).

**btauth/1 vectors** (normative: `ST/docs/guides/signed-requests.mdx:35-101,252-289`; tests: `ST/sdk/python/tests/unit/test_http_auth.py:1-7,20-36,81-104`): sender `//Alice`, receiver `//Bob` (`5FHneW46…694ty`), nonce `1752076800000000000`, body `{"prompt": "hello"}` (19 bytes), `POST /generate?stream=false`, `sha256(body) = 341c5744…c3cc`; exact 8-line payload; ed25519 `//Alice` = `5FA9nQDVg267DEd8m1ZypXLBnvN7SFxYwV7ndqSYGiN9TTpu` with pinned signature `0x1a6b6319…f704`; sr25519 is randomized so only sign→verify is pinned (`signed-requests.mdx:276-279`). Header set: `X-Bittensor-Version: 1`, `-Hotkey`, `-Receiver` (optional but required by default on servers), `-Nonce` (ns, decimal), `-Crypto: ed25519` (omitted for sr25519), `-Signature: 0x…` (`:44-50`). Verification order (shape → receiver → freshness 10 s past / 2 s skew → signature under declared scheme only → replay store) at `:76-101`. The test docstring states the policy: a change that breaks these vectors "needs a protocol version bump, not a fixture update" (`test_http_auth.py:3-6`).

**Keyfile goldens** (`ST/sdk/python/tests/unit/test_keyfiles_golden.py`): `//Alice` = `5GrwvaEF…utQY` (`:25,32-34`); test mnemonic `abandon…about` (`:26-28`); NaCl secretbox keyfile format detection/roundtrip (`:54-63`); **legacy Fernet** format with pbkdf2-hmac-sha256, salt `b"Iguesscyborgslikemyselfhaveatendencytobeparanoidaboutourorigins"`, 10,000,000 iterations (`:66-84`); keyfile JSON field `ss58Address` (`:108-109`); permissions 0600 file / 0700 dir (`:121-140`). The Rust implementation of these formats lives in `ST/sdk/bittensor-core/src/keyfiles/mod.rs` (578 lines) and `keys/encrypted_json.rs`.

**RFC-0078 digest vector**: for the golden V15 metadata with `ChainInfo{spec_version: 461, spec_name: "node-subtensor", base58_prefix: 42, decimals: 9, token_symbol: "TAO"}` the digest must be `affbcc25b1cb6847bc94dcf3a7f35fb8d6a5c59d9d47da92063a9399a5fc11c0`, cross-validated against polkadot-api's `merkleizeMetadata` (`ST/sdk/bittensor-core/src/digest/mod.rs:150-201`).

**Ledger proof vector**: `ST/sdk/bittensor-core/fixtures/ledger_proof_vector.json` (12,767 bytes), byte-exact against `generate_proof_for_extrinsic_parts` and envelope-compatible with polkadot-api (`digest/mod.rs:216-230`). Relevant only if rtao supports Ledger.

**Timelock vectors**: `ST/sdk/bittensor-core/src/timelock/epoch_schedule_vectors.rs` (86 lines) — drand round/epoch schedule pins for commit-reveal (not inspected in depth; listed for completeness).

**Caveat on freshness:** both golden.json and corpus.json are pinned at spec **461** while mainnet runs 477 and main is 478; upstream's Rust tests only assert the two fixtures agree with each other (`codec/mod.rs:124-128`). Upstream regenerates `_generated/` per runtime via the drift gate but re-records goldens "deliberately" (`ST/docs/internals/sdk-tests.mdx:104-109`). rtao should import the 461 vectors as *frozen* conformance tests (they remain valid for decoding 461-era bytes forever, because the metadata is embedded) and additionally record its own per-spec vectors (§5).

### 1.6 The bittensor.com JSON catalogs

Generated by `ST/website/apps/bittensor-website/scripts/generate.py:1227-1283 write_catalogs()` from the Python SDK's intent/read/error registries (not from metadata directly), committed under `website/apps/bittensor-website/public/catalog/` and served at `/catalog/{intents,reads,errors}.json`; the chain-source index is `/code/index.json`.

- **`intents.json`** — array of 83 objects (`DOCS/intents.json`): `name`, `summary`, `description`, `signer` ∈ {coldkey, hotkey}, `origin` ∈ {signed, root, subnet_owner}, `verify` (name of a read used to confirm root-origin writes, or null), `wraps: [[pallet, call]…]` (139 wrapped calls total), `input_schema` (JSON Schema for the intent's parameters, including decimal-string amount forms), `cli` (btcli invocation), `python_class`, `docs_url`, `markdown_url`, `sources: [{pallet, call, path, line, end_line, url, raw_url}]` (resolved dispatchable locations in the monorepo). `generate.py:1183-1204` builds `sources`.
- **`reads.json`** — array of 102 objects: `name`, `summary`, `description`, `params: {name: type}`, `param_docs`, `category` (14 categories: Accounts & keys, Chain, Delegation, Epochs & timing, Hyperparameters, Identity & commitments, Leases & crowdloans, Locks & conviction, Mining & collateral, Neurons & registration, Prices & swaps, Staking, Subnets, Weights & bonds), `python` (namespace call), `docs_url`, `markdown_url`, `hits: [{kind: storage|runtime_api|…, container, name, path, line, url, raw_url}]` — i.e. which storage items / runtime APIs each read touches.
- **`errors.json`** — `{note, codes: {snake_case → {remediation, docs_url, markdown_url}} (18 semantic codes: already_exists, disabled, expired, insufficient_balance, insufficient_liquidity, internal, invalid_argument, limit_exceeded, not_authorized, not_found, not_registered, policy_violation, rate_limited, subnet_not_exists, subtoken_disabled, too_early, unit_mismatch, unknown), chain_errors: {CamelCase → {code, description, docs_url, markdown_url, pallets[], sources[]}}}` with 390 chain error names.
- **`index.json`** — `{commit: "850f2755…", note, search: "/code/search.json?q=<literal>", files: [{path, lines, bytes}]}` — the exact monorepo commit the site was generated from (an upstream "docs version" signal).

**How rtao can diff them release-to-release:** store the previous snapshot; diff keyed by `name`. Semantic signals worth alerting on: (a) new/removed intent names → new SDK behaviour rtao may want to mirror; (b) changes in `wraps` (which pallet/call an intent targets) → the Python SDK re-pointed a behaviour (e.g. deprecations like `set_weights`→mechanism variants); (c) changes in `input_schema` → parameter semantics changed; (d) `errors.json.chain_errors[*].code` reclassification → upstream's error taxonomy moved; (e) new chain error names → new runtime errors to classify; (f) `reads.json[*].hits` changes → storage items a read uses were renamed/added; (g) `index.json.commit` → which monorepo commit to diff. These catalogs are produced only when the production site deploys (after a mainnet release, `watch-mainnet-release.yml:482-525`), so they lag testnet; use them as a *semantic* feed, and metadata as the *structural* feed.

### 1.7 Python codegen descriptors as a cross-check oracle

`ST/sdk/python/bittensor/_generated/` is regenerated from a node's metadata and committed (`ST/docs/internals/sdk-tests.mdx:85-109`). Files and content (`ST/sdk/python/codegen/emit_python.py`):

- `errors.py` (420 lines): `ERRORS: dict[(pallet_index, error_index)] → ErrorInfo(pallet, name, docs)` (`:97-113`) — gives rtao a ready-made expected table of **error indices**.
- `calls.py` (1,704 lines): one `@staticmethod` per extrinsic with parameter names and type-identity annotations (e.g. `amount_staked: 'TaoBalance'`), returning `Call(module, function, params)` (`:116-169`) — parameter names/order and the runtime's own newtype names.
- `storage.py` (477 lines): `Item(container, name, value_type_ident)` per storage entry (`:227-240`); `constants.py` (187); `runtime_apis.py` (189): `Method(api, method)`.
- Headers deliberately omit `spec_version` so a spec bump with no surface change does not create drift (`emit_python.py:22-25`) — rtao should adopt the same rule for its generated code (hash the metadata *content*, exclude `System.Version`).

Gates rtao can mirror (`ST/sdk/python/codegen/check.py`): `--drift <endpoint>` regenerates from a live node and byte-compares, failing on orphans too (`:32-53`); `--coverage` requires every call in 13 covered pallets to be either wrapped by an intent or listed in `RAW_ONLY` (`:58-72,74-300,303-346`) — a *forcing function* for new calls; `--names` requires every chain error name to classify to a semantic code, with descriptions, and keeps `CUSTOM_TRANSACTION_ERRORS` in lockstep with `common/src/transaction_error.rs` (28 codes + 255; `:349-351,354-455`); `--units` checks hyperparameter unit kinds against storage value-type identities (`:472-507`). The local gate boots `node-subtensor --chain local --tmp --alice --validator --rpc-port 9977` and runs `--drift` (`ST/scripts/preflight.sh:308-340`). **Oracle use:** parse `errors.py`/`calls.py`/`storage.py` from the `mainnet`/`testnet`/`devnet` branches with a tiny regex (the formats are regular and sorted) and assert rtao's generated tables agree on (pallet index, call/error names, parameter names, storage names, type idents). This catches rtao codegen regressions independently of subxt.

### 1.8 eco-tests (indexer contract) — what it tells rtao

`ST/eco-tests/` pins storage shapes and runtime-API signatures the TAO.com and Mentat indexers read (`ST/docs/internals/eco-tests.mdx:6-18`); it is compile-time Rust against the pallets as path dependencies (`ST/eco-tests/Cargo.toml:24-50`), so rtao cannot run it, but it is a curated list of "items third parties depend on" — e.g. `Owner`, `StakingHotkeys`, `TotalHotkeyAlpha`, `TotalNetworks`, `Active/Consensus/Dividends/Incentive/LastUpdate/ValidatorPermit/ValidatorTrust/Emission`, `Uids/Keys/Weights/Bonds/Axons` (`ST/eco-tests/src/tests_mentat_indexer.rs:27-60`, `tests_taocom_indexer.rs:23-52`). Any change there triggers `eco-tests-indexer-notify.yml` (`eco-tests.mdx:38-42`). rtao should subscribe to PRs touching `eco-tests/**` as an early warning for metagraph-shape changes, and mirror the same list in its own typed-reads tests.

### 1.9 Smoke suites and upstream's own live monitoring

`scheduled-smoke-tests.yml` runs read-only Moonwall smoke suites every 6 h against devnet/testnet/mainnet (`ST/.github/workflows/scheduled-smoke-tests.yml:7-9,26`); the single test asserts finalization within 10 min and `specVersion > 0` (`ST/ts-tests/suites/smoke/test-block-finalization.ts:16-29`). The same suites gate promotion (`release-train.yml:194-212,282-299`). rtao can run an equivalent read-only "liveness + spec + metadata hash" probe on the same cadence (§3.6).

---

## 2. The upstream release process and what changes when

### 2.1 Mechanics (all cited)

- Trigger: push to `main` touching `runtime/**, pallets/**, node/**, primitives/**, precompiles/**, common/**, Cargo.lock` (`release-train.yml:37-47`); trains queue (`:52-54`).
- Build once with srtool (`--features=metadata-hash`, `PROFILE=production`) → `runtime-<spec>` artifact (`subtensor.wasm` + digest), 90-day retention (`:84-111`).
- `spec_version` is read by grep from `runtime/src/lib.rs` (`:74-79`); each deploy job no-ops unless `build.spec > chain.spec` via `state_getRuntimeVersion` (`:129-143,230-242,426-438`).
- devnet: automatic `setCode` by CI key; verify after 30 s; force-push `devnet` mirror (`:116-192`). testnet: environment reviewers, same flow (`:216-280`). Both mirror pushes trigger `docker.yml` and `docker-localnet.yml` (`docker-localnet.yml:13-16`).
- SDK rc: after the testnet smoke, versions `base rc${GITHUB_RUN_NUMBER}` for both `bittensor` and `bittensor-core`, rc SDK pins core `==` (`:301-349`), wheels via `build-core-wheels.yml` (manylinux x86_64/aarch64 + macOS arm64/x86_64 + sdist, abi3-py310; `ST/.github/workflows/build-core-wheels.yml:3-16`), PEP 740 attestations, `uv publish` (`:358-403`).
- mainnet: `mainnet` environment approval → reserve immutable `v<spec>` tag at the commit → submit CI half of a 2-of-2 multisig (`SudoUncheckedSetCode` proxy on sudo) → publish pre-release with assets and signer instructions (`:412-698`). Triumvirate signs 2-of-3 out-of-band (`release-process.mdx:64-72`).
- Watcher: every 10 min compares finalized `spec_version`/`:code` hash with main; verifies the train artifact's provenance (`ST/.github/scripts/resolve-release-artifact.sh:17-23`) and bytes (`verify-release-artifact.py`); force-moves `mainnet` mirror; promotes the pre-release to final + `latest` (`watch-mainnet-release.yml:25-28,204-341`); then, with `mainnet` approval, publishes stable PyPI packages (stamping `.dev0`→stable, requiring the full wheel matrix with attestations; `:343-480`) and deploys the docs site (`:482-525`).
- Hotfixes follow the identical path (`release-process.mdx:140-144`).

### 2.2 What changes at a spec bump, and what rtao must re-check

| Thing | Behaviour | Evidence | rtao action |
|---|---|---|---|
| `spec_version` | monotonic, bumped per release; `CheckSpecVersion` makes old signed payloads invalid | `runtime/src/lib.rs:241`; `SystemTxExtension` includes `CheckSpecVersion` (`:1644-1651`); Python refuses to attach a signature prepared under an older spec (`_transport/interface.py:617-623`) | re-fetch metadata on runtime-version change; invalidate prepared-but-unsigned payloads |
| `transaction_version` | 1 at 461 and 477; unchanged | golden `network.transaction_version: 1`; `runtime/src/lib.rs:244` | include in payload; alert if it ever changes (would mean call indices re-numbered) |
| Transaction extensions | 13 identifiers, identical at 461 and 477: CheckNonZeroSender, CheckSpecVersion, CheckTxVersion, CheckGenesis, CheckMortality, CheckNonce, CheckWeight, ChargeTransactionPayment, SudoTransactionExtension, CheckShieldedTxValidity, SubtensorTransactionExtension, DrandPriority, CheckMetadataHash | `jq` on `RT/golden_461.json` and `META/finney.json` (§1.1); runtime tuple `runtime/src/lib.rs:1644-1664` | drift job must diff identifiers *and* their extra/implicit type ids; a new non-unit extension breaks signing (subxt 0.51's `custom_extension(name, value)` exists for exactly this case) |
| Extrinsic versions | V16 declares `[4, 5]`; golden extrinsics are v4 (`0x84`) | `jq '.[1].V16.extrinsic.versions' META/finney.json`; golden `extrinsic_hex` prefix | keep signing v4 (what Python/core do; `codec/extrinsic.rs:320`); add v5 only deliberately |
| Metadata hash (`CheckMetadataHash`) | mode byte in extras, `Option<[u8;32]>` in implicit; runtime built with `metadata-hash` on all networks and localnet | `runtime/build.rs:10-16`; `localnet.sh:40,47`; `codec/extrinsic.rs:28-29,44,54,181-229` | recompute digest per spec from V15; conformance vector in §1.5 |
| Pallet indices | unchanged 461→477 | `RT/call_indices.py` output: `pallet index changes: {}` | assert in drift job; a change is a hard break |
| Call indices | append-only observed: SubtensorModule +151 `swap_basket_many`, +152 `pow_register`; AdminUtils +110..113; deprecated stubs kept at 139/140 | `RT/call_indices.py`; `check.py:117-120` ("Deprecated no-op compatibility stubs (call indices 139/140)") | alert on *moved* indices as breaking; appended calls are additive; localnet additionally has `faucet` at 60 |
| Error indices | no moves/adds/removes 461→477 | `RT/err_ev_indices.py` printed nothing under `--- errors ---` | alert on moves; additions are additive |
| Event indices | **not stable**: SubtensorModule 149 `BasketSwapped`→`SubnetLeaseDividendSkipped`; `BasketSwapped` now 151; +150,152-155; AdminUtils +13,14 | `RT/err_ev_indices.py` | never match events by index across specs; match by `(pallet name, event name)` as upstream does (`ST/sdk/python/bittensor/executor.py:265-270`); generated event enums must be per-spec |
| Storage | items removed (`Alpha`, `AlphaMapLastKey`, `TotalHotkeyShares`), added (many), key type changed (`LastRateLimitedBlock`), value type changed (`System.Events`), defaults changed (`Tempo`, `MaxEpochsPerBlock`, …) | `RT/diff_461_477.txt` | typed reads need per-spec tables; metagraph fast-path reads must be re-validated |
| Runtime APIs | versions in metadata V16 (`SubnetInfoRuntimeApi` v2, `BetaBasketRuntimeApi` v5, `StakeInfoRuntimeApi` v1, …); `subxt diff` reports changed `BetaBasketRuntimeApi`, `ContractsApi`, `TransactionPaymentCallApi` 461→477 | `jq '[.[1].V16.apis[]|{name,version}]' META/finney.json`; `RT/diff_461_477.txt` tail; digest `apis` table | diff `(name, version, method signatures)`; upstream's own node-reuse check does exactly this (`verify-base-node-runtime.py:10-15,234-244`) |
| Constants | values change under `prod_or_fast!` and per release | §1.3 | never hard-code; read from metadata; tests must not assume mainnet values on localnet |
| Deprecations | V16 `deprecation_info` on calls | §1.1 | surface in drift report; mark generated wrappers `#[deprecated]` |

### 2.3 Measured 461→477 diff (how I produced it; reusable as the drift-job prototype)

1. Extract V15 metadata from the golden fixture: strip the SCALE `Option<Vec<u8>>` wrapper of `metadata.v15_hex` → `RT/golden_461.scale` (386,733 bytes, `meta` magic, version 15).
2. `subxt diff RT/golden_461.scale META/finney.scale` → `RT/diff_461_477.txt` (82 lines; subxt-cli 0.51.1): reports per-pallet Calls/Storage Entries changes with reasons (`Changed: default value|key type|value type`) and changed Runtime APIs — but **no events, no errors, no constants values, no indices**.
3. `subxt metadata --file RT/golden_461.scale --format json` → `RT/golden_461.json` (`[magic, {"V15": …}]`); `META/finney.json` is `[magic, {"V16": …}]`; pallet `calls`/`errors`/`event` fields are `{ty: <type id>, …}`; call variants come from the registry type → `RT/call_indices.py` and `RT/err_ev_indices.py` compare `(index → name)` maps. Results are in §2.2.

This 3-step pipeline is ~150 lines of Rust with `frame-metadata` + `scale-info` (or `subxt-metadata`) and is what the nightly job should run.

### 2.4 Storage migrations

Migrations are a tuple in `runtime/src/lib.rs:1666+` (`Migrations = (migrate_alpha_v2, migrate_restore_staking_hotkeys, …)`), some multi-block (`runtime-checks.yml:195` detects MBM configuration). Upstream replays them with `try-runtime on-runtime-upgrade --checks=all` against devnet/testnet/mainnet state (snapshot refreshed nightly by `refresh-mainnet-snapshot.yml`, live fallback; `runtime-checks.yml:312-470`) and sudo-upgrades a live mainnet clone (`mainnet-clone.mdx:17-32`). For rtao the only observable effects are storage-shape changes (caught by the metadata diff) and *transitional dual-format reads* ("transitional staking getters read both formats", `runtime/src/lib.rs:1667-1668`), which only a behavioural test on real state catches → the optional mainnet-clone job (§3.5).

### 2.5 Python SDK/CLI versioning and how compatibility is (not) signalled

- Source of truth: `sdk/python/pyproject.toml` `version = "11.3.1.dev0"` (`:7`), dependency `bittensor-core>=0.1.6,<0.2.0` (`:21`), in-repo path source (`:57-58`); `sdk/bittensor-core-py/pyproject.toml` `version = "0.1.6"` (`:9`), `abi3-py310` (`Cargo.toml:17`).
- Channels: `.devN` → TestPyPI on every SDK-affecting push to main (`publish-sdk-dev.yml:3-11,43-50`); `rcN` → PyPI after testnet smoke (N = train run number; `release-train.yml:348-349`), rc SDK pins core `==rcN`; stable → PyPI after mainnet upgrade + approval (`watch-mainnet-release.yml:343-480`); a stable base version that already exists on PyPI fails the train (`release-train.yml:325-346`).
- Observed PyPI timeline: 11.0.0 (2026-07-17, with runtime 432) … 11.1.0 (2026-08-14), 11.3.0 (2026-10-07, with v475), 11.3.1rc68 (2026-10-08, train that produced v477) — so an `rcN` → `v<spec>` mapping is recoverable from the train run number, and the SDK version is bumped by hand when the maintainers decide (v477 notes: "Bump runtime spec to 477 and Python release versions").
- **No minimum/maximum spec is declared anywhere** in the SDK (`grep` for `MIN_SPEC|SUPPORTED_SPEC` in `sdk/python/bittensor` finds nothing; only runtime-reported `spec_version` accessors). Compatibility is enforced by (a) metadata-driven codec per spec at runtime (`runtime.py:16-66`), (b) committed descriptors kept equal to the chain by the drift gate (`check.py:32-53`), (c) the Clone Upgrade Check which fails a runtime PR that breaks the SDK (`contributing.mdx:16-20`). Practical consequence for rtao: there is no upstream "SDK X supports spec Y..Z" table to copy; rtao must *publish its own* compat matrix (§3.9) derived from its drift/e2e runs.

---

## 3. Recommended rtao CI/CD design

Principles: (1) **pin, don't fetch at build time** (crabtensor and rusttensor fetch finney metadata inside `build.rs` — `ECO/timehash13_subtensor-api-rs/build.rs:15-27`, `ECO/womboai_rusttensor/build.rs:15-27` — so their builds depend on a live RPC and silently change under them; CortexLM and one-covenant commit `metadata/finney.scale` but do not record which spec it is). (2) **Separate structural drift (metadata) from behavioural drift (localnet e2e) from semantic drift (catalog JSON).** (3) **Every fixture names the `(genesis, spec_version, block_hash)` it was recorded at** and embeds or references the metadata by hash. (4) **Name-based matching for events/errors; index-based only within one pinned spec.**

### 3.1 Repository layout for pins and fixtures

```
rtao/
  metadata/
    index.toml                     # one row per (network, spec): block_hash, code_hash, sha256(v15), sha256(v16), recorded_at, source
    finney/477.v15.scale  finney/477.v16.scale
    testnet/477.v15.scale …
    devnet/478.v15.scale …
    localnet/477.v15.scale         # from ghcr :v477 (has faucet@60, fast constants) — behavioural tests only
  fixtures/
    upstream/                      # vendored verbatim, with UPSTREAM_COMMIT and sha256 recorded
      golden-461.json  corpus-461.json  ledger_proof_vector.json  btauth-vectors.json (transcribed from docs/guides/signed-requests.mdx)
    rtao/
      golden-<spec>.json  corpus-<spec>.json  # recorded by `cargo xtask record` (§5)
  crates/rtao-xtask/               # record, drift, compat-matrix, import-upstream subcommands
```

### 3.2 PR gates (fast, offline)

- `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -D warnings`, `cargo nextest run --workspace --all-features`, `cargo test --doc` (mirrors `ST/.github/workflows/check-rust.yml:117-264`).
- `cargo deny check` / `cargo audit` (upstream pins cargo-audit 0.22.2, `cargo-audit.yml:44-50`).
- Portability seam: `cargo check -p rtao-core --no-default-features --target wasm32-unknown-unknown` if rtao keeps a no-tokio core (upstream holds this invariant, `check-rust.yml:269-302`); otherwise skip.
- **cargo-semver-checks** (0.51.0, 2026-10-03) on every PR against the last published version of each public crate; fail on undeclared major breaks.
- Generated-code freshness: `cargo xtask codegen --check` regenerates from the pinned `metadata/<net>/<spec>.v16.scale` and diffs — the local analogue of `codegen.check --drift` but offline (no node needed because metadata is pinned).
- Golden conformance tests (§3.4) run here because they are pure.

### 3.3 Nightly "metadata drift" job (the heart of upstream tracking)

Schedule: hourly-to-nightly (`cron: "17 */6 * * *"` matches upstream's smoke cadence; the watcher itself is every 10 min). Steps:

1. For each network in {finney, testnet, devnet}: JSON-RPC `chain_getFinalizedHead` → `state_getRuntimeVersion(at)` → `state_getStorageHash(":code" = 0x3a636f6465, at)` (exactly `mainnet-finalized-runtime.sh:19-26`), compare `(spec_version, code_hash)` with `metadata/index.toml`. If unchanged → done for that network.
2. On change: `state_call Metadata_metadata_at_version 0x0f000000` and `0x10000000` at that block; decode `RuntimeMetadataPrefixed`; store under `metadata/<net>/<spec>.v{15,16}.scale`.
3. Pre-check with the release digest when the network is finney: download `subtensor-digest.json` for `v<spec>`; assert `blake2_256`(wasm) matches on-chain `:code` hash and read `apis`/`transactionVersion` (free sanity check that the release and the chain agree; also tells you the tag exists → localnet `:v<spec>` image should appear within ~1–2 h, see `docker-localnet.yml` build matrix at `:120-136`).
4. Semantic diff old→new (rtao's own tool, since `subxt diff` lacks events/errors/indices/constants): pallets (index, name), calls (index, name, args, type idents, deprecation), events (index, name, fields), errors (index, name), storage (name, hashers, key/value type, default — flag default-only changes as "info"), constants (name, type, value), runtime APIs (name, version, method signatures via type shapes as `verify-base-node-runtime.py:192-244` does), tx extensions (identifier, extra type, implicit type), extrinsic versions, `System.Version`. Classify: **breaking** (moved pallet/call/error index, removed call/storage used by rtao, changed call args, tx-extension change, transaction_version change), **behavioural** (event index move, storage type change, runtime-API version bump, default change), **additive** (new call/event/error/storage/API).
5. Also run `subxt diff <old> <new>` and attach its output verbatim (readable, and a second opinion).
6. Regenerate rtao's generated layer (`cargo xtask codegen`) from the new V16; run offline tests; run `cargo semver-checks` on the generated crate to label the PR major/minor.
7. Open/refresh a PR `drift: <network> <old>→<new>` containing: metadata files, index row, regenerated code, the classified diff as the PR body, the GitHub release notes (`gh api releases/tags/v<spec>` body) when finney, and the catalog diff (§3.7) when available. Label `runtime-breaking`/`runtime-additive`. Use `peter-evans/create-pull-request` or `gh pr create` from a bot token (**inferred** tooling choice).
8. Gate: the PR's e2e matrix (§3.5) must pass against `:v<spec>` (finney) or `:testnet`/`:devnet` before merge.

Devnet/testnet runs give rtao a lead of hours-to-days before mainnet moves (release-process.mdx:14-20): drift PRs from devnet are "upcoming", from finney are "now". Keep one open PR per network, force-updated.

Extra guards: alert (don't PR) if `Metadata_metadata_versions` no longer lists 15 (digest computation would break); alert if `System.Version.spec_name != "node-subtensor"`.

### 3.4 Golden-vector conformance tests imported from upstream

Vendor and replay (all pure; list = what to import):

1. `golden-461.json` → `storage_keys` (key hashing), `calls` (call encoding incl. nested Sudo/Utility/Proxy), `signature_payloads` (extension layout with `mode`/`metadata_hash`), `extrinsics` (v4 framing with fake signature → byte-exact `extrinsic_hex` and `extrinsic_hash`), `multisig` (derivation), `ss58`, `constants`, `storage_values` (raw→decoded incl. None), `events` (raw→decoded), `query_maps` (key decoding), `runtime_calls` (param encoding + output decoding), `block` (header/extrinsic decoding), `payment_info` shape. All decode against the embedded `metadata.v15_hex` — the same loader pattern as `ST/sdk/bittensor-core/src/codec/mod.rs:80-109`.
2. `corpus-461.json` → every `(type id, scale_hex, decoded)` sample must decode shape-equal under rtao's dynamic decoder **if** rtao adopts upstream's JSON rendering (ss58 for AccountId32, `0x` bytes, unwrapped compacts, enum rendering; `codec/value.rs:1-90`). If rtao chooses a different `Value` model, keep the corpus test but compare through a one-way normaliser to upstream's shape — the point is detecting *decoding* regressions, not dictating rtao's in-memory type.
3. btauth/1 vectors (payload bytes, ed25519 signature, addresses; `signed-requests.mdx:252-289`) + negative cases from `test_http_auth.py` (non-canonical scheme spellings `:140`, receiver mismatch, stale nonce, replay).
4. RFC-0078 digest `affbcc25…11c0` over the 461 V15 metadata with chain info `("node-subtensor", 42, 9, "TAO", 461)`.
5. Keyfile formats: NaCl secretbox and legacy Fernet (pbkdf2-sha256, salt and 10,000,000 iterations from `test_keyfiles_golden.py:75-76`) decrypt of a known ciphertext, `ss58Address`/`publicKey`/`secretPhrase` JSON fields, 0600/0700 permission policy.
6. `ledger_proof_vector.json` (only if Ledger support is in scope).
7. Timelock epoch schedule vectors (`timelock/epoch_schedule_vectors.rs`) if rtao implements commit-reveal v3.

Mark each imported file with `UPSTREAM_COMMIT = 850f2755…` and its sha256 in `fixtures/upstream/MANIFEST.toml`; add `cargo xtask import-upstream --rev <sha>` that re-vendors and reports diffs, and have the nightly job run it against `main`, `testnet`, `mainnet` branches so a changed upstream vector opens a PR with the delta (a golden change upstream is itself a signal: `sdk-tests.mdx:104-109`).

### 3.5 e2e against localnet in docker

Matrix (fail-fast off):

| Lane | Image | Purpose | Required? |
|---|---|---|---|
| pinned | `ghcr.io/raofoundation/subtensor-localnet:v<spec in metadata/index.toml for finney>` | the compat floor rtao claims | yes |
| mainnet-moving | `:mainnet` (== current `v<spec>`) | catches a mainnet upgrade before the pin moves | yes (same as pinned until a release) |
| testnet | `:testnet` | next runtime, hours–days early | yes, but allowed to open a "pre-break" issue instead of failing main |
| devnet | `:devnet` | further ahead; noisy | allowed-failure |
| main | `:main` (= `:latest`) | merged-but-unshipped features | allowed-failure, weekly |
| wasm-override | latest `:mainnet` node + released `subtensor.wasm` via `SUBTENSOR_LOCALNET_RUNTIME_WASM` | per-spec runtime without per-spec image (only `v473+` have images) | optional |

Harness: port-ephemeral `docker run -d --rm -p 127.0.0.1::9944 -p 127.0.0.1::9945 <image> [True|False] [--no-purge]`, readiness via the image healthcheck (`docker inspect .State.Health.Status == healthy`, since the image defines one) or `Imported #1` in logs (upstream), `E2E_ENDPOINT` override to reuse a running node (`e2e_support/mod.rs:64-71`), `LOCALNET_IMAGE` env override, `SKIP_PULL` (`:73-78`). Shard with `cargo nextest` or upstream's manifest-driven round-robin (`check-bittensor-e2e-tests.yml:180-194,434-558`: 112 tests, 32 shards, one container per test, `--test-threads=1`, retry once). One container per test is the simplest isolation (fast-runtime boots in ~10–20 s; **inferred** from the 90 s healthcheck start-period and 180 s harness deadline).

Test inventory to mirror (names from `ST/sdk/bittensor-core/tests/e2e-manifest.json`, 73 `intent_*` + 39 `test_*`): transfer exactness, add_stake increases stake, delegation lifecycle, owned subnet registration (`register_network` + `start_call`, `e2e_support/mod.rs:189-218`), serving endpoints, hyperparameter set/unsettable, identity, auto-stake destination, batch atomicity/revert, policy/spend caps, MEV shield (ML-KEM-768 key, shielded pipeline), crowdloan CRUD, nested sudo compose, commitments (regular + timelocked reveal), coldkey swap announcement flow, key association, fee planning vs live fee, pending-op cancel, multisig m-of-n, proxy flow, block number/subscription, metagraph fast path == runtime API, snapshot-pinned reads, quote_stake slippage. Validator/miner specifics for rtao beyond this: set/commit/reveal weights (incl. mechanism variants and CRV3 timelock), metagraph sync at tempo boundaries under `False` (12 s) blocks, btauth client↔server over HTTP between two rtao processes.

Localnet-specific pitfalls (all verified): the sudo key is Alice; `faucet` exists only here; constants are fast values; the chain spec (and genesis hash) is regenerated per container; `pow_register`/PoW difficulty tests need the non-fast runtime or `AdminUtils` sudo tweaks; `localnet_patch.sh` already lowers `DefaultPendingCooldown` and `SetChildren` rate limit. Upstream's clone prep script shows the sudo knobs to open registration on a cloned mainnet (`ST/sdk/python/scripts/prepare_clone_for_e2e.py:8-20,67-74`).

Optional mainnet-clone lane (weekly): run the production node image with `build-patched-spec` against `raw_spec_finney.json` (`clone-mainnet.sh:30-36`; 20–35 min warp sync), then rtao's read-only suite (metagraph for every netuid, all typed reads, runtime APIs) against real state — the only way to catch "decodes on localnet but not on mainnet-scale state" (e.g. bounded vs transitional formats).

### 3.6 Optional Python-interop e2e (docker)

One `docker compose` with: (a) localnet (`:v<spec>`), (b) `python:3.12` + `uv pip install bittensor==<stable matching spec>` (PyPI; stable 11.3.0 for v475/477 era), (c) rtao test binary. Scenarios:

1. **btauth/1 cross-language**: Python `bt.http_auth.sign` → rtao server verifies; rtao client → FastAPI miner recipe from `signed-requests.mdx:104-157`; both schemes; replay/freshness negatives; also verify Python's `<Bytes>…</Bytes>` tolerance is *not* produced by rtao (`:239-243`).
2. **Wallet interop**: `btcli wallet new-coldkey/new-hotkey` (and `regen-coldkey --seed`) in the Python container → rtao reads the keyfiles (encrypted NaCl + plain hotkey) and signs; rtao-created wallets read by `btcli` (`test_keyfiles_golden.py:87-109` flow).
3. **Wire-format parity**: for a fixed call set, have Python emit `codec.call_data`/`signature_payload`/`sign_without_nonce_tracking` with the fake signature (`record_golden.py:239-311`) and rtao emit the same; byte-compare (this is effectively re-recording golden.json with Python and replaying in Rust — a stronger, spec-current version of §3.4).
4. **Behavioural parity**: Python validator + rtao miner and vice versa on a localnet subnet; metagraph equality between `bt.metagraph` and rtao's.

Pin the Python version per matrix cell; the compat matrix (§3.9) records `bittensor` PyPI version × spec.

### 3.7 Catalog diff job

Nightly: fetch `https://bittensor.com/catalog/{intents,reads,errors}.json` and `/code/index.json`; store under `fixtures/catalog/<index.json.commit>/`; diff per §1.6; post a digest to the drift PR/issue. Low cost, high signal for intents rtao chooses to mirror (e.g. "which pallet call does `set_weights` intent wrap this week").

### 3.8 Fuzz, property tests, benchmarks

- `proptest`: SCALE encode/decode round-trips for every registry type (generate values from `scale-info` type defs — the Rust analogue of `record_shape_corpus.py:47-130` byte generator), ss58 encode/decode, compact ints, era encoding (mortal period/phase), btauth payload builder (method/path/body/nonce arbitrary), keyfile encrypt/decrypt.
- `cargo fuzz` (nightly smoke like SN2: `ECO/inference-labs-inc_subnet-2/.github/workflows/fuzz.yml:7,45-51` runs each target briefly on a cron): targets `decode_metadata_prefixed`, `decode_extrinsic`, `decode_events`, `decode_storage_value(type_id, bytes)`, `btauth_verify(headers, body)`, `keyfile_parse`, `ss58_decode`.
- Benchmarks (`criterion`): metadata parse (V15 386 KB / V16 ~405 KB), metagraph decode for a 256-UID subnet from recorded `runtime_calls` bytes, 10k storage-key derivations, bulk `query_map` decode, btauth sign/verify throughput. Upstream's spec has explicit performance targets for the codec phase (`bittensor-core-spec.md:360-383`) and bench scripts (`ST/sdk/python/scripts/bench_transport.py`, `bench_neurons.py`) to compare against. Track with `bencher`/`criterion` history; gate only on gross regressions (>30 %).

### 3.9 Release automation and the compat matrix

- Versioning: SemVer per crate; `cargo-semver-checks` in CI; `release-plz` (or `cargo-release`) opening a release PR per merged change set; tags `rtao-vX.Y.Z`; `cargo publish` via trusted publishing from a `release` environment (mirrors upstream's environment-gated publication model).
- A rtao release is cut whenever (a) a breaking drift PR merged (bump major of the generated-bindings crate; the core crate may stay minor), or (b) features land. The release PR body is generated by `cargo xtask compat-matrix`.
- **Compat matrix** (README + `COMPAT.md` + machine-readable `compat.json`), one row per rtao release:

| rtao | subtensor spec range (validated) | metadata pins | localnet images tested | subxt | bittensor (PyPI) interop tested | btauth |
|---|---|---|---|---|---|---|
| 0.1.0 | 477–478 | finney 477 (code 0x…), testnet 478 | `v477`, `:testnet@sha256:…`, `:devnet@…` | 0.51.1 | 11.3.0 / 11.3.1rc68 | btauth/1 |

"Validated" means: drift diff from the lowest to the highest pinned spec contains no *breaking* class (§3.3 step 4) **and** the e2e lane passed on the image digests listed. Add a runtime guard in rtao: on connect, compare the chain's `(spec_version, metadata hash)` with the compiled pins; if outside the validated range, log a warning and (configurably) switch to dynamic-metadata mode for reads while refusing to sign calls whose index/args are not in the pinned table (behaviour like Python's spec-mismatch refusal, `interface.py:617-623`).

### 3.10 Live-network probes (scheduled, read-only)

Every 6 h (`scheduled-smoke-tests.yml:7-9`): for each network, finality lag (<10 min, as upstream asserts), `spec_version`, `:code` hash, V15 metadata digest (recomputed, compared with the pin), and a metagraph decode of netuid 1 and the largest netuid. Publish a status badge; this is also the signal that moves the drift job out of its idle path.

---

## 4. Maintenance playbook

Effort estimates assume the tooling in §3 exists; "day" = one engineer-day. All marked **inferred** (estimates), the mechanics are verified.

### (a) A new runtime lands (devnet → testnet → mainnet)

Checklist (automated parts in *italics*):

1. *Drift job detects `(spec, code_hash)` change on devnet; fetches V15/V16; opens PR with classified diff + release notes (if a `v<spec>` pre-release exists; devnet/testnet deploys usually precede the tag, since the tag is reserved only at the mainnet proposal step, `release-train.yml:472-505`).*
2. Read the classification. Additive-only (new calls/events/storage): merge after e2e on `:devnet`/`:testnet`; expose new calls/reads if desired (½ day). Behavioural (event index moves, storage type changes, API version bump): verify name-based event matching and typed reads; update per-spec tables; run e2e (½–1 day). Breaking (tx extension change, moved indices, changed args of a used call, `transaction_version` bump): fix the signer/encoder, bump rtao major for affected crates (1–3 days).
3. *When `v<spec>` is finalized on mainnet (watcher), the drift PR for finney appears; e2e runs on `:v<spec>` (image appears after the mirror push → `docker-localnet.yml`), and `:mainnet` flips.*
4. Re-record rtao's own golden/corpus for the new spec (`cargo xtask record --image …:v<spec>`, §5) and commit (10 min); keep the previous spec's fixtures (they remain valid).
5. Update `compat.json` range, cut a rtao release (release-plz PR) — ½ day including review.
6. If the new runtime removed a storage item or call rtao uses: deprecate in rtao with a compile-time `#[deprecated]` mapped from V16 `deprecation_info` (same day as the devnet drift, i.e. days before mainnet).

Typical total: additive runtime ≈ 1 hour of human time; behavioural ≈ ½ day; breaking ≈ 1–3 days. Frequency: ~2–4 finalized runtimes per week in Sept–Oct 2026 (§1.2), so automation of steps 1, 3 is mandatory.

### (b) The Python SDK adds an intent or read

Signals: `intents.json`/`reads.json` diff (§3.7), `_generated/*` diff on the `mainnet`/`testnet` branches (§1.7), PyPI rc version (§2.5). Action: decide whether rtao mirrors the intent (rtao is not obliged to mirror Python's intent layer; calls are already available via codegen). If mirrored: implement against the wrapped `(pallet, call)` from `intents.json[*].wraps`, port the `input_schema` constraints (amount unit rules: `TaoBalance` vs `AlphaBalance` type idents from `calls.py`), add an e2e in the intent lane, and add the read's `hits` to typed-reads tests. Effort: ½–1 day per intent, 1–2 h per read. Upstream's `--coverage` discipline (every call wrapped or declared raw-only, `check.py:303-346`) is worth copying as a test so new chain calls force an explicit decision in rtao.

### (c) btauth/1 changes

Per upstream policy, incompatible wire changes require a protocol version bump (`test_http_auth.py:3-6`), i.e. a `btauth/2` first line and presumably `X-Bittensor-Version: 2`. Detection: diff of `docs/guides/signed-requests.mdx` and `sdk/python/bittensor/http_auth.py` on upstream branches (add both paths to the nightly upstream-file watch), plus the vendored vectors test. Action: implement v2 side-by-side (verify both, sign the configured one), keep v1 for a deprecation window matching upstream's guidance; add new vectors. Effort: ½–1 day. Also watch the legacy-v10 bridge section (`signed-requests.mdx:291-321`) if rtao offers a compatibility verifier.

### (d) subxt releases a breaking version

History: 0.50.0 (2025-12-17) was a deliberately large rewrite (config at value level, `ExtrinsicParams`→`TransactionExtensions`, storage API redone, errors split, historic blocks support, `subxt-core` removed); 0.51.0 (2026-09-14) changed `DefaultTransactionExtensions`/`DefaultExtrinsicParams` to structs and added `custom_extension(name, value)` plus a V4-decoding fix for runtimes exposing multiple extension versions; patch releases every 1–3 months (subxt CHANGELOG). Mitigation: isolate subxt behind one rtao crate (`rtao-chain`) with a narrow internal trait; vendor the metadata and use `subxt-codegen` from the pinned version offline (never `subxt::subxt!` macro against a URL); keep a `subxt-next` CI lane building against the latest crates.io version (allowed-failure) so breakage is seen the week it ships; pin `subxt = "=0.51.1"` and bump via Dependabot/Renovate weekly PRs. Effort on a 0.50-class release: 2–5 days; on a 0.51-class release: ½–1 day. Alternative to reduce exposure: build rtao's codec on `frame-decode` (0.18.1) + `scale-value` (0.18.2) + `frame-metadata` (23.0.1) directly — what subxt itself does internally — and use subxt only for transport/codegen, or not at all (decision for the architects, §7).

### (e) Bonus events worth a runbook

- polkadot-sdk fork rev bump (`ST/Cargo.toml:150,227`): may change extension encodings or metadata version. Watch `Cargo.toml` diff on `main`.
- Metadata V17 / unstable versions: subxt's "latest" fetch may jump; pin requested versions (15 and 16) explicitly, never "latest".
- Node RPC changes (new node image): the smoke probe (§3.10) and the `verify-base-node-runtime.py`-style API signature check cover this.

---

## 5. rtao's own golden fixtures and shape corpus (re-recordable from a localnet, no Python)

### 5.1 Recorder (`cargo xtask record`)

Inputs: `--image ghcr.io/raofoundation/subtensor-localnet:v477` (or `--endpoint ws://…`), `--out fixtures/rtao/`. Refuse non-local endpoints unless `--allow-remote` (as `record_golden.py:41-50`) because it submits a real transfer. Steps mirror upstream so the resulting JSON is schema-compatible with `golden.json` (reuse the same keys and case lists from `record_golden.py:77-145` plus rtao-specific additions):

1. `network` + raw `metadata.v15_hex`/`v16_hex` (store both; upstream stores v15/v14) at a pinned head.
2. Storage keys, composed calls, signature payloads (immortal/mortal), fake-signed extrinsics with `FAKE_SIG = 1..=64` and `signature_version 1`, multisig derivations, ss58 vectors, decoded constants — all deterministic.
3. Submit one `transfer_keep_alive(Bob, 1 TAO)` from Alice; pin its block; record receipt, storage values, raw events, query maps, runtime calls, raw block, payment info at that block.
4. rtao extras: `metadata_digest` (RFC-0078, V15, chain info from `System.Version`+properties), a `set_weights`/`commit_weights`/`reveal_weights` composed-call set, a `metagraph` runtime-API response for netuid 1 (`SubnetInfoRuntimeApi.get_metagraph`), `get_neurons_lite`, a btauth/1 sign→verify self-check, and a **v5 extrinsic** encoding (deliberately recorded but not used for submission) to detect runtime changes in `transaction_extensions_by_version`.
5. Write `golden-<spec>.json` with `recorded_from: {image, image_digest, spec_version, genesis_hash, block_hash, rtao_version, recorded_at}`.

Because the localnet runtime differs from mainnet in constants and the `faucet` call (§1.1), the recorder should also run `--endpoint wss://…finney` in **read-only mode** (no submission) to capture mainnet-only vectors (storage keys/values, runtime calls, block decode) under `golden-<spec>-finney-ro.json`.

### 5.2 Shape corpus recorder (`cargo xtask corpus`)

Pure/offline: walk the portable registry of the pinned V16 (and V15) metadata, generate SCALE bytes per type with a deterministic RNG seeded `type_id*1000+i` (copy `ByteGen` rules: compact widths, every enum variant visited, sequences 0–3, arrays ≤4096, depth ≤8, `record_shape_corpus.py:47-130`), decode with rtao's decoder, serialize the decoded value with a `to_corpus_json` that matches upstream's conventions (`codec/value.rs:58-90`) — AccountId32→ss58, bytes→`0x`, compact unwrapped, variants as `{"Name": payload}`/`"Name"`, maps with string keys. Output `corpus-<spec>.json` with the same schema (`spec_version, codec: "rtao", types_total, types_covered, skipped[], types[]`). Keep coverage ≥95 % (upstream's threshold, `test_shape_corpus.py:41`). Then two tests: (i) rtao decodes its own corpus; (ii) rtao decodes upstream's `corpus-461.json` against the 461 metadata (cross-implementation oracle). When rtao deliberately changes a rendering convention, re-record (ii) becomes a documented divergence list rather than a failure.

### 5.3 Re-record workflow and review discipline

`cargo xtask record --image …:v<spec>` pulls the image, boots with `True`, waits for health, records, tears down (reuse the §3.5 harness). CI job `fixtures-fresh` runs it on every drift PR and diffs against the committed file; a diff is expected only when metadata changed. Review the diff by section (upstream: "eyeball the diff before committing", `sdk-tests.mdx:104-107`). Never hand-edit fixtures; the test that compares `golden-<spec>.network.spec_version` with `metadata/index.toml` prevents mixing specs (upstream's equivalent assertion: `codec/mod.rs:124-128`).

### 5.4 Fake chain for unit tests (no docker)

Upstream's `FakeSubstrate` (dict-backed storage seeding; extrinsics recorded, never applied, `sdk-tests.mdx:50-56`) and `FakeNode` (in-process websocket JSON-RPC server with drop/refuse/silence failure injection, `ST/sdk/python/tests/harness/fake_node.py:1-15`) are worth mirroring in Rust: an in-process `jsonrpsee`/`tokio-tungstenite` fake that serves the pinned metadata, `state_getRuntimeVersion`, `chain_*` heads, seeded storage from a golden file, and `author_submitAndWatchExtrinsic` → scripted `InBlock`/`Finalized` with events taken from `golden.receipt`. This makes executor/retry/reconnect logic testable offline and keeps the docker lane for behaviour only.

---

## 6. Gaps and risks found upstream (things rtao must not assume)

1. `ST/sdk/bittensor-core/tests/e2e_support/mod.rs:23` defaults to `ghcr.io/raofoundation/subtensor-localnet:monorepo-sdk`, a tag that **does not exist** on ghcr today (tag list has no `monorepo*`); CI always sets `LOCALNET_IMAGE` (`check-bittensor-e2e-tests.yml:37`). rtao must default to an existing moving tag (`:mainnet`) or a digest.
2. Event indices are not append-only (§2.2) and `subxt diff` does not cover events/errors/constants/indices — rtao needs its own semantic diff.
3. Localnet metadata ≠ mainnet metadata (faucet call 60, fast constants, patched rate limits) — do not pin finney bindings from the localnet image.
4. Upstream goldens are frozen at spec 461 while mainnet is 477: use them as frozen conformance vectors, not as "current chain" fixtures.
5. Only v432/v473/v475/v477 have per-spec localnet images; older specs need the wasm-override trick or a source build (`Dockerfile-localnet` source-builder target, `:5-21`).
6. Metadata is not a release asset; the digest JSON reports `metadata_version: 14` (subwasm's default extraction), so the digest cannot replace a V15/V16 fetch.
7. No upstream SDK↔spec compatibility declaration exists; rtao's compat matrix is the only one its users will have.
8. The `main` branch runs ahead of mainnet (478 vs 477 today); `:latest` localnet is `main`, not mainnet (`release-process.mdx:134-136`) — pick tags deliberately.
9. Pre-release tags can be abandoned (v470/471/476): never treat "a `v<spec>` tag exists" as "mainnet runs it"; use the finalized-runtime RPC check (`mainnet-finalized-runtime.sh`).

---

## 7. Decisions for the architects (with recommendations)

1. **subxt as the chain layer vs. a thinner `frame-decode`/`scale-value`/`frame-metadata` stack.** Recommendation: use subxt 0.51 for transport, codegen and signing *behind a single rtao crate*, since it already handles V16, multiple extension versions and custom extensions; revisit if the 0.50-class churn repeats. Keep rtao's dynamic decoder able to run on `frame-decode` alone so the shape corpus and drift tool do not depend on subxt.
2. **Metadata version for codegen: V16 (deprecations, extension versions) with V15 kept for digests.** Recommendation: both pinned; generate from V16.
3. **Static codegen per pinned spec vs. dynamic metadata at runtime.** Recommendation: static generated crate per *validated range* (typed, fast, semver-checked) + dynamic fallback for reads outside the range; signing only within the range.
4. **Docker harness: plain `docker` CLI (as upstream) vs. `testcontainers` 0.28.** Recommendation: plain CLI module (~100 lines, no extra deps, identical to upstream's proven flow), with `E2E_ENDPOINT`/`LOCALNET_IMAGE`/`SKIP_PULL` env contract copied verbatim so upstream docs apply.
5. **Vendor upstream fixtures (2.6 MB) vs. fetch-at-test-time by commit.** Recommendation: vendor, with sha256 and upstream commit recorded; refresh via `xtask import-upstream` PRs.
6. **Adopt upstream's decoded-value JSON conventions for rtao's dynamic `Value` rendering.** Recommendation: yes for the corpus/test surface (keeps the 802-type oracle usable), independent of rtao's typed API.
7. **Mirror the Python intents layer or expose calls only.** Recommendation: calls + typed reads first; mirror intents selectively, using `intents.json` as the spec and `--coverage`-style tests to force decisions on new calls.
8. **Where the compat floor sits.** Recommendation: floor = the oldest spec whose localnet image exists and whose drift-to-current is non-breaking (today 473 or 475 → 478); ceiling = highest e2e-passed devnet spec, published as "preview".
9. **Who owns drift PRs.** Recommendation: bot opens, human merges; breaking class requires a release note entry and major bump of the bindings crate.

---

## 8. Open questions

1. Does the `subtensor-localnet:v<spec>` image appear reliably for every finalized release going forward (it did for 473/475/477 but not 466–469, which predate the policy)? Worth confirming with the first release after today.
2. Exact readiness semantics of testcontainers' `WaitFor`/healthcheck for this image (not verified from docs.rs in this session).
3. Will upstream publish metadata (or a V15 digest) as a release asset if asked? A small upstream PR adding `metadata-v15.scale`/`metadata-v16.scale` to `watch-mainnet-release.yml` assets would remove step 2 of §3.3 for finney; worth proposing.
4. Is there a documented policy on call/event index stability? Observed behaviour (calls append-only, events not) is empirical over 461→477 only.
5. Extrinsic v5: will the runtime (or Python SDK) switch default submission to v5, and when? V16 already advertises `[4,5]`.
6. Licence/attribution text required when vendoring `golden.json`/`corpus.json` (repo is Apache-2.0 per image labels; confirm `LICENSE` at repo root).
7. How often do the website catalogs regenerate relative to mainnet releases (observed: production deploy is part of the watcher after approval)?

---

## Appendix A — Command cookbook (all exercised or taken from upstream scripts)

```bash
# finalized runtime identity (mainnet-finalized-runtime.sh:13-26)
rpc() { jq -cn --arg m "$1" --argjson p "$2" '{id:1,jsonrpc:"2.0",method:$m,params:$p}' | curl -sf -H 'Content-Type: application/json' -d @- https://entrypoint-finney.opentensor.ai; }
head=$(rpc chain_getFinalizedHead '[]' | jq -r .result)
rpc state_getRuntimeVersion "[\"$head\"]" | jq '.result | {specVersion, transactionVersion}'
rpc state_getStorageHash "[\"0x3a636f6465\", \"$head\"]" | jq -r .result        # blake2_256 of :code
# metadata V15 / V16 at that block (runtime.py:33-34; client.rs:1241)
rpc state_call "[\"Metadata_metadata_at_version\", \"0x0f000000\", \"$head\"]" | jq -r .result > v15.optionhex
rpc state_call "[\"Metadata_metadata_at_version\", \"0x10000000\", \"$head\"]" | jq -r .result > v16.optionhex
# (strip the SCALE Option<Vec<u8>> wrapper: 0x01 + compact len + bytes)

# subxt-cli 0.51.1 (installed at ~/.cargo/bin/subxt)
subxt metadata --url wss://entrypoint-finney.opentensor.ai:443 --version 15 -f bytes > finney-477.v15.scale
subxt diff old.scale new.scale            # calls/storage/runtime-api diff (no events/errors/indices)
subxt explore --file META/finney.scale pallet SubtensorModule calls set_weights

# release assets + digest (verified for v477)
gh release view v477 --repo RaoFoundation/subtensor --json assets,targetCommitish
curl -fsSL https://github.com/RaoFoundation/subtensor/releases/download/v477/subtensor-digest.json | jq '.runtimes.compressed.subwasm.core_version'

# ghcr tags without auth
tok=$(curl -fsSL --get --data-urlencode 'scope=repository:raofoundation/subtensor-localnet:pull' https://ghcr.io/token | jq -r .token)
curl -fsSL -H "Authorization: Bearer $tok" 'https://ghcr.io/v2/raofoundation/subtensor-localnet/tags/list?n=1000' | jq '.tags'

# localnet (local-development.mdx:13-15; e2e_support/mod.rs:85-99)
docker run --rm -d --name ln -p 127.0.0.1::9944 -p 127.0.0.1::9945 ghcr.io/raofoundation/subtensor-localnet:v477 True
docker inspect --format '{{.State.Health.Status}}' ln ; docker port ln 9944/tcp
```

## Appendix B — Evidence index (files read in full or in the cited ranges)

`ST/docs/internals/{sdk-tests,testing,release-process,contributing,eco-tests,mainnet-clone,repo-layout,scripts,null-consensus-testnet,benchmarks-and-weights}.mdx`; `ST/.github/workflows/{release-train,watch-mainnet-release,docker-localnet,docker,check-localnet-publication,check-node-image-publication,check-bittensor-e2e-tests,publish-sdk-dev,build-core-wheels,update-chainspec,eco-tests,scheduled-smoke-tests,rustdocs,cleanup-localnet-ci-images,cargo-audit,check-rust,runtime-checks}.yml`; `ST/.github/scripts/{resolve-localnet-image,publish-localnet-manifest,test-localnet-image,resolve-node-image,resolve-mainnet-release,mainnet-finalized-runtime,resolve-release-artifact}.sh`, `verify-base-node-runtime.py`, `deploy/README.md`; `ST/scripts/{localnet,localnet_patch,localnet_healthcheck,install_prebuilt_binaries,preflight}.sh`; `ST/Dockerfile-localnet`, `ST/docker-compose.localnet.yml`; `ST/sdk/python/codegen/{__main__,metadata,check,emit_python}.py`, `ST/sdk/python/scripts/{record_golden,record_shape_corpus,prepare_clone_for_e2e}.py`, `ST/sdk/python/tests/{conftest.py,harness/fake_node.py,unit/test_codec_golden.py,unit/test_storage_golden.py,unit/test_shape_corpus.py,unit/test_keyfiles_golden.py,unit/test_http_auth.py,unit/test_wallet_compat.py,unit/test_sp_core_parity.py,e2e/*.py}`, `ST/sdk/python/tests/fixtures/{golden.json,shape_corpus/corpus.json}` (via jq), `ST/sdk/python/bittensor/_generated/*.py` (heads), `ST/sdk/python/bittensor/_transport/{runtime,interface}.py`, `ST/sdk/python/bittensor/executor.py`, `ST/sdk/python/{pyproject.toml,justfile}`, `ST/sdk/bittensor-core-py/{pyproject.toml,Cargo.toml}`; `ST/sdk/bittensor-core/{Cargo.toml,tests/e2e.rs,tests/e2e_support/mod.rs,tests/e2e-manifest.json,tests/Dockerfile.localnet-fast,src/codec/mod.rs,src/codec/extrinsic.rs,src/codec/value.rs,src/digest/mod.rs,src/client.rs}`, `ST/sdk/bittensor-core-spec.md`; `ST/eco-tests/**`; `ST/ts-tests/{moonwall.config.json,suites/smoke/test-block-finalization.ts,e2e-shards.json}`; `ST/clones/scripts/clone-mainnet.sh`; `ST/chainspecs/`, `ST/snapshot.json`, `ST/AGENTS.md`, `ST/rust-toolchain.toml`, `ST/Cargo.toml`, `ST/runtime/{src/lib.rs,build.rs,Cargo.toml}`, `ST/common/src/lib.rs`, `ST/node/src/chain_spec/{localnet.rs,finney.rs,testnet.rs}`, `ST/node/src/cli.rs`, `ST/pallets/subtensor/src/macros/dispatches.rs:1050-1066`, `ST/website/apps/bittensor-website/scripts/generate.py:1180-1300`, `ST/docs/guides/{signed-requests,local-development}.mdx`, `ST/docs/{sdk,migration}.mdx` (grep); `DOCS/{intents,reads,errors,index}.json`; `META/{finney,test}.scale`, `META/finney.json`, `META/subxt-install.log`; live: GitHub releases/PRs (`gh`), ghcr tag lists and manifests, PyPI JSON, crates.io JSON, subxt CHANGELOG, docs.rs testcontainers pages; ecosystem: `ECO/{CortexLM_bittensor-rs,timehash13_subtensor-api-rs,womboai_rusttensor,one-covenant_bittensor-rs,inference-labs-inc_subnet-2,sam0x17_mev-scanner}` (Cargo.toml, build.rs, CI, metadata tests).
