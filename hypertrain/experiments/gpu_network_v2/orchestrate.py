"""Reviewed D2 CLI using the existing provider/lifecycle; no implicit spend authorization."""

from __future__ import annotations

import argparse
import importlib.util
import io
import json
import math
import os
import shlex
import subprocess
import sys
import tarfile
import threading
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from decimal import Decimal
from pathlib import Path
from typing import Any

from pydantic import BaseModel, ConfigDict, Field, StrictInt

from hypertrain.gpu_ops import budget
from hypertrain.gpu_ops.journal import (
    Journal,
    Record,
    durable_write,
    flock,
    fsha,
    sha256,
)
from hypertrain.gpu_ops.launcher import Orchestrator, Reject
from hypertrain.gpu_ops.provider import Provider, list_rows, loopback, read_api_key
from hypertrain.gpu_ops.remote import Ssh


class D2Job(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    role: str
    name: str = Field(pattern=r"^[a-zA-Z0-9-]+$")
    equivalents: StrictInt = Field(ge=1, le=12)
    backend: str = Field(pattern=r"^cuda$")
    directory: Path
    job_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    job_digest: str = Field(pattern=r"^[0-9a-f]{64}$")
    inputs: dict[str, str]


class D2Reservation(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    schema_version: str = Field(alias="schema", pattern=r"^ht-d2-execution-reservation/1$")
    action: str = Field(pattern=r"^execute_plan$")
    owner: str
    run_id: str = Field(pattern=r"^[0-9a-f]{64}$")
    reference_hash: str = Field(pattern=r"^[0-9a-f]{64}$")
    qualification_authority_hash: str = Field(pattern=r"^[0-9a-f]{64}$")
    profile_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    runtime_evidence_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    source_map_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    bundle_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    image_digest: str = Field(pattern=r"^sha256:[0-9a-f]{64}$")
    cutoff_unix: StrictInt = Field(ge=1)
    budget_authority_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    budget_plan_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    quota: dict[str, StrictInt]
    roles: dict[str, dict[str, StrictInt | str]]
    jobs_sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    jobs: list[D2Job] = Field(min_length=122, max_length=122)
    files: dict[str, dict[str, str]]


class DeadlineSsh(Ssh):
    """Existing SSH methods, one absolute clock checked at each subprocess boundary."""

    cutoff: float
    lifecycle: Orchestrator
    _limit: float
    cancel: threading.Event | None = None

    @property
    def timeout(self) -> int:
        remaining = int(min(self._limit, self.cutoff - time.time()))
        if (
            remaining <= 0
            or self.lifecycle.cleanup_started()
            or self.cancel is not None
            and self.cancel.is_set()
        ):
            raise Reject("remote_absolute_deadline_exhausted")
        return remaining

    @timeout.setter
    def timeout(self, value: int) -> None:
        self._limit = value


def deadline_ssh(orch: Orchestrator, role: str, cutoff: float) -> DeadlineSsh:
    ssh = orch.ssh(role)
    bounded = DeadlineSsh(**vars(ssh))
    bounded.cutoff, bounded.lifecycle = cutoff, orch
    return bounded


def effective_bundle(driver: Any, tree: Path, profile: Path) -> tuple[Record, bytes]:
    """Explicit signed profile overlay; all other approved bytes remain unchanged."""
    sources = driver.sources(tree)
    sources["experiments/gpu_network_v2/profile.json"] = fsha(profile)
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        for rel in sorted(sources):
            data = (
                profile.read_bytes()
                if rel == "experiments/gpu_network_v2/profile.json"
                else (tree / rel).read_bytes()
            )
            info = tarfile.TarInfo(rel)
            info.size, info.mode, info.mtime = len(data), 0o600, 0
            archive.addfile(info, io.BytesIO(data))
    return sources, buffer.getvalue()


def refresh_readonly(provider: Provider) -> Record:
    """Fresh GET-only custody/account evidence; no credentials in return value."""
    account = provider.call("GET", "/api/v0/users/current/", "network-account")
    inventory = provider.call("GET", "/api/v1/instances/", "network-instances")
    volumes = provider.call("GET", "/api/v0/volumes/", "network-volumes")
    now = int(time.time())
    filters = urllib.parse.quote(json.dumps({"day": {"gte": now - 2592000, "lte": now}}))
    charges = provider.call(
        "GET", f"/api/v0/charges/?select_filters={filters}&limit=500", "network-charges"
    )
    credit = budget.dec(account.parsed.get("credit")) if account.ok() else None
    live = list_rows(inventory)
    if (
        credit is None
        or live is None
        or not volumes.ok()
        or not charges.ok()
        or volumes.parsed.get("next_token")
        or charges.parsed.get("next_token")
        or not isinstance(volumes.parsed.get("volumes"), list)
        or not isinstance(charges.parsed.get("results"), list)
    ):
        raise Reject("fresh_account_inventory_or_charges_unknown")
    return {
        "authenticated": True,
        "available_credit_usd": str(credit),
        "instances": live,
        "volumes": volumes.parsed["volumes"],
        "charges": charges.parsed["results"],
        "verified_unix": int(time.time()),
        "methods": ["GET"] * 4,
    }


def artifact_budget(profile: dict[str, Any]) -> Record:
    """Shape/count bounds from the actual AdamW checkpoint and trace paths; no allocation."""
    m, inn, lay, a = (profile[k] for k in ("model", "inner", "layout", "artifacts"))
    d, e, ff, layers, vocab = (m[k] for k in ("d_model", "n_experts", "d_ff", "n_layers", "vocab"))
    params = 2 * vocab * d + d + layers * (2 * d + 4 * d * d + d * e + 3 * e * d * ff)
    if (
        params != m["param_count"]
        or inn["opt"] != "adamw"
        or inn["H"] % inn["J"]
        or lay != {"pp": 1, "n_gpus": 2, "dp_size": 1, "ep_size": 2, "zero1": True}
        or m["n_heads"] != m["n_kv_heads"]
        or e != 4
    ):
        raise Reject("unsupported_artifact_geometry")
    tensors = 3 + 10 * layers
    leaves = inn["H"] // inn["J"] + 1
    header = a["safetensors_header_bytes_per_file"]
    state = 12 * params + header
    ef = 4 * params + header
    # SparseLoCo framing plus ceil(.01*n) indices/codes/levels for each tensor.
    delta = math.ceil(params * 0.01) * 5 + tensors * 256
    forward = inn["grad_accum"] * (5 + 6 * layers + 20 * layers)
    # Conservative: every gradient replicated; every parameter owned for all 3 state hooks.
    step_events = forward + tensors * 3 + tensors + tensors * 3 + 6 + tensors * 3
    trace_events = inn["H"] * step_events + (leaves + 1) * 9 * tensors
    trace = trace_events * a["trace_bytes_per_event"]
    sample_count = inn["H"] * inn["micro_batch"] * inn["grad_accum"] * lay["n_gpus"]
    sample_bytes = sample_count * (m["seq_len"] + 1) * 2
    # Inputs remain at job root AND copied into published; proofs/job JSON in metadata budget.
    inputs = 2 * (state + ef + header + sample_bytes)
    outputs = lay["n_gpus"] * ((leaves + 1) * state + ef + delta + trace)
    logs = max(a["logs_bytes_per_execution"], 2 * math.ceil(max(trace, state) / 1024) * 1024)
    execution = inputs + outputs + logs + a["metadata_bytes_per_execution"]
    retained = execution * profile["schedule"]["executions_per_host"]
    # Tar record rounding + path headers + terminating records for every retained file.
    files_per_execution = lay["n_gpus"] * (leaves + 5) + 14
    tar_overhead = (
        files_per_execution * profile["schedule"]["executions_per_host"] * 2048 + 1048576 + 10240
    )
    rescue = retained + tar_overhead
    return {
        "param_count": params,
        "tensor_count": tensors,
        "state_bytes": state,
        "trace_events_per_rank": trace_events,
        "trace_bytes_per_rank": trace,
        "execution_bytes": execution,
        "retained_bytes_per_host": retained,
        "rescue_tar_bytes_per_host": rescue,
        "disk_artifacts_peak_bytes": retained + rescue,
        "artifact_transfer_bytes_per_host": retained + rescue,
        "rescue_min_bytes_per_second": math.ceil(rescue / a["rescue_transfer_seconds"]),
    }


def readiness_budget(profile: dict[str, Any], evidence: dict[str, Any]) -> Record:
    """Measured transport/image staging mandatory; advertised Mbps never qualifies rescue."""
    bounds = artifact_budget(profile)
    a = profile["artifacts"]
    rate, image, stage = (
        a[k]
        for k in (
            "rescue_bytes_per_second",
            "image_and_dependency_bytes",
            "staging_bytes_per_host",
        )
    )
    if (
        type(rate) is not int
        or rate < bounds["rescue_min_bytes_per_second"]
        or type(image) is not int
        or image < 1
        or type(stage) is not int
        or stage < 1
        or evidence.get("measured_rescue_bytes_per_second") != rate
        or evidence.get("image_and_dependency_bytes") != image
        or evidence.get("staging_bytes_per_host") != stage
    ):
        raise Reject("measured_artifact_transport_or_image_budget_missing")
    if bounds["disk_artifacts_peak_bytes"] + image + 2 * stage > a["disk_bytes"]:
        raise Reject("artifact_disk_budget_exceeded")
    if a["rescue_transfer_seconds"] + a["delete_absence_seconds"] > 300:
        raise Reject("rescue_delete_budget_exceeded")
    return {
        **bounds,
        "priced_transfer_gb_per_host": math.ceil(
            (bounds["artifact_transfer_bytes_per_host"] + image + stage) / 1_000_000_000
        ),
    }


def staged_command(
    profile: dict[str, Any],
    root: str,
    name: str,
    python: str,
    image: str,
    backend: str,
    *,
    qualification: bool = False,
) -> str:
    """One real launcher command; CUDA token and interpreter are not inferred from CPU tests."""
    if backend not in ("cpu", "cuda") or not name.replace("-", "").isalnum():
        raise Reject("invalid_staged_job_or_backend")
    pin = profile["runtime"]
    if backend == "cuda":
        if (
            python != pin["remote_python"]
            or image != pin["image_digest"]
            or not pin["driver_allowlist"]
            or (not qualification and not pin["qualified"])
        ):
            raise Reject("cuda_image_interpreter_profile_unqualified")
    env = " ".join(
        shlex.quote(f"{k}={pin[k]}")
        for k in (
            "CUBLAS_WORKSPACE_CONFIG",
            "CUDA_DISABLE_PTX_JIT",
            "OMP_NUM_THREADS",
            "MKL_NUM_THREADS",
            "PYTHONHASHSEED",
            "NVIDIA_TF32_OVERRIDE",
        )
    )
    return (
        f"cd {shlex.quote(root)} && env {env} PYTHONPATH=src "
        f"HT_IMAGE_DIGEST={shlex.quote(image)} {shlex.quote(python)} "
        f"experiments/gpu_network_v2/run.py out/{name} {backend}"
    )


def runtime_admission(profile: dict[str, Any], evidence: dict[str, Any]) -> int:
    """Exact-profile evidence required before CREATE, not a guessed toy throughput."""
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize

    bound = profile["execution_and_transfer_bound_seconds"]
    if (
        type(bound) is not int
        or not 0 < bound <= 40
        or profile["runtime_evidence_sha256"] != sha256_hex(canonicalize(evidence))
    ):
        raise Reject("missing_or_unqualified_exact_profile_runtime")
    if (
        evidence.get("profile_model") != profile["model"]
        or evidence.get("layout") != profile["layout"]
        or evidence.get("device") != "cuda"
        or evidence.get("torch") != "2.14.0+cu130"
        or evidence.get("full_round_transfer_bound_seconds") != bound
        or evidence.get("no_slower_hosts") is not True
    ):
        raise Reject("runtime_evidence_profile_mismatch")
    readiness_budget(profile, evidence)
    pin = profile["runtime"]
    if (
        evidence.get("image_digest") != pin["image_digest"]
        or evidence.get("driver_allowlist") != pin["driver_allowlist"]
        or evidence.get("python") != pin["remote_python"]
        or evidence.get("cuda") != pin["cuda"]
        or pin["backend"] != "cuda"
        or not pin["image_digest"]
        or not pin["qualified"]
        or not evidence.get("full_round_artifacts_sha256")
        or evidence.get("outer_tape_verified") is not True
    ):
        raise Reject("real_cuda_staged_reference_evidence_missing")
    s = profile["schedule"]
    if (
        s["executions_total"] != 126
        or s["executions_per_host"] != 63
        or s["shadow_executions"] != 96
        or s["live_executions"] != 16
        or s["probes"] != 2
        or s["fault_trace_reserve"] != 12
    ):
        raise Reject("execution_reservation_mismatch")
    total = (
        s["staging_seconds"] + 63 * bound + s["beacon_service_seconds"] + s["rescue_delete_seconds"]
    )
    if total > 3300 or total + s["hard_margin_seconds"] > 3600:
        raise Reject("one_hour_schedule_exceeded")
    return total


def admission_plan(
    profile: dict[str, Any],
    evidence: dict[str, Any],
    authorization: dict[str, Any],
    snapshot: dict[str, Any],
    offers: list[dict[str, Any]],
    *,
    qualification: bool = False,
) -> Record:
    """Consume authenticated snapshots/quotes recorded by gpu_ops; malformed is never absent."""
    if qualification:
        q = profile["microqualification"]
        bounds = artifact_budget(profile)
        if (
            q["max_full_rounds_per_host"] != 2
            or q["qualification_deadline_seconds"] != 900
            or sum(
                q[k]
                for k in (
                    "staging_seconds",
                    "probe_and_transfer_seconds",
                    "rescue_delete_seconds",
                    "failure_margin_seconds",
                )
            )
            > 900
            or evidence.get("root_verified_bounded_lifecycle") is not True
            or evidence.get("candidate_image_digest") != profile["runtime"]["image_digest"]
            or not profile["runtime"]["image_digest"]
            or evidence.get("registry_image_verified") is not True
            or not evidence.get("staged_contract_sha256")
            or not evidence.get("deadline_supervisor_contract_sha256")
            or bounds["disk_artifacts_peak_bytes"]
            + q["candidate_image_max_bytes"]
            + 2 * q["candidate_stage_max_bytes"]
            > profile["artifacts"]["disk_bytes"]
        ):
            raise Reject("bounded_microqualification_contract_missing")
        transfer_gb = q["candidate_transfer_gb_per_host"]
    else:
        runtime_admission(profile, evidence)
        transfer_gb = readiness_budget(profile, evidence)["priced_transfer_gb_per_host"]
    if (
        authorization.get("experiment") != "hypertrain-network-v2"
        or authorization.get("currency") != "USD"
        or authorization.get("ceiling_usd") != "50"
        or authorization.get("max_instances") != 2
        or authorization.get("max_seconds_per_instance") != 3600
        or not authorization.get("source_message")
        or not authorization.get("date")
    ):
        raise Reject("fresh_authorization_missing")
    if (
        not snapshot.get("authenticated")
        or snapshot.get("instances") != []
        or snapshot.get("volumes") != []
        or snapshot.get("uncapped_charges") is not False
        or type(snapshot["verified_unix"]) is not int
        or not 0 <= time.time() - snapshot["verified_unix"] <= 30
    ):
        raise Reject("inventory_liability_or_quote_unknown")
    if len(offers) != 2 or len({o["machine_id"] for o in offers}) != 2:
        raise Reject("two_distinct_hosts_required")
    for o in offers:
        hourly = budget.dec(o.get("dph_total"))
        if (
            o.get("gpu_name") != "RTX 5090"
            or o.get("num_gpus") != 2
            or hourly is None
            or hourly > 2
            or o.get("cpu_cores_effective", 0) < 8
            or o.get("cpu_ram", 0) < 32768
            or o.get("disk_space", 0) < 80
            or type(o["verified_unix"]) is not int
            or not 0 <= time.time() - o["verified_unix"] <= 30
        ):
            raise Reject("exact_host_shape_or_fresh_quote_missing")
    prior = Decimal(snapshot["prior_attempts_usd"])
    liability = Decimal(snapshot["active_liabilities_usd"])
    available = Decimal(snapshot["available_credit_usd"]) - liability
    if min(prior, liability, available) < 0:
        raise Reject("invalid_accounting")
    plan = budget.evaluate(
        snapshot0_credit=available + prior,
        current_credit=available,
        offers=offers,
        hard_deadline_seconds=3600,
        disk_gb=80,
        egress_gb=transfer_gb,
        phase_cap=Decimal("50"),
        reserve=Decimal("5"),
    )
    hardware = Decimal(plan["hardware_worst_case_usd"])
    hourly_total = sum(Decimal(str(o["dph_total"])) for o in offers)
    if not plan["admit"] or hardware > 5 or hardware - hourly_total > 1:
        raise Reject("network_v2_cost_ceiling_exceeded")
    return {
        **plan,
        "admission_mode": "qualification" if qualification else "workload",
        "rental_deadline_seconds": 3600,
        "qualification_deadline_seconds": 900,
        "long_workload_allowed": not qualification,
    }


class NetworkRuntime:
    """Reuse existing journal, trusted SSH, stage, rescue and independent supervisor receipts.

    A prepared job is concrete IslandJobV1 + objects in out/<name>; no fake API service.
    This runtime does not claim the L0 probation/ledger/relay integration already exists.
    """

    def __init__(self, lifecycle: Orchestrator, profile: dict[str, Any]) -> None:
        self.lifecycle, self.profile = lifecycle, profile
        self.journal: Journal = lifecycle.j

    def owned_host(self, role: str) -> Record:
        """Current admitted subject, never an advertised or synthetic instance."""
        receipt = self.lifecycle.receipt(role)
        if receipt is None or self.lifecycle.cleanup_started():
            raise Reject("owned-role actual owned role observation absent")
        return {
            "instance_id": receipt["instance_id"],
            "machine_id": self.lifecycle.host[role]["machine_id"],
            "image_digest": self.profile["runtime"]["image_digest"],
        }

    def reserve(self, role: str, name: str, phase: str, executions: int) -> Record:
        """Debit intent before execution. Failed/unfinished intents never refund or retry."""
        if (
            role not in self.lifecycle.roles
            or phase not in ("qualification", "workload")
            or (executions != 2 if phase == "qualification" else not 1 <= executions <= 12)
        ):
            raise Reject("invalid_execution_reservation")
        with flock(self.lifecycle.dir / "network-reservation.lock"):
            if self.lifecycle.cleanup_started():
                raise Reject("network_cleanup_already_started")
            if self.journal.last("network_execution_intent", role=role, name=name):
                raise Reject("execution_intent_not_retryable")
            intents = self.journal.all("network_execution_intent")
            q = sum(r["executions"] for r in intents if r["phase"] == "qualification")
            used_role = sum(r["executions"] for r in intents if r["role"] == role)
            if (
                used_role + executions > 63
                or sum(r["executions"] for r in intents) + executions > 126
            ):
                raise Reject("execution_reservation_exhausted")
            if phase == "qualification":
                if q + executions > 4 or any(
                    r["role"] == role and r["phase"] == "qualification" for r in intents
                ):
                    raise Reject("qualification_reservation_exhausted")
            elif q != 4 or self.journal.last("network_workload_promoted") is None:
                raise Reject("workload_not_promoted")
            return self.journal.append(
                "network_execution_intent",
                role=role,
                name=name,
                phase=phase,
                executions=executions,
            )

    def promote(self, evidence: dict[str, Any], budget_plan: Record) -> Record:
        """Promotion needs root-reviewed actual qualification, measured runtime and cost gates."""
        runtime_admission(self.profile, evidence)
        with flock(self.lifecycle.dir / "network-reservation.lock"):
            if self.lifecycle.cleanup_started():
                raise Reject("network_cleanup_already_started")
            intents = self.journal.all("network_execution_intent", phase="qualification")
            done = self.journal.all("network_qualification_done")
            if (
                len(intents) != 2
                or sum(r["executions"] for r in intents) != 4
                or {r["role"] for r in intents} != set(self.lifecycle.roles)
                or len(done) != 2
                or {r["role"] for r in done} != set(self.lifecycle.roles)
                or evidence.get("root_reviewed_two_host_full_round_transfer") is not True
                or budget_plan.get("admit") is not True
                or budget_plan.get("long_workload_allowed") is not True
                or time.time() + 61 * self.profile["execution_and_transfer_bound_seconds"] + 600
                > self.lifecycle.deadline()
            ):
                raise Reject("qualification_promotion_gate_failed")
            return self.journal.append(
                "network_workload_promoted",
                remaining_total=122,
                remaining_per_host=61,
                hard_deadline_unix=self.lifecycle.deadline(),
                budget_plan_sha256=sha256(
                    json.dumps(budget_plan, sort_keys=True, separators=(",", ":")).encode()
                ),
            )

    def record_qualification(self, role: str, name: str, result: Record) -> Record:
        """Root verified driver custody result; reservation already debited TWO full rounds."""
        intent = self.journal.last("network_execution_intent", role=role, name=name)
        if (
            intent is None
            or intent["phase"] != "qualification"
            or intent["executions"] != 2
            or result.get("passed") is not True
            or result.get("root_verified") is not True
            or not result.get("result_sha256")
            or not result.get("artifact_manifest_sha256")
        ):
            raise Reject("qualification_result_not_verified")
        with flock(self.lifecycle.dir / "network-reservation.lock"):
            old = self.journal.last("network_qualification_done", role=role, name=name)
            if old:
                if old["result"] != result:
                    raise Reject("qualification_result_changed_on_resume")
                return old
            return self.journal.append(
                "network_qualification_done",
                role=role,
                name=name,
                executions=2,
                result=dict(result),
            )

    def stage_context(
        self,
        role: str,
        paths: dict[str, Path],
        context: str,
        name: str,
        cutoff: float,
        cancel: threading.Event | None,
    ) -> None:
        """Private staging shares workload absolute deadline and cancellation boundary."""
        cutoff = min(
            cutoff,
            self.lifecycle.deadline(),
            self.lifecycle.cfg["cleanup_deadline_unix"] - 300,
        )
        ssh = deadline_ssh(self.lifecycle, role, cutoff)
        ssh.cancel = cancel
        root = self.lifecycle.remote_root(role)
        logs = self.lifecycle.dir / "logs"
        if ssh.run(
            f"mkdir -m 700 -p {shlex.quote(root + '/' + context)}",
            name + "-context",
            logs,
        ).returncode:
            raise Reject("continuation_private_context_stage_failed")
        for rel, path in paths.items():
            target = root + "/" + rel
            ssh.put(path, target)
            if ssh.remote_sha256(target, logs) != fsha(path):
                raise Reject("continuation_private_context_transport_changed")
            if ssh.run(f"chmod 600 {shlex.quote(target)}", name + "-mode", logs).returncode:
                raise Reject("continuation_private_context_mode_failed")

    def operation(
        self,
        spec: Any,
        directory: Path,
        *,
        cancel: threading.Event | None = None,
    ) -> Any:
        """Internal accepted descriptor; debit, strict SSH, custody then validation."""
        from hypertrain.miner.island_launch import validate_artifacts
        from hypertrain.protocol.envelope_v2 import parse_envelope, verify_envelope
        from hypertrain.protocol.keys import decode_hotkey

        script = Path(__file__).resolve().parents[2] / "scripts/network_gpu_operation.py"
        engine_spec = importlib.util.spec_from_file_location("network_operation_contract", script)
        assert engine_spec is not None and engine_spec.loader is not None
        engine = importlib.util.module_from_spec(engine_spec)
        sys.modules[engine_spec.name] = engine
        engine_spec.loader.exec_module(engine)
        spec = engine.Operation.model_validate(spec.model_dump(mode="json"))
        # Remote script is an operator support file: explicitly add it to the approved source map.
        decode_hotkey(spec.hotkey)
        role, job = spec.role, spec.job
        cutoff = min(
            spec.cutoff,
            job.deadline,
            self.lifecycle.deadline(),
            self.lifecycle.cfg["cleanup_deadline_unix"] - 300,
        )
        if (
            spec.cutoff != cutoff
            or cutoff <= time.time()
            or cancel is not None
            and cancel.is_set()
            or self.lifecycle.cleanup_started()
            or not self.profile["runtime"]["qualified"]
            or spec.image_digest != self.profile["runtime"]["image_digest"]
            or job.manifest.training.reference_spec.image_digest != spec.image_digest
            or job.manifest.training.reference_spec.layout.model_dump() != self.profile["layout"]
            or self.journal.last("supervisor_ready") is None
            or self.lifecycle.receipt(role) is None
            or self.journal.last("ssh_trusted", role=role) is None
        ):
            raise Reject("operation_context_not_qualified")
        attested = self.journal.last("network_operation_source", role=role)
        map_hash = sha256(json.dumps(spec.sources, sort_keys=True).encode())
        if attested is None or attested["sources_sha256"] != map_hash:
            raise Reject("operation_source_authority_missing")
        if attested["run_id"] != job.run_id or spec.sources.get(
            "scripts/network_gpu_operation.py"
        ) != fsha(script):
            raise Reject("operation_source_or_run_changed")
        from hypertrain.gpu_ops.network_qualification import sources as actual_sources

        tree = Path(__file__).resolve().parents[2]
        actual = actual_sources(tree)
        # Profile is a root-reviewed effective overlay; its hash stays explicitly in spec.
        profile_key = "experiments/gpu_network_v2/profile.json"
        actual["scripts/network_gpu_operation.py"] = fsha(script)
        if set(actual) != set(spec.sources) or any(
            sha != spec.sources[p] for p, sha in actual.items() if p != profile_key
        ):
            raise Reject("operation_actual_source_changed")
        operation_authority = self.journal.last(
            "network_operation_authority",
            role=role,
            binding=spec.binding,
            operation=spec.operation,
            hotkey=spec.hotkey,
            w=job.w,
        )
        if operation_authority is None or operation_authority["spec_sha256"] != sha256(
            spec.model_dump_json().encode()
        ):
            raise Reject("operation_accepted_authority_missing")
        if operation_authority["owner"] != spec.owner:
            raise Reject("operation_owner_changed")
        receipt = self.lifecycle.receipt(role)
        assert receipt is not None
        if (
            receipt["instance_id"] != spec.instance_id
            or self.lifecycle.host[role]["machine_id"] != spec.machine_id
        ):
            raise Reject("operation_accepted_host_changed")
        name = "op-" + sha256(
            (job.run_id + spec.operation + spec.hotkey + spec.binding + job.digest()).encode()
        )
        root = self.lifecycle.remote_root(role)
        remote = root + "/out/" + name
        ssh = deadline_ssh(self.lifecycle, role, cutoff)
        ssh.cancel = cancel
        trusted = self.journal.last("ssh_trusted", role=role)
        assert trusted is not None
        if fsha(ssh.known_hosts) != trusted["known_hosts_sha256"]:
            raise Reject("operation_known_hosts_custody_changed")
        self.reserve(role, name, "workload", 1)
        self.journal.append(
            "network_operation_started",
            role=role,
            name=name,
            binding=spec.binding,
            job_sha256=job.digest(),
            operation=spec.operation,
            hotkey=spec.hotkey,
            sources_sha256=map_hash,
            cutoff=cutoff,
        )
        error: Exception | None = None
        custody: Record | None = None
        try:
            if ssh.run(
                f"mkdir -p {shlex.quote(remote)}",
                name + "-mkdir",
                self.lifecycle.dir / "logs",
            ).returncode:
                raise Reject("operation_stage_failed")
            paths = {rel: directory / rel for rel in job.object_paths.values()}
            local_spec = self.lifecycle.dir / (name + ".json")
            durable_write(local_spec, spec.model_dump_json().encode())
            paths["operation.json"] = local_spec
            for rel, path in paths.items():
                if Path(rel).is_absolute() or ".." in Path(rel).parts or path.is_symlink():
                    raise Reject("operation_input_path")
                target = remote + "/" + rel
                if ssh.run(
                    f"mkdir -p {shlex.quote(str(Path(target).parent))}",
                    name + "-parents",
                    self.lifecycle.dir / "logs",
                ).returncode:
                    raise Reject("operation_stage_failed")
                ssh.put(path, target)
                if ssh.remote_sha256(target, self.lifecycle.dir / "logs") != fsha(path):
                    raise Reject("operation_transport_changed")
            prefix = staged_command(
                self.profile,
                root,
                name,
                self.lifecycle.cfg["remote_python"],
                spec.image_digest,
                "cuda",
            )
            command = prefix.split(" experiments/gpu_network_v2/run.py", 1)[0] + (
                f" scripts/network_gpu_operation.py {shlex.quote(root)} {shlex.quote(remote)} "
                f"{shlex.quote(remote + '/operation.json')}"
            )
            done = threading.Event()
            condition = vars(cancel)["_cond"] if cancel is not None else None

            def stop() -> None:
                if condition is None:
                    return
                with condition:
                    condition.wait_for(
                        lambda: done.is_set() or cancel is not None and cancel.is_set(),
                        timeout=max(0, cutoff - time.time()),
                    )
                if not done.is_set():
                    # SSH disconnect alone cannot kill grandchildren; explicit same-path PID group.
                    killer = self.lifecycle.ssh(role)
                    killer.timeout = int(min(10, cutoff - time.time()))
                    if killer.timeout > 0:
                        killer.run(
                            f"touch {shlex.quote(remote)}/operation-cancelled; "
                            f"test ! -f {shlex.quote(remote)}/operation-pid || "
                            f"kill -TERM -- -$(cat {shlex.quote(remote)}/operation-pid)",
                            name + "-cancel",
                            self.lifecycle.dir / "logs",
                        )

            watcher = threading.Thread(target=stop)
            watcher.start()
            try:
                proc = ssh.run(command, name, self.lifecycle.dir / "logs")
            finally:
                done.set()
                if condition is not None:
                    with condition:
                        condition.notify_all()
                watcher.join()
            if proc.returncode or cancel is not None and cancel.is_set():
                raise Reject("operation_failed_or_cancelled")
        except (Reject, OSError, subprocess.SubprocessError, ValueError) as exc:
            error = exc
            self.journal.append("network_operation_failed", role=role, name=name, error=str(exc))
        finally:
            try:
                custody = self.rescue(role, fresh=True)
            except (
                Reject,
                OSError,
                subprocess.SubprocessError,
                ValueError,
            ) as rescue_error:
                self.journal.append(
                    "network_operation_censored",
                    role=role,
                    name=name,
                    error=str(rescue_error),
                )
                raise
        if error is not None:
            raise error
        assert custody is not None
        archive = Path(custody["tar"])
        if fsha(archive) != custody["tar_sha256"]:
            raise Reject("operation_custody_changed")
        destination = directory / name
        if destination.exists():
            raise Reject("operation_result_destination_exists")
        with tarfile.open(archive) as source:
            for member in source.getmembers():
                prefix = "out/" + name + "/"
                if member.name.startswith(prefix) and member.isfile():
                    rel = member.name.removeprefix(prefix)
                    path = destination / rel
                    if Path(rel).is_absolute() or ".." in Path(rel).parts:
                        raise Reject("operation_custody_path")
                    stream = source.extractfile(member)
                    assert stream is not None
                    path.parent.mkdir(parents=True, exist_ok=True)
                    durable_write(path, stream.read())
        result = json.loads((destination / "operation-result.json").read_bytes())
        if (
            result.get("status") != "CAPTURED_NOT_ACCEPTED"
            or result.get("binding") != spec.binding
            or result.get("job_sha256") != job.digest()
            or result.get("hotkey") != spec.hotkey
            or result.get("operation") != spec.operation
            or result.get("role") != role
        ):
            raise Reject("operation_original_result_binding")
        rel = Path(result["publication"])
        if rel.is_absolute() or ".." in rel.parts:
            raise Reject("operation_publication_path")
        publication = destination / rel
        artifacts = validate_artifacts(job, publication)
        summaries = [
            json.loads((publication / f"rank-{rank}/summary.json").read_bytes())
            for rank in range(job.manifest.training.reference_spec.layout.n_gpus)
        ]
        if any(s["backend"] != "cuda" for s in summaries):
            raise Reject("CPU_publication_cannot_serve_as_CUDA")
        if spec.trace and any(
            not (publication / f"rank-{r}/trace.json").is_file() for r in range(len(summaries))
        ):
            raise Reject("operation_trace_missing")
        for raw in result.get("signed_api", []):
            env = parse_envelope(raw)
            if not verify_envelope(raw) or env.run_id != job.run_id or env.signer != spec.hotkey:
                raise Reject("operation_signed_API_outcome_changed")
        if spec.operation == "live":
            signed = result.get("signed_api", [])
            for row in result.get("accepted_api", []):
                if row["request"] not in signed:
                    raise Reject("operation_API_receipt_without_original_signature")
            if any(
                raw["body"].get("w") != job.w
                for raw in signed
                if raw["type"] in ("AcceptV2", "CommitV2", "DeltaManifestV2")
            ):
                raise Reject("operation_live_API_round_changed")
        if spec.operation == "live" and not {
            "AcceptV2",
            "CommitV2",
            "DeltaManifestV2",
        } <= {row["request"]["type"] for row in result.get("accepted_api", [])}:
            raise Reject("operation_live_API_acceptance_missing")
        if spec.operation == "probe" and not {"WorkScreenV2", "WorkProof"} <= {
            row["type"] for row in result.get("signed_api", [])
        }:
            raise Reject("operation_probe_signed_outcomes_missing")
        if time.time() >= cutoff or cancel is not None and cancel.is_set():
            raise Reject("operation_expired_before_acceptance")
        self.journal.append(
            "network_operation_done",
            role=role,
            name=name,
            result_sha256=fsha(destination / "operation-result.json"),
            tar_sha256=custody["tar_sha256"],
        )
        return artifacts if spec.operation in ("reference", "audit", "referee") else result

    def completion_custody(
        self,
        spec: Any,
        directory: Path,
        *,
        tree: Path,
        cancel: threading.Event | None = None,
    ) -> Record:
        """Authenticate completed trial custody internally; never store/payment acceptance."""
        from hypertrain.gpu_ops.network_qualification import sources
        from hypertrain.miner.island_launch import confined, validate_artifacts
        from hypertrain.protocol.envelope_v2 import Intake, load_json, parse_envelope
        from hypertrain.protocol.jcs import canonicalize
        from hypertrain.protocol.messages_v2 import CommitV2, JoinChallenge, WorkProof

        cutoff = min(
            spec.cutoff,
            spec.job.deadline,
            self.lifecycle.deadline(),
            self.lifecycle.cfg["cleanup_deadline_unix"] - 300,
        )
        if (
            spec.operation not in ("reference", "probe")
            or spec.binding_kind != "trial"
            or spec.backend != "cuda"
            or spec.cutoff != cutoff
            or time.time() >= cutoff
            or cancel is not None
            and cancel.is_set()
            or self.lifecycle.cleanup_started()
            or not self.profile["runtime"]["qualified"]
            or self.journal.last("supervisor_ready") is None
            or self.journal.last("ssh_trusted", role=spec.role) is None
        ):
            raise Reject("completion_context_or_deadline")
        host = self.owned_host(spec.role)
        if host != {
            "instance_id": spec.instance_id,
            "machine_id": spec.machine_id,
            "image_digest": spec.image_digest,
        }:
            raise Reject("completion_owned_host_changed")
        if (
            spec.image_digest != spec.job.manifest.training.reference_spec.image_digest
            or spec.job.manifest.training.reference_spec.layout.model_dump()
            != self.profile["layout"]
        ):
            raise Reject("completion_layout_or_image_changed")
        actual = sources(tree)
        actual["scripts/network_gpu_operation.py"] = fsha(tree / "scripts/network_gpu_operation.py")
        actual["experiments/gpu_network_v2/orchestrate.py"] = fsha(Path(__file__))
        if actual != spec.sources:
            raise Reject("completion_actual_source_changed")
        name = "op-" + sha256(
            (
                spec.job.run_id + spec.operation + spec.hotkey + spec.binding + spec.job.digest()
            ).encode()
        )
        authority = self.journal.last(
            "network_operation_authority",
            role=spec.role,
            binding=spec.binding,
            operation=spec.operation,
            hotkey=spec.hotkey,
            w=spec.job.w,
        )
        if authority is None or authority["owner"] != spec.owner:
            raise Reject("completion_owner_authority_missing")
        # Original Receipt intake also checks admitted owner and exact immutable spec.
        receipt = authority["receipt"]
        self.accept_operation(
            spec,
            canonicalize(receipt),
            owner=spec.owner,
            beacon=receipt["body"]["received_round"],
        )
        attested = self.journal.last("network_operation_source", role=spec.role)
        map_hash = sha256(json.dumps(spec.sources, sort_keys=True).encode())
        if attested is None or (attested["run_id"], attested["sources_sha256"]) != (
            spec.job.run_id,
            map_hash,
        ):
            raise Reject("completion_source_authority_changed")
        started = self.journal.last("network_operation_started", role=spec.role, name=name)
        done = self.journal.last("network_operation_done", role=spec.role, name=name)
        intent = self.journal.last("network_execution_intent", role=spec.role, name=name)
        if (
            started is None
            or done is None
            or intent is None
            or intent["phase"] != "workload"
            or intent["executions"] != 1
            or any(
                started[k] != v
                for k, v in {
                    "binding": spec.binding,
                    "job_sha256": spec.job.digest(),
                    "operation": spec.operation,
                    "hotkey": spec.hotkey,
                    "sources_sha256": map_hash,
                    "cutoff": cutoff,
                }.items()
            )
            or not authority["unix"] <= started["unix"] <= done["unix"] <= cutoff
        ):
            raise Reject("completion_original_done_missing")
        directory = Path(directory)
        original = directory / name
        if original.is_symlink() or original.resolve().parent != directory.resolve():
            raise Reject("completion_custody_path_changed")
        result_path = confined(original, "operation-result.json")
        if fsha(result_path) != done["result_sha256"]:
            raise Reject("completion_result_changed")
        result = load_json(result_path.read_bytes())
        publication_name = result["publication"]
        if not isinstance(publication_name, str):
            raise Reject("completion_publication_path_changed")
        publication = original / publication_name
        if (
            publication.is_symlink()
            or not publication.resolve().is_relative_to(original.resolve())
            or not publication.is_dir()
        ):
            raise Reject("completion_publication_path_changed")
        artifacts = validate_artifacts(spec.job, publication)
        custody_path = confined(original, "publication-custody.json")
        custody = load_json(custody_path.read_bytes())
        body = custody["body"]
        if not isinstance(body, dict):
            raise Reject("completion_custody_binding_changed")
        required_environment = spec.job.manifest.training.reference_spec.env.model_dump(mode="json")
        if (
            custody["status"] != "CAPTURED_NOT_ACCEPTED"
            or custody["sha256"] != sha256(canonicalize(body))
            or result.get("status") != "CAPTURED_NOT_ACCEPTED"
            or result.get("publication_custody_sha256") != fsha(custody_path)
            or any(
                body[k] != v
                for k, v in {
                    "operation_sha256": sha256(spec.model_dump_json().encode()),
                    "operation": spec.operation,
                    "binding": spec.binding,
                    "hotkey": spec.hotkey,
                    "owner": spec.owner,
                    "role": spec.role,
                    "instance_id": spec.instance_id,
                    "machine_id": spec.machine_id,
                    "image_digest": spec.image_digest,
                    "backend": "cuda",
                    "run_id": spec.job.run_id,
                    "job": spec.job.body(),
                    "job_sha256": spec.job.digest(),
                    "sources": spec.sources,
                    "manifest_sha256": spec.job.manifest.digest(),
                    "sample_ids_sha256": sha256(canonicalize(spec.job.sample_ids)),
                    "required_environment": required_environment,
                    "layout": self.profile["layout"],
                    "publication": result["publication"],
                }.items()
            )
            or any(
                result[k] != body[k]
                for k in ("operation", "binding", "hotkey", "role", "job_sha256")
            )
        ):
            raise Reject("completion_custody_binding_changed")
        files = {}
        for path in publication.rglob("*"):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise Reject("completion_publication_special_file")
            if path.is_file():
                files[str(path.relative_to(publication))] = {
                    "sha256": fsha(path),
                    "bytes": path.stat().st_size,
                }
        if files != body["files"] or sha256(canonicalize(files)) != body["publication_sha256"]:
            raise Reject("completion_publication_changed")
        summaries = [
            load_json(confined(publication, f"rank-{r}/summary.json").read_bytes())
            for r in range(spec.job.manifest.training.reference_spec.layout.n_gpus)
        ]
        if summaries != body["rank_summaries"] or any(s["backend"] != "cuda" for s in summaries):
            raise Reject("completion_rank_backend_changed")
        if spec.trace and any(f"rank-{r}/trace.json" not in files for r in range(len(summaries))):
            raise Reject("completion_trace_missing")
        trial = body["trial_authority"]
        if not isinstance(trial, dict):
            raise Reject("completion_original_challenge_changed")
        trial_envelope, seed_beacon = trial["envelope"], trial["seed_beacon"]
        if not isinstance(trial_envelope, dict) or type(seed_beacon) is not int:
            raise Reject("completion_original_challenge_changed")
        challenge = Intake(
            spec.job.run_id,
            {"JoinChallenge": spec.job.manifest.training.coord_pubkey.__eq__},
        ).accept(trial_envelope, seed_beacon)
        from hypertrain.auditor.replay import unpack_state
        from hypertrain.data.trial_assignment import trial_assignment_hash
        from hypertrain.trainer.compress import state_hash

        if (
            not isinstance(challenge, JoinChallenge)
            or challenge.admission_id != spec.binding
            or challenge.manifest_hash != spec.job.run_id
            or challenge.layout != spec.job.manifest.training.reference_spec.layout
            or challenge.assignment_hash
            != trial_assignment_hash(spec.job.manifest, spec.job.w, tuple(spec.job.sample_ids))
            or challenge.theta_hash
            != state_hash(
                unpack_state(
                    confined(publication, spec.job.object_paths["start_state"]).read_bytes()
                )[0]
            )
            or trial["sha256"] != sha256(canonicalize(trial["envelope"]))
            or trial["challenge_hash"] != challenge.digest()
            or trial["nonce"] != challenge.nonce
            or trial["seed_beacon"] != challenge.seed_beacon
            or trial["assignment_hash"] != challenge.assignment_hash
            or parse_envelope(trial_envelope).exp_drand < challenge.deadline_beacon
            or spec.challenge is None
            or spec.context_files.get(spec.challenge) != trial["sha256"]
        ):
            raise Reject("completion_original_challenge_changed")
        signed_cutoff = (
            spec.job.manifest.training.beacon.genesis_time + (challenge.deadline_beacon - 1) * 3
        )
        if (
            spec.job.deadline != signed_cutoff
            or cutoff > signed_cutoff
            or not authority["unix"] <= started["unix"] <= done["unix"] <= signed_cutoff
        ):
            raise Reject("completion_original_signed_deadline_changed")
        if spec.operation == "probe":
            commit_env, proof_env = body["miner_commit"], body["miner_proof"]
            if not isinstance(commit_env, dict) or not isinstance(proof_env, dict):
                raise Reject("completion_original_miner_signature_changed")
            commit_envelope, proof_envelope = (
                commit_env["envelope"],
                proof_env["envelope"],
            )
            if not isinstance(commit_envelope, dict) or not isinstance(proof_envelope, dict):
                raise Reject("completion_original_miner_signature_changed")
            intake = Intake(
                spec.job.run_id,
                {"CommitV2": spec.hotkey.__eq__, "WorkProof": spec.hotkey.__eq__},
            )
            commit = intake.accept(commit_envelope, challenge.seed_beacon)
            proof = intake.accept(proof_envelope, challenge.seed_beacon)
            from hypertrain.protocol.hashing import MerkleTree
            from hypertrain.protocol.messages import LeafPreimage

            leaves = [
                LeafPreimage.model_validate(p) for p in json.loads(artifacts.leaves.read_bytes())
            ]
            summary = summaries[0]["commitments"]
            if not isinstance(summary, dict):
                raise Reject("completion_original_miner_work_changed")
            if (
                not isinstance(commit, CommitV2)
                or not isinstance(proof, WorkProof)
                or (commit.w, commit.hotkey) != (spec.job.w, spec.hotkey)
                or commit.n_leaves != len(leaves)
                or commit.metrics_root
                != MerkleTree(
                    [bytes.fromhex(p.loss_f32) + bytes.fromhex(p.norm_f32) for p in leaves]
                ).root.hex()
                or commit.ef_in_hash
                != state_hash(
                    unpack_state(
                        confined(publication, spec.job.object_paths["ef_in"]).read_bytes()
                    )[0]
                )
                or (proof.admission_id, proof.challenge_hash) != (spec.binding, challenge.digest())
                or any(
                    getattr(commit, k) != summary[k]
                    for k in (
                        "leaves_root",
                        "final_theta_hash",
                        "ef_out_hash",
                        "delta_hash",
                    )
                )
                or proof.leaves_root != commit.leaves_root
                or proof.delta_hash != fsha(artifacts.delta)
                or commit.delta_bytes != artifacts.delta.stat().st_size
                or result.get("miner_commit") != commit_env
            ):
                raise Reject("completion_original_miner_work_changed")
            commit.validate_assignment(spec.job.manifest, len(spec.job.sample_ids))
            for signed, envelope in (
                (commit_env, commit_envelope),
                (proof_env, proof_envelope),
            ):
                if (
                    signed["sha256"] != sha256(canonicalize(envelope))
                    or parse_envelope(envelope).exp_drand < challenge.deadline_beacon
                ):
                    raise Reject("completion_original_miner_signature_changed")
            for ref, path in zip(
                proof.artifact_refs,
                (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves),
                strict=True,
            ):
                if (ref.sha256, ref.size) != (fsha(path), path.stat().st_size):
                    raise Reject("completion_original_proof_object_changed")
        elif body["miner_commit"] is not None:
            raise Reject("completion_reference_claims_miner")
        execution_path = confined(original, "execution-custody.json")
        execution = load_json(execution_path.read_bytes())
        environment = body["environment"]
        if not isinstance(environment, dict):
            raise Reject("completion_execution_custody_changed")
        expected = {
            k: body[k]
            for k in (
                "operation_sha256",
                "job_sha256",
                "binding",
                "operation",
                "instance_id",
                "machine_id",
            )
        }
        expected.update(
            publication_custody_sha256=fsha(custody_path),
            operation_result_sha256=fsha(result_path),
            environment_sha256=sha256(canonicalize(result["environment"])),
            status="CAPTURED_NOT_ACCEPTED",
        )
        if (
            execution != expected
            or result["environment"] != body["environment"]
            or environment.get("backend") != "cuda"
            or environment.get("image_digest") != spec.image_digest
        ):
            raise Reject("completion_execution_custody_changed")
        rescue = next(
            (
                r
                for r in self.journal.all("network_rescued")
                if r["role"] == spec.role and r["tar_sha256"] == done["tar_sha256"]
            ),
            None,
        )
        if rescue is None or fsha(rescue["tar"]) != done["tar_sha256"]:
            raise Reject("completion_original_rescue_changed")
        prefix = "out/" + name + "/"
        with tarfile.open(rescue["tar"]) as archive:
            rescued = {}
            for member in archive.getmembers():
                if member.name.startswith(prefix) and member.isfile():
                    rel = member.name.removeprefix(prefix)
                    if rel in rescued:
                        raise Reject("completion_duplicate_rescue_member")
                    stream = archive.extractfile(member)
                    assert stream is not None
                    rescued[rel] = sha256(stream.read())
        expected_rescue = {
            publication_name + "/" + rel: meta["sha256"] for rel, meta in files.items()
        }
        expected_rescue.update(
            {p.name: fsha(p) for p in (result_path, custody_path, execution_path)}
        )
        if any(rescued.get(rel) != digest for rel, digest in expected_rescue.items()):
            raise Reject("completion_original_rescue_bytes_changed")
        # Existing canonical reader handles confinement, rescue and duplicate publication.
        if spec.operation == "reference":
            artifacts = self.launch_adapter(spec)(
                spec.job, directory, backend="cuda", cancel=cancel, trace=spec.trace
            )
        if (
            time.time() >= cutoff
            or cancel is not None
            and cancel.is_set()
            or self.lifecycle.cleanup_started()
        ):
            raise Reject("completion_expired_before_descriptor")
        descriptor = {
            "status": "NOTSTOREACCEPTED",
            "production_backend_acceptance": False,
            "economic_acceptance": False,
            "spec": spec.model_dump(mode="json"),
            "owner_receipt": receipt,
            "done": done,
            "directory": str(artifacts.directory),
            "publication": body,
            "result_sha256": fsha(result_path),
            "execution_custody_sha256": fsha(execution_path),
            "rescue_sha256": done["tar_sha256"],
        }
        return {"body": descriptor, "sha256": sha256(canonicalize(descriptor))}

    def accept_operation(self, spec: Any, raw: bytes, *, owner: str, beacon: int) -> None:
        """Trusted root Receipt intake; exact operation, no public route."""
        from hypertrain.protocol.envelope_v2 import Intake, parse_envelope
        from hypertrain.protocol.jcs import canonicalize
        from hypertrain.protocol.messages import Receipt

        receipt = Intake(spec.job.run_id, {"Receipt": lambda h: h == owner}).accept(raw, beacon)
        env = parse_envelope(raw)
        context = {
            "role": spec.role,
            "binding": spec.binding,
            "operation": spec.operation,
            "hotkey": spec.hotkey,
            "spec_sha256": sha256(spec.model_dump_json().encode()),
        }
        if not isinstance(receipt, Receipt) or receipt.commit_hash != sha256(canonicalize(context)):
            raise Reject("operation_owner_receipt_changed")
        current = self.journal.last("network_admission_authority")
        if current is None or current["owner"] != owner or spec.owner != owner:
            raise Reject("operation_owner_not_admitted")
        old = self.journal.last(
            "network_operation_authority",
            role=spec.role,
            binding=spec.binding,
            operation=spec.operation,
            hotkey=spec.hotkey,
            w=spec.job.w,
        )
        if old is not None:
            if old["spec_sha256"] != context["spec_sha256"]:
                raise Reject("operation_accepted_context_immutable")
            return
        self.journal.append(
            "network_operation_source",
            role=spec.role,
            run_id=spec.job.run_id,
            sources_sha256=sha256(json.dumps(spec.sources, sort_keys=True).encode()),
        )
        self.journal.append(
            "network_operation_authority",
            **context,
            w=spec.job.w,
            owner=env.signer,
            receipt=env.model_dump(mode="json"),
        )

    def launch_adapter(self, spec: Any) -> Any:
        """Bind trusted accepted trial/lease context once; preserve canonical launch signature."""

        def launch(
            job: Any,
            directory: Path,
            *,
            backend: str,
            cancel: threading.Event | None = None,
            trace: bool = False,
        ) -> Any:
            if (
                job != spec.job
                or backend != "cuda"
                or trace != spec.trace
                or spec.operation not in ("reference", "audit", "referee")
            ):
                raise Reject("operation_accepted_descriptor_changed")
            import tempfile

            from hypertrain.miner.island_launch import validate_artifacts

            name = "op-" + sha256(
                (job.run_id + spec.operation + spec.hotkey + spec.binding + job.digest()).encode()
            )
            directory = Path(directory)
            with flock(directory / "canonical-publication.lock"):
                if (
                    time.time() >= min(spec.cutoff, job.deadline)
                    or cancel is not None
                    and cancel.is_set()
                    or self.lifecycle.cleanup_started()
                ):
                    raise Reject("operation_expired_before_canonical_publication")
                custody = directory / name
                canonical_done = self.journal.last(
                    "network_canonical_done", role=spec.role, name=name
                )
                if (
                    self.journal.last("network_canonical_intent", role=spec.role, name=name)
                    is not None
                    and canonical_done is None
                ):
                    raise Reject("operation_canonical_failed_not_retryable")
                if canonical_done is None:
                    self.journal.append("network_canonical_intent", role=spec.role, name=name)
                done = self.journal.last("network_operation_done", role=spec.role, name=name)
                if done is None:
                    artifacts = self.operation(spec, directory, cancel=cancel)
                else:
                    authority = self.journal.last(
                        "network_operation_authority",
                        role=spec.role,
                        binding=spec.binding,
                        operation=spec.operation,
                        hotkey=spec.hotkey,
                        w=job.w,
                    )
                    result = custody / "operation-result.json"
                    if (
                        authority is None
                        or authority["spec_sha256"] != sha256(spec.model_dump_json().encode())
                        or result.is_symlink()
                        or fsha(result) != done["result_sha256"]
                    ):
                        raise Reject("operation_canonical_duplicate_authority_changed")
                    receipt = self.lifecycle.receipt(spec.role)
                    if (
                        receipt is None
                        or receipt["instance_id"] != spec.instance_id
                        or self.lifecycle.host[spec.role]["machine_id"] != spec.machine_id
                    ):
                        raise Reject("operation_accepted_host_changed")
                    publication_relative = Path(json.loads(result.read_bytes())["publication"])
                    if publication_relative.is_absolute() or ".." in publication_relative.parts:
                        raise Reject("operation_publication_path")
                    artifacts = validate_artifacts(job, custody / publication_relative)
                original = artifacts.directory
                if (
                    custody.is_symlink()
                    or custody.resolve().parent != directory.resolve()
                    or original.is_symlink()
                    or not original.resolve().is_relative_to(custody.resolve())
                ):
                    raise Reject("operation_canonical_custody_path")
                files = {}
                for path in original.rglob("*"):
                    if path.is_symlink() or not (path.is_file() or path.is_dir()):
                        raise Reject("operation_canonical_symlink_or_special")
                    if path.is_file():
                        files[str(path.relative_to(original))] = fsha(path)
                if any(
                    json.loads((original / f"rank-{rank}/summary.json").read_bytes())["backend"]
                    != "cuda"
                    or spec.trace
                    and f"rank-{rank}/trace.json" not in files
                    for rank in range(job.manifest.training.reference_spec.layout.n_gpus)
                ):
                    raise Reject("operation_canonical_rank_backend_or_trace_changed")
                # Exact original rescued bytes also bind an idempotent local return.
                done = self.journal.last("network_operation_done", role=spec.role, name=name)
                assert done is not None
                rescue = next(
                    (
                        row
                        for row in self.journal.all("network_rescued")
                        if row["role"] == spec.role and row["tar_sha256"] == done["tar_sha256"]
                    ),
                    None,
                )
                if rescue is None or fsha(Path(rescue["tar"])) != done["tar_sha256"]:
                    raise Reject("operation_canonical_rescue_changed")
                prefix = "out/" + name + "/" + str(original.relative_to(custody)) + "/"
                with tarfile.open(rescue["tar"]) as archive:
                    rescued = {}
                    for member in archive.getmembers():
                        if member.name.startswith(prefix) and member.isfile():
                            stream = archive.extractfile(member)
                            assert stream is not None
                            rescued[member.name.removeprefix(prefix)] = sha256(stream.read())
                    if rescued != files:
                        raise Reject("operation_canonical_rescue_changed")
                canonical = directory / "published"
                with tempfile.TemporaryDirectory(prefix="canonical-", dir=directory) as temporary:
                    staged = Path(temporary) / "published"
                    staged.mkdir()
                    for relative, digest in files.items():
                        path = staged / relative
                        path.parent.mkdir(parents=True, exist_ok=True)
                        durable_write(path, (original / relative).read_bytes())
                        if fsha(path) != digest:
                            raise Reject("operation_canonical_copy_changed")
                    validate_artifacts(job, staged)
                    for path in sorted(staged.rglob("*"), key=lambda p: len(p.parts), reverse=True):
                        if path.is_dir():
                            fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
                            try:
                                os.fsync(fd)
                            finally:
                                os.close(fd)
                    fd = os.open(staged, os.O_RDONLY | os.O_DIRECTORY)
                    try:
                        os.fsync(fd)
                    finally:
                        os.close(fd)
                    if canonical.exists() or canonical.is_symlink():
                        if canonical.is_symlink() or not canonical.is_dir():
                            raise Reject("operation_canonical_collision")
                        existing = {}
                        for path in canonical.rglob("*"):
                            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                                raise Reject("operation_canonical_collision")
                            if path.is_file():
                                existing[str(path.relative_to(canonical))] = fsha(path)
                        if existing != files:
                            raise Reject("operation_canonical_collision")
                    if (
                        time.time() >= min(spec.cutoff, job.deadline)
                        or cancel is not None
                        and cancel.is_set()
                        or self.lifecycle.cleanup_started()
                    ):
                        raise Reject("operation_expired_before_canonical_publication")
                    if not canonical.exists():
                        os.rename(staged, canonical)
                        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
                        try:
                            os.fsync(fd)
                        finally:
                            os.close(fd)
                    validated = validate_artifacts(job, canonical)
                    if (
                        time.time() >= min(spec.cutoff, job.deadline)
                        or cancel is not None
                        and cancel.is_set()
                    ):
                        raise Reject("operation_expired_before_canonical_result")
                    if canonical_done is None:
                        self.journal.append(
                            "network_canonical_done",
                            role=spec.role,
                            name=name,
                            directory=str(canonical),
                        )
                    return validated

        return launch

    def run_qualification(
        self, role: str, name: str, config_path: str, *, cutoff: float | None = None
    ) -> Record:
        """Launch frozen driver once; TWO executions charged before any SSH command."""
        if (
            not config_path.replace("-", "")
            .replace("_", "")
            .replace("/", "")
            .replace(".", "")
            .isalnum()
        ):
            raise Reject("invalid_qualification_config_path")
        if Path(config_path).is_absolute() or ".." in Path(config_path).parts:
            raise Reject("qualification_config_outside_custody")
        if (
            self.lifecycle.cfg.get("cleanup_contract") != "network-v2"
            or self.journal.last("supervisor_ready") is None
            or self.lifecycle.receipt(role) is None
            or self.journal.last("staged", role=role) is None
            or self.lifecycle.cleanup_started()
        ):
            raise Reject("qualification_lifecycle_not_ready")
        root = self.lifecycle.remote_root(role)
        python = self.lifecycle.cfg.get("remote_python", self.profile["runtime"]["remote_python"])
        image = self.lifecycle.cfg["image"].rsplit("@", 1)[-1]
        self.reserve(role, name, "qualification", 2)
        ssh = (
            self.lifecycle.ssh(role)
            if cutoff is None
            else deadline_ssh(self.lifecycle, role, cutoff)
        )
        intents = self.journal.all("create_intent")
        first_billable = min(float(r["unix"]) for r in intents) if intents else None
        if first_billable is None:
            raise Reject("qualification_first_billable_receipt_missing")
        ssh.timeout = int(min(30, first_billable + 900 - time.time() - 420))
        if ssh.timeout <= 0:
            raise Reject("qualification_deadline_margin")
        config_file = self.lifecycle.dir / f"qualification-config-{role}.json"
        ssh.get(f"{root}/{config_path}", config_file)
        spec = importlib.util.spec_from_file_location(
            "network_driver_contract",
            Path(__file__).resolve().parents[2] / "scripts/network_gpu_qualification.py",
        )
        if spec is None or spec.loader is None:
            raise Reject("qualification_driver_missing")
        driver = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = driver
        spec.loader.exec_module(driver)
        cfg = driver.Qualification.model_validate(json.loads(config_file.read_bytes()))
        receipt = self.lifecycle.receipt(role)
        assert receipt is not None
        if (
            cfg.role != role
            or cfg.instance_id != receipt["instance_id"]
            or first_billable is None
            or cfg.admitted_unix != int(first_billable)
            or ".." in cfg.output.parts
            or not cfg.output.is_relative_to(Path(root) / "out")
            or cfg.tree != Path(root)
            or cfg.image_digest != image
            or cfg.known_hosts_sha256 != fsha(ssh.known_hosts)
        ):
            raise Reject("qualification_driver_custody_or_admission_mismatch")
        prefix = staged_command(self.profile, root, name, python, image, "cuda", qualification=True)
        command = prefix.rsplit("experiments/gpu_network_v2/run.py", 1)[0] + (
            f"scripts/network_gpu_qualification.py run {shlex.quote(config_path)}"
        )
        ssh.timeout = int(
            min(
                180,
                self.lifecycle.deadline() - time.time() - 420,
                cfg.admitted_unix + 900 - time.time() - 420,
            )
        )
        if ssh.timeout <= 0:
            raise Reject("qualification_deadline_margin")
        proc = ssh.run(command, "qualification-" + name, self.lifecycle.dir / "logs")
        if proc.returncode:
            self.journal.append(
                "network_qualification_failed", role=role, name=name, rc=proc.returncode
            )
            raise Reject("qualification_driver_failed")
        return self.journal.append(
            "network_qualification_captured",
            role=role,
            name=name,
            executions=2,
            workload_allowed=False,
        )

    def stage(self, role: str, tree: Path, jobs: dict[str, Path]) -> None:
        """Existing receipt-bound SSH transports source and concrete authenticated job inputs."""
        if not self.lifecycle.receipt(role) or not self.journal.last("ssh_trusted", role=role):
            raise Reject("receipt_bound_ssh_required")
        files = {str(p.relative_to(tree)): p for p in (tree / "src/hypertrain").rglob("*.py")}
        for rel in (
            "experiments/gpu_network_v2/run.py",
            "experiments/gpu_network_v2/profile.json",
            "experiments/gpu_network_v2/orchestrate.py",
            "scripts/network_gpu_qualification.py",
            "docker/Dockerfile.gpu",
            "pyproject.toml",
            "uv.lock",
        ):
            files[rel] = tree / rel
        operation_script = tree / "scripts/network_gpu_operation.py"
        if operation_script.is_file():
            files["scripts/network_gpu_operation.py"] = operation_script
        for name, directory in jobs.items():
            if not name.replace("-", "").isalnum():
                raise Reject("invalid_staged_job")
            for path in directory.rglob("*"):
                if path.is_file():
                    if path.is_symlink():
                        raise Reject("staged_symlink_rejected")
                    files[f"out/{name}/{path.relative_to(directory)}"] = path
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w") as archive:
            for rel, path in sorted(files.items()):
                data = (
                    json.dumps(self.profile, sort_keys=True).encode()
                    if rel == "experiments/gpu_network_v2/profile.json"
                    else path.read_bytes()
                )
                info = tarfile.TarInfo(rel)
                info.size, info.mode, info.mtime = len(data), 0o600, 0
                archive.addfile(info, io.BytesIO(data))
        tar = self.lifecycle.dir / f"network-stage-{role}.tar"
        from hypertrain.protocol.hashing import sha256_hex

        digest = sha256_hex(buffer.getvalue())
        staged = self.journal.last("staged", role=role)
        if staged:
            if staged["tar_sha256"] != digest:
                raise Reject("staged_bundle_changed_on_resume")
            return
        if not tar.exists():
            durable_write(tar, buffer.getvalue())
        elif fsha(tar) != digest:
            raise Reject("stage_bundle_changed_on_resume")
        ssh = self.lifecycle.ssh(role)
        root = self.lifecycle.remote_root(role)
        if ssh.run(
            f"mkdir -p {shlex.quote(root)}/out",
            "network-mkdir",
            self.lifecycle.dir / "logs",
        ).returncode:
            raise Reject("stage_mkdir_failed")
        ssh.put(tar, f"{root}/stage.tar")
        if ssh.remote_sha256(f"{root}/stage.tar", self.lifecycle.dir / "logs") != fsha(tar):
            raise Reject("stage_bundle_hash_mismatch")
        if ssh.run(
            f"cd {shlex.quote(root)} && tar -xf stage.tar --no-same-owner",
            "network-extract",
            self.lifecycle.dir / "logs",
        ).returncode:
            raise Reject("stage_extract_failed")
        self.journal.append(
            "staged",
            role=role,
            tar_sha256=fsha(tar),
            contract="network-v2",
            jobs={name: fsha(directory / "job.json") for name, directory in jobs.items()},
        )

    def run_staged(
        self,
        role: str,
        name: str,
        trace_equivalents: int = 1,
        backend: str = "cuda",
        *,
        qualification: bool = False,
    ) -> Record:
        if not name.replace("-", "").isalnum() or not 1 <= trace_equivalents <= 12:
            raise Reject("invalid_staged_job")
        if backend not in ("cuda", "cpu"):
            raise Reject("invalid_backend")
        old = self.journal.last("network_job_done", role=role, name=name)
        if old:
            return old
        admitted = self.lifecycle.admitted()
        supervisor = self.journal.last("supervisor_ready")
        receipt = self.lifecycle.receipt(role)
        if (
            not admitted
            or not supervisor
            or not receipt
            or not self.journal.last("staged", role=role)
            or self.lifecycle.cleanup_started()
        ):
            raise Reject("lifecycle_receipts_not_ready")
        used = sum(r["executions"] for r in self.journal.all("network_execution_intent", role=role))
        if used + trace_equivalents > 63:
            raise Reject("execution_reservation_exhausted")
        bound = (
            self.profile["microqualification"]["probe_and_transfer_seconds"] // 2
            if qualification
            else self.profile["execution_and_transfer_bound_seconds"]
        )
        if type(bound) is not int or bound > (180 if qualification else 40):
            raise Reject("runtime_unqualified")
        if qualification and (
            trace_equivalents != 1
            or len(self.journal.all("network_qualification_started", role=role)) >= 2
        ):
            raise Reject("microqualification_round_limit")
        if qualification and time.time() + bound + 420 > admitted["unix"] + 900:
            raise Reject("microqualification_deadline_margin")
        remaining = self.lifecycle.deadline() - time.time()
        if remaining < trace_equivalents * bound + 600:
            raise Reject("deadline_teardown_margin")
        if self.journal.last("network_job_started", role=role, name=name):
            raise Reject("incomplete_job_requires_rescue_not_automatic_retry")
        if qualification:
            raise Reject("qualification_requires_driver_two_round_reservation")
        if self.lifecycle.cfg.get("cleanup_contract") == "network-v2":
            self.reserve(role, name, "workload", trace_equivalents)
        self.journal.append(
            "network_job_started",
            role=role,
            name=name,
            equivalents=trace_equivalents,
            instance_id=receipt["instance_id"],
        )
        root = self.lifecycle.remote_root(role)
        image = self.lifecycle.cfg["image"].rsplit("@", 1)[-1]
        python = self.lifecycle.cfg.get("remote_python", self.profile["runtime"]["remote_python"])
        command = staged_command(
            self.profile,
            root,
            name,
            python,
            image,
            backend,
            qualification=qualification,
        )
        if qualification:
            self.journal.append("network_qualification_started", role=role, name=name)
        bounds = artifact_budget(self.profile)
        file_cap = max(
            bounds["trace_bytes_per_rank"],
            bounds["state_bytes"],
            self.profile["artifacts"]["logs_bytes_per_execution"],
        )
        command = f"ulimit -f {math.ceil(file_cap / 1024)}; " + command
        ssh = self.lifecycle.ssh(role)
        ssh.timeout = trace_equivalents * bound
        started = time.monotonic()
        result = ssh.run(command, "network-" + name, self.lifecycle.dir / "logs")
        if result.returncode:
            raise Reject("network_rank_or_deadline_failure")
        return self.journal.append(
            "network_job_done",
            role=role,
            name=name,
            instance_id=receipt["instance_id"],
            ssh_rc=result.returncode,
            elapsed_seconds=time.monotonic() - started,
        )

    def execute_plan(
        self,
        evidence: dict[str, Any],
        jobs: list[Record],
        reservation: Path,
        *,
        store: Any,
        beacon: int,
    ) -> list[Record]:
        """Integrate D2 staged real launches on existing owned hosts; never create a host.

        L0 supplies concrete ordinary lease/admission/trace jobs and verifies final ledger/
        tape. This method executes exactly the reservation, rescues in finally, no fake state.
        """
        from hypertrain.miner.island_launch import confined
        from hypertrain.protocol import envelope_v2
        from hypertrain.protocol.jcs import canonicalize
        from hypertrain.protocol.messages import Receipt
        from hypertrain.protocol.messages_v2 import IslandJobV1

        raw = envelope_v2.load_json(reservation.read_bytes())
        if not isinstance(raw["record"], dict) or not isinstance(raw["receipt"], dict):
            raise Reject("signed_d2_reservation_shape")
        record = D2Reservation.model_validate(raw["record"])
        signed = envelope_v2.parse_envelope(raw["receipt"])
        receipt = Receipt.model_validate(signed.body)
        digest = sha256(canonicalize(raw["record"]))
        admitted = self.lifecycle.admitted()
        authority = self.journal.last("network_admission_authority")
        if (
            type(beacon) is not int
            or beacon < 1
            or signed.type != "Receipt"
            or not envelope_v2.verify_envelope(raw["receipt"])
            or signed.signer != store.owner_hotkey
            or record.owner != store.owner_hotkey
            or signed.run_id != record.run_id
            or receipt.commit_hash != digest
            or not receipt.received_round <= beacon <= signed.exp_drand
            or beacon != store._now(store._db)
            or record.cutoff_unix != self.lifecycle.cfg.get("cleanup_deadline_unix")
            or record.cutoff_unix > self.lifecycle.deadline()
            or record.cutoff_unix <= time.time()
            or self.lifecycle.cleanup_started()
            or admitted is None
            or authority is None
            or authority["owner"] != record.owner
            or record.quota != {"total": 126, "per_role": 63, "qualification": 4, "remaining": 122}
            or record.jobs_sha256 != sha256(canonicalize(jobs))
            or [j.model_dump(mode="json") for j in record.jobs] != jobs
        ):
            raise Reject("signed_d2_reservation_binding_invalid")
        if self.journal.all("network_plan_intake"):
            raise Reject("signed_d2_reservation_not_retryable")
        if any(job.equivalents != 1 for job in record.jobs) or set(record.roles) != {"h0", "h1"}:
            raise Reject("signed_d2_exact_remaining_slots_required")
        supervisor = self.journal.last("supervisor_ready")
        if supervisor is None or type(supervisor.get("supervisor_pid")) is not int:
            raise Reject("signed_d2_supervisor_missing")
        supervisor_fd = os.pidfd_open(supervisor["supervisor_pid"])
        try:
            import select

            if select.select([supervisor_fd], [], [], 0)[0]:
                raise Reject("signed_d2_supervisor_not_alive")
        finally:
            os.close(supervisor_fd)
        files = {}
        for name, file_item in record.files.items():
            if set(file_item) != {"path", "sha256"}:
                raise Reject("signed_d2_file_shape")
            path = Path(file_item["path"])
            if not path.is_absolute() or path.is_symlink() or fsha(path) != file_item["sha256"]:
                raise Reject("signed_d2_file_changed:" + name)
            files[name] = path
        expected = {
            "profile": record.profile_sha256,
            "evidence": record.runtime_evidence_sha256,
            "sources": record.source_map_sha256,
            "bundle": record.bundle_sha256,
            "authorization": record.budget_authority_sha256,
            "plan": record.budget_plan_sha256,
        }
        if any(record.files.get(k, {}).get("sha256") != v for k, v in expected.items()):
            raise Reject("signed_d2_required_files_missing")

        def data(name: str) -> Record:
            return json.loads(files[name].read_bytes())

        manifest = store._run_v2(record.run_id)
        qualification = store._record_v2(record.run_id, "qualification", record.image_digest)
        plan = data("plan")
        if (
            store._backend_v2(manifest) != "cuda"
            or qualification["authority_hash"] != record.qualification_authority_hash
            or record.reference_hash
            != sha256(canonicalize(manifest.training.reference_spec.model_dump(mode="json")))
            or record.image_digest != manifest.training.reference_spec.image_digest
            or data("profile") != self.profile
            or data("evidence") != evidence
            or data("authorization").get("ceiling_usd") != "50"
            or data("authorization").get("currency") != "USD"
            or data("authorization").get("experiment") != "hypertrain-network-v2"
            or plan.get("phase_cap_usd") != "50"
            or Decimal(str(plan["reserve_usd"])) != Decimal("5")
            or plan.get("admit") is not True
            or plan.get("long_workload_allowed") is not True
            or budget.dec(plan.get("debits_so_far_usd")) is None
            or Decimal(plan["debits_so_far_usd"]) < Decimal("9.34")
            or Decimal(plan["debits_so_far_usd"]) + Decimal(plan["worst_case_total_usd"]) > 50
            or Decimal(plan["worst_case_total_usd"]) > Decimal(plan["current_credit_usd"])
            or data("sources") != evidence.get("sources")
            or fsha(files["sources"]) != authority["sources_sha256"]
        ):
            raise Reject("signed_d2_authority_or_budget_differs")
        for rel, want in data("sources").items():
            path = (
                files["source_profile"]
                if rel == "experiments/gpu_network_v2/profile.json"
                else confined(Path(evidence["tree"]), rel)
            )
            if fsha(path) != want:
                raise Reject("signed_d2_source_changed")
        from hypertrain.gpu_ops import network_qualification as engine
        from hypertrain.protocol.hashing import MerkleTree

        configs = tuple(files["qualification-config-" + role] for role in self.lifecycle.roles)
        results = tuple(files["qualification-result-" + role] for role in self.lifecycle.roles)
        if [fsha(p) for p in configs] != qualification["configs"] or [
            fsha(p) for p in results
        ] != qualification["results"]:
            raise Reject("signed_d2_qualification_custody_differs")
        for cfg_path in configs:
            cfg = engine.Qualification.model_validate_json(cfg_path.read_bytes())
            if engine.check_inputs(cfg).manifest != manifest or cfg.sources != data("sources"):
                raise Reject("signed_d2_qualification_run_differs")
        engine.compare(list(results))
        sources, bundle_bytes = effective_bundle(
            engine, Path(evidence["tree"]), files["source_profile"]
        )
        if sources != data("sources") or sha256(bundle_bytes) != record.bundle_sha256:
            raise Reject("signed_d2_source_bundle_differs")
        if any(
            self.profile[k] != json.loads(files["source_profile"].read_bytes())[k]
            for k in ("model", "inner", "outer", "layout")
        ):
            raise Reject("signed_d2_training_profile_changed")
        intents = self.journal.all("network_execution_intent")
        done = self.journal.all("network_qualification_done")
        promoted = self.journal.last("network_workload_promoted")
        if (
            len(intents) != 2
            or any(r["phase"] != "qualification" or r["executions"] != 2 for r in intents)
            or len(done) != 2
            or promoted is None
            or promoted["hard_deadline_unix"] < record.cutoff_unix
            or promoted.get("budget_plan_sha256") != sha256(canonicalize(plan))
            or set(record.roles) != set(self.lifecycle.roles)
            or self.journal.last("supervisor_ready") is None
        ):
            raise Reject("signed_d2_original_qualification_missing")
        for role, binding in record.roles.items():
            self.owned_host(role)
            owned = self.lifecycle.receipt(role)
            intent = next((r for r in intents if r["role"] == role), None)
            qualification_done = next((r for r in done if r["role"] == role), None)
            if (
                owned is None
                or intent is None
                or qualification_done is None
                or binding
                != {
                    "instance_id": owned["instance_id"],
                    "machine_id": self.lifecycle.host[role]["machine_id"],
                    "qualification_intent_sha256": sha256(canonicalize(intent)),
                    "qualification_result_sha256": qualification_done["result"]["result_sha256"],
                }
                or binding["qualification_result_sha256"] not in qualification["results"]
            ):
                raise Reject("signed_d2_owned_role_differs")
        for item in record.jobs:
            job = IslandJobV1.model_validate_json(confined(item.directory, "job.json").read_bytes())
            if (
                not item.directory.is_absolute()
                or item.directory.is_symlink()
                or fsha(item.directory / "job.json") != item.job_sha256
                or job.digest() != item.job_digest
                or job.manifest != manifest
                or job.run_id != record.run_id
                or job.deadline < record.cutoff_unix
                or item.inputs
                != {k: fsha(confined(item.directory, rel)) for k, rel in job.object_paths.items()}
                or any(
                    item.inputs[k] != getattr(job, k + "_sha256")
                    for k in ("start_state", "ef_in", "v0")
                )
            ):
                raise Reject("signed_d2_job_source_differs")
            samples = confined(item.directory, job.object_paths["samples"]).read_bytes()
            proofs = json.loads(
                confined(item.directory, job.object_paths["sample_proofs"]).read_bytes()
            )
            dataset = manifest.training.dataset
            width = {"u16[seq_len+1] token ids": 2, "u32[seq_len+1] token ids": 4}.get(
                dataset.sample_format, 0
            ) * (manifest.training.model.seq_len + 1)
            if (
                width == 0
                or len(samples) != width * len(job.sample_ids)
                or len(proofs) != len(job.sample_ids)
                or any(
                    not MerkleTree.verify(
                        samples[i * width : (i + 1) * width],
                        sample,
                        [bytes.fromhex(p) for p in proof],
                        bytes.fromhex(dataset.merkle_root),
                        dataset.n_samples,
                    )
                    for i, (sample, proof) in enumerate(zip(job.sample_ids, proofs, strict=True))
                )
            ):
                raise Reject("signed_d2_job_samples_differ")
            staged = self.journal.last("staged", role=item.role)
            if staged is None or staged.get("jobs", {}).get(item.name) != item.job_sha256:
                raise Reject("signed_d2_job_not_staged")
        runtime_admission(self.profile, evidence)
        if (
            not 1 <= len(jobs) <= 122
            or len(self.lifecycle.roles) != 2
            or any(
                sum(j["equivalents"] for j in jobs if j["role"] == role) != 61
                for role in self.lifecycle.roles
            )
            or len({(j["role"], j["name"]) for j in jobs}) != len(jobs)
            or any(not 1 <= j["equivalents"] <= 12 for j in jobs)
            or any(j["backend"] != "cuda" for j in jobs)
        ):
            raise Reject("complete_d2_execution_reservation_required")

        with flock(self.lifecycle.dir / "network-reservation.lock"):
            if self.journal.all("network_plan_intake"):
                raise Reject("signed_d2_reservation_not_retryable")
            self.journal.append(
                "network_plan_intake",
                digest=digest,
                run_id=record.run_id,
                cutoff_unix=record.cutoff_unix,
                jobs_sha256=record.jobs_sha256,
            )
        stopped = threading.Event()

        def host(role: str) -> list[Record]:
            rows = []
            try:
                for j in jobs:
                    if stopped.is_set():
                        break
                    if j["role"] == role:
                        rows.append(self.run_staged(role, j["name"], j["equivalents"], "cuda"))
            except Exception:
                stopped.set()
                self.journal.append("network_plan_fault", digest=digest, role=role)
                raise
            return rows

        try:
            with ThreadPoolExecutor(max_workers=2) as pool:
                futures = [pool.submit(host, role) for role in self.lifecycle.roles]
                result = [row for future in futures for row in future.result()]
        finally:
            self.journal.append(
                "network_plan_terminal",
                digest=digest,
                failed=stopped.is_set(),
                consumed=[
                    r
                    for r in self.journal.all("network_execution_intent")
                    if r["phase"] == "workload"
                ],
            )
            # Every role attempted even when a sibling fails. Exceptions retain liability.
            errors = []
            with ThreadPoolExecutor(max_workers=2) as pool:
                pending = {role: pool.submit(self.rescue, role) for role in self.lifecycle.roles}
                for role, future in pending.items():
                    try:
                        future.result()
                    except Exception as e:
                        self.journal.append("network_rescue_failed", role=role, error=repr(e))
                        errors.append(role)
            if errors:
                raise Reject("network_rescue_incomplete:" + ",".join(errors))
        return result

    def rescue(self, role: str, fresh: bool = False) -> Record:
        """Complete path-preserving, byte/hash checked custody; failure never becomes PASS."""
        generation = sha256(
            json.dumps(
                [
                    r
                    for r in self.journal.records()
                    if r.get("role") == role
                    and r["kind"]
                    in (
                        "network_execution_intent",
                        "network_job_started",
                        "network_job_done",
                        "network_qualification_captured",
                        "network_qualification_failed",
                    )
                ],
                sort_keys=True,
            ).encode()
        )
        old = self.journal.last("network_rescued", role=role)
        if old and old.get("generation") == generation and not fresh:
            archive = Path(old["tar"])
            if not archive.is_file() or fsha(archive) != old["tar_sha256"]:
                raise Reject("network_rescue_changed_on_resume")
            verify_export(archive, artifact_budget(self.profile)["rescue_tar_bytes_per_host"])
            return old
        root = self.lifecycle.remote_root(role)
        ssh = self.lifecycle.ssh(role)
        seconds = self.profile["artifacts"]["rescue_transfer_seconds"]
        if self.lifecycle.cfg.get("cleanup_deadline_unix") is not None:
            seconds = min(seconds, self.lifecycle.cfg["cleanup_deadline_unix"] - time.time() - 120)
        started = time.monotonic()

        def remaining() -> None:
            left = seconds - (time.monotonic() - started)
            if left <= 0:
                raise Reject("network_rescue_deadline_exceeded")
            ssh.timeout = left

        remaining()
        python = self.lifecycle.cfg.get("remote_python", self.profile["runtime"]["remote_python"])
        cmd = (
            f"cd {shlex.quote(root)} && env PYTHONPATH=src {shlex.quote(python)} "
            "experiments/gpu_network_v2/orchestrate.py export out rescue.tar "
            "experiments/gpu_network_v2/profile.json"
        )
        if ssh.run(cmd, "network-export", self.lifecycle.dir / "logs").returncode:
            raise Reject("network_export_failed")
        remaining()
        expected = ssh.remote_sha256(f"{root}/rescue.tar", self.lifecycle.dir / "logs")
        destination = self.lifecycle.dir / "rescue" / role
        destination.mkdir(parents=True, exist_ok=True)
        archive = destination / f"network-rescue-{generation}-{time.time_ns()}.tar"
        remaining()
        actual = ssh.get(f"{root}/rescue.tar", archive)
        if actual != expected:
            raise Reject("network_rescue_sha_mismatch")
        manifest = verify_export(
            archive, artifact_budget(self.profile)["rescue_tar_bytes_per_host"]
        )
        remaining()
        if generation != sha256(
            json.dumps(
                [
                    r
                    for r in self.journal.records()
                    if r.get("role") == role
                    and r["kind"]
                    in (
                        "network_execution_intent",
                        "network_job_started",
                        "network_job_done",
                        "network_qualification_captured",
                        "network_qualification_failed",
                    )
                ],
                sort_keys=True,
            ).encode()
        ):
            raise Reject("network_rescue_generation_changed")
        return self.journal.append(
            "network_rescued",
            role=role,
            generation=generation,
            tar=str(archive),
            tar_sha256=actual,
            files=manifest,
            elapsed_seconds=time.monotonic() - started,
        )


def export_artifacts(directory: Path, archive: Path, profile: dict[str, Any]) -> Record:
    """Archive full paths and every byte; enforce actual shape-derived byte budget."""
    from hypertrain.protocol.hashing import sha256_hex

    files: dict[str, dict[str, Any]] = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise Reject("export_symlink_rejected")
        if path.is_file():
            files[str(path.relative_to(directory))] = {
                "bytes": path.stat().st_size,
                "sha256": fsha(path),
            }
    bounds = artifact_budget(profile)
    if sum(v["bytes"] for v in files.values()) > bounds["retained_bytes_per_host"]:
        raise Reject("actual_artifact_budget_exceeded")
    metadata = json.dumps(files, sort_keys=True).encode()
    with tarfile.open(archive, "w") as out:
        for name in files:
            out.add(directory / name, arcname="out/" + name, recursive=False)
        info = tarfile.TarInfo("manifest.json")
        info.size, info.mode, info.mtime = len(metadata), 0o600, 0
        out.addfile(info, io.BytesIO(metadata))
    if archive.stat().st_size > bounds["rescue_tar_bytes_per_host"]:
        raise Reject("actual_rescue_archive_budget_exceeded")
    with archive.open("rb") as file:
        os.fsync(file.fileno())
    return {"bytes": archive.stat().st_size, "manifest_sha256": sha256_hex(metadata)}


def verify_export(archive: Path, maximum_bytes: int) -> Record:
    import hashlib

    if archive.stat().st_size > maximum_bytes:
        raise Reject("rescue_archive_budget_exceeded")
    with tarfile.open(archive) as source:
        entries = source.getmembers()
        if len({m.name for m in entries}) != len(entries):
            raise Reject("rescue_duplicate_path")
        metadata = source.extractfile("manifest.json")
        if metadata is None or source.getmember("manifest.json").size > 1048576:
            raise Reject("rescue_manifest_missing_or_oversized")
        files = json.loads(metadata.read())
        if {m.name for m in entries} != {"manifest.json", *("out/" + n for n in files)}:
            raise Reject("rescue_manifest_coverage_mismatch")
        for name, want in files.items():
            path = Path(name)
            if (
                path.is_absolute()
                or ".." in path.parts
                or not source.getmember("out/" + name).isfile()
            ):
                raise Reject("rescue_unsafe_path")
            stream = source.extractfile("out/" + name)
            assert stream is not None
            digest, size = hashlib.sha256(), 0
            while block := stream.read(65536):
                digest.update(block)
                size += len(block)
            if digest.hexdigest() != want["sha256"] or size != want["bytes"]:
                raise Reject("rescue_content_hash_mismatch")
        return dict(files)


def reviewed_launch(record_path: Path, owner: str, beacon: int, action: str) -> Record:
    """Local preflight only. Existing signed Receipt binds every supplied artifact."""
    from hypertrain.miner.island_launch import confined
    from hypertrain.protocol.envelope_v2 import (
        load_json,
        parse_envelope,
        verify_envelope,
        verify_join,
    )
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.keys import decode_hotkey
    from hypertrain.protocol.messages_v2 import IslandJobV1, JoinRequest

    raw = load_json(record_path.read_bytes())
    if not isinstance(raw["record"], dict) or not isinstance(raw["receipt"], dict):
        raise Reject("reviewed_launch_shape")
    record: Record = json.loads(canonicalize(raw["record"]))
    signed = raw["receipt"]
    env = parse_envelope(signed)
    from hypertrain.protocol.messages import Receipt

    receipt = Receipt.model_validate(env.body)
    if (
        not verify_envelope(signed)
        or env.type != "Receipt"
        or env.signer != owner
        or env.exp_drand < beacon
        or receipt.received_round > beacon
        or receipt.commit_hash != sha256(canonicalize(record))
        or record["action"] != action
        or record["owner"] != owner
        or type(beacon) is not int
        or beacon < 1
    ):
        raise Reject("reviewed_owner_receipt_invalid")
    if record["parent_pid"] == os.getpid():
        raise Reject("persistent_controller_parent_required")
    files = record["files"]
    if not isinstance(files, dict) or not files:
        raise Reject("reviewed_files_missing")
    artifacts = {}
    for name, item in files.items():
        if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
            raise Reject("reviewed_file_shape")
        path = Path(item["path"])
        if not path.is_absolute() or path.is_symlink() or not path.is_file():
            raise Reject("reviewed_file_missing:" + name)
        if fsha(path) != item["sha256"]:
            raise Reject("reviewed_file_changed:" + name)
        artifacts[name] = path

    def data(name: str) -> Any:
        return json.loads(artifacts[name].read_bytes())

    cfg, profile = data("config"), data("profile")
    for required in (
        "src/hypertrain/gpu_ops/launcher.py",
        "src/hypertrain/gpu_ops/supervisor.py",
    ):
        actual = Path(__file__).resolve().parents[2] / required
        if data("lifecycle_hashes").get(required) != fsha(actual):
            raise Reject("reviewed_lifecycle_source_changed")
    roles = record["roles"]
    if (
        set(roles) != {"h0", "h1"}
        or {h["role"] for h in cfg["hosts"]} != set(roles)
        or len(cfg["hosts"]) != 2
        or cfg["cleanup_contract"] != "network-v2"
        or cfg["disk_gb"] != 80
        or cfg["hard_deadline_seconds"] != 3600
        or cfg["hard_grace_seconds"] != 0
        or type(cfg["boot_timeout_seconds"]) is not int
        or not 0 < cfg["boot_timeout_seconds"] <= 300
        or cfg["keyscan_attempts"] != 1
        or cfg["cleanup_deadline_unix"] != record["cutoff_unix"]
        or type(cfg["create_margin_seconds"]) is not int
        or not 420 <= cfg["create_margin_seconds"] <= 900
        or cfg["remote_python"] != profile["runtime"]["remote_python"]
        or cfg["network_profile_file"] != str(artifacts["profile"])
        or not cfg["image"].endswith("@" + profile["runtime"]["image_digest"])
        or not profile["runtime"]["driver_allowlist"]
        or type(record["parent_pid"]) is not int
        or record["parent_pid"] <= 0
        or type(record["cutoff_unix"]) is not int
        or not time.time() < record["cutoff_unix"] <= time.time() + 3600
        or record["quota"] != {"total": 126, "per_role": 63, "qualification": 4, "remaining": 122}
    ):
        raise Reject("reviewed_layout_clock_or_config")
    artifact_budget(profile)
    if (
        profile["runtime"]["backend"] != "cuda"
        or profile["runtime"]["remote_python"] != "/opt/hypertrain/venv/bin/python"
        or type(record["staging_bytes"]) is not int
        or not 0 < record["staging_bytes"] <= 1_000_000_000
        or record["public_dataset_reviewed"] is not True
        or record["full_genesis_reviewed"] is not True
    ):
        raise Reject("reviewed_source_seed_authority_missing")
    registry = data("registry")
    if "sha256:" + fsha(artifacts["registry"]) != profile["runtime"]["image_digest"]:
        raise Reject("reviewed_registry_digest")
    layers = registry.get("layers")
    if (
        registry.get("schemaVersion") != 2
        or not isinstance(layers, list)
        or not layers
        or sum(d["size"] for d in layers) + registry["config"]["size"] > 20_000_000_000
    ):
        raise Reject("reviewed_complete_image_layers_required")
    tree = Path(record["tree"])
    if not tree.is_absolute():
        raise Reject("reviewed_source_tree")
    spec = importlib.util.spec_from_file_location(
        "network_cli_driver", tree / "scripts/network_gpu_qualification.py"
    )
    if spec is None or spec.loader is None:
        raise Reject("reviewed_driver_missing")
    # Check before executing the supplied engine; its bytes must also be in the signed source map.
    sources = data("sources")
    source_profile = artifacts["source_profile"] if action == "continue" else artifacts["profile"]
    if action == "continue" and any(
        data("source_profile")[k] != profile[k] for k in ("model", "inner", "outer", "layout")
    ):
        raise Reject("reviewed_source_profile_training_changed")
    for rel, digest in sources.items():
        actual = (
            source_profile
            if rel == "experiments/gpu_network_v2/profile.json"
            else confined(tree, rel)
        )
        if fsha(actual) != digest:
            raise Reject("reviewed_source_changed:" + rel)
    if sources.get("scripts/network_gpu_qualification.py") != fsha(Path(spec.origin or "")):
        raise Reject("reviewed_driver_not_bound")
    driver = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = driver
    spec.loader.exec_module(driver)
    expected_sources, expected_bundle = effective_bundle(driver, tree, source_profile)
    if sources != expected_sources or sha256(expected_bundle) != fsha(artifacts["bundle"]):
        raise Reject("reviewed_source_bundle_mismatch")
    if Path(__file__).resolve() != tree / "experiments/gpu_network_v2/orchestrate.py":
        raise Reject("actual_local_overlay_import_mismatch")
    import hypertrain.gpu_ops.launcher as installed_launcher

    if Path(installed_launcher.__file__).resolve() != tree / "src/hypertrain/gpu_ops/launcher.py":
        raise Reject("actual_local_overlay_import_mismatch")
    for inputs in roles.values():
        if not inputs["name"].replace("-", "").isalnum():
            raise Reject("reviewed_job_name")
        job = IslandJobV1.model_validate_json(artifacts[inputs["job"]].read_bytes())
        decode_hotkey(inputs["hotkey"])
        join = JoinRequest.model_validate_json(artifacts[inputs["join"]].read_bytes())
        seed_context = data(inputs["seed_context"])
        admission_id = sha256(f"join|{env.run_id}|{join.coldkey}|{join.request_id}".encode())
        if (
            not verify_join(join, env.run_id, beacon)
            or join.hotkey != inputs["hotkey"]
            or join.policy_hash != job.manifest.network.admission_policy_hash
            or seed_context
            != {
                "run_id": env.run_id,
                "owner": owner,
                "hotkey": join.hotkey,
                "coldkey": join.coldkey,
                "request_id": join.request_id,
                "policy_hash": join.policy_hash,
                "job_sha256": fsha(artifacts[inputs["job"]]),
                "admission_id": admission_id,
                "state": "PROBATION",
                "join_sha256": fsha(artifacts[inputs["join"]]),
            }
        ):
            raise Reject("reviewed_seed_identity_authority")
        if (
            job.run_id != env.run_id
            or job.global_step0 != 0
            or job.deadline < record["cutoff_unix"]
        ):
            raise Reject("reviewed_seed_run_or_deadline")
        training = job.manifest.training.model_dump(mode="json")
        if any(
            any(
                training[k].get(key) != value
                for key, value in profile[k].items()
                if key != "lr_schedule"
            )
            for k in ("model", "inner", "outer")
        ) or any(
            training["inner"]["lr_schedule"].get(key) != value
            for key, value in profile["inner"]["lr_schedule"].items()
        ):
            raise Reject("reviewed_seed_profile")
        ref = training["reference_spec"]
        if (
            ref["layout"] != profile["layout"]
            or ref["image_digest"] != profile["runtime"]["image_digest"]
            or ref["driver_allowlist"] != profile["runtime"]["driver_allowlist"]
            or job.manifest.training.dataset.n_samples < 4096
        ):
            raise Reject("reviewed_seed_reference")
        directory = artifacts[inputs["job"]].parent
        for key, rel in job.object_paths.items():
            path = confined(directory, rel)
            if (
                inputs["objects"].get(key) not in artifacts
                or path != artifacts[inputs["objects"][key]]
            ):
                raise Reject("reviewed_seed_object_custody")
        for key in ("start_state", "ef_in", "v0"):
            if fsha(artifacts[inputs["objects"][key]]) != getattr(job, key + "_sha256"):
                raise Reject("reviewed_seed_hash")
        # Reuse complete deterministic carry genesis; never accept theta-only state.
        from hypertrain.auditor.replay import AnchorCache, pack_state, unpack_state

        theta, state = unpack_state(artifacts[inputs["objects"]["start_state"]].read_bytes())
        anchor = AnchorCache().genesis(job.manifest, inputs["hotkey"], theta)
        if (
            state is None
            or pack_state(theta, state) != pack_state(anchor.theta, anchor.state)
            or artifacts[inputs["objects"]["ef_in"]].read_bytes() != pack_state(anchor.ef)
            or artifacts[inputs["objects"]["v0"]].read_bytes() != pack_state({})
        ):
            raise Reject("reviewed_seed_full_genesis")
        from hypertrain.protocol.hashing import MerkleTree

        samples = artifacts[inputs["objects"]["samples"]].read_bytes()
        proofs = json.loads(artifacts[inputs["objects"]["sample_proofs"]].read_bytes())
        dataset = job.manifest.training.dataset
        sample_bytes = {
            "u16[seq_len+1] token ids": 2,
            "u32[seq_len+1] token ids": 4,
        }.get(dataset.sample_format)
        width = (sample_bytes or 0) * (profile["model"]["seq_len"] + 1)
        if (
            sample_bytes is None
            or len(samples) != width * len(job.sample_ids)
            or len(proofs) != len(job.sample_ids)
            or any(
                not MerkleTree.verify(
                    samples[index * width : (index + 1) * width],
                    sample,
                    [bytes.fromhex(p) for p in proof],
                    bytes.fromhex(dataset.merkle_root),
                    dataset.n_samples,
                )
                for index, (sample, proof) in enumerate(zip(job.sample_ids, proofs, strict=True))
            )
        ):
            raise Reject("reviewed_public_sample_membership")
    snapshot, offers = data("financial"), data("offers")
    if any(type(snapshot[k]) is not int for k in ("verified_unix", "account_id")) or any(
        type(o[k]) is not int
        for o in offers
        for k in ("verified_unix", "id", "machine_id", "num_gpus")
    ):
        raise Reject("reviewed_financial_strict_integers")
    observed = time.time()

    def fresh(row: Record) -> bool:
        return (
            all(
                type(row[k]) is int and observed - 30 <= row[k] <= observed
                for k in ("verified_unix", "requested_unix", "received_unix")
            )
            and row["requested_unix"] <= row["received_unix"] <= row["verified_unix"]
        )

    if (
        not fresh(snapshot)
        or any(not fresh(o) for o in offers)
        or not data("observations")
        or not all(fresh(o) for o in data("observations"))
        or {o["machine_id"] for o in offers} != {h["machine_id"] for h in cfg["hosts"]}
    ):
        raise Reject("reviewed_financial_freshness")
    authorization = data("authorization")
    if (
        authorization.get("experiment") != "hypertrain-network-v2"
        or authorization.get("currency") != "USD"
        or authorization.get("ceiling_usd") != "50"
        or authorization.get("max_instances") != 2
        or authorization.get("max_seconds_per_instance") != 3600
        or not authorization.get("source_message")
        or not authorization.get("date")
        or snapshot.get("authenticated") is not True
        or snapshot.get("uncapped_charges") is not False
        or snapshot.get("volumes") != []
        or len(offers) != 2
        or len({o["machine_id"] for o in offers}) != 2
        or any(o["num_gpus"] != 2 or o["gpu_name"] != "RTX 5090" for o in offers)
    ):
        raise Reject("reviewed_authorization_or_two_host_financial")
    if action == "continue":
        runtime_admission(profile, data("evidence"))
        cost = budget.evaluate(
            snapshot0_credit=Decimal(snapshot["available_credit_usd"])
            + Decimal(snapshot["prior_attempts_usd"]),
            current_credit=Decimal(snapshot["available_credit_usd"])
            - Decimal(snapshot["active_liabilities_usd"]),
            offers=offers,
            hard_deadline_seconds=3600,
            disk_gb=80,
            egress_gb=30,
            phase_cap=Decimal("50"),
            reserve=Decimal("5"),
        )
        plan = {**cost, "long_workload_allowed": True}
        if not cost["admit"] or Decimal(cost["hardware_worst_case_usd"]) > 5:
            raise Reject("reviewed_continuation_budget")
        if "graph" not in artifacts or "graph_driver" not in artifacts or "jobs" in artifacts:
            raise Reject("signed_continuation_graph_required")
        if artifacts["graph_driver"].resolve() != tree / "scripts/network_service_proof.py":
            raise Reject("signed_continuation_driver_source")
        graph = json.loads(artifacts["graph"].read_bytes())
        if graph.get("owner") != owner or graph.get("run_id") != env.run_id:
            raise Reject("reviewed_graph_owner_run")
        sys.path.insert(0, str(tree / "scripts"))
        graph_spec = importlib.util.spec_from_file_location(
            "signed_continuation_driver", artifacts["graph_driver"]
        )
        assert graph_spec is not None and graph_spec.loader is not None
        continuation_driver = importlib.util.module_from_spec(graph_spec)
        sys.modules[graph_spec.name] = continuation_driver
        graph_spec.loader.exec_module(continuation_driver)
        continuation_driver.ContinuationGraph.model_validate_json(artifacts["graph"].read_bytes())
        driver.compare([artifacts[inputs["qualification_result"]] for inputs in roles.values()])
    else:
        planning_snapshot = dict(snapshot)
        if action == "qualification":
            if len(snapshot["instances"]) != 2:
                raise Reject("qualification_owned_inventory_required")
            planning_snapshot["instances"] = []
        plan = admission_plan(
            profile,
            data("evidence"),
            data("authorization"),
            planning_snapshot,
            offers,
            qualification=True,
        )
    if plan != data("plan"):
        raise Reject("reviewed_plan_differs")
    return {
        "record": record,
        "files": artifacts,
        "config": cfg,
        "profile": profile,
        "driver": driver,
        "plan": plan,
        "offers": offers,
        "financial": snapshot,
        "run_id": env.run_id,
        "continuation_driver": continuation_driver if action == "continue" else None,
    }


def launch_action(
    action: str,
    checked: Record,
    run_dir: Path,
    *,
    live: bool = False,
    cancel: threading.Event | None = None,
) -> Record:
    """Use the existing lifecycle. A short CLI never masquerades as the controller parent."""
    import select

    record, cfg, files = checked["record"], checked["config"], checked["files"]
    cancel = cancel if cancel is not None else threading.Event()
    if cancel.is_set():
        raise Reject("continuation_cancelled_before_action")
    if not loopback(cfg["base_url"]) and not live:
        raise Reject("live_create_requires_explicit_flag_and_reviewed_authorization")
    if not record["cutoff_unix"] > time.time():
        raise Reject("reviewed_cutoff_expired")
    margin = (
        61 * checked["profile"]["execution_and_transfer_bound_seconds"] + 600
        if action == "continue"
        else 900
        if action == "admit"
        else 600
    )
    if time.time() + margin > record["cutoff_unix"]:
        raise Reject("reviewed_cutoff_work_margin")
    if not 0 <= time.time() - checked["financial"]["verified_unix"] <= 30:
        raise Reject("financial_expired_before_action")
    parent_fd = os.pidfd_open(record["parent_pid"])
    try:
        if select.select([parent_fd], [], [], 0)[0]:
            raise Reject("reviewed_parent_not_alive")
    finally:
        os.close(parent_fd)
    run_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    j = Journal(run_dir)
    provider = Provider(cfg["base_url"], read_api_key(cfg["key_file"]), run_dir, j, live)
    orch = Orchestrator(cfg, j, provider, run_dir)
    runtime = NetworkRuntime(orch, checked["profile"])
    if action == "admit":
        if j.last("transaction_open") or j.last("create_intent"):
            raise Reject("admission_not_retryable")
        durable_write(run_dir / "config.json", files["config"].read_bytes())
        j.append("transaction_open", pid=record["parent_pid"], live=live)
        j.append(
            "network_admission_authority",
            digest=sha256(json.dumps(record, sort_keys=True).encode()),
            owner=record["owner"],
            config_sha256=fsha(files["config"]),
            sources_sha256=fsha(files["sources"]),
        )
        quotes = {
            role: next(
                o for o in checked["offers"] if o["machine_id"] == orch.host[role]["machine_id"]
            )
            for role in orch.roles
        }
        j.append(
            "admitted",
            plan=checked["plan"],
            quotes=quotes,
            deadline_unix=record["cutoff_unix"],
            baseline_ids=[],
            account_id=checked["financial"]["account_id"],
        )
        old_pythonpath = os.environ.get("PYTHONPATH")
        os.environ["PYTHONPATH"] = str(Path(record["tree"]) / "src")
        try:
            ready = orch.ensure_supervisor()
        finally:
            if old_pythonpath is None:
                del os.environ["PYTHONPATH"]
            else:
                os.environ["PYTHONPATH"] = old_pythonpath
        if (
            ready["supervisor_pid"] in (os.getpid(), record["parent_pid"])
            or type(ready["supervisor_pid"]) is not int
        ):
            raise Reject("independent_supervisor_required")
        fd = os.pidfd_open(ready["supervisor_pid"])
        try:
            if select.select([fd], [], [], 0)[0]:
                raise Reject("supervisor_not_alive")
        finally:
            os.close(fd)
        try:
            for role in orch.roles:
                if time.time() > checked["financial"]["verified_unix"] + 30:
                    raise Reject("financial_expired_before_create")
                orch.create(role)
            return {
                "status": "ADMITTED_NOT_QUALIFIED",
                "receipts": [orch.receipt(r) for r in orch.roles],
            }
        except Exception:
            orch.cleanup(force=True)
            raise
    opened = j.all("transaction_open")
    authority = j.last("network_admission_authority")
    if (
        authority is None
        or authority["owner"] != record["owner"]
        or authority["sources_sha256"] != fsha(files["sources"])
    ):
        raise Reject("existing_admission_authority_mismatch")
    if (
        not opened
        or opened[0]["pid"] != record["parent_pid"]
        or orch.cleanup_started()
        or len(j.all("create_intent")) != 2
        or not all(orch.receipt(r) for r in orch.roles)
        or not j.last("supervisor_ready")
        or time.time() >= orch.deadline()
    ):
        raise Reject("existing_lifecycle_required")
    original = json.loads((run_dir / "config.json").read_bytes())
    if record["cutoff_unix"] > original["cleanup_deadline_unix"]:
        raise Reject("existing_cutoff_cannot_extend")
    if (
        action == "qualification"
        and cfg["network_profile_file"] != original["network_profile_file"]
    ):
        raise Reject("existing_profile_cannot_change_before_qualification")
    if any(
        cfg[key] != original[key]
        for key in (
            "hosts",
            "image",
            "base_url",
            "key_file",
            "remote_root",
            "remote_python",
            "cleanup_contract",
            "disk_gb",
            "hard_deadline_seconds",
            "hard_grace_seconds",
        )
    ):
        raise Reject("existing_lifecycle_config_changed")
    receipts = [orch.receipt(r) for r in orch.roles]
    if {row["id"] for row in checked["financial"]["instances"]} != {
        row["instance_id"] for row in receipts if row is not None
    }:
        raise Reject("continuation_owned_inventory_mismatch")
    try:
        if action == "qualification":
            with ThreadPoolExecutor(max_workers=2) as pool:
                results = list(
                    pool.map(
                        lambda role: qualify_role(checked, orch, runtime, role),
                        orch.roles,
                    )
                )
            return {"status": "CAPTURED_PENDING_ROOT_REVIEW", "results": results}
        evidence = json.loads(files["evidence"].read_bytes())
        original_profile = json.loads(Path(original["network_profile_file"]).read_bytes())
        if any(
            original_profile[k] != checked["profile"][k]
            for k in ("model", "inner", "outer", "layout")
        ):
            raise Reject("continuation_training_profile_changed")
        for role in orch.roles:
            expected = j.last("cli_import_attested", role=role)
            if (
                expected is None
                or expected["sources_sha256"] != fsha(files["sources"])
                or expected["bundle_sha256"] != fsha(files["bundle"])
            ):
                raise Reject("existing_staged_import_authority_missing")
        existing_ready = j.last("supervisor_ready")
        assert existing_ready is not None
        fd = os.pidfd_open(existing_ready["supervisor_pid"])
        try:
            if select.select([fd], [], [], 0)[0]:
                raise Reject("supervisor_not_alive")
        finally:
            os.close(fd)
        if len(j.all("network_execution_intent")) != 2 or any(
            r["phase"] != "qualification" for r in j.all("network_execution_intent")
        ):
            raise Reject("unfinished_continuation_not_retryable")
        for role, inputs in record["roles"].items():
            result = json.loads(files[inputs["qualification_receipt"]].read_bytes())
            captured = j.last("network_qualification_captured", role=role, name=inputs["name"])
            actual = checked["driver"].Result.model_validate_json(
                files[inputs["qualification_result"]].read_bytes()
            )
            receipt = orch.receipt(role)
            assert receipt is not None
            if (
                captured is None
                or actual.role != role
                or actual.instance_id != receipt["instance_id"]
                or actual.machine_id != orch.host[role]["machine_id"]
                or actual.config_sha256 != fsha(run_dir / f"cli-qualification-{role}.json")
                or actual.sources != json.loads(files["sources"].read_bytes())
                or actual.environment.image_digest != cfg["image"].rsplit("@", 1)[-1]
                or result["result_sha256"] != fsha(files[inputs["qualification_result"]])
            ):
                raise Reject("reviewed_qualification_result_binding")
            custody = runtime.rescue(role)
            if result["artifact_manifest_sha256"] != sha256(
                json.dumps(custody["files"], sort_keys=True).encode()
            ):
                raise Reject("reviewed_qualification_custody_mismatch")
            runtime.record_qualification(role, inputs["name"], result)
        source = Path(record["tree"]) / "scripts/network_service_proof.py"
        if "graph_driver" not in files or fsha(source) != fsha(files["graph_driver"]):
            raise Reject("signed_continuation_driver_changed")
        graph_driver = checked["continuation_driver"]
        if Path(graph_driver.__file__).resolve() != source:
            raise Reject("signed_continuation_driver_import")
        from hypertrain.protocol.messages_v2 import IslandJobV1

        manifest = IslandJobV1.model_validate_json(
            files[record["roles"]["h0"]["job"]].read_bytes()
        ).manifest
        graph_driver.continuation_graph(
            files["graph"], runtime, record["owner"], checked["run_id"], manifest
        )
        graph_doc = json.loads(files["graph"].read_bytes())
        if graph_doc.get("operation_sources", {}).get(
            "experiments/gpu_network_v2/profile.json"
        ) != fsha(files["profile"]):
            raise Reject("signed_continuation_effective_profile")
        if cancel.is_set() or j.last("network_graph_started"):
            raise Reject("signed_continuation_cancelled_or_not_retryable")
        runtime.promote(evidence, checked["plan"])
        j.append(
            "network_graph_started",
            graph_sha256=fsha(files["graph"]),
            run_id=checked["run_id"],
        )
        results = graph_driver.decoder_continue(files["graph"], runtime, checked, cancel)
        if (
            results.get("status") != "SIGNED_GRAPH_SETTLED_PENDING_RELEASE_REVIEW"
            or results.get("run_id") != checked["run_id"]
            or results.get("graph_sha256") != fsha(files["graph"])
            or results.get("fault_proof_complete") is not False
            or len(results.get("trial_finalizations", [])) != 48
            or len(results.get("lineage", [])) != 2
            or results.get("normal_total") != 116
            or results.get("per_host") != 58
        ):
            raise Reject("signed_continuation_not_settled")
        cleanup = orch.cleanup(force=True)
        return {
            "status": "SIGNED_GRAPH_SETTLED_PENDING_RELEASE_REVIEW",
            "results": results,
            "cleanup": cleanup,
        }
    except Exception:
        cancel.set()
        orch.cleanup(force=True)
        raise


def qualify_role(checked: Record, orch: Orchestrator, runtime: NetworkRuntime, role: str) -> Record:
    """Stage exact source/seed bytes, inspect actual import origin, then invoke frozen driver."""
    inputs, files = checked["record"]["roles"][role], checked["files"]
    orch.boot(role)
    orch.attach(role)
    orch.trust(role)
    root = orch.remote_root(role)
    first = min(r["unix"] for r in orch.j.all("create_intent"))
    stage_cutoff = min(first + 300, orch.deadline() - 420, checked["record"]["cutoff_unix"] - 420)
    ssh = deadline_ssh(orch, role, stage_cutoff)

    def remaining() -> None:
        ssh.timeout = 60
        _ = ssh.timeout

    remaining()
    if ssh.run(f"mkdir -p {shlex.quote(root)}/out/seed", "cli-mkdir", orch.dir / "logs").returncode:
        raise Reject("source_staging_failed")
    remaining()
    ssh.put(files["bundle"], root + "/source.tar")
    if ssh.remote_sha256(root + "/source.tar", orch.dir / "logs") != fsha(files["bundle"]):
        raise Reject("source_transport_hash")
    remaining()
    if ssh.run(
        f"cd {shlex.quote(root)} && tar -xf source.tar --no-same-owner",
        "cli-extract",
        orch.dir / "logs",
    ).returncode:
        raise Reject("source_extract_failed")
    job = checked["driver"].IslandJobV1.model_validate_json(files[inputs["job"]].read_bytes())
    uploads = {
        "job.json": files[inputs["job"]],
        **{job.object_paths[k]: files[v] for k, v in inputs["objects"].items()},
    }
    for rel, path in uploads.items():
        remaining()
        ssh.put(path, root + "/out/seed/" + rel)
        if ssh.remote_sha256(root + "/out/seed/" + rel, orch.dir / "logs") != fsha(path):
            raise Reject("seed_transport_hash")
    for name in ("profile", "registry"):
        remaining()
        ssh.put(files[name], root + "/" + name + ".json")
        if ssh.remote_sha256(root + "/" + name + ".json", orch.dir / "logs") != fsha(files[name]):
            raise Reject("qualification_metadata_transport_hash")
    # Fixed inspector checks source map and module paths inside real remote Python.
    sources = json.loads(files["sources"].read_bytes())
    probe = (
        "import hashlib,json,pathlib,sys,importlib.util; "
        f"root=pathlib.Path({root!r}).resolve(); expected={sources!r}; "
        "assert all(hashlib.sha256((root/p).read_bytes()).hexdigest()==h "
        "for p,h in expected.items()); "
        "import hypertrain.gpu_ops.launcher,hypertrain.trainer,torch; "
        "assert pathlib.Path(hypertrain.gpu_ops.launcher.__file__).resolve()=="
        "root/'src/hypertrain/gpu_ops/launcher.py'; "
        "assert torch.cuda.is_available() and torch.cuda.device_count()==2; "
        "assert torch.__version__=='2.14.0+cu130' and torch.version.cuda=='13.0'; "
        "assert pathlib.Path(torch.__file__).resolve().is_relative_to('/venv/main'); "
        "assert all(torch.cuda.get_device_properties(i).multi_processor_count==170 "
        "for i in range(2)); "
        "print('IMPORT_ATTESTED')"
    )
    image = checked["profile"]["runtime"]["image_digest"]
    command = staged_command(
        checked["profile"],
        root,
        inputs["name"],
        orch.cfg["remote_python"],
        image,
        "cuda",
        qualification=True,
    )
    command = (
        command.split(" experiments/gpu_network_v2/run.py", 1)[0] + " -c " + shlex.quote(probe)
    )
    remaining()
    inspected = ssh.run(command, "cli-import", orch.dir / "logs")
    if inspected.returncode or inspected.stdout.strip() != "IMPORT_ATTESTED":
        raise Reject("actual_staged_import_attestation_failed")
    orch.j.append(
        "cli_import_attested",
        role=role,
        sources_sha256=fsha(files["sources"]),
        bundle_sha256=fsha(files["bundle"]),
        image_digest=image,
    )
    receipt = orch.receipt(role)
    ready = orch.j.last("supervisor_ready")
    assert receipt is not None and ready is not None
    known_hash = fsha(ssh.known_hosts)
    contract_hash = fsha(files["config"])
    custody = {
        "instance_id": receipt["instance_id"],
        "machine_id": orch.host[role]["machine_id"],
        "role": role,
        "admitted_unix": int(first),
        "image_digest": image,
        "known_hosts_sha256": known_hash,
        "lifecycle_contract_sha256": contract_hash,
        "supervisor_pid": ready["supervisor_pid"],
        "deadline_unix": min(orch.deadline(), checked["record"]["cutoff_unix"]),
        "cuda_observed": True,
    }
    local_receipt = orch.dir / f"cli-receipt-{role}.json"
    durable_write(local_receipt, json.dumps(custody, sort_keys=True).encode())
    cfg = checked["driver"].Qualification(
        tree=root,
        profile=root + "/profile.json",
        profile_sha256=fsha(files["profile"]),
        sources=sources,
        registry_manifest=root + "/registry.json",
        image_digest=image,
        driver_allowlist=checked["profile"]["runtime"]["driver_allowlist"],
        seed_job=root + "/out/seed/job.json",
        hotkey=inputs["hotkey"],
        output=root + "/out/" + inputs["name"],
        admitted_unix=int(first),
        instance_id=receipt["instance_id"],
        machine_id=custody["machine_id"],
        role=role,
        lifecycle_contract_sha256=contract_hash,
        known_hosts_sha256=known_hash,
        lifecycle_receipts=root + "/receipts.json",
        lifecycle_receipts_sha256=fsha(local_receipt),
    )
    local_config = orch.dir / f"cli-qualification-{role}.json"
    durable_write(local_config, cfg.model_dump_json().encode())
    remaining()
    ssh.put(local_receipt, root + "/receipts.json")
    if ssh.remote_sha256(root + "/receipts.json", orch.dir / "logs") != fsha(local_receipt):
        raise Reject("qualification_receipt_transport_hash")
    remaining()
    ssh.put(local_config, root + "/qualification.json")
    if ssh.remote_sha256(root + "/qualification.json", orch.dir / "logs") != fsha(local_config):
        raise Reject("qualification_config_transport_hash")
    orch.j.append("staged", role=role, tar_sha256=fsha(files["bundle"]), contract="network-v2")
    result = runtime.run_qualification(
        role,
        inputs["name"],
        "qualification.json",
        cutoff=min(first + 480, orch.deadline() - 420, checked["record"]["cutoff_unix"] - 420),
    )
    runtime.rescue(role, fresh=True)
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Reviewed D2 lifecycle; no implicit live permission"
    )
    parser.add_argument("action", choices=("plan", "admit", "qualification", "continue", "export"))
    parser.add_argument("input", type=Path)
    parser.add_argument("extra", nargs="*")
    parser.add_argument("--owner")
    parser.add_argument("--beacon", type=int)
    parser.add_argument("--run-dir", type=Path)
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args(argv)
    if args.action == "export":
        print(
            json.dumps(
                export_artifacts(
                    args.input,
                    Path(args.extra[0]),
                    json.loads(Path(args.extra[1]).read_bytes()),
                )
            )
        )
    elif args.action == "plan":
        profile = json.loads(args.input.read_bytes())
        print(json.dumps({"paid_execution": False, "artifact_budget": artifact_budget(profile)}))
    else:
        if not args.owner or args.beacon is None or args.run_dir is None:
            parser.error("lifecycle actions require --owner --beacon --run-dir")
        checked = reviewed_launch(args.input, args.owner, args.beacon, args.action)
        import signal

        cancel = threading.Event()
        previous = {s: signal.getsignal(s) for s in (signal.SIGTERM, signal.SIGINT)}
        try:
            for s in previous:
                signal.signal(s, lambda signum, frame: cancel.set())
            print(
                json.dumps(
                    launch_action(
                        args.action,
                        checked,
                        args.run_dir,
                        live=args.live,
                        cancel=cancel,
                    )
                )
            )
        finally:
            for s, handler in previous.items():
                signal.signal(s, handler)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
