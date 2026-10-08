"""Child process: run_phase_b.run_round on 8 emulated ranks over the fake CUDA proxy; prints JSON.

Own process because torch.tensor patching and default-device state must not leak into other tests.
Usage: _phase_b_proxy.py SHARD SHA256
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import torch

import hypertrain.trainer  # noqa: F401  (determinism pin must precede any torch import)

TREE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(TREE / "tests/trainer"))
from fake_device import PROXY, FakeDeviceMode, to_device  # noqa: E402

from hypertrain.trainer.island import emulate  # noqa: E402

B = TREE / "experiments/gpu_phase_b"
spec_ = importlib.util.spec_from_file_location("rb", B / "run_phase_b.py")
assert spec_ is not None and spec_.loader is not None
rb = importlib.util.module_from_spec(spec_)
spec_.loader.exec_module(rb)
spec = json.loads((B / "phase_b.json").read_text())["profiles"]["tiny"]
get, n_rows = rb.shard_reader(Path(sys.argv[1]), sys.argv[2], spec["model"]["seq_len"])
real_tensor = torch.tensor


def tensor_on(data: Any, *args: Any, **kwargs: Any) -> torch.Tensor:
    """CUDA semantics of torch.tensor(..., device=dev): data lands on the proxy, not real meta."""
    dev = kwargs.pop("device", None)
    t = real_tensor(data, *args, device="cpu", **kwargs)
    return to_device(t) if dev is not None and torch.device(dev) == PROXY else t


def rank(comm: Any) -> dict[str, Any]:
    with FakeDeviceMode(default_on_device=True):
        assert torch.zeros(1).device == PROXY
        return dict(rb.run_round(spec, comm, to_device, get, n_rows))


torch.tensor = tensor_on  # type: ignore[assignment]
print(json.dumps(emulate(8, rank)))
