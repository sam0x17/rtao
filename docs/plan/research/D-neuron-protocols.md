# D — Neuron-to-neuron networking protocols rtao must speak (wire formats, verified)

Date: 2026-10-13. Reader: rtao architects (expert Rust/Substrate). Everything below is cited to files on disk or to GitHub objects fetched today with `gh api`; claims that are inferred rather than read are marked **[inferred]**.

Path aliases used in citations:

- `$UP` = `SP/upstream`
- `$META` = `SP/meta`
- `gh:<owner>/<repo>@<branch>:<path>` = file fetched from GitHub on 2026-10-13 (not on disk)

Protocol inventory (what exists in the wild today):

| Protocol | Status | Normative source | Who speaks it |
| --- | --- | --- | --- |
| **btauth/1** (hotkey-signed HTTP headers) | current, official v11 | `$UP/subtensor/docs/guides/signed-requests.mdx` + `$UP/subtensor/sdk/python/bittensor/http_auth.py` | v11 SDK users (new subnets; migration target) |
| **Legacy Synapse / Axon / Dendrite** (`bt_header_*`) | archived with v10.5.0, still deployed widely | `$UP/bittensor/bittensor/core/{synapse,axon,dendrite,stream}.py` | SN8 Vanta, SN13 Data Universe, SN21 Omega, … (survey §6) |
| **Epistula v2** (Manifold) | de-facto community standard, several divergent implementations | https://epistula.sybil.com/protocol + `gobt`/BitMind/Apex code | SN4 Targon, SN34 BitMind, SN1 Apex (variant), IOTA |
| **fiber** signed headers (+ optional RSA/Fernet envelope) | Rayon Labs lightweight SDK, v2.5.0 on disk, v2.7.0 fork used by SN56 | `$UP/fiber/fiber/{validator,miner,encrypted}` | SN56 Gradients, (historically SN19) |
| **btlightning** QUIC | crates.io 0.2.15 (2026-08-05), used by SN2 | `$UP/inference-labs-inc_btlightning/ARCHITECTURE.md` + `crates/btlightning/src` | SN2 Omron |
| per-subnet custom (X-Chutes-*, Basilica gRPC) | subnet-specific | GitHub | SN64 Chutes, SN39 Basilica |

---

## 1. btauth/1 — precise spec for a Rust implementer

### 1.1 Headers

Header names are case-insensitive, values exact (`signed-requests.mdx:40-50`; the Python verifier lowercases all incoming keys, `http_auth.py:270`).

| Header | Value | Emitted by `sign()` |
| --- | --- | --- |
| `X-Bittensor-Version` | `1` | always (`http_auth.py:229-230`) |
| `X-Bittensor-Hotkey` | sender hotkey, ss58 | always (`:231`) |
| `X-Bittensor-Receiver` | receiver hotkey, ss58 | only when `receiver_ss58` given (`:237-238`) |
| `X-Bittensor-Nonce` | unix time in **nanoseconds**, decimal | always; `time.time_ns()` unless injected (`:213`, `:232`) |
| `X-Bittensor-Crypto` | `ed25519` | only when scheme != `sr25519` (`:235-236`); absent means sr25519 |
| `X-Bittensor-Signature` | `0x` + lowercase hex of the 64-byte signature | always (`:233`) |

Constants: `PROTOCOL = "btauth/1"`, `VERSION = "1"`, `DEFAULT_MAX_AGE = 10.0` s, `DEFAULT_ALLOWED_SKEW = 2.0` s, unbound-receiver sentinel `_NO_RECEIVER = "-"`, default scheme `sr25519` (`http_auth.py:58-75`).

Reserved, **not** part of btauth/1: `X-Bittensor-Drand-Round`, `X-Bittensor-Drand-Randomness` (`signed-requests.mdx:220-224`). Do not emit or interpret them.

### 1.2 Signed payload (byte-exact)

UTF-8 bytes of exactly eight lines joined by `\n`, **no trailing newline** (`signed-requests.mdx:52-64`; `http_auth.py:175-185`):

```
btauth/1
<scheme: "sr25519" | "ed25519">
<HTTP method, uppercase>
<request target: path + "?query" exactly as sent, percent-encoded wire form>
<sha256(raw body bytes), lowercase hex>
<nonce, decimal>
<sender hotkey ss58>
<receiver hotkey ss58, or "-" if unbound>
```

Python reference (`http_auth.py:175-185`):

```python
lines = (PROTOCOL, scheme, method.upper(), path, hashlib.sha256(body).hexdigest(),
         str(nonce_ns), sender_ss58, receiver_ss58 or _NO_RECEIVER)
return "\n".join(lines).encode()
```

Rules that matter for a Rust port:

- Method is upper-cased by `build_payload` (`:178`); the verifier passes whatever the framework reports, so a server must pass the real method.
- Request target is the **percent-encoded** form (`/a%20b?q=1`, never `/a b`); ASGI gives decoded `path`, so the FastAPI recipe rebuilds it from `scope["raw_path"]` + `"?" + query_string` (`signed-requests.mdx:66-69`, `:119-126`). In Rust/hyper the raw `PathAndQuery` is already the wire form — use `uri.path_and_query()`; do not normalise, do not strip a trailing `?`.
- Empty body → `sha256(b"")` = `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` (GET round-trip test, `test_http_auth.py:117-127`).
- `scheme` on the verifier side is **re-derived** from the parsed crypto type, never echoed from the header string (`http_auth.py:289-291`), so `X-Bittensor-Crypto: SR25519` or `1` verify fine (`test_http_auth.py:139-142`) but the signed line is always canonical lowercase.

### 1.3 Crypto

- Crypto type codes (btwallet/py-substrate-interface convention): `CRYPTO_ED25519 = 0`, `CRYPTO_SR25519 = 1` (`$UP/subtensor/sdk/bittensor-core/src/keys/mod.rs:28-29`); names `{0: "ed25519", 1: "sr25519"}` (`$UP/subtensor/sdk/python/bittensor/wallets.py:22-25`); `parse_crypto_type` accepts the names case-insensitively or the digits (`wallets.py:41-52`).
- Signing = `sp_core::sr25519::Pair::sign` / `sp_core::ed25519::Pair::sign` on the raw payload (`keys/mod.rs:391-399`), i.e. sr25519 with the substrate signing context `b"substrate"` (schnorrkel `SigningContext`), ed25519 plain RFC 8032 (`signed-requests.mdx:73-76`, `:233-239`). Signature is the raw 64 bytes.
- Verification = `sp_core::{sr25519,ed25519}::Pair::verify` under the declared scheme only (`keys/mod.rs:79-100`); **never try both schemes** (`signed-requests.mdx:87-93`; test `test_http_auth.py:199-205`).
- Asymmetry to know: the Python SDK's verifier (via `bittensor_core::keys::verify`) additionally accepts a signature over `b"<Bytes>" + payload + b"</Bytes>"` for Polkadot-JS `signRaw` compatibility (`keys/mod.rs:457-469`; documented `signed-requests.mdx:240-244`). Other implementations are not required to accept the wrapped form and **must never produce it**. Recommendation for rtao: do not accept wrapped by default; offer an opt-in `accept_polkadotjs_wrapped` flag if ever needed.
- ss58: Bittensor format 42 (`wallets.py:30-36` pins `SS58_FORMAT`); the verifier decodes the sender ss58 to a 32-byte key. Undecodable ss58 or wrong-length signature → `MalformedAuth` (`http_auth.py:324-328`).

### 1.4 `sign()` algorithm (client)

(`http_auth.py:188-239`)

1. Resolve a hotkey signer; map its crypto type to `"sr25519"`/`"ed25519"`, else error.
2. `nonce = time.time_ns()` (or injected).
3. `payload = build_payload(scheme, method, path, body, nonce, sender_ss58, receiver_ss58)`.
4. `signature = signer.sign(payload)` (must be synchronous bytes).
5. Emit headers per §1.1. Every attempt, **including retries of an identical body, signs a fresh nonce** (`signed-requests.mdx:101-102`, `:163-167`); transport-level retries that resend an already-signed request are rejected as replays.

