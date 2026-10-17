"""Epistula v2 interop between rtao-compat-epistula and the Python references.

Run with the M10 interop interpreter in isolated mode (``python -I``). The
upstream reference modules are loaded by path from ``--upstream``:

* BitMind ``gas/protocol/epistula.py`` (SN34) at
  5fd96727b8f41d8376348f6dd8a11cd5069bce45, the full FastAPI verifier;
* Apex ``epistula.py`` (SN1) at bd71ceb070cd72b4bd80562e96884b66e86d908a,
  with ``common.settings`` stubbed to its ``__SPEC_VERSION__``;
* the epistula.sybil.com reference Python verifier, transcribed below with
  ``bittensor_wallet.Keypair`` in place of ``substrateinterface.Keypair``.

Subcommands:

``gen-vectors``  sign fixtures with the references, record every reference
                 verifier's verdict for each fixture and mutation, write JSON.
``verify-rust``  verify rtao-signed headers with every reference verifier.
``bm-server``    serve BitMind's ``get_verifier`` dependency over HTTP.
``bm-client``    drive an rtao server with BitMind's httpx header hook.
"""

import argparse
import asyncio
import importlib.util
import json
import re
import sys
import time as real_time
import types
import uuid as real_uuid
from hashlib import sha256
from math import ceil
from pathlib import Path

import numpy as np
from bittensor_wallet import Keypair

FIXED_TIME = 1700000000.123
FIXED_TS = 1700000000123
FIXED_UUID = "8b0e5f5e-3f6c-4a7e-9c1d-2b9a4f6d7e10"

ALICE = Keypair.create_from_uri("//Alice")
BOB = Keypair.create_from_uri("//Bob")
CHARLIE = Keypair.create_from_uri("//Charlie")
DAVE = Keypair.create_from_uri("//Dave")
BOB_SS58_PREFIX0 = "14E5nqKAp3oAJcmzgZhUD2RcptBeUBScxKHgJKU4HPNcKVf3"

BM_PATH = "bitmind-subnet/gas/protocol/epistula.py"
APEX_PATH = "apex/epistula.py"
APEX_SETTINGS_PATH = "apex/settings.py"


class FakeTime:
    """Stands in for the ``time`` module inside a reference module."""

    def __init__(self, seconds):
        self.seconds = seconds

    def time(self):
        return self.seconds


class FakeUuidModule:
    def __init__(self, value):
        self.value = value

    def uuid4(self):
        return real_uuid.UUID(self.value)


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_references(upstream):
    upstream = Path(upstream)
    bm = load_module("bm_epistula", upstream / BM_PATH)
    if not getattr(load_references, "verbose", False):
        bm.bt.logging.off()
    settings_text = (upstream / APEX_SETTINGS_PATH).read_text()
    spec_version = int(re.search(r"^__SPEC_VERSION__ = (\d+)$", settings_text, re.M).group(1))
    common = types.ModuleType("common")
    settings = types.ModuleType("common.settings")
    settings.__SPEC_VERSION__ = spec_version
    common.settings = settings
    sys.modules["common"] = common
    sys.modules["common.settings"] = settings
    apex = load_module("apex_epistula", upstream / APEX_PATH)
    return bm, apex


# Transcribed from the epistula.sybil.com reference implementation
# (https://epistula.sybil.com/implementation, Python verify_signature_v2,
# fetched 2026-10-17; .local/m10/upstream/epistula-spec/implementation-code.txt
# lines 127-153). The site states no licence; this CI-only test oracle is not
# shipped. Changes: bittensor_wallet.Keypair replaces
# substrateinterface.Keypair as the parity reference for signature parsing.
# It is a transcription, not the published code.
def spec_verify_signature_v2(signature, body, timestamp, uuid, signed_for, signed_by, now):
    if not isinstance(signature, str):
        return "Invalid Signature"
    timestamp = int(timestamp)
    if not isinstance(timestamp, int):
        return "Invalid Timestamp"
    if not isinstance(signed_by, str):
        return "Invalid Sender key"
    if not isinstance(signed_for, str):
        return "Invalid receiver key"
    if not isinstance(uuid, str):
        return "Invalid uuid"
    if not isinstance(body, bytes):
        return "Body is not of type bytes"
    allowed_delta_ms = 8000
    keypair = Keypair(ss58_address=signed_by)
    if timestamp + allowed_delta_ms < now:
        return "Request is too stale"
    message = f"{sha256(body).hexdigest()}.{uuid}.{timestamp}.{signed_for}"
    verified = keypair.verify(message, signature)
    if not verified:
        return "Signature Mismatch"
    return None


