from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import numpy as np
import pytest

import hypertrain.trainer  # noqa: F401  (determinism pin must precede any torch import)

from .conftest import ACCOUNT, DIGEST, KEY, TREE, journal

B = TREE / "experiments/gpu_phase_b"
CHECKER = TREE / "scripts/check_gpu_evidence.py"


def _load(name: str) -> Any:
    spec = importlib.util.spec_from_file_location(f"phase_b_{name}", B / f"{name}.py")
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def offer8(i: int) -> dict[str, Any]:
    return {
        "id": 800 + i,
        "machine_id": 6000 + i,
        "host_id": 90 + i,
        "gpu_name": "RTX 5090",
        "num_gpus": 8,
        "dph_total": 4.0,
        "storage_cost": 0.2,
        "inet_up_cost": 0.004,
        "inet_down_cost": 0.003,
        "driver_version": ["580.95.05", "595.84"][i],
        "geolocation": "CA",
    }


def phase_b_cfg(tmp: Path, base: str, **over: Any) -> Path:
    key = tmp / "vast-key"
    if not key.exists():
        fd = os.open(key, os.O_WRONLY | os.O_CREAT, 0o600)
        os.write(fd, KEY.encode())
        os.close(fd)
    snap = tmp / "budget-snapshot-0.json"
    snap.write_text(json.dumps({"account_id": ACCOUNT, "credit_usd": "28.00"}))
    shard = tmp / "shard.u32"
    if not shard.exists():
        rng = np.random.default_rng(0)
        shard.write_bytes(
            rng.integers(0, 259, size=(64, 1025), dtype=np.uint32).astype("<u4").tobytes()
        )
    cfg = json.loads((B / "live-config.json").read_text())
    defaults: dict[str, Any] = dict(
        base_url=base,
        key_file=str(key),
        budget_snapshot=str(snap),
        image="docker.io/vastai/pytorch@" + DIGEST,
        phase_cap_usd="27.37",
        hard_deadline_seconds=3600,
        hosts=[{"role": f"h{i}", "machine_id": 6000 + i, "max_dph_total": 4.2} for i in range(2)],
        shard=str(shard),
        remote_root=str(tmp / "remote-{role}"),
        remote_python=sys.executable,
        setup_command="",
        profile="tiny",
        device="cpu",
        evidence_path=str(tmp / "evidence.json"),
        supervisor_check_seconds=0.2,
        cleanup_margin_seconds=60,
        create_margin_seconds=600,
    )
    cfg.update({**defaults, **over})
    p = tmp / "cfg-b.json"
    p.write_text(json.dumps(cfg))
    return p


def run_b(cfg: Path, run_dir: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(B / "orchestrate.py"),
            "run",
            "--config",
            str(cfg),
            "--run-dir",
            str(run_dir),
        ],
        capture_output=True,
        text=True,
        timeout=1500,
        env={**os.environ, "HT_GPU_VIRTUAL_TIME": "1", "OMP_NUM_THREADS": "1"},
    )


def checker(path: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, str(CHECKER), str(path)], capture_output=True, text=True)


