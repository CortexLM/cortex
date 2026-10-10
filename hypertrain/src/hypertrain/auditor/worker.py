"""Auditor worker: lease an audit job, replay it, post a signed ReplayVerdict.

Challenge routes (bearer = worker token; paths and lease/heartbeat/complete/fail bodies follow
docs/challenge-routes.md; the job payload, serves and objects routes are this client's needs):
  POST /v1/worker/lease                      -> 204 | 200 Job JSON (see ``Job``)
  POST /v1/worker/jobs/{id}/heartbeat        {lease}                 -> 200
  GET  /v1/worker/jobs/{id}/serves?lease=    -> {now_round, serves: [{serve, blob_sha256}]}
  GET  /v1/objects/{sha256}                  -> raw bytes (content-addressed, sha256 checked)
  POST /v1/worker/jobs/{id}/complete         {lease, verdict: ht/1 ReplayVerdict}  -> 200
  POST /v1/worker/jobs/{id}/fail             {lease, reason, retry}  -> 200
"""

from __future__ import annotations

import hashlib
import threading
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal, Protocol

import httpx

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import (
    AnchorCache,
    AuditInputError,
    AuditInputs,
    NotReady,
    Outcome,
    ServedState,
    audit_full,
    audit_segments,
    committed_norms,
    make_verdict,
    select_segments,
    unpack_state,
)
from hypertrain.gpu_ops.work_screen import IslandLaunch
from hypertrain.miner.island_launch import IslandArtifacts, confined, launch_island
from hypertrain.protocol.envelope import seal
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import (
    AuditChallenge,
    Commit,
    LeafPreimage,
    ReplayEnv,
    RunManifest,
    StateServe,
)
from hypertrain.protocol.messages_v2 import AuditJobV2, CommitV2, IslandJobV1
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, Params, SampleFn


class LeaseGuard:
    """L0 verified-beacon subscriber calls advance/revoke; trainer checks every boundary.

    A local hard timer also cancels torchrun while blocked inside a collective. No renewals.
    """

    def __init__(self, lease_expires: int, absolute_deadline: int, deadline_unix: float) -> None:
        self.expires = min(lease_expires, absolute_deadline)
        self.cancelled = threading.Event()
        import time

        self.timer = threading.Timer(max(0.0, deadline_unix - time.time()), self.cancelled.set)
        self.timer.daemon = True

    def __enter__(self) -> LeaseGuard:
        self.timer.start()
        return self

    def __exit__(self, *args: Any) -> None:
        self.timer.cancel()

    def advance(self, verified_beacon: int) -> None:
        if verified_beacon >= self.expires:
            self.cancelled.set()

    def revoke(self) -> None:
        self.cancelled.set()

    def check(self) -> None:
        if self.cancelled.is_set():
            raise AuditInputError("audit lease expired or revoked")


