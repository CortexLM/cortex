"""scripts/b300/gpu_probe.py verdict: bitwise equality across repeats and devices, errors fail."""

from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("gpu_probe", ROOT / "scripts/b300/gpu_probe.py")
assert spec is not None and spec.loader is not None
gpu_probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gpu_probe)  # imports hypertrain.trainer before torch
torch = gpu_probe.torch


def test_cpu_smoke_is_bitwise_stable_and_one_flipped_digest_fails():
    runs = {"cpu": [gpu_probe.workload(torch.device("cpu")) for _ in range(2)]}
    assert gpu_probe.evaluate(runs)["ok"], runs
    flipped = {**runs, "cuda:7": [{**runs["cpu"][0], "matmul_bf16": "0" * 64}]}
    verdict = gpu_probe.evaluate(flipped)
    assert not verdict["ok"] and verdict["mismatches"] == ["cuda:7 repeat 0"]


def test_a_kernel_error_on_every_device_still_fails():
    err = {"sdpa-flash": "error: CUDA error: no kernel image is available for execution"}
    verdict = gpu_probe.evaluate({f"cuda:{i}": [err, err] for i in range(8)})
    assert not verdict["ok"] and verdict["mismatches"] == []
    assert len(verdict["errors"]) == 8
