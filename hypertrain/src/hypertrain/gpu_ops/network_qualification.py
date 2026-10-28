"""Existing-host CUDA carry qualification; no provider actions or workload promotion.

Run with the existing project environment: python scripts/network_gpu_qualification.py
sources TREE | run CONFIG.json | compare CFG_A CFG_B HOST_A/result.json HOST_B/result.json
(--label-only | --known-hosts KH_A KH_B --host-fingerprint FP_A FP_B).
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
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Annotated, Literal, NamedTuple

from pydantic import BaseModel, ConfigDict, Field, StrictInt, model_validator

from hypertrain.gpu_ops import host_attest
from hypertrain.gpu_ops.cuda_executor_runner import Refused, measure_image_sources, sources_digest
from hypertrain.gpu_ops.journal import durable_write, fsha
from hypertrain.gpu_ops.launcher import Reject
from hypertrain.miner.island_launch import (
    IslandFailure,
    confined,
    gpu_uuid,
    island_env,
    launch_argv,
    validate_artifacts,
)
from hypertrain.protocol.messages import Hex64, ImageDigest, Pos
from hypertrain.protocol.messages_v2 import IslandJobV1, RunManifestV2

LEGACY_TIMING = {
    "horizon": 3600,
    "execution_cap": 40,
    "teardown": 300,
    "qualification": 900,
    "qual_stage": 300,
    "qual_probe": 180,
}
# ponytail: finite ceiling for one prune_rescued lifecycle; raise with a reviewed profile change.
MAX_LIFECYCLE_SECONDS = 86400
# CUDA compute capability (major, minor) -> SM counts a qualification may record, one row per
# measured device class (docs/adr/0007-sm-counts-per-arch.md). A device is qualified only as a
# pair in this table that also equals the manifest's reference_spec.sm_count.
SM_COUNTS: dict[tuple[int, int], frozenset[int]] = {
    (12, 0): frozenset({170}),  # RTX 5090 (sm_120), gpu_network_v2 hosts
    (10, 3): frozenset({148}),  # B300 SXM6 AC (sm_103), docs/b300-run.md hosts A and B
}


def unqualified_device(capabilities: list[tuple[int, int]], sm_counts: list[int]) -> str | None:
    """Why the first device whose (capability, SM count) pair is not in SM_COUNTS is refused."""
    for index, ((major, minor), sm) in enumerate(zip(capabilities, sm_counts, strict=True)):
        device = f"device {index}: capability {major}.{minor}"
        accepted = SM_COUNTS.get((major, minor))
        if accepted is None:
            return f"{device} is not a qualified device class"
        if sm not in accepted:
            return f"{device} qualifies {sorted(accepted)} SMs, not {sm}"
    return None


def lifecycle_timing(profile: dict) -> dict[str, int]:
    """Legacy "all" keeps its exact one-hour constants. prune_rescued derives every window from
    reviewed profile fields, each a positive int that must fit one finite lifecycle horizon."""
    a = profile.get("artifacts", {})
    if a.get("remote_retention", "all") == "all":
        return dict(LEGACY_TIMING)
    s, q = profile["schedule"], profile["microqualification"]
    fields = [
        s["hard_deadline_seconds"],
        s["staging_seconds"],
        s["beacon_service_seconds"],
        s["rescue_delete_seconds"],
        s["hard_margin_seconds"],
        s["executions_per_host"],
        q["qualification_deadline_seconds"],
        q["staging_seconds"],
        q["probe_and_transfer_seconds"],
        q["rescue_delete_seconds"],
        q["failure_margin_seconds"],
        a["rescue_transfer_seconds"],
        a["delete_absence_seconds"],
    ]
    if any(type(v) is not int or v <= 0 for v in fields):
        raise Reject("lifecycle_timing_invalid")
    horizon, rescue_delete = s["hard_deadline_seconds"], s["rescue_delete_seconds"]
    qualification = q["qualification_deadline_seconds"]
    cap = (horizon - s["staging_seconds"] - s["beacon_service_seconds"] - rescue_delete) - s[
        "hard_margin_seconds"
    ]
    cap //= s["executions_per_host"]
    if (
        horizon > MAX_LIFECYCLE_SECONDS
        or a["rescue_transfer_seconds"] + a["delete_absence_seconds"] > rescue_delete
        or q["rescue_delete_seconds"] < a["rescue_transfer_seconds"] + a["delete_absence_seconds"]
        or q["staging_seconds"]
        + q["probe_and_transfer_seconds"]
        + q["rescue_delete_seconds"]
        + q["failure_margin_seconds"]
        > qualification
        or qualification + s["hard_margin_seconds"] > horizon
        or cap <= 0
    ):
        raise Reject("lifecycle_timing_invalid")
    return {
        "horizon": horizon,
        "execution_cap": cap,
        "teardown": rescue_delete,
        "qualification": qualification,
        "qual_stage": q["staging_seconds"],
        "qual_probe": q["probe_and_transfer_seconds"],
    }


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
    drivers: Annotated[list[str], Field(min_length=2, max_length=8)]
    sm_counts: Annotated[list[Pos], Field(min_length=2, max_length=8)]
    # One per device. None only in a result written before capabilities were recorded; it is
    # omitted from JSON so such a result.json keeps its bytes.
    capabilities: list[tuple[StrictInt, StrictInt]] | None = Field(
        None, exclude_if=lambda v: v is None
    )
    image_digest: ImageDigest
    # One nvidia-smi form UUID per device, in device order. None only in a result written before
    # the field existed; it is omitted from JSON so such a result.json keeps its bytes.
    gpu_uuids: list[str] | None = Field(None, exclude_if=lambda v: v is None)
    # sha256 of the package files the image itself holds (cuda_executor_runner.sources_digest). None
    # only in a result written before the field existed, omitted from JSON like the two above.
    image_sources_sha256: Hex64 | None = Field(None, exclude_if=lambda v: v is None)

    @model_validator(mode="after")
    def _one_driver_per_device(self) -> Environment:
        if len(self.drivers) != len(self.sm_counts):
            raise ValueError("driver/device count mismatch")
        return self

    @model_validator(mode="after")
    def _one_distinct_uuid_per_device(self) -> Environment:
        if self.gpu_uuids is None:
            return self
        if len(self.gpu_uuids) != len(self.sm_counts):
            raise ValueError("gpu_uuids must hold one GPU UUID per device")
        if not all(host_attest.GPU_UUID.fullmatch(u) for u in self.gpu_uuids):
            raise ValueError("gpu_uuids must be lowercase GPU-<uuid> strings")
        if len(set(self.gpu_uuids)) != len(self.gpu_uuids):
            raise ValueError("gpu_uuids must be distinct")
        return self

    @model_validator(mode="after")
    def _qualified_devices(self) -> Environment:
        if self.capabilities is None:  # Literal[170] era: every such result is an RTX 5090.
            if any(s not in SM_COUNTS[(12, 0)] for s in self.sm_counts):
                raise ValueError("a result without capabilities qualifies 170-SM RTX 5090 only")
            return self
        if len(self.capabilities) != len(self.sm_counts):
            raise ValueError("capability/device count mismatch")
        reason = unqualified_device(self.capabilities, self.sm_counts)
        if reason is not None:
            raise ValueError(reason)
        return self


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


def round_files(ranks: int, H: int, J: int) -> list[str]:  # noqa: N803 - manifest names
    """Every rank's final artifacts plus rank-0's J-th checkpoints (only rank 0 writes them)."""
    if min(ranks, H, J) < 1 or H % J:
        raise Reject("qualification_layout_invalid")
    return [
        f"rank-{rank}/{rel}"
        for rank in range(ranks)
        for rel in (
            "state.safetensors",
            "ef.safetensors",
            "delta.bin",
            "leaves.json",
            "trace.json",
            *(f"checkpoints/{t}.safetensors" for t in range(0, H + 1, J) if not rank),
        )
    ]


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


def package_sources(tree_sources: Mapping[str, str]) -> dict[str, str]:
    """The `src/hypertrain/**/*.py` entries of `sources()`: what the installed image must hold."""
    return {
        name: digest
        for name, digest in tree_sources.items()
        if name.startswith("src/hypertrain/") and name.endswith(".py")
    }


def bind_image(tree_sources: Mapping[str, str], measured: Mapping[str, str]) -> None:
    """The package the image runs must be the package the tree holds, file for file."""
    wanted = package_sources(tree_sources)
    for name in sorted(wanted.keys() | measured.keys()):
        if wanted.get(name) != measured.get(name):
            raise Reject(f"qualification_tree_is_not_the_image:{name}")


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
    timing = lifecycle_timing(json.loads(cfg.profile.read_bytes()))
    if (
        receipts["instance_id"] != cfg.instance_id
        or receipts["machine_id"] != cfg.machine_id
        or receipts["role"] != cfg.role
        or receipts["admitted_unix"] != cfg.admitted_unix
        or receipts["image_digest"] != cfg.image_digest
        or receipts["known_hosts_sha256"] != cfg.known_hosts_sha256
        or receipts["lifecycle_contract_sha256"] != cfg.lifecycle_contract_sha256
        or receipts["supervisor_pid"] <= 0
        or receipts["deadline_unix"] > cfg.admitted_unix + timing["horizon"]
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


def inspect_environment(cfg: Qualification, n_gpus: int, sm_count: int) -> Environment:
    __import__("hypertrain.trainer")  # Configure determinism before torch import.
    import torch

    if not torch.cuda.is_available() or torch.cuda.device_count() != n_gpus:
        raise Reject("qualification_requires_actual_layout_cuda_devices")
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
    if len(drivers) != n_gpus:
        raise Reject("qualification_requires_actual_layout_cuda_devices")
    if any(d.strip() not in cfg.driver_allowlist for d in drivers):
        raise Reject("qualification_driver_mismatch")
    devices = [torch.cuda.get_device_properties(i) for i in range(n_gpus)]
    sm_counts = [d.multi_processor_count for d in devices]
    capabilities = [(d.major, d.minor) for d in devices]
    try:
        uuids = [gpu_uuid(d.uuid) for d in devices]
    except (IslandFailure, ValueError) as exc:
        raise Reject("qualification_gpu_uuid_malformed") from exc
    if any(s != sm_count for s in sm_counts):
        raise Reject("qualification_sm_profile_mismatch")
    reason = unqualified_device(capabilities, sm_counts)
    if reason is not None:
        raise Reject(f"qualification_sm_profile_unqualified:{reason}")
    return Environment.model_validate(
        {
            "backend": "cuda",
            "torch": str(torch.__version__),
            "cuda": torch.version.cuda,
            "python": sys.executable,
            "torch_path": str(Path(torch.__file__).resolve()),
            "drivers": [d.strip() for d in drivers],
            "sm_counts": sm_counts,
            "capabilities": capabilities,
            "image_digest": cfg.image_digest,
            "gpu_uuids": uuids,
        }
    )


def child_environment(cfg: Qualification, job: IslandJobV1) -> dict[str, str]:
    """The environment of a qualification island: this process's, under the manifest's pin.

    The island worker refuses a process that does not honor the pin, and `capture` refuses a rank
    whose summary does not show it (`validate_artifacts`), so a pinned manifest is qualified
    under exactly its pin whatever the host's own environment says.
    """
    return island_env(job, trace=True, base={**os.environ, "PYTHONPATH": str(cfg.tree / "src")})


def capture(directory: Path, job: IslandJobV1) -> dict[str, str]:
    validate_artifacts(job, directory)
    n_gpus = job.manifest.training.reference_spec.layout.n_gpus
    for rank in range(n_gpus):
        summary = json.loads((directory / f"rank-{rank}/summary.json").read_bytes())
        if summary["backend"] != "cuda":
            raise Reject("qualification_cpu_artifacts_refused")
    inner = job.manifest.training.inner
    return {rel: fsha(directory / rel) for rel in round_files(n_gpus, inner.H, inner.J)}


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
    ref = job.manifest.training.reference_spec
    environment = inspect_environment(cfg, ref.layout.n_gpus, ref.sm_count)
    # The tree is what runs here (PYTHONPATH) and the image is what audits run: they must be one.
    try:
        measured = measure_image_sources()
    except Refused as error:
        raise Reject("qualification_image_sources_unmeasurable") from error
    bind_image(cfg.sources, measured)
    environment = environment.model_copy(update={"image_sources_sha256": sources_digest(measured)})
    inner = job.manifest.training.inner
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
    timing = lifecycle_timing(json.loads(cfg.profile.read_bytes()))
    # Legacy: 600 + 900 means staging ends by admitted + 300 (= qual_stage).
    if time.time() > cfg.admitted_unix + timing["qual_stage"]:
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
                body.update(w=job.w + 1, global_step0=job.global_step0 + inner.H)
            left = min(
                timing["qual_probe"] // 2,
                cfg.admitted_unix + timing["qual_stage"] + timing["qual_probe"] - time.time(),
            )
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
                    env=child_environment(cfg, current),
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


# Host custody.json: flat {relative path: sha256}; 1 MiB holds thousands of entries.
INVENTORY_BYTES = 1 << 20


def inventory(data: bytes) -> dict[str, str]:
    """Parse the driver's custody.json (written after every round file) at a trust boundary."""
    if len(data) > INVENTORY_BYTES:
        raise Reject("qualification_inventory_oversized")
    try:
        parsed = json.loads(data)
    except ValueError as exc:
        raise Reject("qualification_inventory_malformed") from exc
    if not isinstance(parsed, dict) or not all(
        isinstance(v, str) and re.fullmatch(r"[0-9a-f]{64}", v) for v in parsed.values()
    ):
        raise Reject("qualification_inventory_malformed")
    return parsed


