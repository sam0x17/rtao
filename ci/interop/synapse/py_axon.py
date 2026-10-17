"""Serve the bittensor==10.5.0 interop axon on 127.0.0.1:<port> until killed."""

import asyncio
import sys
import warnings

import uvicorn

sys.path.insert(0, sys.argv[2])
warnings.simplefilter("ignore")
import common  # noqa: E402


async def main(port: int) -> None:
    axon = common.build_axon(port)
    # Same app and middleware as Axon.start(), bound to loopback only.
    config = uvicorn.Config(axon.app, host="127.0.0.1", port=port, log_level="critical", loop="asyncio")
    server = uvicorn.Server(config)
    task = asyncio.create_task(server.serve())
    while not server.started:
        await asyncio.sleep(0.05)
    print(f"READY {port} {common.BOB.ss58_address}", flush=True)
    await task


if __name__ == "__main__":
    asyncio.run(main(int(sys.argv[1])))
