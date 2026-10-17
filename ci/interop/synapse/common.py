"""Shared bittensor==10.5.0 test synapses for the rtao-compat-synapse interop harness.

Imported by py_axon.py, py_dendrite.py and capture_fixtures.py. Must be run
with an interpreter that has exactly requirements.txt installed (bittensor 10.5.0,
no torch); see run.sh.
"""

import asyncio
from typing import ClassVar, Optional, Tuple

import bittensor as bt
import bittensor.core.tensor as bt_tensor
import bittensor.utils.networking as networking
from bittensor_wallet import Keypair

# No external IP lookups (Axon.__init__ and Dendrite.__init__ call this).
networking.get_external_ip = lambda: "203.0.113.7"
import bittensor.core.dendrite as bt_dendrite  # noqa: E402

bt_dendrite.networking.get_external_ip = networking.get_external_ip

# bittensor 10.5.0's dtype table imports torch on first lookup, so no Tensor
# validates without torch. Mark torch dtypes as already added; numpy dtypes
# then work. (Finding recorded in the interop report.)
bt_tensor.dtypes.torch = True

ALICE = Keypair.create_from_uri("//Alice")
BOB = Keypair.create_from_uri("//Bob")
CHARLIE = Keypair.create_from_uri("//Charlie")


class MockWallet:
    def __init__(self, keypair):
        self.hotkey = keypair
        self.coldkeypub = keypair
        self.coldkey = keypair


class Mixed(bt.Synapse):
    i: int = 0
    f: float = 0.0
    s: str = ""
    l: list[str] = []
    d: dict[str, list[float]] = {}
    n: Optional[int] = None
    out: Optional[str] = None
    required_hash_fields: ClassVar[tuple[str, ...]] = ("i", "f", "s", "l", "d", "n")


class Req(bt.Synapse):
    rs: str
    ri: int
    rl: list[int]
    rd: dict[str, int]
    rf: float
    rb: bool
    echo: Optional[str] = None
    required_hash_fields: ClassVar[tuple[str, ...]] = ("rs", "ri", "rl", "rd", "rf", "rb")


class LegacyHashed(bt.Synapse):
    a: int = 0
    b: int = 0
    d: Optional[list[str]] = None
    required_hash_fields: Optional[list[str]] = ["b", "a", "d"]


class Tens(bt.Synapse):
    t: Optional[bt.Tensor] = None
    total: Optional[float] = None
    required_hash_fields: ClassVar[tuple[str, ...]] = ("t",)


class Deny(bt.Synapse):
    pass


class Status404(bt.Synapse):
    pass


class Boom(bt.Synapse):
    pass


class Slow(bt.Synapse):
    delay: float = 0.0


class NanResp(bt.Synapse):
    v: Optional[float] = None


class WhoAmI(bt.Synapse):
    seen: Optional[str] = None


class NanReq(bt.Synapse):
    v: float = 0.0
    vs: list[float] = []
    seen: Optional[str] = None
    required_hash_fields: ClassVar[tuple[str, ...]] = ("v", "vs")


class Echo(bt.StreamingSynapse):
    text: str = ""
    completion: str = ""

    async def process_streaming_response(self, response):
        async for chunk in response.content.iter_any():
            self.completion += chunk.decode()
            yield chunk

    def extract_response_json(self, response) -> dict:
        return {"text": self.text, "completion": self.completion}

    def deserialize(self):
        return self


def build_axon(port: int) -> bt.Axon:
    axon = bt.Axon(
        wallet=MockWallet(BOB), port=port, ip="127.0.0.1", external_ip="127.0.0.1"
    )

    def fwd_mixed(synapse: Mixed) -> Mixed:
        synapse.out = f"{synapse.i}|{synapse.s}|{len(synapse.l)}"
        return synapse

    def fwd_req(synapse: Req) -> Req:
        synapse.echo = synapse.rs
        return synapse

    def fwd_legacy(synapse: LegacyHashed) -> LegacyHashed:
        return synapse

    def fwd_tens(synapse: Tens) -> Tens:
        synapse.total = float(synapse.t.deserialize().sum()) if synapse.t else None
        return synapse

    def fwd_deny(synapse: Deny) -> Deny:
        return synapse

    def bl_deny(synapse: Deny) -> Tuple[bool, str]:
        return True, "denied by test"

    def fwd_404(synapse: Status404) -> Status404:
        synapse.axon.status_code = 404
        return synapse

    def fwd_boom(synapse: Boom) -> Boom:
        raise RuntimeError("boom")

    async def fwd_slow(synapse: Slow) -> Slow:
        await asyncio.sleep(synapse.delay)
        return synapse

    def fwd_nan(synapse: NanResp) -> NanResp:
        synapse.v = float("nan")
        return synapse

    def fwd_who(synapse: WhoAmI) -> WhoAmI:
        synapse.seen = synapse.dendrite.hotkey
        return synapse

    def fwd_echo(synapse: Echo) -> Echo.BTStreamingResponse:
        async def streamer(send):
            for _ in range(3):
                await send({"type": "http.response.body", "body": b"tok\n", "more_body": True})

        return synapse.create_streaming_response(streamer)

    def fwd_nanreq(synapse: NanReq) -> NanReq:
        synapse.seen = repr((synapse.v, synapse.vs))
        synapse.v = 0.0
        synapse.vs = []
        return synapse

    axon.attach(forward_fn=fwd_mixed)
    axon.attach(forward_fn=fwd_req)
    axon.attach(forward_fn=fwd_legacy)
    axon.attach(forward_fn=fwd_tens)
    axon.attach(forward_fn=fwd_deny, blacklist_fn=bl_deny)
    axon.attach(forward_fn=fwd_404)
    axon.attach(forward_fn=fwd_boom)
    axon.attach(forward_fn=fwd_slow)
    axon.attach(forward_fn=fwd_nan)
    axon.attach(forward_fn=fwd_who)
    axon.attach(forward_fn=fwd_echo)
    axon.attach(forward_fn=fwd_nanreq)
    return axon
