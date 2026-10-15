"""Lightweight boundary/custody tests; no CUDA or training qualification."""

from __future__ import annotations

import importlib.util
import json
import math
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
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import param_shapes

TREE = Path(__file__).parents[2]


def results(tmp_path: Path, ranks: int = 2, H: int = 30, J: int = 5) -> list[Path]:  # noqa: N803
    """Two synthetic CUDA-labelled JSON sets exercise parsing, never execute GPUs."""
    paths = []
    for i in range(2):
        directory = tmp_path / f"host-{i}"
        hashes = []
        for index in range(2):
            root = directory / f"round-{index}/published"
            files = {}
            for rel in driver.round_files(ranks, H, J):
                path = root / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(bytes([index]))
                files[rel] = fsha(path)
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
                drivers=["595.84"] * ranks,
                sm_counts=[170] * ranks,
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


def manifest(ranks: int = 2, H: int = 30, J: int = 5) -> RunManifestV2:  # noqa: N803
    """Caller-validated authority for compare(); results never supply their own shape."""
    body = example_manifest().body()
    shapes = param_shapes(TrainConfig.from_manifest(example_manifest()).model)
    body["model"]["param_count"] = sum(math.prod(s) for s in shapes.values())
    body["inner"].update(H=H, J=J)
    body["reference_spec"].update(image_digest="sha256:" + "44" * 32)
    body["reference_spec"]["layout"].update(n_gpus=ranks, dp_size=ranks, ep_size=1)
    network = {
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
    }
    return RunManifestV2.model_validate(
        {"manifest_version": 2, "training": body, "network": network}
    )


def test_bitwise_comparison_preserves_workload_gate(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    # When
    result = driver.compare(paths, manifest())
    # Then
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}


