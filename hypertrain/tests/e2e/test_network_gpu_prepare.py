"""Metadata-only frozen preparation; no trials, training, provider or CUDA."""

import hashlib
import io
import json
import os
import shutil
import sqlite3
import stat
import subprocess
import sys
import tarfile
import time
import urllib.parse
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(scope="module")
def freeze_path(tmp_path_factory):
    """Public source metadata only; preparation creates its own signed CPU authority."""
    from hypertrain.gpu_ops import network_qualification

    snapshot = tmp_path_factory.mktemp("public-preparation-source")
    tree = snapshot / "tree"
    required = set(network_qualification.sources(ROOT)) | {
        "scripts/network_gpu_operation.py",
        "scripts/network_gpu_seed.py",
        "scripts/network_service_proof.py",
        "scripts/network_authority_snapshot.py",
    }
    for relative in sorted(required):
        source, target = ROOT / relative, tree / relative
        assert source.is_file() and not source.is_symlink()
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    profile_path = tree / "experiments/gpu_network_v2/profile.json"
    profile = json.loads(profile_path.read_bytes())
    profile["runtime"].update(image_digest="sha256:" + "ab" * 32, driver_allowlist=["595.84"])
    assert profile["runtime"]["qualified"] is False
    profile_path.write_text(json.dumps(profile, sort_keys=True))

    def sha(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def artifact(path):
        return {"path": str(path), "bytes": path.stat().st_size, "sha256": sha(path)}

    canonical = network_qualification.sources(tree)
    operation = {
        **canonical,
        "scripts/network_gpu_operation.py": sha(tree / "scripts/network_gpu_operation.py"),
    }
    staged = {relative: sha(tree / relative) for relative in sorted(required)}
    maps = {
        "qualification-sources.json": canonical,
        "operation-sources.json": operation,
        "sources.json": staged,
        "files.json": {
            relative: {
                "origin": str(tree / relative),
                "size": (tree / relative).stat().st_size,
                "sha256": digest,
            }
            for relative, digest in staged.items()
        },
    }
    for name, values in maps.items():
        (snapshot / name).write_text(json.dumps(values, sort_keys=True))
    qualification = snapshot / "qualification-source.tar"
    qualification.write_bytes(network_qualification.bundle(tree))
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        for relative in sorted(staged):
            data = (tree / relative).read_bytes()
            member = tarfile.TarInfo(relative)
            member.size, member.mode, member.mtime = len(data), 0o600, 0
            archive.addfile(member, io.BytesIO(data))
    effective = snapshot / "effective-source.tar"
    effective.write_bytes(buffer.getvalue())
    freeze = {
        "snapshot": str(snapshot),
        "archive": artifact(effective),
        "qualification_archive": artifact(qualification),
        "maps": {name: artifact(snapshot / name) for name in maps},
        "approved_pins": {
            name: staged[name]
            for name in (
                "scripts/network_service_proof.py",
                "scripts/network_gpu_operation.py",
                "experiments/gpu_network_v2/orchestrate.py",
            )
        },
        "service_source_observed": {},
        "qualification_parser": {},
        "image_candidate": {"ref": "metadata-only@" + profile["runtime"]["image_digest"]},
        "paid_GO": False,
        "launchable_private_config": False,
    }
    path = snapshot / "freeze.json"
    path.write_text(json.dumps(freeze, sort_keys=True))
    return path


def test_prepare_executable_frozen_artifacts_and_refusal(tmp_path, freeze_path):
    output = tmp_path / "prepared"
    command = [
        sys.executable,
        str(ROOT / "scripts/network_gpu_prepare.py"),
        "--freeze",
        str(freeze_path),
        "--output",
        str(output),
        "--cpu-genesis-unix",
        "1700000000",
        "--candidate-driver",
        "580.95.05",
        "--candidate-driver",
        "580.159.03",
    ]
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        timeout=60,
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
    )
    assert result.returncode == 0, result.stderr
    prepared = json.loads(result.stdout)
    assert prepared["status"] == "LOCAL_PREPARED_NOT_LAUNCHABLE"
    assert prepared["candidate_input_scope"]["drivers"] == ["580.95.05", "580.159.03"]
    manifest = json.loads((output / "public/seed/manifest.json").read_bytes())
    assert manifest["training"]["reference_spec"]["driver_allowlist"] == ["580.95.05", "580.159.03"]
    original = json.loads(freeze_path.read_bytes())
    source_map = json.loads(Path(original["maps"]["sources.json"]["path"]).read_bytes())
    for relative, expected in source_map.items():
        path = output / "source-freeze/tree" / relative
        if relative != "experiments/gpu_network_v2/profile.json":
            assert hashlib.sha256(path.read_bytes()).hexdigest() == expected
    assert (
        prepared["count"]["required_total"] == 126 and prepared["count"]["required_per_host"] == 63
    )
    assert set(prepared["remaining"].values()) == {"NOT_PRESENT"}
    assert "missing_or_unqualified_exact_profile_runtime" in prepared["launch_refusal"]
    assert prepared["controller_refusal"] == "reviewed_layout_clock_or_config"
    assert prepared["kernels_executed"] == prepared["provider_calls"] == 0
    assert list((output / "production-service").iterdir()) == []
    for relative, digest in json.loads((output / "public/index.json").read_bytes()).items():
        assert hashlib.sha256((output / "public" / relative).read_bytes()).hexdigest() == digest
        assert "private" not in relative and not relative.endswith((".key", ".seed", ".hex"))
    for path in (output / "private").rglob("*"):
        if path.is_file():
            assert stat.S_IMODE(path.stat().st_mode) == 0o600
    with sqlite3.connect(output / "cpu-service/challenge.db") as db:
        assert db.execute("SELECT COUNT(*) FROM runs").fetchone()[0] == 1
        assert db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM escrow_origins").fetchone()[0] == 4
        assert db.execute("SELECT bls FROM beacon").fetchone()[0] == 0
    assert len(json.loads((output / "public/membership.json").read_bytes())) == 4
    probe = """
import ast,importlib.util,json,sys
from pathlib import Path
from types import SimpleNamespace
spec=importlib.util.spec_from_file_location('prepare',sys.argv[1])
prepare=importlib.util.module_from_spec(spec);spec.loader.exec_module(prepare)
_,_,maps,seed,driver,controller=prepare.load_frozen(Path(sys.argv[2]))
output=Path(sys.argv[3])
job=seed.validate(output/'public/seed',sys.argv[4])
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.jcs import canonicalize
from hypertrain.gpu_ops.launcher import Reject
owner=Keypair((output/'private/roles/owner.seed').read_bytes())
hot=Keypair((output/'private/roles/hot-0.seed').read_bytes())
subjects=[('source',maps['operation-sources.json']),
 ('index',json.loads((output/'public/index.json').read_bytes())),
 ('freeze',json.loads(Path(sys.argv[2]).read_bytes()))]
for name,subject in subjects:
 raw=json.loads((output/'public'/f'{name}-receipt.json').read_bytes())
 assert envelope_v2.verify_envelope(raw) and raw['signer']==owner.ss58 and raw['run_id']==job.run_id
 assert raw['body']['commit_hash']==driver.sha256_hex(canonicalize(subject))
source=Path(sys.argv[2]).parent/'tree/src/hypertrain/gpu_ops/network_qualification.py'
node=next(n for n in ast.parse(source.read_text()).body
 if isinstance(n,ast.FunctionDef) and n.name=='inspect_environment')
guard=next(n for n in node.body
 if isinstance(n,ast.If) and 'qualification_driver_mismatch' in ast.unparse(n))
code=compile(ast.fix_missing_locations(ast.Module(body=[guard],type_ignores=[])),str(source),'exec')
cfg=SimpleNamespace(driver_allowlist=['580.95.05','580.159.03'])
exec(code,{'cfg':cfg,'drivers':['580.95.05','580.159.03'],'Reject':Reject})
for unexpected in ('595.71','610','595.71.05','610.57.04'):
 try:
  exec(code,{'cfg':cfg,'drivers':['580.95.05',unexpected],'Reject':Reject})
 except Reject as error:
  assert str(error)=='qualification_driver_mismatch'
 else:
  raise AssertionError('unexpected driver accepted')
# Use the canonical owned-host consumer against an actually absent receipt;
# a prepared source bundle is not an observed allocation or GPU attestation.
runtime=controller.NetworkRuntime(
 SimpleNamespace(receipt=lambda role:None,cleanup_started=lambda:False,
                 j=controller.Journal(output/'absent-owned-role-journal')),{}
)
factory=prepare.service_factory(driver,runtime,owner,job.run_id,maps['operation-sources.json'],{hot.ss58:{'role':'h0'}},lambda:1)
try:
 factory('reference',job,output/'future-operation',{'run_id':job.run_id,'hotkey':hot.ss58})
except Reject as error:
 assert str(error)=='owned-role actual owned role observation absent'
else:
 raise AssertionError('dynamic context invented an owned host')
print('DYNAMIC_CONTEXT_REFUSES_ABSENT_OWNED_ROLE')
"""
    dynamic = subprocess.run(
        [
            sys.executable,
            "-c",
            probe,
            str(ROOT / "scripts/network_gpu_prepare.py"),
            str(output / "source-freeze/freeze.json"),
            str(output),
            prepared["seed_index_sha256"],
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert dynamic.returncode == 0, dynamic.stderr
    assert "DYNAMIC_CONTEXT_REFUSES_ABSENT_OWNED_ROLE" in dynamic.stdout
    again = subprocess.run(command, capture_output=True, text=True, timeout=60)
    assert again.returncode != 0 and "new output" in again.stderr


def test_freeze_changed_map_rejects_before_private_writes(tmp_path, freeze_path):
    freeze = json.loads(freeze_path.read_bytes())
    freeze["maps"]["sources.json"]["sha256"] = "0" * 64
    altered = tmp_path / "freeze.json"
    altered.write_text(json.dumps(freeze))
    output = tmp_path / "prepared"
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts/network_gpu_prepare.py"),
            "--freeze",
            str(altered),
            "--output",
            str(output),
            "--cpu-genesis-unix",
            "1700000000",
        ],
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert result.returncode != 0 and "frozen archive/map changed" in result.stderr
    assert not output.exists()


def test_parser_requires_explicit_cpu_clock(capsys):
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "prepare_parser", ROOT / "scripts/network_gpu_prepare.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    old = sys.argv
    sys.argv = ["network_gpu_prepare.py", "--freeze", "unused-freeze.json", "--output", "unused"]
    try:
        with pytest.raises(SystemExit) as error:
            module.main()
        assert error.value.code == 2
    finally:
        sys.argv = old


@pytest.mark.parametrize("unexpected", ["595.71", "610", "595.71.05", "610.57.04"])
def test_unapproved_candidate_rejects_before_artifacts(tmp_path, freeze_path, unexpected):
    output = tmp_path / "prepared"
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts/network_gpu_prepare.py"),
            "--freeze",
            str(freeze_path),
            "--output",
            str(output),
            "--cpu-genesis-unix",
            "1700000000",
            "--candidate-driver",
            "580.95.05",
            "--candidate-driver",
            unexpected,
        ],
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert result.returncode != 0 and "exact approved ordered pair" in result.stderr
    assert not output.exists()


