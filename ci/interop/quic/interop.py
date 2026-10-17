#!/usr/bin/env python3
"""rtao-transport-quic <-> btlightning (Python bindings) interop harness.

Runs both directions against btlightning-py built from
inference-labs-inc/lightning @ 53c090b436238f78a4a0dd30cbbfcba1cbebe2e0:

  A. rtao client (quic_peer suite) -> Python LightningServer
  B. Python Lightning client -> rtao server (quic_peer serve)
  C. rtao raw wire suite -> rtao server (same expectations as A)
  D. fixture capture of a Python client HandshakeRequest (quic_peer capture)

Usage: interop.py --peer PATH/TO/quic_peer [--capture-dir DIR]
Exits non-zero if any case fails. Every peer listens on 127.0.0.1 only.
"""

import argparse
import json
import socket
import subprocess
import sys
import threading
import time
import traceback

from btlightning import Lightning, LightningServer

MINER_SEED = bytes([1] * 32)
VALIDATOR_SEED = bytes([2] * 32)
MINER_HOTKEY = "5CcyqxXnJucaCnQQvvUg5EPzj1uoNAxACZvzArHw5aVDvgNH"
VALIDATOR_HOTKEY = "5CfCr47V5Dte6bwxNBE8K9oNnQd9fiay6aDEEkgYtFv7w4Fq"

RESULTS = []


def record(direction, case, ok, detail=""):
    RESULTS.append({"direction": direction, "case": case, "ok": bool(ok), "detail": str(detail)[:400]})
    print(json.dumps(RESULTS[-1]), flush=True)


def free_udp_port():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


# ---------------------------------------------------------------- Python server


def stream_handler(data):
    n = int(data.get("n", 3))
    return iter([f"chunk-{i}".encode() for i in range(1, n + 1)])


def stream_fail_handler(data):
    def gen():
        yield b"chunk-1"
        yield b"chunk-2"
        raise RuntimeError("mid-stream failure")

    return gen()


def fail_handler(data):
    raise RuntimeError("boom")


def start_py_server():
    port = free_udp_port()
    server = LightningServer(miner_hotkey=MINER_HOTKEY, host="127.0.0.1", port=port)
    server.set_miner_keypair(MINER_SEED)
    server.register_synapse_handler("echo", lambda data: data)
    server.register_synapse_handler("fail", fail_handler)
    server.register_synapse_handler("add", lambda data: {"sum": data["a"] + data["b"]})
    server.register_streaming_handler("stream", stream_handler)
    server.register_streaming_handler("stream_fail", stream_fail_handler)
    server.start()
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    time.sleep(0.2)
    return server, t, port


def run_suite(peer, direction, addr, rate_addr, capture=None):
    cmd = [peer, "suite", "--seed-hex", "02" * 32, "--addr", addr, "--hotkey", MINER_HOTKEY,
           "--rate-addr", rate_addr]
    if capture:
        cmd += ["--capture", capture]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
    seen = 0
    for line in proc.stdout.splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        seen += 1
        record(direction, row["case"], row["ok"], row["detail"])
    if seen == 0 or (proc.returncode != 0 and all(r["ok"] for r in RESULTS if r["direction"] == direction)):
        record(direction, "suite_process", False, f"rc={proc.returncode} stderr={proc.stderr[-400:]}")


def direction_a(peer, capture_dir):
    server, t, port = start_py_server()
    rate_server, rt, rate_port = start_py_server()
    try:
        capture = f"{capture_dir}/python_server_handshake.json" if capture_dir else None
        run_suite(peer, "A rtao-client->py-server", f"127.0.0.1:{port}", f"127.0.0.1:{rate_port}", capture)
    finally:
        server.stop()
        rate_server.stop()
        t.join(timeout=5)
        rt.join(timeout=5)


# ---------------------------------------------------------------- rtao server


class RtaoServer:
    def __init__(self, peer, *extra, mode="serve"):
        args = [peer, mode, "--seed-hex", "01" * 32, *extra]
        self.proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True)
        line = self.proc.stdout.readline().split()
        if not line or line[0] != "LISTEN":
            raise RuntimeError(f"peer did not start: {line} {self.proc.stderr.read()}")
        self.port = int(line[1])
        self.fingerprint = line[2]
        self.hotkey = line[3]
        self.axon = {"hotkey": self.hotkey, "ip": "127.0.0.1", "port": self.port}

    def stop(self):
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def py_client():
    client = Lightning(wallet_hotkey=VALIDATOR_HOTKEY)
    client.set_validator_keypair(VALIDATOR_SEED)
    return client


def active_miners(client, axon):
    """Python's initialize_connections returns None even when a handshake is
    rejected; its connection stats show whether the miner was registered."""
    stats = client.get_connection_stats()
    status = stats.get(f"connection_{axon['ip']}:{axon['port']}", "missing")
    return int(stats.get("active_miners", "0")), status


def initialized(client, axon):
    client.initialize_connections([axon])
    miners, status = active_miners(client, axon)
    return miners == 1 and status == "active", f"active_miners={miners} status={status}"


def case(direction, name, fn):
    try:
        ok, detail = fn()
    except Exception as e:  # noqa: BLE001 - every failure is a recorded case
        ok, detail = False, f"{type(e).__name__}: {e} {traceback.format_exc(limit=2)}"
    record(direction, name, ok, detail)


