"""Seeded native-balance, conform and classification behavior from pinned Python."""

from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import math
from pathlib import Path
import random
import struct

from bittensor.balance import Balance
from bittensor import hyperparams as hp
from bittensor.intents.weights import _Preflight, _conform, clip_to_max_weight, normalize
from bittensor.result import _NAME_TO_CODE, _SUBSTRING_FALLBACK, chain_error_from_substrate_request, classify_error
from bittensor._transport.errors import SubstrateRequestException
from bittensor._transport.rpc import classify_rpc_error
from bittensor.timelock import round_at, reveal_time

from common import jsonable, write

SEED = 0x5254414F

# This literal input recipe is also parsed by Rust. These are inputs only;
# every result below comes from the pinned Python implementation in CI.
HYPER_INT_INPUTS = [
    "-1", "0", "1", "2", "4", "5", "59", "60", "359", "360", "361",
    "32767", "32768", "65535", "65536", "999999", "1000000", "1000001",
    "999999999", "1000000000", "1000000001",
    "9007199254740991", "9007199254740992", "9007199254740993",
    "18446744073709551614", "18446744073709551615", "18446744073709551616",
    "18446744073709551617", "170141183460469231731687303715884105727",
    "-170141183460469231731687303715884105728",
    "340282366920938463463374607431768211455",
    "340282366920938463463374607431768211456",
]
HYPER_FLOAT_BITS = [
    0x0000000000000000, 0x8000000000000000,
    0x0000000000000001, 0x8000000000000001,
    0x3FDFFFFFFFFFFFFF, 0x3FE0000000000000, 0x3FE0000000000001,
    0x3FEFFFFFFFFFFFFF, 0x3FF0000000000000, 0x3FF0000000000001,
    0xBFE0000000000000, 0x3FF8000000000000, 0x4004000000000000,
    0x4008000000000000, 0x4014000000000000, 0x4076800000000000,
    0x4340000000000000, 0x43F0000000000000,
    0x7FEFFFFFFFFFFFFF, 0x7FF0000000000000, 0xFFF0000000000000,
    0x7FF8000000000000,
]
HYPER_TIE_NUMERATORS = [0, 1, 2, 3, 127, 128, 32766, 32767, 499999]
HYPER_TEXT_INPUTS = [
    "", " ", "0", "1", "2", "-1", "+1", "++1", "01", "1_0", "1__0",
    "0.0", "0.5", "0.7", "1.0", "1.5", "3.0", "1e0", "7e-1", "1e10000",
    "true", "false", " +TRUE ", "yes", "on", "nan", "nan.", "inf", "-inf",
    "\u2003+1\u3000", " +\uff11_\uff12 ", "\u0661.\u0665", "bad'\"value", "1\n2",
]


def hyper_bits(value: float) -> int:
    return int.from_bytes(struct.pack(">d", value), "big")


def hyper_capture(function, *args):
    try:
        result = function(*args)
    except Exception as error:
        return {"error": {"type": type(error).__name__, "message": str(error)}}
    if isinstance(result, float):
        return {"ok": {"f64_bits": hyper_bits(result)}}
    return {"ok": jsonable(result)}


def hyper_cases(name: str):
    def integer(case_id: str, value: int):
        return case_id, {"type": "int", "value": value}, value

    def floating(case_id: str, value: float):
        return case_id, {"type": "f64", "bits": hyper_bits(value)}, value

    for index, token in enumerate(HYPER_INT_INPUTS):
        yield integer(f"integer/anchor/{index}", int(token))
    lo, hi = hp.raw_bounds(name)
    for label, boundary in [("minimum", lo), ("maximum", hi)]:
        for offset in [-1, 0, 1]:
            yield integer(f"integer/{label}/{offset}", boundary + offset)
    for value in [False, True]:
        yield f"bool/{int(value)}", {"type": "bool", "value": value}, value
    for index, bits in enumerate(HYPER_FLOAT_BITS):
        yield floating(f"float/anchor/{index}", struct.unpack(">d", bits.to_bytes(8, "big"))[0])
    kind = hp.kind_of(name)
    scale = hp._FRACTION_DENOMINATOR.get(kind)
    if scale is None:
        scale = hp.RAO_PER_TAO if kind == "rao" else hp.FIXED128_ONE if kind == "fixed128" else 1
    for numerator in HYPER_TIE_NUMERATORS:
        middle = (numerator + 0.5) / scale
        for label, value in [("below", math.nextafter(middle, -math.inf)), ("tie", middle),
                             ("above", math.nextafter(middle, math.inf))]:
            yield floating(f"float/tie/{numerator}/{label}", value)
    for label, boundary in [("minimum", lo), ("maximum", hi)]:
        middle = float(boundary) / scale
        for side, value in [("below", math.nextafter(middle, -math.inf)), ("at", middle),
                            ("above", math.nextafter(middle, math.inf))]:
            yield floating(f"float/{label}/{side}", value)
    for index, value in enumerate(HYPER_TEXT_INPUTS):
        yield f"text/{index}", {"type": "text", "value": value}, value


