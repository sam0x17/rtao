"""Capture bittensor==10.5.0 golden vectors for rtao-compat-synapse's offline tests.

Usage: capture_fixtures.py <out_dir> <harness_dir>

Writes requests.json, hashes.json, responses.json, tensors.json and
provenance.json. Time and uuids are frozen so everything except sr25519
signatures (randomized) and CPython size fields is deterministic.
"""

import hashlib
import importlib.metadata as metadata
import json
import math
import sys
import uuid as uuid_module
import warnings

warnings.simplefilter("ignore")
sys.path.insert(0, sys.argv[2])
import bittensor as bt  # noqa: E402
import bittensor.core.dendrite as bt_dendrite  # noqa: E402
import numpy as np  # noqa: E402
from bittensor.utils import get_hash  # noqa: E402
from pydantic import BaseModel  # noqa: E402
from starlette.testclient import TestClient  # noqa: E402

import common  # noqa: E402

NONCE = 1760000000123456789
UUID = "5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a"
EXTERNAL_IP = "203.0.113.7"


class FrozenTime:
    def __getattr__(self, name):
        import time

        return getattr(time, name)

    @staticmethod
    def time_ns():
        return NONCE


bt_dendrite.time = FrozenTime()
uuid_module.uuid1 = lambda *a, **k: uuid_module.UUID(UUID)

OUT = sys.argv[1]


