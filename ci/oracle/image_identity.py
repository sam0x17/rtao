"""Authenticate the built oracle's source, interpreter, binding and Cargo lock."""

from __future__ import annotations

import argparse
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tomllib

import bittensor_core

parser = argparse.ArgumentParser()
parser.add_argument("--commit", required=True)
parser.add_argument("--tag", required=True)
parser.add_argument("--input-sha256", required=True)
args = parser.parse_args()
assert sys.implementation.name == "cpython"
assert platform.python_version() == os.environ["RTAO_ORACLE_PYTHON"] == "3.12.11"
source = Path("/upstream")
assert subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip() == args.commit
lock_bytes = (source / "Cargo.lock").read_bytes()
expected = json.loads(Path("/oracle/oracle-lock.json").read_text())
assert expected["upstream_commit"] == args.commit
actual = [{"name": row["name"], "version": row["version"], "source": row.get("source")}
          for row in tomllib.loads(lock_bytes.decode())["package"]]
for package in expected["packages"]:
    assert actual.count(package) == 1, f"byte-defining package absent or duplicated: {package}"
runtime = json.loads(Path("/oracle/oracle-runtime-lock.json").read_text())
assert runtime["python"] == {"implementation": "cpython", "version": "3.12.11"}
assert runtime["uv"] == "0.8.24"
build_backends = {name: version(name) for name in runtime["build_backends"]}
assert build_backends == runtime["build_backends"]
project = tomllib.loads((source / "sdk/python/pyproject.toml").read_text())
assert project["tool"]["uv"]["sources"]["bittensor-core"]["path"] == "../bittensor-core-py"
binding = Path(bittensor_core.__file__).resolve()
assert binding.is_relative_to(source / "sdk/python/.venv") and binding.suffix == ".so"
identity = {
    "schema": 1, "parity_tag": args.tag, "parity_commit": args.commit,
    "oracle_input_sha256": args.input_sha256, "python": platform.python_version(),
    "implementation": sys.implementation.name, "uv": subprocess.check_output(["uv", "--version"], text=True).strip(),
    "rustc": subprocess.check_output(["rustc", "--version", "--verbose"], text=True).strip(),
    "binding": str(binding), "binding_sha256": hashlib.sha256(binding.read_bytes()).hexdigest(),
    "upstream_cargo_lock_sha256": hashlib.sha256(lock_bytes).hexdigest(),
    "upstream_uv_lock_sha256": hashlib.sha256((source / "sdk/python/uv.lock").read_bytes()).hexdigest(),
    "runtime_lock": runtime,
    "build_backends": build_backends,
    "build_requirements_sha256": hashlib.sha256(Path("/oracle/build-requirements.txt").read_bytes()).hexdigest(),
}
assert identity["uv"] == "uv 0.8.24"
Path("/oracle/image.json").write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n")