class Headers:
    """Case-insensitive first-value lookup, like Starlette's ``Headers.get``."""

    def __init__(self, pairs):
        self.pairs = [(name.lower(), value) for name, value in pairs]

    def get(self, name, default=None):
        name = name.lower()
        for key, value in self.pairs:
            if key == name:
                return value
        return default


class FakeRequest:
    def __init__(self, pairs, body):
        self.headers = Headers(pairs)
        self.client = None
        self._body = body

    async def body(self):
        return self._body


def fake_wallet(hotkey):
    return types.SimpleNamespace(hotkey=types.SimpleNamespace(ss58_address=hotkey.ss58_address))


def fake_metagraph():
    # Alice, Bob and Charlie hold validator stake; Dave is below BitMind's minimum.
    return types.SimpleNamespace(
        hotkeys=[ALICE.ss58_address, BOB.ss58_address, CHARLIE.ss58_address, DAVE.ss58_address],
        S=np.array([25000.0, 25000.0, 25000.0, 100.0], dtype=np.float32),
    )


def verdict(error):
    return {"status": 200, "detail": None} if error is None else error


def run_bitmind(bm, pairs, body, now_ms):
    """BitMind's complete dependency (version, receiver, metagraph, signature)."""
    from fastapi import HTTPException

    saved = bm.time
    bm.time = FakeTime(now_ms / 1000.0)
    try:
        asyncio.run(
            bm.determine_epistula_version_and_verify(
                FakeRequest(pairs, body), fake_wallet(BOB), fake_metagraph(), False
            )
        )
        return verdict(None)
    except HTTPException as error:
        return {"status": error.status_code, "detail": error.detail}
    except Exception as error:  # FastAPI would answer 500.
        return {"status": 500, "detail": f"{type(error).__name__}: {error}"}
    finally:
        bm.time = saved


def run_spec(pairs, body, now_ms):
    headers = Headers(pairs)
    try:
        error = spec_verify_signature_v2(
            headers.get("Epistula-Request-Signature"),
            body,
            headers.get("Epistula-Timestamp"),
            headers.get("Epistula-Uuid"),
            headers.get("Epistula-Signed-For"),
            headers.get("Epistula-Signed-By"),
            now_ms,
        )
    except Exception as error:
        return {"status": 500, "detail": f"{type(error).__name__}: {error}"}
    return verdict(None) if error is None else {"status": 400, "detail": error}


def run_apex(apex, pairs, body, now_ms, timeout_ms=15000):
    headers = Headers(pairs)
    fields = {}
    for name in [
        "Epistula-Timestamp",
        "Epistula-Signed-By",
        "Epistula-Request-Signature",
        "X-Request-Id",
        "X-Spec-Version",
    ]:
        value = headers.get(name)
        if value is not None:
            fields[name] = value
    try:
        model = apex.EpistulaHeaders(**fields)
        model.verify_signature_v2(body, now_ms, timeout_ms)
    except ValueError as error:
        message = str(error)
        if message in ("Signature Mismatch", "Request is too stale", "Invalid Timestamp"):
            return {"status": 400, "detail": message}
        return {"status": 500, "detail": f"{type(error).__name__}: {error}"}
    except Exception as error:
        return {"status": 500, "detail": f"{type(error).__name__}: {error}"}
    return verdict(None)


def bm_generate(bm, keypair, body, signed_for, seconds=FIXED_TIME, uuid=FIXED_UUID):
    saved = (bm.time, bm.uuid4)
    bm.time = FakeTime(seconds)
    bm.uuid4 = lambda: real_uuid.UUID(uuid)
    try:
        return bm.generate_header(keypair, body, signed_for)
    finally:
        bm.time, bm.uuid4 = saved


