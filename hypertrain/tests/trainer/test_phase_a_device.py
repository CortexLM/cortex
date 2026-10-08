from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np
import pytest
import torch
from fake_device import PROXY, REAL_SET_DEFAULT_DEVICE, CudaRedirect, FakeDeviceMode

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "experiments" / "gpu_phase_a" / "run_phase_a.py"


def _load() -> Any:
    spec = importlib.util.spec_from_file_location("run_phase_a_under_test", SCRIPT)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _shard(tmp: Path) -> tuple[Path, str]:
    import hashlib

    rng = np.random.default_rng(0)
    data = rng.integers(0, 259, size=(64, 1025), dtype=np.uint32).astype("<u4").tobytes()
    p = tmp / "shard.u32"
    p.write_bytes(data)
    return p, hashlib.sha256(data).hexdigest()


def _main(
    rp: Any, monkeypatch: pytest.MonkeyPatch, tmp: Path, device: str, negative: bool = False
) -> dict[str, Any]:
    shard, sha = _shard(tmp)
    out = tmp / f"{device}-{int(negative)}.json"
    argv = ["run_phase_a.py", "--profile", "tiny", "--device", device, "--shard", str(shard)]
    argv += ["--shard-sha256", sha, "--out", str(out)] + (["--negative"] if negative else [])
    monkeypatch.setattr(sys, "argv", argv)
    assert rp.main() == 0
    result: dict[str, Any] = json.loads(out.read_text())
    return result


def _as_cuda_host(monkeypatch: pytest.MonkeyPatch, rp: Any) -> None:
    """Production cuda branch on a CUDA-less host: gate passes, cuda == proxy device."""

    def env_record(device: str) -> dict[str, Any]:
        return {"torch": rp.EXPECTED_TORCH, "sm_count": rp.EXPECTED_SM_COUNT, "device": device}

    def set_default_device(dev: Any) -> None:
        is_cuda = dev is not None and torch.device(dev).type == "cuda"
        REAL_SET_DEFAULT_DEVICE(PROXY if is_cuda else dev)

    init = rp.init_params
    monkeypatch.setattr(rp, "env_record", env_record)
    monkeypatch.setattr(torch, "set_default_device", set_default_device)
    monkeypatch.setattr(
        rp,
        "init_params",
        lambda cfg: {k: v.as_subclass(CudaRedirect) for k, v in init(cfg).items()},
    )


@pytest.mark.parametrize("negative", [False, True], ids=["honest", "negative"])
def test_run_phase_a_cuda_path_on_proxy_matches_cpu(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, negative: bool
) -> None:
    rp = _load()
    cpu = _main(rp, monkeypatch, tmp_path, "cpu", negative)
    honest = _main(rp, monkeypatch, tmp_path, "cpu", False)
    _as_cuda_host(monkeypatch, rp)
    with FakeDeviceMode(default_on_device=False):
        dev = _main(rp, monkeypatch, tmp_path, "cuda", negative)
        assert torch.get_default_device() == PROXY
        assert isinstance(torch.zeros(1), torch.Tensor) and torch.zeros(1).device == PROXY
    assert torch.get_default_device().type == "cpu"
    print(f"tiny negative={negative} cpu_root={cpu['leaves_root']} proxy_root={dev['leaves_root']}")
    assert dev["leaves"] == cpu["leaves"]
    assert dev["leaves_root"] == cpu["leaves_root"]
    assert dev["delta_hash"] == cpu["delta_hash"]
    assert dev["final_theta_hash"] == cpu["final_theta_hash"]
    if negative:
        k = dev["expected_first_divergent_leaf"]
        assert dev["leaves"][:k] == honest["leaves"][:k]
        assert all(x != y for x, y in zip(dev["leaves"][k:], honest["leaves"][k:], strict=True))
