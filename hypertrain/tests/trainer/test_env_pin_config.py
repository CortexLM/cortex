"""ADR 0011 part 3: the manifest's environment pin reaches the trainer's configuration."""

from __future__ import annotations

from tests.miner.test_island_launch_v2 import tiny_manifest

from env_pin_fixtures import CPU_FP32
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.protocol.example import example_manifest
from hypertrain.trainer.config import TrainConfig


def test_train_config_carries_the_pin() -> None:
    pin = EnvPin.from_body(CPU_FP32)
    v1 = example_manifest().body()
    assert TrainConfig.from_manifest(v1).env_pin is None
    # A v1 body carrying the field is enforced, not rejected: v2 training bodies are
    # validated through the v1 model (TrainConfig.from_manifest).
    v1["reference_spec"]["env"]["pinned"] = CPU_FP32
    assert TrainConfig.from_manifest(v1).env_pin == pin
    assert TrainConfig.from_manifest_v2(tiny_manifest(2, pinned=CPU_FP32)).env_pin == pin
    assert TrainConfig.from_manifest_v2(tiny_manifest(2)).env_pin is None
