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
