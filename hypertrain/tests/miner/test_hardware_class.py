from __future__ import annotations

from hypertrain.miner.core import Gpu
from hypertrain.miner.hardware_class import HardwareClass, admit, classes_for, determinism_digest

SETTINGS = {"deterministic_algorithms": True, "num_threads": 1, "torch_preloaded": False}
RTX5090 = classes_for(
    [Gpu(0, "NVIDIA GeForce RTX 5090", 170, "GPU-a")], "12.0", "2.14.0", "13.0", SETTINGS
)[0]
H100 = HardwareClass("h100 80gb hbm3", "9.0", "2.14.0", "13.0", RTX5090.determinism)


def test_class_normalizes_name_and_ignores_process_local_settings() -> None:
    assert RTX5090.gpu == "geforce rtx 5090"
    assert RTX5090.determinism == determinism_digest({**SETTINGS, "torch_preloaded": True})


def test_admits_same_class() -> None:
    assert admit({RTX5090}, [RTX5090, RTX5090]).accepted


def test_rejects_class_outside_run_set() -> None:
    other_torch = HardwareClass(RTX5090.gpu, "12.0", "2.13.0", "13.0", RTX5090.determinism)
    got = admit({RTX5090}, [other_torch], reduction_order="unit")
    assert not got.accepted and "not allowed" in got.reason
    assert not admit({RTX5090}, [H100]).accepted


def test_mixed_island_needs_unit_order() -> None:
    got = admit({RTX5090, H100}, [RTX5090, H100])
    assert not got.accepted and "reduction_order 'unit'" in got.reason
    assert admit({RTX5090, H100}, [RTX5090, H100], reduction_order="unit").accepted


def test_rejects_empty_or_unknown_order() -> None:
    assert not admit({RTX5090}, []).accepted
    assert not admit({RTX5090}, [RTX5090], reduction_order="fast").accepted