def apex_generate(apex, keypair, body, signed_for):
    saved = (apex.time, apex.uuid)
    apex.time = FakeTime(FIXED_TIME)
    apex.uuid = FakeUuidModule(FIXED_UUID)
    try:
        return apex.generate_header(keypair, body, signed_for)
    finally:
        apex.time, apex.uuid = saved


def replace(pairs, name, value):
    """Replace (or with ``None`` remove) a header, keeping order."""
    out = [(key, val) for key, val in pairs if key.lower() != name.lower()]
    if value is not None:
        out.append((name, value))
    return out


def cmd_gen_vectors(args):
    bm, apex = load_references(args.upstream)
    bodies = {
        "json": b'{"key": "value"}',
        "empty": b"",
        "hello": b"Hello World",
        "compact": b'{"hello":"world"}',
    }
    bases = []
    for name, body in bodies.items():
        headers = bm_generate(bm, ALICE, body, BOB.ss58_address)
        assert headers["Epistula-Timestamp"] == str(FIXED_TS), headers
        bases.append(("bm-" + name + "-bound", "bitmind generate_header", body, list(headers.items())))
    unbound = bm_generate(bm, ALICE, bodies["json"], None)
    bases.append(("bm-json-unbound", "bitmind generate_header", bodies["json"], list(unbound.items())))

    # A Polkadot-JS signRaw-style signature over <Bytes>M1</Bytes>.
    m1 = f"{sha256(bodies['json']).hexdigest()}.{FIXED_UUID}.{FIXED_TS}.{BOB.ss58_address}"
    wrapped = bm_generate(bm, ALICE, bodies["json"], BOB.ss58_address)
    wrapped["Epistula-Request-Signature"] = "0x" + ALICE.sign(b"<Bytes>" + m1.encode() + b"</Bytes>").hex()
    bases.append(("bm-json-wrapped", "btwallet sign(<Bytes>M1</Bytes>)", bodies["json"], list(wrapped.items())))

    apex_body = apex.create_message_body({"b": 1, "a": "x"})
    assert apex_body == b'{"a": "x", "b": 1}', apex_body
    apex_empty = apex.create_message_body({})
    for name, body, receiver in [
        ("apex-unbound", apex_body, None),
        ("apex-bound", apex_body, BOB.ss58_address),
        ("apex-empty", apex_empty, None),
    ]:
        headers = apex_generate(apex, ALICE, body, receiver)
        assert headers["Epistula-Timestamp"] == str(FIXED_TS), headers
        bases.append((name, "apex generate_header", body, list(headers.items())))

    vectors = []

    def record(vector_id, base, generator, body, pairs, now_ms):
        vectors.append(
            {
                "id": vector_id,
                "base": base,
                "generator": generator,
                "body_hex": body.hex(),
                "headers": [[k, v] for k, v in pairs],
                "now_ms": now_ms,
                "bitmind": run_bitmind(bm, pairs, body, now_ms),
                "spec": run_spec(pairs, body, now_ms),
                "apex": run_apex(apex, pairs, body, now_ms),
            }
        )

    for vector_id, generator, body, pairs in bases:
        record(vector_id, vector_id, generator, body, pairs, FIXED_TS)

    # Mutations of the M1 fixture, as in spec-epistula.md section 11.3.
    _, generator, body, m1_pairs = bases[0]
    headers = dict(m1_pairs)
    signature = headers["Epistula-Request-Signature"]
    mutations = [
        ("m1-body-trailing-space", body + b" ", m1_pairs, FIXED_TS),
        ("m1-for-bob-prefix0", body, replace(m1_pairs, "Epistula-Signed-For", BOB_SS58_PREFIX0), FIXED_TS),
        ("m1-for-charlie", body, replace(m1_pairs, "Epistula-Signed-For", CHARLIE.ss58_address), FIXED_TS),
        ("m1-for-absent", body, replace(m1_pairs, "Epistula-Signed-For", None), FIXED_TS),
        ("m1-for-empty", body, replace(m1_pairs, "Epistula-Signed-For", ""), FIXED_TS),
        ("m1-version-1", body, replace(m1_pairs, "Epistula-Version", "1"), FIXED_TS),
        ("m1-version-absent", body, replace(m1_pairs, "Epistula-Version", None), FIXED_TS),
        ("m1-ts-plus", body, replace(m1_pairs, "Epistula-Timestamp", "+" + str(FIXED_TS)), FIXED_TS),
        ("m1-ts-leading-zero", body, replace(m1_pairs, "Epistula-Timestamp", "0" + str(FIXED_TS)), FIXED_TS),
        ("m1-ts-underscore", body, replace(m1_pairs, "Epistula-Timestamp", "1_700_000_000_123"), FIXED_TS),
        ("m1-ts-float", body, replace(m1_pairs, "Epistula-Timestamp", str(FIXED_TS) + ".0"), FIXED_TS),
        ("m1-ts-absent", body, replace(m1_pairs, "Epistula-Timestamp", None), FIXED_TS),
        ("m1-sig-no-prefix", body, replace(m1_pairs, "Epistula-Request-Signature", signature[2:]), FIXED_TS),
        ("m1-sig-double-prefix", body, replace(m1_pairs, "Epistula-Request-Signature", "0x" + signature), FIXED_TS),
        ("m1-sig-upper", body, replace(m1_pairs, "Epistula-Request-Signature", "0x" + signature[2:].upper()), FIXED_TS),
        ("m1-sig-short", body, replace(m1_pairs, "Epistula-Request-Signature", signature[:-2]), FIXED_TS),
        ("m1-sig-absent", body, replace(m1_pairs, "Epistula-Request-Signature", None), FIXED_TS),
        ("m1-by-bob", body, replace(m1_pairs, "Epistula-Signed-By", BOB.ss58_address), FIXED_TS),
        ("m1-uuid-other", body, replace(m1_pairs, "Epistula-Uuid", str(real_uuid.UUID(int=1))), FIXED_TS),
        ("m1-uuid-absent", body, replace(m1_pairs, "Epistula-Uuid", None), FIXED_TS),
        ("m1-now-age-equal", body, m1_pairs, FIXED_TS + 15000),
        ("m1-now-age-tie", body, m1_pairs, FIXED_TS + 15250),
        ("m1-now-stale", body, m1_pairs, FIXED_TS + 15001),
        ("m1-now-stale-20s", body, m1_pairs, FIXED_TS + 20001),
        ("m1-now-spec-age-edge", body, m1_pairs, FIXED_TS + 8000),
        ("m1-now-spec-stale", body, m1_pairs, FIXED_TS + 8001),
        ("m1-now-future-equal", body, m1_pairs, FIXED_TS - 15000),
        ("m1-now-future", body, m1_pairs, FIXED_TS - 15001),
        ("m1-now-future-2001", body, m1_pairs, FIXED_TS - 2001),
    ]
    for vector_id, mutated_body, pairs, now_ms in mutations:
        record(vector_id, bases[0][0], generator, mutated_body, pairs, now_ms)

    # Self-checks: the references accept their own compliant output.
    by_id = {vector["id"]: vector for vector in vectors}
    for vector_id in ["bm-json-bound", "bm-empty-bound", "bm-hello-bound", "bm-compact-bound", "bm-json-wrapped"]:
        assert by_id[vector_id]["bitmind"]["status"] == 200, by_id[vector_id]
        assert by_id[vector_id]["spec"]["status"] == 200, by_id[vector_id]
    for vector_id in ["apex-unbound", "apex-empty"]:
        assert by_id[vector_id]["apex"]["status"] == 200, by_id[vector_id]
    assert by_id["apex-bound"]["apex"]["detail"] == "Signature Mismatch"

    import bittensor_wallet
    from importlib.metadata import version

    out = {
        "generator": "ci/interop/epistula/epistula_interop.py gen-vectors",
        "python": sys.version.split()[0],
        "versions": {
            "bittensor": version("bittensor"),
            "bittensor-wallet": version("bittensor-wallet"),
            "fastapi": version("fastapi"),
            "pydantic": version("pydantic"),
        },
        "upstream": {
            "bitmind": "BitMind-AI/bitmind-subnet@5fd96727b8f41d8376348f6dd8a11cd5069bce45 " + BM_PATH.split("/", 1)[1],
            "apex": "macrocosm-os/apex@bd71ceb070cd72b4bd80562e96884b66e86d908a shared/common/src/common/utils/epistula.py",
            "spec": "epistula.sybil.com/implementation verify_signature_v2 (bittensor_wallet.Keypair)",
        },
        "me": BOB.ss58_address,
        "vectors": vectors,
    }
    del bittensor_wallet
    Path(args.out).write_text(json.dumps(out, indent=1) + "\n")
    print(f"wrote {len(vectors)} vectors to {args.out}")


