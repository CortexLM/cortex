"""Trusted existing-host operation runner; no provider or public callback API."""

from __future__ import annotations

import contextlib
import io
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, StrictInt

from hypertrain.gpu_ops.journal import durable_write, fsha
from hypertrain.miner.island_launch import IslandArtifacts, confined, validate_artifacts
from hypertrain.protocol.envelope_v2 import load_json, parse_envelope, seal, verify_envelope
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair, decode_hotkey
from hypertrain.protocol.messages import LeafPreimage
from hypertrain.protocol.messages_v2 import CommitV2, IslandJobV1, JoinChallenge, WorkProof


class Operation(BaseModel):
    """Internal accepted descriptor from trusted service adapter, not wire authority."""

    model_config = ConfigDict(extra="forbid", frozen=True)
    operation: Literal["reference", "audit", "referee", "probe", "live"]
    binding: str = Field(pattern=r"^[0-9a-f]{64}$")
    hotkey: str
    owner: str
    role: str = Field(pattern=r"^h[01]$")
    instance_id: StrictInt = Field(gt=0)
    machine_id: StrictInt = Field(gt=0)
    job: IslandJobV1
    binding_kind: Literal["trial", "commit-lease"]
    sources: dict[str, str]
    image_digest: str
    cutoff: StrictInt
    backend: Literal["cuda"] = "cuda"
    trace: bool = True
    miner_config: str | None = None
    ca: str | None = None
    master: str | None = None
    challenge: str | None = None
    context_files: dict[str, str] = Field(default_factory=dict)


def check(op: Operation, tree: Path, directory: Path) -> None:
    """Check exact materialized source, original objects and immutable public context."""
    decode_hotkey(op.hotkey)
    decode_hotkey(op.owner)
    if (
        time.time() >= op.cutoff
        or op.cutoff > op.job.deadline
        or (directory / "operation-cancelled").exists()
    ):
        raise ValueError("operation deadline")
    if op.image_digest != op.job.manifest.training.reference_spec.image_digest:
        raise ValueError("operation image")
    if (
        op.operation in ("reference", "probe")
        and op.binding_kind != "trial"
        or op.operation in ("audit", "referee", "live")
        and op.binding_kind != "commit-lease"
    ):
        raise ValueError("operation accepted binding kind")
    from hypertrain.gpu_ops import network_qualification as engine

    observed = engine.sources(tree)
    observed["scripts/network_gpu_operation.py"] = fsha(tree / "scripts/network_gpu_operation.py")
    if op.sources != observed:
        raise ValueError("operation source map")
    for rel, digest in op.context_files.items():
        if fsha(confined(tree, rel)) != digest:
            raise ValueError("operation context file hash")
    required = (
        (op.miner_config, op.ca)
        if op.operation == "live"
        else (op.miner_config, op.ca, op.challenge)
        if op.operation == "probe"
        else ()
    )
    if any(path is None or path not in op.context_files for path in required):
        raise ValueError("operation context file pins missing")
    if op.operation == "reference" and op.challenge is not None:
        if op.challenge not in op.context_files:
            raise ValueError("reference challenge context pin missing")
    for key in ("start_state", "ef_in", "v0"):
        if fsha(confined(directory, op.job.object_paths[key])) != getattr(op.job, key + "_sha256"):
            raise ValueError("operation input hash")


def trial_authority(op: Operation, raw: dict) -> JoinChallenge:
    """Validate original coordinator authority, not a claimed nonce or result."""
    from hypertrain.data.trial_assignment import trial_assignment_hash

    env = parse_envelope(raw)
    challenge = JoinChallenge.model_validate(env.body)
    if (
        env.type != "JoinChallenge"
        or env.signer != op.job.manifest.training.coord_pubkey
        or env.run_id != op.job.run_id
        or not verify_envelope(raw)
        or env.exp_drand < challenge.deadline_beacon
        or challenge.admission_id != op.binding
        or challenge.manifest_hash != op.job.run_id
        or challenge.layout != op.job.manifest.training.reference_spec.layout
        or challenge.assignment_hash
        != trial_assignment_hash(op.job.manifest, op.job.w, tuple(op.job.sample_ids))
    ):
        raise ValueError("custody original trial authority differs")
    return challenge


