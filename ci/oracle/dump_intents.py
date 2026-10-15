"""Plan the registry's samples, recording every executed seed state."""

from __future__ import annotations

from pathlib import Path

from bittensor.client import Client
from bittensor.intents import REGISTRY, build
from bittensor.intents.base import BuiltCall
from tests.harness.samples import INTENT_SAMPLES, dev_wallet

from common import M2_INTENTS, RecordingSubstrate, apply_seed, call_tree, jsonable, source_path, state, write


async def dump(directory: Path, scope: str, extras: dict) -> None:
    assert set(REGISTRY) == set(INTENT_SAMPLES), "intent registry/sample coverage changed"
    names = M2_INTENTS if scope == "M2" else sorted(REGISTRY)
    vectors = []
    for name in names:
        samples = [{"id": "upstream", "args": INTENT_SAMPLES[name]}] + extras.get("intents", {}).get(name, [])
        seeds = []
        for sample in samples:
            fake = RecordingSubstrate()
            fake.seed_runtime("StakeInfoRuntimeApi", "get_stake_info_for_coldkey", [])
            wallet = dev_wallet()
            # This is the table test's explicit seed for the root-claim plans.
            if name in ("claim_root", "claim_root_with_hotkey"):
                fake.seed("System", "Account", [wallet.coldkey.ss58_address], {"data": {"free": 10**9}})
            apply_seed(fake, sample)
            before = state(fake)
            intent = build(name, sample["args"])
            built = await intent.build(fake, wallet)
            call = built.call if isinstance(built, BuiltCall) else built
            client = Client("local", substrate=fake)
            plan = await client.plan(intent, wallet)
            signer = wallet.hotkey if intent.signer == "hotkey" else wallet.coldkey
            preflight = await intent.preflight(fake, signer.ss58_address, signer.ss58_address, call=call)
            extras_value = dict(built.extras) if isinstance(built, BuiltCall) else {}
            if "reveal_round" in extras_value:
                extras_value["reveal_round"] = {"excluded": "randomized-or-clock-derived"}
            vectors.append({"name": name, "sample": sample["id"], "args": sample["args"],
                            "source": source_path(type(intent)), "schema": type(intent).json_schema(),
                            "intent": intent.to_dict(), "call": call_tree(call),
                            "preflight": jsonable(preflight), "plan": jsonable(plan.to_dict()),
                            "fee_rao": None if plan.fee is None else plan.fee.rao,
                            "spend": jsonable(intent.spend()), "extras": jsonable(extras_value),
                            "touched": fake.touched})
            seeds.append({"sample": sample["id"], "state": before})
        write(directory / "seeds" / f"{name}.json", {"schema": 1, "kind": "intent", "name": name, "samples": seeds})
    write(directory / "intents.json", {"schema": 1, "scope": scope, "vectors": vectors})