def execute_island_audit(
    job: AuditJobV2,
    directory: Path,
    cache: AnchorCache,
    guard: LeaseGuard,
    *,
    now_round: int,
    deadline_unix: int,
    backend: Literal["cpu", "cuda"] = "cuda",
    publication_job: IslandJobV1 | None = None,
    publication_directory: Path | None = None,
    launch: IslandLaunch | None = None,
) -> tuple[Outcome, IslandArtifacts]:
    """Actual torchrun audit runtime; L0 stages authenticated objects and sample proofs.

    Subscribe guard.advance to verified beacon events before invoking; no heartbeat renewal.
    directory contains start_state, ef_in, v0, samples, sample_proofs. No HTTP route invented.
    """
    import json

    from hypertrain.auditor.replay import VerifiedAnchor, optimizer_hash, pack_state
    from hypertrain.gpu_ops.journal import durable_write
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.trainer.compress import state_hash

    job.validate_embedded(now_round)
    guard.check()
    start = job.start_state
    prior = cache.prior(job.manifest, start)
    if prior.backend not in ("genesis", backend):
        raise AuditInputError("CPU anchor cannot serve as CUDA replay oracle")
    theta_blob = confined(directory, "start_state").read_bytes()
    ef_blob = confined(directory, "ef_in").read_bytes()
    v0_blob = confined(directory, "v0").read_bytes()
    theta, st = unpack_state(theta_blob)
    ef, _ = unpack_state(ef_blob)
    if (
        sha256_hex(theta_blob) != start.state_object_sha256
        or st is None
        or optimizer_hash(st) != optimizer_hash(prior.state)
        or state_hash(theta) != start.theta_hash
        or (job.manifest.training.inner.state_policy == "carry" and st.step != start.global_step0)
        or sha256_hex(ef_blob) != job.ef_in.sha256
        or len(ef_blob) != job.ef_in.size
        or sha256_hex(ef_blob) != start.ef_object_sha256
        or state_hash(ef) != start.ef_hash
        or sha256_hex(v0_blob) != job.v0.sha256
        or len(v0_blob) != job.v0.size
    ):
        raise AuditInputError("untrusted audit start/state/EF")
    start_path = "start_state"
    start_hash = start.state_object_sha256
    if job.manifest.training.inner.state_policy != "carry":
        # The authenticated anchor still includes m/v; reset/derived kernels take theta only.
        start_path = "audit-theta.safetensors"
        packed = pack_state(theta)
        start_hash = sha256_hex(packed)
        path = directory / start_path
        if path.exists():
            if confined(directory, start_path).read_bytes() != packed:
                raise AuditInputError("conflicting prepared audit theta")
        else:
            durable_write(path, packed)
    local = IslandJobV1(
        job_version=1,
        run_id=job.run_id,
        w=start.w,
        manifest=job.manifest,
        sample_ids=job.sample_ids,
        global_step0=start.global_step0,
        start_state_sha256=start_hash,
        ef_in_sha256=job.ef_in.sha256,
        v0_sha256=job.v0.sha256,
        deadline=deadline_unix,
        object_paths={
            "start_state": start_path,
            **{k: k for k in ("ef_in", "v0", "samples", "sample_proofs")},
        },
    )
    if publication_job is not None:
        if publication_directory is None or publication_job != local.model_copy(
            update={"object_paths": publication_job.object_paths}
        ):
            raise AuditInputError("accepted audit publication job differs from independent replay")
        for name, relative in publication_job.object_paths.items():
            expected = (
                pack_state(theta)
                if name == "start_state" and start_path != "start_state"
                else confined(directory, name).read_bytes()
            )
            if confined(publication_directory, relative).read_bytes() != expected:
                raise AuditInputError("accepted audit publication input differs")
        local, directory = publication_job, publication_directory
    elif publication_directory is not None:
        raise AuditInputError("publication directory lacks accepted descriptor")
    artifacts = (launch or launch_island)(
        local, directory, backend=backend, cancel=guard.cancelled, trace=True
    )
    if launch is not None:
        from hypertrain.miner.island_launch import validate_artifacts

        artifacts = validate_artifacts(local, artifacts.directory)
    from hypertrain.auditor.island_bisect import IslandParty

    IslandParty.published(start.hotkey, local, artifacts.directory)
    guard.check()
    commit = CommitV2.model_validate(job.commit_envelope["body"])
    summary = json.loads((artifacts.directory / "rank-0/summary.json").read_bytes())["commitments"]
    leaves = [LeafPreimage.model_validate(x) for x in json.loads(artifacts.leaves.read_bytes())]
    first = next(
        (i for i, (a, b) in enumerate(zip(job.preimages, leaves, strict=True)) if a != b), None
    )
    matches = (
        first is None
        and summary["leaves_root"] == commit.leaves_root
        and summary["delta_hash"] == commit.delta_hash
        and summary["final_theta_hash"] == commit.final_theta_hash
        and summary["ef_out_hash"] == commit.ef_out_hash
        and state_hash(ef) == commit.ef_in_hash
    )
    outcome = Outcome("MATCH" if matches else "MISMATCH", first, summary["leaves_root"])
    if matches:
        theta, st = unpack_state(artifacts.state.read_bytes())
        ef, _ = unpack_state(artifacts.ef.read_bytes())
        assert st is not None
        proof = sha256_hex(
            bytes.fromhex(job.job_id + summary["leaves_root"] + summary["delta_hash"])
        )
        anchor_hash = sha256_hex(bytes.fromhex(start.digest() + proof))
        layout = cache.layout_hash(job.manifest)
        cache.entries[(job.run_id, commit.hotkey, commit.w, layout)] = VerifiedAnchor(
            job.run_id, commit.hotkey, commit.w, layout, st, ef, proof, anchor_hash, theta, backend
        )
    return outcome, artifacts


