"""Existing-host CUDA carry qualification; no provider actions or workload promotion.

Run with the existing project environment: python scripts/network_gpu_qualification.py
sources TREE | run CONFIG.json | compare HOST_A/result.json HOST_B/result.json.
"""

from __future__ import annotations

import csv
import hashlib
import io
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import time
from pathlib import Path
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, StrictInt

from hypertrain.gpu_ops.journal import durable_write, fsha
from hypertrain.gpu_ops.launcher import Reject
from hypertrain.miner.island_launch import confined, launch_argv, validate_artifacts
from hypertrain.protocol.messages import Hex64, ImageDigest
from hypertrain.protocol.messages_v2 import IslandJobV1


class Boundary(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class Qualification(Boundary):
    tree: Path
    profile: Path
    profile_sha256: Hex64
    sources: dict[str, Hex64]
    registry_manifest: Path
    image_digest: ImageDigest
    driver_allowlist: Annotated[list[str], Field(min_length=1)]
    seed_job: Path
    hotkey: str
    output: Path
    admitted_unix: StrictInt
    instance_id: StrictInt
    machine_id: StrictInt
    role: str
    lifecycle_contract_sha256: Hex64
    known_hosts_sha256: Hex64
    lifecycle_receipts: Path
    lifecycle_receipts_sha256: Hex64


class Environment(Boundary):
    backend: Literal["cuda"]
    torch: Literal["2.14.0+cu130"]
    cuda: Literal["13.0"]
    python: Literal["/opt/hypertrain/venv/bin/python"]
    torch_path: str
    drivers: Annotated[list[str], Field(min_length=2, max_length=2)]
    sm_counts: Annotated[list[Literal[170]], Field(min_length=2, max_length=2)]
    image_digest: ImageDigest


class Result(Boundary):
    status: Literal["CUDA_ROUNDS_CAPTURED_NOT_WORKLOAD_PASS"]
    config_sha256: Hex64
    profile_sha256: Hex64
    sources: dict[str, Hex64]
    environment: Environment
    instance_id: StrictInt
    machine_id: StrictInt
    role: str
    rounds: Annotated[list[dict[str, Hex64]], Field(min_length=2, max_length=2)]
    elapsed_seconds: float
    workload_allowed: Literal[False]


def select_inventory(text: str, allocated: int, drivers: list[str]) -> tuple[str, str]:
    """Validate physical CSV; select two full UUIDs, not CUDA qualification."""
    rows = list(csv.reader(io.StringIO(text), skipinitialspace=True))
    if type(allocated) is not int or allocated < 2 or len(rows) != allocated:
        raise ValueError("actual allocated GPU count mismatch")
    uuids = []
    for row in rows:
        if (
            len(row) != 3
            or not re.fullmatch(r"GPU-[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", row[0])
            or row[1].strip() not in ("NVIDIA GeForce RTX 5090", "GeForce RTX 5090")
            or row[2].strip() not in drivers
        ):
            raise ValueError("allocated GPU UUID/name/driver mismatch")
        uuids.append("GPU-" + row[0][4:].lower())
    if len(set(uuids)) != allocated:
        raise ValueError("duplicate allocated GPU UUID")
    selected = sorted(uuids)
    return selected[0], selected[1]


def sources(tree: Path) -> dict[str, str]:
    paths = sorted((tree / "src/hypertrain").rglob("*.py")) + [
        tree / p
        for p in (
            "experiments/gpu_network_v2/run.py",
            "experiments/gpu_network_v2/orchestrate.py",
            "experiments/gpu_network_v2/profile.json",
            "scripts/network_gpu_qualification.py",
            "docker/Dockerfile.gpu",
            "pyproject.toml",
            "uv.lock",
        )
    ]
    if any(p.is_symlink() for p in paths):
        raise Reject("qualification_source_symlink")
    return {str(p.relative_to(tree)): fsha(p) for p in paths}


def bundle(tree: Path) -> bytes:
    """Deterministic source-only archive; jobs/receipts stay separately hash-bound."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        for rel in sorted(sources(tree)):
            data = (tree / rel).read_bytes()
            info = tarfile.TarInfo(rel)
            info.size, info.mode, info.mtime = len(data), 0o600, 0
            archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def check_inputs(cfg: Qualification) -> IslandJobV1:
    if sources(cfg.tree) != cfg.sources or fsha(cfg.profile) != cfg.profile_sha256:
        raise Reject("qualification_source_or_profile_changed")
    if "sha256:" + fsha(cfg.registry_manifest) != cfg.image_digest:
        raise Reject("qualification_registry_digest_mismatch")
    registry = json.loads(cfg.registry_manifest.read_bytes())
    descriptors = registry.get("layers", registry.get("manifests", []))
    if (
        registry.get("schemaVersion") != 2
        or not descriptors
        or sum(d["size"] for d in descriptors) > 20_000_000_000
    ):
        raise Reject("qualification_registry_descriptors_missing")
    if fsha(cfg.lifecycle_receipts) != cfg.lifecycle_receipts_sha256:
        raise Reject("qualification_lifecycle_receipts_changed")
    receipts = json.loads(cfg.lifecycle_receipts.read_bytes())
    if (
        receipts["instance_id"] != cfg.instance_id
        or receipts["machine_id"] != cfg.machine_id
        or receipts["role"] != cfg.role
        or receipts["admitted_unix"] != cfg.admitted_unix
        or receipts["image_digest"] != cfg.image_digest
        or receipts["known_hosts_sha256"] != cfg.known_hosts_sha256
        or receipts["lifecycle_contract_sha256"] != cfg.lifecycle_contract_sha256
        or receipts["supervisor_pid"] <= 0
        or receipts["deadline_unix"] > cfg.admitted_unix + 3600
        or receipts["cuda_observed"] is not True
    ):
        raise Reject("qualification_owned_lifecycle_receipt_missing")
    p = json.loads(cfg.profile.read_bytes())
    job = IslandJobV1.model_validate_json(cfg.seed_job.read_bytes())
    training = job.manifest.training.model_dump(mode="json")
    exact = all(
        all(training[section].get(k) == v for k, v in p[section].items() if k != "lr_schedule")
        for section in ("model", "inner", "outer")
    )
    exact = exact and all(
        training["inner"]["lr_schedule"].get(k) == v for k, v in p["inner"]["lr_schedule"].items()
    )
    if (
        p["profile_version"] != 2
        or not exact
        or training["reference_spec"]["layout"] != p["layout"]
        or training["reference_spec"]["image_digest"] != cfg.image_digest
        or training["reference_spec"]["driver_allowlist"] != cfg.driver_allowlist
        or job.global_step0 != 0
        or job.manifest.training.dataset.n_samples < 4096
        or cfg.instance_id <= 0
        or cfg.machine_id <= 0
    ):
        raise Reject("qualification_exact_profile_or_receipt_mismatch")
    return job


def inspect_environment(cfg: Qualification) -> Environment:
    __import__("hypertrain.trainer")  # Configure determinism before torch import.
    import torch

    if not torch.cuda.is_available() or torch.cuda.device_count() != 2:
        raise Reject("qualification_requires_actual_two_cuda_devices")
    if not Path(torch.__file__).resolve().is_relative_to("/venv/main"):
        raise Reject("qualification_torch_import_origin_mismatch")
    smi = shutil.which("nvidia-smi")
    if smi is None:
        raise Reject("qualification_driver_inspector_missing")
    drivers = subprocess.run(  # noqa: S603 - fixed GPU query, resolved executable
        [smi, "--query-gpu=driver_version", "--format=csv,noheader"],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    ).stdout.splitlines()
    if len(drivers) != 2 or any(d.strip() not in cfg.driver_allowlist for d in drivers):
        raise Reject("qualification_driver_mismatch")
    return Environment.model_validate(
        {
            "backend": "cuda",
            "torch": str(torch.__version__),
            "cuda": torch.version.cuda,
            "python": sys.executable,
            "torch_path": str(Path(torch.__file__).resolve()),
            "drivers": [d.strip() for d in drivers],
            "sm_counts": [
                torch.cuda.get_device_properties(i).multi_processor_count for i in range(2)
            ],
            "image_digest": cfg.image_digest,
        }
    )


def capture(directory: Path, job: IslandJobV1) -> dict[str, str]:
    validate_artifacts(job, directory)
    fingerprints = {}
    for rank in range(2):
        summary = json.loads((directory / f"rank-{rank}/summary.json").read_bytes())
        if summary["backend"] != "cuda":
            raise Reject("qualification_cpu_artifacts_refused")
        for rel in [
            "state.safetensors",
            "ef.safetensors",
            "delta.bin",
            "leaves.json",
            "trace.json",
        ] + [f"checkpoints/{t}.safetensors" for t in range(0, 31, 5)]:
            fingerprints[f"rank-{rank}/{rel}"] = fsha(directory / f"rank-{rank}/{rel}")
        if not (directory / f"rank-{rank}/trace.json").is_file():
            raise Reject("qualification_trace_missing")
    return fingerprints


def run(config_path: Path) -> Result:
    cfg = Qualification.model_validate_json(config_path.read_bytes())
    job = check_inputs(cfg)
    for key, value in {
        "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
        "CUDA_DISABLE_PTX_JIT": "1",
        "OMP_NUM_THREADS": "1",
        "MKL_NUM_THREADS": "1",
        "PYTHONHASHSEED": "0",
        "NVIDIA_TF32_OVERRIDE": "0",
        "HT_ISLAND_TRACE": "1",
    }.items():
        os.environ[key] = value
    if os.environ.get("HT_IMAGE_DIGEST") != cfg.image_digest:
        raise Reject("qualification_actual_image_receipt_missing")
    environment = inspect_environment(cfg)
    from hypertrain.auditor.replay import AnchorCache, pack_state, unpack_state

    theta, state = unpack_state(
        confined(cfg.seed_job.parent, job.object_paths["start_state"]).read_bytes()
    )
    anchor = AnchorCache().genesis(job.manifest, cfg.hotkey, theta)
    if state is None or pack_state(theta, state) != pack_state(anchor.theta, anchor.state):
        raise Reject("qualification_requires_authenticated_genesis_carry")
    if confined(cfg.seed_job.parent, job.object_paths["ef_in"]).read_bytes() != pack_state(
        anchor.ef
    ):
        raise Reject("qualification_requires_genesis_ef")
    if time.time() + 600 > cfg.admitted_unix + 900:
        raise Reject("qualification_staging_deadline_exceeded")
    cfg.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    durable_write(cfg.output / "config.json", config_path.read_bytes())
    started = time.monotonic()
    rounds = []
    try:
        previous = None
        for index in range(2):
            directory = cfg.output / f"round-{index}/published"
            directory.mkdir(mode=0o700, parents=True)
            for relative in job.object_paths.values():
                target = directory / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(confined(cfg.seed_job.parent, relative), target)
            body = job.model_dump(mode="json")
            if previous is not None:
                for key, rel in [("start_state", "state.safetensors"), ("ef_in", "ef.safetensors")]:
                    shutil.copyfile(previous / "rank-0" / rel, directory / job.object_paths[key])
                    body[key + "_sha256"] = fsha(directory / job.object_paths[key])
                body.update(w=job.w + 1, global_step0=30)
            left = min(90, cfg.admitted_unix + 480 - time.time())
            if left <= 0:
                raise Reject("qualification_probe_deadline_exceeded")
            body["deadline"] = min(job.deadline, int(time.time() + left))
            current = IslandJobV1.model_validate(body)
            durable_write(directory / "job.json", current.model_dump_json().encode())
            with (
                (directory / "driver.stdout").open("xb") as out,
                (directory / "driver.stderr").open("xb") as err,
            ):
                with subprocess.Popen(  # noqa: S603 - shared fixed torchrun argv, typed job
                    launch_argv(current, directory, "cuda"),
                    cwd=cfg.tree,
                    stdout=out,
                    stderr=err,
                    start_new_session=True,
                    env={**os.environ, "PYTHONPATH": str(cfg.tree / "src")},
                ) as proc:
                    try:
                        code = proc.wait(timeout=left)
                    finally:
                        if proc.poll() is None:
                            os.killpg(proc.pid, signal.SIGKILL)
                    if code:
                        raise Reject("qualification_cuda_runner_failed")
            previous = directory
            rounds.append(capture(previous, current))
        result = Result(
            status="CUDA_ROUNDS_CAPTURED_NOT_WORKLOAD_PASS",
            config_sha256=fsha(config_path),
            profile_sha256=cfg.profile_sha256,
            sources=cfg.sources,
            environment=environment,
            instance_id=cfg.instance_id,
            machine_id=cfg.machine_id,
            role=cfg.role,
            rounds=rounds,
            elapsed_seconds=time.monotonic() - started,
            workload_allowed=False,
        )
        durable_write(cfg.output / "result.json", result.model_dump_json().encode())
        return result
    finally:
        # ponytail: custody only; provider teardown remains existing root-owned gpu_ops.
        inventory = {
            str(p.relative_to(cfg.output)): fsha(p) for p in cfg.output.rglob("*") if p.is_file()
        }
        durable_write(cfg.output / "custody.json", json.dumps(inventory, sort_keys=True).encode())


def compare(paths: list[Path]) -> dict[str, bool]:
    if len(paths) != 2:
        raise Reject("qualification_requires_two_host_results")
    a, b = [Result.model_validate_json(p.read_bytes()) for p in paths]
    if (
        a.machine_id == b.machine_id
        or a.instance_id == b.instance_id
        or a.role == b.role
        or a.profile_sha256 != b.profile_sha256
        or a.sources != b.sources
        or a.environment.image_digest != b.environment.image_digest
        or a.rounds != b.rounds
    ):
        raise Reject("qualification_two_host_binding_or_bitwise_mismatch")
    for path, result in zip(paths, (a, b), strict=True):
        for index, files in enumerate(result.rounds):
            expected = {
                f"rank-{rank}/{rel}"
                for rank in range(2)
                for rel in [
                    "state.safetensors",
                    "ef.safetensors",
                    "delta.bin",
                    "leaves.json",
                    "trace.json",
                ]
                + [f"checkpoints/{t}.safetensors" for t in range(0, 31, 5)]
            }
            if set(files) != expected:
                raise Reject("qualification_full_round_coverage_missing")
            for rel, want in files.items():
                if fsha(confined(path.parent / f"round-{index}/published", rel)) != want:
                    raise Reject("qualification_artifact_custody_changed")
    return {"two_host_bitwise_match": True, "workload_allowed": False}


def main() -> None:
    """Existing source-host qualification CLI; no provider or workload promotion."""
    match sys.argv[1:]:
        case ["sources", tree]:
            print(json.dumps(sources(Path(tree)), sort_keys=True))
        case ["bundle-hash", tree]:
            data = bundle(Path(tree))
            print(json.dumps({"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}))
        case ["bundle", tree, destination]:
            durable_write(Path(destination), bundle(Path(tree)))
        case ["run", config]:
            print(run(Path(config)).model_dump_json())
        case ["compare", a, b]:
            print(json.dumps(compare([Path(a), Path(b)])))
        case _:
            raise SystemExit(
                "usage: sources TREE | run CONFIG | compare A/result.json B/result.json"
            )