def host_inventory(result_path: Path) -> dict[str, str]:
    """Bounded read of custody.json beside result.json; it must list this exact result."""
    path = result_path.parent / "custody.json"
    if path.is_symlink() or not path.is_file():
        raise Reject("qualification_inventory_missing")
    with path.open("rb") as file:
        claimed = inventory(file.read(INVENTORY_BYTES + 1))
    if claimed.get("result.json") != fsha(result_path):
        raise Reject("qualification_inventory_unbound")
    return claimed


class HostPin(NamedTuple):
    """What the owner commits to for one host: its known_hosts file and the SHA256: fingerprint
    typed from the provider console, outside the repository."""

    known_hosts: Path
    fingerprint: str


def pinned_keys(configs: Sequence[bytes], pins: Sequence[HostPin]) -> list[str]:
    """Each host's SSH key: the one in the known_hosts file its config hashes, and the one the
    owner typed the fingerprint of."""
    if len(configs) != 2 or len(pins) != 2:
        raise Reject("qualification_requires_two_host_results")
    keys = []
    for config, pin in zip(configs, pins, strict=True):
        known_hosts = pin.known_hosts.read_bytes()
        committed = Qualification.model_validate_json(config).known_hosts_sha256
        if hashlib.sha256(known_hosts).hexdigest() != committed:
            raise Reject("qualification_known_hosts_changed")
        key = host_attest.pinned_key(known_hosts)
        if host_attest.fingerprint(key) != pin.fingerprint:
            raise Reject("qualification_host_fingerprint_mismatch")
        keys.append(key)
    return keys