def py_float_interval(ts):
    return ceil(ts / 1e4) * 1e4


# The harness's own secret-signature checker. No upstream receiver verifies
# secret signatures; the messages follow the Python/JS reference signers and
# gobt boilerplate/epistula.go, so this confirms rtao's reading of them.
def check_secret(pairs, sender, receiver, form):
    headers = Headers(pairs)
    ts = int(headers.get("Epistula-Timestamp"))
    keypair = Keypair(ss58_address=sender)
    for k, offset in enumerate([-1, 0, 1]):
        if form == "python":
            message = str(py_float_interval(ts) + offset) + "." + receiver
        elif form == "gobt":
            message = "%d.%s" % (int(ceil(float(ts) / 1e4)) + offset, sender)
        else:
            raise ValueError(form)
        signature = headers.get(f"Epistula-Secret-Signature-{k}")
        if signature is None or not keypair.verify(message, signature):
            return f"secret signature {k} does not verify over {message!r}"
    return None


def cmd_verify_rust(args):
    bm, apex = load_references(args.upstream)
    data = json.loads(Path(args.input).read_text())
    failures = 0
    for vector in data["vectors"]:
        pairs = [tuple(pair) for pair in vector["headers"]]
        body = bytes.fromhex(vector["body_hex"])
        now_ms = vector["now_ms"]
        results = {
            "bitmind": run_bitmind(bm, pairs, body, now_ms),
            "spec": run_spec(pairs, body, now_ms),
            "apex": run_apex(apex, pairs, body, now_ms),
        }
        if vector.get("secret"):
            error = check_secret(pairs, vector["sender"], vector.get("receiver"), vector["secret"])
            results["secret"] = verdict(None) if error is None else {"status": 400, "detail": error}
        for verifier, expected in vector["expect"].items():
            got = results[verifier]
            ok = got["status"] == expected["status"] and (
                expected.get("detail") is None or got["detail"] == expected["detail"]
            )
            failures += not ok
            print(f"{'PASS' if ok else 'FAIL'} {vector['id']} {verifier}: {got['status']} {got['detail']}")
    # Apex create_message_body: json.dumps(data, default=str, sort_keys=True).
    for case in data.get("canonical", []):
        expected = apex.create_message_body(json.loads(case["source"]))
        ok = expected.hex() == case["rust_hex"]
        failures += not ok
        print(f"{'PASS' if ok else 'FAIL'} canonical_json {expected[:60]!r}")
    print(f"verify-rust: {failures} failures")
    sys.exit(1 if failures else 0)


