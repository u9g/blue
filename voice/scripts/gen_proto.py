"""Regenerates src/blue (protobuf messages + gRPC stubs) from the repo's proto/ directory.

The voice agent only needs the client-facing protocols: events, client, auth.
Run with `uv run python scripts/gen_proto.py` (or `just proto-py`).
"""

import shutil
import sys
from pathlib import Path

import grpc_tools
from grpc_tools import protoc

VOICE = Path(__file__).resolve().parents[1]
PROTO = VOICE.parent / "proto"
OUT = VOICE / "src"
FILES = [
    "blue/events/v1/events.proto",
    "blue/client/v1/client.proto",
    "blue/auth/v1/auth.proto",
]


def main() -> int:
    shutil.rmtree(OUT / "blue", ignore_errors=True)
    well_known = Path(grpc_tools.__file__).parent / "_proto"
    return protoc.main(
        [
            "grpc_tools.protoc",
            f"-I{PROTO}",
            f"-I{well_known}",
            f"--python_out={OUT}",
            f"--pyi_out={OUT}",
            f"--grpc_python_out={OUT}",
            *(str(PROTO / f) for f in FILES),
        ]
    )


if __name__ == "__main__":
    sys.exit(main())
