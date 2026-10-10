"""Lightweight boundary/custody tests; no CUDA or training qualification."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest
from pydantic import ValidationError

from hypertrain.gpu_ops import network_qualification as driver
from hypertrain.gpu_ops.journal import fsha
from hypertrain.gpu_ops.launcher import Reject
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages_v2 import IslandJobV1, RunManifestV2

TREE = Path(__file__).parents[2]


def results(tmp_path: Path) -> list[Path]:
    """Two synthetic CUDA-labelled JSON sets exercise parsing, never execute GPUs."""
    paths = []
    for i in range(2):
        directory = tmp_path / f"host-{i}"
        hashes = []
        for index in range(2):
            root = directory / f"round-{index}/published"
            root.mkdir(parents=True)
            files = {}
            for rank in range(2):
                for rel in [
                    "state.safetensors",
                    "ef.safetensors",
                    "delta.bin",
                    "leaves.json",
                    "trace.json",
                ] + [f"checkpoints/{t}.safetensors" for t in range(0, 31, 5)]:
                    path = root / f"rank-{rank}" / rel
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(bytes([index]))
                    files[str(path.relative_to(root))] = fsha(path)
            hashes.append(files)
        result = driver.Result(
            status="CUDA_ROUNDS_CAPTURED_NOT_WORKLOAD_PASS",
            config_sha256="11" * 32,
            profile_sha256="22" * 32,
            sources={"src/x.py": "33" * 32},
            environment=driver.Environment(
                backend="cuda",
                torch="2.14.0+cu130",
                cuda="13.0",
                python="/opt/hypertrain/venv/bin/python",
                torch_path="/venv/torch/__init__.py",
                drivers=["595.84", "595.84"],
                sm_counts=[170, 170],
                image_digest="sha256:" + "44" * 32,
            ),
            instance_id=i + 1,
            machine_id=i + 10,
            role=f"h{i}",
            rounds=hashes,
            elapsed_seconds=1,
            workload_allowed=False,
        )
        path = directory / "result.json"
        path.write_text(result.model_dump_json())
        paths.append(path)
    return paths


def test_bitwise_comparison_preserves_workload_gate(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    # When
    result = driver.compare(paths)
    # Then
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}


def test_tampered_artifact_rejects_claimed_matching_hashes(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    (paths[1].parent / "round-1/published/rank-0/state.safetensors").write_bytes(b"tampered")
    # When / Then
    with pytest.raises(Reject, match="custody_changed"):
        driver.compare(paths)


def test_same_machine_rejects_two_host_claim(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    data = json.loads(paths[1].read_bytes())
    data["machine_id"] = 10
    paths[1].write_text(json.dumps(data))
    # When / Then
    with pytest.raises(Reject, match="binding_or_bitwise"):
        driver.compare(paths)


@pytest.mark.parametrize(
    "field,value", [("backend", "cpu"), ("torch", "2.14.0+cpu"), ("sm_counts", [170])]
)
def test_cpu_or_incomplete_environment_rejects(
    tmp_path: Path, field: str, value: str | list[int]
) -> None:
    # Given
    paths = results(tmp_path)
    data = json.loads(paths[0].read_bytes())["environment"]
    data[field] = value
    # When / Then
    with pytest.raises(ValidationError):
        driver.Environment.model_validate(data)


def test_source_manifest_detects_profile_change(tmp_path: Path) -> None:
    # Given
    for rel in [
        "src/hypertrain/x.py",
        "experiments/gpu_network_v2/run.py",
        "experiments/gpu_network_v2/orchestrate.py",
        "experiments/gpu_network_v2/profile.json",
        "scripts/network_gpu_qualification.py",
        "docker/Dockerfile.gpu",
        "pyproject.toml",
        "uv.lock",
    ]:
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"initial")
    initial = driver.sources(tmp_path)
    (tmp_path / "experiments/gpu_network_v2/profile.json").write_bytes(b"changed")
    # When
    changed = driver.sources(tmp_path)
    # Then
    assert initial != changed


def test_missing_runtime_proof_rejects_before_launch(tmp_path: Path) -> None:
    # Given
    config = tmp_path / "config.json"
    config.write_text("{}")
    # When / Then
    with pytest.raises(ValidationError):
        driver.run(config)


def test_partial_round_coverage_rejects_even_identical_results(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    for path in paths:
        data = json.loads(path.read_bytes())
        del data["rounds"][0]["rank-1/checkpoints/30.safetensors"]
        path.write_text(json.dumps(data))
    # When / Then
    with pytest.raises(Reject, match="coverage_missing"):
        driver.compare(paths)


def test_exact_profile_accepts_wire_defaults_without_training(tmp_path: Path) -> None:
    # Given
    profile_path = TREE / "experiments/gpu_network_v2/profile.json"
    profile = json.loads(profile_path.read_bytes())
    body = example_manifest().body()
    body["model"].update(profile["model"])
    body["inner"].update({k: v for k, v in profile["inner"].items() if k != "lr_schedule"})
    body["inner"]["lr_schedule"].update(profile["inner"]["lr_schedule"])
    body["outer"].update(profile["outer"])
    registry = tmp_path / "registry.json"
    registry.write_bytes(b'{"schemaVersion":2,"layers":[{"size":42}]}')
    image = "sha256:" + fsha(registry)
    body["reference_spec"].update(image_digest=image, driver_allowlist=["595.84"], sm_count=170)
    body["reference_spec"]["layout"].update(profile["layout"])
    wrapper = RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": body,
            "network": {
                **{
                    k: "11" * 32
                    for k in (
                        "admission_policy_hash",
                        "economics_policy_hash",
                        "aggregation_policy_hash",
                        "dispute_policy_hash",
                        "audit_policy_hash",
                        "relay_registry_hash",
                    )
                },
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )
    job = IslandJobV1(
        job_version=1,
        run_id=wrapper.run_id(),
        w=0,
        manifest=wrapper,
        sample_ids=list(range(60)),
        global_step0=0,
        start_state_sha256="22" * 32,
        ef_in_sha256="33" * 32,
        v0_sha256="44" * 32,
        object_paths={
            "start_state": "start",
            "ef_in": "ef",
            "v0": "v0",
            "samples": "samples",
            "sample_proofs": "proofs",
        },
        deadline=5000,
    )
    job_path = tmp_path / "job.json"
    job_path.write_text(job.model_dump_json())
    receipts = tmp_path / "receipts.json"
    receipts.write_text(
        json.dumps(
            {
                "instance_id": 1,
                "machine_id": 2,
                "role": "h0",
                "admitted_unix": 100,
                "image_digest": image,
                "known_hosts_sha256": "55" * 32,
                "lifecycle_contract_sha256": "66" * 32,
                "supervisor_pid": 123,
                "deadline_unix": 3700,
                "cuda_observed": True,
            }
        )
    )
    cfg = driver.Qualification(
        tree=TREE,
        profile=profile_path,
        profile_sha256=fsha(profile_path),
        sources=driver.sources(TREE),
        registry_manifest=registry,
        image_digest=image,
        driver_allowlist=["595.84"],
        seed_job=job_path,
        hotkey=body["coord_pubkey"],
        output=tmp_path / "out",
        admitted_unix=100,
        instance_id=1,
        machine_id=2,
        role="h0",
        lifecycle_contract_sha256="66" * 32,
        known_hosts_sha256="55" * 32,
        lifecycle_receipts=receipts,
        lifecycle_receipts_sha256=fsha(receipts),
    )
    # When
    accepted = driver.check_inputs(cfg)
    # Then
    assert accepted.run_id == wrapper.run_id()


def test_script_reexports_exact_packaged_engine() -> None:
    # Given: existing source-host tools still load the script by filename.
    spec = importlib.util.spec_from_file_location(
        "qualification_wrapper_test", TREE / "scripts/network_gpu_qualification.py"
    )
    assert spec and spec.loader
    wrapper = importlib.util.module_from_spec(spec)
    # When
    spec.loader.exec_module(wrapper)
    # Then: there is only one strict schema/function implementation.
    for name in (
        "Boundary",
        "Qualification",
        "Environment",
        "Result",
        "sources",
        "bundle",
        "check_inputs",
        "inspect_environment",
        "capture",
        "run",
        "compare",
        "main",
    ):
        assert getattr(wrapper, name) is getattr(driver, name)


def test_package_artifact_import_without_repository_cwd(tmp_path: Path) -> None:
    # Given: final packaging gate supplies an actual built wheel; source tests use package root.
    artifact = Path(os.environ.get("HT_QUALIFICATION_WHEEL", Path(driver.__file__).parents[2]))
    code = """
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from pydantic import ValidationError
from hypertrain.gpu_ops import network_qualification as engine
from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages_v2 import RunManifestV2
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import param_shapes
assert str(Path(engine.__file__)).startswith(sys.argv[1])
assert not Path("scripts/network_gpu_qualification.py").exists()
assert engine.Qualification.__module__ == "hypertrain.gpu_ops.network_qualification"
payload = {"tree":"tree", "profile":"profile", "profile_sha256":"11"*32,
"sources":{}, "registry_manifest":"registry", "image_digest":"sha256:"+"22"*32,
"driver_allowlist":["driver"], "seed_job":"job", "hotkey":"key", "output":"out",
"admitted_unix":1, "instance_id":1, "machine_id":2, "role":"h0",
"lifecycle_contract_sha256":"33"*32, "known_hosts_sha256":"44"*32,
"lifecycle_receipts":"receipt", "lifecycle_receipts_sha256":"55"*32}
assert engine.Qualification.model_validate(payload).tree == Path("tree")
for bad in ({**payload,"instance_id":True}, {**payload,"unexpected":1}):
    try:
        engine.Qualification.model_validate(bad)
    except ValidationError:
        pass
    else:
        raise AssertionError("strict schema weakened")
