from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import httpx
import pytest
from aud_fixtures import manifest
from test_island_replay_v2 import fixture

from hypertrain.auditor import __main__ as cli
from hypertrain.auditor.__main__ import RefusedJob, RunScopedApi, audit_v2, main
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RunManifestV2


def _api(run_id: str, calls: list[tuple[str, Any]]) -> RunScopedApi:
    job = {"id": "j1", "lease": "l1", "manifest": manifest().body()}

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append((request.url.path, json.loads(request.content or b"null")))
        if request.url.path == "/v1/worker/lease":
            return httpx.Response(200, json=job)
        return httpx.Response(200, json={"ok": True})

    return RunScopedApi(
        httpx.Client(transport=httpx.MockTransport(handler)), "t", "http://c", run_id
    )


def test_foreign_run_job_is_handed_back_not_replayed() -> None:
    calls: list[tuple[str, Any]] = []
    assert _api("cd" * 32, calls).lease() is None
    assert calls[-1] == (
        "/v1/worker/jobs/j1/fail",
        {"lease": "l1", "reason": "auditor serves another run", "retry": True},
    )


def test_own_run_job_is_returned() -> None:
    calls: list[tuple[str, Any]] = []
    raw = _api(manifest().run_id(), calls).lease()
    assert raw is not None and raw["id"] == "j1" and len(calls) == 1


def test_contract_one_stays_default_and_requires_token_and_shards() -> None:
    with pytest.raises(SystemExit) as exit_info:
        main(["--api", "http://c", "--run-id", "ab" * 32, "--keyfile", "k"])
    assert exit_info.value.code == 2


COORD = Keypair(bytes(range(32)))  # example manifest coord_pubkey
AUDITOR = Keypair(bytes([45]) * 32)  # auditor bound into the fixture job


@pytest.fixture(scope="module")
def job() -> Any:
    return fixture()[0]


def _v2(job: Any, **fault: Any) -> tuple[httpx.Client, list[tuple[str, Any]]]:
    run = job.run_id
    verdict = {
        "challenge_hash": fault.get("challenge", body_digest(job.challenge_envelope["body"])),
        "first_bad_leaf": None,
        "result": "MATCH",
        "recomputed_leaves_root": "66" * 32,
        "replay_env": {
            "image_digest": job.manifest.training.reference_spec.image_digest,
            "driver": "cpu",
            "gpu_uuid_sha256": "77" * 32,
            "sm_count": 1,
        },
    }
    lease = envelope_v2.seal(fault.get("signer", COORD), "AuditJobV2", run, job, 10_000)
    calls: list[tuple[str, Any]] = []
    timeouts: list[float] = fault.setdefault("timeouts", [])
    queue = [lease] * fault.get("jobs", 1)

    busy = fault.get("busy", {})  # path suffix -> how many 503 (beacon pause) answers first

    def handler(request: httpx.Request) -> httpx.Response:
        path = request.url.path.removeprefix(f"/v2/runs/{run}")
        body = json.loads(request.content or b"null")
        calls.append((path, body))
        for suffix, left in busy.items():
            if body is not None and path.endswith(suffix) and left:
                busy[suffix] = left - 1
                return httpx.Response(503, json={"detail": "beacon infrastructure paused"})
        if path.endswith(("/execute", "/complete")):
            timeouts.append(request.extensions["timeout"]["read"])
        if path == "":
            served = fault.get("manifest", job.manifest)
            return httpx.Response(
                200,
                json={"contract": 2, "now_round": 3, "manifest_envelope": {"body": served.body()}},
            )
        if path == "/worker/lease":
            if "lease" in fault:
                return fault["lease"]
            if queue:
                return httpx.Response(200, json=queue.pop())
            empty = fault.get("empty", 204)
            detail = {"detail": "SERVICE_RUNTIME_NOT_ENFORCED: no authenticated queued lease"}
            return httpx.Response(empty, json=detail)
        if path.endswith("/execute"):
            if fault.get("stall"):
                raise httpx.ReadTimeout("master stalled", request=request)
            return httpx.Response(200, json={"verdict": verdict})
        assert path.endswith("/complete") and body["body"] == verdict  # signs master's verdict
        return httpx.Response(200, json={"result": verdict["result"]})

    return httpx.Client(transport=httpx.MockTransport(handler)), calls