def test_phase_b_e2e_mock_two_hosts(mock_factory: Any, tmp_path: Path) -> None:
    m = mock_factory(offers=[offer8(i) for i in range(2)], put_secret=True)
    rd = tmp_path / "run"
    proc = run_b(phase_b_cfg(tmp_path, m.base), rd)
    assert proc.returncode == 0, (
        proc.stdout + proc.stderr + (rd / "journal.jsonl").read_text()[-3000:]
    )
    st = m.state()
    assert st["put_count"] == 2 and all(r["deleted"] for r in st["instances"].values())
    ev = json.loads((tmp_path / "evidence.json").read_text())
    det = [r["result"] for r in ev["runs"] if r["arm"] == "det"]
    assert len(det) == 2 and all(d["n_gpus"] == 8 and d["ranks_agree"] for d in det)
    for k in (
        "leaves",
        "leaves_root",
        "delta_hash",
        "final_theta_hash",
        "topk_index_sha256",
        "topk_value_sha256",
    ):
        assert det[0][k] == det[1][k], k
    assert ev["mismatches"] == [] and ev["verdict"] == "CENSORED" and ev["off_hardware_gate"]
    assert set(ev["overhead"]["per_host_ratio"]) == {"h0", "h1"}
    assert all(ev["network"]["nccl"][r]["world"] == 8 for r in ("h0", "h1"))
    assert ev["network"]["iperf"] == "CENSORED" and ev["network"]["iperf_endpoint"] is None
    assert ev["inventory"] == {"parsed": True, "total_rows": 0, "owned_instances": 0}
    rows = journal(rd)
    first_delete = min(i for i, r in enumerate(rows) if r["kind"] == "delete_ack")
    assert max(i for i, r in enumerate(rows) if r["kind"] == "rescued") < first_delete
    vm = [r["vmono"] for r in rows if r["kind"] == "provider_call" and r["method"] == "PUT"]
    assert all(b - a >= 5.0 for a, b in zip(vm, vm[1:], strict=False))
    secret = f"{st['next_id'] - 1:064x}"
    blob = b"".join(f.read_bytes() for f in rd.rglob("*") if f.is_file())
    assert secret.encode() not in blob
    puts = [
        base64.b64decode(json.loads(f.read_text())["raw_base64"]).decode()
        for f in (rd / "raw").glob("*-put-*.json")
    ]
    assert len(puts) == 2 and all('"instance_api_key": "***"' in t for t in puts)
    ok = checker(tmp_path / "evidence.json")
    assert ok.returncode == 0, ok.stdout
    leaked = next((rd / "raw").glob("*-put-h0.json"))
    rec = json.loads(leaked.read_text())
    rec["raw_base64"] = base64.b64encode(f'{{"instance_api_key": "{secret}"}}'.encode()).decode()
    leaked.write_text(json.dumps(rec))
    bad = checker(tmp_path / "evidence.json")
    assert bad.returncode == 1 and "unmasked instance secret" in bad.stdout


def test_phase_b_remote_failure_rescues_and_deletes_now(mock_factory: Any, tmp_path: Path) -> None:
    m = mock_factory(offers=[offer8(i) for i in range(2)])
    rd = tmp_path / "run"
    proc = run_b(phase_b_cfg(tmp_path, m.base, profile="no_such_profile"), rd)
    assert proc.returncode == 3, proc.stdout + proc.stderr
    rows = journal(rd)
    started = [(r["role"], r["name"]) for r in rows if r["kind"] == "job_started"]
    assert sorted(started) == [("h0", "probe"), ("h1", "probe")]
    assert all(r["exit"] not in (None, "0") for r in rows if r["kind"] == "job_done")
    closed = [r for r in rows if r["kind"] == "transaction_closed"]
    assert closed and closed[0]["actor"] == "launcher"
    assert closed[0]["outcome"].startswith("failed:remote_exit:")
    assert not [r for r in rows if r["kind"] == "supervisor_deadline_reached"]
    first_delete = min(i for i, r in enumerate(rows) if r["kind"] == "delete_ack")
    rescued = [i for i, r in enumerate(rows) if r["kind"] == "rescued"]
    assert len(rescued) == 2 and max(rescued) < first_delete
    assert all(r["deleted"] for r in m.state()["instances"].values())
    ev = json.loads((tmp_path / "evidence.json").read_text())
    assert ev["verdict"] == "CENSORED" and ev["inventory"]["owned_instances"] == 0
    assert checker(tmp_path / "evidence.json").returncode == 0


def test_phase_b_oom_log_watch_kills_fast(mock_factory: Any, tmp_path: Path) -> None:
    m = mock_factory(offers=[offer8(i) for i in range(2)])
    rd = tmp_path / "run"
    cfg = phase_b_cfg(tmp_path, m.base, remote_env={"HT_PHASE_B_FAULT": "oom_hang"})
    t0 = time.monotonic()
    proc = run_b(cfg, rd)
    assert proc.returncode == 3, proc.stdout + proc.stderr
    assert time.monotonic() - t0 < 300  # the injected hang lasts 600 s
    rows = journal(rd)
    done = [r for r in rows if r["kind"] == "job_done"]
    assert {r["role"] for r in done} == {"h0", "h1"} and all(r["exit"] == "97" for r in done)
    assert all(r["name"] == "probe" for r in done)
    assert "WATCH_KILL" in (rd / "rescue/h0/probe.log").read_text()
    assert len([r for r in rows if r["kind"] == "absence_confirmed"]) == 2


