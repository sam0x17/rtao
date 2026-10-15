"""Run upstream's current-spec recorders against a CI-owned localnet only."""

from __future__ import annotations

import argparse
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import hashlib

assert os.environ.get("GITHUB_ACTIONS") == "true"
assert os.environ.get("RTAO_PYTHON_LANE") == "record"
assert platform.python_version() == "3.12.11"
parser = argparse.ArgumentParser()
parser.add_argument("--output", default="/output")
parser.add_argument("--spec", required=True, type=int)
args = parser.parse_args()
assert args.spec > 0
source = Path("/upstream/sdk/python")
endpoint = "ws://127.0.0.1:9944"
# record_golden reads sys.argv on import. Keep its localhost submission guard
# active, rather than setting RECORD_GOLDEN_ALLOW_REMOTE.
assert os.environ.get("RECORD_GOLDEN_ALLOW_REMOTE") is None
sys.argv = [str(source / "scripts/record_golden.py"), endpoint]
spec = importlib.util.spec_from_file_location("upstream_record_golden", source / "scripts/record_golden.py")
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)


async def record() -> None:
    connection = module.SubstrateConnection(endpoint, ss58_format=module.SS58_FORMAT)
    dropped = []
    try:
        await connection.initialize()
        codec = await connection._runtimes.codec_at(None)
        assert codec.spec_version == args.spec, "localnet spec does not match the parity pin"
        ir = codec.metadata_ir()
        pallets = {pallet.name: pallet for pallet in ir.pallets}
        apis = {api.name: set(api.methods) for api in ir.runtime_apis}
        for family in ["STORAGE_KEY_CASES", "STORAGE_VALUE_CASES", "QUERY_MAP_CASES", "CALL_CASES", "CONSTANT_CASES", "RUNTIME_CALL_CASES"]:
            kept = []
            for case in getattr(module, family):
                container, name = case[:2]
                pallet = pallets.get(container)
                if family == "RUNTIME_CALL_CASES":
                    exists = name in apis.get(container, set())
                elif pallet is None:
                    exists = False
                elif family == "CALL_CASES":
                    exists = any(call.name == name for call in pallet.calls)
                elif family == "CONSTANT_CASES":
                    exists = name in pallet.constants
                else:
                    exists = any(storage.name == name for storage in pallet.storage)
                if exists:
                    kept.append(case)
                else:
                    dropped.append({"family": family, "container": container, "name": name,
                                    "reason": "absent from live metadata"})
            setattr(module, family, kept)
        golden = await module.record(connection)
        assert golden["network"]["spec_version"] == args.spec
    finally:
        await connection.close()
    module.OUT.parent.mkdir(parents=True, exist_ok=True)
    module.OUT.write_text(json.dumps(golden, indent=1, allow_nan=False))
    subprocess.run([sys.executable, str(source / "scripts/record_shape_corpus.py")], cwd=source, check=True)
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    assert not any(output.iterdir()), "recording output must be fresh"
    shutil.copyfile(module.OUT, output / "golden.json")
    shutil.copytree(source / "tests/fixtures/shape_corpus", output / "shape_corpus")
    (output / "dropped.json").write_text(json.dumps(dropped, indent=2, sort_keys=True) + "\n")
    identity = json.loads(Path("/oracle/image.json").read_text())
    identity["execution_image"] = os.environ["RTAO_ORACLE_EXECUTION_IMAGE"]
    identity["recorded_spec"] = args.spec
    (output / "image.json").write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n")
    files = {str(path.relative_to(output)): hashlib.sha256(path.read_bytes()).hexdigest()
             for path in sorted(output.rglob("*")) if path.is_file()}
    (output / "manifest.json").write_text(json.dumps({"schema": 1, "spec": args.spec,
          "image": identity, "files": files}, indent=2, sort_keys=True) + "\n")


asyncio.run(record())