def cmd_bm_server(args):
    import uvicorn
    from fastapi import Depends, FastAPI, Request

    bm, _ = load_references(args.upstream)
    verifier = bm.get_verifier(fake_wallet(BOB), fake_metagraph())
    app = FastAPI()

    @app.post("/echo", dependencies=[Depends(verifier)])
    async def echo(request: Request):
        body = await request.body()
        return {"ok": True, "sha256": sha256(body).hexdigest()}

    @app.get("/health", dependencies=[Depends(verifier)])
    async def health():
        return {"ok": True}

    uvicorn.run(app, host="127.0.0.1", port=args.port, log_level="warning")


def cmd_bm_client(args):
    import httpx

    bm, apex = load_references(args.upstream)
    failures = 0

    def check(name, response, status, detail=None, prefix=False):
        nonlocal failures
        try:
            got = response.json().get("detail")
        except Exception:
            got = response.text
        ok = response.status_code == status and (
            detail is None or (got.startswith(detail) if prefix else got == detail)
        )
        failures += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}: {response.status_code} {got}")
        return response

    def signed(keypair, body, receiver, seconds=None, version=True, uuid=None):
        headers = bm_generate(
            bm, keypair, body, receiver, real_time.time() if seconds is None else seconds,
            uuid or str(real_uuid.uuid4()),
        )
        if not version:
            headers.pop("Epistula-Version")
        headers["Content-Type"] = "application/json"
        return headers

    bob = BOB.ss58_address
    body = json.dumps({"prompt": "hello", "n": 1}).encode()
    with httpx.Client(base_url=args.base, timeout=10) as client:
        # BitMind's own httpx event hook, exactly as its validators use it.
        async def run_hook_client():
            hook = bm.create_header_hook(ALICE, bob, None)
            async with httpx.AsyncClient(base_url=args.base, event_hooks={"request": [hook]}, timeout=10) as hooked:
                first = await hooked.post("/bitmind/echo", content=body)
                health = await hooked.get("/bitmind/health")
                rtao = await hooked.post("/rtao/echo", content=body)
                return first, health, rtao

        first, health, rtao = asyncio.run(run_hook_client())
        check("hook POST /bitmind/echo", first, 200)
        if first.status_code == 200:
            echoed = first.json()
            ok = echoed["signed_by"] == ALICE.ss58_address and echoed["sha256"] == sha256(body).hexdigest()
            failures += not ok
            print(f"{'PASS' if ok else 'FAIL'} echoed caller {echoed}")
        check("hook GET /bitmind/health (empty body)", health, 200)
        check("hook POST /rtao/echo (RTAO preset accepts BitMind senders)", rtao, 200)
        replay_headers = {
            key: value
            for key, value in first.request.headers.items()
            if key.lower() not in ("host", "content-length")
        }
        check(
            "replay identical request",
            client.post("/bitmind/echo", content=body, headers=replay_headers),
            400,
            "Request has already been processed",
        )
        headers = signed(ALICE, body, bob)
        check("tampered body", client.post("/bitmind/echo", content=body + b" ", headers=headers), 400, "Signature Mismatch")
        headers = signed(ALICE, body, CHARLIE.ss58_address)
        check(
            "signed for another receiver",
            client.post("/bitmind/echo", content=body, headers=headers),
            400,
            "Bad Request, message is not intended for self",
        )
        headers = signed(DAVE, body, bob)
        check("sender outside policy", client.post("/bitmind/echo", content=body, headers=headers), 401, "Signer not in metagraph")
        headers = signed(ALICE, body, bob, seconds=real_time.time() - 20)
        check(
            "stale request",
            client.post("/bitmind/echo", content=body, headers=headers),
            400,
            "Request is too stale: 20.",
            prefix=True,
        )
        headers = signed(ALICE, body, bob, seconds=real_time.time() + 5)
        check(
            "future request on RTAO preset (2 s bound)",
            client.post("/rtao/echo", content=body, headers=headers),
            400,
            "Request timestamp is in the future: ",
            prefix=True,
        )
        headers = signed(ALICE, body, bob, version=False)
        check("missing version", client.post("/bitmind/echo", content=body, headers=headers), 400, "Unknown Epistula version")
        # The epistula.sybil.com reference signer is BitMind's generate_header verbatim.
        headers = signed(ALICE, b"", bob)
        check("signed empty POST", client.post("/bitmind/echo", content=b"", headers=headers), 200)
        saved = (apex.time,)
        apex.time = real_time
        try:
            apex_headers = apex.generate_header(ALICE, body, None)
        finally:
            (apex.time,) = saved
        check("Apex headers to BitMind preset", client.post("/bitmind/echo", content=body, headers=apex_headers), 400, "Unknown Epistula version")
    print(f"bm-client: {failures} failures")
    sys.exit(1 if failures else 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--upstream", required=True, help="M10 upstream source root")
    sub = parser.add_subparsers(dest="command", required=True)
    gen = sub.add_parser("gen-vectors")
    gen.add_argument("--out", required=True)
    verify = sub.add_parser("verify-rust")
    verify.add_argument("--in", dest="input", required=True)
    server = sub.add_parser("bm-server")
    server.add_argument("--port", type=int, required=True)
    client = sub.add_parser("bm-client")
    client.add_argument("--base", required=True)
    args = parser.parse_args()
    {
        "gen-vectors": cmd_gen_vectors,
        "verify-rust": cmd_verify_rust,
        "bm-server": cmd_bm_server,
        "bm-client": cmd_bm_client,
    }[args.command](args)


if __name__ == "__main__":
    main()
