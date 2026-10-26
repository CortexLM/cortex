"""QA: `hypertrain-miner run --config tests/fixtures/miner.toml --rounds 2` against a real local
challenge server (uvicorn), then a fresh-journal resend of round 0 that gets 409 on Commit.

Usage: uv run --frozen --extra trainer python tests/miner/qa_cli.py  (exit 0 = both scenarios pass)
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

import httpx
import uvicorn

import hypertrain.trainer  # noqa: F401

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

from miner_harness import SEEDS, Operator, manifest, write_keyfile  # noqa: E402

from challenge.conftest import (  # noqa: E402
    ADMIN,
    COORD_SEED,
    INTERNAL,
    WORKER,
    Master,
    make_config,
    verify_fixture,
)
from hypertrain.challenge.app import create_app  # noqa: E402
from hypertrain.data.shards import ShardWriter, finalize  # noqa: E402
from hypertrain.protocol.keys import Keypair  # noqa: E402

PORT = 18765
QA = ROOT / ".qa-miner"


def build_shards(out: Path) -> dict[str, object]:
    import numpy as np

    w = ShardWriter(out, seq_len=16, samples_per_shard=32, max_shards=3)
    rng = np.random.default_rng(7)
    while not w.full:
        w.add(rng.integers(0, 64, 17, dtype=np.uint32).astype("<u4"))
    m, _ = finalize(w, {"name": "qa-random", "sha256": "0" * 64})
    (out / "manifest.json").write_text(m.to_json())
    return {
        "merkle_root": m.merkle_root,
        "depth": m.depth,
        "n_samples": m.n_samples,
        "shard_sha256_root": m.shard_sha256_root,
    }


class Pacer(threading.Thread):
    """Operator relay: advances drand rounds, but holds each deadline until the rostered miner
    reached the matching state (bounded wait), then aggregates after d_upload."""

    def __init__(self, op: Operator, hotkey: str, last_w: int) -> None:
        super().__init__(daemon=True)
        self.op, self.hk, self.last_w, self.stop = op, hotkey, last_w, threading.Event()

    def _status(self, w: int) -> str | None:
        v = self.op.round(w)
        return (
            next((m["status"] for m in v["miners"] if m["hotkey"] == self.hk), None) if v else None
        )

    def run(self) -> None:
        while not self.stop.is_set():
            w = max(self.op.theta)
            b = self.op.round(w)["round_open"]["body"]  # type: ignore[index]
            hold = {b["d_commit"] - 1: {"COMMITTED", "UPLOADED"}, b["d_upload"] - 1: {"UPLOADED"}}
            want = hold.get(self.op.d)
            deadline = time.monotonic() + 120
            while want and self._status(w) not in want and time.monotonic() < deadline:
                if self.stop.wait(0.05):
                    return
            if self.op.d >= b["d_upload"]:
                if w >= self.last_w:
                    self.stop.wait(0.05)
                    continue
                self.op.aggregate(w)
            self.op.push(self.op.d + 1)


def main() -> int:
    shutil.rmtree(QA, ignore_errors=True)
    (QA / "states").mkdir(parents=True)
    secrets = QA / "secrets"
    secrets.mkdir()
    for name, value in (
        ("internal.token", INTERNAL),
        ("admin.token", ADMIN),
        ("worker.token", WORKER),
        ("coord.key", COORD_SEED.hex()),
    ):
        (secrets / name).write_text(value)
    dataset = build_shards(QA / "data")
    m = manifest(dataset=dataset)
    master = Master()
    kp = Keypair(SEEDS[0])
    master.registered.add(kp.ss58)
    write_keyfile(QA / "miner.key", SEEDS[0])
    app = create_app(
        make_config(QA / "state", secrets),
        transport=httpx.MockTransport(master.handler),
        verify_beacon=verify_fixture,
    )
    server = uvicorn.Server(uvicorn.Config(app, host="127.0.0.1", port=PORT, log_level="warning"))
    server_thread = threading.Thread(target=server.run, daemon=True)
    server_thread.start()
    while not server.started:
        time.sleep(0.05)
    client = httpx.Client(base_url=f"http://127.0.0.1:{PORT}", timeout=60)
    op = Operator(client, app.state.store, m, QA / "states")
    op.start([kp.ss58])
    pacer = Pacer(op, kp.ss58, last_w=1)
    pacer.start()
    env = {**os.environ, "HYPERTRAIN_MINER_RUN_ID": op.run_id}
    cmd = ["hypertrain-miner", "run", "--config", "tests/fixtures/miner.toml", "--rounds", "2"]
    print("$", " ".join(cmd), flush=True)
    happy = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=900)
    print(happy.stderr + happy.stdout, f"exit={happy.returncode}", flush=True)
    status = {w: next(x["status"] for x in op.round(w)["miners"]) for w in (0, 1)}  # type: ignore[index]
    print("server statuses", json.dumps(status), flush=True)
    ok_happy = happy.returncode == 0 and status == {0: "UPLOADED", 1: "UPLOADED"}

    env2 = {**env, "HYPERTRAIN_MINER_WORKDIR": str(QA / "work-fresh")}
    cmd2 = [*cmd[:-1], "1", "--start", "0"]
    print("$", " ".join(cmd2), "(fresh journal, round 0 already UPLOADED)", flush=True)
    dup = subprocess.run(cmd2, cwd=ROOT, env=env2, capture_output=True, text=True, timeout=900)
    print(dup.stderr + dup.stdout, f"exit={dup.returncode}", flush=True)
    journal = next((QA / "work-fresh").rglob("journal.jsonl")).read_text()
    ok_dup = (
        dup.returncode == 0
        and "commit 409 for round 0" in dup.stderr
        and '"via": "409"' in journal
        and "upload 409" not in dup.stderr
        and next(x["status"] for x in op.round(0)["miners"]) == "UPLOADED"  # type: ignore[index]
    )
    # Join both threads before returning: daemon threads alive at interpreter shutdown abort
    # with "terminate called without an active exception" (exit 134).
    pacer.stop.set()
    pacer.join(timeout=60)
    server.should_exit = True
    server_thread.join(timeout=60)
    client.close()
    print(json.dumps({"happy": ok_happy, "duplicate_commit_409": ok_dup}), flush=True)
    return 0 if ok_happy and ok_dup else 1


if __name__ == "__main__":
    sys.exit(main())
