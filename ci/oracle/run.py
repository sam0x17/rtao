"""Atomically produce oracle outputs in a designated hosted Python lane."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import platform
import shutil
import sys
import tempfile

from common import manifest, write
from dump_intents import dump as intents
from dump_reads import dump as reads
from dump_pure import dump as pure
from dump_cli import dump as cli

parser = argparse.ArgumentParser()
parser.add_argument("--output", default="/output")
parser.add_argument("--scope", choices=["M2", "all"], default="M2")
parser.add_argument("--inputs", default="/inputs")
args = parser.parse_args()
assert os.environ.get("RTAO_PYTHON_LANE") == "oracle"
assert os.environ.get("GITHUB_ACTIONS") == "true"
assert sys.implementation.name == "cpython" and platform.python_version() == "3.12.11"
identity = json.loads(Path("/oracle/image.json").read_text())
identity["execution_image"] = os.environ["RTAO_ORACLE_EXECUTION_IMAGE"]
inputs = Path(args.inputs)
extras = json.loads((inputs / "samples_extra.json").read_text())
assert extras["schema"] == 1 and set(extras) <= {"schema", "intents", "reads"}
output = Path(args.output)
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix="oracle-", dir=output) as temporary:
    staging = Path(temporary)
    asyncio.run(intents(staging, args.scope, extras))
    asyncio.run(reads(staging, args.scope, extras))
    pure(staging)
    cli(staging)
    write(staging / "image.json", identity)
    write(staging / "samples_extra.json", extras)
    write(staging / "lock.json", json.loads(Path("/oracle/oracle-lock.json").read_text()))
    write(staging / "runtime-lock.json", json.loads(Path("/oracle/oracle-runtime-lock.json").read_text()))
    write(staging / "manifest.json", manifest(staging, identity, args.scope))
    for path in sorted(staging.iterdir()):
        destination = output / path.name
        assert not destination.exists(), "oracle output must be a fresh task directory"
        shutil.move(str(path), destination)