def dump(name, data):
    with open(f"{OUT}/{name}", "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=1, ensure_ascii=False, sort_keys=False)
        handle.write("\n")


def info(port=8091):
    return bt.AxonInfo(version=10005000, ip="198.51.100.9", ip_type=4, port=port,
                       hotkey=common.BOB.ss58_address, coldkey=common.BOB.ss58_address)


def tensor():
    return bt.Tensor.serialize(np.array([[1.5, 2.0], [3.0, 4.0]], dtype=np.float32))


def samples():
    s_text = "it's \"q\" é \U0001F600 ​ \\"
    return [
        bt.Synapse(),
        common.Mixed(i=7, f=1.5, s=s_text, l=["a", "b'c"], d={"x": [0.1, 1e-5, 1e16], "a": [2.0]}),
        common.Req(rs="hello", ri=5, rl=[1, 2], rd={"k": 1}, rf=0.25, rb=True),
        common.LegacyHashed(a=1, b=2, d=["foobar"]),
        common.Tens(t=tensor()),
        common.Tens(),
        common.Echo(text="hi"),
    ]


def requests():
    dendrite = bt.Dendrite(wallet=common.ALICE)
    dendrite.external_ip = EXTERNAL_IP
    out = []
    for sample in samples():
        syn = dendrite.preprocess_synapse_for_request(info(), sample, 12.0)
        headers = list(syn.to_headers().items())
        body = json.dumps(syn.model_dump())
        message = f"{syn.dendrite.nonce}.{syn.dendrite.hotkey}.{syn.axon.hotkey}.{syn.dendrite.uuid}.{syn.body_hash}"
        out.append({"name": syn.name, "headers": headers, "body": body, "body_hash": syn.body_hash, "message": message})
    return out


def py_json(value):
    """Encode a Python value losslessly for the Rust test's PyValue builder."""
    if value is None or isinstance(value, bool) or isinstance(value, str):
        return value
    if isinstance(value, int):
        return {"int": str(value)}
    if isinstance(value, float):
        return {"float": repr(value)}
    if isinstance(value, list):
        return [py_json(item) for item in value]
    if isinstance(value, tuple):
        return {"tuple": [py_json(item) for item in value]}
    if isinstance(value, dict):
        return {"dict": [[py_json(k), py_json(v)] for k, v in value.items()]}
    raise TypeError(type(value))


class Inner(BaseModel):
    x: int = 1
    y: list[float] = [0.5]


class Holder(bt.Synapse):
    v: object = None


def hashes():
    values = [
        "", "plain", "it's", 'say "hi"', "both ' and \"", "line\nbreak", "tab\tand\\", "\x7f", " ", "​",
        "\U0001F600", "", "\x00\x1f", "͸",
        ["it's", 'say "hi"', "both ' and \"", "line\nbreak", "\x7f", " ", "​", "\U0001F600", "",
         "\x00", "͸", "\U000e0001", "퟿"],
        0.1, 1e-05, 1e16, -0.0, math.inf, -math.inf, 1.0, 123456789.125, 5e-324, 1.7976931348623157e308,
        [0.1, 1e-05, 1e16, -0.0, math.inf],
        {"a": [0.5, 2.0], "z": [], "m": [1e-7]},
        True, False, None, 2 ** 70, -(2 ** 70), 0,
        (1,), (1, "a", None), (),
        {"k": {"nested": (1, 2)}, "b": [True, None]},
        Inner().model_dump(),
        tensor().model_dump(),
        [[], {}, [[]]],
    ]
    out = []
    for value in values:
        text = str(Holder(v=value).model_dump()["v"])
        out.append({"value": py_json(value), "str": text, "hash": get_hash(text)})
    for sample in samples():
        out.append({"synapse": sample.name, "body_hash": sample.body_hash})
    return out


def responses():
    axon = common.build_axon(18093)
    client = TestClient(axon.app)
    dendrite = bt.Dendrite(wallet=common.ALICE)
    real_time = __import__("time")
    out = {}

    def request(case, synapse, mutate=None, path=None, body=None, ctype="application/json", method="POST"):
        dendrite.uuid = str(uuid_module.UUID(int=len(out) + 1))
        bt_dendrite.time = real_time
        syn = dendrite.preprocess_synapse_for_request(info(18093), synapse, 12.0)
        bt_dendrite.time = FrozenTime()
        headers = syn.to_headers()
        payload = json.dumps(syn.model_dump()) if body is None else body
        if mutate:
            mutate(headers)
        if ctype:
            headers["content-type"] = ctype
        response = client.request(method, f"/{path or syn.name}", headers=headers, content=payload)
        try:
            data = response.json()
        except Exception:
            data = response.text
        out[case] = {
            "status": response.status_code,
            "header_names": sorted(name for name in response.headers.keys() if name not in ("content-length",)),
            "headers": {k: v for k, v in response.headers.items()
                        if k in ("name", "bt_header_axon_status_code", "bt_header_axon_status_message", "computed_body_hash",
                                 "content-type", "bt_header_dendrite_hotkey", "bt_header_axon_hotkey", "allow")},
            "body": data,
        }

    request("ping", bt.Synapse())
    request("unknown_route", bt.Synapse(), path="Nope")
    request("blacklisted", common.Deny())
    request("missing_signature", bt.Synapse(), lambda h: h.pop("bt_header_dendrite_signature"))
    request("signature_without_0x", bt.Synapse(), lambda h: h.update(bt_header_dendrite_signature="00" * 64))
    request("signature_63_bytes", bt.Synapse(), lambda h: h.update(bt_header_dendrite_signature="0x" + "00" * 63))
    request("missing_hotkey", bt.Synapse(), lambda h: h.pop("bt_header_dendrite_hotkey"))
    request("bad_hotkey", bt.Synapse(), lambda h: h.update(bt_header_dendrite_hotkey="0OIl"))
    request("header_parse_failure", bt.Synapse(), lambda h: h.update(bt_header_dendrite_nonce="abc"))
    request("invalid_json", bt.Synapse(), body="{")
    request("empty_body", bt.Synapse(), body="")
    request("text_plain", bt.Synapse(), ctype="text/plain")
    request("status_404", common.Status404())
    request("boom", common.Boom())
    request("hash_header_tampered", common.Mixed(i=1), lambda h: h.update(computed_body_hash=get_hash("")))
    # FastAPI answers these after the middleware's blacklist and verification.
    request("extra_segment", bt.Synapse(), path="Synapse/extra")
    request("extra_segment_unsigned", bt.Synapse(), lambda h: h.pop("bt_header_dendrite_signature"),
            path="Synapse/extra")
    request("put_method", bt.Synapse(), method="PUT")
    # Uvicorn and the test client percent-decode the path.
    request("percent_encoded", bt.Synapse(), path="Syn%61pse")
    return out


def tensors():
    arrays = [
        ("float32", np.array([[1.5, 2.0], [3.0, 4.0]], dtype=np.float32)),
        ("int64", np.array(3, dtype=np.int64)),
        ("bool", np.array([True, False])),
        ("float16", np.arange(6, dtype=np.float16).reshape(2, 3)),
        ("uint8", np.arange(300, dtype=np.uint8)),
        ("float64", np.zeros((0, 2))),
    ]
    out = []
    for dtype, array in arrays:
        t = bt.Tensor.serialize(array)
        out.append({"dtype": t.dtype, "shape": t.shape, "buffer": t.buffer,
                    "array_shape": list(array.shape), "data_hex": np.ascontiguousarray(array).tobytes().hex()})
    return out


def main():
    dump("requests.json", requests())
    dump("hashes.json", hashes())
    dump("responses.json", responses())
    dump("tensors.json", tensors())
    s1 = (f"1700000000000000000.{common.ALICE.ss58_address}.{common.BOB.ss58_address}."
          f"5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a.{get_hash('')}")
    packages = ["bittensor", "bittensor-wallet", "aiohttp", "fastapi", "starlette", "pydantic", "pydantic-core", "uvicorn",
                "msgpack-numpy-opentensor", "numpy", "async-substrate-interface"]
    freeze = "\n".join(sorted(f"{d.metadata['Name']}=={d.version}" for d in metadata.distributions()))
    dump("provenance.json", {
        "bittensor": "v10.5.0 (b9af04ad3452dde398460d464598837313226101)",
        "python": sys.version.split()[0],
        "packages": {p: metadata.version(p) for p in packages},
        "pip_freeze_sha256": hashlib.sha256(freeze.encode()).hexdigest(),
        "requirements_sha256": hashlib.sha256(open(f"{sys.argv[2]}/requirements.txt", "rb").read()).hexdigest(),
        "frozen_nonce": NONCE,
        "frozen_uuid": UUID,
        "external_ip": EXTERNAL_IP,
        "s1": {"message": s1, "python_signature": "0x" + common.ALICE.sign(s1).hex()},
        "torch_workaround": "bittensor.core.tensor.dtypes.torch = True (Tensor is unusable without torch otherwise)",
    })


main()