def _posts(calls: list[tuple[str, Any]]) -> list[str]:
    return [p for p, b in calls if b is not None]


def test_v2_lease_execute_complete_signs_master_verdict(job: Any, capsys: Any) -> None:
    client, calls = _v2(job)
    assert audit_v2(client, "http://m", job.run_id, AUDITOR, once=True) == 0
    jid = job.job_id
    assert _posts(calls) == [
        "/worker/lease",
        f"/worker/jobs/{jid}/execute",
        f"/worker/jobs/{jid}/complete",
    ]
    signed = [b for p, b in calls if b is not None]
    assert all(envelope_v2.verify_envelope(b) and b["signer"] == AUDITOR.ss58 for b in signed)
    assert [b["type"] for b in signed] == ["Receipt", "Receipt", "ReplayVerdict"]
    assert signed[1]["body"]["commit_hash"] == job.lease_nonce
    assert signed[0]["body"]["commit_hash"] != job.lease_nonce
    assert json.loads(capsys.readouterr().out) == {"job": jid, "result": "MATCH"}


def test_v2_service_drains_queue_then_exits_and_once_settles_one(job: Any) -> None:
    client, calls = _v2(job, jobs=2)
    assert audit_v2(client, "http://m", job.run_id, AUDITOR, once=False) == 0
    assert _posts(calls).count("/worker/lease") == 3 and _posts(calls)[-1] == "/worker/lease"
    client, calls = _v2(job, jobs=1, empty=503)
    assert audit_v2(client, "http://m", job.run_id, AUDITOR, once=False) == 0
    client, calls = _v2(job, jobs=2)
    assert audit_v2(client, "http://m", job.run_id, AUDITOR, once=True) == 0
    assert _posts(calls).count("/worker/lease") == 1


@pytest.mark.parametrize(
    "fault",
    [
        {"signer": AUDITOR},  # lease not signed by coordinator
        {"challenge": "88" * 32},  # executed verdict answers another challenge
    ],
)
def test_v2_refuses_unauthenticated_job_or_foreign_verdict(job: Any, fault: Any) -> None:
    client, calls = _v2(job, **fault)
    with pytest.raises(RefusedJob):
        audit_v2(client, "http://m", job.run_id, AUDITOR, once=True)
    assert not any(p.endswith("/complete") for p in _posts(calls))


def test_v2_refuses_other_auditor_and_wrong_run(job: Any) -> None:
    client, calls = _v2(job)
    with pytest.raises(RefusedJob):
        audit_v2(client, "http://m", job.run_id, Keypair(bytes([46]) * 32), once=True)
    assert _posts(calls) == ["/worker/lease"]
    body = job.manifest.body()
    body["training"]["init_state_hash"] = "99" * 32
    client, calls = _v2(job, manifest=RunManifestV2.model_validate(body))
    with pytest.raises(RefusedJob):
        audit_v2(client, "http://m", job.run_id, AUDITOR, once=True)
    assert _posts(calls) == []


def test_v2_execute_bounded_by_lease_wall_deadline_complete_by_margin(job: Any) -> None:
    beacon = job.manifest.training.beacon
    deadline = beacon.genesis_time + (job.lease_expires - 1) * beacon.period
    seen: list[float] = []
    client, _ = _v2(job, timeouts=seen)
    audit_v2(client, "http://m", job.run_id, AUDITOR, once=True, clock=lambda: deadline - 30)
    assert seen == [30 + cli.MARGIN_S, cli.MARGIN_S]
    seen.clear()
    client, _ = _v2(job, timeouts=seen)
    audit_v2(client, "http://m", job.run_id, AUDITOR, once=True, clock=lambda: deadline + 999)
    assert seen == [cli.MARGIN_S, cli.MARGIN_S]


