"""Dump head and pinned reads through the upstream CLI's JSON conversion."""

from __future__ import annotations

from pathlib import Path

from bittensor.client import Client
from bittensor.cli.query import _jsonable
from bittensor.reads import REGISTRY
from tests.harness.samples import READ_SAMPLES
from tests.unit.test_reads_table import seeded_substrate

from common import M2_READS, RecordingSubstrate, apply_seed, source_path, state, write


async def dump(directory: Path, scope: str, extras: dict) -> None:
    assert set(REGISTRY) == set(READ_SAMPLES), "read registry/sample coverage changed"
    names = M2_READS if scope == "M2" else sorted(REGISTRY)
    vectors = []
    for name in names:
        samples = [{"id": "upstream", "params": READ_SAMPLES[name]}] + extras.get("reads", {}).get(name, [])
        seeds = []
        for sample in samples:
            for block in (None, 90):
                fake = RecordingSubstrate()
                # Reuse the table test's structured runtime seeds, rather than
                # copying constants or imperative Python seed bodies.
                baseline = seeded_substrate()
                fake.__dict__.update(baseline.__dict__)
                fake.touched = []
                apply_seed(fake, sample)
                before = state(fake)
                client = Client("local", substrate=fake)
                view = client if block is None else await client.at(block)
                result = await view.read(name, **sample["params"])
                spec = REGISTRY[name]
                import inspect
                function = inspect.unwrap(spec.fetch)
                vectors.append({"name": name, "sample": sample["id"], "block": block,
                                "params": sample["params"], "source": source_path(function),
                                "result": _jsonable(result), "touched": fake.touched})
                seeds.append({"sample": sample["id"], "block": block, "state": before})
        write(directory / "seeds" / f"{name}.json", {"schema": 1, "kind": "read", "name": name, "samples": seeds})
    write(directory / "reads.json", {"schema": 1, "scope": scope, "vectors": vectors})
