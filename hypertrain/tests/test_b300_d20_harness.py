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
import shlex
import sys
from collections.abc import Callable, Iterator
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


def service_on_the_escrow_fixture(
    harness: ModuleType, monkeypatch: pytest.MonkeyPatch
) -> tuple[SimpleNamespace, list[object]]:
    """The loader `_load_service` wraps, on the escrow fixture; `_patch_store` records its owner."""
    svc = SimpleNamespace(fixture=load("d20_escrow_fixture", FIXTURE), OWNER=object())
    patched: list[object] = []
    monkeypatch.setattr(harness, "_load", lambda: svc)
    monkeypatch.setattr(harness, "_patch_store", patched.append)
    return svc, patched


def test_load_service_stops_instead_of_returning_when_the_device_class_is_refused(
    harness: ModuleType, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    events = tmp_path / "events.jsonl"
    monkeypatch.setenv("HT_D20_EVENTS", str(events))
    service_on_the_escrow_fixture(harness, monkeypatch)
    fake_cuda(monkeypatch, [(10, 3, 132)])

    with pytest.raises(SystemExit, match="qualifies \\[148\\] SMs, not 132"):
        harness._load_service()

    event = json.loads(events.read_text())
    assert event["event"] == "device_class"
    assert event["unqualified"] == "device 0: capability 10.3 qualifies [148] SMs, not 132"


@pytest.mark.parametrize(
    ("model", "arch"),
    [("dense", "decoder"), ("kimi", "kimi-k3-pro-tiny")],
    ids=["dense", "kimi"],
)
def test_load_service_returns_the_service_with_its_setup_pinned_to_the_b300(
    harness: ModuleType, monkeypatch: pytest.MonkeyPatch, model: str, arch: str
) -> None:
    svc, patched = service_on_the_escrow_fixture(harness, monkeypatch)
    monkeypatch.setattr(harness, "MODEL", model)
    fake_cuda(monkeypatch, [(10, 3, 148)])

    assert harness._load_service() is svc

    assert patched == [svc.OWNER]
    manifest = svc.fixture.setup().manifest
    reference = manifest.training.reference_spec
    assert reference.image_digest == "sha256:" + "ab" * 32
    assert (reference.driver_allowlist, reference.sm_count) == (["595.91.07"], 148)
    assert manifest.training.model.arch == arch


@pytest.fixture
def entry_points(harness: ModuleType, monkeypatch: pytest.MonkeyPatch) -> list[tuple[str, str]]:
    """main()'s three downstream scripts as recorders of (name, model), each with its own code."""
    ran: list[tuple[str, str]] = []

    def recorder(name: str, rc: int) -> Callable[[], int]:
        def run() -> int:
            ran.append((name, harness.MODEL))
            return rc

        return run

    monkeypatch.setattr(harness.smoke, "main", recorder("smoke", 11))
    monkeypatch.setitem(
        sys.modules, "mine_multi_round_v2", SimpleNamespace(main=recorder("multi", 12))
    )
    monkeypatch.setattr(harness, "_job_kimi", recorder("job", 13))
    monkeypatch.setenv("HYPERTRAIN_MINER_DEVICE", "cuda")
    return ran


@pytest.mark.parametrize(
    ("mode", "script", "model", "rc"),
    [
        ("smoke", "smoke", "dense", 11),
        ("smoke-kimi", "smoke", "kimi", 11),
        ("multi", "multi", "dense", 12),
        ("multi-kimi", "multi", "kimi", 12),
        ("job-kimi", "job", "kimi", 13),
    ],
)
def test_main_runs_the_script_of_the_mode_with_the_model_of_the_mode(
    harness: ModuleType,
    entry_points: list[tuple[str, str]],
    monkeypatch: pytest.MonkeyPatch,
    mode: str,
    script: str,
    model: str,
    rc: int,
) -> None:
    monkeypatch.setattr(sys, "argv", ["gpu_run_v2.py", mode])

    assert harness.main() == rc

    assert entry_points == [(script, model)]


@pytest.mark.parametrize("device", [None, "cpu"], ids=["unset", "cpu"])
def test_main_refuses_to_run_unless_the_miner_device_is_cuda(
    harness: ModuleType,
    entry_points: list[tuple[str, str]],
    monkeypatch: pytest.MonkeyPatch,
    device: str | None,
) -> None:
    if device is None:
        monkeypatch.delenv("HYPERTRAIN_MINER_DEVICE")
    else:
        monkeypatch.setenv("HYPERTRAIN_MINER_DEVICE", device)
    monkeypatch.setattr(sys, "argv", ["gpu_run_v2.py", "smoke"])

    with pytest.raises(SystemExit, match="set HYPERTRAIN_MINER_DEVICE=cuda"):
        harness.main()

    assert entry_points == []


@pytest.mark.parametrize(
    "argv",
    [[], ["dp8"], ["smoke", "multi"]],
    ids=["no_mode", "unknown_mode", "two_modes"],
)
def test_main_answers_usage_and_runs_nothing_for_an_unknown_mode(
    harness: ModuleType,
    entry_points: list[tuple[str, str]],
    monkeypatch: pytest.MonkeyPatch,
    argv: list[str],
) -> None:
    monkeypatch.setattr(sys, "argv", ["gpu_run_v2.py", *argv])

    with pytest.raises(SystemExit, match="usage: gpu_run_v2.py smoke\\|multi"):
        harness.main()

    assert entry_points == []


UUID_A = "GPU-0a1b2c3d-0000-4000-8000-00000000000a"
UUID_B = "GPU-0a1b2c3d-0000-4000-8000-00000000000b"


def fake_nvidia_smi(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, rows: list[str] | None
) -> None:
    """The only nvidia-smi on PATH: it answers the harness's uuid/driver query and nothing else.

    `rows=None` puts no nvidia-smi on PATH at all.
    """
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    if rows is not None:
        shim = bin_dir / "nvidia-smi"
        shim.write_text(
            "#!/bin/sh\n"
            '[ "$*" = "--query-gpu=uuid,driver_version --format=csv,noheader" ] ||\n'
            '  { echo "unexpected nvidia-smi arguments: $*" >&2; exit 125; }\n'
            f"printf '%s\\n' {' '.join(shlex.quote(row) for row in rows)}\n"
        )
        shim.chmod(0o755)
    monkeypatch.setenv("PATH", str(bin_dir))


def test_local_gpus_returns_the_measured_record_when_both_sides_see_exactly_n(
    harness: ModuleType, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # The second UUID is upper case: the record carries the canonical `gpu_uuid` form.
    fake_nvidia_smi(monkeypatch, tmp_path, [f"{UUID_A}, 595.91.07", f"{UUID_B.upper()}, 595.91.08"])
    # Distinct per-GPU values, so a mix-up of two indices shows in the record.
    fake_cuda(monkeypatch, [(10, 3, 148), (10, 3, 147)])

    assert harness._local_gpus(2) == {
        "backend": "cuda",
        "image_digest": "sha256:" + "ab" * 32,
        "drivers": ["595.91.07", "595.91.08"],
        "sm_counts": [148, 147],
        "gpu_uuids": [UUID_A, UUID_B],
    }


@pytest.mark.parametrize(
    ("smi_rows", "torch_devices", "message"),
    [
        ([f"{UUID_A}, 595.91.07", f"{UUID_B}, 595.91.07"], [(10, 3, 148)], "nvidia-smi sees 2"),
        ([f"{UUID_A}, 595.91.07"], [(10, 3, 148), (10, 3, 148)], "nvidia-smi sees 1"),
        ([], [(10, 3, 148)], "nvidia-smi sees 0"),
        (None, [(10, 3, 148)], "nvidia-smi unavailable"),
    ],
    ids=["smi_sees_more", "torch_sees_more", "smi_sees_none", "no_nvidia_smi"],
)
def test_local_gpus_refuses_unless_nvidia_smi_and_torch_both_see_exactly_n(
    harness: ModuleType,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    smi_rows: list[str] | None,
    torch_devices: list[tuple[int, int, int]],
    message: str,
) -> None:
    fake_nvidia_smi(monkeypatch, tmp_path, smi_rows)
    fake_cuda(monkeypatch, torch_devices)

    with pytest.raises(RuntimeError, match=message):
        harness._local_gpus(1)
