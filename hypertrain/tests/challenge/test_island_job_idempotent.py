"""A miner resuming after a crash gets the same staged island job back, never a 500."""

from __future__ import annotations

import importlib.util
import os
import sys
import time
from pathlib import Path

import pytest

from hypertrain.auditor.replay import pack_state
from hypertrain.challenge.store import ChallengeError, _stage_once
from hypertrain.data.store import put_large
from hypertrain.protocol.messages_v2 import StartStateV2
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

spec = importlib.util.spec_from_file_location(
    "island_job_idempotent_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)
network = fixtures.network
HOT = fixtures.HOT[0].ss58


def staged_round(network, monkeypatch, shift: float = 0.0, global_step0: int = 0):
    """Real store, objects, dataset and records table; only round/start authority is pinned."""
    store = network.store.for_run(network.manifest.run_id())
    theta = init_params(TrainConfig.from_manifest_v2(network.manifest).model)
    theta = {name: value + shift for name, value in theta.items()}
    start = StartStateV2(
        run_id=network.manifest.run_id(),
        w=0,
        hotkey=HOT,
        theta_hash="11" * 32,
        state_object_sha256=put_large(pack_state(theta), store.objects.put),
        opt_state_hash="22" * 32,
        ef_object_sha256=put_large(pack_state(theta), store.objects.put),
        ef_hash="33" * 32,
        parent_anchor_hash="44" * 32,
        global_step0=global_step0,
        anchor_verdict_hash="55" * 32,
    )
    real = type(store)._record_v2
    pinned = {"start": start.body(), "round": {"body": {"d_commit": 10, "roster": []}}}
    monkeypatch.setattr(
        store,
        "_record_v2",
        lambda run, kind, identity: pinned.get(kind) or real(store, run, kind, identity),
    )
    monkeypatch.setattr(store, "_owner_v2", lambda *_: fixtures.COLD[0].ss58)
    batch = network.manifest.training.batch_samples()
    monkeypatch.setattr(store, "_assignment_v2", lambda *_: tuple(range(batch)))
    return store, store.state_dir / "jobs-v2" / "0" / HOT


def test_island_job_reemission_is_idempotent(network, monkeypatch):
    store, _ = staged_round(network, monkeypatch)
    first = store.island_job_v2(network.manifest.run_id(), 0, HOT)
    assert store.island_job_v2(network.manifest.run_id(), 0, HOT) == first


def test_island_job_reemission_refuses_altered_staged_input(network, monkeypatch):
    store, directory = staged_round(network, monkeypatch)
    store.island_job_v2(network.manifest.run_id(), 0, HOT)
    altered = bytearray((directory / "samples").read_bytes())
    altered[0] ^= 1
    (directory / "samples").write_bytes(altered)
    with pytest.raises(ChallengeError) as refused:
        store.island_job_v2(network.manifest.run_id(), 0, HOT)
    assert refused.value.status == 409
    assert (directory / "samples").read_bytes() == altered


def test_island_job_reemission_ignores_crash_leftover_temp_file(network, monkeypatch):
    store, directory = staged_round(network, monkeypatch)
    first = store.island_job_v2(network.manifest.run_id(), 0, HOT)
    # SIGKILL mid-write of the next input: its final name absent, a torn temp left behind.
    (directory / "ef_in").unlink()
    (directory / ".ef_in.k3j9x2q1.part").write_bytes(b"torn")  # the mkstemp temp name
    assert store.island_job_v2(network.manifest.run_id(), 0, HOT) == first
    assert (directory / "ef_in").read_bytes() == (directory / "start_state").read_bytes()


# theta: the staged input bytes differ; global_step0: same bytes, only the job record differs.
@pytest.mark.parametrize("change", [{"shift": 1.0}, {"global_step0": 7}], ids=["theta", "step"])
def test_island_job_reemission_refuses_different_start_state(network, monkeypatch, change):
    store, directory = staged_round(network, monkeypatch)
    first = store.island_job_v2(network.manifest.run_id(), 0, HOT)
    before = (directory / "start_state").read_bytes()
    store, _ = staged_round(network, monkeypatch, **change)
    with pytest.raises(ChallengeError) as refused:
        store.island_job_v2(network.manifest.run_id(), 0, HOT)
    assert refused.value.status == 409
    assert (directory / "start_state").read_bytes() == before
    assert store._record_v2(network.manifest.run_id(), "island-job", "0:" + HOT) == first["job"]


def test_island_job_route_second_get_is_200_same_body(network, monkeypatch):
    staged_round(network, monkeypatch)
    first = network.client.get(network.url + "/rounds/0/job/" + HOT)
    assert first.status_code == 200, first.text
    second = network.client.get(network.url + "/rounds/0/job/" + HOT)
    assert second.status_code == 200, second.text
    assert second.json() == first.json()


def _part(path: Path, data: bytes, age: float = 0.0) -> Path:
    path.write_bytes(data)
    when = time.time() - age
    os.utime(path, (when, when))
    return path


def test_stage_once_purges_stale_part_files_of_its_own_name(tmp_path):
    stale = _part(tmp_path / ".start_state.abc123.part", b"torn", age=3600)
    _stage_once(tmp_path / "start_state", b"x")
    assert not stale.exists()
    assert (tmp_path / "start_state").read_bytes() == b"x"


def test_stage_once_keeps_fresh_part_files_and_other_names(tmp_path):
    stale = _part(tmp_path / ".start_state.abc123.part", b"torn", age=3600)
    fresh = _part(tmp_path / ".start_state.def456.part", b"live")  # a concurrent emission's temp
    other = _part(tmp_path / ".ef_in.zzz.part", b"other", age=3600)
    _stage_once(tmp_path / "start_state", b"x")
    assert not stale.exists()
    assert fresh.read_bytes() == b"live"
    assert other.read_bytes() == b"other"
    assert (tmp_path / "start_state").read_bytes() == b"x"


def test_stage_once_purge_does_not_follow_symlinks(tmp_path):
    target = _part(tmp_path / "outside", b"keep", age=3600)
    job = tmp_path / "job"
    job.mkdir()
    link = job / ".start_state.link.part"
    link.symlink_to(target)
    os.utime(link, (target.stat().st_mtime,) * 2, follow_symlinks=False)  # stale link too
    _stage_once(job / "start_state", b"x")
    assert link.is_symlink()
    assert target.read_bytes() == b"keep"
