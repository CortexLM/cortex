"""Multi-GPU miner: installed `hypertrain-miner run-v2` as a real subprocess on 2 gloo ranks
against the real v2 service (HTTP), plus GPU enumeration/rejection and the v1 boundary."""

from __future__ import annotations

import contextlib
import importlib.util
import json
import os
import socket
import subprocess
import sys
import threading
from collections.abc import Iterator
from pathlib import Path
from types import SimpleNamespace

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)
from hypertrain.miner import core
from hypertrain.miner.core import Gpu, HardwareMismatch, select_gpus
from hypertrain.miner.island_launch import IslandFailure, gpu_uuid, launch_island
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import RunManifest
from hypertrain.protocol.messages_v2 import RunManifestV2

_spec = importlib.util.spec_from_file_location(
    "multigpu_service_fixture", Path(__file__).parents[1] / "challenge/test_service_network_v2.py"
)
assert _spec is not None and _spec.loader is not None
service = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = service
_spec.loader.exec_module(service)

U = "0a1b2c3d-0000-4000-8000-00000000000"


def gpus(*sms: int) -> tuple[Gpu, ...]:
    return tuple(Gpu(i, "RTX 5090", sm, f"GPU-{U}{i}") for i, sm in enumerate(sms))


@pytest.fixture
def two_rank_network(tmp_path, request, monkeypatch):
    """The real service fixture with a pinned DP2 + ZeRO-1 island layout."""
    setup_type = service.fixture.Setup

    def two_rank(manifest, policy, admission_policy, rows, tree):
        body = manifest.body()
        body["training"]["reference_spec"]["layout"].update(
            n_gpus=2, dp_size=2, ep_size=1, zero1=True
        )
        return setup_type(RunManifestV2.model_validate(body), policy, admission_policy, rows, tree)

    monkeypatch.setattr(service.fixture, "Setup", two_rank)
    yield from service.network.__wrapped__(tmp_path, request)


@contextlib.contextmanager
def http_service(app) -> Iterator[int]:
    import uvicorn

    class Ready(uvicorn.Server):
        def __init__(self, config):
            super().__init__(config)
            self.ready = threading.Event()

        async def startup(self, sockets=None):
            await super().startup(sockets=sockets)
            self.ready.set()

    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    server = Ready(uvicorn.Config(app, log_level="error"))
    thread = threading.Thread(target=lambda: server.run(sockets=[sock]), daemon=True)
    thread.start()
    assert server.ready.wait(30)
    try:
        yield sock.getsockname()[1]
    finally:
        server.should_exit = True
        thread.join(timeout=10)
        assert not thread.is_alive()


def test_installed_run_v2_trains_two_rank_island_as_subprocess(two_rank_network, tmp_path):
    # Given: the real service on a socket, a two-rank job staged from the served manifest.
    network = two_rank_network
    manifest = network.manifest
    assert manifest.training.reference_spec.layout.n_gpus == 2
    s = service.fixture.Setup(
        manifest,
        network.setup.policy,
        network.setup.admission_policy,
        network.setup.rows,
        network.setup.tree,
    )
    job = service.fixture.stage(s, tmp_path / "job", (0, 1), 0)
    (tmp_path / "job/job.json").write_text(job.model_dump_json())
    key = tmp_path / "miner.key"
    key.write_bytes(b"\x55" * 32)
    key.chmod(0o600)
    script = Path(sys.executable).with_name("hypertrain-miner")
    assert script.is_file(), "console script not installed in the test interpreter's env"
    with http_service(network.client.app) as port:
        config = tmp_path / "miner.toml"
        config.write_text(
            "\n".join(
                f"{k} = {json.dumps(str(v))}"
                for k, v in {
                    "api": f"http://127.0.0.1:{port}",
                    "keyfile": key,
                    "workdir": tmp_path / "work",
                    "state_source": "cpu",
                    "image_digest": manifest.training.reference_spec.image_digest,
                    "run_id": manifest.run_id(),
                    "owner_hotkey": service.OWNER.ss58,
                    "device": "cpu",
                }.items()
            )
        )
        # When: the installed CLI runs as its own OS process (torchrun under it).
        proc = subprocess.run(  # noqa: S603
            [
                str(script),
                "run-v2",
                "--config",
                str(config),
                "--job",
                str(tmp_path / "job/job.json"),
            ],
            capture_output=True,
            text=True,
            timeout=600,
            env={**os.environ, "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1"},
        )
    assert proc.returncode == 0, proc.stderr[-4000:]
    # Then: exactly n_gpus ranks published one bitwise-identical island commitment.
    result = json.loads(proc.stdout.strip().splitlines()[-1])
    published = tmp_path / "job/published"
    assert sorted(p.name for p in published.glob("rank-*")) == ["rank-0", "rank-1"]
    s0, s1 = (json.loads((published / f"rank-{r}/summary.json").read_bytes()) for r in (0, 1))
    assert s0["backend"] == s1["backend"] == "cpu"
    assert s0["commitments"] == s1["commitments"]
    assert [s0["work"]["rank"], s1["work"]["rank"]] == [0, 1]
    assert (published / "rank-0/state.safetensors").read_bytes() == (
        published / "rank-1/state.safetensors"
    ).read_bytes()
    assert Path(result["state"]) == published / "rank-0/state.safetensors"
    assert (published / "rank-0/trace.json").is_file() and (
        published / "rank-1/trace.json"
    ).is_file()