def probe_commit(op: Operation, artifacts: IslandArtifacts, key: Keypair, challenge: dict) -> dict:
    """Actual miner signs original publication; no launcher or acceptance assertion."""
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.trainer.compress import state_hash

    bound = trial_authority(op, challenge)
    if key.ss58 != op.hotkey or op.operation != "probe":
        raise ValueError("custody miner signing identity")
    artifacts = validate_artifacts(op.job, artifacts.directory)
    summary = load_json((artifacts.directory / "rank-0/summary.json").read_bytes())["commitments"]
    if not isinstance(summary, dict):
        raise ValueError("custody publication commitments must be an object")
    hashes: dict[str, str] = {}
    for name in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash"):
        value = summary[name]
        if not isinstance(value, str):
            raise ValueError("custody publication commitment must be a string")
        hashes[name] = value
    preimages = [LeafPreimage.model_validate(p) for p in json.loads(artifacts.leaves.read_bytes())]
    ef, _ = unpack_state(confined(artifacts.directory, op.job.object_paths["ef_in"]).read_bytes())
    commit = CommitV2(
        w=op.job.w,
        hotkey=key.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=len(preimages),
        metrics_root=MerkleTree(
            [bytes.fromhex(p.loss_f32) + bytes.fromhex(p.norm_f32) for p in preimages]
        ).root.hex(),
        tokens=len(op.job.sample_ids) * op.job.manifest.training.model.seq_len,
        delta_bytes=artifacts.delta.stat().st_size,
        ef_in_hash=state_hash(ef),
        **hashes,
    )
    commit.validate_assignment(op.job.manifest, len(op.job.sample_ids))
    return seal(key, "CommitV2", op.job.run_id, commit, bound.deadline_beacon)