MATRIX = {
    "none": None,
    "true": True,
    "false": False,
    "zero": 0,
    "int": 42,
    "neg": -7,
    "float": 3.14,
    "str": "hello",
    "empty": "",
    "bytes": b"\xde\xad\xbe\xef",
    "list": [1, 2, 3],
    "nested": {"nested": {"key": "val"}},
    "u64max": 2**64 - 1,
    "i64min": -(2**63),
}


def direction_b(peer):
    d = "B py-client->rtao-server"
    server = RtaoServer(peer, "--handler-timeout-ms", "1500")
    client = py_client()
    try:
        assert server.hotkey == MINER_HOTKEY, server.hotkey
        case(d, "initialize_connections", lambda: initialized(client, server.axon))

        def query(synapse, data=None, timeout=None):
            return client.query_axon(server.axon, {"synapse_type": synapse, "data": data or {}}, timeout_secs=timeout)

        def echo():
            r = query("echo", dict(MATRIX))
            bad = {k: r["data"].get(k) for k, v in MATRIX.items() if r["data"].get(k) != v or type(r["data"].get(k)) is not type(v)}
            return r["success"] and not bad, f"bad={bad} err={r['error']}"

        case(d, "echo_matrix", echo)
        case(d, "typed_add", lambda: ((r := query("add", {"a": 2, "b": 3}))["data"] == {"sum": 5}, r))
        case(d, "caller_identity", lambda: ((r := query("whoami"))["data"].get("hotkey") == VALIDATOR_HOTKEY, r))
        for name, synapse, want in [
            ("unrecognized", "nope", "unrecognized synapse type"),
            ("handler_error", "fail", "request processing failed"),
        ]:
            case(d, name, lambda s=synapse, w=want: ((r := query(s))["success"] is False and r["error"] == w, r))
        case(d, "handler_timeout",
             lambda: ((r := query("slow", {"ms": 4000}))["success"] is False and r["error"] == "handler timed out", r))

        def stream():
            got = list(client.query_axon_stream(server.axon, {"synapse_type": "stream", "data": {}}))
            return got == [b"chunk-1", b"chunk-2", b"chunk-3"], got

        case(d, "stream", stream)

        def stream_many():
            got = list(client.query_axon_stream(server.axon, {"synapse_type": "stream", "data": {"n": 200}}))
            return len(got) == 200 and got[-1] == b"chunk-200", len(got)

        case(d, "stream_200_chunks", stream_many)

        def stream_fail():
            got = []
            try:
                for c in client.query_axon_stream(server.axon, {"synapse_type": "stream_fail", "data": {}}):
                    got.append(c)
            except RuntimeError as e:
                return len(got) == 2 and "stream processing failed" in str(e), f"{got} {e}"
            return False, f"no error, got {got}"

        case(d, "stream_fail", stream_fail)

        def stream_unknown():
            try:
                list(client.query_axon_stream(server.axon, {"synapse_type": "nope", "data": {}}))
            except RuntimeError as e:
                return "unrecognized synapse type" in str(e), str(e)
            return False, "no error"

        case(d, "stream_unrecognized", stream_unknown)
    finally:
        client.close()
        server.stop()

    denied = RtaoServer(peer, "--deny-all")
    client = py_client()
    try:
        def permit_gate():
            client.initialize_connections([denied.axon])
            miners, status = active_miners(client, denied.axon)
            if miners != 0 or status != "missing":
                return False, f"denied miner registered: active_miners={miners} status={status}"
            try:
                r = client.query_axon(denied.axon, {"synapse_type": "echo", "data": {}})
            except ConnectionError as e:
                return True, f"not registered; query raised ConnectionError: {e}"
            return False, f"query succeeded: {r}"

        case(d, "permit_gate_rejects", permit_gate)
    finally:
        client.close()
        denied.stop()


def direction_c(peer):
    server = RtaoServer(peer)
    rate = RtaoServer(peer)
    try:
        run_suite(peer, "C rtao-client->rtao-server", f"127.0.0.1:{server.port}", f"127.0.0.1:{rate.port}")
    finally:
        server.stop()
        rate.stop()


def direction_d(peer, capture_dir):
    d = "D py-client handshake capture"
    out = f"{capture_dir}/python_client_handshake.json"
    server = RtaoServer(peer, "--out", out, mode="capture")
    client = py_client()
    try:
        # The capture peer accepts only a request rtao verifies.
        case(d, "python_client_handshake_accepted", lambda: initialized(client, server.axon))
        server.proc.wait(timeout=15)
        with open(out) as f:
            fixture = json.load(f)
        case(d, "rtao_verifies_python_request", lambda: (fixture["rtao_verified"] is True, fixture["request_frame_hex"][:40]))
    finally:
        client.close()
        if server.proc.poll() is None:
            server.proc.kill()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--peer", required=True)
    ap.add_argument("--capture-dir", default=None)
    ap.add_argument("--results", default=None)
    args = ap.parse_args()
    direction_a(args.peer, args.capture_dir)
    direction_b(args.peer)
    direction_c(args.peer)
    if args.capture_dir:
        direction_d(args.peer, args.capture_dir)
    failed = [r for r in RESULTS if not r["ok"]]
    summary = {"total": len(RESULTS), "passed": len(RESULTS) - len(failed), "failed": len(failed)}
    print(json.dumps({"summary": summary}), flush=True)
    if args.results:
        with open(args.results, "w") as f:
            json.dump({"summary": summary, "results": RESULTS}, f, indent=2)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
