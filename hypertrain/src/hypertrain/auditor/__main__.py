"""`python -m hypertrain.auditor --api URL --run-id HEX --token-file F --keyfile F --data-dir D`.

Leases audit jobs for ONE run, replays them on the local verified shards and posts verdicts.
Jobs of any other run are handed back (retry) so they are never replayed on the wrong data.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import sys
from pathlib import Path
from typing import Any

import httpx

import hypertrain.trainer  # noqa: F401  (determinism before torch)
from hypertrain.auditor.worker import Auditor, HttpApi
from hypertrain.miner.cli import shard_sampler
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol.messages import ReplayEnv, RunManifest


class RunScopedApi(HttpApi):
    def __init__(self, client: httpx.Client, token: str, base: str, run_id: str) -> None:
        super().__init__(client, token, base)
        self.run_id = run_id

    def lease(self) -> dict[str, Any] | None:
        raw = super().lease()
        if raw is not None and RunManifest.model_validate(raw["manifest"]).run_id() != self.run_id:
            self.fail(str(raw["id"]), str(raw["lease"]), "auditor serves another run", True)
            return None
        return raw


def replay_env(image_digest: str, device: str) -> ReplayEnv:
    import torch

    if device == "cpu":
        return ReplayEnv(
            image_digest=image_digest,
            driver="cpu",
            gpu_uuid_sha256=hashlib.sha256(b"cpu").hexdigest(),
            sm_count=1,
        )
    props = torch.cuda.get_device_properties(0)
    version = Path("/proc/driver/nvidia/version")
    parts = version.read_text().split() if version.is_file() else []
    driver = next((p for p in parts if p[:1].isdigit() and "." in p), "unknown")
    return ReplayEnv(
        image_digest=image_digest,
        driver=driver,
        gpu_uuid_sha256=hashlib.sha256(str(props.uuid).encode()).hexdigest(),
        sm_count=props.multi_processor_count,
    )


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m hypertrain.auditor")
    p.add_argument("--api", required=True)
    p.add_argument("--run-id", required=True)
    p.add_argument("--token-file", type=Path, required=True)
    p.add_argument("--keyfile", type=Path, required=True)
    p.add_argument("--data-dir", type=Path, required=True)
    p.add_argument("--device", choices=["cuda", "cpu"], default="cuda")
    p.add_argument("--image-digest", default=os.environ.get("HYPERTRAIN_IMAGE_DIGEST", ""))
    p.add_argument("--once", action="store_true", help="settle at most one job, then exit")
    a = p.parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    if a.device == "cuda":
        import torch

        torch.set_default_device("cuda")
    client = httpx.Client()
    base = a.api.rstrip("/")
    r = client.get(f"{base}/v1/runs/{a.run_id}", timeout=60)
    r.raise_for_status()
    manifest = RunManifest.model_validate(r.json()["manifest_envelope"]["body"])
    if manifest.run_id() != a.run_id:
        logging.error("served manifest hashes to another run_id")
        return 2
    api = RunScopedApi(client, a.token_file.read_text().strip(), base, a.run_id)
    auditor = Auditor(
        api,
        load_keyfile(a.keyfile),
        replay_env(a.image_digest, a.device),
        shard_sampler(a.data_dir, manifest),
    )
    while True:
        result = auditor.run_once()
        if result is not None:
            print(json.dumps({"result": result}), flush=True)
        if a.once or result is None:
            return 0


if __name__ == "__main__":
    sys.exit(main())
