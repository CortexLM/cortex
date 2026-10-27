# ruff: noqa: F811
"""The generic CUDA executor with results through R2 (docs/adr/0013-executor-artifacts-via-r2.md).

The whole path runs: `cuda_executor.load` builds the artifact executor from the settings file, the
fake ssh and docker host (tests/gpu_ops/fake_cuda_host.py) execute the shipped runner, the runner
talks real HTTP to a loopback server in front of a fake R2, and the challenge side publishes with
the unchanged result reader. Only the island and the GPU are canned. Not GPU evidence."""

from __future__ import annotations

import json
import os
import shutil
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from hypertrain.gpu_ops import cuda_executor
from hypertrain.gpu_ops.cuda_executor import ARTIFACTS_ENV
from hypertrain.miner.island_launch import IslandFailure

from .artifact_fakes import MiB, TimedFakeR2, credentials_file, serve_http
from .test_cuda_executor_dispatch import (  # noqa: F401 - fixtures and helpers of the harness
    _hang_guard,
    assert_nothing_published,
    canned,
    dispatch,
    journal_kinds,
    journal_of,
    make_pool,
    network,
    publication_digest,
    record_of,
)


@pytest.fixture
def r2(tmp_path: Path) -> Iterator[tuple[TimedFakeR2, Any]]:
    store = TimedFakeR2()
    front = serve_http(store)
    yield store, front
    front.close()


def enable(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, front: Any, **settings: Any) -> Path:
    """Write the settings file next to a credential file that points at the loopback server."""
    base = tmp_path / "artifacts"
    base.mkdir(exist_ok=True)
    body = {
        "credentials_file": str(credentials_file(base, front.base)),
        "part_bytes": 8 * MiB,
        "poll_ms": 10,
        "retry_base_ms": 5,
        "workers": 3,
        "offer_wait_seconds": 30,
        "max_artifact_bytes": 1 << 30,
        **settings,
    }
    path = base / "artifacts.json"
    path.write_text(json.dumps(body))
    monkeypatch.setenv(ARTIFACTS_ENV, str(path))
    return path


def pad(pool: Any, tmp_path: Path, mib: int) -> None:
    """The canned rank tree plus a rank file of `mib` MiB: a result of several 8 MiB parts."""
    tree = tmp_path / "padded"
    shutil.copytree(pool.canned.tree, tree)
    (tree / "rank-0" / "pad.bin").write_bytes(os.urandom(mib * MiB))
    pool.toolbox.scenario.set_global(canned={pool.canned.job.digest(): str(tree)})


def run_end(pool: Any) -> dict[str, Any]:
    (event,) = pool.toolbox.events("run_end")
    return event


def parts_sent(store: TimedFakeR2) -> int:
    return len([row for row in store.served("UploadPart") if row[2] == 200])


def objects_left(store: TimedFakeR2) -> list[str]:
    return sorted(store.objects)


# -- the result of the job ----------------------------------------------------------------------


def test_the_result_comes_through_r2_and_publishes_what_the_classic_path_publishes(
    network, make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",))
    pad(pool, tmp_path, 20)
    classic = canned.stage(tmp_path / "classic")
    dispatch(pool, classic, "audit")
    classic_run = run_end(pool)

    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, "audit")

    assert publication_digest(home / "published") == publication_digest(classic / "published")
    record, base = record_of(home), record_of(classic)
    assert record["publication_sha256"] == base["publication_sha256"]
    artifact = record.pop("artifact")
    assert artifact["transport"] == "r2" and artifact["sha256"] == record["result_tar_sha256"]
    assert artifact["parts"] == 3 and artifact["part_bytes"] == 8 * MiB
    assert all(type(v) in (int, str) for v in artifact.values())
    network.store._dispatch_env_v2(canned.manifest, home)  # the store still accepts the record

    # nothing but a status line came back on ssh; the tar went to R2 in 3 parts and is gone again
    r2_run = [e for e in pool.toolbox.events("run_end") if e["name"] != classic_run["name"]][0]
    assert classic_run["stdout_bytes"] > 20 * MiB and r2_run["stdout_bytes"] < 1024
    assert parts_sent(store) == 3 and store.uploads == {}
    assert objects_left(store) == []  # object, block list and mailbox: all deleted
    kinds = journal_kinds(pool)
    assert kinds.count("exec_artifact_begin") == kinds.count("exec_artifact_end") == 1
    since = kinds[kinds.index("exec_artifact_begin") :]
    assert since.index("exec_artifact_done") < since.index("exec_published")


