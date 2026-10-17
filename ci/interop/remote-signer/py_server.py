"""Python peer: a minimal rtao-remote-signer/1 server over bittensor-wallet keys.

Usage: py_server.py   (prints "listening on 127.0.0.1:PORT", then serves)

It authenticates btauth/1 with the upstream check order, admits only //Charlie,
re-derives the signing payload from call, extension bytes and the >256-byte
blake2 rule, and signs with //Alice. It has no runtime decoder and is not a
policy reference; it exists so the Rust RemoteSigner client is exercised
against an independent implementation.
"""

from __future__ import annotations

import hashlib
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from bittensor_wallet import Keypair

import btauth

ALICE = Keypair.create_from_uri("//Alice")
CHARLIE = Keypair.create_from_uri("//Charlie")
STORE = btauth.NonceStore()
GENESIS = sys.argv[1] if len(sys.argv) > 1 else "0x" + "00" * 32


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # keep CI logs focused on results
        return

    def reply(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authenticate(self, body):
        try:
            return btauth.verify(
                dict(self.headers.items()), body, method=self.command, path=self.path,
                self_ss58=ALICE.ss58_address, store=STORE, allowed={CHARLIE.ss58_address},
            )
        except btauth.AuthError as error:
            self.reply(error.status, {"detail": str(error)})
            return None

    def body(self):
        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length) if length else b""

    def do_GET(self):
        body = self.body()
        if self.authenticate(body) is None:
            return
        if self.path != "/accounts":
            return self.reply(404, {"detail": "not found"})
        self.reply(200, {
            "version": "rtao-remote-signer/1",
            "server_key": ALICE.ss58_address,
            "accounts": [{"account": ALICE.ss58_address, "crypto": "sr25519"}],
            "genesis_hash": GENESIS,
            "purposes": ["extrinsic"],
        })

    def do_POST(self):
        body = self.body()
        if self.authenticate(body) is None:
            return
        if self.path != "/sign":
            return self.reply(404, {"detail": "not found"})
        try:
            request = json.loads(body)
            if request["version"] != "rtao-remote-signer/1" or request["account"] != ALICE.ss58_address:
                raise ValueError("unsupported version or account")
            parts = [bytes.fromhex(request[key][2:]) for key in ("call", "included_in_extrinsic", "included_in_signed_data")]
            payload = bytes.fromhex(request["payload"][2:])
        except (KeyError, ValueError, TypeError) as error:
            return self.reply(400, {"detail": f"malformed request: {error}", "code": "malformed_request"})
        data = b"".join(parts)
        expected = data if len(data) <= 256 else hashlib.blake2b(data, digest_size=32).digest()
        if expected != payload:
            return self.reply(422, {"detail": "payload differs from call, extra and additional data", "code": "tampered_request"})
        signature = bytes(ALICE.sign(expected))
        self.reply(200, {
            "version": "rtao-remote-signer/1",
            "account": ALICE.ss58_address,
            "signature": "0x01" + signature.hex(),
        })


def main():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"listening on 127.0.0.1:{server.server_address[1]}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