HEARTBEAT_SECONDS = 300.0
VERDICT_TTL_ROUNDS = 1200


class ChallengeApi(Protocol):
    def lease(self) -> dict[str, Any] | None: ...

    def heartbeat(self, job: str, lease: str) -> None: ...

    def serves(self, job: str, lease: str) -> dict[str, Any]: ...

    def object(self, sha256: str) -> bytes: ...

    def verdict(self, job: str, lease: str, envelope: dict[str, Any]) -> dict[str, Any]: ...

    def fail(self, job: str, lease: str, reason: str, retry: bool) -> None: ...


class HttpApi:
    def __init__(self, client: httpx.Client, token: str, base: str = "") -> None:
        self.client, self.base = client, base.rstrip("/")
        self.headers = {"authorization": f"Bearer {token}"}

    def _call(self, method: str, path: str, **kw: Any) -> httpx.Response:
        r = self.client.request(method, self.base + path, headers=self.headers, timeout=60, **kw)
        if r.status_code >= 400:
            raise RuntimeError(f"{method} {path}: {r.status_code} {r.text[:300]}")
        return r

    def lease(self) -> dict[str, Any] | None:
        r = self._call("POST", "/v1/worker/lease")
        return None if r.status_code == 204 else dict(r.json())

    def heartbeat(self, job: str, lease: str) -> None:
        self._call("POST", f"/v1/worker/jobs/{job}/heartbeat", json={"lease": lease})

    def serves(self, job: str, lease: str) -> dict[str, Any]:
        return dict(
            self._call("GET", f"/v1/worker/jobs/{job}/serves", params={"lease": lease}).json()
        )

    def object(self, sha256: str) -> bytes:
        data = self._call("GET", f"/v1/objects/{sha256}").content
        if hashlib.sha256(data).hexdigest() != sha256:
            raise RuntimeError(f"object {sha256} failed its sha256 check")
        return data

    def verdict(self, job: str, lease: str, envelope: dict[str, Any]) -> dict[str, Any]:
        body = {"lease": lease, "verdict": envelope}
        return dict(self._call("POST", f"/v1/worker/jobs/{job}/complete", json=body).json())

    def fail(self, job: str, lease: str, reason: str, retry: bool) -> None:
        body = {"lease": lease, "reason": reason[:500], "retry": retry}
        self._call("POST", f"/v1/worker/jobs/{job}/fail", json=body)


@dataclass(frozen=True)
class Job:
    """Job JSON: {id, lease, manifest (RunManifest body), challenge (AuditChallenge body),
    commit (Commit body), leaves [hex], preimages [LeafPreimage body],
    assignment {sample_ids, global_step0}, theta_start_sha256, ef_in_sha256|null,
    v0_sha256|null}. Tensor blobs are safetensors packed as ``replay.pack_state``."""

    job: str
    lease: str
    manifest: RunManifest
    challenge: AuditChallenge
    commit: Commit
    leaves: list[str]
    preimages: list[LeafPreimage]
    assignment: Assignment
    blobs: Mapping[str, str | None]

    @classmethod
    def parse(cls, raw: Mapping[str, Any]) -> Job:
        manifest = RunManifest.model_validate(raw["manifest"])
        a = raw["assignment"]
        ids = a["sample_ids"]
        if not isinstance(ids, list) or any(type(i) is not int or i < 0 for i in ids):
            raise AuditInputError("assignment.sample_ids must be non-negative ints")
        if type(a["global_step0"]) is not int or a["global_step0"] < 0:
            raise AuditInputError("assignment.global_step0 must be a non-negative int")
        challenge = AuditChallenge.model_validate(raw["challenge"])
        return cls(
            job=str(raw["id"]),
            lease=str(raw["lease"]),
            manifest=manifest,
            challenge=challenge,
            commit=Commit.model_validate(raw["commit"]),
            leaves=[str(x) for x in raw["leaves"]],
            preimages=[LeafPreimage.model_validate(p) for p in raw["preimages"]],
            assignment=Assignment(manifest.run_id(), challenge.w, tuple(ids), a["global_step0"]),
            blobs={k: raw.get(k) for k in ("theta_start_sha256", "ef_in_sha256", "v0_sha256")},
        )


