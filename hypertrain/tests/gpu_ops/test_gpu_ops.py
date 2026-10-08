from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import urllib.response
from decimal import Decimal
from pathlib import Path
from typing import Any

import pytest

from hypertrain.gpu_ops import budget, provider
from hypertrain.gpu_ops.journal import Journal
from hypertrain.gpu_ops.provider import KeyFileError, Provider, RefusedWrite, read_api_key, redact

from .conftest import KEY, TREE, journal, launch, offer, wait_pid_exit

CHECKER = TREE / "scripts/check_gpu_evidence.py"


def kinds(run_dir: Path, kind: str) -> list[dict[str, Any]]:
    return [r for r in journal(run_dir) if r["kind"] == kind]


def no_leftovers(mock: Any) -> None:
    st = mock.state()
    assert all(r["deleted"] for r in st["instances"].values())


def test_redaction_masks_key_query_bearer_and_literal() -> None:
    raw = f"GET /api/v0/x?api_key={KEY}&q=1 Authorization: Bearer {KEY} echo {KEY}"
    out = redact(raw, KEY)
    assert KEY not in out
    assert "api_key=***" in out and "Bearer ***" in out
    assert redact("api_key=zzz9", None) == "api_key=***"


def test_key_file_requires_0600(tmp_path: Path) -> None:
    p = tmp_path / "k"
    p.write_text(KEY)
    os.chmod(p, 0o644)
    with pytest.raises(KeyFileError):
        read_api_key(p)
    os.chmod(p, 0o600)
    assert read_api_key(p) == KEY


def test_non_loopback_write_refused_without_live(tmp_path: Path) -> None:
    j = Journal(tmp_path)
    p = Provider("https://console.vast.ai", KEY, tmp_path, j, live=False)
    with pytest.raises(RefusedWrite):
        p.call("PUT", "/api/v0/asks/1/", "t", body={})
    assert j.all("provider_call") == []


def test_raw_logs_never_contain_key(mock_factory: Any, tmp_path: Path) -> None:
    m = mock_factory()
    j = Journal(tmp_path / "r")
    (tmp_path / "r").mkdir(exist_ok=True)
    p = Provider(m.base, KEY, tmp_path / "r", j, live=False)
    r = p.call("GET", f"/api/v0/users/current/?api_key={KEY}", "acct")
    assert r.ok()
    blob = b"".join(f.read_bytes() for f in (tmp_path / "r").rglob("*") if f.is_file())
    assert KEY.encode() not in blob


def test_budget_gate_admits_within_cap_and_rejects_over() -> None:
    offers = [offer(i) for i in range(3)]
    ok = budget.evaluate(
        snapshot0_credit=Decimal("28"),
        current_credit=Decimal("28"),
        offers=offers,
        hard_deadline_seconds=3600,
        disk_gb=20,
        egress_gb=2,
    )
    assert ok["admit"], ok
    assert Decimal(ok["worst_case_total_usd"]) == Decimal("5") + 3 * Decimal("0.43")
    over = budget.evaluate(
        snapshot0_credit=Decimal("28"),
        current_credit=Decimal("28"),
        offers=offers,
        hard_deadline_seconds=6 * 3600,
        disk_gb=20,
        egress_gb=2,
    )
    assert not over["admit"] and not over["checks"]["phase_cap"]
    poor = budget.evaluate(
        snapshot0_credit=Decimal("28"),
        current_credit=Decimal("6"),
        offers=offers,
        hard_deadline_seconds=3600,
        disk_gb=20,
        egress_gb=2,
    )
    assert not poor["admit"] and not poor["checks"]["credit_covers"]


def test_mock_enforces_live_paths(mock_factory: Any, tmp_path: Path) -> None:
    m = mock_factory()
    j = Journal(tmp_path)
    p = Provider(m.base, KEY, tmp_path, j, live=False)
    assert p.call("POST", "/api/v0/instances/1/ssh", "noslash", body={}).status == 308
    assert p.call("GET", "/api/v0/instances/", "v0list").status == 410
    assert p.call("GET", "/api/v0/users/current", "noslash2").status == 308


