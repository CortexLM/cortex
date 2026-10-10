"""Bounded D3 CPU service proof. Requires admitted authority and read-only service journal."""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import signal
import socket
import sqlite3
import ssl
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Collection, Mapping
from contextlib import closing
from pathlib import Path
from types import FrameType, ModuleType
from typing import TYPE_CHECKING, Literal
from urllib.parse import urlsplit

import httpx
import uvicorn
from pydantic import BaseModel, ConfigDict, Field, JsonValue, StrictInt, field_validator

import hypertrain.trainer  # noqa: F401
import network_authority_snapshot as authority
from hypertrain.aggregator.checkpoint import (
    network_inputs,
    verify_network_checkpoint,
    write_network_checkpoint,
)
from hypertrain.aggregator.core import load_state
from hypertrain.aggregator.tape_v2 import TapeV2, replay_tape
from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.data.store import LocalFSStore
from hypertrain.gpu_ops.journal import Journal, durable_write, fsha
from hypertrain.gpu_ops.work_screen import IslandLaunch
from hypertrain.miner.admission import sign_join
from hypertrain.miner.core import load_keyfile
from hypertrain.miner.island_launch import IslandArtifacts
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, f32hex
from hypertrain.protocol.messages_v2 import (
    AggregationPolicyV2,
    CommitV2,
    EscrowLock,
    HardwareHint,
    IslandJobV1,
    JoinChallenge,
    PolicyHashes,
    RosterEntryV2,
    RoundOpenV2,
    RunManifestV2,
    StartStateV2,
    WorkProof,
    WorkScreenV2,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

if TYPE_CHECKING:
    from experiments.gpu_network_v2.orchestrate import NetworkRuntime

    from hypertrain.challenge.store import ChallengeStore


class DriverError(RuntimeError):
    pass


class Settings(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    master_https: str
    relay_https: str
    ca: Path
    snapshot: Path
    snapshot_digest: str = Field(pattern=r"^[0-9a-f]{64}$")
    secrets: Path
    service_state: Path
    evidence: Path
    verified_beacon: Path
    fixture_clock_directory: Path
    fixture_clock_offset: float = Field(allow_inf_nan=False)
    rounds: int = Field(default=2, strict=True, ge=1, le=2)
    child_timeout: int = Field(default=600, strict=True, ge=1, le=900)
    successor_registry: Path | None = None

    @field_validator("master_https", "relay_https")
    @classmethod
    def https(cls, value: str) -> str:
        url = urlsplit(value)
        if url.scheme != "https" or not url.hostname or url.username or url.query or url.fragment:
            raise ValueError("explicit HTTPS endpoint without credentials/query required")
        return value.rstrip("/")


def record(state: Path, kind: str, identity: str, run_id: str = authority.RUN) -> dict:
    """Read accepted service authority, never write admission/ledger rows."""
    with closing(sqlite3.connect((state / "challenge.db").as_uri() + "?mode=ro", uri=True)) as db:
        row = db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
            (run_id, kind, identity),
        ).fetchone()
    if row is None:
        raise DriverError(f"missing accepted {kind}:{identity}")
    return json.loads(row[0])


def durable_destination(path: Path) -> bool:
    return not path.exists() and not any("pytest" in part for part in path.resolve().parts)


def run_child(
    argv: list[str],
    env: dict[str, str],
    log: Path,
    cancel: threading.Event,
    timeout: int,
) -> None:
    """Event subscription precedes trigger; cancellation kills/reaps entire child group."""
    if cancel.is_set():
        raise DriverError("cancelled before miner")
    done = threading.Event()
    condition = vars(cancel)["_cond"]
    with log.open("xb") as output:
        process = subprocess.Popen(  # noqa: S603 - caller constructs fixed Python product CLI
            argv, env=env, stdout=output, stderr=output, start_new_session=True
        )

        def kill() -> None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

        def watch() -> None:
            with condition:
                condition.wait_for(lambda: done.is_set() or cancel.is_set(), timeout=timeout)
            if not done.is_set():
                kill()

        watcher = threading.Thread(target=watch)
        watcher.start()
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            kill()
            process.wait()
            raise DriverError("miner hard deadline") from error
        finally:
            done.set()
            with condition:
                condition.notify_all()
            watcher.join()
            kill()
        if cancel.is_set() or code:
            raise DriverError(f"miner cancelled/exit {code}; see {log}")


def miner_command(w: int) -> list[str]:
    """Use shipped actual CLI, never fixture training or an alternate miner."""
    return [sys.executable, "-m", "hypertrain.miner.cli", "run-v2", "--round", str(w)]


def bound_clock(settings: Settings, bundle: authority.Bundle, manifest: RunManifestV2) -> float:
    """Consume bootstrap runtime, not an independently selected child clock origin."""
    runtime = json.loads((settings.service_state / "runtime.json").read_bytes())
    offset = runtime["fixture_clock_offset"]
    if type(offset) not in (float, int) or not math.isfinite(offset):
        raise DriverError("nonfinite bootstrap clock offset")
    beacon = authority.fixture_beacon(runtime["beacon"])
    supplied = authority.fixture_beacon(json.loads(settings.verified_beacon.read_bytes()))
    if beacon != supplied or beacon.round != bundle.beacon_round:
        raise DriverError("bootstrap clock original beacon mismatch")
    if offset != settings.fixture_clock_offset:
        raise DriverError("child clock offset differs from bootstrap runtime")
    current = time.time() - offset
    historical = manifest.training.beacon.genesis_time + (beacon.round - 1) * 3
    if current < historical:
        raise DriverError("bootstrap clock precedes original historical origin")
    return current - time.monotonic()


def prepare(settings: Settings) -> tuple[authority.Bundle, RunManifestV2]:
    """Validate admitted portable bytes and role identities before any live write."""
    bundle = authority.validate(settings.snapshot, settings.snapshot_digest)
    if not durable_destination(settings.evidence):
        raise DriverError("fresh durable evidence outside pytest retention required")
    if not (settings.service_state / "challenge.db").is_file():
        raise DriverError("MISSING_ADAPTER: mount verified bootstrap service journal read-only")
    restored = authority._authority(settings.service_state)
    if (
        restored.public_roles != bundle.public_roles
        or restored.manifest_envelope_sha256 != bundle.manifest_envelope_sha256
        or restored.genesis_envelope_sha256 != bundle.genesis_envelope_sha256
    ):
        raise DriverError("restored original authority differs from admitted package")
    if settings.secrets.is_symlink() or settings.secrets.stat().st_mode & 0o077:
        raise DriverError("private role secret directory required")
    for role, public in bundle.public_roles.items():
        if role == "coord" or role.startswith(("hot-", "auditor-")):
            if load_keyfile(settings.secrets / f"{role}.seed").ss58 != public:
                raise DriverError(f"wrong role key {role}")
    token = settings.secrets / "admin.token"
    if (
        token.is_symlink()
        or not token.is_file()
        or token.stat().st_mode & 0o077
        or token.stat().st_uid != os.getuid()
        or not token.read_bytes().strip()
    ):
        raise DriverError("private admin token required")
    with closing(
        sqlite3.connect((settings.service_state / "challenge.db").as_uri() + "?mode=ro", uri=True)
    ) as db:
        manifest = RunManifestV2.model_validate_json(
            db.execute("SELECT manifest FROM runs").fetchone()[0]
        )
        if db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind='round'").fetchone()[0]:
            raise DriverError("fresh restored-admission service required; no implicit resume")
    if manifest.run_id() != bundle.run_id:
        raise DriverError("service snapshot run mismatch")
    monotonic_origin = bound_clock(settings, bundle, manifest)
    if not (settings.fixture_clock_directory / "sitecustomize.py").is_file():
        raise DriverError("MISSING_ADAPTER: explicit bootstrap child fixture clock required")
    expected_clock = settings.service_state / "fixture-clock/sitecustomize.py"
    if (
        settings.fixture_clock_directory / "sitecustomize.py"
    ).resolve() != expected_clock.resolve():
        raise DriverError("child clock must be bootstrap-owned code")
    ssl.create_default_context(cafile=str(settings.ca))
    settings.evidence.mkdir(parents=True, mode=0o700)
    child_clock = settings.evidence / "child-clock"
    child_clock.mkdir()
    (child_clock / "sitecustomize.py").write_text(
        f"import time\ntime.time=lambda:time.monotonic()+{monotonic_origin!r}\n"
    )
    sources = settings.evidence / "source"
    sources.mkdir()
    for path in (Path(__file__), Path(authority.__file__)):
        shutil.copyfile(path, sources / path.name)
    with (
        closing(
            sqlite3.connect(
                (settings.service_state / "challenge.db").as_uri() + "?mode=ro",
                uri=True,
            )
        ) as src,
        closing(sqlite3.connect(settings.evidence / "initial-service.db")) as dst,
    ):
        src.backup(dst)
    return bundle, manifest


def proof(settings: Settings, cancel: threading.Event) -> None:
    import torch

    bundle, manifest = prepare(settings)
    coord = load_keyfile(settings.secrets / "coord.seed")
    objects = LocalFSStore(settings.service_state / "objects")
    root = f"/v2/runs/{bundle.run_id}"
    admin = {"Authorization": "Bearer " + (settings.secrets / "admin.token").read_text().strip()}
    cfg = TrainConfig.from_manifest_v2(manifest)
    rounds: list[dict] = []
    grants: list[str] = []
    with httpx.Client(
        base_url=settings.master_https,
        verify=ssl.create_default_context(cafile=str(settings.ca)),
        timeout=settings.child_timeout,
        follow_redirects=False,
    ) as client:

        def call(
            method: str, path: str, body: dict | None = None, privileged: bool = False
        ) -> dict:
            if cancel.is_set():
                raise DriverError("proof cancelled")
            response = client.request(method, path, json=body, headers=admin if privileged else {})
            response.raise_for_status()
            value = response.json()
            with (settings.evidence / "http.jsonl").open("ab") as log:
                log.write(canonicalize({"method": method, "path": path, "response": value}) + b"\n")
                log.flush()
                os.fsync(log.fileno())
            return value

        status = call("GET", root)
        signed_manifest = status["manifest_envelope"]
        if not envelope_v2.verify_envelope(signed_manifest) or (
            signed_manifest["signer"] != bundle.public_roles["owner"]
            or signed_manifest["body"] != manifest.body()
        ):
            raise DriverError("live manifest authority mismatch")
        beacon = json.loads(settings.verified_beacon.read_bytes())
        current = authority.fixture_beacon(beacon)
        if status["now_round"] != current.round or current.round != bundle.beacon_round:
            raise DriverError("live/verified/restored beacon mismatch")

        def advance(number: int) -> None:
            from hypertrain.beacon.core import FixtureBeacon

            b = FixtureBeacon(current=number).get(number)
            payload: dict[str, int | str] = {
                "round": number,
                "signature": b.signature,
                "randomness": b.randomness,
            }
            authority.fixture_beacon(payload)
            call("POST", "/v1/admin/beacon", payload, True)

        registry = RelayRegistryV1.model_validate_json(
            objects.get(manifest.network.relay_registry_hash)
        )
        if settings.successor_registry is not None:
            successor = json.loads(settings.successor_registry.read_bytes())
            env = relay_envelope.parse_envelope(successor)
            if (
                env.signer != coord.ss58
                or env.run_id != bundle.run_id
                or not relay_envelope.verify_envelope(successor)
            ):
                raise DriverError("successor registry authority mismatch")
            call("POST", root + "/admin/relay-registry", successor, True)
            accepted = record(settings.service_state, "registry", "current")
            if accepted != successor:
                raise DriverError("accepted successor differs")
            registry = RelayRegistryV1.model_validate(accepted["body"])
        if settings.relay_https not in {spec.https_url.rstrip("/") for spec in registry.specs}:
            raise DriverError("relay endpoint not pinned; accepted successor adapter required")
        for w in range(settings.rounds):
            now = call("GET", root)["now_round"]
            theta = init_params(cfg.model)
            predecessor = "0" * 64
            if w:
                applied = record(settings.service_state, "applied", str(w - 1))
                theta = {
                    k: torch.from_numpy(v.copy())
                    for k, v in load_state(objects, applied["out_state"]).theta.items()
                }
                predecessor = applied["tape_hash"]
            starts, roster = [], []
            for i in range(4):
                hot = bundle.public_roles[f"hot-{i}"]
                admission = call("GET", root + "/admission/" + hot)
                if not admission["eligible"] or admission["record"]["state"] != "ACTIVE":
                    raise DriverError("live admission no longer ACTIVE/funded")
                cache = AnchorCache()
                if w:
                    a = record(settings.service_state, "anchor", f"{w - 1}:{hot}")
                    anchor = cache.restore(
                        Path(a["path"]),
                        manifest,
                        a["anchor_hash"],
                        a["proof_hash"],
                        a["state_root"],
                        a["ef_hash"],
                        expected_hotkey=hot,
                        expected_round=w - 1,
                        expected_backend=a["backend"],
                    )
                else:
                    anchor = cache.genesis(manifest, hot, theta)
                if anchor.state.step != w * cfg.inner.H:
                    raise DriverError("carried optimizer step mismatch")
                starts.append(
                    StartStateV2(
                        run_id=bundle.run_id,
                        w=w,
                        hotkey=hot,
                        theta_hash=state_hash(theta),
                        state_object_sha256=sha256_hex(pack_state(theta, anchor.state)),
                        opt_state_hash=optimizer_hash(anchor.state),
                        ef_object_sha256=sha256_hex(pack_state(anchor.ef)),
                        ef_hash=state_hash(anchor.ef),
                        parent_anchor_hash=anchor.anchor_hash,
                        global_step0=anchor.state.step,
                        anchor_verdict_hash=anchor.proof_hash,
                    )
                )
                roster.append(
                    {
                        "hotkey": hot,
                        "slot": i,
                        "q_i": f32hex(1),
                        "admission_id": admission["record"]["admission_id"],
                        "coldkey_group": admission["record"]["coldkey"],
                        "state": "ACTIVE",
                        "eligible_weight": 4194304,
                    }
                )
            opening = RoundOpenV2(
                w=w,
                prev_final_hash=sha256_hex(canonicalize(rounds[-1]["finalize"]["body"]))
                if w
                else "0" * 64,
                theta_hash=state_hash(theta),
                outer_state_hash="0" * 64,
                center_hash="0" * 64,
                roster_hash=sha256_hex(canonicalize(roster)),
                honeypot_commit="0" * 64,
                d_open=now + 1,
                d_assign=now + 2,
                d_commit=now + 100,
                d_audit=now + 101,
                d_upload=now + 280,
                d_final=now + 300,
                contract_version=2,
                policy_hashes=PolicyHashes.model_validate(
                    {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
                ),
                registry_epoch=registry.epoch,
                start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
                audit_mode="anchored-full",
                roster=[RosterEntryV2.model_validate(row) for row in roster],
            )
            call(
                "POST",
                root + "/admin/rounds",
                envelope_v2.seal(coord, "RoundOpenV2", bundle.run_id, opening, opening.d_final),
                True,
            )
            advance(opening.d_assign)
            call("GET", root + f"/rounds/{w}")
            for i in range(4):
                child_env = {
                    k: v for k, v in os.environ.items() if not k.startswith("HYPERTRAIN_MINER_")
                }
                child_env.update(
                    SSL_CERT_FILE=str(settings.ca),
                    OMP_NUM_THREADS="1",
                    MKL_NUM_THREADS="1",
                    HT_FIXTURE_CLOCK_OFFSET=str(settings.fixture_clock_offset),
                    PYTHONPATH=str(settings.evidence / "child-clock")
                    + os.pathsep
                    + child_env.get("PYTHONPATH", ""),
                    HYPERTRAIN_MINER_API=settings.master_https,
                    HYPERTRAIN_MINER_KEYFILE=str(settings.secrets / f"hot-{i}.seed"),
                    HYPERTRAIN_MINER_WORKDIR=str(settings.evidence / "miners"),
                    HYPERTRAIN_MINER_STATE_SOURCE="cpu",
                    HYPERTRAIN_MINER_DEVICE="cpu",
                    HYPERTRAIN_MINER_IMAGE_DIGEST=manifest.training.reference_spec.image_digest,
                    HYPERTRAIN_MINER_RUN_ID=bundle.run_id,
                    HYPERTRAIN_MINER_OWNER_HOTKEY=bundle.public_roles["owner"],
                )
                run_child(
                    miner_command(w),
                    child_env,
                    settings.evidence / f"miner-{w}-{i}.log",
                    cancel,
                    settings.child_timeout,
                )
                delta = record(
                    settings.service_state,
                    "delta",
                    f"{w}:" + bundle.public_roles[f"hot-{i}"],
                )
                grants.append(delta["body"]["grant_hash"])
            advance(opening.d_audit)
            call("POST", root + f"/admin/rounds/{w}/audits", privileged=True)
            for i in range(4):
                auditor = load_keyfile(settings.secrets / f"auditor-{i % 2}.seed")
                now = call("GET", root)["now_round"]
                request = {
                    "w": w,
                    "commit_hash": f"{w * 4 + i + 1:064x}",
                    "received_round": now,
                }
                lease = call(
                    "POST",
                    root + "/worker/lease",
                    envelope_v2.seal(auditor, "Receipt", bundle.run_id, request, opening.d_final),
                )
                if not envelope_v2.verify_envelope(lease) or lease["signer"] != coord.ss58:
                    raise DriverError("audit lease authority mismatch")
                job = lease["body"]
                request["commit_hash"] = job["lease_nonce"]
                execution = call(
                    "POST",
                    root + "/worker/jobs/" + job["job_id"] + "/execute",
                    envelope_v2.seal(auditor, "Receipt", bundle.run_id, request, opening.d_final),
                )
                if execution["verdict"]["result"] != "MATCH":
                    raise DriverError("independent inner audit did not MATCH")
                call(
                    "POST",
                    root + "/worker/jobs/" + job["job_id"] + "/complete",
                    envelope_v2.seal(
                        auditor,
                        "ReplayVerdict",
                        bundle.run_id,
                        execution["verdict"],
                        opening.d_final,
                    ),
                )
            applied = call("POST", root + f"/admin/rounds/{w}/aggregate", privileged=True)
            tape = TapeV2.from_bytes(objects.get(applied["tape_hash"]))
            if (
                len(tape.body.allocation.entries) != 4
                or sum(e.weight_units for e in tape.body.allocation.entries) != 16777216
            ):
                raise DriverError("four identity weights do not sum Q")
            if any(e.probation or e.weight_units != 4194304 for e in tape.body.allocation.entries):
                raise DriverError("shadow/probation influence or unequal four identity weights")
            if {e.hotkey for e in tape.body.allocation.entries} != {r["hotkey"] for r in roster}:
                raise DriverError("allocation identities differ from actual four-miner roster")
            inputs = record(settings.service_state, "tape-inputs", str(w))["inputs"]
            if any(
                x["commit"]["tokens"] != manifest.training.batch_samples() * cfg.model.seq_len
                for x in inputs
            ):
                raise DriverError("N2 token entitlement differs")
            replayed = replay_tape(
                objects,
                tape,
                manifest,
                AggregationPolicyV2.model_validate_json(
                    objects.get(manifest.network.aggregation_policy_hash)
                ),
                objects.get(manifest.network.economics_policy_hash),
                signer=coord.ss58,
                w=w,
                prev_state=applied["prev_state"],
                predecessor_tape_hash=predecessor,
                inputs=network_inputs(inputs),
                reference_reward_units=100,
            )
            if replayed.to_bytes() != objects.get(applied["out_state"]):
                raise DriverError("independent outer tape state differs")
            final = Finalize(
                w=w,
                final_theta_hash_w1=applied["theta_hash"],
                included=sorted(e["hotkey"] for e in roster),
                entitlements_root="0" * 64,
            )
            call(
                "POST",
                root + f"/admin/rounds/{w}/finalize",
                envelope_v2.seal(coord, "Finalize", bundle.run_id, final, opening.d_final),
                True,
            )
            settled = record(settings.service_state, "finality", str(w))
            if (
                settled["disposition"] != "SETTLED"
                or settled["unresolved_audits"]
                or settled["unresolved_disputes"]
            ):
                raise DriverError("service finality not independently settled")
            rounds.append(
                {
                    "round_open": record(settings.service_state, "round", str(w)),
                    "finalize": record(settings.service_state, "finalize", str(w)),
                    "inputs": inputs,
                    "tape_hash": applied["tape_hash"],
                    "prev_state": applied["prev_state"],
                    "predecessor_tape_hash": predecessor,
                    "reference_reward_units": 100,
                }
            )
        write_network_checkpoint(settings.evidence / "checkpoint", coord, objects, manifest, rounds)
        if verify_network_checkpoint(settings.evidence / "checkpoint", coord.ss58):
            raise DriverError("independent outer checkpoint replay failed")
        for grant in grants:
            call("POST", root + f"/admin/relay/{grant}/extend", privileged=True)
        horizons = [
            call("GET", root + "/admin/relay-settlement/" + g, privileged=True) for g in grants
        ]
        advance(
            max(
                max(h["vesting_beacon"], h["closed_beacon"], h["finality_beacon"]) + 100
                for h in horizons
            )
        )
        for grant in grants:
            released = call("POST", root + f"/admin/relay/{grant}/release", privileged=True)
            if (
                released["type"] != "CustodyRelease"
                or released["run_id"] != bundle.run_id
                or released["signer"] != coord.ss58
                or not relay_envelope.verify_envelope(released)
            ):
                raise DriverError("accepted custody release signature differs")


def durable_json(path: Path, value: dict) -> None:
    """Atomic, fsynced publication; never expose a partially written verdict."""
    temporary = path.with_suffix(".pending")
    with temporary.open("wb") as stream:
        stream.write(canonicalize(value))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def finalize_evidence(settings: Settings, completed: bool, failure: str | None) -> None:
    """FAIL first, rescue independently, commit PASS only after verified durable index."""
    root = settings.evidence
    errors = []
    result = {
        "status": "FAIL",
        "run_id": authority.RUN,
        "rounds_requested": settings.rounds,
        "inner_audits_expected": 4 * settings.rounds,
        "outer_checkpoint_replay": completed,
        "snapshot_digest": settings.snapshot_digest,
        "missing_provenance": authority.MISSING,
        "gpu_claim": False,
        "workload_error": failure,
        "export_complete": False,
    }
    durable_json(root / "result.json", result)
    destination = root / "service-export"
    try:
        destination.mkdir(exist_ok=True)
        with (
            closing(
                sqlite3.connect(
                    (settings.service_state / "challenge.db").as_uri() + "?mode=ro",
                    uri=True,
                )
            ) as src,
            closing(sqlite3.connect(destination / "challenge.db")) as dst,
        ):
            src.backup(dst)
    except (OSError, sqlite3.Error) as error:
        errors.append(f"database export: {error}")
    for name in (
        "objects",
        "trials-v2",
        "anchors-v2",
        "jobs-v2",
        "audit-v2",
        "audit-geometry-v2",
        "ledger",
    ):
        source = settings.service_state / name
        try:
            if source.exists():
                shutil.copytree(source, destination / name, dirs_exist_ok=True)
        except OSError as error:
            errors.append(f"{name} export: {error}")
    files = {}
    for path in sorted(root.rglob("*")):
        if path in (root / "index.json", root / "result.json") or path.suffix == ".pending":
            continue
        try:
            if path.is_file():
                data = path.read_bytes()
                with path.open("rb") as stream:
                    os.fsync(stream.fileno())
                files[path.relative_to(root).as_posix()] = {
                    "sha256": sha256_hex(data),
                    "size": len(data),
                }
        except OSError as error:
            errors.append(f"index {path.relative_to(root)}: {error}")
    index = {"files": files, "errors": errors, "export_complete": not errors}
    index_hash = None
    try:
        durable_json(root / "index.json", index)
        raw = (root / "index.json").read_bytes()
        if raw != canonicalize(index):
            raise DriverError("published index differs")
        index_hash = sha256_hex(raw)
    except (OSError, DriverError) as error:
        errors.append(f"index publication: {error}")
        # A censored failure index is rescue evidence, never a complete custody claim.
        try:
            durable_json(
                root / "index.json",
                {
                    "files": files,
                    "errors": errors,
                    "export_complete": False,
                },
            )
            index_hash = sha256_hex((root / "index.json").read_bytes())
        except OSError as rescue_error:
            errors.append(f"censored index publication: {rescue_error}")
    result.update(
        status="PASS" if completed and not errors else "FAIL",
        export_complete=not errors,
        errors=errors,
        index_sha256=index_hash,
    )
    durable_json(root / "result.json", result)
    if errors:
        raise DriverError("incomplete evidence export: " + "; ".join(errors))


def decoder_join_lock(
    client: httpx.Client,
    manifest: RunManifestV2,
    hot: Keypair,
    cold: Keypair,
    origins: list[str],
    units: int,
    request_id: str,
    operation_id: str,
    expiry: int,
) -> str:
    """Ordinary dual-signed join then conserved admission lock; no state writes."""
    root = f"/v2/runs/{manifest.run_id()}"
    request = sign_join(
        hot,
        cold,
        run_id=manifest.run_id(),
        request_id=request_id,
        expires_beacon=expiry,
        policy_hash=manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(
            device_name="advisory",
            device_count=manifest.training.reference_spec.layout.n_gpus,
            driver="not-qualification",
        ),
    )
    response = client.post(root + "/join", content=canonicalize(request.body()))
    response.raise_for_status()
    admission_id = response.json()["admission_id"]
    lock = EscrowLock(
        operation_id=operation_id,
        owner=cold.ss58,
        units=units,
        origin_ids=origins,
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = client.post(
        root + "/escrow/lock",
        content=canonicalize(envelope_v2.seal(cold, "EscrowLock", manifest.run_id(), lock, expiry)),
    )
    response.raise_for_status()
    return admission_id


def decoder_operation(
    client: httpx.Client,
    runtime: NetworkRuntime,
    operation: dict,
    receipt: dict,
    hot: Keypair,
    pins: dict,
    artifacts: Path,
    cancel: threading.Event,
    finalize: dict | None = None,
) -> dict:
    """Trusted Runtime dispatch; only probe/live API producers, never raw island PASS."""
    if TYPE_CHECKING:
        from scripts.network_gpu_operation import Operation
    else:
        from network_gpu_operation import Operation

    spec = Operation.model_validate(operation)
    if cancel.is_set() or spec.hotkey != hot.ss58 or spec.operation not in ("probe", "live"):
        raise DriverError("decoder operation identity/kind/cancellation differs")
    if spec.owner != pins["owner"] or spec.job.run_id != pins["run_id"]:
        raise DriverError("decoder trusted owner/run differs")
    root = f"/v2/runs/{spec.job.run_id}"
    if spec.operation == "probe":
        challenge_response = client.get(root + "/join/" + spec.binding + "/challenge")
        challenge_response.raise_for_status()
        signed_challenge = envelope_v2.parse_envelope(challenge_response.json())
        if (
            signed_challenge.type != "JoinChallenge"
            or signed_challenge.signer != spec.job.manifest.training.coord_pubkey
            or not envelope_v2.verify_envelope(signed_challenge.model_dump())
        ):
            raise DriverError("decoder signed trial authority differs")
        challenge = JoinChallenge.model_validate(signed_challenge.body)
        if spec.binding != challenge.admission_id or spec.job.run_id != challenge.manifest_hash:
            raise DriverError("decoder trial operation binding differs")
    else:
        round_response = client.get(root + f"/rounds/{spec.job.w}")
        round_response.raise_for_status()
        assignments = round_response.json()["assignment"]
        if not any(a["hotkey"] == hot.ss58 for a in assignments):
            raise DriverError("decoder live assignment identity differs")
    runtime.accept_operation(
        spec, canonicalize(receipt), owner=pins["owner"], beacon=pins["beacon"]
    )
    if spec.operation == "probe":
        reference = client.post(
            root + "/admin/join/" + challenge.admission_id + "/reference",
            headers=pins["admin_headers"],
        )
        # Service callback debits and executes the independent reference.
        reference.raise_for_status()
    result = runtime.operation(spec, artifacts, cancel=cancel)
    if result.get("status") != "CAPTURED_NOT_ACCEPTED":
        raise DriverError("API operation producer incomplete")
    api = result["cli_outcome"]
    if spec.operation == "live":
        if api != {"status": "UPLOADED"}:
            raise DriverError("actual API miner upload absent")
        if any(
            result.get(k) != v
            for k, v in {
                "operation": "live",
                "binding": spec.binding,
                "hotkey": hot.ss58,
                "role": spec.role,
                "job_sha256": spec.job.digest(),
            }.items()
        ):
            raise DriverError("live original artifact binding differs")
        signed = result["signed_api"]
        kinds = ["AcceptV2", "CommitV2", "WorkProof", "DeltaManifestV2"]
        if [x["type"] for x in signed] != kinds:
            raise DriverError("actual signed live API submissions absent")
        accepted = result["accepted_api"]
        if [entry["request"] for entry in accepted] != signed:
            raise DriverError("live original API acceptance absent")
        commit_env = envelope_v2.parse_envelope(accepted[1]["request"])
        commit = CommitV2.model_validate(commit_env.body)
        commit.validate_assignment(spec.job.manifest, len(spec.job.sample_ids))
        for raw in signed:
            env = envelope_v2.parse_envelope(raw)
            if (
                env.signer != hot.ss58
                or env.run_id != spec.job.run_id
                or not envelope_v2.verify_envelope(raw)
            ):
                raise DriverError("live signed identity differs")
            match env.type:
                case "WorkProof":
                    live_proof = WorkProof.model_validate(env.body)
                    if (
                        live_proof.challenge_hash != sha256_hex(canonicalize(commit_env.body))
                        or live_proof.leaves_root != commit.leaves_root
                        or live_proof.delta_hash != commit.delta_hash
                    ):
                        raise DriverError("live WorkProof accepted commit differs")
                case "AcceptV2" | "CommitV2" | "DeltaManifestV2":
                    if env.body["w"] != spec.job.w or env.body["hotkey"] != hot.ss58:
                        raise DriverError("live signed identity differs")
        return result
    proof = envelope_v2.parse_envelope(api["proof"])
    screen = envelope_v2.parse_envelope(api["screen"])
    if any(
        e.signer != hot.ss58
        or e.run_id != spec.job.run_id
        or not envelope_v2.verify_envelope(e.model_dump())
        for e in (proof, screen)
    ):
        raise DriverError("decoder probe signer differs")
    if proof.type != "WorkProof" or screen.type != "WorkScreenV2":
        raise DriverError("decoder probe message types differ")
    p, s = (
        WorkProof.model_validate(proof.body),
        WorkScreenV2.model_validate(screen.body),
    )
    if p.challenge_hash != challenge.digest() or s.challenge_hash != challenge.digest():
        raise DriverError("decoder probe challenge differs")
    response = client.post(
        root + "/join/proof", json={"proof": api["proof"], "screen": api["screen"]}
    )
    response.raise_for_status()
    if finalize is None:
        return response.json()  # Proof verified, not finalized or graduated.
    signed_final = envelope_v2.parse_envelope(finalize)
    if (
        signed_final.type != "Finalize"
        or signed_final.signer != spec.job.manifest.training.coord_pubkey
        or signed_final.run_id != spec.job.run_id
        or signed_final.body["w"] != spec.job.w
        or signed_final.body["included"] != [hot.ss58]
        or not envelope_v2.verify_envelope(finalize)
    ):
        raise DriverError("trial finality coordinator authority differs")
    response = client.post(
        root + "/admin/join/" + challenge.admission_id + "/finalize",
        content=canonicalize(finalize),
        headers=pins["admin_headers"],
    )
    response.raise_for_status()
    if response.json()["state"] not in ("PROBATION", "ACTIVE"):
        raise DriverError("decoder trial not finalized")
    return response.json()


def decoder_live_graph(
    client: httpx.Client,
    runtime: NetworkRuntime,
    manifest: RunManifestV2,
    rounds: list[dict | Callable[[], dict]],
    coordinator: Keypair,
    admin: dict[str, str],
    advance: Callable[[int], None],
    objects: LocalFSStore,
    checkpoint: Path,
    cancel: threading.Event,
) -> list[dict]:
    """One/two ordinary live rounds from root-reviewed opening/operation contexts."""
    if not 1 <= len(rounds) <= 2:
        raise DriverError("decoder graph requires one or two complete rounds")
    run_id = manifest.run_id()
    lineage: list[dict] = []
    root = f"/v2/runs/{run_id}"

    def call(method: str, path: str, body: dict | None = None, privileged: bool = False) -> dict:
        if cancel.is_set():
            raise DriverError("decoder live graph cancelled")
        response = client.request(
            method, root + path, json=body, headers=admin if privileged else {}
        )
        response.raise_for_status()
        return response.json()

    for round_entry in rounds:
        context = round_entry() if callable(round_entry) else round_entry
        opening = envelope_v2.parse_envelope(context["round_open"])
        body = RoundOpenV2.model_validate(opening.body)
        if (
            opening.run_id != run_id
            or opening.signer != coordinator.ss58
            or coordinator.ss58 != manifest.training.coord_pubkey
            or not envelope_v2.verify_envelope(context["round_open"])
        ):
            raise DriverError("decoder opening coordinator/run authority differs")
        if len(body.roster) != 4 or (
            not callable(context["miners"]) and len(context["miners"]) != 4
        ):
            raise DriverError("decoder complete four-identity roster required")
        call("POST", "/admin/rounds", context["round_open"], True)
        advance(body.d_assign)
        miners = context["miners"]() if callable(context["miners"]) else context["miners"]
        if len(miners) != 4:
            raise DriverError("decoder complete assigned miner roster required")
        for miner in miners:
            decoder_operation(
                client,
                runtime,
                miner["operation"],
                miner["receipt"],
                miner["key"],
                miner["pins"],
                miner["artifacts"],
                cancel,
            )
        advance(body.d_audit)
        call("POST", f"/admin/rounds/{body.w}/audits", privileged=True)
        for i, auditor in enumerate(context["auditors"]):
            request: dict[str, JsonValue] = {
                "w": body.w,
                "commit_hash": f"{body.w * 4 + i + 1:064x}",
                "received_round": body.d_audit,
            }
            lease = call(
                "POST",
                "/worker/lease",
                envelope_v2.seal(auditor, "Receipt", run_id, request, body.d_final),
            )
            env = envelope_v2.parse_envelope(lease)
            if (
                env.signer != coordinator.ss58
                or env.run_id != run_id
                or not envelope_v2.verify_envelope(lease)
            ):
                raise DriverError("decoder audit lease coordinator differs")
            request["commit_hash"] = env.body["lease_nonce"]
            job_id = env.body["job_id"]
            if not isinstance(job_id, str):
                raise DriverError("decoder audit lease job differs")
            base = "/worker/jobs/" + job_id
            audit = call(
                "POST",
                base + "/execute",
                envelope_v2.seal(auditor, "Receipt", run_id, request, body.d_final),
            )
            if audit["verdict"]["result"] != "MATCH":
                raise DriverError("decoder independent audit mismatch")
            call(
                "POST",
                base + "/complete",
                envelope_v2.seal(auditor, "ReplayVerdict", run_id, audit["verdict"], body.d_final),
            )
        applied = call("POST", f"/admin/rounds/{body.w}/aggregate", privileged=True)
        tape = TapeV2.from_bytes(objects.get(applied["tape_hash"]))
        entries = tape.body.allocation.entries
        if {e.hotkey for e in entries} != {r.hotkey for r in body.roster} or any(
            e.probation or e.weight_units != 4194304 for e in entries
        ):
            raise DriverError("decoder weight identity/shadow/Q mismatch")
        inputs = context["accepted_inputs"]()
        predecessor = lineage[-1]["tape_hash"] if lineage else "0" * 64
        replayed = replay_tape(
            objects,
            tape,
            manifest,
            AggregationPolicyV2.model_validate_json(
                objects.get(manifest.network.aggregation_policy_hash)
            ),
            objects.get(manifest.network.economics_policy_hash),
            signer=coordinator.ss58,
            w=body.w,
            prev_state=applied["prev_state"],
            predecessor_tape_hash=predecessor,
            inputs=network_inputs(inputs),
            reference_reward_units=100,
        )
        if replayed.to_bytes() != objects.get(applied["out_state"]):
            raise DriverError("decoder original outer replay differs")
        final = Finalize(
            w=body.w,
            final_theta_hash_w1=applied["theta_hash"],
            included=sorted(r.hotkey for r in body.roster),
            entitlements_root="0" * 64,
        )
        signed = envelope_v2.seal(coordinator, "Finalize", run_id, final, body.d_final)
        call("POST", f"/admin/rounds/{body.w}/finalize", signed, True)
        settled = context["accepted_finality"]()
        if (
            settled["disposition"] != "SETTLED"
            or settled["unresolved_audits"]
            or settled["unresolved_disputes"]
        ):
            raise DriverError("decoder actual finality unsettled")
        lineage.append(
            {
                "round_open": context["round_open"],
                "finalize": signed,
                "inputs": inputs,
                "tape_hash": applied["tape_hash"],
                "prev_state": applied["prev_state"],
                "predecessor_tape_hash": predecessor,
                "reference_reward_units": 100,
            }
        )
    write_network_checkpoint(checkpoint, coordinator, objects, manifest, lineage)
    if verify_network_checkpoint(checkpoint, coordinator.ss58):
        raise DriverError("decoder independent checkpoint replay differs")
    return lineage


def decoder_service_factory(
    runtime: NetworkRuntime,
    plan: Callable[[str, IslandJobV1, Path, Mapping], tuple[dict, dict | None, dict]],
) -> Callable[[str, IslandJobV1, Path, Mapping], IslandLaunch]:
    """Constructor-only trusted plan supplies admitted Operation and owner receipt."""

    def factory(
        operation: str, job: IslandJobV1, directory: Path, context: Mapping
    ) -> IslandLaunch:
        if TYPE_CHECKING:
            from scripts.network_gpu_operation import Operation
        else:
            from network_gpu_operation import Operation

        raw, receipt, pins = plan(operation, job, directory, context)
        spec = Operation.model_validate(raw)
        if spec.operation != operation or spec.job != job or spec.owner != pins["owner"]:
            raise DriverError("service role factory accepted descriptor differs")
        runtime.accept_operation(
            spec, canonicalize(receipt), owner=pins["owner"], beacon=pins["beacon"]
        )
        if spec.operation == "reference":
            if spec.challenge is None or set(spec.context_files) != {spec.challenge}:
                raise DriverError("reference original challenge context missing")
            challenge_path = spec.challenge
            path = directory / "original-join-challenge.json"
            if fsha(path) != spec.context_files[challenge_path]:
                raise DriverError("reference original challenge bytes changed")
            original_launch = runtime.launch_adapter(spec)

            def reference_launch(
                job: IslandJobV1,
                directory: Path,
                *,
                backend: Literal["cpu", "cuda"],
                cancel: threading.Event | None = None,
                trace: bool = False,
            ) -> IslandArtifacts:
                if job != spec.job or trace and not spec.trace:
                    raise DriverError("reference original launch binding differs")
                if cancel is not None and cancel.is_set() or time.time() >= spec.cutoff:
                    raise DriverError("reference context deadline/cancellation")
                if fsha(path) != spec.context_files[challenge_path]:
                    raise DriverError("reference original challenge bytes changed")
                runtime.stage_context(
                    spec.role,
                    {challenge_path: path},
                    str(Path(challenge_path).parent),
                    "reference-" + spec.job.digest(),
                    spec.cutoff,
                    cancel,
                )
                if cancel is not None and cancel.is_set() or time.time() >= spec.cutoff:
                    raise DriverError("reference context deadline/cancellation")
                return original_launch(
                    job,
                    directory,
                    backend=backend,
                    cancel=cancel,
                    trace=spec.trace,
                )

            return reference_launch
        return runtime.launch_adapter(spec)

    return factory


def service_factory(
    driver: ModuleType,
    runtime: NetworkRuntime,
    owner: Keypair,
    run_id: str,
    sources: dict[str, str],
    roles: dict,
    current_beacon: Callable[[], int],
) -> Callable[[str, IslandJobV1, Path, Mapping], IslandLaunch]:
    """Original accepted service context authorizes exactly one guarded operation."""

    def factory(
        operation: str, job: IslandJobV1, directory: Path, context: Mapping
    ) -> IslandLaunch:
        beacon = current_beacon()
        key = operation + ":" + job.digest()
        receipts: dict[str, dict | None] = {key: None}
        plan = driver.decoder_context_plan(
            runtime, owner.ss58, run_id, sources, roles, receipts, beacon
        )
        raw, _, _ = plan(operation, job, directory, context)
        if TYPE_CHECKING:
            from scripts.network_gpu_operation import Operation
        else:
            from network_gpu_operation import Operation

        spec = Operation.model_validate(raw)
        subject = {
            "role": spec.role,
            "binding": spec.binding,
            "operation": spec.operation,
            "hotkey": spec.hotkey,
            "spec_sha256": sha256_hex(spec.model_dump_json().encode()),
        }
        receipts[key] = envelope_v2.seal(
            owner,
            "Receipt",
            run_id,
            {
                "w": job.w,
                "commit_hash": sha256_hex(canonicalize(subject)),
                "received_round": beacon,
            },
            beacon + 1,
        )
        return driver.decoder_service_factory(runtime, plan)(operation, job, directory, context)

    return factory


class ContinuationIdentity(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    hotkey: str
    coldkey: str
    role: str = Field(pattern=r"^h[01]$")
    hot_key_file: Path
    cold_key_file: Path
    origin_ids: list[str] = Field(min_length=1)
    admission_units: StrictInt = Field(gt=0)


class ContinuationGraph(BaseModel):
    """Hash-bound private master-local inputs; never a serialized callback or job grid."""

    model_config = ConfigDict(extra="forbid", frozen=True)
    run_id: str = Field(pattern=r"^[0-9a-f]{64}$")
    owner: str
    owner_key_file: Path
    coord_key_file: Path
    auditor_key_files: list[Path] = Field(min_length=4, max_length=4)
    identities: list[ContinuationIdentity] = Field(min_length=4, max_length=4)
    hosts: dict[str, dict[str, StrictInt | str]]
    state_dir: Path
    evidence: Path
    master_https: str
    relay_https: str
    metagraph_https: str
    listen_host: str
    listen_port: StrictInt = Field(gt=0, lt=65536)
    ca: Path
    cert: Path
    tls_key: Path
    admin_token_file: Path
    qualification_receipt: Path
    economic_authority: Path | None = None
    operation_sources: dict[str, str]
    private_files: dict[str, str]

    @field_validator("master_https", "relay_https", "metagraph_https")
    @classmethod
    def https(cls, value: str) -> str:
        return Settings.https(value)


def continuation_graph(
    path: Path, runtime: NetworkRuntime, owner: str, run_id: str, manifest: RunManifestV2
) -> ContinuationGraph:
    """Validate immutable authority/host/file bindings before any signed graph work."""
    graph = ContinuationGraph.model_validate_json(path.read_bytes())
    if graph.economic_authority is None:
        raise DriverError("current continuation economic authority required")
    if graph.owner != owner or graph.run_id != run_id:
        raise DriverError("continuation graph owner/run differs")
    if (
        set(graph.hosts) != set(runtime.lifecycle.roles)
        or len({i.hotkey for i in graph.identities}) != 4
        or len({i.coldkey for i in graph.identities}) != 4
        or any(sum(i.role == r for i in graph.identities) != 2 for r in graph.hosts)
    ):
        raise DriverError("continuation complete role roster differs")
    for role, observed in graph.hosts.items():
        frozen = runtime.owned_host(role)
        if observed != {k: frozen[k] for k in ("instance_id", "machine_id", "image_digest")}:
            raise DriverError("continuation graph owned instance differs")
    paths = [
        graph.owner_key_file,
        graph.coord_key_file,
        *graph.auditor_key_files,
        graph.ca,
        graph.cert,
        graph.tls_key,
        graph.admin_token_file,
        graph.qualification_receipt,
        graph.economic_authority,
        *(i.hot_key_file for i in graph.identities),
        *(i.cold_key_file for i in graph.identities),
    ]
    for p in paths:
        if (
            not p.is_absolute()
            or p.is_symlink()
            or graph.private_files.get(str(p)) != sha256_hex(p.read_bytes())
        ):
            raise DriverError("continuation private input hash differs")
    secret_paths = [
        graph.owner_key_file,
        graph.coord_key_file,
        *graph.auditor_key_files,
        graph.tls_key,
        graph.admin_token_file,
        *(i.hot_key_file for i in graph.identities),
        *(i.cold_key_file for i in graph.identities),
    ]
    if any(p.stat().st_mode & 0o077 for p in secret_paths):
        raise DriverError("continuation private signing/TLS inputs not restrictive")
    if load_keyfile(graph.owner_key_file).ss58 != owner:
        raise DriverError("continuation owner signing identity differs")
    if manifest.run_id() != run_id or (
        load_keyfile(graph.coord_key_file).ss58 != manifest.training.coord_pubkey
    ):
        raise DriverError("continuation coordinator signing identity differs")
    for identity in graph.identities:
        if (
            load_keyfile(identity.hot_key_file).ss58,
            load_keyfile(identity.cold_key_file).ss58,
        ) != (identity.hotkey, identity.coldkey):
            raise DriverError("continuation miner signing identity differs")
    if any(load_keyfile(p).ss58 not in manifest.training.auditors for p in graph.auditor_key_files):
        raise DriverError("continuation auditor signing identity differs")
    if graph.evidence.exists() or graph.evidence.is_symlink():
        raise DriverError("continuation evidence must be a new owned directory")
    if (
        not graph.state_dir.is_absolute()
        or graph.state_dir.is_symlink()
        or not graph.evidence.is_absolute()
    ):
        raise DriverError("continuation state/evidence roots must be private absolute paths")
    from hypertrain.gpu_ops.network_qualification import sources

    expected = sources(Path(__file__).resolve().parents[1])
    expected["scripts/network_gpu_operation.py"] = sha256_hex(
        (Path(__file__).parent / "network_gpu_operation.py").read_bytes()
    )
    profile_key = "experiments/gpu_network_v2/profile.json"
    if set(expected) != set(graph.operation_sources) or any(
        digest != graph.operation_sources[p] for p, digest in expected.items() if p != profile_key
    ):
        raise DriverError("continuation operation source map differs")
    if urlsplit(graph.master_https).port != graph.listen_port:
        raise DriverError("continuation listener differs from pinned master")
    return graph


def continuation_graph_hash(graph: ContinuationGraph) -> str:
    body = graph.model_dump(mode="json", exclude={"economic_authority"})
    if graph.economic_authority is not None:
        body["private_files"].pop(str(graph.economic_authority), None)
    return sha256_hex(canonicalize(body))


def continuation_allocation_authority(
    runtime: NetworkRuntime,
    checked: dict,
    owner: str,
    run_id: str,
    beacon: int,
    role: str,
    host: dict,
) -> str:
    """Authenticate shipped source closure before parsing real physical CSV."""
    from hypertrain.gpu_ops.network_qualification import select_inventory, sources

    worker_path = checked["files"]["allocation_worker_receipt"]
    source_path = checked["files"]["source_receipt"]
    worker = json.loads(worker_path.read_bytes())
    source = json.loads(source_path.read_bytes())
    for payload in (worker, source):
        env = envelope_v2.parse_envelope(payload["receipt"])
        received_round = env.body["received_round"]
        if type(received_round) is not int:
            raise DriverError("original signed allocation/worker authority differs")
        if (
            env.type != "Receipt"
            or env.signer != owner
            or env.run_id != run_id
            or env.exp_drand < beacon
            or received_round > beacon
            or not envelope_v2.verify_envelope(payload["receipt"])
            or env.body["commit_hash"] != sha256_hex(canonicalize(payload["record"]))
        ):
            raise DriverError("original signed allocation/worker authority differs")
    record = worker["record"]
    if (
        record.get("allocation_contract") != "canonical-shipped-source-v1"
        or record["run_id"] != run_id
        or record["training_ranks"] != 2
        or record["physical_allocation"] != ">=2"
        or record["original_worker_sha256"] != graph_worker_hash(checked)
    ):
        raise DriverError("original allocation worker/run/layout differs")
    tree = Path(checked["record"]["tree"])
    expected = sources(tree)
    loader = tree / "scripts/network_service_proof.py"
    if loader.is_symlink():
        raise DriverError("canonical allocation loader source symlink")
    expected["scripts/network_service_proof.py"] = fsha(loader)
    profile_hash = sha256_hex(canonicalize(runtime.profile))
    if (
        record["files"] != expected
        or source["record"].get("files") != expected
        or record.get("profile_sha256") != profile_hash
        or source["record"].get("profile_sha256") != profile_hash
        or fsha(Path(__file__)) != expected["scripts/network_service_proof.py"]
        or fsha(Path(sys.modules[select_inventory.__module__].__file__ or ""))
        != expected["src/hypertrain/gpu_ops/network_qualification.py"]
    ):
        raise DriverError("signed canonical allocation source/profile closure differs")
    allocation = runtime.journal.last("allocated_gpu_inventory", role=role)
    if (
        allocation is None
        or type(host["quote"]["num_gpus"]) is not int
        or allocation["allocated_count"] != host["quote"]["num_gpus"]
    ):
        raise DriverError("original actual allocated physical count differs")
    try:
        selected = select_inventory(
            allocation["inventory"],
            allocation["allocated_count"],
            runtime.profile["runtime"]["driver_allowlist"],
        )
    except ValueError as exc:
        raise DriverError("original allocated physical CSV differs") from exc
    if tuple(allocation["selected_uuids"]) != selected:
        raise DriverError("original actual selected full UUID order differs")
    return sha256_hex(
        canonicalize(
            {
                "allocation": allocation,
                "worker_receipt": worker,
                "source_receipt": source,
            }
        )
    )


def graph_worker_hash(checked: dict) -> str:
    source = Path(checked["record"]["tree"]) / "src/hypertrain/miner/island_worker.py"
    if source.is_symlink():
        raise DriverError("original worker source symlink")
    return fsha(source)


def continuation_operator_record(
    graph: ContinuationGraph,
    runtime: NetworkRuntime,
    checked: dict,
    backend: dict,
    beacon: int,
    expires: int,
    launch_path: Path,
) -> dict:
    """Exact finite subjects from the existing verified launch; no accounting oracle."""
    from hypertrain.protocol.messages_v2 import IslandJobV1

    manifest = IslandJobV1.model_validate_json(
        checked["files"][checked["record"]["roles"]["h0"]["job"]].read_bytes()
    ).manifest
    authorization = json.loads(checked["files"]["authorization"].read_bytes())
    decoder_execution_count(runtime.profile)
    if continuation_graph_hash(
        ContinuationGraph.model_validate_json(checked["files"]["graph"].read_bytes())
    ) != continuation_graph_hash(graph):
        raise DriverError("original signed graph semantic binding differs")
    from decimal import Decimal

    accrual = json.loads(checked["files"]["accrual_receipt"].read_bytes())
    accrued = accrual["record"]
    accrual_env = envelope_v2.parse_envelope(accrual["receipt"])
    if (
        accrual_env.signer != graph.owner
        or accrual_env.type != "Receipt"
        or not envelope_v2.verify_envelope(accrual["receipt"])
        or accrual_env.body["commit_hash"] != sha256_hex(canonicalize(accrued))
        or accrual_env.exp_drand < beacon
        or accrued.get("one_cleanup_usd") != "5"
        or fsha(Path(accrued["evidence_path"])) != accrued["evidence_sha256"]
    ):
        raise DriverError("original signed accrued liability authority differs")
    for root, closure in accrued["closed_predecessors"].items():
        if closure.get("outcome") not in ("CENSORED_RESCUE", "CENSORED", "PASS"):
            raise DriverError("original closure unknown/live")
        for name in ("journal.jsonl", "lifecycle-evidence.json"):
            if fsha(Path(root) / name) != closure[name + "_sha256"]:
                raise DriverError("original closure custody differs")
        closed = json.loads((Path(root) / "lifecycle-evidence.json").read_bytes())
        if (
            closed.get("inventory", {}).get("parsed") is not True
            or closed["inventory"].get("owned_instances") != 0
        ):
            raise DriverError("original predecessor closure unknown/live")
    finance = checked["financial"]
    opening, credit = (
        Decimal(accrued["opening_credit_usd"]),
        Decimal(finance["available_credit_usd"]),
    )
    debit = max(Decimal(0), opening - credit)
    total_accrued = Decimal(accrued["accrual_usd"])
    if (
        Decimal(finance["prior_attempts_usd"]) != max(debit, total_accrued)
        or Decimal(finance["active_liabilities_usd"]) != max(Decimal(0), total_accrued - debit)
        or Decimal(checked["plan"]["snapshot0_credit_usd"]) != opening
    ):
        raise DriverError("original accrued/debit-once admission arithmetic differs")
    if any(
        type(value) is not str or not Decimal(value).is_finite() or Decimal(value) < 0
        for key in ("opening_credit_usd", "accrual_usd", "one_cleanup_usd")
        for value in [accrued[key]]
    ):
        raise DriverError("original decimal USD values invalid")
    if (
        checked["record"]["action"] != "continue"
        or authorization.get("scope") != "FULL116_CONTINUATION"
        or checked["plan"].get("long_workload_allowed") is not True
        or checked["plan"].get("admit") is not True
        or checked["plan"].get("reserve_usd") != "5"
    ):
        raise DriverError("full116 explicit economic authorization required")
    hosts = {
        role: {
            **runtime.owned_host(role),
            "allocation": next(h for h in checked["config"]["hosts"] if h["role"] == role),
            "quote": next(
                o
                for o in checked["offers"]
                if o["machine_id"] == runtime.owned_host(role)["machine_id"]
            ),
        }
        for role in ("h0", "h1")
    }
    for role, host in hosts.items():
        host["uuid_authority_hash"] = continuation_allocation_authority(
            runtime, checked, graph.owner, graph.run_id, beacon, role, host
        )
    return {
        "domain": "shadow-operator-admission-v1",
        "run_id": graph.run_id,
        "owner": graph.owner,
        "admission_hash": sha256_hex(canonicalize(checked["record"])),
        "manifest_hash": manifest.run_id(),
        "economics_policy_hash": manifest.network.economics_policy_hash,
        "graph_hash": continuation_graph_hash(graph),
        "workload": {
            "action": "continue",
            "schedule_hash": sha256_hex(canonicalize(runtime.profile["schedule"])),
            "identities": [
                {"hotkey": i.hotkey, "coldkey": i.coldkey, "role": i.role} for i in graph.identities
            ],
            "shadow_ordinals": list(range(12)),
            "live_rounds": [0, 1],
            "qualification": 4,
            "normal": 116,
            "ceiling": 126,
            "per_host": 63,
            "fault_allowance": 10,
        },
        "backend_qualification_authority_hash": backend["authority_hash"],
        "source_map_hash": sha256_hex(canonicalize(graph.operation_sources)),
        "profile_hash": fsha(checked["files"]["profile"]),
        "hosts": hosts,
        "budget": {
            "scope": "FULL116_CONTINUATION",
            "launch_path": str(launch_path),
            "launch_sha256": fsha(launch_path),
            "files": {
                name: fsha(path)
                for name, path in checked["files"].items()
                if name not in ("graph",)
            },
            "plan": checked["plan"],
            "financial": checked["financial"],
            "full_quotes": checked["offers"],
            "disk_gb": 80,
            "egress_gb": 30,
        },
        "accepted_beacon": beacon,
        "cutoff_unix": int(min(runtime.lifecycle.deadline(), checked["record"]["cutoff_unix"]))
        - 300,
        "expires_beacon": expires,
    }


def continuation_operator_intake(
    store: ChallengeStore, graph_path: Path, runtime: NetworkRuntime
) -> str:
    """Owner record is private detached authority; intake remains a store transaction."""
    return store.accept_shadow_operator_v2(graph_path, runtime)


def continuation_qualification(
    store: ChallengeStore,
    manifest: RunManifestV2,
    graph: ContinuationGraph,
    runtime: NetworkRuntime,
    checked: dict,
) -> None:
    """Consume original bootstrap object evidence and its flat accepted record."""
    from hypertrain.gpu_ops.journal import fsha
    from hypertrain.gpu_ops.network_qualification import Qualification

    if store._run_v2(graph.run_id) != manifest or store._backend_v2(manifest) != "cuda":
        raise DriverError("accepted production backend differs")
    with store._lock:
        backend = store._record_v2(
            graph.run_id, "qualification", manifest.training.reference_spec.image_digest
        )
    evidence = envelope_v2.load_json(store.objects.get(backend["authority_hash"]))
    if canonicalize(backend["receipt"]) != canonicalize(
        json.loads(graph.qualification_receipt.read_bytes())
    ) or evidence["results"] != [
        fsha(checked["files"][checked["record"]["roles"][r]["qualification_result"]])
        for r in ("h0", "h1")
    ]:
        raise DriverError("accepted production qualification receipt differs")
    configs = evidence["configs"]
    if not isinstance(configs, list):
        raise DriverError("accepted production qualification receipt differs")
    for role, config_hash in zip(("h0", "h1"), configs, strict=True):
        config_path = runtime.lifecycle.dir / f"cli-qualification-{role}.json"
        qualification = Qualification.model_validate_json(config_path.read_bytes())
        host = runtime.owned_host(role)
        if fsha(config_path) != config_hash or (
            qualification.instance_id,
            qualification.machine_id,
            qualification.role,
            qualification.image_digest,
        ) != (host["instance_id"], host["machine_id"], role, host["image_digest"]):
            raise DriverError("accepted qualification owned instance differs")


def continuation_results(journal: Journal, roles: Collection[str]) -> list[dict]:
    """Require actual accepted done/result custody for every physical workload intent."""
    started = journal.all("network_operation_started")
    completed = journal.all("network_operation_done")
    work = journal.all("network_execution_intent", phase="workload")

    def keys(rows: list[dict]) -> set[tuple[str, str]]:
        return {(r["role"], r["name"]) for r in rows}

    if (
        len(completed) != 112
        or len(keys(completed)) != 112
        or keys(completed) != keys(started)
        or keys(completed) != keys(work)
        or len(started) != 112
        or len(work) != 112
    ):
        raise DriverError("continuation accepted operation coverage differs")
    result = []
    for done in completed:
        start = next(r for r in started if (r["role"], r["name"]) == (done["role"], done["name"]))
        if any(
            not re.fullmatch(r"[0-9a-f]{64}", done.get(k, ""))
            for k in ("result_sha256", "tar_sha256")
        ):
            raise DriverError("continuation accepted result/custody hash absent")
        result.append({**start, **done})
    for role in roles:
        if {
            kind: sum(r["role"] == role and r["operation"] == kind for r in result)
            for kind in ("reference", "probe", "live", "audit")
        } != {
            "reference": 24,
            "probe": 24,
            "live": 4,
            "audit": 4,
        }:
            raise DriverError("continuation accepted role/kind coverage differs")
    return result


def continuation_close(
    store: ChallengeStore,
    server: uvicorn.Server | None,
    thread: threading.Thread | None,
    watcher: threading.Thread | None,
    cancel: threading.Event,
    done: threading.Event,
) -> None:
    """Always close local resources; retain a pending graph exception plus teardown notes."""
    primary = sys.exception()
    errors: list[Exception] = []
    try:
        cancel.set()
        for guard in tuple(store._lease_guards_v2.values()):
            guard.cancelled.set()
        done.set()
        with vars(cancel)["_cond"]:
            vars(cancel)["_cond"].notify_all()
        if server is not None:
            server.should_exit = True
        if watcher is not None:
            watcher.join(timeout=5)
            if watcher.is_alive():
                raise DriverError("continuation cancellation watcher failed to stop")
        if thread is not None:
            thread.join(timeout=10)
            if thread.is_alive():
                if server is not None:
                    server.force_exit = True
                raise DriverError("continuation HTTPS failed to stop")
    except (DriverError, RuntimeError, OSError) as exc:
        errors.append(exc)
    finally:
        try:
            store.close_v2_notifications()
        except (RuntimeError, OSError, sqlite3.Error) as exc:
            errors.append(exc)
        finally:
            try:
                store._db.close()
            except sqlite3.Error as exc:
                errors.append(exc)
    if primary is not None:
        for error in errors:
            primary.add_note(f"continuation teardown: {error}")
    elif errors:
        for error in errors[1:]:
            errors[0].add_note(f"continuation teardown: {error}")
        raise errors[0]


def decoder_continue(
    graph_path: Path, runtime: NetworkRuntime, checked: dict, cancel: threading.Event
) -> dict:
    """Run the signed admission/live graph, never raw island jobs or fixture authority."""
    import uvicorn

    from hypertrain.beacon import parse_round
    from hypertrain.challenge.admission import trial_samples
    from hypertrain.challenge.app import Config, create_app
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.gpu_ops.journal import durable_write, fsha
    from hypertrain.ledger import Params
    from hypertrain.protocol.messages_v2 import EconomicsPolicyV2, IslandJobV1

    if TYPE_CHECKING:
        from scripts.network_gpu_operation import Operation
    else:
        from network_gpu_operation import Operation

    if cancel.is_set():
        raise DriverError("continuation cancelled before graph intake")
    seed = checked["files"][checked["record"]["roles"]["h0"]["job"]]
    manifest = IslandJobV1.model_validate_json(seed.read_bytes()).manifest
    graph = continuation_graph(
        graph_path, runtime, checked["record"]["owner"], checked["run_id"], manifest
    )
    cutoff = int(min(runtime.lifecycle.deadline(), checked["record"]["cutoff_unix"])) - 300
    owner, coord = (
        load_keyfile(graph.owner_key_file),
        load_keyfile(graph.coord_key_file),
    )
    if manifest.run_id() != graph.run_id or coord.ss58 != manifest.training.coord_pubkey:
        raise DriverError("continuation manifest/coordinator differs")
    if not (graph.state_dir / "challenge.db").is_file():
        raise DriverError("accepted production store absent")
    objects = LocalFSStore(graph.state_dir / "objects")
    policy = EconomicsPolicyV2.model_validate_json(
        objects.get(manifest.network.economics_policy_hash)
    )
    if policy.ledger_mode == "test":
        raise DriverError("CPU/test genesis cannot authorize signed continuation")
    store = ChallengeStore(
        graph.state_dir,
        Params(
            "hypertrain",
            manifest.training.beacon.genesis_time,
            4320,
            1,
            manifest.training.verify.E_vest_rounds,
        ),
        coord,
        owner.ss58,
        parse_round,
        objects,
    )
    server = thread = watcher = None
    done = threading.Event()
    try:
        continuation_qualification(store, manifest, graph, runtime, checked)

        def now() -> int:
            with store._lock:
                current = store._fresh_v2(manifest, "signed-continuation")
                if not store._beacon_v2(current).bls_verified:
                    raise DriverError("production beacon not BLS verified")
                return current

        def advance(number: int) -> None:
            with store.beacon_arrived:
                ready = store.beacon_arrived.wait_for(
                    lambda: cancel.is_set() or store._now(store._db) >= number,
                    timeout=max(0, cutoff - time.time()),
                )
            if not ready or cancel.is_set():
                raise DriverError("continuation beacon/cancellation deadline")
            now()

        now()
        store.accept_shadow_operator_v2(graph_path, runtime)
        roles = {i.hotkey: {"role": i.role} for i in graph.identities}
        store._role_launch = service_factory(
            sys.modules[__name__],
            runtime,
            owner,
            graph.run_id,
            graph.operation_sources,
            roles,
            now,
        )
        app_config = Config(
            "hypertrain",
            graph.state_dir,
            graph.metagraph_https,
            None,
            graph.admin_token_file,
            None,
            graph.coord_key_file,
            owner.ss58,
            store.params,
        )
        app = create_app(app_config, objects=objects, _store=store)
        started = threading.Event()

        class ContinuationServer(uvicorn.Server):
            async def startup(self, sockets: list[socket.socket] | None = None) -> None:
                await super().startup(sockets)
                started.set()

        server = ContinuationServer(
            uvicorn.Config(
                app,
                host=graph.listen_host,
                port=graph.listen_port,
                ssl_certfile=str(graph.cert),
                ssl_keyfile=str(graph.tls_key),
                log_level="error",
                timeout_graceful_shutdown=5,
            )
        )

        def stop() -> None:
            with vars(cancel)["_cond"]:
                vars(cancel)["_cond"].wait_for(lambda: done.is_set() or cancel.is_set())
            if cancel.is_set():
                for guard in tuple(store._lease_guards_v2.values()):
                    guard.cancelled.set()
                with store.beacon_arrived:
                    store.beacon_arrived.notify_all()
                server.should_exit = True

        watcher = threading.Thread(target=stop, name="continuation-cancel", daemon=True)
        watcher.start()
        thread = threading.Thread(target=server.run, name="continuation-https", daemon=True)
        thread.start()
        if not started.wait(timeout=min(15, max(0, runtime.lifecycle.deadline() - time.time()))):
            raise DriverError("continuation HTTPS startup deadline")
        if not server.started or cancel.is_set():
            raise DriverError("continuation HTTPS failed/cancelled")
        graph.evidence.mkdir(mode=0o700, parents=True, exist_ok=False)
        admin = {"Authorization": "Bearer " + graph.admin_token_file.read_text().strip()}
        with httpx.Client(
            base_url=graph.master_https,
            verify=ssl.create_default_context(cafile=str(graph.ca)),
            timeout=60,
            follow_redirects=False,
        ) as client:
            registry = store._registry_v2(manifest)
            if graph.relay_https not in {s.https_url.rstrip("/") for s in registry.specs}:
                raise DriverError("continuation relay not accepted registry")

            def materialize(
                identity: ContinuationIdentity,
                job: IslandJobV1,
                directory: Path,
                kind: Literal["reference", "audit", "referee", "probe", "live"],
                binding: str,
                challenge: dict | None = None,
            ) -> dict:
                if cancel.is_set() or time.time() >= min(job.deadline, cutoff):
                    raise DriverError("continuation context cancelled/expired")
                frozen = runtime.owned_host(identity.role)
                remote_root = runtime.lifecycle.remote_root(identity.role)
                name = "op-" + sha256_hex(
                    (job.run_id + kind + identity.hotkey + binding + job.digest()).encode()
                )
                context = "context/" + name
                key_rel, cfg_rel, ca_rel = (
                    context + "/miner.key",
                    context + "/miner.toml",
                    context + "/ca.pem",
                )
                config = (
                    f"api={json.dumps(graph.master_https)}\n"
                    f"keyfile={json.dumps(remote_root + '/' + key_rel)}\n"
                    f"workdir={json.dumps(remote_root + '/out/' + name + '/miner')}\n"
                    'state_source="network"\ndevice="cuda"\n'
                    f"image_digest={json.dumps(frozen['image_digest'])}\n"
                    f"run_id={json.dumps(graph.run_id)}\nowner_hotkey={json.dumps(owner.ss58)}\n"
                ).encode()
                local_config = directory / "miner.toml"
                durable_write(local_config, config)
                paths = {
                    key_rel: identity.hot_key_file,
                    cfg_rel: local_config,
                    ca_rel: graph.ca,
                }
                challenge_rel = None
                if challenge is not None:
                    challenge_rel = context + "/challenge.json"
                    path = directory / "challenge.json"
                    durable_write(path, canonicalize(challenge))
                    paths[challenge_rel] = path
                runtime.stage_context(
                    identity.role, paths, context, name, min(job.deadline, cutoff), cancel
                )
                spec = Operation(
                    operation=kind,
                    binding=binding,
                    hotkey=identity.hotkey,
                    owner=owner.ss58,
                    role=identity.role,
                    instance_id=frozen["instance_id"],
                    machine_id=frozen["machine_id"],
                    job=job,
                    binding_kind="trial" if kind == "probe" else "commit-lease",
                    sources=graph.operation_sources,
                    image_digest=frozen["image_digest"],
                    cutoff=min(job.deadline, cutoff),
                    trace=kind in ("probe", "live"),
                    miner_config=cfg_rel,
                    ca=ca_rel,
                    master=graph.master_https,
                    challenge=challenge_rel,
                    context_files={rel: fsha(path) for rel, path in paths.items()},
                )
                subject = {
                    "role": spec.role,
                    "binding": spec.binding,
                    "operation": spec.operation,
                    "hotkey": spec.hotkey,
                    "spec_sha256": sha256_hex(spec.model_dump_json().encode()),
                }
                beacon = now()
                receipt = envelope_v2.seal(
                    owner,
                    "Receipt",
                    job.run_id,
                    {
                        "w": job.w,
                        "commit_hash": sha256_hex(canonicalize(subject)),
                        "received_round": beacon,
                    },
                    beacon + 1,
                )
                return {
                    "operation": spec.model_dump(mode="json"),
                    "receipt": receipt,
                    "key": load_keyfile(identity.hot_key_file),
                    "pins": {
                        "owner": owner.ss58,
                        "run_id": job.run_id,
                        "beacon": beacon,
                        "admin_headers": admin,
                    },
                    "artifacts": directory,
                }

            identities, trials = [], []
            for identity in graph.identities:
                if cancel.is_set():
                    raise DriverError("continuation cancelled before admission")
                hot, cold = (
                    load_keyfile(identity.hot_key_file),
                    load_keyfile(identity.cold_key_file),
                )
                if (hot.ss58, cold.ss58) != (identity.hotkey, identity.coldkey):
                    raise DriverError("continuation identity signing keys differ")
                request_id = sha256_hex((graph.run_id + identity.hotkey + "join").encode())
                with store._lock:
                    accepted = store._db.execute(
                        "SELECT admission_id,coldkey,clean_count,pending_dispute "
                        "FROM admissions_v2 WHERE hotkey=?",
                        (hot.ss58,),
                    ).fetchone()
                if accepted is not None:
                    if (
                        accepted["coldkey"] != cold.ss58
                        or accepted["clean_count"] != 0
                        or accepted["pending_dispute"]
                    ):
                        raise DriverError("continuation prior admission differs")
                    admission = accepted["admission_id"]
                else:
                    admission = decoder_join_lock(
                        client,
                        manifest,
                        hot,
                        cold,
                        identity.origin_ids,
                        identity.admission_units,
                        request_id,
                        sha256_hex((request_id + "lock").encode()),
                        now() + 400,
                    )
                identities.append((identity, admission))
                for _ in range(12):
                    with store._lock:
                        row = store._db.execute(
                            "SELECT seed_beacon FROM admissions_v2 WHERE admission_id=?",
                            (admission,),
                        ).fetchone()
                    advance(row[0])
                    response = client.get(f"/v2/runs/{graph.run_id}/join/{admission}/challenge")
                    response.raise_for_status()
                    signed = response.json()
                    challenge = JoinChallenge.model_validate(
                        envelope_v2.parse_envelope(signed).body
                    )
                    with store._lock:
                        epoch = store._db.execute(
                            "SELECT trial_epoch FROM admissions_v2 WHERE admission_id=?",
                            (admission,),
                        ).fetchone()[0]
                        job, directory = store.stage_trial_v2(
                            graph.run_id,
                            challenge,
                            trial_samples(
                                manifest,
                                admission,
                                challenge.nonce,
                                store._beacon_v2(challenge.seed_beacon),
                            ),
                            epoch,
                        )
                    op = materialize(identity, job, directory, "probe", admission, signed)
                    decoder_operation(
                        client,
                        runtime,
                        op["operation"],
                        op["receipt"],
                        hot,
                        op["pins"],
                        directory,
                        cancel,
                    )
                    with store._lock:
                        reference = WorkProof.model_validate_json(
                            store._db.execute(
                                "SELECT reference_json FROM admissions_v2 WHERE admission_id=?",
                                (admission,),
                            ).fetchone()[0]
                        )
                    from hypertrain.auditor.replay import unpack_state

                    theta, _ = unpack_state(objects.get(reference.artifact_refs[0].sha256))
                    final = envelope_v2.seal(
                        coord,
                        "Finalize",
                        graph.run_id,
                        Finalize(
                            w=job.w,
                            final_theta_hash_w1=state_hash(theta),
                            included=[hot.ss58],
                            entitlements_root=sha256_hex(canonicalize(reference.body())),
                        ),
                        challenge.deadline_beacon,
                    )
                    response = client.post(
                        f"/v2/runs/{graph.run_id}/admin/join/{admission}/finalize",
                        json=final,
                        headers=admin,
                    )
                    response.raise_for_status()
                    trials.append(response.json())

            def round_context(w: int) -> Callable[[], dict]:
                def build() -> dict:
                    import torch

                    from hypertrain.aggregator.capacity_worker import (
                        GenesisRequest,
                        collect_genesis,
                    )
                    from hypertrain.auditor.replay import unpack_state

                    cfg = TrainConfig.from_manifest_v2(manifest)
                    roster = []
                    for slot, (identity, admission) in enumerate(identities):
                        status = store.admission_v2(
                            graph.run_id, "status", identity=identity.hotkey
                        )
                        if not status["eligible"] or status["record"]["state"] != "ACTIVE":
                            raise DriverError("continuation identity lacks actual graduation")
                        roster.append(
                            {
                                "hotkey": identity.hotkey,
                                "slot": slot,
                                "q_i": f32hex(1),
                                "admission_id": admission,
                                "coldkey_group": identity.coldkey,
                                "state": "ACTIVE",
                                "eligible_weight": 4194304,
                            }
                        )
                    if not w:
                        beacon = now()
                        intent = RoundOpenV2(
                            w=0,
                            prev_final_hash="0" * 64,
                            theta_hash="0" * 64,
                            outer_state_hash="0" * 64,
                            center_hash="0" * 64,
                            roster_hash=sha256_hex(canonicalize(roster)),
                            honeypot_commit="0" * 64,
                            d_open=beacon + 1,
                            d_assign=beacon + 2,
                            d_commit=beacon + 100,
                            d_audit=beacon + 101,
                            d_upload=beacon + 180,
                            d_final=beacon + 200,
                            contract_version=2,
                            policy_hashes=PolicyHashes.model_validate(
                                {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
                            ),
                            registry_epoch=registry.epoch,
                            start_state_index_hash="0" * 64,
                            audit_mode="anchored-full",
                            roster=[RosterEntryV2.model_validate(row) for row in roster],
                        )
                        store._prepare_capacity_genesis_v2(
                            graph.run_id,
                            canonicalize(
                                envelope_v2.seal(
                                    coord, "RoundOpenV2", graph.run_id, intent, intent.d_final
                                )
                            ),
                            cancel=cancel,
                        )
                    genesis_dir = graph.state_dir / "capacity-genesis" / graph.run_id / "1"
                    genesis_request = GenesisRequest.model_validate_json(
                        (genesis_dir / "request.json").read_bytes()
                    )
                    genesis, blobs = collect_genesis(genesis_dir, genesis_request)
                    if genesis_request.manifest != manifest:
                        raise DriverError("continuation accepted genesis manifest differs")
                    theta, _ = unpack_state(blobs[genesis.starts[0].state_object_sha256])
                    for digest, payload in blobs.items():
                        if objects.get(digest) != payload:
                            raise DriverError("accepted genesis object custody differs")
                    state_hashes = genesis.outer_hashes
                    previous_final = "0" * 64
                    if w:
                        applied = record(graph.state_dir, "applied", str(w - 1), graph.run_id)
                        theta = {
                            k: torch.from_numpy(v.copy())
                            for k, v in load_state(objects, applied["out_state"]).theta.items()
                        }
                        previous_final = sha256_hex(
                            canonicalize(
                                record(graph.state_dir, "finalize", str(w - 1), graph.run_id)[
                                    "body"
                                ]
                            )
                        )
                        previous_tape = TapeV2.from_bytes(objects.get(applied["tape_hash"]))
                        state_hashes = previous_tape.body.out_hashes
                    starts = []
                    for identity, _ in identities:
                        status = store.admission_v2(
                            graph.run_id, "status", identity=identity.hotkey
                        )
                        if not status["eligible"] or status["record"]["state"] != "ACTIVE":
                            raise DriverError("continuation identity lacks actual graduation")
                        cache = AnchorCache()
                        anchor = (
                            store._restore_anchor_v2(graph.run_id, identity.hotkey, w - 1, cache)
                            if w
                            else cache.genesis(manifest, identity.hotkey, theta)
                        )
                        if anchor.state.step != w * cfg.inner.H:
                            raise DriverError("continuation carry step differs")
                        starts.append(
                            StartStateV2(
                                run_id=graph.run_id,
                                w=w,
                                hotkey=identity.hotkey,
                                theta_hash=state_hash(theta),
                                state_object_sha256=sha256_hex(pack_state(theta, anchor.state)),
                                opt_state_hash=optimizer_hash(anchor.state),
                                ef_object_sha256=sha256_hex(pack_state(anchor.ef)),
                                ef_hash=state_hash(anchor.ef),
                                parent_anchor_hash=anchor.anchor_hash,
                                global_step0=anchor.state.step,
                                anchor_verdict_hash=anchor.proof_hash,
                            )
                        )
                    beacon = now()
                    opening = RoundOpenV2(
                        w=w,
                        prev_final_hash=previous_final,
                        theta_hash=state_hash(theta),
                        outer_state_hash=state_hashes["outer_state_hash"],
                        center_hash=state_hashes["center_hash"],
                        roster_hash=sha256_hex(canonicalize(roster)),
                        honeypot_commit="0" * 64,
                        d_open=beacon + 1,
                        d_assign=beacon + 2,
                        d_commit=beacon + 100,
                        d_audit=beacon + 101,
                        d_upload=beacon + 180,
                        d_final=beacon + 200,
                        contract_version=2,
                        policy_hashes=PolicyHashes.model_validate(
                            {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
                        ),
                        registry_epoch=registry.epoch,
                        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
                        audit_mode="anchored-full",
                        roster=[RosterEntryV2.model_validate(row) for row in roster],
                    )
                    if not w:
                        intent = RoundOpenV2.model_validate(genesis_request.opening["body"])
                        if [r.body() for r in intent.roster] != roster:
                            raise DriverError("continuation prepared roster differs")
                        opening = intent.model_copy(
                            update={
                                "theta_hash": genesis.outer_hashes["theta_hash"],
                                "outer_state_hash": genesis.outer_hashes["outer_state_hash"],
                                "center_hash": genesis.outer_hashes["center_hash"],
                                "start_state_index_hash": sha256_hex(
                                    canonicalize([s.body() for s in genesis.starts])
                                ),
                            }
                        )

                    def miners() -> list[dict]:
                        entries = []
                        for identity, _ in identities:
                            value = store.island_job_v2(graph.run_id, w, identity.hotkey)
                            job = IslandJobV1.model_validate(value["job"])
                            directory = graph.evidence / f"live-{w}-{identity.hotkey}"
                            directory.mkdir(mode=0o700)
                            for rel, digest in value["objects"].items():
                                if Path(rel).is_absolute() or ".." in Path(rel).parts:
                                    raise DriverError("continuation accepted object path")
                                path = directory / rel
                                path.parent.mkdir(parents=True, exist_ok=True)
                                path.write_bytes(objects.get(digest))
                            binding = sha256_hex(
                                canonicalize(
                                    {
                                        "run_id": graph.run_id,
                                        "w": w,
                                        "hotkey": identity.hotkey,
                                        "assignment": store._record_v2(
                                            graph.run_id,
                                            "assignment",
                                            f"{w}:{identity.hotkey}",
                                        ),
                                    }
                                )
                            )
                            entries.append(materialize(identity, job, directory, "live", binding))
                        return entries

                    return {
                        "round_open": envelope_v2.seal(
                            coord, "RoundOpenV2", graph.run_id, opening, opening.d_final
                        ),
                        "miners": miners,
                        "auditors": [load_keyfile(p) for p in graph.auditor_key_files],
                        "accepted_inputs": lambda: record(
                            graph.state_dir, "tape-inputs", str(w), graph.run_id
                        )["inputs"],
                        "accepted_finality": lambda: record(
                            graph.state_dir, "finality", str(w), graph.run_id
                        ),
                    }

                return build

            lineage = decoder_live_graph(
                client,
                runtime,
                manifest,
                [round_context(0), round_context(1)],
                coord,
                admin,
                advance,
                objects,
                graph.evidence / "checkpoint",
                cancel,
            )
        intents = runtime.journal.all("network_execution_intent")
        if any(
            sum(r["executions"] for r in intents if r["role"] == role) != 58 for role in graph.hosts
        ):
            raise DriverError("continuation normal debit coverage differs")
        if cancel.is_set() or len(trials) != 48 or len(lineage) != 2:
            raise DriverError("continuation cancelled/incomplete accepted graph")
        operation_results = continuation_results(runtime.journal, graph.hosts)
        result = {
            "status": "SIGNED_GRAPH_SETTLED_PENDING_RELEASE_REVIEW",
            "run_id": graph.run_id,
            "graph_sha256": fsha(graph_path),
            "trial_finalizations": trials,
            "lineage": lineage,
            "normal_total": 116,
            "per_host": 58,
            "ceiling": 126,
            "per_host_ceiling": 63,
            "unused_faulttrace": 10,
            "fault_proof_complete": False,
            "review_sha256": sha256_hex(canonicalize(checked["record"])),
            "profile_sha256": fsha(checked["files"]["profile"]),
            "source_map_sha256": sha256_hex(canonicalize(graph.operation_sources)),
            "qualification_receipt_sha256": fsha(graph.qualification_receipt),
            "hosts": graph.hosts,
            "operation_receipts": runtime.journal.all("network_operation_authority"),
            "operation_results": operation_results,
            "checkpoint_manifest_sha256": fsha(graph.evidence / "checkpoint" / "MANIFEST.json"),
        }
        durable_json(graph.evidence / "result.json", result)
        return result
    finally:
        continuation_close(store, server, thread, watcher, cancel, done)


def decoder_context_plan(
    runtime: NetworkRuntime,
    owner: str,
    run_id: str,
    sources: dict,
    roles: dict,
    receipts: dict,
    beacon: int,
) -> Callable[[str, IslandJobV1, Path, dict], tuple[dict, dict | None, dict]]:
    """Inline trusted bootstrap: original service context plus preaccepted root receipts."""

    def plan(
        operation: str, job: IslandJobV1, directory: Path, context: Mapping
    ) -> tuple[dict, dict | None, dict]:
        if operation not in ("reference", "audit", "referee"):
            raise DriverError("service factory operation not reference/audit/referee")
        if TYPE_CHECKING:
            from scripts.network_gpu_operation import Operation
        else:
            from network_gpu_operation import Operation

        if job.run_id != run_id or context["run_id"] != run_id:
            raise DriverError("service context run differs from admitted bootstrap")
        hotkey = (
            context["hotkey"]
            if operation == "reference"
            else (
                context["audit_job"]["start_state"]["hotkey"]
                if operation == "audit"
                else context["turn"]["contest"]["miner"]
            )
        )
        host = roles[hotkey]
        frozen = runtime.owned_host(host["role"])
        if operation == "reference":
            signed = context["challenge_envelope"]
            env = envelope_v2.parse_envelope(signed)
            if (
                env.type != "JoinChallenge"
                or env.run_id != run_id
                or env.signer != job.manifest.training.coord_pubkey
                or not envelope_v2.verify_envelope(signed)
            ):
                raise DriverError("reference original challenge authority differs")
            challenge = JoinChallenge.model_validate(env.body)
            binding, kind, trace = challenge.admission_id, "trial", True
            if (
                binding != context["admission_id"]
                or challenge.body() != context["challenge"]
                or challenge.digest() != context["challenge_hash"]
                or job.w != context["epoch"]
                or challenge.layout != job.manifest.training.reference_spec.layout
            ):
                raise DriverError("reference accepted challenge differs")
            from hypertrain.data.trial_assignment import trial_assignment_hash

            if (
                challenge.assignment_hash
                != trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids))
                or env.exp_drand < challenge.deadline_beacon
            ):
                raise DriverError("reference original assignment/deadline differs")
            # Preserve original envelope bytes as canonical wire JSON; never re-sign it.
            wire = canonicalize(signed)
            path = directory / "original-join-challenge.json"
            if path.exists():
                if path.read_bytes() != wire:
                    raise DriverError("reference original challenge file conflicts")
            else:
                durable_write(path, wire)
            remote_challenge = "reference-context-" + job.digest() + "/original-join-challenge.json"
            challenge_files = {remote_challenge: sha256_hex(wire)}
        elif operation in ("audit", "referee"):
            descriptor = context["descriptor"]
            digest = sha256_hex(canonicalize(descriptor))
            envelope = context["descriptor_receipt"]
            if operation == "audit" and "receipt" in envelope:
                envelope = envelope["receipt"]
            signed = envelope_v2.parse_envelope(envelope)
            if (
                digest != context["descriptor_hash"]
                or signed.signer != job.manifest.training.coord_pubkey
                or signed.run_id != run_id
                or signed.type != "Receipt"
                or signed.body["commit_hash"] != digest
                or not envelope_v2.verify_envelope(envelope)
            ):
                raise DriverError("original accepted trace descriptor authority differs")
            if descriptor["job"] != job.body():
                raise DriverError("trace accepted job differs")
            binding, kind, trace = digest, "commit-lease", True
        else:
            raise DriverError("service factory operation not reference/audit/referee")
        spec = Operation.model_validate(
            {
                "operation": operation,
                "binding": binding,
                "hotkey": hotkey,
                "owner": owner,
                "role": host["role"],
                "instance_id": frozen["instance_id"],
                "machine_id": frozen["machine_id"],
                "job": job,
                "binding_kind": kind,
                "sources": sources,
                "image_digest": frozen["image_digest"],
                "cutoff": job.deadline,
                "trace": trace,
                "challenge": remote_challenge if operation == "reference" else None,
                "context_files": challenge_files if operation == "reference" else {},
            }
        )
        # Never sign an owner authorization here; root must supply exact reviewed context receipt.
        receipt = receipts[operation + ":" + job.digest()]
        return spec.model_dump(mode="json"), receipt, {"owner": owner, "beacon": beacon}

    return plan


def decoder_execution_count(profile: dict) -> dict:
    """No-kernel reconciliation; independent audit publishes its own trace once."""
    schedule = profile["schedule"]
    if (
        profile["model"]["arch"] != "decoder"
        or profile["inner"]["H"] != 30
        or profile["inner"]["J"] != 5
        or profile["layout"]["n_gpus"] != 2
        or schedule["logical_identities"] != 4
        or schedule["shadow_participations_per_identity"] != 12
        or schedule["executions_total"] != 126
        or schedule["executions_per_host"] != 63
        or schedule["shadow_executions"] != 96
        or schedule["live_executions"] != 16
        or schedule["probes"] != 2
        or schedule["fault_trace_reserve"] != 12
    ):
        raise DriverError("decoder original execution contract differs")
    shadow = 4 * 12 * 2  # Trusted reference + miner screen; staging has no training kernel.
    live = 4 * 2 * 2  # Miner + independently replayed and traced audit; no geometry rerun.
    qualification = 4
    faults_remaining = schedule["fault_trace_reserve"] - 2
    required = qualification + shadow + live + faults_remaining
    return {
        "status": "COUNT_FITS",
        "reason": "D2_EXECUTION_COUNT_ONLY",
        "qualification": qualification,
        "shadow": shadow,
        "live": live,
        "faulttrace_remaining": faults_remaining,
        "required_total": required,
        "required_per_host": required // 2,
        "ceiling": 126,
        "per_host_ceiling": 63,
        "excess": required - 126,
        "hot_roles": {"h0": [0, 1], "h1": [2, 3]},
        "backend_authority": "NONE",
        "kernels_executed": 0,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument("--config", type=Path)
    inputs.add_argument("--decoder-count-profile", type=Path)
    args = parser.parse_args(argv)
    if args.decoder_count_profile is not None:
        print(
            json.dumps(decoder_execution_count(json.loads(args.decoder_count_profile.read_bytes())))
        )
        return 0
    settings = Settings.model_validate_json(args.config.read_bytes())
    cancel = threading.Event()

    def stop(signum: int, frame: FrameType | None) -> None:
        cancel.set()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    owned = not settings.evidence.exists()
    completed = False
    try:
        proof(settings, cancel)
        completed = True
    finally:
        if owned and settings.evidence.is_dir():
            workload_error = sys.exception()
            try:
                finalize_evidence(
                    settings, completed, str(workload_error) if workload_error else None
                )
            except (OSError, sqlite3.Error, DriverError) as rescue_error:
                if workload_error is None:
                    raise
                workload_error.add_note(f"evidence rescue error: {rescue_error}")
                print(f"evidence rescue error: {rescue_error}", file=sys.stderr)
    print((settings.evidence / "result.json").read_text())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