def _attested(
    paths: list[Path], raw: list[bytes], results: tuple[Result, Result], keys: Sequence[str]
) -> None:
    """Each host signed, with its pinned key, a statement that says what its result.json says."""
    if keys[0] == keys[1]:
        raise Reject("qualification_hosts_not_distinct:host_key")
    statements = []
    for path, data, result, key in zip(paths, raw, results, keys, strict=True):
        attestation = host_attest.load(path)
        host_attest.verify(attestation.raw, attestation.signature, key)
        statement, environment = attestation.statement, result.environment
        if (
            statement.result_sha256 != hashlib.sha256(data).hexdigest()
            or statement.config_sha256 != result.config_sha256
            or statement.image_digest != environment.image_digest
            or environment.gpu_uuids is None
            or not set(environment.gpu_uuids) <= set(statement.gpu_uuids)
            or not set(environment.drivers) <= set(statement.drivers)
        ):
            raise Reject("qualification_attestation_unbound")
        statements.append(statement)
    first, second = statements
    if set(first.gpu_uuids) & set(second.gpu_uuids):
        raise Reject("qualification_hosts_not_distinct:gpu_uuid")
    if first.boot_id == second.boot_id:
        raise Reject("qualification_hosts_not_distinct:boot_id")


def _image_sources(results: tuple[Result, Result], *, strict: bool) -> None:
    """Each result's image measurement is the digest of its own `sources`, and the hosts agree.
    A result from before the field passes only label-only, as a whole pair."""
    claimed = [r.environment.image_sources_sha256 for r in results]
    if strict and None in claimed:
        raise Reject("qualification_image_sources_missing")
    for result, digest in zip(results, claimed, strict=True):
        if digest is not None and digest != sources_digest(package_sources(result.sources)):
            raise Reject("qualification_image_sources_mismatch")
    if claimed[0] != claimed[1]:
        raise Reject("qualification_image_sources_mismatch")


