"""Require initialized metadata and drand readiness in the hosted record lane."""

from __future__ import annotations

import asyncio
import os
import sys

from bittensor._transport.interface import SubstrateConnection

assert os.environ.get("GITHUB_ACTIONS") == "true"
assert os.environ.get("RTAO_PYTHON_LANE") == "record"
assert sys.version_info[:3] == (3, 12, 11)


async def main() -> None:
    connection = SubstrateConnection("ws://127.0.0.1:9944", ss58_format=42)
    try:
        await connection.initialize()
        for _ in range(30):
            value = await connection.query("Drand", "LastStoredRound")
            if int(value or 0) > 0:
                return
            await asyncio.sleep(1)
        raise RuntimeError("LocalnetError::DrandUnreachable")
    finally:
        await connection.close()


asyncio.run(main())
