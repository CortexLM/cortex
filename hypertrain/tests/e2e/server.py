"""Challenge app process for the e2e suite: real uvicorn on 127.0.0.1, fixture beacon check,
metagraph served from a JSON file. Prints `READY <port>` once the app has started.

usage: python server.py STATE_DIR SECRETS_DIR REGISTRY_JSON VEST_ROUNDS
"""

from __future__ import annotations

import json
import socket
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import common  # noqa: E402,I001  (imports hypertrain.trainer first: determinism before torch)
import httpx  # noqa: E402
import uvicorn  # noqa: E402

from hypertrain.challenge.app import Config, create_app  # noqa: E402
from hypertrain.ledger import Params  # noqa: E402
from hypertrain.protocol.messages import QUICKNET_GENESIS  # noqa: E402


def main() -> None:
    state, secrets, registry, vest = (
        Path(sys.argv[1]),
        Path(sys.argv[2]),
        Path(sys.argv[3]),
        int(sys.argv[4]),
    )

    def metagraph(request: httpx.Request) -> httpx.Response:
        hotkeys = json.loads(registry.read_text())
        return httpx.Response(200, json={"hotkeys": {k: i for i, k in enumerate(hotkeys)}})

    config = Config(
        common.SLUG,
        state,
        "http://master.test",
        secrets / "internal.token",
        secrets / "admin.token",
        secrets / "worker.token",
        secrets / "coord.key",
        common.OWNER.ss58,
        Params(common.SLUG, QUICKNET_GENESIS, common.EPOCH_SECONDS, 1, vest),
    )
    app = create_app(
        config, transport=httpx.MockTransport(metagraph), verify_beacon=common.verify_fixture
    )
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    sock.listen(64)
    port = sock.getsockname()[1]
    print(f"READY {port}", flush=True)  # socket already listening; /health gates readiness
    uvicorn.Server(uvicorn.Config(app, log_level="warning")).run(sockets=[sock])


if __name__ == "__main__":
    main()
