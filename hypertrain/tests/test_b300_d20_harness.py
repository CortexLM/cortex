"""scripts/b300/d20/gpu_run_v2.py: the kimi-k3-pro-tiny model swap, checked on CPU.

The harness itself needs a B300 and its evidence is the run. The swap is the part that is cheap to
check here and expensive to get wrong on a rented GPU: the fixture Setup must come out as a valid
v2 manifest for the kimi arch whose hashes and dataset agree with the model.
"""

from __future__ import annotations

import importlib.util
import math
import sys
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / "scripts/b300/d20/gpu_run_v2.py"
FIXTURE = ROOT / "tests/ledger/test_escrow_v2.py"


def load(name: str, path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def harness(monkeypatch: pytest.MonkeyPatch) -> Iterator[ModuleType]:
    """The harness module, without leaving its sys.path entry or its imports behind."""
    monkeypatch.setenv("HT_IMAGE_DIGEST", "sha256:" + "ab" * 32)
    monkeypatch.setattr(sys, "path", list(sys.path))
    names = ("d20_gpu_run_v2", "mine_smoke_v2", "d20_escrow_fixture")
    yield load("d20_gpu_run_v2", HARNESS)
    for name in names:
        sys.modules.pop(name, None)


def test_kimi_setup_is_a_valid_kimi_manifest_with_matching_hashes_and_data(
    harness: ModuleType, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hypertrain.models.kimi_tiny as kt
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    fixture = load("d20_escrow_fixture", FIXTURE)
    before = fixture.setup()
    assert before.manifest.training.model.arch == "decoder"

    swapped = harness._kimi_setup(fixture, before)

    training = swapped.manifest.training
    # Given the repository's own param-count rule, not the harness's copy of the geometry.
    assert training.model.arch == kt.ARCH
    assert training.model.param_count == sum(
        math.prod(s) for s in kt.param_shapes(kt.MICRO).values()
    )
    assert training.model.compute_dtype == "bf16"
    # The start state the miners and auditors share is the kimi model's, not the fixture decoder's.
    cfg = TrainConfig.from_manifest_v2(swapped.manifest)
    assert training.init_state_hash == state_hash(init_params(cfg.model))
    assert training.init_state_hash != before.manifest.training.init_state_hash
    # The dataset the Merkle root commits to holds token ids the model can embed.
    assert len(swapped.rows) == training.dataset.n_samples
    assert int(swapped.rows.max()) < training.model.vocab
    assert swapped.rows.shape[1] == training.model.seq_len + 1
    # The input Setup is not modified.
    assert before.manifest.training.model.arch == "decoder"
    monkeypatch.setattr(harness, "KIMI_DTYPE", "fp32")
    assert harness._kimi_setup(fixture, before).manifest.training.model.compute_dtype == "fp32"
