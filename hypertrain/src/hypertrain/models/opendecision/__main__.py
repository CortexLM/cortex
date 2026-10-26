"""`python -m hypertrain.models.opendecision {warmstart,eval}` (design B2, A19, B3 metrics)."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

from hypertrain.auditor.replay import pack_state, unpack_state
from hypertrain.datasets.shards16 import ShardSet16Manifest, U16ShardSamples, verify_shards16
from hypertrain.models.opendecision import check_manifest, eval_holdout, extend_params
from hypertrain.protocol.messages import RunManifest
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig


def _manifest(path: Path) -> RunManifest:
    raw = json.loads(path.read_text())
    m = RunManifest.model_validate(raw.get("body", raw))
    check_manifest(m)
    return m


def warmstart(state: Path, manifest: Path, out: Path) -> str:
    """Previous-stage theta + deterministic init of the new names -> ``out/<state hash>``."""
    cfg = TrainConfig.from_manifest(_manifest(manifest).body()).model
    theta, _ = unpack_state(state.read_bytes())
    full = extend_params(theta, cfg)
    h = state_hash(full)
    out.mkdir(parents=True, exist_ok=True)
    (out / h).write_bytes(pack_state(full))
    return h


def evaluate(manifest: Path, state: Path, holdout: Path, w: int, metrics: Path) -> float:
    m = _manifest(manifest)
    cfg = TrainConfig.from_manifest(m.body()).model
    hm = ShardSet16Manifest.from_json((holdout / "manifest.json").read_text())
    if errors := verify_shards16(holdout, hm):
        raise SystemExit(f"holdout verification failed: {errors[:3]}")
    if hm.seq_len != cfg.seq_len:
        raise SystemExit(f"holdout seq_len {hm.seq_len} != run seq_len {cfg.seq_len}")
    theta, _ = unpack_state(state.read_bytes())
    samples = U16ShardSamples(holdout, hm.n_shards, hm.samples_per_shard, hm.seq_len)
    value = eval_holdout(cfg, theta, samples.row, range(hm.n_samples))
    metrics.parent.mkdir(parents=True, exist_ok=True)
    with metrics.open("a") as f:
        f.write(json.dumps({"run_id": m.run_id(), "w": w, "value": value}) + "\n")
    return value


def main(argv: Sequence[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m hypertrain.models.opendecision")
    sub = ap.add_subparsers(dest="cmd", required=True)
    ws = sub.add_parser("warmstart")
    ws.add_argument("--state", type=Path, required=True)
    ws.add_argument("--manifest", type=Path, required=True)
    ws.add_argument("--out", type=Path, required=True)
    ev = sub.add_parser("eval")
    ev.add_argument("--manifest", type=Path, required=True)
    ev.add_argument("--state", type=Path, required=True)
    ev.add_argument("--holdout", type=Path, required=True)
    ev.add_argument("--w", type=int, required=True)
    ev.add_argument("--metrics", type=Path, required=True)
    a = ap.parse_args(argv)
    if a.cmd == "warmstart":
        print(warmstart(a.state, a.manifest, a.out))
    else:
        print(json.dumps({"value": evaluate(a.manifest, a.state, a.holdout, a.w, a.metrics)}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
