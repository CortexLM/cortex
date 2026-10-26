from __future__ import annotations

import hashlib
from dataclasses import replace
from pathlib import Path

import numpy as np
import torch

from hypertrain.aggregator.core import Aggregator, OuterParams, OuterState
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Commit, f32hex
from hypertrain.trainer.compress import compress
from hypertrain.trainer.config import CompressConfig

SHAPES = {"blk.0.w": (4, 8), "blk.1.b": (16,), "emb": (3, 5)}
COORD = Keypair(bytes(range(32)))
H0 = hashlib.sha256(b"x").hexdigest()


def hotkey(i: int) -> str:
    return Keypair(bytes([i + 1]) * 32).ss58


def theta0() -> dict[str, np.ndarray]:
    rng = np.random.default_rng(7)
    return {n: rng.standard_normal(s).astype(np.float32) for n, s in SHAPES.items()}


def params(**kw: object) -> OuterParams:
    p = OuterParams.from_manifest(example_manifest())
    return replace(p, **kw)  # type: ignore[arg-type]


def raw_delta(seed: int, scale: float = 0.1) -> dict[str, np.ndarray]:
    rng = np.random.default_rng(seed)
    return {n: (rng.standard_normal(s) * scale).astype(np.float32) for n, s in SHAPES.items()}


def put_delta(store: LocalFSStore, d: dict[str, np.ndarray]) -> tuple[str, int]:
    td = {n: torch.from_numpy(a.copy()) for n, a in d.items()}
    ef = {n: torch.zeros_like(t) for n, t in td.items()}
    payload, _ = compress(CompressConfig("dense-int8", ef_beta=0.0), td, ef)
    return store.put(payload), len(payload)


def commit(store: LocalFSStore, w: int, who: int, d: dict[str, np.ndarray]) -> Commit:
    h, n = put_delta(store, d)
    return Commit(
        w=w,
        hotkey=hotkey(who),
        leaf_scheme="ht-leaf-v1",
        n_leaves=7,
        leaves_root=H0,
        metrics_root=H0,
        final_theta_hash=H0,
        ef_in_hash=H0,
        ef_out_hash=H0,
        delta_hash=h,
        delta_bytes=n,
        tokens=1,
    )


def setup(root: Path, p: OuterParams | None = None) -> tuple[Aggregator, str]:
    root.mkdir(parents=True, exist_ok=True)
    store = LocalFSStore(root)
    agg = Aggregator(store, example_manifest().run_id(), p or params(), COORD, root / "agg-state")
    return agg, agg.put_state(OuterState.init(theta0()))


def fresh(root: Path, p: OuterParams | None = None) -> Aggregator:
    run_id = example_manifest().run_id()
    return Aggregator(LocalFSStore(root), run_id, p or params(), COORD, root / "agg-state")


def scenario(root: Path) -> dict[str, object]:
    """Round 0: 3 honest + faulty (index 9, 50x norm); round 1: 3 honest on tainted theta."""
    agg, s0 = setup(root)
    c0 = [commit(agg.store, 0, i, raw_delta(i)) for i in range(3)]
    c0.append(commit(agg.store, 0, 9, raw_delta(99, scale=5.0)))
    t0 = agg.apply_round(0, s0, c0)
    c1 = [commit(agg.store, 1, i, raw_delta(10 + i)) for i in range(3)]
    t1 = agg.apply_round(1, t0["body"]["out_state"], c1)
    return {"agg": agg, "s0": s0, "c0": c0, "c1": c1, "t0": t0, "t1": t1, "bad": hotkey(9)}


def p32(x: float) -> str:
    return f32hex(x)