def test_tampered_artifact_rejects_claimed_matching_hashes(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    (paths[1].parent / "round-1/published/rank-0/state.safetensors").write_bytes(b"tampered")
    # When / Then
    with pytest.raises(Reject, match="custody_changed"):
        driver.compare(paths, manifest())


def test_same_machine_rejects_two_host_claim(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    data = json.loads(paths[1].read_bytes())
    data["machine_id"] = 10
    paths[1].write_text(json.dumps(data))
    # When / Then
    with pytest.raises(Reject, match="binding_or_bitwise"):
        driver.compare(paths, manifest())


def hash_only(paths: list[Path]) -> None:
    """What a hash-only rescue leaves locally: custody.json + JSON, no round tensors."""
    for path in paths:
        root = path.parent
        files = {str(p.relative_to(root)): fsha(p) for p in root.rglob("*") if p.is_file()}
        (root / "custody.json").write_text(json.dumps(files, sort_keys=True))
        for p in root.glob("round-*/published/**/*"):
            if p.is_file():
                p.unlink()
        # The rescue carries per-rank summaries and job bindings (HASH_RESCUE_NAMES).
        for rank in root.glob("round-*/published/rank-*"):
            (rank / "summary.json").write_text('{"backend": "cuda"}')
        for published in root.glob("round-*/published"):
            (published / "job.json").write_text("{}")


def test_hash_only_rescue_compares_from_bound_host_inventories(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    hash_only(paths)
    # When
    result = driver.compare(paths, manifest())
    # Then
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}


def test_full_rescue_missing_tensor_rejects_despite_inventory(tmp_path: Path) -> None:
    # Given: a full-rescue tree with one tensor deleted and a custody.json that still lists it.
    paths = results(tmp_path)
    root = paths[1].parent
    files = {str(p.relative_to(root)): fsha(p) for p in root.rglob("*") if p.is_file()}
    (root / "custody.json").write_text(json.dumps(files, sort_keys=True))
    (root / "round-1/published/rank-0/state.safetensors").unlink()
    # When / Then
    with pytest.raises(Reject, match="custody_changed"):
        driver.compare(paths, manifest())


@pytest.mark.parametrize(
    "fault,match",
    [
        ("mismatch", "custody_changed"),
        ("missing_rank", "custody_changed"),
        ("unbound", "inventory_unbound"),
        ("malformed", "inventory_malformed"),
        ("not_hex", "inventory_malformed"),
        ("oversized", "inventory_oversized"),
        ("absent", "inventory_missing"),
    ],
)
def test_hash_only_inventory_faults_reject(tmp_path: Path, fault: str, match: str) -> None:
    # Given
    paths = results(tmp_path)
    hash_only(paths)
    custody = paths[1].parent / "custody.json"
    data = json.loads(custody.read_bytes())
    if fault == "mismatch":  # Host B's inventory disagrees with what both results claim.
        data["round-1/published/rank-1/state.safetensors"] = "ab" * 32
    elif fault == "missing_rank":
        data = {k: v for k, v in data.items() if "/rank-1/" not in k}
    elif fault == "unbound":
        data["result.json"] = "00" * 32
    elif fault == "not_hex":
        data["round-0/published/rank-0/delta.bin"] = "ZZ" * 32
    custody.write_text(json.dumps(data))
    if fault == "malformed":
        custody.write_bytes(b"[not json")
    elif fault == "oversized":
        custody.write_bytes(b" " * (driver.INVENTORY_BYTES + 1))
    elif fault == "absent":
        custody.unlink()
    # When / Then
    with pytest.raises(Reject, match=match):
        driver.compare(paths, manifest())


def test_cross_host_hash_mismatch_rejects_with_hash_only_rescue(tmp_path: Path) -> None:
    # Given: host B honestly reports a different round hash for one rank.
    paths = results(tmp_path)
    data = json.loads(paths[1].read_bytes())
    data["rounds"][1]["rank-0/delta.bin"] = "cd" * 32
    paths[1].write_text(json.dumps(data))
    hash_only(paths)
    # When / Then
    with pytest.raises(Reject, match="binding_or_bitwise"):
        driver.compare(paths, manifest())


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
        driver.compare(paths, manifest())


def test_eight_gpu_h4_j2_results_compare_with_full_rank_coverage(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path, ranks=8, H=4, J=2)
    # When
    result = driver.compare(paths, manifest(8, 4, 2))
    # Then: 8 ranks x (5 finals + checkpoints 0,2,4), workload gate unchanged.
    rounds = json.loads(paths[0].read_bytes())["rounds"]
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}
    assert [len(r) for r in rounds] == [64, 64] and "rank-7/checkpoints/4.safetensors" in rounds[1]


@pytest.mark.parametrize("attack", ["missing_rank", "same_host", "tamper", "wrong_image"])
def test_eight_gpu_results_reject_rank_host_custody_and_shape_attacks(
    tmp_path: Path, attack: str
) -> None:
    # Given
    paths = results(tmp_path, ranks=8, H=4, J=2)
    datas = [json.loads(p.read_bytes()) for p in paths]
    if attack == "missing_rank":  # Both hosts agree on a 7-rank capture.
        for data in datas:
            data["rounds"] = [
                {k: v for k, v in r.items() if not k.startswith("rank-7/")} for r in data["rounds"]
            ]
    elif attack == "same_host":
        datas[1]["machine_id"] = datas[0]["machine_id"]
    elif attack == "wrong_image":  # Both hosts agree on an image the manifest never named.
        for data in datas:
            data["environment"]["image_digest"] = "sha256:" + "55" * 32
    for path, data in zip(paths, datas, strict=True):
        path.write_text(json.dumps(data))
    if attack == "tamper":
        (paths[1].parent / "round-1/published/rank-7/checkpoints/2.safetensors").write_bytes(b"x")
    match = {"missing_rank": "coverage_missing", "tamper": "custody_changed"}
    # When / Then
    with pytest.raises(Reject, match=match.get(attack, "binding_or_bitwise")):
        driver.compare(paths, manifest(8, 4, 2))


@pytest.mark.parametrize(
    "attack", ["two_ranks_j4", "eight_ranks_j4", "legacy_j15", "no_checkpoints"]
)
def test_coordinated_self_consistent_results_cannot_redefine_layout(
    tmp_path: Path, attack: str
) -> None:
    # Given: both hosts agree bitwise on a shape the 8-GPU H4/J2 seed manifest never named.
    shape = {"two_ranks_j4": (2, 4, 4), "eight_ranks_j4": (8, 4, 4), "legacy_j15": (8, 30, 15)}
    paths = results(tmp_path, *shape.get(attack, (8, 4, 2)))
    if attack == "no_checkpoints":  # What a J=-1 capture listed: finals only.
        for path in paths:
            data = json.loads(path.read_bytes())
            data["rounds"] = [
                {k: v for k, v in r.items() if "/checkpoints/" not in k} for r in data["rounds"]
            ]
            path.write_text(json.dumps(data))
    # When / Then: the manifest, not the results, defines ranks and checkpoints.
    with pytest.raises(Reject, match="binding_or_bitwise|coverage_missing"):
        driver.compare(paths, manifest(8, 4, 2))


@pytest.mark.parametrize("shape", [(8, 30, 0), (8, 30, -1), (8, 30, 7), (0, 4, 2)])
def test_round_files_refuses_non_positive_or_non_dividing_shape(
    shape: tuple[int, int, int],
) -> None:
    # When / Then
    with pytest.raises(Reject, match="layout_invalid"):
        driver.round_files(*shape)


def test_environment_rejects_driver_count_differing_from_devices(tmp_path: Path) -> None:
    # Given
    data = json.loads(results(tmp_path, ranks=8, H=4, J=2)[0].read_bytes())["environment"]
    data["drivers"] = data["drivers"][:7]
    # When / Then
    with pytest.raises(ValidationError, match="driver/device count"):
        driver.Environment.model_validate(data)


def test_eight_gpu_seed_manifest_drives_rank_and_checkpoint_coverage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: exact profile with an 8-GPU dp4 x ep2 layout and H4/J2 inner loop.
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["layout"].update(n_gpus=8, dp_size=4, ep_size=2)
    profile["inner"].update(H=4, J=2)
    profile_path = tmp_path / "profile-8.json"
    profile_path.write_text(json.dumps(profile))
    job = driver.check_inputs(seed_config(tmp_path, profile_path))
    published = tmp_path / "published"
    for rel in driver.round_files(8, 4, 2):
        (published / rel).parent.mkdir(parents=True, exist_ok=True)
        (published / rel).write_bytes(rel.encode())
    for rank in range(8):
        (published / f"rank-{rank}/summary.json").write_text('{"backend": "cuda"}')
    monkeypatch.setattr(driver, "validate_artifacts", lambda job, directory: None)
    # When
    captured = driver.capture(published, job)
    # Then
    assert len(job.sample_ids) == 32 and set(captured) == set(driver.round_files(8, 4, 2))
    (published / "rank-7/checkpoints/4.safetensors").unlink()
    with pytest.raises(FileNotFoundError):
        driver.capture(published, job)


def test_exact_profile_accepts_wire_defaults_without_training(tmp_path: Path) -> None:
    # Given
    cfg = seed_config(tmp_path, TREE / "experiments/gpu_network_v2/profile.json")
    # When
    accepted = driver.check_inputs(cfg)
    # Then
    assert accepted.run_id == json.loads(cfg.seed_job.read_bytes())["run_id"]


def seed_config(tmp_path: Path, profile_path: Path) -> driver.Qualification:
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
        sample_ids=list(range(wrapper.training.batch_samples())),
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
    return driver.Qualification(
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