def test_select_gpus_lists_all_and_rejects_short_or_mixed_hosts():
    assert select_gpus(gpus(170, 170, 170), 2, 170) == (f"GPU-{U}0", f"GPU-{U}1")
    with pytest.raises(HardwareMismatch, match="1 visible GPU"):
        select_gpus(gpus(170), 2, 170)
    with pytest.raises(HardwareMismatch, match="heterogeneous SM"):
        select_gpus(gpus(170, 132), 1, 170)
    with pytest.raises(HardwareMismatch, match="!= reference"):
        select_gpus(gpus(132, 132), 2, 170)
    dup = (Gpu(0, "a", 170, f"GPU-{U}0"), Gpu(1, "a", 170, f"GPU-{U}0"))
    with pytest.raises(HardwareMismatch, match="duplicate"):
        select_gpus(dup, 1, 170)


def test_detect_gpus_enumerates_every_visible_device(monkeypatch):
    import torch

    props = [
        SimpleNamespace(name="RTX 5090", multi_processor_count=170, uuid=f"{U}{i}")
        for i in range(3)
    ]
    monkeypatch.setattr(torch.cuda, "is_available", lambda: True)
    monkeypatch.setattr(torch.cuda, "device_count", lambda: 3)
    monkeypatch.setattr(torch.cuda, "get_device_properties", lambda i: props[i])
    assert core.detect_gpus() == gpus(170, 170, 170)
    monkeypatch.setattr(torch.cuda, "is_available", lambda: False)
    assert core.detect_gpus() == ()


def test_gpu_uuid_normalizes_torch_and_smi_forms():
    assert gpu_uuid(f"{U}0".upper()) == gpu_uuid(f"GPU-{U}0") == f"GPU-{U}0"
    with pytest.raises(IslandFailure):
        gpu_uuid("GPU-0")


def test_v1_run_multi_gpu_layout_names_run_v2():
    body = example_manifest().body()
    body["reference_spec"]["layout"].update(n_gpus=2, dp_size=2, ep_size=1, zero1=True)
    with pytest.raises(HardwareMismatch, match="run-v2"):
        core.self_check("cpu", RunManifest.model_validate(body))


def test_cuda_launch_refused_before_torchrun_on_short_host(monkeypatch, tmp_path):
    # Given: a two-rank job; host exposes one GPU.
    sys.path.insert(0, str(Path(__file__).parent))
    from test_island_launch_v2 import staged_job

    job = staged_job(tmp_path, 2)
    monkeypatch.setattr(core, "detect_gpus", lambda: gpus(170))
    miner = object.__new__(core.NetworkMiner)
    miner.cfg = SimpleNamespace(device="cuda")
    miner.manifest = job.manifest.model_copy(
        update={
            "training": job.manifest.training.model_copy(
                update={
                    "reference_spec": job.manifest.training.reference_spec.model_copy(
                        update={"sm_count": 170}
                    )
                }
            )
        }
    )
    with pytest.raises(HardwareMismatch, match="1 visible GPU"):
        miner._devices()
    # And the launcher itself refuses device lists that do not match layout.n_gpus.
    with pytest.raises(IslandFailure, match="need 2 distinct"):
        launch_island(job, tmp_path, backend="cuda", devices=(f"GPU-{U}0",))
    with pytest.raises(IslandFailure, match="need 2 distinct"):
        launch_island(job, tmp_path, backend="cpu", devices=(f"GPU-{U}0", f"GPU-{U}1"))
    assert not (tmp_path / "published").exists()