### 1.5 `verify()` algorithm (server), in order

(`http_auth.py:242-343`; order is normative, `signed-requests.mdx:78-99`.) Inputs: headers, **raw received body bytes**, received method, received request target, own hotkey ss58, `max_age` (10 s), `allowed_skew` (2 s), `require_receiver=True`, nonce store, `now_ns`.

| Step | Check | Error class (Python message) |
| --- | --- | --- |
| 1 | `X-Bittensor-Version` == `"1"` | `MalformedAuth("missing or unsupported X-Bittensor-Version (expected '1')")` (`:272-274`) |
| 1 | hotkey header present and non-empty | `MalformedAuth("missing X-Bittensor-Hotkey")` (`:275-277`) |
| 1 | nonce header present, `int()` parses | `MalformedAuth("missing X-Bittensor-Nonce")` / `("X-Bittensor-Nonce is not a decimal integer")` (`:278-284`) |
| 1 | crypto header (default `sr25519`) parses | `MalformedAuth("unsupported X-Bittensor-Crypto")` (`:285-288`) |
| 1 | signature header present, hex after optional `0x` | `MalformedAuth("missing X-Bittensor-Signature")` / `("X-Bittensor-Signature is not hex")` (`:292-298`) |
| 2 | receiver: absent and `require_receiver` → reject; present and != own hotkey → reject | `WrongReceiver("missing X-Bittensor-Receiver: this server requires receiver-bound requests")` / `("request was signed for a different receiving hotkey")` (`:300-307`) |
| 3 | `now - nonce > max_age*1e9` → stale; `nonce - now > allowed_skew*1e9` → future | `StaleRequest("nonce is older than the 10s freshness window")` / `("nonce is more than 2s in the future")` (`:309-313`) |
| 4 | rebuild payload from received method/path/body (hash always recomputed) and the **received receiver header value** (`None` → `"-"`); verify under declared scheme only | `MalformedAuth("invalid hotkey address or signature encoding")` on decode failure; `BadSignature("signature does not verify against the claimed hotkey")` (`:315-330`) |
| 5 | store sanity: `max_age + allowed_skew <= store.retention` else **ValueError (server misconfiguration, not an auth error)** (`:332-339`) | — |
| 5 | `store.check_and_store(sender, nonce)` must return True | `ReplayedRequest("this nonce was already accepted from this hotkey")` (`:340-341`) |

Returns `Caller{hotkey_ss58, nonce_ns, crypto_type}` (`:106-112`, `:343`). Error messages never echo signature/payload and are safe to return to the caller (`:78-83`).

Subtleties for a faithful port:

- The receiver line in the rebuilt payload is the *received* header value; with `require_receiver=False` an unbound request (`"-"`) verifies (`test_http_auth.py:217-221`).
- Freshness is checked **before** the signature (cheap rejection), the replay store is consulted **only after** the signature verifies so unauthenticated traffic cannot poison it (`signed-requests.mdx:94-99`).
- A tampered nonce fails as `BadSignature` (not stale) because the nonce is inside the signed payload (`test_http_auth.py:183-186`).

### 1.6 Replay protection: nonce store semantics

- Protocol: `check_and_store(hotkey_ss58, nonce_ns) -> bool` — True (and record) if never seen, False if seen. Entries only need to outlive the freshness window (`http_auth.py:115-123`).
- Default `InMemoryNonceStore(retention=60.0)`: set of `(hotkey, nonce)` + FIFO expiry deque keyed on `time.monotonic()`, lock-protected; expired entries are evicted lazily on each call (`:126-151`). Retention must be `>= max_age + allowed_skew`; the verifier refuses to run otherwise (`:334-339`; test `:164-171`).
- Multi-process/multi-host: supply a store backed by shared storage; "`SET NX EX` in Redis is exactly this shape" (`signed-requests.mdx:97-99`). Key = `(hotkey, nonce)`, TTL = retention.
- Nothing requires monotonic nonces per sender (unlike legacy Synapse): two concurrent requests from one hotkey with different nanosecond nonces are both valid.

### 1.7 Error → HTTP status mapping

The SDK leaves HTTP mapping to the framework: the FastAPI recipe maps **every** `AuthError` to `401` with `detail=str(e)` (`signed-requests.mdx:134-135`), and returns `403` for a verified-but-unwelcome caller (metagraph policy) (`:155-156`). The module docstring says "map it to HTTP 401/403" (`http_auth.py:30-31`). There is no normative per-subclass table; the following is the recommendation for rtao's tower layer (keep 401 for all five auth classes so that behaviour matches the recipe, expose the class in a response header/body for diagnostics):

| Class | Suggested status | Note |
| --- | --- | --- |
| `MalformedAuth` | 401 (could be 400) | missing/unparseable headers |
| `WrongReceiver` | 401 | proxying defence |
| `StaleRequest` | 401 | client should re-sign (clock) |
| `ReplayedRequest` | 401 | includes transport-level retries |
| `BadSignature` | 401 | |
| policy rejection (not registered / no permit / low stake) | 403 | not an `AuthError` |

### 1.8 Golden test vectors (verbatim from `signed-requests.mdx:252-289`, duplicated in `test_http_auth.py:19-36`)

Fixtures: sender `//Alice`, receiver `//Bob` (sr25519: `5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty`), nonce `1752076800000000000` (2025-07-09T16:00:00Z in ns), body `{"prompt": "hello"}` (19 bytes, no trailing newline), request `POST /generate?stream=false`.

`sha256(body)` = `341c57448e531310fbbe83f44cea2a5e838bd9e8a6b82b269f01d0dbbc23c3cc`.

**sr25519** — `//Alice` is `5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY`. The exact signed payload:

```
btauth/1
sr25519
POST
/generate?stream=false
341c57448e531310fbbe83f44cea2a5e838bd9e8a6b82b269f01d0dbbc23c3cc
1752076800000000000
5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
```

sr25519 signatures are randomized, so no fixed signature bytes exist: a conforming implementation signs this payload and verifies it against Alice's public key (and verifies any signature produced by another implementation).

**ed25519** — deterministic, so the signature bytes are pinned. `//Alice` with ed25519 derivation is `5FA9nQDVg267DEd8m1ZypXLBnvN7SFxYwV7ndqSYGiN9TTpu`; the payload is identical except line 2 is `ed25519` and line 7 is that address. The signature must be exactly:

```
0x1a6b631988bf1f7ea093ab9d5acf61f05c316d0b0e428b24b487b75a5f6ee0e1a56a26918fdf871b6cf09437e6588023a4ce423cf622621b2a7feee17612f704
```

The test file's note is the governing policy: "a change that breaks these tests breaks every non-Python implementation and needs a protocol version bump, not a fixture update" (`test_http_auth.py:3-6`). Other behaviours pinned by the Python tests that rtao's suite should mirror: lowercased header keys verify (`:134-136`); `X-Bittensor-Crypto` omitted for sr25519 (`:130-131`); stale at `now = nonce + 11 s`, future at `now = nonce - 3 s` (`:148-153`); replay on second verify with same store (`:156-161`); `retention=0` store re-accepts (`:174-177`); tampered body/method/path → `BadSignature` (`:189-196`); missing any of Version/Hotkey/Nonce/Signature → `MalformedAuth` (`:227-234`); `Nonce=not-a-number`, `Crypto=ecdsa`, `Signature=0xnothex`, `Version=2` → `MalformedAuth` (`:237-251`).

### 1.9 Framework recipe shapes → rtao equivalents

FastAPI server recipe (`signed-requests.mdx:111-141`): dependency reads `await request.body()` (raw, cached), rebuilds target from `raw_path` + `query_string`, calls `verify(...)`, maps `AuthError` → `HTTPException(401)`, and only then parses JSON. Policy snippet (`:147-156`): metagraph lookup by hotkey, require `validator_permit` and `tao_stake >= 1000 τ`, 403 otherwise; use `caller.hotkey_ss58` as the rate-limit/priority key.