def compare(
    paths: list[Path], manifest: RunManifestV2, *, host_keys: Sequence[str] | None
) -> dict[str, bool]:
    """Ranks/H/J/image come from the caller's validated manifest, never from a Result.

    A full rescue proves every round file by its rescued bytes. A hash-only rescue (no round
    file rescued at all) proves them by the host inventory: host-computed hashes, bound to
    result.json and cross-checked between the two hosts. A partial tree is never accepted.

    Machine ids, instance ids and roles are labels the result's writer chooses. `host_keys`, the
    SSH host keys pinned for the two hosts in the order of `paths`, make each host sign a
    statement about its own result (host_attest). None is the explicit label-only mode: nothing
    then proves two machines, so only a caller whose machine ids come from a provider listing
    (the Vast lifecycle) may use it.
    """
    if len(paths) != 2 or (host_keys is not None and len(host_keys) != 2):
        raise Reject("qualification_requires_two_host_results")
    raw = [p.read_bytes() for p in paths]
    a, b = [Result.model_validate_json(data) for data in raw]
    ref, inner = manifest.training.reference_spec, manifest.training.inner
    if (
        a.machine_id == b.machine_id
        or a.instance_id == b.instance_id
        or a.role == b.role
        or a.profile_sha256 != b.profile_sha256
        or a.sources != b.sources
        or a.environment.image_digest != b.environment.image_digest
        or any(
            r.environment.image_digest != ref.image_digest
            or len(r.environment.sm_counts) != ref.layout.n_gpus
            for r in (a, b)
        )
        or a.rounds != b.rounds
    ):
        raise Reject("qualification_two_host_binding_or_bitwise_mismatch")
    expected = set(round_files(ref.layout.n_gpus, inner.H, inner.J))
    for path, result in zip(paths, (a, b), strict=True):
        # Hash-only rescue still carries JSON (job.json, summary.json): only tensors decide.
        hash_only = not any(
            (p := path.parent / f"round-{index}/published/{rel}").is_file() or p.is_symlink()
            for index in range(len(result.rounds))
            for rel in expected
        )
        claimed = host_inventory(path) if hash_only else {}
        for index, files in enumerate(result.rounds):
            if set(files) != expected:
                raise Reject("qualification_full_round_coverage_missing")
            for rel, want in files.items():
                published = path.parent / f"round-{index}/published"
                if hash_only:
                    actual = claimed.get(f"round-{index}/published/{rel}")
                elif (published / rel).exists() or (published / rel).is_symlink():
                    actual = fsha(confined(published, rel))
                else:
                    actual = None
                if actual != want:
                    raise Reject("qualification_artifact_custody_changed")
    _image_sources((a, b), strict=host_keys is not None)
    if host_keys is not None:
        _attested(paths, raw, (a, b), host_keys)
    return {"two_host_bitwise_match": True, "workload_allowed": False}