def test_an_operation_that_is_not_listed_never_touches_the_store(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",))
    enable(tmp_path, monkeypatch, front)
    dispatch(pool, canned.stage(tmp_path / "home"), "reference")  # served: audit and referee
    assert store.ledger == [] and run_end(pool)["stdout_bytes"] > 1024


def test_a_store_that_is_down_at_the_start_leaves_the_job_to_the_ssh_path(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    store.fault_key("HeadObject", "declare.json", *([503] * 10))
    pool = make_pool(labels=("a",))
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, "audit")

    assert "artifact" not in record_of(home) and run_end(pool)["stdout_bytes"] > 1024
    assert journal_kinds(pool).count("exec_artifact_unavailable") == 1
    assert store.served("UploadPart") == []


def test_a_host_lost_after_its_last_part_does_not_cost_a_second_attempt(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",))
    pad(pool, tmp_path, 20)
    pool.toolbox.scenario.global_runs({"mode": "vanish"})  # ssh cut once the upload is complete
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, "audit")

    assert len(journal_of(pool, "exec_attempt")) == 1
    assert len(journal_of(pool, "exec_artifact_salvaged")) == 1
    assert record_of(home)["artifact"]["parts"] == 3 and store.uploads == {}


def test_a_host_lost_mid_upload_publishes_nothing_and_the_next_attempt_sends_everything(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",))
    pad(pool, tmp_path, 20)
    pool.toolbox.scenario.global_runs({"mode": "ok", "die_after_puts": 1}, {"mode": "ok"})
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, "audit")

    assert len(journal_of(pool, "exec_host_unavailable")) == 1  # a lost host is not an attempt
    begun = [r["nonce"] for r in journal_of(pool, "exec_artifact_begin")]
    assert len(begun) == 2 and len(set(begun)) == 2
    keys = {n: [row for row in store.served("UploadPart") if f"/{n}/" in row[1]] for n in begun}
    assert len(keys[begun[0]]) <= 1 and len([r for r in keys[begun[1]] if r[2] == 200]) == 3
    assert store.uploads == {}  # the first attempt's open upload was aborted
    assert record_of(home)["artifact"]["sha256"] != ""
    assert (home / "published").is_dir()


def test_a_host_without_a_route_to_the_store_is_skipped_and_not_counted(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    store.fault_key("GetObject", "offer.json", *([503] * 4))  # the container's preflight
    pool = make_pool(labels=("a",))
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, "audit")

    (unavailable,) = journal_of(pool, "exec_host_unavailable")
    assert "cannot reach the artifact store" in unavailable["reason"]
    assert journal_of(pool, "exec_attempt_failed") == [] and (home / "published").is_dir()
    assert len(pool.toolbox.events("launch")) == 1  # the island ran once, after the host answered


def test_a_store_that_fails_during_the_upload_fails_the_attempt_with_nothing_left(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    store.fault_key("UploadPart", "", *([503] * 60))
    pool = make_pool(labels=("a",), config={"max_attempts": 1})
    pad(pool, tmp_path, 20)
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="artifact transfer failed: HTTP 503"):
        dispatch(pool, home, "audit")

    assert_nothing_published(home)
    assert store.uploads == {} and objects_left(store) == []
    assert len(journal_of(pool, "exec_attempt_failed")) == 1


def test_an_answering_thread_that_dies_ends_the_attempt_and_removes_the_container(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    store.fault_key("HeadObject", "declare.json", None, *([503] * 2000))  # the probe passes
    pool = make_pool(labels=("a",), config={"max_attempts": 1})
    enable(
        tmp_path, monkeypatch, front, retry_attempts=2, offer_wait_seconds=60, store_blip_seconds=1
    )
    home = canned.stage(tmp_path / "home")
    started = time.monotonic()
    with pytest.raises(IslandFailure, match="offer thread: R2Error"):
        dispatch(pool, home, "audit")
    # the container was stopped at once, not left to wait out its minute for an offer
    assert time.monotonic() - started < 20

    assert_nothing_published(home)
    assert pool.toolbox.events(
        "rm"
    )  # docker rm -f of its own container, not a wait for the deadline


def test_a_result_over_the_cap_is_refused_and_the_attempt_says_so(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",), config={"max_attempts": 1})
    pad(pool, tmp_path, 20)
    enable(tmp_path, monkeypatch, front, max_artifact_bytes=1 * MiB)
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match=r"declaration refused \(too_large\)"):
        dispatch(pool, home, "audit")

    assert_nothing_published(home)
    assert store.served("CreateMultipartUpload") == [] and store.served("UploadPart") == []


def test_one_executor_serves_an_r2_job_and_then_a_classic_one_without_carrying_anything_over(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",))
    enable(tmp_path, monkeypatch, front)
    factory = cuda_executor.load(pool.config_path)
    first = canned.stage(tmp_path / "first")
    dispatch(pool, first, "audit", factory=factory)
    assert record_of(first)["artifact"]["transport"] == "r2"

    store.fault_key("HeadObject", "declare.json", *([503] * 10))  # R2 is gone for the next job
    second = canned.stage(tmp_path / "second")
    dispatch(pool, second, "audit", factory=factory)
    assert "artifact" not in record_of(second)


def test_a_host_lost_when_the_store_cannot_say_what_it_holds_is_still_a_lost_host(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",))
    pad(pool, tmp_path, 20)
    pool.toolbox.scenario.global_runs({"mode": "vanish"}, {"mode": "ok"})
    # the container's own listing passes; the challenge's answer to "are all parts there?" does not
    store.fault_key("ListParts", "", None, 503, 503, 503, 503)
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, "audit")

    assert len(journal_of(pool, "exec_host_unavailable")) == 1  # not an escaped storage error
    assert pool.toolbox.events("rm")  # the lost host's container was removed by name
    assert len(journal_of(pool, "exec_attempt")) == 2 and (home / "published").is_dir()


def test_artifacts_refuse_a_cap_above_what_the_result_reader_accepts(
    make_pool, tmp_path, monkeypatch, r2
):
    _, front = r2
    pool = make_pool(labels=("a",), config={"max_result_bytes": 2 * MiB})
    enable(tmp_path, monkeypatch, front, max_artifact_bytes=64 * MiB)
    with pytest.raises(ValueError, match="max_result_bytes"):
        cuda_executor.load(pool.config_path)  # _read_result would refuse the tar after the transfer


def test_a_host_that_floods_stdout_in_r2_mode_is_cut_at_a_status_line_not_at_16_gib(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",), config={"max_attempts": 1})
    pool.toolbox.scenario.global_runs({"mode": "oversize", "bytes": 4 * MiB, "linger": 20})
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    started = time.monotonic()
    with pytest.raises(IslandFailure, match="exceeds max_result_bytes"):
        dispatch(pool, home, "audit")

    assert time.monotonic() - started < 15 and store.uploads == {}


# -- the credential and the settings, as the challenge loads them ------------------------------


def test_artifacts_need_the_container_to_reach_the_store(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    _, front = r2
    pool = make_pool(labels=("a",), config={"network": "none"})
    enable(tmp_path, monkeypatch, front)
    with pytest.raises(ValueError, match="network"):
        cuda_executor.load(pool.config_path)  # the challenge maps ValueError to 503


def test_a_credential_file_that_others_can_read_stops_the_executor_from_loading(
    make_pool, tmp_path, monkeypatch, r2
):
    _, front = r2
    pool = make_pool(labels=("a",))
    path = enable(tmp_path, monkeypatch, front)
    Path(json.loads(path.read_text())["credentials_file"]).chmod(0o644)
    with pytest.raises(ValueError, match="mode 0600"):
        cuda_executor.load(pool.config_path)


def find(needles: set[str], roots: list[Path]) -> dict[str, str]:
    """Which file under the roots holds which needle."""
    files = [p for root in roots for p in sorted(root.rglob("*")) if p.is_file()]
    return {str(p): n for p in files for n in sorted(needles) if n.encode() in p.read_bytes()}


def test_nothing_of_a_url_is_left_where_the_challenge_or_the_host_keeps_files(
    make_pool, canned, tmp_path, monkeypatch, r2
):
    store, front = r2
    pool = make_pool(labels=("a",))
    pad(pool, tmp_path, 20)
    store.fault_key("UploadPart", "", 503, None, None, None)  # one retried part on the way
    enable(tmp_path, monkeypatch, front)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, "audit")

    used = set(store.signatures)  # the signatures the container really sent: mailbox, parts, lists
    assert len(used) >= 5
    roots = [pool.state_dir, home, pool.toolbox.root, tmp_path / "artifacts"]
    assert find(used, roots) == {}
    # the scan can see one: plant a signature in the tree and look again
    planted = home / "planted.txt"
    planted.write_text(sorted(used)[0])
    assert find(used, roots) == {str(planted): sorted(used)[0]}
