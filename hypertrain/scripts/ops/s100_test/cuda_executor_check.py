"""Probe the hosts of a cuda-executor config the way a job would, and run nothing on them.

    docker exec -i hypertrain-test-challenge python -I - CONFIG [sha256:<gpu image digest>] \
        < cuda_executor_check.py

Per host it opens the real ssh transport (the pinned host key, the key file by path, and
`sudo -n docker` if configured), reads nvidia-smi and the image inventory, and prints one JSON
line: reachable or why not, the GPUs (uuid, name, driver, capability, MiB used), the GPUs a
compute app holds, whether the image is present, and the machine identity the executor would
record. Nothing secret is read or printed. Exit 0 only if every host answered with at least one
GPU. The driver and capability it prints are what the TEST manifest's driver_allowlist, sm_count
and pinned gpu class must match (B300: 595.91.07, 10.3 = sm_103, 148 SMs).
"""

from __future__ import annotations

import json
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from hypertrain.gpu_ops import cuda_executor as ce
from hypertrain.miner.island_launch import IslandFailure

PLACEHOLDER_DIGEST = "sha256:" + "0" * 64


def probe(config_path: Path, digest: str = PLACEHOLDER_DIGEST) -> list[dict[str, Any]]:
    config = ce.ExecutorConfig.model_validate_json(config_path.read_bytes())
    executor = ce.Executor(config)
    image = f"{config.image_repository}@{digest}"
    rows = []
    for host in config.hosts:
        row: dict[str, Any] = {"label": host.label, "address": host.address, "port": host.port}
        with tempfile.TemporaryDirectory(prefix="check-") as work:
            try:
                snapshot = executor._snapshot(
                    host.label, image, time.time() + 120, None, Path(work)
                )
                row.update(
                    status="ok",
                    machine_id=executor._admit(host.label, snapshot),
                    image_present=image in (snapshot.repo_digests or ()),
                    repo_digests=list(snapshot.repo_digests or ()),
                    gpus=[
                        {
                            "uuid": g.uuid,
                            "name": g.name,
                            "driver": g.driver,
                            "capability": list(g.capability),
                            "memory_used_mib": g.memory_mib,
                        }
                        for g in snapshot.gpus
                    ],
                    busy=sorted(snapshot.busy),
                    idle=sorted(
                        g.uuid
                        for g in snapshot.gpus
                        if g.uuid not in snapshot.busy
                        and g.memory_mib <= config.idle_gpu_memory_mib
                    ),
                )
            except ce.HostRefused as error:
                row.update(status="refused", detail=str(error))
            except IslandFailure as error:
                row.update(status="unreachable", detail=str(error))
        rows.append(row)
    return rows


def main(argv: list[str]) -> int:
    if not 1 <= len(argv) <= 2:
        raise SystemExit("usage: CONFIG [sha256:<gpu image digest>]")
    rows = probe(Path(argv[0]), *argv[1:])
    for row in rows:
        print(json.dumps(row, sort_keys=True))
    return 0 if all(r["status"] == "ok" and r["gpus"] for r in rows) else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