@pytest.mark.parametrize(
    "fault",
    [
        {"stall": True},
        {"lease": httpx.Response(200, json={"type": "AuditJobV2", "body": "garbage"})},
        {"lease": httpx.Response(409, json={"detail": "one live lease per auditor"})},
    ],
)
def test_v2_cli_exits_2_without_complete_on_stall_malformed_or_live_lease(
    job: Any, fault: Any, monkeypatch: Any, tmp_path: Path
) -> None:
    client, calls = _v2(job, **fault)
    monkeypatch.setattr(cli.httpx, "Client", lambda: client)
    monkeypatch.setattr(cli, "load_keyfile", lambda _: AUDITOR)
    argv = ["--api", "http://m", "--run-id", job.run_id, "--keyfile", str(tmp_path / "k")]
    assert main([*argv, "--contract", "2"]) == 2
    assert not any(p.endswith("/complete") for p in _posts(calls))


class Wall:
    """Fake wall clock; `sleep` moves it."""

    def __init__(self, at: float) -> None:
        self.at, self.slept = at, []

    def __call__(self) -> float:
        return self.at

    def sleep(self, seconds: float) -> None:
        self.slept.append(seconds)
        self.at += seconds


def _deadline(job: Any) -> float:
    beacon = job.manifest.training.beacon
    return beacon.genesis_time + (min(job.lease_expires, job.absolute_deadline) - 1) * beacon.period


def test_v2_a_beacon_pause_on_lease_and_complete_is_waited_out(job: Any) -> None:
    # S17d: the master answered 503 "beacon infrastructure paused" while its pusher lagged.
    client, calls = _v2(job, busy={"/worker/lease": 2, "/complete": 2})
    wall = Wall(_deadline(job) - 60)
    assert audit_v2(client, "http://m", job.run_id, AUDITOR, True, wall, wall.sleep) == 0
    jid = job.job_id
    assert _posts(calls) == [
        *["/worker/lease"] * 3,
        f"/worker/jobs/{jid}/execute",
        *[f"/worker/jobs/{jid}/complete"] * 3,
    ]
    assert wall.slept == [cli.RETRY_S] * 4
    completes = [b for p, b in calls if p.endswith("/complete")]
    assert completes[0] == completes[1] == completes[2]  # the same signed verdict, re-posted


def test_v2_complete_stops_retrying_at_the_lease_wall_deadline(job: Any) -> None:
    client, calls = _v2(job, busy={"/complete": 1000})
    wall = Wall(_deadline(job) - 10)
    with pytest.raises(httpx.HTTPStatusError, match="503"):
        audit_v2(client, "http://m", job.run_id, AUDITOR, True, wall, wall.sleep)
    assert _posts(calls).count(f"/worker/jobs/{job.job_id}/complete") == 4
    assert wall() < _deadline(job) and wall.slept == [cli.RETRY_S] * 3


def test_v2_execute_is_posted_once_and_a_lease_outage_is_bounded(job: Any) -> None:
    client, calls = _v2(job, busy={"/execute": 1})
    wall = Wall(_deadline(job) - 60)
    with pytest.raises(httpx.HTTPStatusError, match="503"):
        audit_v2(client, "http://m", job.run_id, AUDITOR, True, wall, wall.sleep)
    assert _posts(calls).count(f"/worker/jobs/{job.job_id}/execute") == 1 and wall.slept == []
    client, calls = _v2(job, busy={"/worker/lease": 1000})
    wall = Wall(0.0)
    with pytest.raises(httpx.HTTPStatusError, match="503"):
        audit_v2(client, "http://m", job.run_id, AUDITOR, True, wall, wall.sleep)
    assert _posts(calls) == ["/worker/lease"] * 100 and wall() < cli.LEASE_RETRY_S
