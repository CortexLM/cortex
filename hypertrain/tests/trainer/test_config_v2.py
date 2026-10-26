from __future__ import annotations

import importlib.util
from pathlib import Path

from hypertrain.protocol.example import example_manifest
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import island_from_manifest


def test_v1_conversion_and_golden_unchanged() -> None:
    m = example_manifest()
    assert m.run_id() == "ad8beb30084a2e53f817c45f1cbfdf1491162575f10f2c3d190645081b2ba2ff"
    cfg, _, run = island_from_manifest(m.body())
    assert cfg == TrainConfig.from_manifest(m.body())
    assert run == m.run_id()


def test_v2_adapter_wrapper_is_authoritative() -> None:
    spec = importlib.util.spec_from_file_location(
        "config_fixture", Path(__file__).parents[1] / "miner/test_island_launch_v2.py"
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    w = mod.tiny_manifest(8)
    cfg, lay, run = island_from_manifest(w.body())
    assert cfg == TrainConfig.from_manifest_v2(w)
    assert lay.n_gpus == 8
    assert run == w.run_id() != w.training.training_hash()
