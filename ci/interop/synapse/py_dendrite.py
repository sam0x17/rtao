"""bittensor==10.5.0 dendrite scenario runner (spec M10-01 section 9.3).

Usage:
  py_dendrite.py <port> <python|rust> <harness_dir>
      Run the matrix against the axon on 127.0.0.1:<port>; `python` expects a
      bittensor axon (py_axon.py), `rust` the rtao interop_axon example. One
      JSON line per check; exit status 1 on any failure.
  py_dendrite.py --verify-s3 '<json from interop_dendrite --sign-s1>' <harness_dir>
      Vector S3/I18: verify a Rust-made signature with bittensor-wallet.
"""

import asyncio
import json
import re
import sys
import time
import uuid
import warnings

warnings.simplefilter("ignore")
sys.path.insert(0, sys.argv[-1])
import aiohttp  # noqa: E402
import bittensor as bt  # noqa: E402
import numpy as np  # noqa: E402
from bittensor_wallet import Keypair  # noqa: E402

import common  # noqa: E402

H1 = "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"
INTERNAL = re.compile(r"^Internal Server Error #[0-9a-f-]{36}$")
ROUTES = "['Synapse', 'Mixed', 'Req', 'LegacyHashed', 'Tens', 'Deny', 'Status404', 'Boom', 'Slow', 'NanResp', 'WhoAmI', 'Echo', 'NanReq']"

failures = 0
PEER = ""


def check(case: str, ok: bool, detail) -> None:
    global failures
    if not ok:
        failures += 1
    print(json.dumps({"id": case, "direction": f"python-dendrite->{PEER}-axon", "pass": bool(ok), "detail": str(detail)[:800]}), flush=True)


def resign(syn, nonce=None, fresh_uuid=False, keypair=common.ALICE):
    if nonce is not None:
        syn.dendrite.nonce = nonce
    if fresh_uuid:
        syn.dendrite.uuid = str(uuid.uuid1())
    message = f"{syn.dendrite.nonce}.{syn.dendrite.hotkey}.{syn.axon.hotkey}.{syn.dendrite.uuid}.{syn.body_hash}"
    syn.dendrite.signature = "0x" + keypair.sign(message).hex()
    return message


class Raw:
    def __init__(self, status, headers, body):
        self.status, self.headers, self.body = status, headers, body

    def json(self):
        try:
            return json.loads(self.body)
        except Exception:
            return None

    def message(self):
        data = self.json()
        return data.get("message") if isinstance(data, dict) else None