def execution_environment(op: Operation) -> dict:
    """Observe this original child; no training or inferred CUDA qualification."""
    import torch

    observed: dict[str, JsonValue] = {
        "backend": op.backend,
        "python": sys.executable,
        "torch": str(torch.__version__),
        "torch_path": str(Path(torch.__file__).resolve()),
        "cuda": torch.version.cuda,
        "image_digest": op.image_digest,
    }
    if op.backend == "cuda":
        smi = shutil.which("nvidia-smi")
        if smi is None or not torch.cuda.is_available():
            raise ValueError("custody CUDA environment unavailable")
        drivers = subprocess.run(
            args=["/usr/bin/nvidia-smi", "--query-gpu=driver_version", "--format=csv,noheader"],
            executable=smi,
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout.splitlines()
        layout = op.job.manifest.training.reference_spec.layout
        if torch.cuda.device_count() != layout.n_gpus or len(drivers) != layout.n_gpus:
            raise ValueError("custody CUDA device layout differs")
        if any(
            d.strip() not in op.job.manifest.training.reference_spec.driver_allowlist
            for d in drivers
        ):
            raise ValueError("custody CUDA driver differs")
        observed["drivers"] = [d.strip() for d in drivers]
        observed["sm_counts"] = [
            torch.cuda.get_device_properties(i).multi_processor_count for i in range(layout.n_gpus)
        ]
    return observed


def publication_custody(
    op: Operation,
    directory: Path,
    artifacts: IslandArtifacts,
    challenge: dict | None,
    commit: dict | None = None,
    proof: dict | None = None,
) -> dict:
    """Hash original all-rank execution output without replay or accepted-status claim."""
    validated = validate_artifacts(op.job, artifacts.directory)
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.trainer.compress import state_hash

    start, _ = unpack_state(
        confined(artifacts.directory, op.job.object_paths["start_state"]).read_bytes()
    )
    files: dict[str, JsonValue] = {}
    for path in sorted(artifacts.directory.rglob("*")):
        if path.is_symlink():
            raise ValueError("custody publication symlink")
        if path.is_file():
            rel = str(path.relative_to(artifacts.directory))
            confined(artifacts.directory, rel)
            files[rel] = {"sha256": fsha(path), "bytes": path.stat().st_size}
    ranks: list[dict[str, JsonValue]] = []
    for rank in range(op.job.manifest.training.reference_spec.layout.n_gpus):
        raw = load_json(confined(artifacts.directory, f"rank-{rank}/summary.json").read_bytes())
        if raw["backend"] != op.backend:
            raise ValueError("custody execution backend differs")
        if op.trace and f"rank-{rank}/trace.json" not in files:
            raise ValueError("custody original trace absent")
        ranks.append(raw)
    body: dict[str, JsonValue] = {
        "operation": op.operation,
        "operation_sha256": sha256_hex(op.model_dump_json().encode()),
        "binding": op.binding,
        "hotkey": op.hotkey,
        "owner": op.owner,
        "role": op.role,
        "instance_id": op.instance_id,
        "machine_id": op.machine_id,
        "run_id": op.job.run_id,
        "job": op.job.body(),
        "job_sha256": op.job.digest(),
        "manifest_sha256": op.job.manifest.digest(),
        "sample_ids_sha256": sha256_hex(canonicalize(op.job.sample_ids)),
        "layout": op.job.manifest.training.reference_spec.layout.model_dump(mode="json"),
        "sources": {name: pin for name, pin in op.sources.items()},
        "image_digest": op.image_digest,
        "backend": op.backend,
        "rank_summaries": [rank for rank in ranks],
        "environment": execution_environment(op),
        "required_environment": op.job.manifest.training.reference_spec.env.model_dump(mode="json"),
        "publication": str(artifacts.directory.relative_to(directory)),
        "files": files,
        "publication_sha256": sha256_hex(canonicalize(files)),
        "trial_authority": None,
        "miner_commit": None,
    }
    if challenge is not None:
        bound = trial_authority(op, challenge)
        if bound.theta_hash != state_hash(start):
            raise ValueError("custody original challenge start differs")
        body["trial_authority"] = {
            "envelope": challenge,
            "sha256": sha256_hex(canonicalize(challenge)),
            "challenge_hash": bound.digest(),
            "nonce": bound.nonce,
            "seed_beacon": bound.seed_beacon,
            "assignment_hash": bound.assignment_hash,
        }
    if commit is not None:
        env = parse_envelope(commit)
        c = CommitV2.model_validate(env.body)
        summary = ranks[0]["commitments"]
        if not isinstance(summary, dict):
            raise ValueError("custody publication commitments must be an object")
        if (
            challenge is None
            or env.type != "CommitV2"
            or not verify_envelope(commit)
            or env.signer != op.hotkey
            or env.run_id != op.job.run_id
            or (c.hotkey, c.w) != (op.hotkey, op.job.w)
            or c.leaves_root != summary["leaves_root"]
            or c.delta_hash != fsha(validated.delta)
            or c.final_theta_hash != summary["final_theta_hash"]
            or c.ef_out_hash != summary["ef_out_hash"]
            or env.exp_drand < JoinChallenge.model_validate(challenge["body"]).deadline_beacon
            or c.delta_bytes != validated.delta.stat().st_size
        ):
            raise ValueError("custody original miner commit differs")
        c.validate_assignment(op.job.manifest, len(op.job.sample_ids))
        body["miner_commit"] = {"envelope": commit, "sha256": sha256_hex(canonicalize(commit))}
    if proof is not None:
        env = parse_envelope(proof)
        p = WorkProof.model_validate(env.body)
        summary = ranks[0]["commitments"]
        if not isinstance(summary, dict):
            raise ValueError("custody publication commitments must be an object")
        if (
            challenge is None
            or env.type != "WorkProof"
            or not verify_envelope(proof)
            or env.signer != op.hotkey
            or env.run_id != op.job.run_id
            or p.challenge_hash != JoinChallenge.model_validate(challenge["body"]).digest()
            or p.admission_id != op.binding
            or p.delta_hash != fsha(validated.delta)
            or p.leaves_root != summary["leaves_root"]
        ):
            raise ValueError("custody original miner proof differs")
        for ref, path in zip(
            p.artifact_refs,
            (validated.state, validated.ef, validated.delta, validated.leaves),
            strict=True,
        ):
            if ref.sha256 != fsha(path) or ref.size != path.stat().st_size:
                raise ValueError("custody original proof object differs")
        body["miner_proof"] = {"envelope": proof, "sha256": sha256_hex(canonicalize(proof))}
    return {
        "body": body,
        "sha256": sha256_hex(canonicalize(body)),
        "status": "CAPTURED_NOT_ACCEPTED",
    }


def publish_custody_result(
    op: Operation,
    directory: Path,
    result: dict,
    custody: dict,
    cancel: threading.Event,
) -> None:
    """Final finite-lifecycle gate after hashing, before any success publication."""
    if cancel.is_set() or time.time() >= op.cutoff:
        raise ValueError("custody publication deadline/cancellation")
    durable_write(directory / "publication-custody.json", canonicalize(custody))
    result["publication_custody_sha256"] = fsha(directory / "publication-custody.json")
    result["miner_commit"] = custody["body"]["miner_commit"]
    result["environment"] = custody["body"]["environment"]
    if cancel.is_set() or time.time() >= op.cutoff:
        raise ValueError("custody publication deadline/cancellation")
    durable_write(directory / "operation-result.json", json.dumps(result, sort_keys=True).encode())
    durable_write(
        directory / "execution-custody.json",
        canonicalize(
            {
                "operation_sha256": custody["body"]["operation_sha256"],
                "job_sha256": op.job.digest(),
                "binding": op.binding,
                "operation": op.operation,
                "instance_id": op.instance_id,
                "machine_id": op.machine_id,
                "publication_custody_sha256": result["publication_custody_sha256"],
                "operation_result_sha256": fsha(directory / "operation-result.json"),
                "environment_sha256": sha256_hex(canonicalize(result["environment"])),
                "status": "CAPTURED_NOT_ACCEPTED",
            }
        ),
    )


def _capture_miner_cli(
    op: Operation, tree: Path, directory: Path, cancel: threading.Event
) -> tuple[dict, Path, dict | None, dict | None, dict | None]:
    """Capture original miner CLI/API output only; no completion or acceptance claim."""
    if op.operation not in ("probe", "live"):
        raise ValueError("miner capture operation kind")
    result: dict = {}
    original_challenge = None
    miner_commit = None
    miner_proof = None
    from hypertrain.miner import cli
    from hypertrain.miner.core import MinerConfig, NetworkMiner, load_keyfile
    from hypertrain.protocol.envelope_v2 import parse_envelope, verify_envelope

    if op.miner_config is None or op.ca is None or op.master is None:
        raise ValueError("miner operation context missing")
    cfg = MinerConfig.load(confined(tree, op.miner_config), env={})
    key = load_keyfile(cfg.keyfile)
    if (
        cfg.run_id != op.job.run_id
        or cfg.owner_hotkey != op.owner
        or key.ss58 != op.hotkey
        or cfg.api != op.master
        or cfg.device != "cuda"
        or cfg.image_digest != op.image_digest
        or cfg.workdir.resolve() != directory.resolve() / "miner"
    ):
        raise ValueError("miner operation identity/config")
    os.environ["SSL_CERT_FILE"] = str(confined(tree, op.ca))
    for name in list(os.environ):
        if name.startswith("HYPERTRAIN_MINER_"):
            del os.environ[name]
    # In this private CLI child, only observe original transport; never change request/response.
    import httpx

    original = httpx.Client
    import hypertrain.miner.island_launch as island

    original_launch = island.launch_island
    operation_cancel = cancel

    def guarded_launch(
        job: IslandJobV1,
        directory: Path,
        *,
        backend: Literal["cpu", "cuda"],
        cancel: threading.Event | None = cancel,
        trace: bool = False,
    ) -> IslandArtifacts:
        nonlocal miner_commit
        if job != op.job or backend != op.backend or trace != op.trace:
            raise ValueError("miner launch accepted descriptor changed")
        if cancel is not operation_cancel or time.time() >= op.cutoff or operation_cancel.is_set():
            raise ValueError("miner launch deadline/cancellation")
        if op.operation == "probe":
            publication = directory / "published"
            publication.mkdir()
            for relative in job.object_paths.values():
                target = publication / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(confined(directory, relative), target)
            durable_write(publication / "job.json", job.model_dump_json().encode())
            from hypertrain.miner.island_launch import _launch_unbounded

            _launch_unbounded(
                job,
                publication,
                backend,
                dict(os.environ, HT_ISLAND_TRACE="1" if trace else "0"),
                cancel,
            )
            captured = validate_artifacts(job, publication)
            assert op.challenge is not None
            miner_commit = probe_commit(
                op, captured, key, load_json(confined(tree, op.challenge).read_bytes())
            )
            return captured
        return original_launch(job, directory, backend=backend, trace=trace, cancel=cancel)

    signed: list[dict] = []
    accepted: list[dict] = []
    from hypertrain.protocol.messages_v2 import MAX_MANIFEST_BYTES

    # One run_round emits Accept, Commit, WorkProof, Delta; no hidden capture retries.
    capture_limit = 4 if op.operation == "live" else 0
    capture_bytes = MAX_MANIFEST_BYTES + 1024
    responses = 0

    def publish_capture(filename: str, payload: bytes) -> None:
        """Replace only this owned regular capture, relative to a no-follow directory fd."""
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        temporary = "." + filename + ".next"
        try:
            if os.fstat(fd).st_uid != os.getuid():
                raise ValueError("capture directory owner")

            def existing() -> os.stat_result | None:
                try:
                    info = os.stat(filename, dir_fd=fd, follow_symlinks=False)
                except FileNotFoundError:
                    return None
                if (
                    not stat.S_ISREG(info.st_mode)
                    or info.st_uid != os.getuid()
                    or info.st_nlink != 1
                ):
                    raise ValueError("capture path not owned regular file")
                return info

            prior = existing()
            stream_fd = os.open(
                temporary,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                0o600,
                dir_fd=fd,
            )
            try:
                with os.fdopen(stream_fd, "wb") as stream:
                    stream.write(payload)
                    stream.flush()
                    os.fsync(stream.fileno())
                current = existing()
                if (None if current is None else (current.st_dev, current.st_ino)) != (
                    None if prior is None else (prior.st_dev, prior.st_ino)
                ):
                    raise ValueError("capture destination changed")
                os.replace(temporary, filename, src_dir_fd=fd, dst_dir_fd=fd)
                os.fsync(fd)
            finally:
                try:
                    os.unlink(temporary, dir_fd=fd)
                except FileNotFoundError:
                    pass
        finally:
            os.close(fd)

    def request_hook(request: httpx.Request) -> None:
        if request.method == "POST" and request.content:
            body = json.loads(request.content)
            if isinstance(body, dict) and body.get("v") == "ht/2":
                if len(signed) >= capture_limit or len(request.content) > capture_bytes:
                    raise ValueError("capture request count/byte bound")
                if time.time() >= op.cutoff or cancel.is_set():
                    raise ValueError("capture deadline/cancellation")
                env = parse_envelope(body)
                if (
                    not verify_envelope(body)
                    or env.run_id != op.job.run_id
                    or env.signer != op.hotkey
                ):
                    raise ValueError("miner original signature/run")
                index = len(signed)
                publish_capture(f"signed-api-{index}.bin", request.content)
                request.extensions["operation_capture_index"] = index
                signed.append(body)
                publish_capture("signed-api.json", json.dumps(signed).encode())

    def response_hook(response: httpx.Response) -> None:
        nonlocal responses
        if response.request.method == "POST" and response.request.content:
            raw = json.loads(response.request.content)
            if isinstance(raw, dict) and raw.get("v") == "ht/2":
                index = response.request.extensions["operation_capture_index"]
                if responses >= capture_limit or index != responses:
                    raise ValueError("capture response order/count")
                content = bytearray()
                for chunk in response.iter_bytes():
                    if len(content) + len(chunk) > capture_bytes:
                        raise ValueError("capture response byte bound")
                    content.extend(chunk)
                response._content = bytes(content)
                provenance = {
                    "index": index,
                    "method": response.request.method,
                    "url": str(response.request.url),
                    "status": response.status_code,
                    "request_hex": response.request.content.hex(),
                    "response_hex": bytes(content).hex(),
                }
                publish_capture(f"response-api-{index}.json", json.dumps(provenance).encode())
                responses += 1
                response.raise_for_status()
                accepted.append({"request": raw, "response": response.json(), **provenance})
                publish_capture("accepted-api.json", json.dumps(accepted).encode())
                if time.time() >= op.cutoff or cancel.is_set():
                    raise ValueError("capture deadline/cancellation")
        if response.request.url.path.endswith("/job/" + op.hotkey):
            response.read()
            actual = IslandJobV1.model_validate(response.json()["job"])
            if actual != op.job:
                raise ValueError("accepted live job changed")

    class ObservedClient(httpx.Client):
        def __init__(self, *, timeout: float, follow_redirects: bool) -> None:
            super().__init__(
                timeout=timeout,
                follow_redirects=follow_redirects,
                event_hooks={"request": [request_hook], "response": [response_hook]},
            )

    argv = [
        "probe-v2" if op.operation == "probe" else "run-v2",
        "--config",
        str(tree / op.miner_config),
    ]
    if op.operation == "probe":
        if op.challenge is None:
            raise ValueError("probe challenge missing")
        argv += [
            "--job",
            str(directory / "job.json"),
            "--challenge",
            str(confined(tree, op.challenge)),
        ]
        from hypertrain.protocol.messages_v2 import JoinChallenge

        challenge_env = parse_envelope(confined(tree, op.challenge).read_bytes())
        original_challenge = challenge_env.model_dump(mode="json")
        challenge = JoinChallenge.model_validate(challenge_env.body)
        if (
            not verify_envelope(challenge_env.model_dump(mode="json"))
            or challenge_env.signer != op.job.manifest.training.coord_pubkey
            or challenge.admission_id != op.binding
        ):
            raise ValueError("probe accepted trial differs")
    else:
        argv += ["--round", str(op.job.w)]
    output = io.StringIO()
    try:
        with contextlib.redirect_stdout(output):
            if op.operation == "probe":
                assert op.challenge is not None
                with ObservedClient(timeout=120, follow_redirects=False) as client:
                    outcome = NetworkMiner(cfg, client).probe(
                        directory / "job.json",
                        confined(tree, op.challenge),
                        launch=guarded_launch,
                        cancel=cancel,
                        trace=op.trace,
                    )
                print(json.dumps(outcome, sort_keys=True))
                code = 0
            else:
                island.__dict__["launch_island"] = guarded_launch
                cli.httpx.__dict__["Client"] = ObservedClient
                code = cli.main(argv)
    finally:
        if op.operation != "probe":
            cli.httpx.__dict__["Client"] = original
            island.__dict__["launch_island"] = original_launch
    if code:
        raise ValueError("product miner CLI failed")
    outcome = json.loads(output.getvalue())
    if op.operation == "probe":
        miner_proof = outcome["proof"]
        signed.extend([outcome["proof"], outcome["screen"]])
        for raw in signed:
            signed_env = parse_envelope(raw)
            if (
                not verify_envelope(raw)
                or signed_env.run_id != op.job.run_id
                or signed_env.signer != op.hotkey
            ):
                raise ValueError("probe signed outcome")
        published = directory / "published"
    else:
        if not {"AcceptV2", "CommitV2", "DeltaManifestV2"} <= {raw["type"] for raw in signed}:
            raise ValueError("live signed outcomes missing")
        published = cfg.workdir / op.job.run_id / op.hotkey / str(op.job.w) / "published"
    result["signed_api"] = signed
    result["accepted_api"] = accepted
    result["cli_outcome"] = outcome
    return result, published, original_challenge, miner_commit, miner_proof


def execute(op: Operation, tree: Path, directory: Path) -> None:
    """Only real product launch/CLI; original signed outbound API bodies kept unchanged."""
    check(op, tree, directory)
    cancel = threading.Event()
    signal.signal(signal.SIGTERM, lambda signum, frame: cancel.set())
    signal.signal(signal.SIGINT, lambda signum, frame: cancel.set())
    durable_write(directory / "job.json", op.job.model_dump_json().encode())
    result: dict = {
        "binding": op.binding,
        "operation": op.operation,
        "hotkey": op.hotkey,
        "role": op.role,
        "job_sha256": op.job.digest(),
        "status": "FAILED",
    }
    original_challenge = None
    miner_commit = None
    miner_proof = None
    if op.operation in ("reference", "audit", "referee"):
        from hypertrain.miner.island_launch import _launch_unbounded

        publication = directory / "published"
        publication.mkdir()
        for rel in op.job.object_paths.values():
            target = publication / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(confined(directory, rel), target)
        durable_write(publication / "job.json", op.job.model_dump_json().encode())
        env = dict(os.environ, HT_ISLAND_TRACE="1" if op.trace else "0")
        _launch_unbounded(op.job, publication, op.backend, env, cancel)
        artifacts = validate_artifacts(op.job, publication)
        if op.operation == "reference" and op.challenge is not None:
            original_challenge = load_json(confined(tree, op.challenge).read_bytes())
    else:
        captured, published, original_challenge, miner_commit, miner_proof = _capture_miner_cli(
            op, tree, directory, cancel
        )
        result.update(captured)
        artifacts = validate_artifacts(op.job, published)
    if time.time() >= op.cutoff or cancel.is_set():
        raise ValueError("operation expired before result")
    result.update(
        status="CAPTURED_NOT_ACCEPTED", publication=str(artifacts.directory.relative_to(directory))
    )
    custody = publication_custody(
        op, directory, artifacts, original_challenge, miner_commit, miner_proof
    )
    publish_custody_result(op, directory, result, custody, cancel)


def main() -> None:
    tree, directory, spec = (Path(v).resolve() for v in sys.argv[1:4])
    op = Operation.model_validate(load_json(spec.read_bytes()))
    os.environ["HT_IMAGE_DIGEST"] = op.image_digest
    if len(sys.argv) == 5 and sys.argv[4] == "child":
        execute(op, tree, directory)
        return
    check(op, tree, directory)
    directory.mkdir(parents=True, exist_ok=True)
    if (directory / "operation-result.json").exists():
        raise ValueError("operation not retryable")
    durable_write(directory / "operation-result.json", b'{"status":"FAILED_OR_UNFINISHED"}')
    with (
        (directory / "operation-stdout.log").open("xb") as out,
        (directory / "operation-stderr.log").open("xb") as err,
    ):
        proc = subprocess.Popen(  # noqa: S603 - fixed self/interpreter argv; descriptor already parsed
            [sys.executable, __file__, str(tree), str(directory), str(spec), "child"],
            stdout=out,
            stderr=err,
            start_new_session=True,
        )
        durable_write(directory / "operation-pid", str(proc.pid).encode())
        if (directory / "operation-cancelled").exists():
            os.killpg(proc.pid, signal.SIGKILL)

        def terminate(signum: int, frame: object) -> None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass

        signal.signal(signal.SIGTERM, terminate)
        signal.signal(signal.SIGINT, terminate)
        try:
            code = proc.wait(timeout=max(0.0, op.cutoff - time.time() - 2))
        finally:
            terminate(signal.SIGTERM, None)
            try:
                proc.wait(timeout=max(0.0, op.cutoff - time.time()))
            except subprocess.TimeoutExpired:
                pass
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait()
        if code:
            raise ValueError("operation child failed; custody retained")


if __name__ == "__main__":
    main()