USAGE = (
    "usage: sources TREE | run CONFIG | compare CFG_A CFG_B A/result.json B/result.json "
    "(--label-only | --known-hosts KH_A KH_B --host-fingerprint FP_A FP_B)"
)


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
        case ["compare", config_a, config_b, a, b, *mode]:
            pins: list[HostPin] | None
            match mode:
                case ["--label-only"]:
                    pins = None
                case ["--known-hosts", hosts_a, hosts_b, "--host-fingerprint", fp_a, fp_b]:
                    pins = [HostPin(Path(hosts_a), fp_a), HostPin(Path(hosts_b), fp_b)]
                case _:
                    raise SystemExit(USAGE)
            jobs = []
            for config_path, result in ((Path(config_a), Path(a)), (Path(config_b), Path(b))):
                if Result.model_validate_json(result.read_bytes()).config_sha256 != fsha(
                    config_path
                ):
                    raise Reject("qualification_result_config_mismatch")
                jobs.append(
                    check_inputs(Qualification.model_validate_json(config_path.read_bytes()))
                )
            if jobs[0].manifest != jobs[1].manifest:
                raise Reject("qualification_two_host_manifest_mismatch")
            keys = None
            if pins is not None:
                keys = pinned_keys([Path(config_a).read_bytes(), Path(config_b).read_bytes()], pins)
            print(json.dumps(compare([Path(a), Path(b)], jobs[0].manifest, host_keys=keys)))
        case _:
            raise SystemExit(USAGE)
