# Derived from RaoFoundation/subtensor sdk/python/bittensor/http_auth.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
"""Minimal btauth/1 signing and verification over bittensor-wallet keypairs.

The pinned bittensor 10.5.0 release predates ``bittensor.http_auth``; this is
the upstream payload, header and check-order contract ported onto the
``bittensor_wallet.Keypair`` primitives shipped with that release.
"""

from __future__ import annotations

import hashlib
import threading
import time

from bittensor_wallet import Keypair

PROTOCOL = "btauth/1"
VERSION = "1"
NO_RECEIVER = "-"
SCHEMES = {0: "ed25519", 1: "sr25519"}
CRYPTO_TYPES = {"ed25519": 0, "sr25519": 1}


class AuthError(Exception):
    """Authentication failure; ``status`` is the HTTP status to answer with."""

    status = 401


class Forbidden(AuthError):
    status = 403


def build_payload(scheme, method, path, body, nonce_ns, sender_ss58, receiver_ss58):
    lines = (
        PROTOCOL,
        scheme,
        method.upper(),
        path,
        hashlib.sha256(body).hexdigest(),
        str(nonce_ns),
        sender_ss58,
        receiver_ss58 or NO_RECEIVER,
    )
    return "\n".join(lines).encode()


def sign(keypair: Keypair, method, path, body=b"", receiver_ss58=None, nonce_ns=None):
    scheme = SCHEMES[keypair.crypto_type]
    nonce = time.time_ns() if nonce_ns is None else int(nonce_ns)
    payload = build_payload(scheme, method, path, body, nonce, keypair.ss58_address, receiver_ss58)
    headers = {
        "X-Bittensor-Version": VERSION,
        "X-Bittensor-Hotkey": keypair.ss58_address,
        "X-Bittensor-Nonce": str(nonce),
        "X-Bittensor-Signature": "0x" + bytes(keypair.sign(payload)).hex(),
    }
    if scheme != "sr25519":
        headers["X-Bittensor-Crypto"] = scheme
    if receiver_ss58 is not None:
        headers["X-Bittensor-Receiver"] = receiver_ss58
    return headers


class NonceStore:
    def __init__(self, retention=60.0):
        self.retention = retention
        self._seen = {}
        self._lock = threading.Lock()

    def check_and_store(self, sender, nonce):
        now = time.monotonic()
        with self._lock:
            self._seen = {k: t for k, t in self._seen.items() if t > now}
            if (sender, nonce) in self._seen:
                return False
            self._seen[(sender, nonce)] = now + self.retention
            return True


def verify(headers, body, *, method, path, self_ss58, store, allowed=None, max_age=10.0, skew=2.0):
    """Upstream order: shape, receiver, freshness, signature, (allowlist), replay."""
    h = {k.lower(): v for k, v in headers.items()}
    if h.get("x-bittensor-version") != VERSION:
        raise AuthError("missing or unsupported X-Bittensor-Version (expected '1')")
    sender = h.get("x-bittensor-hotkey")
    if not sender:
        raise AuthError("missing X-Bittensor-Hotkey")
    try:
        nonce = int(h["x-bittensor-nonce"])
    except (KeyError, ValueError):
        raise AuthError("X-Bittensor-Nonce is not a decimal integer") from None
    scheme = h.get("x-bittensor-crypto", "sr25519")
    if scheme not in CRYPTO_TYPES:
        raise AuthError("unsupported X-Bittensor-Crypto")
    try:
        signature = bytes.fromhex(h["x-bittensor-signature"].removeprefix("0x"))
    except (KeyError, ValueError):
        raise AuthError("X-Bittensor-Signature is not hex") from None
    receiver = h.get("x-bittensor-receiver")
    if receiver is None:
        raise AuthError("missing X-Bittensor-Receiver: this server requires receiver-bound requests")
    if receiver != self_ss58:
        raise AuthError("request was signed for a different receiving hotkey")
    now = time.time_ns()
    if now - nonce > max_age * 1e9:
        raise AuthError(f"nonce is older than the {max_age:g}s freshness window")
    if nonce - now > skew * 1e9:
        raise AuthError(f"nonce is more than {skew:g}s in the future")
    payload = build_payload(scheme, method, path, body, nonce, sender, receiver)
    try:
        valid = Keypair(ss58_address=sender, crypto_type=CRYPTO_TYPES[scheme]).verify(payload, signature)
    except Exception:
        raise AuthError("invalid hotkey address or signature encoding") from None
    if not valid:
        raise AuthError("signature does not verify against the claimed hotkey")
    if allowed is not None and sender not in allowed:
        raise Forbidden("client key is not authorized")
    if not store.check_and_store(sender, nonce):
        raise AuthError("this nonce was already accepted from this hotkey")
    return sender