async def main(port: int) -> None:
    d = bt.Dendrite(wallet=common.ALICE)
    info = bt.AxonInfo(version=10005000, ip="127.0.0.1", ip_type=4, port=port,
                       hotkey=common.BOB.ss58_address, coldkey=common.BOB.ss58_address)
    url = f"http://127.0.0.1:{port}"
    python = PEER == "python"
    session = aiohttp.ClientSession()

    def prepare(syn, timeout=12.0):
        syn = d.preprocess_synapse_for_request(info, syn, timeout)
        headers = syn.to_headers()
        return syn, headers

    async def send(syn, headers, body=None, ctype="application/json", path=None):
        if body is None:
            body = json.dumps(syn.model_dump()).encode()
        h = dict(headers)
        if ctype is not None:
            h["Content-Type"] = ctype
        async with session.post(f"{url}/{path or syn.name}", headers=h, data=body,
                                skip_auto_headers=["Content-Type"]) as r:
            return Raw(r.status, r.headers, await r.read())

    # I1
    r = await d.call(info, bt.Synapse(), deserialize=False)
    check("I1", r.dendrite.status_code == 200 and r.axon.status_message == "Success", r.dendrite)
    syn, h = prepare(bt.Synapse())
    raw = await send(syn, h)
    ok = (raw.status == 200 and raw.headers.get("bt_header_axon_status_code") == "200"
          and raw.headers.get("bt_header_axon_status_message") == "Success"
          and raw.headers.get("name") == "Synapse" and raw.headers.get("computed_body_hash") == H1
          and raw.headers.get("content-type") == "application/json"
          and raw.headers.get("bt_header_dendrite_hotkey") == common.ALICE.ss58_address)
    check("I1.headers", ok, dict(raw.headers))

    # I2
    s_text = "it's \"q\" é \U0001F600 ​ \\"
    mixed = common.Mixed(i=7, f=1.5, s=s_text, l=["a", "b'c"], d={"x": [0.1, 1e-5, 1e16], "a": [2.0]})
    r = await d.call(info, mixed, deserialize=False)
    check("I2", r.dendrite.status_code == 200 and r.out == f"7|{s_text}|2" and list(r.d) == ["x", "a"],
          (r.dendrite.status_code, r.dendrite.status_message, r.out))

    # I3
    req = dict(rs="hello", ri=5, rl=[1, 2], rd={"k": 1}, rf=0.25, rb=True)
    r = await d.call(info, common.Req(**req), deserialize=False)
    check("I3", r.dendrite.status_code == 200 and r.echo == "hello", (r.dendrite.status_code, r.dendrite.status_message))
    syn, h = prepare(common.Req(**req))
    h.pop("bt_header_input_obj_rb")
    raw = await send(syn, h)
    ok = (raw.status == 500 and INTERNAL.match(raw.message() or "")) if python else raw.status == 200
    check("I3.missing-dummy", ok, (raw.status, raw.body[:200]))

    legacy = common.LegacyHashed(a=1, b=2, d=["foobar"])
    check("H3.legacy-hash", legacy.body_hash == "ae06397d08f30f75c91395c59f05c62ac3b62b88250eb78b109213258e6ced0c", legacy.body_hash)
    r = await d.call(info, legacy, deserialize=False)
    check("I2.legacy", r.dendrite.status_code == 200, (r.dendrite.status_code, r.dendrite.status_message))

    # I4
    syn, h = prepare(common.Mixed(**mixed.model_dump(include={"i", "f", "s", "l", "d"})))
    body = syn.model_dump()
    body["i"] = 8
    raw = await send(syn, h, json.dumps(body).encode())
    check("I4", raw.status == 500 and INTERNAL.match(raw.message() or ""), (raw.status, raw.body[:200]))

    # I5
    syn, h = prepare(common.Mixed(i=1))
    h["computed_body_hash"] = H1
    raw = await send(syn, h)
    expected = (f"Not Verified with error: Signature mismatch with {syn.dendrite.nonce}.{common.ALICE.ss58_address}."
                f"{common.BOB.ss58_address}.{syn.dendrite.uuid}.{H1} and {syn.dendrite.signature}")
    check("I5", raw.status == 401 and raw.message() == expected, (raw.status, raw.message()))

    # I6
    syn, h = prepare(bt.Synapse())
    resign(syn, nonce=time.time_ns() - 17_000_000_000, fresh_uuid=True)
    raw = await send(syn, syn.to_headers())
    m = raw.message() or ""
    ok = raw.status == 401 and re.fullmatch(
        r"Not Verified with error: Nonce is too old: acceptable delta is 16\.00 seconds but request was 17\.\d\d seconds old", m)
    check("I6", ok, (raw.status, m))

    # I7
    syn, h = prepare(bt.Synapse())
    first = await send(syn, h)
    second = await send(syn, h)
    check("I7", first.status == 200 and second.status == 401 and second.message() ==
          "Not Verified with error: Nonce is too old, a newer one was last processed", (first.status, second.status, second.message()))

    # I8
    syn, h = prepare(bt.Synapse())
    resign(syn, nonce=time.time_ns() - 60_000_000_000, fresh_uuid=True)
    h = syn.to_headers()
    h.pop("bt_header_dendrite_version")
    raw = await send(syn, h)
    ok = raw.status == 200 if python else (raw.status == 401 and (raw.message() or "").startswith(
        "Not Verified with error: Nonce is too old: acceptable delta is 16.00"))
    check("I8", ok, (raw.status, raw.message()))

    # I9
    r = await d.call(info, common.Deny(), deserialize=False)
    check("I9", r.axon.status_code == 403 and r.axon.status_message == "Forbidden. Key is blacklisted: denied by test.",
          (r.axon.status_code, r.axon.status_message))

    # I10
    syn, h = prepare(common.Status404())
    raw = await send(syn, h)
    data = raw.json() or {}
    check("I10", raw.status == 404 and data.get("name") == "Status404" and data.get("axon", {}).get("status_code") == 404,
          (raw.status, raw.body[:300]))
    r = await d.call(info, common.Status404(), deserialize=False)
    check("I10.merge", r.dendrite.status_code == 404, (r.dendrite.status_code, r.dendrite.status_message))

    # I11
    r = await d.call(info, common.Boom(), deserialize=False)
    check("I11", r.dendrite.status_code == 500 and INTERNAL.match(r.dendrite.status_message or ""),
          (r.dendrite.status_code, r.dendrite.status_message))

    # I12
    syn, h = prepare(bt.Synapse())
    raw = await send(syn, h, path="Nope")
    check("I12", raw.status == 404 and raw.message() == f"Synapse name 'Nope' not found. Available synapses {ROUTES}",
          (raw.status, raw.message()))

    # I13
    syn, h = prepare(bt.Synapse())
    raw = await send(syn, h, b"{")
    check("I13", raw.status == 422 and isinstance((raw.json() or {}).get("detail"), list), (raw.status, raw.body[:300]))

    # I14
    chunks = []
    final = None
    async for item in d.call_stream(info, common.Echo(text="hi"), deserialize=False):
        if isinstance(item, bt.Synapse):
            final = item
        else:
            chunks.append(item)
    check("I14", b"".join(chunks) == b"tok\n" * 3 and final is not None and final.dendrite.status_code == 200
          and final.completion == "tok\n" * 3, (chunks, final and final.dendrite))
    syn, h = prepare(common.Echo())
    raw = await send(syn, h)
    check("I14.headers", raw.status == 200 and raw.headers.get("content-type") == "text/event-stream"
          and raw.headers.get("bt_header_axon_status_code") == "200" and raw.headers.get("computed_body_hash") == H1,
          dict(raw.headers))

    # I15
    started = time.monotonic()
    r = await d.call(info, common.Slow(delay=3.0), timeout=2.0, deserialize=False)
    check("I15", r.dendrite.status_code == 408 and r.dendrite.status_message == "Request timeout after 2.0 seconds"
          and time.monotonic() - started < 2.9, (r.dendrite.status_code, r.dendrite.status_message))

    # I16
    tensor = bt.Tensor.serialize(np.array([[1.5, 2.0], [3.0, 4.0]], dtype=np.float32))
    r = await d.call(info, common.Tens(t=tensor), deserialize=False)
    ok = (r.dendrite.status_code == 200 and r.total == 10.5 and r.t is not None and r.t.buffer == tensor.buffer
          and r.t.dtype == "float32" and r.t.shape == [2, 2] and np.array_equal(r.t.deserialize(), tensor.deserialize()))
    check("I16", ok, (r.dendrite.status_code, r.dendrite.status_message, r.total))

    # I17
    d2 = bt.Dendrite(wallet=common.ALICE)
    results = await d2.forward([info] * 8, bt.Synapse(), deserialize=False)
    codes = [x.dendrite.status_code for x in results]
    rejected = [x for x in results if x.dendrite.status_code == 401]
    ok = codes.count(200) >= 1 and codes.count(200) + len(rejected) == 8 and all(
        x.dendrite.status_message == "Not Verified with error: Nonce is too old, a newer one was last processed" for x in rejected)
    check("I17", ok, f"accepted {codes.count(200)} rejected {len(rejected)}")
    await d2.aclose_session()

    # I17.order: with one uuid, exactly the out-of-order arrival is refused.
    d3 = bt.Dendrite(wallet=common.ALICE)
    first = d3.preprocess_synapse_for_request(info, bt.Synapse(), 12.0)
    second = d3.preprocess_synapse_for_request(info, bt.Synapse(), 12.0)
    third = d3.preprocess_synapse_for_request(info, bt.Synapse(), 12.0)
    nonces = [first.dendrite.nonce, second.dendrite.nonce, third.dendrite.nonce]
    order = [await send(syn, syn.to_headers()) for syn in (second, first, third)]
    ok = (nonces == sorted(set(nonces)) and [r.status for r in order] == [200, 401, 200]
          and order[1].message() == "Not Verified with error: Nonce is too old, a newer one was last processed")
    check("I17.order", ok, (nonces, [(r.status, r.message()) for r in order]))
    await d3.aclose_session()

    # M1: aiohttp sends non-finite floats as bare NaN/Infinity tokens.
    r = await d.call(info, common.NanReq(v=float("nan"), vs=[float("inf"), float("-inf")]), deserialize=False)
    check("M1.nan-request", r.dendrite.status_code == 200 and r.seen == "(nan, [inf, -inf])",
          (r.dendrite.status_code, r.dendrite.status_message, r.seen))

    # I19
    syn, h = prepare(bt.Synapse())
    resign(syn, nonce=time.time_ns() - 3_600_000_000_000, fresh_uuid=True)
    h = syn.to_headers()
    h["timeout"] = "nan"
    raw = await send(syn, h)
    ok = raw.status == 200 if python else (raw.status == 401 and (raw.message() or "").startswith(
        "Not Verified with error: Invalid timeout: nan"))
    check("I19", ok, (raw.status, raw.message()))

    # I20
    syn, h = prepare(common.WhoAmI())
    body = syn.model_dump()
    body["dendrite"]["hotkey"] = common.CHARLIE.ss58_address
    raw = await send(syn, h, json.dumps(body).encode())
    seen = (raw.json() or {}).get("seen")
    expected = common.CHARLIE.ss58_address if python else common.ALICE.ss58_address
    check("I20", raw.status == 200 and seen == expected, (raw.status, seen))

    # I21
    syn, h = prepare(bt.Synapse())
    raw = await send(syn, h, ctype="text/plain")
    check("I21", raw.status == 500 and INTERNAL.match(raw.message() or ""), (raw.status, raw.body[:200]))
    syn, h = prepare(bt.Synapse())
    raw = await send(syn, h, ctype=None)
    check("I21.no-content-type", raw.status == 500 and INTERNAL.match(raw.message() or ""), (raw.status, raw.body[:200]))

    # I22
    syn, h = prepare(bt.Synapse())
    raw = await send(syn, h, b"")
    check("I22", raw.status == 500 and INTERNAL.match(raw.message() or ""), (raw.status, raw.body[:200]))

    # I23
    syn, h = prepare(common.NanResp())
    raw = await send(syn, h)
    data = raw.json() or {}
    check("I23", (raw.status == 200 and "v" in data and data["v"] is None) or (python and raw.status == 500),
          (raw.status, raw.body[:300]))

    # I24: S4, S7-S11
    syn, h = prepare(bt.Synapse())
    message = f"{syn.dendrite.nonce}.{syn.dendrite.hotkey}.{syn.axon.hotkey}.{syn.dendrite.uuid}.{syn.body_hash}"
    zeros = "0x" + "00" * 64
    h["bt_header_dendrite_signature"] = zeros
    raw = await send(syn, h)
    check("S4", raw.status == 401 and raw.message() == f"Not Verified with error: Signature mismatch with {message} and {zeros}",
          (raw.status, raw.message()))
    for case, value, status, text in [
        ("S7", "00" * 64, 401, "Not Verified with error: Invalid signature format. Expected hex string."),
        ("S8", "0x" + "00" * 63, 401, "Not Verified with error: Invalid SR25519 signature."),
        ("S9", None, 200, None),
        ("S10", "", 401, "Not Verified with error: Missing Signature"),
    ]:
        syn, h = prepare(bt.Synapse())
        h["bt_header_dendrite_signature"] = value if value is not None else "0x" + syn.dendrite.signature
        raw = await send(syn, h)
        check(case, raw.status == status and (text is None or raw.message() == text), (raw.status, raw.message()))
    syn, h = prepare(bt.Synapse())
    h["bt_header_dendrite_nonce"] = f"+{syn.dendrite.nonce}"
    raw = await send(syn, h)
    check("S11", raw.status == 200, (raw.status, raw.message()))

    await session.close()
    await d.aclose_session()


if __name__ == "__main__":
    if sys.argv[1] == "--verify-s3":
        data = json.loads(sys.argv[2])
        PEER = "rust-signer"
        ok = Keypair(ss58_address=common.ALICE.ss58_address).verify(data["message"], data["signature"])
        check("I18/S3", ok is True, data["signature"])
    else:
        PEER = sys.argv[2]
        asyncio.run(main(int(sys.argv[1])))
    print(json.dumps({"summary": True, "failures": failures}), flush=True)
    sys.exit(1 if failures else 0)