body = example_manifest().body()
body["model"]["param_count"] = sum(__import__("math").prod(shape) for shape in
    param_shapes(TrainConfig.from_manifest(example_manifest()).model).values())
manifest = RunManifestV2.model_validate({"manifest_version":2,"training":body,"network":{
**{n:"11"*32 for n in ("admission_policy_hash","economics_policy_hash",
"aggregation_policy_hash","dispute_policy_hash","audit_policy_hash","relay_registry_hash")},
"audit_mode":"anchored-full","full_anchor_version":1,
"capabilities":["island-replay","all-level-disputes","transport-receipts"]}})
store = ChallengeStore.__new__(ChallengeStore)
try:
    store._bootstrap_backend_v2(manifest, (Path("missing-config"),)*2,
                                (Path("missing-result"),)*2, b"")
except FileNotFoundError as error:
    assert error.filename == "missing-config", str(error)
else:
    raise AssertionError("expected real config read after packaged importer")
print("PACKAGED_QUALIFICATION_STRICT_SCHEMA_AND_STORE_IMPORT_PASS")
"""
    # When: isolated interpreter ignores checkout PYTHONPATH and runs outside repository.
    result = subprocess.run(
        [sys.executable, "-I", "-c", code, str(artifact.resolve())],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    # Then: real package/store seam imports without a sibling scripts tree.
    assert result.returncode == 0, result.stderr
    assert "PACKAGED_QUALIFICATION_STRICT_SCHEMA_AND_STORE_IMPORT_PASS" in result.stdout