httpx client recipe (`:169-197`): an `httpx.Auth` with `requires_request_body = True` that signs `request.method`, `request.url.raw_path` (path + query as sent), `request.content`, receiver = miner hotkey; base URL `http://{miner.axon}` where `axon` is the metagraph's `"ip:port"`; fan-out is plain `asyncio.gather` (`:199-200`).

rtao mapping (design recommendation):

- `rtao-auth` (no I/O): `build_payload`, `sign`, `verify`, `Caller`, `AuthError` enum with the five variants, `NonceStore` trait + `InMemoryNonceStore` (+ `redis` feature implementing `SET NX EX`).
- Server: a tower `Layer`/`Service` or axum `FromRequest` extractor that (a) buffers the body up to a configurable limit **before** any JSON extractor runs, (b) uses `req.uri().path_and_query()` verbatim, (c) runs `verify`, (d) inserts `Caller` into extensions, (e) responds 401 with a JSON `{"detail": msg}` body to mirror FastAPI's shape. Streaming request bodies cannot be verified (hash needs all bytes) — document that btauth/1 requires buffered request bodies; responses may stream freely (signing covers only the request).
- Client: a `reqwest` middleware (or helper `sign_request(&mut Request, signer, receiver)`) that requires a fully materialised body (`Body::as_bytes()`), signs `method`, `url.path()` + `?` + `url.query()` (reqwest's `Url` keeps the percent-encoded form), and **re-signs on every retry** (so retries must be done above the signing layer, not via a transport-level retry policy).

### 1.10 Rust implementation notes

- Crypto: `sp-core` (`sr25519::Pair::{sign,verify}`, `ed25519::Pair`) matches the Python backend exactly (same crate, `keys/mod.rs:16-18`); lighter alternatives: `schnorrkel` with `signing_context(b"substrate")` and `ed25519-dalek`. `sha2::Sha256`, `hex`, ss58 via `sp_core::crypto::Ss58Codec` or a small base58+blake2b-512 checksum implementation (`keys/mod.rs:70-77` shows the checksum layout).
- Nanosecond nonce fits `u64` until 2554; parse as `u64`/`i128`, compare with `i128` arithmetic (Python compares `now - nonce > max_age * 1e9` as float — rtao should use integer ns).
- Pin §1.8 in unit tests; add proptests for payload canonicalisation (no trailing `\n`, uppercase method, lowercase hex).

---

## 2. Legacy v10 Synapse / Axon / Dendrite wire format (for an optional interop crate)

Source: archived `bittensor` 10.5.0 (`$UP/bittensor/pyproject.toml:7`; archived 2026-07-10 per git log). The v11 migration guide states v11 "contains **no** miner/validator networking stack" and gives a bridge recipe for accepting v10 callers (`$UP/subtensor/docs/migration.mdx:455-480`; `signed-requests.mdx:291-321`).

### 2.1 Transport and routing

- Dendrite sends `POST http://{ip}:{port}/{SynapseClassName}` (`dendrite.py:221-241`, `:566-568`); if the target ip equals the dendrite's own external ip the host is rewritten to `0.0.0.0` (`:237-241`).
- Headers = `synapse.to_headers()`; body = `json=synapse.model_dump()` (the full pydantic model including `dendrite`/`axon` TerminalInfo); client timeout `aiohttp.ClientTimeout(total=timeout)` with default 12 s (`:580-582`, `:541`).
- Axon registers each attached Synapse type at path `/{request_name}` with methods `GET` and `POST`, and a FastAPI dependency `verify_body_integrity` (`axon.py:560-563`); a default `Synapse` "ping" route is always attached (`:406-409`).

### 2.2 Header set (`synapse.py:590-666`)

| Header | Source |
| --- | --- |
| `name` | synapse class name (`:619`; `set_name_type` forces `name = cls.__name__`, `:411-413`) |
| `timeout` | `str(float)` (`:619`) |
| `bt_header_axon_<field>` | every non-None `TerminalInfo` field of `synapse.axon` (`:623-629`) |
| `bt_header_dendrite_<field>` | every non-None field of `synapse.dendrite` (`:631-637`) |
| `bt_header_input_obj_<field>` | for each **required** (non-Optional) model field: base64(json.dumps(**empty instance of the value's type**)) — a dummy placeholder, not the value (`:644-662`) |
| `header_size` | `str(sys.getsizeof(headers))` (Python object size; meaningless on the wire) (`:664`) |
| `total_size` | `str(get_size(self))` (`:665`) |
| `computed_body_hash` | `self.body_hash` (`:666`) |

`TerminalInfo` fields (`synapse.py:84-247`): `status_code: int`, `status_message: str`, `process_time: float`, `ip: str`, `port: int`, `version: int`, `nonce: int`, `uuid: str`, `hotkey: str`, `signature: str`; all optional; before-validators cast `process_time`→float and `port/version/nonce/status_code`→int (`:242-247`). Parsing back (`:729-818`, `from_headers` `:821`): keys containing `bt_header_axon_`/`bt_header_dendrite_`/`bt_header_input_obj_` are split on the prefix; `timeout`, `name`, `header_size`, `total_size`, `computed_body_hash` copied verbatim; unknown headers ignored.

Synapse base fields (`synapse.py:416-484`): `name`, `timeout` (default 12.0), `total_size`, `header_size`, `dendrite`, `axon`, `computed_body_hash` (frozen, default `""`), class var `required_hash_fields: tuple[str, ...] = ()`.

### 2.3 Body hash algorithm

`get_hash(content) = hashlib.sha3_256(content.encode("utf-8")).hexdigest()` (`$UP/bittensor/bittensor/utils/__init__.py:230-237`). `Synapse.body_hash` (`synapse.py:671-726`):

```
hashes = [sha3_256_hex(str(model_dump()[field])) for field in required_hash_fields]
body_hash = sha3_256_hex("".join(hashes))
```

- Field order = the class-var tuple order; the deprecated pattern of declaring `required_hash_fields` as a pydantic field (used by SN8 Vanta, `gh:taoshidev/vanta-network@development:template/protocol.py:66,75,83`, and the old stream tutorial `$UP/bittensor-subnet-template/docs/stream_tutorial/protocol.py:59-64`) is re-ordered to `model_dump()` order (`synapse.py:695-713`).
- `str()` of the Python value is hashed (not JSON): lists render as `['a', 'b']`, dicts with single quotes, floats via Python repr. A Rust implementation must reproduce Python `str()` for the concrete field types used by the target subnet — this is why the v11 docs call the scheme "schema-dependent, so it cannot be verified generically" (`signed-requests.mdx:296-299`).
- With no `required_hash_fields` the body hash is `sha3_256("")` = `a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a`.

### 2.4 Request signature (dendrite → axon)

(`dendrite.py:685-722`) Dendrite sets `synapse.dendrite = TerminalInfo(ip=external_ip, version=version_as_int, nonce=time.time_ns(), uuid=self.uuid, hotkey=keypair.ss58)` and `synapse.axon = TerminalInfo(ip, port, hotkey=target hotkey)`, then:

```
message = f"{dendrite.nonce}.{dendrite.hotkey}.{axon.hotkey}.{dendrite.uuid}.{synapse.body_hash}"
dendrite.signature = "0x" + keypair.sign(message).hex()
```

- `uuid` = `str(uuid.uuid1())` fixed per Dendrite instance (`:111`); the axon keys its nonce table by `f"{dendrite.hotkey}:{dendrite.uuid}"` (`axon.py:933`).
- Signing uses btwallet `Keypair.sign` (sr25519, substrate context; btwallet is the same `sp-core` lineage as `bittensor_core`, and its `verify` has the same `<Bytes>` fallback — `keys/mod.rs:401` says "Matches btwallet's `<Bytes>` wrapping fallback").
- `version_as_int` = `major*1000² + minor*1000 + patch` (`$UP/bittensor/bittensor/core/settings.py:160-168`), e.g. 10.5.0 → `10005000`; `V_7_2_0 = 7002000` is the threshold for the time-window nonce rule (`axon.py:55`).

### 2.5 Axon verification (`default_verify`, `axon.py:871-990`)

Order of checks: nonce present (`:936-937`); if `dendrite.version >= 7002000`: when no nonce stored for the endpoint key, require `nonce > now_ns - ALLOWED_DELTA - timeout_ns` where `ALLOWED_DELTA = 4_000_000_000` ns and `timeout` is the synapse's own `timeout` header (`$UP/bittensor/bittensor/utils/axon_utils.py:3-22`; `axon.py:947-960`); when a nonce is stored, require strictly increasing (`:965-968`); pre-7.2 versions only get the monotonic check (`:971-975`). There is **no upper bound** on future nonces. Then signature present (`:977-978`), `Keypair(ss58=dendrite.hotkey).verify(message, signature)` with `message = f"{dendrite.nonce}.{dendrite.hotkey}.{self.wallet.hotkey.ss58}.{dendrite.uuid}.{synapse.computed_body_hash}"` (`:927-930`, `:980`) — note the hash in the signed message is the **client-supplied header value** (`computed_body_hash`), and finally the nonce is stored (`:986`).

Body integrity is enforced separately and later: the route dependency `verify_body_integrity` re-parses the JSON body into the Synapse class, recomputes `syn.body_hash`, and raises `ValueError("Hash mismatch ...")` if it differs from the `computed_body_hash` header (`axon.py:679-736`, `:719-733`).

### 2.6 Axon pipeline order and status codes

`AxonMiddleware.dispatch` (`axon.py:1114-1214`): `preprocess` → `blacklist` → `verify` → `priority` → `run` (route; the body-integrity dependency and `forward_fn` execute here) → response post-processing in the route (`synapse_to_response`, `:1508-1562`).

- `preprocess` (`:1216-1290`): request name = first path segment (`:1244`); unknown → `UnknownSynapseError`; `Synapse.from_headers(request.headers)` (`:1264`); sets `axon.version`, `axon.uuid`, `axon.nonce = time_ns()`, `axon.status_code = 100` (`:1277`); fills `dendrite.ip/port` from the TCP peer; **signs the axon side**: `message = f"{axon.nonce}.{dendrite.hotkey}.{axon.hotkey}.{axon.uuid}"`, `axon.signature = "0x" + hotkey.sign(message).hex()` (`:1287-1288`). (`axon.hotkey` here comes from the request header the dendrite set.) The dendrite never verifies this response signature — `process_server_response` only merges headers (`dendrite.py:726-787`).
- Hooks: `blacklist_fn(synapse) -> (bool, reason)` → 403 `BlacklistedException("Forbidden. Key is blacklisted: {reason}.")` (`:1350-1403`); `verify_fn` (default `default_verify`) any exception → 401 `NotVerifiedException` (`:1293-1348`); `priority_fn -> float` run through a `PriorityThreadPoolExecutor`, `TimeoutError` → 408 `PriorityException` (`:1405-1468`).
- Error responses: `create_error_response` → `JSONResponse(status_code=axon.status_code or 400, headers=synapse.to_headers(), content={"message": status_message})` (`:991-1010`). `log_and_handle_error` default codes: `PriorityException` 503, `UnknownSynapseError` 404, `BlacklistedException` 403, `NotVerifiedException` 401, `InvalidRequestNameError`/`SynapseParsingError` 400, other `SynapseException` 500, non-Synapse exceptions 500 with `"Internal Server Error #<uuid4>"` (`:1014-1076`). Consequence **[inferred from control flow]**: the body-hash `ValueError` raised inside the route dependency is a non-`SynapseException` and therefore surfaces as **500**, not the 400 the docstring promises (`:698`).
- Success: `synapse_to_response` sets `axon.status_code = 200`, `status_message = "Success"`, `process_time`, body = serialized synapse, and copies `to_headers()` onto the response (`:1534-1559`), so the response carries `bt_header_axon_status_code`, `bt_header_axon_status_message`, `bt_header_axon_process_time`, `bt_header_axon_nonce`, `bt_header_axon_uuid`, `bt_header_axon_version`, `bt_header_axon_signature`, plus the echoed dendrite headers.
- Dendrite side: on 200 the whole local synapse is overwritten by the server's JSON (`dendrite.py:745-757`); otherwise `axon.status_code = HTTP status`, `axon.status_message = json["message"]` (`:759-763`); then `dendrite.status_code/message` mirror `axon.*` (`:785-786`). Client-side exception mapping (`dendrite.py:21-29`): connector error 503, `asyncio.TimeoutError` 408, payload error 400, generic client error 500, server timeout 504, disconnected/connection error 503. Helper predicates: `is_success` (200), `is_timeout` (408), `is_blacklist` (403), `failed_verification` (401) (`synapse.py:519-583`).

### 2.7 Streaming (`StreamingSynapse`)

- Server returns `BTStreamingResponse`, which sends `content-type: text/event-stream` plus the synapse headers, invokes the user's `token_streamer(send)` and then an empty final body frame (`$UP/bittensor/bittensor/core/stream.py:43-108`, `:85-93`). Despite the content type, the framing is **raw chunked bytes** chosen by the subnet (the tutorial sends `"".join(tokens).encode()` chunks with `more_body: True`, `$UP/bittensor-subnet-template/docs/stream_tutorial/README.md:204-206`, `:331-333`); there are no `data:` SSE lines.
- Client: `dendrite.call_stream` POSTs the same way and iterates `synapse.process_streaming_response(response)` (abstract; the tutorial does `async for chunk in response.content.iter_any(): tokens = chunk.decode().split("\n")`, `protocol.py:91-99`), then `extract_response_json(response)` rebuilds a dict from `_raw_headers` (`:110-154`), then `process_server_response` (`dendrite.py:655-665`).

### 2.8 Chain-side conventions of the legacy stack

- `Axon.info()` publishes `AxonInfo(version=version_as_int, ip=external_ip, ip_type, port=external_port, hotkey, coldkey, protocol=4, placeholder1=0, placeholder2=0)` (`axon.py:413-425`); `serve_axon_extrinsic` hard-codes `protocol=4` (`$UP/bittensor/bittensor/core/extrinsics/serving.py:207`). Legacy `AxonInfo.protocol` default 4; `is_serving = ip != "0.0.0.0"` (`$UP/bittensor/bittensor/core/chain_data/axon_info.py:41-48`). IP ↔ int via `netaddr` (`utils/networking.py:33-66`).
- TLS: legacy `Certificate` is a str `chr(algorithm) + public_key` (`utils/__init__.py:100-107`) passed to `serve_axon_tls`; nothing in dendrite/axon actually used HTTPS (no `https`/ssl handling in `dendrite.py`/`axon.py`).

### 2.9 What a Rust `rtao-compat-synapse` crate must provide

1. Header codec for `bt_header_*`, `name`, `timeout`, `computed_body_hash`, `total_size`, `header_size` (+ generate the dummy `bt_header_input_obj_*` for required fields).
2. `Sha3_256` body hash with a `PythonStr` trait reproducing `str()` for the field types used (str, int, float, bool, list/dict of those) — make this per-schema and testable against Python.
3. Dendrite signer (`"{nonce}.{sender}.{receiver}.{uuid}.{body_hash}"`, uuid1-style string, ns nonce) and Axon verifier with the exact window rule (4 s + `timeout`), per-`hotkey:uuid` monotonic table, and the response-side axon signature.
4. Status-code semantics of §2.6 and the error body `{"message": ...}`.
5. Raw-chunk streaming over `text/event-stream`.

Existing Rust attempt: `$UP/CortexLM_bittensor-rs` (last commit 2026-04-22) has `bittensor-synapse` (correct sha3 hash and signing message, `bittensor-synapse/src/hashing.rs:10-14`, `signing.rs:7-14`; correct `bt_header_*` prefixes, `header.rs:7-15`) but its `bittensor-dendrite` emits non-interoperable header names `bt-nonce`/`bt-uuid`/`bt-signature` (`bittensor-dendrite/src/signing.rs:13-18`) and its `bittensor-axon` verification middleware does not actually verify the sr25519 signature (`bittensor-axon/src/middleware.rs:90-95`). Useful as a reference for shape, not as a dependency.

---

## 3. Epistula v2 — precise description and the implementation zoo

Origin: Manifold Labs (SN4 Targon), 2024 (https://manifold.inc/releases/epistula-Release); spec site https://epistula.sybil.com/ (`/protocol`, `/implementation`); examples repo `gh:manifold-inc/epistula-examples@main` (python/go/typescript; `rust/todo.tmp` is empty). Not an Opentensor/Rao standard; the v11 docs never mention it.

### 3.1 Headers (spec)

Required: `Epistula-Version` (= `"2"`), `Epistula-Timestamp` (unix **milliseconds**, decimal), `Epistula-Uuid` (uuid4), `Epistula-Signed-By` (sender ss58), `Epistula-Request-Signature` (`0x` + hex sr25519 over the message below). Optional: `Epistula-Signed-For` (receiver ss58), `Epistula-Secret-Signature-0/1/2` (interval signatures, only when `Signed-For` is set). (spec `/protocol` and `/implementation`, fetched 2026-10-13.)

### 3.2 Signed strings (spec)

```
request:  "{sha256(body).hexdigest()}.{uuid}.{timestamp}.{signed_for or ''}"
interval: ts_ms = round(time.time()*1000); I = ceil(ts_ms / 1e4) * 1e4
          sig_k = sign(f"{I + (k-1)}.{signed_for}")   for k in 0,1,2   # offsets -1, 0, +1 (ms!)
```

Spec verification (`verify_signature_v2(signature, body, timestamp, uuid, signed_for, signed_by, now)`): `ALLOWED_DELTA_MS = 8000`; reject if `timestamp + 8000 < now_ms` ("Request is too stale"); no future bound; rebuild the message from the received `Epistula-Signed-For` header value (the spec code does not itself compare it with the server's hotkey — a verifier must add that check; gobt instead substitutes its own address, see §3.3); `Keypair(ss58=signed_by).verify(message, signature)`. The secret signatures are a DDoS-mitigation cache (stale time `1.6 min × (index+1)`), and the reference verifier never checks them. The spec's prose says "rounding to the nearest 10,000th" but the code uses `ceil`, and the ±1 offsets are ±1 **millisecond** on a 10 s grid (so sig 0/2 are never valid for a neighbouring interval) — treat them as opaque.

Epistula v1 (examples repo, `python/main.py`, `go/main.go`, `typescript/send_message.ts`): body `{"data", "nonce": time_ns, "signed_by", "signed_for", "version": 1}`, header `Body-Signature = "0x" + hex(sign(json.dumps(body)))`, 5 s window in ns; Go verifies with `schnorrkel.NewSigningContext([]byte("substrate"), message)`. v1 is obsolete; it was superseded by v2 because of rollout bugs (release page).

### 3.3 Implementations compared (each is its own dialect)

| Implementation | Message | Window | Secret sigs | Notes |
| --- | --- | --- | --- | --- |
| spec `/implementation` (Python) | `{sha256(body)}.{uuid}.{ts}.{signed_for}` | stale only, 8000 ms | emitted, never verified | Version `"2"` |
| **SN34 BitMind** `gh:BitMind-AI/bitmind-subnet@main:gas/protocol/epistula.py` | same (line 47, 89); for multipart it hashes each part then `sha256(json.dumps(body_copy))` (lines 29-39) | stale **and future**, `allowed_delta_ms=15000` (lines 63-88) | emitted per spec (51-58) | `EPISTULA_VERSION = "2"` (15); `MIN_VALIDATOR_STAKE = 20000` (16) |
| **gobt** (Go, used by SN4 Targon) `gh:subtrahend-labs/gobt@master:boilerplate/epistula.go` | sign: `{hash}.{uuid}.{ts}.{rSS58}` (33); verify: `{hash}.{uuid}.{ts}.{self_ss58}` (113) | parses ms timestamp as **seconds** (`time.Unix(i,0)`, 98) with 8000 ms delta (100) — effectively no freshness check | interval = `ceil(ts/1e4)` **without** `*1e4`, signs `{I±1}.{own address}` (31, 39-41) | verify via `sr25519.NewSigningContext("substrate")` (85) |
| **SN1 Apex** `$UP/prompting/shared/common/src/common/utils/epistula.py` | sign: `{sha256(body)}.{ts}.{signed_for or ''}` (91) — **no uuid**; verify: `{sha256(body)}.{ts}.` (64) | `timestamp + timeout < now` (62), units caller-defined | emitted with offsets [-1,0,1] (105-107) | extra headers `X-Spec-Version`, `X-Request-Id` (11, 32-33, 97-98); body canonicalised with `json.dumps(sort_keys=True)` (112-114); used for miner/validator → orchestrator calls (`src/validator/src/validator/validator_api_client.py:34`) |
| **IOTA** `gh:macrocosm-os/iota@main:shared/common/src/common/utils/epistula.py` | same family as Apex **[inferred from path/org; not read]** | | | orchestrator-mediated (`README` lines 19-20) |

Who speaks it today: SN4 Targon miners expose `GET /cvm` verified with `boilerplate.VerifyEpistulaHeaders` (`gh:manifold-inc/targon@main:cmd/miner/main.go:117-131`); SN34 BitMind validators→miners; SN1 Apex and IOTA for orchestrator APIs (not validator→miner axon traffic).

### 3.4 Recommended canonical behaviour for `rtao-compat-epistula`

Implement the spec dialect (`{sha256}.{uuid}.{ts_ms}.{signed_for}`, version `"2"`, hex `0x` sr25519 signatures with substrate context), with knobs: `allowed_delta_ms` (default 8000; BitMind 15000), `reject_future` (BitMind yes, spec no), `require_uuid` (false for the Apex dialect, which also changes the message to `{sha256}.{ts}.{signed_for}`), `emit_secret_signatures` (default true when `signed_for` set; verifier ignores them). Replay protection is **not** in the spec beyond "receiver should store UUIDs for 5 s"; rtao should add a uuid store with the same `NonceStore` trait as btauth.

---

## 4. btlightning (QUIC) — what it is and whether rtao should integrate it

### 4.1 Status

- crates.io `btlightning` max version **0.2.15**, published 2026-08-05, MIT, repo `inference-labs-inc/btlightning` (crates.io API, fetched 2026-10-13); on-disk clone is at tag `v0.2.15` / commit `53c090b` (2026-08-05) but its `crates/btlightning/Cargo.toml:3` still says `version = "0.1.0"` — the published version is set at release time **[inferred]**. SN2 depends on `btlightning = "0.2.15"` with features `btwallet`, `subtensor` (`$UP/inference-labs-inc_subnet-2/Cargo.toml:47`).
- Dependencies: `quinn 0.11`, `rustls 0.23` (ring), `rcgen 0.13`, `tokio`, `rmp-serde`/`rmpv` (MessagePack), `sp-core 34`, optional `btwallet 4.1.0` and `subxt 0.50` (`crates/btlightning/Cargo.toml:15-33`, `:39-43`, `:49-54`). Python bindings via PyO3 (`crates/btlightning-py`), PyPI `btlightning`.

### 4.2 Transport

ALPN `btlightning` (`client.rs:511`); server generates a self-signed cert on every `start()` (SAN `localhost`), clients accept any cert (`AcceptAnyCertVerifier`) — TLS is encryption only, identity is the sr25519 handshake (`ARCHITECTURE.md:56-60`, `:123-127`). Defaults: idle timeout 150 s, keep-alive 30 s (client `client.rs:72-73`; table `ARCHITECTURE.md:113-121`), server max 128 bidi streams, 0 uni streams; optional QUIC Retry address validation (default on) and a source-IP allowlist applied at `quinn::Incoming` with `ignore()` (`ARCHITECTURE.md:97-105`).

### 4.3 Framing

`[msg_type: u8][payload_len: u32 BE][payload ≤ 64 MiB]`, payload MessagePack with **named map keys** (custom `NamedSerializer`, because `rmp-serde` defaults to positional arrays) (`ARCHITECTURE.md:31-39`, `:246-252`; `types.rs:7`, `:654-729`). Message types (`types.rs:599-606`): `0x01 HandshakeRequest`, `0x02 HandshakeResponse`, `0x03 SynapsePacket`, `0x04 SynapseResponse`, `0x05 StreamChunk`, `0x06 StreamEnd`.

Structs (`types.rs`): `HandshakeRequest { validator_hotkey, timestamp: u64 (s), nonce: String, signature: String }` (538-547); `HandshakeResponse { miner_hotkey, timestamp, signature, accepted, connection_id, cert_fingerprint: Option<String> }` (551-565); `SynapsePacket { synapse_type, data: HashMap<String, rmpv::Value>, timestamp }` (569-573); `SynapseResponse { success, data, timestamp, error }` (577-582); `StreamChunk { data: Vec<u8> }` (585-588); `StreamEnd { success, error }` (591-594). `QuicAxonInfo { hotkey, ip, port, protocol }` (13-22).

### 4.4 Handshake (sr25519)

- Client: `cert_fp = blake2_256(peer DER cert)` (`client.rs:1364-1367`), `nonce` = 32 lowercase hex chars of 16 random bytes (`client.rs:1726-1730`), `timestamp` = unix seconds, message `"handshake:{validator_hotkey}:{timestamp}:{nonce}:{base64_std(cert_fp)}"` (`types.rs:100-110`), signature = **base64 (standard alphabet)** of the 64-byte sr25519 signature (`client.rs:1458-1476`).
- Server (`server/handshake.rs:154-229`): reject if `now > ts && now - ts >= max_signature_age_secs` (default 300, `server/config.rs:52`, capped at 3600, `:108-110`); reject if `ts > now + 60` (`MAX_FUTURE_CLOCK_SKEW_SECS`, `handshake.rs:18`); nonce must be exactly 32 hex chars (`:277-279`); verify with `sp_core::sr25519::Pair::verify` after ss58 decode and base64 decode (`signing.rs:5-38`); then nonce uniqueness in an `IndexMap<nonce, ts>` capped at `max_nonce_entries` 100 000 (`:211-226`); optional validator-permit gate (`:47-61`); per-IP handshake rate limit 30/min (`config.rs:59`). Response signed over `"handshake_response:{validator_hotkey}:{miner_hotkey}:{timestamp}:{nonce}:{cert_fp_b64}"` (`types.rs:112-123`, `handshake.rs:231-249`); client checks the returned `cert_fingerprint` equals its observed one and verifies the miner signature (`client.rs:1494`, `:1507`).

### 4.5 Per-request authentication: none

After the handshake, requests on the connection are authorised by mapping the QUIC remote `SocketAddr` → validator hotkey and checking the connection is marked verified (`server/dispatch.rs:230-266`). `SynapsePacket` carries no signature; identity is the connection. Consequences for rtao: no per-request replay protection is needed (QUIC streams are not replayable by a third party), but because the index is keyed by `SocketAddr` a NAT rebinding / QUIC path migration would orphan the mapping until the client re-handshakes **[inferred from the `addr_to_hotkey` design, `ARCHITECTURE.md:153-161`]**, and there is no per-request receiver binding beyond the handshake's cert fingerprint.

### 4.6 Request/response and streaming

Each exchange opens a fresh bidi stream; dispatch order streaming handlers → async handlers → sync handlers by `synapse_type`; unknown type → `success:false, "unrecognized synapse type"` (`ARCHITECTURE.md:170-188`). Streaming: handler writes `Vec<u8>` chunks through an `mpsc::channel(32)`, framed as `StreamChunk`, terminated by `StreamEnd` (`:190-210`). Typed wrappers deserialise via `rmpv::ext::from_value` (`:212-214`).

### 4.7 Chain integration and the `protocol` byte

`metagraph.rs` decodes `NeuronInfoLite` via SCALE (`:88-130`), formats IPv4 only, and treats **`protocol == 4` as "QUIC"** (`QUIC_PROTOCOL: u8 = 4`, `:23`; `quic_miners()` filters on it, `:318-333`; doc comment "4 = QUIC, 0 = HTTP", `:145`). That conflicts with every other SDK, where 4 is the unconditional default (legacy `axon.py:422`, fiber `post_ip_to_chain.py:69`, v11 `intents/serving.py:47`, Targon `serve.go:30` comment says `// HTTP`). SN2 publishes `serve_axon(..., protocol=4)` for its QUIC port (`$UP/inference-labs-inc_subnet-2/crates/sn2-miner/src/main.rs:171`; `crates/sn2-chain/src/registration.rs:22-90`, IPv6 rejected `:40-42`).

### 4.8 SN2's actual usage

Validator → miner is QUIC only: `MinerClient` wraps `LightningClient`, builds `QuicAxonInfo::new(hotkey, ip, port, 4)`, logs `transport = "quic"` (`crates/sn2-validator/src/miner_client.rs:6`, `:133-175`). Miner runs `LightningServer` with handler timeout, validator-permit gating, source allowlist and typed async handlers (`crates/sn2-miner/src/lightning_server.rs:12-90`). The README says "communicating over both HTTP and QUIC" and "HTTP (axum)" (`README.md:18`, `:301`), but at clone `96d03c4` (2026-09-08) no `axum::`/`Router::new`/`.route(` call exists under `crates/` (grep) — the HTTP side is gone or external **[inferred]**.

### 4.9 Verdict for rtao

btlightning is a single-vendor transport with a stable-looking but 0.x wire format, MessagePack payloads, no per-request signatures, and a `protocol`-byte convention that collides with the ecosystem default. Recommendation: **do not** make it part of the core; offer `rtao-transport-quic` as an optional feature-gated crate that depends on `btlightning` (keeps wire compatibility with SN2 for free, including the Python bindings for interop tests) behind an rtao `Transport` trait, rather than re-implementing the framing. Revisit if Rao/Opentensor standardises a QUIC profile for btauth.

---

## 5. Endpoint publication and discovery (chain side, shared by all protocols)

- Extrinsic `SubtensorModule.serve_axon(netuid: NetUid(u16), version: u32, ip: u128, port: u16, ip_type: u8, protocol: u8, placeholder1: u8, placeholder2: u8)`, call index 4, `Pays::No`; `serve_axon_tls` adds `certificate: Vec<u8>`, call index 40 (`$UP/subtensor/docs/tx/serve-axon.mdx:77-103`, `serve-axon-tls.mdx:80-107`); shapes confirmed in finney metadata V16 spec 477 via `subxt explore --file=$META/finney.scale pallet SubtensorModule calls serve_axon`. Signed by the **hotkey**, which must be registered on the subnet (`serve-axon.mdx:8-18`; `serving.rs:265-269`).
- Validation (`$UP/subtensor/pallets/subtensor/src/subnets/serving.rs:42-104`, `:214-297`): subnet must exist; `ip_type ∈ {4,6}`; IPv4 must be `< u32::MAX` and not `0x7f000001`; IPv6 not `u128::MAX` and not `::1`; `ip = 0` allowed for axons ("for testing"); `port != 0` (`InvalidPort`); rate limit `current_block - prev.block >= ServingRateLimit[netuid]` unless limit 0 or first serve (`:165-173`), runtime default **50 blocks** (`$UP/subtensor/runtime/src/lib.rs:943`, `:1025`; storage `ServingRateLimit` `lib.rs:2507-2508`). Private ranges are not excluded (`:218` TODO). Errors: `InvalidIpType`, `InvalidIpAddress`, `ServingRateLimitExceeded`, `InvalidPort`, `HotKeyNotRegisteredInNetwork`, `SubnetNotExists` (indices 11-13 listed in `docs/concepts/transactions.mdx:121-123`).
- Storage `Axons: map (netuid, hotkey) -> AxonInfo { block: u64, version: u32, ip: u128, port: u16, ip_type: u8, protocol: u8, placeholder1: u8, placeholder2: u8 }` (`lib.rs:308-325`; `subxt explore ... storage Axons`). `NeuronCertificates: map (netuid, hotkey) -> NeuronCertificate { public_key: BoundedVec<u8, 64>, algorithm: u8 }`, built from `[algorithm_byte] ++ public_key` with total length ≤ 65 (`lib.rs:332-355`); the chain only publishes the key — no chain-side TLS (`serve-axon-tls.mdx:8-14`). Silent failure: an undecodable certificate is simply not stored while the axon update proceeds (`serving.rs:72-76`).
- `protocol` semantics: pallet doc says "UDP:1 or TCP:0" (`serving.rs:20`), v11 docs say "subnet-defined" (`serve-axon.mdx:27`), every SDK writes 4, btlightning reads 4 as QUIC (§4.7). rtao should expose it as an opaque `u8` defaulting to 4.
- Reset: `ResetAxon` republishes `version 0, ip 0, port 1, ip_type 4, protocol 4` (`$UP/subtensor/sdk/python/bittensor/intents/serving.py:175-186`; `docs/tx/reset-axon.mdx:8-12`).
- Discovery: `SubnetInfoRuntimeApi.get_metagraph` returns `hotkeys: Vec<AccountId32>` and `axons: Vec<AxonInfo>` aligned by uid; `NeuronInfoRuntimeApi.get_neurons_lite` embeds `axon_info` (both via `subxt explore ... api`). v11 Python renders `MetagraphNeuron.axon` as `"ip:port"` (IPv6 as `"[v6]:port"`) or `None` when `ip == 0` (`$UP/subtensor/sdk/python/bittensor/metagraph.py:154`, `:563-573`); legacy used `is_serving = ip != "0.0.0.0"` (`axon_info.py:46-48`). IP encoding: `int(ipaddress.ip_address(ip))` big-endian integer for both v4 and v6, `ip_type = address.version` (`intents/serving.py:50-63`); v11 defaults `protocol=4`, `version=1` (`:46-48`).
- Operational guidance from the docs: `serve_axon` "does not start a server"; re-publishing is rate-limited; move machines by publishing the new address first; "All miner traffic comes through validators … Requests transit validators and miners with no confidentiality guarantee in either direction" (`$UP/subtensor/docs/guides/mining/index.mdx:114-135`); validators publish an axon only "if your subnet's protocol requires validators to be reachable" (`validating.mdx:262-263`). fiber's local-dev hack publishes `0.0.0.1` and rewrites it to `localhost`/`host.docker.internal` client-side (`$UP/fiber/fiber/validator/client.py:46-52`).

---

## 6. Survey of prominent subnets (repo facts via `gh api` on 2026-10-13; protocol column from files read, marked otherwise)

| SN | Repo (default branch) | Lang | SDK / chain deps | Neuron protocol | Last push |
| --- | --- | --- | --- | --- | --- |
| 1 Apex | `macrocosm-os/apex` (renamed from `prompting`; on-disk clone v4.4.12) | Python | `bittensor==10.4.0` (`src/validator/pyproject.toml:10`), `bittensor-wallet>=4.0.0` (`shared/common/pyproject.toml:16`) | Epistula **variant** (no uuid; `X-Spec-Version`, `X-Request-Id`) for miner/validator → central orchestrator (`shared/common/src/common/utils/epistula.py`); no validator→miner axon traffic **[inferred from code layout]** | 2026-10-02 |
| 2 Omron | `inference-labs-inc/subnet-2` (main; clone 96d03c4) | Rust | `btwallet 4.1.0`, `subxt 0.50`, `btlightning 0.2.15` (`Cargo.toml:44-47`) | **QUIC via btlightning** (handshake-authenticated connection, msgpack frames); `serve_axon(protocol=4)` | 2026-09-30 |
| 3 Templar | `tplr-ai/templar` (main; README points to `one-covenant/templar`) | Python | `bittensor>=9.10.1`, `bt-decode` (`pyproject.toml:16-17`) | No neuron↔neuron RPC: gradients shared via object storage; chain for weights (README lines 44-45, 69-71) **[inferred]** | 2026-10-04 |
| 4 Targon | `manifold-inc/targon` (main) | **Go** | `manifold-inc/manifold-sdk v0.0.1`, `subtrahend-labs/gobt` (boilerplate), `go-schnorrkel` (`go.mod`) | **Epistula v2 (gobt dialect)**: miner `GET /cvm` verifies `Epistula-*` headers (`cmd/miner/main.go:117-131`); validator pulls CVM/TDX attestations over HTTP (`internal/validator/callbacks/get_nodes.go:20-37`); `serve_axon` protocol 4 "HTTP" (`internal/miner/setup/serve.go:30`) | 2026-09-25 |
| 8 Vanta (ex-PTN) | `taoshidev/vanta-network` (development; redirect from `proprietary-trading-network`) | Python | `bittensor == 10.3.0` (`requirements.txt:3`) | **Legacy Synapse**: `SendSignal`, `GetPositions`, `ValidatorCheckpoint` with `required_hash_fields` (`template/protocol.py:44-83`) | 2026-10-12 |
| 9 Pretraining | `macrocosm-os/pretraining` (main) | Python | `bittensor==9.0.0` (`requirements.txt:3`) | No neuron RPC: HF model uploads + chain commitments (README 26-27); repo stale — IOTA (`macrocosm-os/iota`, pushed 2026-10-12, orchestrator-mediated, has `epistula.py`) appears to be the live SN9 code **[inferred]** | 2025-05-28 |
| 13 Data Universe | `macrocosm-os/data-universe` (main) | Python | `bittensor==10.3.0` (`requirements.txt:4`) | **Legacy Synapse**: `GetMinerIndex`, `GetDataEntityBucket`, `GetContentsByBuckets`, `OnDemandRequest` (`common/protocol.py:15-99`); has `common/old_protocol.py` | 2026-10-09 |
| 19 Nineteen | `rayonlabs/nineteen` → **404** (also `namoray/nineteen` 404) | — | historically fiber (fiber is authored by rayonlabs/namoray, `$UP/fiber/pyproject.toml:13`) | **unverified** — public repo not found | — |
| 21 Omega | `omegalabsinc/omegalabs-bittensor-subnet` (main) | Python | `bittensor[torch]==9.10.1`, `substrate-interface==1.7.11` (`requirements.txt:1-30`) | **Legacy Synapse**: `Videos`, `Audios` (`omega/protocol.py:48,114`) | 2025-12-17 |
| 27 NI Compute | `neuralinternet/compute-subnet` → **404**; only `neuralinternet/SN27-opencompute` (Streamlit dashboard, no bittensor dep) found | Python | — | **unverified** (dashboard repo only) | 2025-12-17 |
| 34 BitMind | `BitMind-AI/bitmind-subnet` (main) | Python | `bittensor==10.4.0`, `async-substrate-interface==2.2.1` (`pyproject.toml:16,51`) | **Epistula v2** (spec dialect + future-skew check, 15 s) (`gas/protocol/epistula.py`) | 2026-09-28 |
| 39 Basilica | `one-covenant/basilica` (main) | **Rust** | `bittensor-rs` git fork (`Cargo.toml:219`), `subxt 0.44` (`:192-194`), `sp-core 34` (`:107`) | **Custom gRPC (tonic 0.10)** miner→validator bids signed over `"{miner_hotkey}|{timestamp}"` etc. (`crates/basilica-protocol/proto/miner_discovery.proto:42-63`, `crates/basilica-validator/src/grpc/registration.rs:56-119`), validator→miner over SSH (README 33, 50-51), axum REST for external API | 2026-10-12 |
| 56 Gradients | `rayonlabs/G.O.D` (main) | Python | `fiber @ git+https://github.com/besimray/fiber.git@v2.7.0` (`pyproject.toml:28`) | **fiber** signed headers (`validator-hotkey`/`miner-hotkey`/`nonce`/`signature`) on fiber miner server and validator client (`miner/endpoints/training_repo.py`, `validator/endpoints/nodes.py` import fiber) | 2026-10-09 |
| 62 Ridges | `ridgesai/ridges` (main) | Python | `bittensor==10.5.0`, `async-substrate-interface==2.2.1` (`pyproject.toml:11-13`) | No axon traffic: miners upload `agent.py` to the Ridges platform, validators pull and run it (README lines 4, 128-143) | 2026-10-09 |
| 64 Chutes | `chutesai/chutes-miner` (main; `chutesai/chutes` 2026-07-23) | Python | `substrateinterface.Keypair` directly (`src/chutes-common/chutes_common/auth.py:11`) | **Custom**: `X-Chutes-{Validator,Hotkey,Miner,Nonce,Signature}` (`constants.py:1-5`); nonce = unix seconds, 30 s window (`auth.py:51`); message `"{hotkey}:{nonce}:{sha256(body)}"` or `"{hotkey}:{nonce}:{purpose}"` (`:73-90`), hex signature without `0x` (`:131`) | 2026-10-10 |

Supporting on-disk facts: fiber 2.5.0 (`$UP/fiber/pyproject.toml:7`, last commit 2025-12-20) — plain mode signs `"{nonce}:{miner_hotkey}"` (GET) or `"{nonce}:{miner_hotkey}:{sha256(body)}"` (`fiber/utils.py:14-17`; `chain/signatures.py:10-27`), nonce `"{time_ns}_{10 random alnum}"` (`validator/generate_nonce.py:6-7`), window ±120 s and single-use with 120 s TTL (`miner/security/nonce_management.py:9-55`; `miner/core/miner_constants.py:3`), 401 for nonce/signature/miner-hotkey mismatch and 403 for not-in-metagraph/low-stake (`miner/dependencies.py:17-101`); encrypted mode adds `GET /public-encryption-key` (RSA-2048 PEM) and `POST /exchange-symmetric-key` (RSA-OAEP-SHA256 of a 32-byte key, header `symmetric-key-uuid`, message `"{nonce}:{miner_hotkey}:{symmetric_key_uuid}"`), then Fernet-encrypted `application/octet-stream` bodies carrying a `nonce` field, keys kept 5 h (`encrypted/validator/handshake.py:19-81`, `encrypted/utils.py:14-15`, `encrypted/miner/security/encryption.py:24-74`, `encrypted/miner/core/models/encryption.py:13-15`). Note the on-disk fiber is the `chutesai/fiber` "trust-removal" line; SN56 pins a different fork (`besimray/fiber v2.7.0`), so treat fiber as "a family", not one wire format.

Takeaways: of 13 verifiable subnets, 3 run legacy Synapse (all pinned to `bittensor` 9.10–10.3), 3 run an Epistula dialect, 1 fiber, 1 QUIC/btlightning, 2 custom signed-header/gRPC schemes, 3 have no validator→miner RPC at all, and **none has adopted btauth/1 yet** (no `X-Bittensor-*`/`http_auth` hits in any tree or dependency manifest).

---

## 7. Other Rust ecosystem crates relevant to neuron networking

- `$UP/CortexLM_bittensor-rs` (2026-04-22): `bittensor-synapse` / `bittensor-axon` (axum) / `bittensor-dendrite` (reqwest) — partial legacy port, see §2.9 caveats.
- `$UP/womboai_rusttensor` (2024-11-22) and `$UP/timehash13_subtensor-api-rs` (crabtensor, 2026-03-30): only `serve_axon_payload(netuid, SocketAddr, AxonProtocol)` + `AxonInfo` decoding (`src/axon.rs:74-86` / `:70-86`); no neuron transport.
- `$UP/one-covenant_bittensor-rs` (2026-07-20): metagraph/`NeuronDiscovery` axon extraction only (`bittensor-rs/src/discovery.rs:128-139`, `:239`).
- `$UP/ErgodicLabs_btt` (2026-07-09): CLI `serve_axon`/reset via subxt dynamic tx (`src/commands/axon.rs:33-140`).
- `$UP/inference-labs-inc_btlightning` (§4) is the only Rust crate with a working validator↔miner transport.

---

## 8. Recommendations

### 8.1 Native (must-have): btauth/1

Crates: `rtao-auth` (pure, `no_std`-friendly except time; `build_payload`/`sign`/`verify`/`NonceStore`), `rtao-http` (tower layer + axum extractor + reqwest signing helper; feature `redis` for a shared nonce store). Ship §1.8 vectors as tests, mirror every behaviour in the Python test file, and keep `verify` byte-identical to `http_auth.py` including message strings (they are the de-facto contract). Expose `require_receiver`, `max_age`, `allowed_skew`, store retention invariant (fail at construction, not per request), and a `Policy` hook (metagraph-based 403) separate from auth.

### 8.2 Optional compat crates

- `rtao-compat-synapse`: legacy `bt_header_*` codec + sha3 body hash + nonce window + status semantics (§2.9). Needed to talk to SN8/SN13/SN21-style miners/validators during their migration; keep it out of the default feature set and document the schema-dependence.
- `rtao-compat-epistula`: spec dialect with knobs (§3.4); also covers fiber-style and Chutes-style signed headers cheaply if designed as a generic "signed-string-header" scheme (`message_template`, `nonce` type, `window`, header names), since all of them are `sign(sr25519, "<fields joined by sep>")`.
- `rtao-transport-quic`: thin adapter over `btlightning` (§4.9), feature-gated.

### 8.3 Interop test strategy against Python

1. **Golden vectors first**: unit tests on §1.8 (sr25519 verify of Python-produced signatures captured as fixtures; ed25519 pinned bytes), plus the Python negative cases.
2. **Live cross-language harness** (CI job, Linux): a `uv`-managed venv with `bittensor==11.3.x` running the FastAPI recipe and the httpx `HotkeyAuth` recipe verbatim from `signed-requests.mdx`; matrix = {rtao client → Python server, Python client → rtao server} × {sr25519, ed25519} × {GET empty body, POST JSON, percent-encoded path+query, unbound receiver with/without opt-out, stale/future nonce, replay, tampered body}. Assert status codes and `detail` strings.
3. **Legacy**: a second venv with `bittensor==10.5.0` (archived wheels) running a minimal `Axon`/`Dendrite` with a test Synapse that has `required_hash_fields` of mixed types; test both directions including the `Nonce is too old` window, monotonic nonce per `hotkey:uuid`, and streaming (`text/event-stream` raw chunks).
4. **Epistula**: use BitMind's `gas/protocol/epistula.py` (spec-conformant, includes future-skew) and `gobt` (Go) as oracles; include the Apex no-uuid dialect as a parametrised case.
5. **QUIC**: `pip install btlightning` and drive the Python `Lightning` client/server against rtao's adapter (same crate underneath, so this mainly guards the rtao API surface).
6. **Property/fuzz**: canonicalisation of request targets (percent-encoding preserved), header case, nonce boundaries at exactly `max_age`/`allowed_skew`, store retention edge, multi-process store via a Redis container.
7. **Drift watch**: a CI job that diffs `sdk/python/bittensor/http_auth.py` and `docs/guides/signed-requests.mdx` against pinned hashes and fails when they change, since the spec explicitly promises a version bump for incompatible edits.

---

## 9. Open questions

- Will Rao publish an official HTTP status mapping and a streaming convention for btauth/1 (currently "SSE / websockets in your framework", `migration.mdx:470`)? rtao should pick axum SSE + a documented body-hash rule for streamed *requests* (unsupported) now and track upstream.
- Is the `<Bytes>` wrapper tolerance going to stay in the Python verifier? (Affects whether rtao should accept it for parity.)
- Does any subnet intend to adopt btauth/1 in 2026? None found; migration pressure may come from `bittensor` 11 removing `Axon`/`Dendrite`.
- Will the `protocol` byte be standardised (QUIC=4 collides with the HTTP default)?
- Are the drand-anchored freshness headers (`X-Bittensor-Drand-*`) going to ship, and should rtao reserve API surface for them?
