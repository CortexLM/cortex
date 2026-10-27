"""scripts/b300/d20/gpu_run_v2.py: the kimi-k3-pro-tiny model swap and the device-class guard.

The harness itself needs a B300 and its evidence is the run. These two parts are cheap to check
on CPU and expensive to get wrong on a rented GPU: the fixture Setup must come out as a valid v2
manifest for the kimi arch whose hashes, dataset commitments and staged job agree with the model,
and the harness must refuse to run on a device class the repository does not qualify.
"""

from __future__ import annotations

import importlib.util
import json
import math
import sys
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType, SimpleNamespace

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
    """The harness on a private copy of mine_smoke_v2, with no HT_D20_* in the environment."""
    for variable in ("HT_D20_KIMI_DTYPE", "HT_D20_SM_COUNT", "HT_D20_DRIVER", "HT_D20_EVENTS"):
        monkeypatch.delenv(variable, raising=False)
    monkeypatch.setenv("HT_IMAGE_DIGEST", "sha256:" + "ab" * 32)
    monkeypatch.setattr(sys, "path", list(sys.path))
    # The harness rebinds mine_smoke_v2._load_service: never do that to a module other tests hold.
    monkeypatch.delitem(sys.modules, "mine_smoke_v2", raising=False)
    yield load("d20_gpu_run_v2", HARNESS)
    for name in ("d20_gpu_run_v2", "mine_smoke_v2", "d20_escrow_fixture"):
        sys.modules.pop(name, None)


def test_kimi_setup_is_a_valid_kimi_manifest_with_matching_hashes_data_and_job(
    harness: ModuleType, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    import numpy as np

    import hypertrain.models.kimi_tiny as kt
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    fixture = load("d20_escrow_fixture", FIXTURE)
    before = fixture.setup()
    assert before.manifest.training.model.arch == "decoder"

    swapped = harness._kimi_setup(fixture, before)

    training = swapped.manifest.training
    model = training.model
    # Given the repository's own param-count rule, not the harness's copy of the geometry.
    assert model.arch == kt.ARCH
    assert model.param_count == sum(math.prod(s) for s in kt.param_shapes(kt.MICRO).values())
    assert model.compute_dtype == "bf16"
    # The constants of the D18 CPU test this ports (tests/miner/test_client_network_v2.py).
    assert (model.init_seed, model.rope_theta) == (7, 50000)
    # The start state the miners and auditors share is the kimi model's, not the fixture decoder's.
    cfg = TrainConfig.from_manifest_v2(swapped.manifest)
    assert training.init_state_hash == state_hash(init_params(cfg.model))
    assert training.init_state_hash != before.manifest.training.init_state_hash
    # The dataset the manifest commits to is the one the Setup carries; its tokens are embeddable.
    dataset = training.dataset
    assert swapped.rows.dtype == np.dtype("<u4")
    assert swapped.rows.shape == (dataset.n_samples, model.seq_len + 1)
    assert int(swapped.rows.max()) < model.vocab
    assert dataset.merkle_root == swapped.tree.root.hex()
    assert dataset.depth == (dataset.n_samples - 1).bit_length()
    for i in (0, 5, dataset.n_samples - 1):
        assert MerkleTree.verify(
            swapped.rows[i].tobytes(),
            i,
            swapped.tree.proof(i),
            bytes.fromhex(dataset.merkle_root),
            dataset.n_samples,
        )
    # The policies are the fixture's, and the job job-kimi stages (samples 0..3) is valid.
    assert (swapped.policy, swapped.admission_policy) == (before.policy, before.admission_policy)
    assert training.inner.H * training.inner.micro_batch * training.inner.grad_accum == 4
    job = fixture.stage(swapped, tmp_path / "attempt", (0, 1, 2, 3), 0)
    assert job.manifest == swapped.manifest
    # The input Setup is not modified, and the dtype follows the knob.
    assert before.manifest.training.model.arch == "decoder"
    monkeypatch.setattr(harness, "KIMI_DTYPE", "fp32")
    assert harness._kimi_setup(fixture, before).manifest.training.model.compute_dtype == "fp32"


def fake_cuda(monkeypatch: pytest.MonkeyPatch, devices: list[tuple[int, int, int]]) -> None:
    import torch

    props = [SimpleNamespace(major=a, minor=b, multi_processor_count=c) for a, b, c in devices]
    monkeypatch.setattr(torch.cuda, "device_count", lambda: len(props))
    monkeypatch.setattr(torch.cuda, "get_device_properties", lambda i: props[i])


def test_device_class_records_the_repository_verdict_for_a_qualified_pair(
    harness: ModuleType, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    events = tmp_path / "events.jsonl"
    monkeypatch.setenv("HT_D20_EVENTS", str(events))
    fake_cuda(monkeypatch, [(10, 3, 148)])

    harness._device_class()

    event = json.loads(events.read_text())
    assert event["event"] == "device_class"
    assert (event["capabilities"], event["sm_counts"], event["unqualified"]) == (
        [[10, 3]],
        [148],
        None,
    )
    assert harness.DEVICE_CLASS["unqualified"] is None


@pytest.mark.parametrize(
    ("devices", "message"),
    [
        ([(10, 3, 132)], "qualifies \\[148\\] SMs, not 132"),  # B300 capability, wrong SM count
        ([(9, 0, 132)], "not a qualified device class"),  # unknown pair
        ([], "no CUDA device"),  # the table has nothing to refuse on an empty list
    ],
    ids=["wrong_sm_count", "unknown_pair", "no_gpu"],
)
def test_device_class_stops_the_harness_on_an_unqualified_or_absent_device(
    harness: ModuleType,
    monkeypatch: pytest.MonkeyPatch,
    devices: list[tuple[int, int, int]],
    message: str,
) -> None:
    fake_cuda(monkeypatch, devices)

    with pytest.raises(SystemExit, match=message):
        harness._device_class()
