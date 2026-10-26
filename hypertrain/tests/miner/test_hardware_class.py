from __future__ import annotations

import pytest

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
    assert admit({RTX5090}, [RTX5090, RTX5090], arch="decoder").accepted


def test_rejects_class_outside_run_set() -> None:
    other_torch = HardwareClass(RTX5090.gpu, "12.0", "2.13.0", "13.0", RTX5090.determinism)
    got = admit({RTX5090}, [other_torch], reduction_order="unit", arch="decoder")
    assert not got.accepted and "not allowed" in got.reason
    assert not admit({RTX5090}, [H100], arch="decoder").accepted


def test_mixed_island_needs_unit_order() -> None:
    got = admit({RTX5090, H100}, [RTX5090, H100], arch="decoder")
    assert not got.accepted and "reduction_order 'unit'" in got.reason
    assert admit({RTX5090, H100}, [RTX5090, H100], reduction_order="unit", arch="decoder").accepted


def test_rejects_empty_or_unknown_order() -> None:
    assert not admit({RTX5090}, [], arch="decoder").accepted
    assert not admit({RTX5090}, [RTX5090], reduction_order="fast", arch="decoder").accepted


def test_unit_order_refused_for_a_rank_only_arch() -> None:
    mixed = ({RTX5090, H100}, [RTX5090, H100])
    got = admit(*mixed, reduction_order="unit", arch="kimi-k3-pro-tiny")
    assert not got.accepted and "does not allow reduction_order 'unit'" in got.reason
    assert not admit(*mixed, reduction_order="rank", arch="kimi-k3-pro-tiny").accepted
    assert admit({RTX5090}, [RTX5090], arch="kimi-k3-pro-tiny").accepted
    assert admit(*mixed, reduction_order="unit", arch="decoder").accepted
    # od-encoder fails closed: no test shows its unit-order world invariance yet.
    assert not admit(*mixed, reduction_order="unit", arch="od-encoder").accepted
    assert not admit({RTX5090}, [RTX5090], arch="nope").accepted


def test_admission_requires_the_run_arch() -> None:
    # No default: a caller that forgets the arch must not get the decoder's orders.
    with pytest.raises(TypeError):
        admit({RTX5090, H100}, [RTX5090, H100], reduction_order="unit")  # type: ignore[call-arg]