def hyper_row(name: str) -> dict:
    methods = ["metadata_kind", "kind_of", "doc_of", "short_of", "codec_of", "raw_bounds", "value_forms"]
    row = {"name": name, "methods": {method: hyper_capture(getattr(hp, method), name) for method in methods},
           "inputs": []}
    for case_id, descriptor, value in hyper_cases(name):
        case = {"id": case_id, "input": descriptor,
                "annotate": hyper_capture(hp.annotate, name, value),
                "to_raw": hyper_capture(hp.to_raw, name, value)}
        # normalized's public raw argument is numeric; annotate and to_raw
        # also exercise text. Preserve bool as its own Python numeric type.
        if descriptor["type"] != "text":
            case["normalized"] = hyper_capture(hp.normalized, name, value)
        row["inputs"].append(case)
    return row


def dump_hyperparams(directory: Path) -> None:
    source = "sdk/python/bittensor/hyperparams.py"
    settings = "sdk/python/bittensor/settings.py"
    document = {"schema": 1, "domain": "native-codec", "recipe": 1, "source": source,
                "source_sha256": hashlib.sha256((Path("/upstream") / source).read_bytes()).hexdigest(),
                "settings_sha256": hashlib.sha256((Path("/upstream") / settings).read_bytes()).hexdigest(),
                "table_names": sorted(hp.HYPERPARAMS),
                "storage_idents": [{"parameter": name, "ident": getattr(item, "value_type_ident", "") or ""}
                                   for name, item in sorted(hp.STORAGE_ITEMS.items())],
                "vectors": [hyper_row(name) for name in sorted(hp.HYPERPARAMS) + ["__rtao_unknown__"]]}
    original_meta, original_storage = hp.HYPERPARAMS, hp.STORAGE_ITEMS
    # Controlled input tables force precedence branches that the current
    # catalog may not contain. Outputs still use the actual upstream methods.
    fixtures = {
        "__rtao_metadata__": hp.Hyperparam("int", "oracle metadata fixture", "metadata fixture"),
        "__rtao_explicit__": hp.Hyperparam("int", "oracle explicit fixture", "explicit fixture", signed=True, bits=8),
        "__rtao_signed__": hp.Hyperparam("blocks", "oracle signed fixture", "signed fixture"),
        "__rtao_signed_alone__": hp.Hyperparam("int", "oracle signed-alone fixture", "signed-alone fixture", signed=True),
        "__rtao_fallback__": hp.Hyperparam("int", "oracle fallback fixture"),
        "__rtao_unknown_ident__": hp.Hyperparam("bool", "oracle unknown-ident fixture", "unknown-ident fixture"),
    }
    descriptors = {
        "__rtao_metadata__": "TaoBalance", "__rtao_explicit__": "PerU16", "__rtao_signed__": "i16",
        "__rtao_signed_alone__": "u8", "__rtao_unknown_ident__": "UnknownOracleIdent",
        "__rtao_no_meta__": "PerU16",
    }
    try:
        hp.HYPERPARAMS = {**original_meta, **fixtures}
        hp.STORAGE_ITEMS = {**original_storage,
                            **{name: hp.st.Item("Oracle", name, ident) for name, ident in descriptors.items()}}
        document["precedence"] = [hyper_row(name) for name in sorted(set(fixtures) | set(descriptors))]
    finally:
        hp.HYPERPARAMS, hp.STORAGE_ITEMS = original_meta, original_storage
    write(directory / "pure/hyperparams.json", document)


def capture(function, *args, **kwargs):
    try:
        result = function(*args, **kwargs)
    except Exception as error:
        return {"error": {"type": type(error).__name__, "message": str(error)}}
    try:
        return {"ok": jsonable(result)}
    except ValueError:
        # Direct clip is an internal helper; invalid nonfinite input can return
        # nonfinite intermediates. Preserve their bits outside JSON numbers.
        return {"nonfinite_f64_bits": [int.from_bytes(struct.pack(">d", value), "big") for value in result]}