def test_prepare_qualification_base_reuses_v2_assets_without_synthetic_roles(tmp_path, freeze_path):
    """prune_rescued qualification base: V2 job/profile by reference, no seeds/joins/genesis."""
    v2 = tmp_path / "v2"
    (v2 / "out").mkdir(parents=True)
    (v2 / "job").mkdir()
    tree = Path(json.loads(freeze_path.read_bytes())["snapshot"]) / "tree"
    profile = json.loads((tree / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["artifacts"] |= {"remote_retention": "prune_rescued", "disk_bytes": 429495787520}
    profile["admission"] |= {"ceiling_usd": "500", "min_disk_gib": 399}
    platform = tmp_path / "amd64.manifest.json"
    platform.write_text(
        json.dumps({"schemaVersion": 2, "config": {"size": 1}, "layers": [{"size": 2}]})
    )
    index = tmp_path / "index.manifest.json"
    index.write_text(
        json.dumps(
            {
                "schemaVersion": 2,
                "manifests": [
                    {
                        "digest": "sha256:" + hashlib.sha256(platform.read_bytes()).hexdigest(),
                        "platform": {"architecture": "amd64", "os": "linux"},
                    }
                ],
            }
        )
    )
    profile["runtime"]["image_digest"] = "sha256:" + hashlib.sha256(index.read_bytes()).hexdigest()
    (v2 / "out/profile.json").write_text(json.dumps(profile))
    (v2 / "out/manifest-envelope.json").write_text("{}")
    objects = {"start_state": "start_state", "ef_in": "ef_in", "v0": "v0"}
    for rel in objects.values():
        (v2 / "job" / rel).write_bytes(rel.encode())
    (v2 / "job/job.json").write_text(json.dumps({"run_id": "ab" * 32, "object_paths": objects}))
    ownership = tmp_path / "ownership-reconciled.json"
    ownership.write_text(
        json.dumps(
            {
                "api_lookup": {
                    "hostA": {"instance_id": 55618302, "machine_id": 147322},
                    "hostB": {"instance_id": 55618304, "machine_id": 138496},
                }
            }
        )
    )
    output = tmp_path / "base"
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts/network_gpu_prepare.py"),
            "--freeze",
            str(freeze_path),
            "--output",
            str(output),
            "--cpu-genesis-unix",
            "1",
            "--qualification-base",
            str(v2 / "out"),
            str(v2 / "job"),
            str(ownership),
            str(index),
            str(platform),
        ],
        capture_output=True,
        text=True,
        timeout=120,
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
    )
    assert result.returncode == 0, result.stderr
    prepared = json.loads(result.stdout)
    assert prepared["status"] == "QUALIFICATION_BASE_NOT_LAUNCHABLE"
    assert prepared["kernels_executed"] == prepared["provider_calls"] == 0
    record = json.loads((output / "private/review-base.json").read_bytes())["record"]
    bundle = Path(record["files"]["bundle"]["path"])
    assert record["staging_bytes"] == bundle.stat().st_size + sum(
        p.stat().st_size for p in (v2 / "job").iterdir()
    )
    assert record["files"]["ownership"]["path"] == str(ownership)
    assert record["files"]["registry_platform"]["path"] == str(platform)
    assert record["role_inputs"]["objects"] == {k: "object_" + k for k in objects}
    cfg = json.loads((output / "private/lifecycle-config.json").read_bytes())
    assert cfg["hosts"] == [
        {"role": "h0", "machine_id": 147322},
        {"role": "h1", "machine_id": 138496},
    ]
    assert cfg["disk_gb"] == 399
    # Fresh admit: measured offer rows select the hosts; duplicates or wrong shape reject.
    limits_gpus = profile["admission"]["gpus_per_host"]
    driver = "595.71.05"
    pinned = v2 / "out-fresh"
    pinned.mkdir()
    profile["runtime"]["driver_allowlist"] = [driver]
    (pinned / "profile.json").write_text(json.dumps(profile))
    (pinned / "manifest-envelope.json").write_text("{}")

    def offer(i, machine, **change):
        row = {"id": i, "machine_id": machine, "num_gpus": limits_gpus}
        row |= {"gpu_name": "RTX 5090", "driver_version": driver}
        return {**row, "inet_up": 800, "inet_down": 800.0, **change}

    slow = offer(2, 140854, inet_up=799.99)
    no_down = {k: v for k, v in offer(2, 140854).items() if k != "inet_down"}

    for name, rows, ok in (
        ("fresh", [offer(1, 139775), offer(2, 140854)], True),
        ("dup", [offer(1, 139775), offer(2, 139775)], False),
        ("shape", [offer(1, 139775), offer(2, 140854, num_gpus=1)], False),
        ("driver", [offer(1, 139775), offer(2, 140854, driver_version="0")], False),
        ("one", [offer(1, 139775)], False),
        ("not-dict", [offer(1, 139775), [140854]], False),
        ("bool-id", [offer(1, 139775), offer(2, True)], False),
        ("slow-up", [offer(1, 139775), slow], False),
        ("missing-down", [offer(1, 139775), no_down], False),
        ("nan-down", [offer(1, 139775), offer(2, 140854, inet_down=float("nan"))], False),
        ("inf-up", [offer(1, 139775), offer(2, 140854, inet_up=float("inf"))], False),
        ("bool-up", [offer(1, 139775), offer(2, 140854, inet_up=True)], False),
        ("negative-down", [offer(1, 139775), offer(2, 140854, inet_down=-900)], False),
        ("string-up", [offer(1, 139775), offer(2, 140854, inet_up="900")], False),
    ):
        offers = tmp_path / (name + "-offers.json")
        offers.write_text(json.dumps(rows))
        argv = [sys.executable, str(ROOT / "scripts/network_gpu_prepare.py")]
        argv += ["--freeze", str(freeze_path), "--output", str(tmp_path / ("base-" + name))]
        argv += ["--cpu-genesis-unix", "1", "--qualification-base"]
        argv += [str(pinned), str(v2 / "job"), str(offers), str(index), str(platform)]
        run = subprocess.run(argv, capture_output=True, text=True, timeout=120)
        assert (run.returncode == 0) is ok, run.stderr
        if ok:
            fresh = tmp_path / "base-fresh/private/lifecycle-config.json"
            assert json.loads(fresh.read_bytes())["hosts"] == [
                {"role": "h0", "machine_id": 139775},
                {"role": "h1", "machine_id": 140854},
            ]
        else:
            assert "two distinct exact-shape" in run.stderr
    assert {"base_url", "boot_timeout_seconds", "keyscan_attempts", "create_margin_seconds"} <= set(
        cfg
    )
    assert "key_file" not in cfg and "cleanup_deadline_unix" not in cfg  # added at run time
    # Qualification-only: no synthetic role seeds, joins, genesis, relay, TLS or service state.
    assert sorted(p.name for p in (output / "private").iterdir()) == [
        "lifecycle-config.json",
        "review-base.json",
    ]
    assert sorted(p.name for p in (output / "public").iterdir()) == [
        "lifecycle-hashes.json",
        "qualification-source.tar",
        "qualification-sources.json",
    ]
    overlay = json.loads((output / "public/qualification-sources.json").read_bytes())
    assert (
        overlay["experiments/gpu_network_v2/profile.json"]
        == hashlib.sha256((v2 / "out/profile.json").read_bytes()).hexdigest()
    )
    index.write_text(json.dumps({"schemaVersion": 2, "manifests": []}))
    refused = subprocess.run(
        [*result.args[:5], str(tmp_path / "refused"), *result.args[6:]],
        capture_output=True,
        text=True,
        timeout=120,
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
    )
    assert (
        refused.returncode != 0
        and "registry index must be the pinned image digest" in refused.stderr
    )