def test_phase_b_boot_timeout_rescues_and_deletes_pair(mock_factory: Any, tmp_path: Path) -> None:
    m = mock_factory(offers=[offer8(i) for i in range(2)], never_running=True)
    rd = tmp_path / "run"
    proc = run_b(phase_b_cfg(tmp_path, m.base, boot_timeout_seconds=900), rd)
    assert proc.returncode == 3, proc.stdout + proc.stderr
    rows = journal(rd)
    closed = [r for r in rows if r["kind"] == "transaction_closed"]
    assert closed and closed[0]["outcome"] == "failed:boot_timeout:h0"
    assert closed[0]["actor"] == "launcher" and not [r for r in rows if r["kind"] == "job_started"]
    assert not [r for r in rows if r["kind"] == "supervisor_deadline_reached"]
    assert len([r for r in rows if r["kind"] == "absence_confirmed"]) == 2
    assert all(r["deleted"] for r in m.state()["instances"].values())


def test_phase_b_credit_short_censored_without_create(mock_factory: Any, tmp_path: Path) -> None:
    m = mock_factory(offers=[offer8(i) for i in range(2)], credit=9.0)
    rd = tmp_path / "run"
    proc = run_b(phase_b_cfg(tmp_path, m.base), rd)
    assert proc.returncode == 2 and "CENSORED" in proc.stdout, proc.stdout + proc.stderr
    assert m.state()["put_count"] == 0
    assert not [r for r in journal(rd) if r["kind"] in ("create_intent", "receipt")]
    ev = json.loads((tmp_path / "evidence.json").read_text())
    assert ev["verdict"] == "CENSORED" and ev["admission"]["plan"]["current_credit_usd"] == "9.0"
    assert ev["admission"]["plan"]["checks"]["credit_covers"] is False
    assert checker(tmp_path / "evidence.json").returncode == 0


def test_verdict_overhead_and_iperf_parsing() -> None:
    o = _load("orchestrate")
    env = {"device": "cuda", "sm_counts": [170], "device_count": 8}
    res = {k: "aa" * 32 for k in o.SAME_KEYS if k != "leaves"} | {
        "leaves": ["11", "22"],
        "env": env,
        "n_gpus": 8,
        "ranks_agree": True,
        "tok_per_s": 100.0,
    }
    v = o.compute_verdict({"h0": res, "h1": dict(res)}, {"h0": 1, "h1": 2}, 8)
    assert v["verdict"] == "PASS"
    flip = dict(res, topk_value_sha256="bb" * 32)
    v = o.compute_verdict({"h0": res, "h1": flip}, {"h0": 1, "h1": 2}, 8)
    assert v["verdict"] == "FAIL" and v["mismatches"][0]["fields"] == ["topk_value_sha256"]
    assert (
        o.compute_verdict({"h0": res, "h1": dict(res)}, {"h0": 1, "h1": 1}, 8)["verdict"]
        == "CENSORED"
    )
    assert (
        o.compute_verdict({"h0": res, "h1": None}, {"h0": 1, "h1": 2}, 8)["verdict"] == "CENSORED"
    )
    fast = dict(res, tok_per_s=161.0)
    assert o.overhead({"h0": res}, {"h0": dict(res, tok_per_s=150.0)})["verdict"] == "PASS"
    assert o.overhead({"h0": res}, {"h0": fast})["verdict"] == "REPLAN"
    assert o.overhead({"h0": res}, {"h0": None})["verdict"] == "CENSORED"
    show = {
        "public_ipaddr": "1.2.3.4",
        "ports": {"5201/tcp": [{"HostIp": "0.0.0.0", "HostPort": "40123"}]},
    }
    assert o.iperf_endpoint(show) == ("1.2.3.4", 40123)
    assert o.iperf_endpoint({"public_ipaddr": "1.2.3.4", "ports": None}) is None
    assert (
        o.iperf_endpoint(
            {"public_ipaddr": "1.2.3.4", "ports": {"5201/tcp": [{"HostPort": "x;rm"}]}}
        )
        is None
    )


