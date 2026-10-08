"""`hypertrain-miner join|run --config miner.toml` (config file + HYPERTRAIN_MINER_* env only)."""

from __future__ import annotations

import argparse
import json
import logging
import sys
from dataclasses import asdict
from pathlib import Path

import httpx
import numpy as np
import numpy.typing as npt

from hypertrain.data.shards import ShardSamples, ShardSetManifest, verify_shards
from hypertrain.datasets.shards16 import ShardSet16Manifest, U16ShardSamples, verify_shards16
from hypertrain.miner.core import Miner, MinerConfig, MinerError, canonical
from hypertrain.protocol.messages import RunManifest
from hypertrain.trainer.loop import SampleFn


def _u16_sampler(data_dir: Path, manifest: RunManifest) -> SampleFn:
    m = ShardSet16Manifest.from_json((data_dir / "manifest.json").read_text())
    ds = manifest.dataset
    if (m.merkle_root, m.shard_sha256_root, m.n_samples, m.unit_sha256_root) != (
        ds.merkle_root,
        ds.shard_sha256_root,
        ds.n_samples,
        ds.unit_sha256_root,
    ):
        raise MinerError(
            "local shards are not the run's dataset (merkle/shard root/n_samples/unit root)"
        )
    errors = verify_shards16(data_dir, m)
    if errors:
        raise MinerError(f"shard verification failed: {errors[:3]}")
    samples = U16ShardSamples(data_dir, m.n_shards, m.samples_per_shard, m.seq_len)
    return samples.row


def shard_sampler(data_dir: Path, manifest: RunManifest) -> SampleFn:
    if manifest.dataset.sample_format.startswith("u16"):
        return _u16_sampler(data_dir, manifest)
    m = ShardSetManifest.from_json((data_dir / "manifest.json").read_text())
    ds = manifest.dataset
    if (m.merkle_root, m.shard_sha256_root, m.n_samples) != (
        ds.merkle_root,
        ds.shard_sha256_root,
        ds.n_samples,
    ):
        raise MinerError("local shards are not the run's dataset (merkle/shard root/n_samples)")
    errors = verify_shards(data_dir, m)
    if errors:
        raise MinerError(f"shard verification failed: {errors[:3]}")
    samples = ShardSamples(data_dir, m.n_shards, m.samples_per_shard, m.seq_len)

    def get(i: int) -> npt.NDArray[np.uint32]:
        return samples.row(i)

    return get


def _miner(cfg: MinerConfig) -> Miner:
    if cfg.data_dir is None:
        raise MinerError("data_dir is required")
    client = httpx.Client()
    status = client.get(f"{cfg.api.rstrip('/')}/v1/runs/{cfg.run_id}") if cfg.run_id else None
    if status is not None and status.status_code == 200:
        manifest = RunManifest.model_validate(status.json()["manifest_envelope"]["body"])
        return Miner(cfg, client, shard_sampler(cfg.data_dir, manifest))
    lazy: dict[str, SampleFn] = {}

    def get(i: int) -> npt.NDArray[np.uint32]:
        return lazy["get"](i)

    miner = Miner(cfg, client, get)
    assert cfg.data_dir is not None
    lazy["get"] = shard_sampler(cfg.data_dir, miner.manifest)
    return miner


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="hypertrain-miner")
    sub = p.add_subparsers(dest="command", required=True)
    for name in ("join", "run"):
        s = sub.add_parser(name)
        s.add_argument("--config", type=Path)
        if name == "run":
            s.add_argument("--rounds", type=int, default=1)
            s.add_argument("--start", type=int)
    args = p.parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    try:
        cfg = MinerConfig.load(args.config)
        miner = _miner(cfg)
        if args.command == "join":
            # ponytail: admission is the operator's PUT /roster (no public join route in
            # contract v1); we emit a hotkey-signed request carrying the hardware self-check.
            req = {"run_id": miner.run_id, "hotkey": miner.kp.ss58, "hardware": asdict(miner.hw)}
            sig = miner.kp.sign(b"hypertrain/1|Join|" + canonical(req)).hex()
            print(json.dumps({**req, "sig": sig}, sort_keys=True))
            return 0
        results = miner.run(args.rounds, args.start)
    except MinerError as error:
        logging.error("%s: %s", type(error).__name__, error)
        return 2
    print(json.dumps({"rounds": [{"w": w, "status": s} for w, s in results]}))
    return 0 if all(s == "UPLOADED" for _, s in results) else 1


if __name__ == "__main__":
    sys.exit(main())
