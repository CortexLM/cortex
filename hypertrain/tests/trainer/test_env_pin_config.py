"""ADR 0011 part 3: the manifest's environment pin reaches the trainer's configuration."""

from __future__ import annotations

from tests.miner.test_island_launch_v2 import tiny_manifest

from env_pin_fixtures import CPU_AVX2, CPU_AVX512
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.protocol.example import example_manifest
from hypertrain.trainer.config import TrainConfig


def test_train_config_carries_the_pin_of_every_manifest() -> None:
    avx2, avx512 = EnvPin.from_body(CPU_AVX2), EnvPin.from_body(CPU_AVX512)
    v1 = example_manifest().body()
    assert TrainConfig.from_manifest(v1).env_pin == avx2
    v1["reference_spec"]["env"]["pinned"] = CPU_AVX512
    assert TrainConfig.from_manifest(v1).env_pin == avx512
    assert TrainConfig.from_manifest_v2(tiny_manifest(2)).env_pin == avx2
    assert TrainConfig.from_manifest_v2(tiny_manifest(2, pinned=CPU_AVX512)).env_pin == avx512