def test_select_hosts_pick_rules() -> None:
    s = _load("select_hosts")
    from decimal import Decimal

    cfg = {"num_gpus": 8, "disk_gb": 60, "egress_gb": 2}
    base = offer8(0) | {"reliability2": 0.99, "cuda_max_good": 13.0, "disk_space": 100}
    offers = [
        base,
        base | {"id": 2, "machine_id": 2, "host_id": 2, "dph_total": 4.5},  # same driver as #1
        base | {"id": 3, "machine_id": 3, "host_id": 90, "dph_total": 4.5},  # same host_id
        base | {"id": 4, "machine_id": 4, "host_id": 4, "reliability2": 0.9, "driver_version": "x"},
        base | {"id": 5, "machine_id": 5, "host_id": 5, "disk_space": 16, "driver_version": "y"},
        base
        | {"id": 6, "machine_id": 6, "host_id": 6, "dph_total": 6.0, "driver_version": "595.84"},
    ]
    got = s.pick(offers, cfg, 2, Decimal(2))
    assert [o["id"] for o in got] == [800, 6]
    assert s.pick(offers[:3], cfg, 2, Decimal(2)) == [base]
    strict = cfg | {"min_reliability": 0.99, "min_inet_down_mbps": 1000}
    fast = [o | {"inet_down": 1500} for o in offers]
    fast[0] = fast[0] | {"inet_down": 900}
    assert [o["id"] for o in s.pick(fast, strict, 2, Decimal(2))] == [2, 6]
    assert s.pick(offers, strict, 2, Decimal(2)) == []


@pytest.mark.parametrize("arm", ["det"])
def test_fake_device_emulated_island_matches_gloo(tmp_path: Path, arm: str) -> None:
    """8 emulated ranks on the CUDA-free proxy device == 8-rank gloo run of the same worker."""
    rb = _load("run_phase_b")
    spec = json.loads((B / "phase_b.json").read_text())["profiles"]["tiny"]
    shard = TREE / "data/train/shard-00000.u32"
    sha = hashlib.sha256(shard.read_bytes()).hexdigest()
    get, n_rows = rb.shard_reader(shard, sha, spec["model"]["seq_len"])
    out = tmp_path / "gloo.json"
    proc = subprocess.run(
        [
            sys.executable,
            "-m",
            "torch.distributed.run",
            "--standalone",
            "--nproc-per-node",
            "8",
            str(B / "run_phase_b.py"),
            "--arm",
            arm,
            "--profile",
            "tiny",
            "--device",
            "cpu",
            "--shard",
            str(shard),
            "--shard-sha256",
            sha,
            "--out",
            str(out),
        ],
        capture_output=True,
        text=True,
        timeout=900,
        env={**os.environ, "OMP_NUM_THREADS": "1"},
    )
    assert proc.returncode == 0, proc.stderr[-3000:]
    gloo = json.loads(out.read_text())

    child = subprocess.run(
        [sys.executable, str(Path(__file__).with_name("_phase_b_proxy.py")), str(shard), sha],
        capture_output=True,
        text=True,
        timeout=900,
        env={**os.environ, "OMP_NUM_THREADS": "1"},
    )
    assert child.returncode == 0, child.stderr[-3000:]
    results = json.loads(child.stdout)
    for r in results:
        for k in (
            "leaves",
            "leaves_root",
            "delta_hash",
            "final_theta_hash",
            "topk_index_sha256",
            "topk_value_sha256",
        ):
            assert r[k] == gloo[k], k
    assert (
        gloo["ranks_agree"] and gloo["n_gpus"] == 8 and gloo["ops"] == ["all_gather", "all_to_all"]
    )