def test_runtime_financial_offers_keep_every_budget_price_field(tmp_path, monkeypatch):
    """GET-only financial step: offers keep the provider price fields budget.evaluate prices."""
    import importlib.util

    from hypertrain.gpu_ops import provider as provider_module
    from hypertrain.gpu_ops.provider import Response

    spec = importlib.util.spec_from_file_location(
        "prepare_runtime", ROOT / "scripts/prepare_runtime.py"
    )
    runtime = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runtime)
    profile = json.loads((ROOT / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile_path = tmp_path / "profile.json"
    profile_path.write_text(json.dumps(profile))
    config = tmp_path / "config.json"
    hosts = [{"role": "h0", "machine_id": 11}, {"role": "h1", "machine_id": 12}]
    config.write_text(json.dumps({"hosts": hosts, "network_profile_file": str(profile_path)}))
    key = tmp_path / "key"
    key.write_text("a" * 32)
    now = int(time.time())
    row = {
        "num_gpus": 2,
        "gpu_name": "x",
        "dph_total": 1.5,
        "storage_cost": 0.4,
        "inet_up_cost": 0.003,
        "inet_down_cost": 0.002,
        "disk_space": 400.0,
        "cpu_cores_effective": 32.0,
        "cpu_ram": 65536,
        "driver_version": "1",
        "actual_status": "running",
        "start_date": now - 3600,
    }
    bodies = {
        "/api/v0/users/current/": {"id": 7, "credit": 90.0},
        "/api/v1/instances/": {
            "success": True,
            "instances": [
                {**row, "id": 1, "machine_id": 11},
                {**row, "id": 2, "machine_id": 12},
                {**row, "id": 3, "machine_id": 99, "actual_status": "exited"},
                {
                    **row,
                    "id": 4,
                    "machine_id": 98,
                    "actual_status": "exited",
                    "cur_state": "stopped",
                    "storage_total_cost": 0.096,
                },
            ],
        },
        "/api/v0/volumes/": {"volumes": []},
        "/api/v0/charges/": {"results": [{"source": "instance-1", "amount": 2.0}]},
    }

    def call(self, method, path, tag, **kwargs):
        assert method == "GET"
        return Response(200, bodies[path.split("?")[0]], None, "", None)

    monkeypatch.setattr(provider_module.Provider, "call", call)
    result = runtime.financial(ROOT, config, key, "test", tmp_path / "out")
    offers = json.loads((tmp_path / "out/offers.json").read_bytes())
    assert [o["id"] for o in offers] == [1, 2]
    for offer in offers:
        for field in ("dph_total", "storage_cost", "inet_up_cost", "inet_down_cost"):
            assert offer[field] == row[field]
    snapshot = json.loads((tmp_path / "out/financial.json").read_bytes())
    assert snapshot["account_id"] == 7 and snapshot["available_credit_usd"] == "90.0"
    horizon = sys.modules["runtime_controller"].lifecycle_timing(profile)["horizon"]
    # Foreign only: unknown-state exited bills full dph; a stopped one bills storage only.
    assert snapshot["active_liabilities_usd"] == f"{(1.5 + 0.096) * horizon / 3600:.6f}"
    assert result["must_seal_before_unix"] == result["verified_unix"] + 30

    # Fresh admit (no owned rows): the live ask create will use must carry up/down >= 800.
    bodies["/api/v1/instances/"] = {"success": True, "instances": []}
    for name, link, ok in (
        ("exact", {"inet_up": 800, "inet_down": 800.0}, True),
        ("slow", {"inet_up": 800, "inet_down": 799.99}, False),
        ("missing", {"inet_up": 800}, False),
        ("bool", {"inet_up": True, "inet_down": 900}, False),
    ):

        def fresh(self, method, path, tag, link=link, **kwargs):
            assert method == "GET"
            if path.startswith("/api/v0/bundles/"):
                machine = json.loads(urllib.parse.parse_qs(path.split("?")[1])["q"][0])
                mid = machine["machine_id"]["eq"]
                offer = {**row, "id": 100 + mid, "machine_id": mid, **link}
                return Response(200, {"offers": [offer]}, None, "", None)
            return Response(200, bodies[path.split("?")[0]], None, "", None)

        monkeypatch.setattr(provider_module.Provider, "call", fresh)
        out = tmp_path / ("fresh-" + name)
        if ok:
            runtime.financial(ROOT, config, key, "test", out)
            fresh_offers = json.loads((out / "offers.json").read_bytes())
            assert [o["id"] for o in fresh_offers] == [111, 112]
        else:
            with pytest.raises(SystemExit, match="below 800"):
                runtime.financial(ROOT, config, key, "test", out)
            assert not (out / "offers.json").exists()  # rejected before any plan input