def test_put_spacing_evidence_ignores_raw_persistence_delay(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    now = 100.0
    starts: list[float] = []
    completions: list[float] = []
    persisted = 0
    write = provider.durable_write
    monkeypatch.setenv("HT_GPU_VIRTUAL_TIME", "1")
    monkeypatch.setattr(provider.time, "monotonic", lambda: now)
    j = Journal(tmp_path)
    p = Provider("http://127.0.0.1", KEY, tmp_path, j, live=False)
    endpoint = p.endpoint("PUT", "/api/v0/asks/900/")
    p.pace[endpoint] = 5.0

    def respond(request: Any, timeout: float) -> Any:
        starts.append(p.monotonic())
        completions.append(p.monotonic())
        return urllib.response.addinfourl(
            io.BytesIO(b'{"success": true}'), {}, request.full_url, 200
        )

    def persist(path: Any, data: bytes) -> None:
        nonlocal now, persisted
        write(path, data)
        if persisted == 0:
            now += 2.0
        persisted += 1

    monkeypatch.setattr(provider._OPENER, "open", respond)
    monkeypatch.setattr(provider, "durable_write", persist)
    for i in range(2):
        assert p.call("PUT", f"/api/v0/asks/{900 + i}/", f"put-{i}", body={}).ok()

    vm = [r["vmono"] for r in j.all("provider_call", method="PUT")]
    assert starts == completions == [100.0, 105.0]
    assert vm == completions, f"response completions={completions}, journal timestamps={vm}"
    assert all(b - a >= 5.0 for a, b in zip(vm, vm[1:], strict=False))

def test_happy_e2e_three_hosts_identical_roots(
    mock_factory: Any, run_cfg: Any, tmp_path: Path
) -> None:
    m = mock_factory()
    cfg = run_cfg(m)
    rd = tmp_path / "run"
    proc = launch(cfg, rd)
    assert proc.returncode == 0, (
        proc.stdout + proc.stderr + (rd / "journal.jsonl").read_text()[-3000:]
    )
    st = m.state()
    assert st["put_count"] == 3
    ev = json.loads((tmp_path / "evidence.json").read_text())
    assert ev["inventory"] == {"parsed": True, "total_rows": 0, "owned_instances": 0}
    assert ev["negative_control"]["ok"] and ev["mismatches"] == []
    roots = {r["result"]["leaves_root"] for r in ev["runs"] if r["name"] != "neg"}
    assert len(roots) == 1 and len(ev["runs"]) == 7
    assert ev["verdict"] == "CENSORED" and ev["off_hardware_gate"]
    puts = [e["unix"] for e in st["events"] if e["method"] == "PUT"]
    assert all(b - a >= 0 for a, b in zip(puts, puts[1:], strict=False))
    vm = [r["vmono"] for r in kinds(rd, "provider_call") if r["method"] == "PUT"]
    assert all(b - a >= 5.0 for a, b in zip(vm, vm[1:], strict=False))
    assert {r["role"] for r in kinds(rd, "rescued")} == {"h0", "h1", "h2"}
    for r in kinds(rd, "rescued"):
        assert {"out/run1.json", "out/run2.json"} <= set(r["files"])
    first_delete = min(i for i, r in enumerate(journal(rd)) if r["kind"] == "delete_ack")
    last_rescue = max(i for i, r in enumerate(journal(rd)) if r["kind"] == "rescued")
    assert last_rescue < first_delete
    no_leftovers(m)
    assert wait_pid_exit(kinds(rd, "supervisor_ready")[0]["supervisor_pid"], 60)
    assert (
        subprocess.run([sys.executable, str(CHECKER), str(tmp_path / "evidence.json")]).returncode
        == 0
    )
    again = launch(cfg, rd)
    assert (
        again.returncode == 0 and "ALREADY_CLOSED" in again.stdout and m.state()["put_count"] == 3
    )


def test_429_and_malformed_list_never_absent_no_recreate(
    mock_factory: Any, run_cfg: Any, tmp_path: Path
) -> None:
    m = mock_factory(put_429_created=True, list_429=2, list_malformed=2)
    rd = tmp_path / "run"
    proc = launch(run_cfg(m), rd)
    assert proc.returncode == 0, proc.stdout + proc.stderr
    st = m.state()
    assert st["put_count"] == 3, "429 on PUT must not trigger a second CREATE for the same role"
    unknown = kinds(rd, "recover_unknown")
    assert {u["http_status"] for u in unknown} >= {429, 200}
    rec = kinds(rd, "receipt")
    assert [r["source"] for r in rec if r["role"] == "h0"] == ["recovered_after_PUT"]
    assert not kinds(rd, "create_failed")
    no_leftovers(m)


def test_crash_after_put_resume_recovers_without_second_put(
    mock_factory: Any, run_cfg: Any, tmp_path: Path
) -> None:
    m = mock_factory()
    cfg = run_cfg(m)
    rd = tmp_path / "run"
    crashed = launch(cfg, rd, env={"HT_GPU_CRASH_AT": "after_put:h1"})
    assert crashed.returncode == 137
    assert m.state()["put_count"] == 2
    resumed = launch(cfg, rd)
    assert resumed.returncode == 0, resumed.stdout + resumed.stderr
    assert m.state()["put_count"] == 3
    assert [r["source"] for r in kinds(rd, "receipt") if r["role"] == "h1"] == ["recovered"]
    no_leftovers(m)


def test_crash_before_put_with_unknown_list_does_not_recreate(
    mock_factory: Any, run_cfg: Any, tmp_path: Path
) -> None:
    m = mock_factory(list_429=50)
    cfg = run_cfg(m)
    rd = tmp_path / "run"
    first = launch(cfg, rd, env={"HT_GPU_CRASH_AT": "before_put:h1"})
    assert first.returncode == 137 and m.state()["put_count"] == 1
    resumed = launch(cfg, rd)
    assert resumed.returncode == 4, resumed.stdout
    assert m.state()["put_count"] == 1
    assert kinds(rd, "create_unresolved")[0]["state"] == "unknown"
    assert kinds(rd, "liability_open")
    assert not kinds(rd, "transaction_closed")
    sup = kinds(rd, "supervisor_ready")[0]["supervisor_pid"]
    os.kill(sup, 15)
    assert wait_pid_exit(sup, 60)


def test_supervisor_deadline_cleanup_rescues_then_deletes(
    mock_factory: Any, run_cfg: Any, tmp_path: Path
) -> None:
    m = mock_factory()
    cfg = run_cfg(
        m, hard_deadline_seconds=4, create_margin_seconds=-3600, cleanup_margin_seconds=-3600
    )
    rd = tmp_path / "run"
    crashed = launch(cfg, rd, env={"HT_GPU_CRASH_AT": "after_put:h2"})
    assert crashed.returncode == 137
    sup = kinds(rd, "supervisor_ready")[0]["supervisor_pid"]
    assert wait_pid_exit(sup, 300), "supervisor did not finish deadline cleanup"
    closed = kinds(rd, "transaction_closed")
    assert (
        closed
        and closed[0]["actor"] == "supervisor"
        and closed[0]["inventory"]["owned_instances"] == 0
    )
    assert kinds(rd, "supervisor_cleanup_started")
    no_leftovers(m)
    assert m.state()["put_count"] == 3


def test_admission_rejected_over_cap_creates_nothing(
    mock_factory: Any, run_cfg: Any, tmp_path: Path
) -> None:
    m = mock_factory()
    rd = tmp_path / "run"
    proc = launch(run_cfg(m, hard_deadline_seconds=8 * 3600), rd)
    assert proc.returncode == 2 and "CENSORED" in proc.stdout
    assert m.state()["put_count"] == 0
    ev = json.loads((tmp_path / "evidence.json").read_text())
    assert ev["verdict"] == "CENSORED" and ev["inventory"]["owned_instances"] == 0


def test_host_key_change_rejected(mock_factory: Any, run_cfg: Any, tmp_path: Path) -> None:
    from hypertrain.gpu_ops.remote import Ssh, TrustError, ensure_identity

    m = mock_factory()
    j = Journal(tmp_path)
    p = Provider(m.base, KEY, tmp_path, j, live=False)
    r = p.call("PUT", "/api/v0/asks/900/", "put", body={"label": "x", "image": "i"})
    port = m.state()["instances"][str(r.parsed["new_contract"])]["ssh_port"]
    kh = tmp_path / "kh"
    s = Ssh("127.0.0.1", port, "root", ensure_identity(tmp_path), kh, j, "h0")
    assert s.trust(20) == "pinned"
    assert s.trust(20) == "verified"
    kh.write_text(f"[127.0.0.1]:{port} ssh-ed25519 {'A' * 68}\n")
    with pytest.raises(TrustError):
        s.trust(20)
    with pytest.raises(TrustError):
        Ssh("127.0.0.1;rm -rf /", port, "root", kh, kh, j, "x")


def _ev(**over: Any) -> dict[str, Any]:
    leaves = ["11" * 32, "22" * 32]
    res = {
        "leaves": leaves,
        "leaves_root": "aa" * 32,
        "delta_hash": "bb" * 32,
        "final_theta_hash": "dd" * 32,
        "env": {"sm_count": 170, "device": "cuda"},
    }
    neg = dict(
        res, leaves=[leaves[0], "33" * 32], leaves_root="cc" * 32, expected_first_divergent_leaf=1
    )
    hosts = [
        {"role": f"h{i}", "machine_id": 10 + i, "driver_version": f"580.95.0{i}"} for i in range(3)
    ]
    runs = [
        {"role": f"h{i}", "name": f"run{k}", "machine_id": 10 + i, "result": res}
        for i in range(3)
        for k in (1, 2)
    ]
    runs.append({"role": "h0", "name": "neg", "machine_id": 10, "result": neg})
    ev = {
        "verdict": "PASS",
        "image": "docker.io/x@sha256:" + "ab" * 32,
        "image_digest": "sha256:" + "ab" * 32,
        "hosts": hosts,
        "runs": runs,
        "inventory": {"parsed": True, "total_rows": 0, "owned_instances": 0},
        "cost": {"cap_usd": "10", "lines": [{"item": "debit", "usd": "3.10"}], "total_usd": "3.10"},
    }
    ev.update(over)
    return ev


@pytest.mark.parametrize(
    ("over", "ok"),
    [
        ({}, True),
        ({"inventory": {"parsed": True, "total_rows": 1, "owned_instances": 1}}, False),
        ({"inventory": {"parsed": False, "total_rows": None, "owned_instances": None}}, False),
        (
            {
                "cost": {
                    "cap_usd": "10",
                    "lines": [{"item": "d", "usd": "10.01"}],
                    "total_usd": "10.01",
                }
            },
            False,
        ),
        ({"image_digest": None}, False),
        ({"verdict": "CENSORED", "runs": []}, True),
        (
            {
                "hosts": [
                    {"role": f"h{i}", "machine_id": 10 + i, "driver_version": "1"} for i in range(3)
                ]
            },
            False,
        ),
        (
            {
                "runs": [
                    dict(r, result=dict(r["result"], env={"sm_count": 170, "device": "cpu"}))
                    for r in _ev()["runs"]
                ]
            },
            False,
        ),
        (
            {
                "runs": [
                    dict(r, result=dict(r["result"], final_theta_hash="ee" * 32))
                    if r["role"] == "h2" and r["name"] == "run1"
                    else r
                    for r in _ev()["runs"]
                ]
            },
            False,
        ),
        ({"raw": {"instance_api_key": "7875e0437f396a1d"}}, False),
        ({"raw": {"instance_api_key": "***"}}, True),
    ],
)
def test_check_gpu_evidence(tmp_path: Path, over: dict[str, Any], ok: bool) -> None:
    f = tmp_path / "ev.json"
    f.write_text(json.dumps(_ev(**over)))
    rc = subprocess.run([sys.executable, str(CHECKER), str(f)], capture_output=True).returncode
    assert (rc == 0) is ok


def test_check_gpu_evidence_leftover_fixture() -> None:
    fixture = Path(__file__).parent / "fixtures/leftover-instance.json"
    proc = subprocess.run(
        [sys.executable, str(CHECKER), str(fixture)], capture_output=True, text=True
    )
    assert proc.returncode == 1 and "owned_instances must be 0" in proc.stdout


def test_journal_torn_tail_quarantined(tmp_path: Path) -> None:
    j = Journal(tmp_path)
    j.append("a", x=1)
    with open(j.path, "a") as f:
        f.write('{"kind": "b", "x"')
    j2 = Journal(tmp_path)
    j2.append("c")
    assert [r["kind"] for r in j2.records()] == ["a", "journal_repaired", "c"]
    assert list(tmp_path.glob("journal.torn-*.bin"))