def dump(directory: Path) -> None:
    dump_hyperparams(directory)
    rng = random.Random(SEED)
    maximum = (1 << 64) - 1
    values = {0, 1, 499, 500, 999_999_999, 1_000_000_000, 1_000_000_001, maximum}
    for boundary in [1 << 53, 1 << 63, maximum, 10**9, 10**18]:
        values.update(value for value in range(boundary - 8, boundary + 9) if 0 <= value <= maximum)
    while len(values) < 10_000:
        values.add(rng.getrandbits(64))
    write(directory / "pure/balance.json", {"schema": 1, "domain": "u64-rao", "seed": SEED,
          "source": "sdk/python/bittensor/balance.py", "vectors": [
              {"rao": value, "display": str(Balance.from_rao(value))} for value in sorted(values)]})
    weight_cases = [([], []), ([0], [0.0]), ([0], [1.0]), ([0, 1], [1.0, 1.0]),
                    ([0, 1, 2], [1.0, 2.0, 100.0]), ([0, 1], [1.0, 0.5 / 65535]),
                    ([0, 1], [1.0, 1.5 / 65535]), ([0, 1], [1.0, 2.5 / 65535]),
                    ([0, 1], [math.nan, 1.0]), ([0, 1], [math.inf, 1.0]),
                    ([0, 1], [-1.0, 1.0]), ([65536], [1.0]), ([0], [1.0, 2.0]),
                    ([0, 1, 2], [1e16, 1.0, 1.0]),
                    ([0, 1, 2, 3], [1.0, 1e-16, 1e-16, 1e-16])]
    for size in [2, 3, 5, 16, 256, 2500]:
        for _ in range(8):
            weight_cases.append((list(range(size)), [rng.random() * rng.choice([1e-15, 1.0, 1e15]) for _ in range(size)]))
    weight_cases.append((list(range(2500)), [float(rng.randrange(65536)) for _ in range(2500)]))
    vectors = []
    for uids, weights in weight_cases:
        bits = [int.from_bytes(struct.pack(">d", value), "big") for value in weights]
        for limit, minimum in [(65535, 0), (32768, 2), (1000, 4)]:
            preflight = _Preflight(uid=0, commit_reveal=False, min_allowed_weights=minimum, max_weight_limit=limit)
            vectors.append({"uids": uids, "weights_f64_bits": bits, "limit_u16": limit,
                            "min_allowed_weights": minimum, "uid": 0, "netuid": 1,
                            "normalize": capture(normalize, uids, weights),
                            "clip": capture(clip_to_max_weight, weights, limit / 65535),
                            "conform": capture(_conform, uids, weights, preflight, 1),
                            "conform_raw_u16": capture(_conform, uids, weights, preflight, 1, True)})
    write(directory / "pure/conform.json", {"schema": 1, "seed": SEED,
          "source": "sdk/python/bittensor/intents/weights.py", "vectors": vectors})
    messages = ["", "unrecognized message", "Too many requests", "priority is too low",
                "Custom error: 0", "Custom error: 4", "Custom error: 255", "Transaction is outdated",
                "Invalid Transaction: Payment", "state discarded", "not registered"]
    messages += [needle for needle, _ in _SUBSTRING_FALLBACK]
    classified = [{"text": text, "name": None, "code": classify_error(text).value} for text in messages]
    classified += [{"text": "deliberately irrelevant", "name": name,
                    "code": classify_error("deliberately irrelevant", name).value}
                   for name in sorted(set(_NAME_TO_CODE) | {"UnmappedRateLimitExceeded", "Unknown"})]
    payloads = [
        {"code": -32004, "message": "Too many requests"},
        {"code": -32005, "message": "Too many subscriptions"},
        {"code": -32004, "message": "unrelated failure"},
        {"code": -32000, "message": "state discarded", "data": "State already discarded for 0x" + "11" * 32},
        {"code": -32004, "message": "storage work rate limit exceeded"},
        {"code": -32005, "message": "upstream capacity exceeded",
         "data": {"policy": "public", "reason": "capacity", "retryAfter": 15}},
        {"code": -32000, "message": "State already discarded for 0x" + "22" * 32},
        {"code": 1010, "message": "Invalid Transaction", "data": "Transaction is outdated"},
    ]
    # Explicit construction keeps the custom-code cases readable and avoids
    # copying private classifier implementations into the oracle.
    payloads += [{"code": 1010, "message": "Invalid Transaction", "data": {"Custom": number}} for number in [0, 1, 4, 255]]
    requests = []
    for payload in payloads:
        error = classify_rpc_error(payload)
        requests.append({"payload": payload, "rpc": {"type": type(error).__name__, "message": str(error),
                         "retry_after": getattr(error, "retry_after", None),
                         "policy": getattr(error, "policy", None), "reason": getattr(error, "reason", None),
                         "block_hash": getattr(error, "block_hash", None)},
                         "chain": chain_error_from_substrate_request(SubstrateRequestException({"error": payload})).to_dict()})
    write(directory / "pure/classification.json", {"schema": 1,
          "source": "sdk/python/bittensor/result.py", "messages": classified, "rpc": requests})
    stamps = [1_692_803_367, 1_692_803_368, 1_700_000_000, 1_800_000_000]
    write(directory / "pure/timelock.json", {"schema": 1, "source": "sdk/python/bittensor/timelock.py",
          "round_at": [{"unix": stamp, "result": capture(round_at, datetime.fromtimestamp(stamp, timezone.utc))} for stamp in stamps],
          "reveal_time": [{"round": value, "result": capture(reveal_time, value)} for value in [1, 2, 10, 1_000_000]]})
