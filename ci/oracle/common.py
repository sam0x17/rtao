"""Strict JSON and complete FakeSubstrate state for the CI-only oracle."""

from __future__ import annotations

from dataclasses import asdict, is_dataclass
from datetime import datetime
import hashlib
import json
import math
from pathlib import Path
from typing import Any

from bittensor.balance import Balance
from bittensor.intents._money import UNBOUNDED
from tests.harness.fake_substrate import ComposedCall, DEFAULT_STORAGE, FakeSubstrate

M2_INTENTS = (
    "set_weights", "commit_weights", "reveal_weights", "serve_axon", "serve_axon_tls",
    "serve_prometheus", "reset_axon", "transfer", "transfer_all",
)
M2_READS = (
    "block_time", "timestamp", "blocks_since_last_step", "blocks_since_last_update",
    "epoch_status", "max_weight_limit", "min_allowed_weights", "reveal_period",
    "weights_rate_limit", "uid", "commit_reveal_enabled", "mechanism_count", "weights",
    "metagraph", "token_symbols",
)


def apply_seed(fake: FakeSubstrate, sample: dict) -> None:
    """Apply reviewed inputs only; expected outputs always come from upstream."""
    for section, method in (("storage_defaults", fake.seed_default),
                            ("storage_maps", fake.seed_map),
                            ("constants", fake.seed_constant),
                            ("runtime", fake.seed_runtime)):
        for row in sample.get(section, []):
            method(row["container"], row["name"], row["value"])
    for row in sample.get("storage", []):
        fake.seed(row["container"], row["name"], row["params"], row["value"])


def jsonable(value: Any, *, seed: bool = False) -> Any:
    if isinstance(value, Balance):
        return value.rao if seed else str(value)
    if value is UNBOUNDED:
        return {"unbounded": True}
    if isinstance(value, ComposedCall):
        return call_tree(value)
    if isinstance(value, datetime):
        return value.isoformat(timespec="seconds")
    if isinstance(value, (bytes, bytearray)):
        return "0x" + bytes(value).hex()
    if is_dataclass(value) and not isinstance(value, type):
        return jsonable(asdict(value), seed=seed)
    if isinstance(value, dict):
        return {str(key): jsonable(item, seed=seed) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [jsonable(item, seed=seed) for item in value]
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("nonfinite values cannot enter the oracle JSON")
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    raise TypeError(f"unsupported oracle value {type(value).__name__}")


def call_tree(value: Any) -> Any:
    if isinstance(value, ComposedCall):
        module, function, params = value.module, value.function, value.params
    elif isinstance(value, tuple) and len(value) == 3 and isinstance(value[0], str) and isinstance(value[1], str):
        module, function, params = value
    elif isinstance(value, dict):
        return {str(key): call_tree(item) for key, item in value.items()}
    elif isinstance(value, (list, tuple)):
        return [call_tree(item) for item in value]
    else:
        return jsonable(value)
    # Timelock ciphertext and reveal round are intentionally nondeterministic.
    # Preserve the fact of their presence without claiming byte parity.
    return {"module": module, "function": function,
            "params": {key: {"excluded": "randomized-or-clock-derived"}
                       if key in {"commit", "reveal_round"} and "timelocked" in function
                       else call_tree(item) for key, item in params.items()}}


def write(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, allow_nan=False, indent=2, sort_keys=True) + "\n")


def source_path(function: Any) -> str:
    import inspect
    return str(Path(inspect.getsourcefile(function)).relative_to(Path("/upstream")))


def state(fake: FakeSubstrate) -> dict:
    def entries(mapping: dict, *, keys: bool = False) -> list:
        result = []
        for (container, name), value in sorted(mapping.items()):
            if keys:
                for params, item in value.items():
                    result.append({"container": container, "name": name, "params": jsonable(params, seed=True), "value": jsonable(item, seed=True)})
            else:
                if callable(value):
                    raise TypeError(f"runtime seed {container}.{name} is callable; provide explicit parameter cases")
                result.append({"container": container, "name": name, "value": jsonable(value, seed=True)})
        return result
    defaults = {**DEFAULT_STORAGE, **fake._storage_defaults}
    return {"storage": entries(fake._storage, keys=True), "storage_defaults": entries(defaults),
            "storage_maps": entries(fake._maps), "constants": entries(fake._constants),
            "runtime": entries(fake._runtime), "fee_rao": fake.fee.rao,
            "weight": jsonable(fake.weight, seed=True), "block": fake.block,
            "finalized": fake.finalized, "spec_version": fake.runtime_spec_version,
            "token_symbols": jsonable(fake.token_symbols), "nonces": dict(fake._nonces),
            "events": jsonable(fake._events, seed=True)}


class RecordingSubstrate(FakeSubstrate):
    def __init__(self) -> None:
        super().__init__()
        self.touched: list[dict] = []

    def touch(self, kind: str, container: str, name: str, block_hash: str | None = None) -> None:
        self.touched.append({"kind": kind, "container": container, "name": name, "block_hash": block_hash})

    async def query(self, module, storage_function, params=None, block_hash=None):
        if not getattr(self, "_inside_batch", False):
            self.touch("storage", module, storage_function, block_hash)
        return await super().query(module, storage_function, params, block_hash)

    async def query_map(self, module, storage_function, params=None, block_hash=None):
        self.touch("storage", module, storage_function, block_hash)
        return await super().query_map(module, storage_function, params, block_hash)

    async def query_batch(self, module, storage_function, param_sets, block_hash=None):
        self.touch("storage", module, storage_function, block_hash)
        self._inside_batch = True
        try:
            return await super().query_batch(module, storage_function, param_sets, block_hash)
        finally:
            self._inside_batch = False

    async def map_has_entries(self, module, storage_function, params, block_hash=None):
        self.touch("storage", module, storage_function, block_hash)
        return await super().map_has_entries(module, storage_function, params, block_hash)

    async def runtime_call(self, api, method, params, block_hash=None):
        self.touch("runtime", api, method, block_hash)
        return await super().runtime_call(api, method, params, block_hash)

    async def constant(self, module, name, block_hash=None):
        self.touch("constant", module, name)
        return await super().constant(module, name)


def manifest(directory: Path, identity: dict, scope: str) -> dict:
    files = {}
    for path in sorted(directory.rglob("*.json")):
        if path.name == "manifest.json":
            continue
        files[str(path.relative_to(directory))] = hashlib.sha256(path.read_bytes()).hexdigest()
    return {"schema": 1, "scope": scope, "image": identity, "files": files}
