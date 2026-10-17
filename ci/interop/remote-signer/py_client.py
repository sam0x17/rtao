"""Python peer: drive the Rust rtao-remote-signerd with bittensor-wallet keys.

Usage: py_client.py URL GOLDEN_JSON

Every request body is built here from the vendored golden vectors, signed with
btauth/1 by a bittensor-wallet keypair, and the server's MultiSignature is
verified with bittensor-wallet. Exits non-zero on the first failed check.
"""

from __future__ import annotations

import copy
import hashlib
import json
import sys
import urllib.error
import urllib.request

from bittensor_wallet import Keypair

import btauth

ALICE = Keypair.create_from_uri("//Alice")
BOB = Keypair.create_from_uri("//Bob")
CHARLIE = Keypair.create_from_uri("//Charlie")
DAVE = Keypair.create_from_uri("//Dave")
FERDIE_ED = Keypair.create_from_uri("//Ferdie", crypto_type=0)
EXTENSIONS = [
    "CheckNonZeroSender", "CheckSpecVersion", "CheckTxVersion", "CheckGenesis",
    "CheckMortality", "CheckNonce", "CheckWeight", "ChargeTransactionPayment",
    "SudoTransactionExtension", "CheckShieldedTxValidity", "SubtensorTransactionExtension",
    "DrandPriority", "CheckMetadataHash",
]
failures = 0


def check(name, condition, detail=""):
    global failures
    print(("PASS " if condition else "FAIL ") + name + ("" if condition else f": {detail}"))
    if not condition:
        failures += 1


def metadata_hash(golden):
    framed = bytes.fromhex(golden["metadata"]["v15_hex"][2:])
    assert framed[0] == 1
    rest = framed[1:]
    mode = rest[0] & 3
    offset = {0: 1, 1: 2, 2: 4}[mode]
    inner = rest[offset:]
    assert inner[:4] == b"meta"
    return "0x" + hashlib.blake2b(inner, digest_size=32).hexdigest()


def body_v1(golden, call_hex=None, era="0x2500", nonce=3, immortal=False):
    genesis = golden["network"]["genesis_hash"]
    vector = golden["signature_payloads"][0 if immortal else 1]
    block = genesis if immortal else vector["era_birth_block_hash"]
    call = call_hex or vector["call_data_hex"]
    era_bytes = bytes.fromhex(era[2:])
    extra = era_bytes + bytes([nonce << 2, 0, 0])
    additional = (
        bytes.fromhex("cd01000001000000") + bytes.fromhex(genesis[2:]) + bytes.fromhex(block[2:]) + b"\x00"
    )
    data = bytes.fromhex(call[2:]) + extra + additional
    payload = data if len(data) <= 256 else hashlib.blake2b(data, digest_size=32).digest()
    if call_hex is None:
        assert payload.hex() == vector["payload_hex"][2:], "golden payload derivation"
    return {
        "version": "rtao-remote-signer/1",
        "account": ALICE.ss58_address,
        "crypto": "sr25519",
        "purpose": "extrinsic",
        "chain": {
            "genesis_hash": genesis,
            "spec_version": golden["network"]["spec_version"],
            "transaction_version": golden["network"]["transaction_version"],
            "metadata_blake2_256": metadata_hash(golden),
        },
        "call": call,
        "included_in_extrinsic": "0x" + extra.hex(),
        "included_in_signed_data": "0x" + additional.hex(),
        "payload": "0x" + payload.hex(),
        "payload_json": {
            "address": ALICE.ss58_address,
            "blockHash": block,
            "genesisHash": genesis,
            "method": call,
            "nonce": f"0x{nonce:08x}",
            "specVersion": f"0x{golden['network']['spec_version']:08x}",
            "tip": "0x" + "00" * 16,
            "transactionVersion": f"0x{golden['network']['transaction_version']:08x}",
            "era": era,
            "version": 4,
            "signedExtensions": EXTENSIONS,
            "mode": 0,
            "metadataHash": None,
        },
    }