class Auditor:
    def __init__(
        self,
        api: ChallengeApi,
        keypair: Keypair,
        env: ReplayEnv,
        get_sample: SampleFn,
        heartbeat_seconds: float = HEARTBEAT_SECONDS,
    ) -> None:
        self.api, self.keypair, self.env, self.get = api, keypair, env, get_sample
        self.heartbeat_seconds = heartbeat_seconds

    def _params(self, sha: str | None) -> Params | None:
        if sha is None:
            return None
        theta, _ = unpack_state(self.api.object(sha))
        return theta

    def _outcome(self, job: Job) -> Outcome:
        cfg = TrainConfig.from_manifest(job.manifest.body())
        theta = self._params(job.blobs["theta_start_sha256"])
        if theta is None:
            raise AuditInputError("job has no theta_start_sha256")
        x = AuditInputs(
            cfg=cfg,
            run_id=job.manifest.run_id(),
            challenge=job.challenge,
            commit=job.commit,
            assignment=job.assignment,
            leaves=job.leaves,
            preimages=job.preimages,
            theta_start=theta,
            ef_in=self._params(job.blobs["ef_in_sha256"]),
            v0=self._params(job.blobs["v0_sha256"]),
        )
        if job.challenge.mode == "full":
            return audit_full(x, self.get)
        got = self.api.serves(job.job, job.lease)
        serves: dict[int, ServedState | None] = {}
        for row in got["serves"]:
            s = StateServe.model_validate(row["serve"])
            if s.t % cfg.inner.J == 0:
                serves[s.t // cfg.inner.J] = ServedState(s, self.api.object(row["blob_sha256"]))
        v = job.manifest.verify
        expected = None
        if len(job.preimages) == cfg.inner.n_leaves:
            expected = select_segments(
                x.run_id,
                job.challenge.w,
                job.challenge.beacon_sig_sha256,
                job.challenge.target,
                committed_norms(job.preimages),
                v.k_segments,
                v.Q_top,
            )
        return audit_segments(x, self.get, serves, int(got["now_round"]), expected)

    def run_once(self) -> str | None:
        """Lease and settle one job; returns the posted result, "FAILED", or None when idle."""
        raw = self.api.lease()
        if raw is None:
            return None
        job_id, lease = str(raw.get("id")), str(raw.get("lease"))
        stop = threading.Event()
        beat = threading.Thread(target=self._beat, args=(job_id, lease, stop), daemon=True)
        beat.start()
        try:
            job = Job.parse(raw)
            out = self._outcome(job)
            verdict = make_verdict(job.challenge, out, self.env)
            exp = job.challenge.serve_deadline + VERDICT_TTL_ROUNDS
            env = seal(self.keypair, "ReplayVerdict", job.manifest.run_id(), verdict, exp)
            self.api.verdict(job_id, lease, env)
            return out.result
        except NotReady as error:
            self.api.fail(job_id, lease, str(error), retry=True)
        except (AuditInputError, KeyError, ValueError) as error:
            self.api.fail(job_id, lease, f"bad job: {error!r}", retry=False)
        except Exception as error:  # noqa: BLE001 - infrastructure: reported, retried
            self.api.fail(job_id, lease, repr(error), retry=True)
        finally:
            stop.set()
            beat.join()
        return "FAILED"

    def _beat(self, job: str, lease: str, stop: threading.Event) -> None:
        while not stop.wait(self.heartbeat_seconds):
            try:
                self.api.heartbeat(job, lease)
            except RuntimeError:
                return
