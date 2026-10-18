"""Print the compatibility lane matrix: one localnet lane per distinct digest.

Validation needs e2e and interop coverage on every distinct digest in the
pinned localnet image matrix. Lanes that share a digest run the same image,
so the first lane in preference order represents it.
"""

import json
import re
import sys
import tomllib

PREFERENCE = ("pinned", "testnet", "devnet", "main", "mainnet")


def main() -> None:
    with open(sys.argv[1], "rb") as handle:
        localnet = tomllib.load(handle)["localnet"]
    lanes = [name for name, value in localnet.items() if isinstance(value, dict)]
    if sorted(lanes) != sorted(PREFERENCE):
        sys.exit(f"unexpected localnet lanes {sorted(lanes)}")
    matrix = []
    seen = set()
    for lane in PREFERENCE:
        digest = localnet[lane]["digest"]
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
            sys.exit(f"invalid {lane} digest")
        if digest not in seen:
            seen.add(digest)
            matrix.append({"lane": lane, "digest": digest})
    print("lanes=" + json.dumps(matrix, separators=(",", ":")))


if __name__ == "__main__":
    main()