def send(url, method, path, body, headers):
    request = urllib.request.Request(url.rstrip("/") + path, data=body if method == "POST" else None, method=method)
    for name, value in headers.items():
        request.add_header(name, value)
    request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, json.loads(response.read() or b"null")
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            return error.code, json.loads(raw)
        except ValueError:
            return error.code, raw.decode(errors="replace")


def post(url, keypair, body_bytes, receiver=None, headers=None):
    headers = headers or btauth.sign(keypair, "POST", "/sign", body_bytes, receiver or ALICE.ss58_address)
    return send(url, "POST", "/sign", body_bytes, headers), headers


def verify_multisignature(body, response):
    signature = bytes.fromhex(response["signature"][2:])
    payload = bytes.fromhex(body["payload"][2:])
    return (
        len(signature) == 65
        and signature[0] == 1
        and Keypair(ss58_address=ALICE.ss58_address).verify(payload, signature[1:])
    )


def main():
    url, golden_path = sys.argv[1], sys.argv[2]
    with open(golden_path) as handle:
        golden = json.load(handle)

    headers = btauth.sign(CHARLIE, "GET", "/accounts", b"", ALICE.ss58_address)
    status, accounts = send(url, "GET", "/accounts", b"", headers)
    check("accounts lists Alice for Charlie", status == 200 and accounts["accounts"] == [
        {"account": ALICE.ss58_address, "crypto": "sr25519"}] and accounts["server_key"] == ALICE.ss58_address,
        (status, accounts))

    v1 = body_v1(golden)
    raw = json.dumps(v1).encode()
    (status, reply), signed_headers = post(url, CHARLIE, raw)
    check("sr25519 client signs V1", status == 200 and reply["account"] == ALICE.ss58_address
          and verify_multisignature(v1, reply), (status, reply))

    (status, reply), _ = post(url, FERDIE_ED, raw)
    check("ed25519 btauth client signs V1", status == 200 and verify_multisignature(v1, reply), (status, reply))

    (status, reply), _ = post(url, CHARLIE, raw, headers=signed_headers)
    check("verbatim replay refused", status == 401
          and reply == {"detail": "this nonce was already accepted from this hotkey"}, (status, reply))

    flipped = bytearray(raw)
    flipped[len(flipped) // 2] ^= 1
    fresh = btauth.sign(CHARLIE, "POST", "/sign", raw, ALICE.ss58_address)
    status, reply = send(url, "POST", "/sign", bytes(flipped), fresh)
    check("tampered body refused by btauth", status == 401
          and reply["detail"] == "signature does not verify against the claimed hotkey", (status, reply))

    inconsistent = copy.deepcopy(v1)
    inconsistent["payload_json"]["nonce"] = "0x00000004"
    (status, reply), _ = post(url, CHARLIE, json.dumps(inconsistent).encode())
    check("inconsistent authenticated body is tampered_request", status == 422
          and reply.get("code") == "tampered_request", (status, reply))

    (status, reply), _ = post(url, DAVE, raw)
    check("unknown client key refused", status == 403 and reply["detail"] == "client key is not authorized",
          (status, reply))

    (status, reply), _ = post(url, CHARLIE, raw, receiver=BOB.ss58_address)
    check("wrong receiver refused", status == 401
          and reply["detail"] == "request was signed for a different receiving hotkey", (status, reply))

    (status, reply), _ = post(url, CHARLIE, json.dumps(body_v1(golden, era="0x00", nonce=7, immortal=True)).encode())
    check("immortal era refused", status == 422 and reply.get("code") == "era_rejected", (status, reply))

    over = body_v1(golden, call_hex=golden["signature_payloads"][1]["call_data_hex"][:-4] + "e9c0")
    (status, reply), _ = post(url, CHARLIE, json.dumps(over).encode())
    check("transfer of 12346 rao exceeds max_spend", status == 403 and reply.get("code") == "policy_denied"
          and reply["violations"][0].startswith("spend "), (status, reply))

    print(f"{failures} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
