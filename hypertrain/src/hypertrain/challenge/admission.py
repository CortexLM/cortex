"""Bounded, persisted, shadow admission backed by actual full-round references."""

from __future__ import annotations

import hmac
import secrets
import threading
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Literal

from pydantic import JsonValue

if TYPE_CHECKING:
    from experiments.gpu_network_v2.orchestrate import NetworkRuntime
    from scripts.network_gpu_operation import Operation

from hypertrain.beacon.core import BeaconRound
from hypertrain.challenge.admission_store import AdmissionError, AdmissionRecord, AdmissionStore
from hypertrain.challenge.trust_v2 import FundedStatus, nominal_q
from hypertrain.data.store import Store
from hypertrain.data.trial_assignment import trial_assignment_hash, trial_samples
from hypertrain.gpu_ops.work_screen import IslandLaunch, screen_work
from hypertrain.ledger.escrow_v2 import EscrowV2, digest
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.envelope_v2 import Intake, load_json, parse_envelope, seal, verify_join
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    EscrowLock,
    IslandJobV1,
    JoinChallenge,
    JoinRequest,
    RotateRequest,
    WorkProof,
    WorkScreenV2,
)


@dataclass(frozen=True, slots=True)
class AdmissionStatus:
    record: AdmissionRecord
    funding: FundedStatus
    nominal_q_ppm: int
    actual_q_ppm: int
    eligible: bool
    shadow_only: bool


class Admission:
    """Trusted service API, not routes. L0 supplies current verified beacons and jobs.

    Job staging authenticates genesis/carry starts and exact-image qualification.
    Default CUDA references; explicit CPU test mode never qualifies production.
    """

    def __init__(
        self,
        escrow: EscrowV2,
        policy: AdmissionPolicyV2,
        coordinator: Keypair,
        *,
        beacon: Callable[[int], BeaconRound],
        stage_reference: Callable[[JoinChallenge, tuple[int, ...], int], tuple[IslandJobV1, Path]],
        objects: Store,
        ip_secret: bytes,
        qualified: Callable[[WorkScreenV2], bool],
        conservative_bound: Callable[[], bool],
        backend: Literal["cpu", "cuda"] = "cuda",
    ) -> None:
        if digest(policy) != escrow.manifest.network.admission_policy_hash:
            raise AdmissionError("ADMISSION_POLICY_HASH")
        if coordinator.ss58 != escrow.manifest.training.coord_pubkey:
            raise AdmissionError("COORDINATOR_AUTHORITY")
        if backend == "cpu" and escrow.policy.ledger_mode != "test":
            raise AdmissionError("CPU_NOT_PRODUCTION_QUALIFICATION")
        self.escrow, self.policy, self.coord = escrow, policy, coordinator
        self.store = AdmissionStore(escrow)
        self.beacon, self.stage_reference, self.objects = beacon, stage_reference, objects
        self.ip_secret, self.qualified = ip_secret, qualified
        self.conservative_bound, self.backend = conservative_bound, backend

    def _beacon(self, now: int) -> BeaconRound:
        b = self.beacon(now)
        if b.round != now or (
            self.escrow.policy.ledger_mode == "production" and not b.bls_verified
        ):
            raise AdmissionError("UNVERIFIED_BEACON")
        return b

    def join(self, raw: bytes, *, now: int, ip_prefix: str) -> AdmissionRecord:
        self._beacon(now)
        request = JoinRequest.model_validate(load_json(raw, max_bytes=65536))
        if not verify_join(request, self.escrow.run_id, now):
            raise AdmissionError("DUAL_SIGNATURE_RUN_OR_EXPIRY")
        if request.policy_hash != digest(self.policy):
            raise AdmissionError("POLICY_CHANGED")
        key = f"join|{self.escrow.run_id}|{request.coldkey}|{request.request_id}"
        request_hash = body_digest(request.model_dump(mode="json", exclude={"hot_sig", "cold_sig"}))
        with self.escrow.tx():
            old = self.store.db.execute(
                "SELECT * FROM admission_reservations WHERE reservation=?", (key,)
            ).fetchone()
            if old:
                self.store.reserve(key, request_hash, old["receipt"])
                return self.store.by_id(old["receipt"])
            if self.store.db.execute(
                "SELECT 1 FROM admission_history WHERE hotkey=?", (request.hotkey,)
            ).fetchone():
                raise AdmissionError("HOTKEY_HISTORY_EXISTS")
            if self.store.db.execute(
                "SELECT 1 FROM admissions_v2 WHERE coldkey=? AND state!='ACTIVE'",
                (request.coldkey,),
            ).fetchone():
                raise AdmissionError("COLDKEY_PROBATION_EXISTS")
            count = self.store.db.execute(
                "SELECT COUNT(*) FROM admissions_v2 WHERE state IN ('APPLIED','PROBATION')"
            ).fetchone()[0]
            if count >= self.policy.max_pending:
                raise AdmissionError("PENDING_CAPACITY")
            for identity in (request.hotkey, request.coldkey):
                self.store.quota(identity, now, self.policy.join_per_beacon, self.policy.join_burst)
            epoch = now // self.policy.work_screen_epoch_rounds
            prefix = hmac.digest(self.ip_secret, f"{epoch}|{ip_prefix}".encode(), "sha256").hex()
            self.store.db.execute(
                "DELETE FROM admission_quota WHERE identity LIKE 'ip:%' AND beacon<?",
                (epoch * self.policy.work_screen_epoch_rounds,),
            )
            self.store.quota(
                f"ip:{prefix}", now, self.policy.ip_prefix_per_beacon, self.policy.ip_prefix_burst
            )
            admission_id = sha256_hex(key.encode())
            self.store.db.execute(
                """INSERT INTO admissions_v2(admission_id,run_id,hotkey,coldkey,state,
                    reason,policy_hash,received_beacon,seed_beacon)
              VALUES(?,?,?,?,?,?,?,?,?)""",
                (
                    admission_id,
                    self.escrow.run_id,
                    request.hotkey,
                    request.coldkey,
                    "APPLIED",
                    "JOIN_ACCEPTED",
                    request.policy_hash,
                    now,
                    now + 1,
                ),
            )
            self.store.db.execute(
                "INSERT INTO admission_history VALUES(?,?,?)",
                (request.hotkey, admission_id, request.coldkey),
            )
            self.store.reserve(key, request_hash, admission_id)
            return self.store.by_id(admission_id)

    def challenge(self, admission_id: str, *, now: int) -> dict[str, JsonValue]:
        self._beacon(now)
        with self.escrow.tx():
            r = self.store.by_id(admission_id)
            row = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
            ).fetchone()
            if r.state not in ("APPLIED", "PROBATION", "ACTIVE") or r.pending_dispute:
                raise AdmissionError("CHALLENGE_STATE")
            if row["challenge"]:
                c = JoinChallenge.model_validate_json(row["challenge"])
                trial = self.store.db.execute(
                    "SELECT outcome FROM admission_trial_results WHERE epoch=?",
                    (row["trial_epoch"],),
                ).fetchone()
                if trial and trial["outcome"] == "OPEN":
                    if now > c.deadline_beacon:
                        self.store.transition(admission_id, "SUSPENDED", "CHALLENGE_EXPIRED", now)
                        self.store.db.execute(
                            "UPDATE admission_trial_results SET outcome='EXPIRED' WHERE epoch=?",
                            (row["trial_epoch"],),
                        )
                    reservation = self.store.db.execute(
                        "SELECT receipt FROM admission_reservations WHERE reservation=?",
                        (f"challenge|{admission_id}|{c.nonce}",),
                    ).fetchone()
                    if reservation is None:
                        raise AdmissionError("MISSING_CHALLENGE_RECEIPT")
                    return load_json(reservation["receipt"])
            seed = row["seed_beacon"]
            if now < seed:
                raise AdmissionError("SEED_NOT_READY")
            if now > seed + self.policy.challenge_rounds:
                raise AdmissionError("CHALLENGE_EXPIRED")
            b = self._beacon(seed)
            running = self.store.db.execute(
                "SELECT COUNT(*) FROM admission_trial_results WHERE outcome='OPEN'"
            ).fetchone()[0]
            if running >= self.policy.max_trial_replays:
                raise AdmissionError("TRIAL_CAPACITY")
            trial_epoch = self.store.db.execute(
                "SELECT COALESCE(MAX(epoch),-1)+1 FROM admission_trials"
            ).fetchone()[0]
            nonce = secrets.token_hex(32)
            samples = trial_samples(self.escrow.manifest, admission_id, nonce, b)
            c = JoinChallenge(
                admission_id=admission_id,
                nonce=nonce,
                seed_beacon=seed,
                deadline_beacon=seed + 100,
                manifest_hash=self.escrow.run_id,
                theta_hash=self.escrow.manifest.training.init_state_hash,
                assignment_hash=trial_assignment_hash(self.escrow.manifest, trial_epoch, samples),
                layout=self.escrow.manifest.training.reference_spec.layout,
                artifact_limits=self.policy.artifact_limits,
                policy_hash=digest(self.policy),
            )
            self.store.db.execute(
                "INSERT INTO "
                "admission_trials(epoch,admission_id,nonce,received_beacon) "
                "VALUES(?,?,?,?)",
                (trial_epoch, admission_id, nonce, now),
            )
            self.store.db.execute(
                "INSERT INTO admission_trial_results(epoch,challenge) VALUES(?,?)",
                (trial_epoch, c.model_dump_json()),
            )
            self.store.db.execute(
                "UPDATE admissions_v2 SET "
                "challenge=?,trial_epoch=?,proof_digest=NULL,"
                "reference_commitment=NULL,reference_json=NULL "
                "WHERE admission_id=?",
                (c.model_dump_json(), trial_epoch, admission_id),
            )
            envelope = seal(self.coord, "JoinChallenge", self.escrow.run_id, c, c.deadline_beacon)
            from hypertrain.protocol.jcs import canonicalize

            self.store.reserve(
                f"challenge|{admission_id}|{c.nonce}", c.digest(), canonicalize(envelope).decode()
            )
            return envelope

    def prepare_reference(
        self,
        admission_id: str,
        *,
        now: int,
        trusted_launch: Callable[[IslandJobV1, Path, JoinChallenge, int], IslandLaunch]
        | None = None,
        cancel: threading.Event | None = None,
        retain_shadow_custody: bool = False,
    ) -> str:
        """Execute independently from trusted start; commit before accepting newcomer proof."""
        self._beacon(now)
        row = self.store.db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
        ).fetchone()
        if row is None or row["challenge"] is None:
            raise AdmissionError("NO_CHALLENGE")
        c = JoinChallenge.model_validate_json(row["challenge"])
        trial = self.store.db.execute(
            "SELECT outcome FROM admission_trial_results WHERE epoch=?", (row["trial_epoch"],)
        ).fetchone()
        if trial is None or trial["outcome"] != "OPEN":
            raise AdmissionError("REFERENCE_TRIAL_CLOSED")
        if row["reference_commitment"] is not None:
            return str(row["reference_commitment"])
        samples = trial_samples(
            self.escrow.manifest, admission_id, c.nonce, self._beacon(c.seed_beacon)
        )
        job, directory = self.stage_reference(c, samples, row["trial_epoch"])
        if tuple(job.sample_ids) != samples or job.w != row["trial_epoch"]:
            raise AdmissionError("REFERENCE_ASSIGNMENT")
        if trusted_launch is None:
            screen, proof = screen_work(job, directory, c, now_beacon=now, backend=self.backend)
        else:
            screen, proof = screen_work(
                job,
                directory,
                c,
                now_beacon=now,
                backend=self.backend,
                launch=trusted_launch(job, directory, c, row["trial_epoch"]),
                cancel=cancel,
            )
        if not self.qualified(screen):
            raise AdmissionError("UNQUALIFIED_REFERENCE")
        for name, ref in zip(
            ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json"),
            proof.artifact_refs,
            strict=True,
        ):
            data = (directory / "published" / "rank-0" / name).read_bytes()
            if self.objects.put(data) != ref.sha256:
                raise AdmissionError("REFERENCE_OBJECT_HASH")
        with self.escrow.tx():
            current = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
            ).fetchone()
            if current["challenge"] != c.model_dump_json() or current["proof_digest"] is not None:
                raise AdmissionError("REFERENCE_STATE_CHANGED")
            commitment = digest(proof)
            self.store.db.execute(
                "UPDATE admissions_v2 SET "
                "reference_commitment=?,reference_json=?,reference_beacon=? WHERE "
                "admission_id=?",
                (commitment, proof.model_dump_json(), now, admission_id),
            )
            self.store.db.execute(
                "UPDATE admission_trial_results SET reference=? WHERE epoch=?",
                (proof.model_dump_json(), current["trial_epoch"]),
            )
            if retain_shadow_custody:
                self.retain_trial_publication("reference", job, directory, screen, proof, now)
            return commitment

    def retain_runtime_custody(
        self,
        runtime: NetworkRuntime,
        spec: Operation,
        directory: Path,
        *,
        tree: Path,
        now: int,
        cancel: threading.Event | None = None,
    ) -> dict[str, str]:
        """Retain authenticated internal completion only; never production acceptance."""
        from experiments.gpu_network_v2.orchestrate import NetworkRuntime

        from hypertrain.protocol.jcs import canonicalize

        if type(runtime) is not NetworkRuntime:
            raise AdmissionError("SHADOW_TRUSTED_RUNTIME_REQUIRED")
        self._beacon(now)
        with self.escrow.tx():
            descriptor = NetworkRuntime.completion_custody(
                runtime, spec, directory, tree=tree, cancel=cancel
            )
            body = descriptor["body"]
            if descriptor["sha256"] != sha256_hex(canonicalize(body)):
                raise AdmissionError("SHADOW_RUNTIME_DESCRIPTOR_HASH")
            if spec.job.manifest != self.escrow.manifest:
                raise AdmissionError("SHADOW_RUNTIME_MANIFEST")
            row = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=? AND run_id=?",
                (spec.binding, self.escrow.run_id),
            ).fetchone()
            if row is None or row["challenge"] is None or row["reference_json"] is None:
                raise AdmissionError("SHADOW_RUNTIME_REFERENCE_NOT_ACCEPTED")
            publication = body["publication"]
            challenge = JoinChallenge.model_validate_json(row["challenge"])
            signed_challenge = self.store.db.execute(
                "SELECT receipt FROM admission_reservations WHERE reservation=?",
                (f"challenge|{spec.binding}|{challenge.nonce}",),
            ).fetchone()
            if (
                row["hotkey"] != spec.hotkey
                or row["trial_epoch"] != spec.job.w
                or publication["trial_authority"]["challenge_hash"] != challenge.digest()
                or signed_challenge is None
                or Intake(self.escrow.run_id, {"JoinChallenge": self.coord.ss58.__eq__}).accept(
                    signed_challenge["receipt"], challenge.seed_beacon
                )
                != challenge
                or not challenge.seed_beacon <= now <= challenge.deadline_beacon
                or row["reference_commitment"]
                != digest(WorkProof.model_validate_json(row["reference_json"]))
            ):
                raise AdmissionError("SHADOW_RUNTIME_ACCEPTED_CONTEXT")
            accepted = self.store.db.execute(
                "SELECT * FROM admission_trial_results WHERE epoch=?", (spec.job.w,)
            ).fetchone()
            reference = WorkProof.model_validate_json(row["reference_json"])
            if accepted is None or accepted["reference"] != row["reference_json"]:
                raise AdmissionError("SHADOW_RUNTIME_REFERENCE_NOT_ACCEPTED")
            if spec.operation == "probe":
                if accepted["proof"] is None or accepted["screen"] is None:
                    raise AdmissionError("SHADOW_RUNTIME_PROOF_NOT_ACCEPTED")
                proof = Intake(self.escrow.run_id, {"WorkProof": spec.hotkey.__eq__}).accept(
                    accepted["proof"], now
                )
                screen = Intake(self.escrow.run_id, {"WorkScreenV2": spec.hotkey.__eq__}).accept(
                    accepted["screen"], now
                )
                assert isinstance(proof, WorkProof) and isinstance(screen, WorkScreenV2)
                if (
                    proof != reference
                    or proof
                    != WorkProof.model_validate(publication["miner_proof"]["envelope"]["body"])
                    or screen.challenge_hash != challenge.digest()
                    or screen.nonce != challenge.nonce
                    or screen.admission_id != spec.binding
                    or screen.artifact_hashes != [ref.sha256 for ref in proof.artifact_refs]
                    or screen.layout != challenge.layout
                    or screen.policy_hash != challenge.policy_hash
                    or screen.image_digest != spec.image_digest
                    or [rank.model_dump(mode="json") for rank in screen.rank_results]
                    != [summary["work"] for summary in publication["rank_summaries"]]
                    or not self.qualified(screen)
                ):
                    raise AdmissionError("SHADOW_RUNTIME_ACCEPTED_PROOF")
            else:
                summaries = publication["rank_summaries"]
                if (
                    reference.challenge_hash != challenge.digest()
                    or reference.leaves_root != summaries[0]["commitments"]["leaves_root"]
                    or reference.delta_hash != summaries[0]["commitments"]["delta_hash"]
                    or any(
                        publication["files"]["rank-0/" + name]["sha256"] != ref.sha256
                        or publication["files"]["rank-0/" + name]["bytes"] != ref.size
                        for name, ref in zip(
                            ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json"),
                            reference.artifact_refs,
                            strict=True,
                        )
                    )
                ):
                    raise AdmissionError("SHADOW_RUNTIME_ACCEPTED_REFERENCE")
            # Internal custody is not accepted backend/economic authority or shadow-execution.
            key = f"{spec.binding}|{spec.job.w}|{spec.operation}"
            raw = canonicalize(descriptor).decode()
            old = self.store.db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='runtime-custody' AND id=?",
                (self.escrow.run_id, key),
            ).fetchone()
            if old is not None and old["data"] != raw:
                raise AdmissionError("SHADOW_RUNTIME_CUSTODY_CONFLICT")
            if self.objects.put(canonicalize(body)) != descriptor["sha256"]:
                raise AdmissionError("SHADOW_RUNTIME_OBJECT_HASH")
            self.store.db.execute(
                "INSERT OR IGNORE INTO records_v2 VALUES(?,?,?,?)",
                (self.escrow.run_id, "runtime-custody", key, raw),
            )
            return {"custody_hash": descriptor["sha256"], "status": "NOTSTOREACCEPTED"}

    def retain_trial_publication(
        self,
        operation: Literal["reference", "probe"],
        job: IslandJobV1,
        directory: Path,
        screen: WorkScreenV2,
        proof: WorkProof,
        now: int,
        *,
        commit_raw: bytes | None = None,
    ) -> dict[str, str]:
        """Internal completion seam only; uploaded objects never create this record."""
        from hypertrain.miner.island_launch import confined, validate_artifacts
        from hypertrain.protocol.jcs import canonicalize
        from hypertrain.protocol.messages_v2 import CommitV2

        self._beacon(now)
        if self.backend != "cpu" or self.escrow.policy.ledger_mode != "test":
            raise AdmissionError("SHADOW_ACCEPTED_RUNTIME_CUSTODY_UNAVAILABLE")
        row = self.store.db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (proof.admission_id,)
        ).fetchone()
        if row is None or row["challenge"] is None:
            raise AdmissionError("SHADOW_EXECUTION_CHALLENGE")
        challenge = JoinChallenge.model_validate_json(row["challenge"])
        if (
            row["run_id"] != job.run_id
            or row["trial_epoch"] != job.w
            or proof.challenge_hash != challenge.digest()
            or screen.challenge_hash != challenge.digest()
            or screen.nonce != challenge.nonce
            or screen.admission_id != proof.admission_id
            or row["reference_json"] is None
            or WorkProof.model_validate_json(row["reference_json"]) != proof
            or challenge.assignment_hash
            != trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids))
            or job.manifest != self.escrow.manifest
            or not challenge.seed_beacon <= now <= challenge.deadline_beacon
        ):
            raise AdmissionError("SHADOW_EXECUTION_BINDING")
        commit: CommitV2 | None = None
        if operation == "probe":
            accepted = self.store.db.execute(
                "SELECT proof,screen FROM admission_trial_results WHERE epoch=?", (job.w,)
            ).fetchone()
            if (
                accepted is None
                or accepted["proof"] is None
                or accepted["screen"] is None
                or WorkProof.model_validate(parse_envelope(accepted["proof"]).body) != proof
                or WorkScreenV2.model_validate(parse_envelope(accepted["screen"]).body) != screen
            ):
                raise AdmissionError("SHADOW_MINER_PROOF_NOT_ACCEPTED")
            if commit_raw is None:
                raise AdmissionError("SHADOW_MINER_COMMIT_MISSING")
            accepted_commit = Intake(self.escrow.run_id, {"CommitV2": row["hotkey"].__eq__}).accept(
                commit_raw, now
            )
            if not isinstance(accepted_commit, CommitV2) or accepted_commit.w != job.w:
                raise AdmissionError("SHADOW_MINER_COMMIT_BINDING")
            commit = accepted_commit
            commit.validate_assignment(job.manifest, len(job.sample_ids))
        publication = directory / "published"
        artifacts = validate_artifacts(job, publication)
        if tuple(screen.rank_results) != artifacts.ranks:
            raise AdmissionError("SHADOW_PUBLICATION_RANK_SCREEN")
        if operation == "probe":
            assert commit is not None
            from hypertrain.auditor.replay import unpack_state
            from hypertrain.protocol.hashing import MerkleTree
            from hypertrain.protocol.messages import LeafPreimage
            from hypertrain.trainer.compress import state_hash

            summary = load_json((publication / "rank-0/summary.json").read_bytes())["commitments"]
            assert isinstance(summary, dict)
            import json

            leaves = [
                LeafPreimage.model_validate(p) for p in json.loads(artifacts.leaves.read_bytes())
            ]
            if (
                commit.leaves_root != summary["leaves_root"]
                or commit.final_theta_hash != summary["final_theta_hash"]
                or commit.ef_out_hash != summary["ef_out_hash"]
                or commit.delta_hash != sha256_hex(artifacts.delta.read_bytes())
                or commit.delta_bytes != artifacts.delta.stat().st_size
                or commit.metrics_root
                != MerkleTree(
                    [bytes.fromhex(p.loss_f32) + bytes.fromhex(p.norm_f32) for p in leaves]
                ).root.hex()
                or commit.ef_in_hash
                != state_hash(
                    unpack_state(confined(publication, job.object_paths["ef_in"]).read_bytes())[0]
                )
            ):
                raise AdmissionError("SHADOW_MINER_COMMIT_PUBLICATION")
        files = {}
        for path in sorted(publication.rglob("*")):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise AdmissionError("SHADOW_PUBLICATION_SPECIAL_FILE")
            if path.is_file():
                relative = str(path.relative_to(publication))
                data = confined(publication, relative).read_bytes()
                if len(data) > challenge.artifact_limits.max_object_bytes:
                    raise AdmissionError("SHADOW_PUBLICATION_OBJECT_LIMIT")
                files[relative] = {"sha256": self.objects.put(data), "bytes": len(data)}
        if len(canonicalize(files)) > challenge.artifact_limits.max_chunk_manifest_bytes:
            raise AdmissionError("SHADOW_PUBLICATION_MANIFEST_LIMIT")
        if any(
            load_json(confined(publication, f"rank-{rank}/summary.json").read_bytes())["backend"]
            != self.backend
            for rank in range(len(artifacts.ranks))
        ) or not self.qualified(screen):
            raise AdmissionError("SHADOW_PUBLICATION_BACKEND")
        for ref, path in zip(
            proof.artifact_refs,
            (
                artifacts.state,
                artifacts.ef,
                artifacts.delta,
                artifacts.leaves,
            ),
            strict=True,
        ):
            data = path.read_bytes()
            if sha256_hex(data) != ref.sha256 or len(data) != ref.size:
                raise AdmissionError("SHADOW_PUBLICATION_PROOF")
        job_hash = self.objects.put(canonicalize(job.body()))
        publication_hash = self.objects.put(canonicalize(files))
        result = {
            "job_hash": job_hash,
            "publication_hash": publication_hash,
            "proof": proof.body(),
            "screen": screen.body(),
        }
        if operation == "probe":
            assert commit is not None and commit_raw is not None
            result["commit_hash"] = digest(commit)
            result["commit_envelope"] = parse_envelope(commit_raw).model_dump(mode="json")
        result_hash = self.objects.put(canonicalize(result))
        body = {
            "operation": operation,
            "admission_id": proof.admission_id,
            "challenge_hash": proof.challenge_hash,
            "run_id": job.run_id,
            "epoch": job.w,
            "job_hash": job_hash,
            "publication_hash": publication_hash,
            "result_hash": result_hash,
            "backend": self.backend,
            "accepted_beacon": now,
        }
        operation_hash = self.objects.put(canonicalize(body))
        record = {**body, "directory": str(publication.resolve())}
        old = self.store.db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='shadow-execution' AND id=?",
            (job.run_id, operation_hash),
        ).fetchone()
        if old is not None and old["data"] != canonicalize(record).decode():
            raise AdmissionError("SHADOW_EXECUTION_CONFLICT")
        self.store.db.execute(
            "INSERT OR IGNORE INTO records_v2 VALUES(?,?,?,?)",
            (job.run_id, "shadow-execution", operation_hash, canonicalize(record).decode()),
        )
        return {
            "job_hash": job_hash,
            "publication_hash": publication_hash,
            "result_hash": result_hash,
            "operation_hash": operation_hash,
        }

    def proof(self, raw: bytes, screen_raw: bytes, *, now: int) -> AdmissionRecord:
        self._beacon(now)
        env = parse_envelope(raw)
        screen_env = parse_envelope(screen_raw)
        with self.escrow.tx():
            if env.type != "WorkProof" or screen_env.type != "WorkScreenV2":
                raise AdmissionError("PROOF_TYPE")
            proof = WorkProof.model_validate(env.body)
            r = self.store.by_id(proof.admission_id)
            intake = Intake(
                self.escrow.run_id,
                {"WorkProof": lambda s: s == r.hotkey, "WorkScreenV2": lambda s: s == r.hotkey},
            )
            intake.accept(raw, now)
            intake.accept(screen_raw, now)
            screen = WorkScreenV2.model_validate(screen_env.body)
            row = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (r.admission_id,)
            ).fetchone()
            if row["challenge"] is None or row["reference_json"] is None:
                raise AdmissionError("REFERENCE_NOT_COMMITTED")
            c = JoinChallenge.model_validate_json(row["challenge"])
            if (
                r.state not in ("APPLIED", "PROBATION", "ACTIVE")
                or r.pending_dispute
                or now > c.deadline_beacon
            ):
                raise AdmissionError("PROOF_STATE_OR_DEADLINE")
            if (
                (
                    proof.challenge_hash,
                    screen.challenge_hash,
                    screen.admission_id,
                    screen.nonce,
                    screen.policy_hash,
                    screen.image_digest,
                    screen.layout,
                )
                != (
                    c.digest(),
                    c.digest(),
                    c.admission_id,
                    c.nonce,
                    c.policy_hash,
                    self.escrow.manifest.training.reference_spec.image_digest,
                    c.layout,
                )
                or screen.artifact_hashes != [a.sha256 for a in proof.artifact_refs]
                or not self.qualified(screen)
            ):
                raise AdmissionError("WORK_SCREEN_BINDING")
            if row["proof_digest"] is not None:
                if (
                    row["proof_digest"] != digest(proof)
                    or row["screen_json"] != screen.model_dump_json()
                ):
                    raise AdmissionError("PROOF_REPLAY_CONFLICT")
                return r
            if now < row["reference_beacon"]:
                raise AdmissionError("PROOF_BEFORE_REFERENCE")
            expected = WorkProof.model_validate_json(row["reference_json"])
            if expected != proof:
                evidence_hash = sha256_hex(
                    f"ht-admission-mismatch-v2|{c.digest()}|{digest(expected)}|{digest(proof)}".encode()
                )
                dispute_id = sha256_hex(
                    f"ht-admission-dispute-v2|{self.escrow.run_id}|{r.admission_id}|{evidence_hash}".encode()
                )
                self.store.bind_pending(r, dispute_id, evidence_hash)
                self.store.transition(r.admission_id, "SUSPENDED", "PENDING_REPLAY_MISMATCH", now)
                self.store.db.execute(
                    "UPDATE admissions_v2 SET pending_dispute=1,strikes=strikes+1 "
                    "WHERE admission_id=?",
                    (r.admission_id,),
                )
                self.store.db.execute(
                    "UPDATE admission_trial_results SET proof=?,screen=?,outcome='MISMATCH' "
                    "WHERE epoch=?",
                    (raw.decode(), screen_raw.decode(), row["trial_epoch"]),
                )
                return self.store.by_id(r.admission_id)
            for a in proof.artifact_refs:
                if a.size > c.artifact_limits.max_object_bytes:
                    raise AdmissionError("PROOF_OBJECT_LIMIT")
                data = self.objects.get(a.sha256)
                if len(data) != a.size or sha256_hex(data) != a.sha256:
                    raise AdmissionError("PROOF_OBJECT_HASH")
            self.store.db.execute(
                "UPDATE admissions_v2 SET "
                "proof_digest=?,screen_json=?,screen_until=? WHERE admission_id=?",
                (
                    digest(proof),
                    screen.model_dump_json(),
                    now + self.policy.work_screen_epoch_rounds,
                    r.admission_id,
                ),
            )
            self.store.db.execute(
                "UPDATE admission_trial_results SET proof=?,screen=? WHERE epoch=?",
                (raw.decode(), screen_raw.decode(), row["trial_epoch"]),
            )
            if r.state == "APPLIED":
                self.store.transition(r.admission_id, "PROBATION", "FULL_REFERENCE_MATCH", now)
            return self.store.by_id(r.admission_id)

    def reference_due(self, raw: bytes, *, now: int) -> str | None:
        """Admission whose open trial still needs the trusted reference for this signed proof.

        The proof signature is checked first so unauthenticated bodies never start a replay.
        """
        env = parse_envelope(raw)
        if env.type != "WorkProof":
            raise AdmissionError("PROOF_TYPE")
        proof = WorkProof.model_validate(env.body)
        with self.escrow.tx():
            r = self.store.by_id(proof.admission_id)
            Intake(self.escrow.run_id, {"WorkProof": r.hotkey.__eq__}).accept(raw, now)
            row = self.store.db.execute(
                "SELECT challenge,reference_commitment,trial_epoch FROM admissions_v2 "
                "WHERE admission_id=?",
                (r.admission_id,),
            ).fetchone()
            if row["challenge"] is None or row["reference_commitment"] is not None:
                return None
            trial = self.store.db.execute(
                "SELECT outcome FROM admission_trial_results WHERE epoch=?", (row["trial_epoch"],)
            ).fetchone()
            if trial is None or trial["outcome"] != "OPEN":
                return None
            return r.admission_id

    def trial_finality(self, admission_id: str, *, now: int) -> bytes | None:
        """Coordinator-signed Finalize for a verified, unsettled trial, else None.

        Bound exactly as finalize_trial checks it; the caller must submit it through
        finalize_trial so the automatic path and the admin override share every check.
        """
        import hypertrain.trainer  # noqa: F401
        from hypertrain.auditor.replay import unpack_state
        from hypertrain.protocol.jcs import canonicalize
        from hypertrain.trainer.compress import state_hash

        with self.escrow.tx():
            r = self.store.by_id(admission_id)
            row = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
            ).fetchone()
            trial = self.store.db.execute(
                "SELECT finalized_beacon FROM admission_trials WHERE epoch=?",
                (row["trial_epoch"],),
            ).fetchone()
            if (
                row["proof_digest"] is None
                or row["reference_json"] is None
                or trial is None
                or trial["finalized_beacon"] is not None
                or r.state not in ("PROBATION", "ACTIVE")
                or r.pending_dispute
            ):
                return None
            expected = WorkProof.model_validate_json(row["reference_json"])
            challenge = JoinChallenge.model_validate_json(row["challenge"])
            theta, _ = unpack_state(self.objects.get(expected.artifact_refs[0].sha256))
            final = Finalize(
                w=row["trial_epoch"],
                final_theta_hash_w1=state_hash(theta),
                included=[r.hotkey],
                entitlements_root=digest(expected),
            )
            return canonicalize(
                seal(
                    self.coord,
                    "Finalize",
                    self.escrow.run_id,
                    final,
                    max(now, challenge.deadline_beacon) + 1000,
                )
            )

    def finalize_trial(self, admission_id: str, raw: bytes, *, now: int) -> AdmissionStatus:
        """One authenticated finalization per actually replayed shadow participation."""
        self._beacon(now)
        env = parse_envelope(raw)
        if env.type != "Finalize":
            raise AdmissionError("TRIAL_FINALITY_TYPE")
        with self.escrow.tx():
            r = self.store.by_id(admission_id)
            final = Intake(self.escrow.run_id, {"Finalize": lambda s: s == self.coord.ss58}).accept(
                raw, now
            )
            assert isinstance(final, Finalize)
            row = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
            ).fetchone()
            trial = self.store.db.execute(
                "SELECT * FROM admission_trials WHERE epoch=?", (row["trial_epoch"],)
            ).fetchone()
            if trial is None or row["proof_digest"] is None:
                raise AdmissionError("UNVERIFIED_TRIAL")
            expected = WorkProof.model_validate_json(row["reference_json"])
            state_ref = expected.artifact_refs[0]
            import hypertrain.trainer  # noqa: F401
            from hypertrain.auditor.replay import unpack_state
            from hypertrain.trainer.compress import state_hash

            theta, _ = unpack_state(self.objects.get(state_ref.sha256))
            if (
                final.w != row["trial_epoch"]
                or final.included != [r.hotkey]
                or final.entitlements_root != digest(expected)
                or final.final_theta_hash_w1 != state_hash(theta)
            ):
                raise AdmissionError("TRIAL_FINALITY_BINDING")
            key = f"trial-final|{admission_id}|{final.w}"
            self.store.reserve(key, body_digest(env.body), raw.decode())
            if trial["finalized_beacon"] is not None:
                return self.status(r.hotkey, now=now)
            if r.state not in ("PROBATION", "ACTIVE") or r.pending_dispute:
                raise AdmissionError("UNSETTLED_TRIAL")
            count = self.store.db.execute(
                "SELECT COUNT(*) FROM admission_trials WHERE finalized_beacon=?", (now,)
            ).fetchone()[0]
            if count >= self.policy.max_trials_per_beacon:
                raise AdmissionError("TRIAL_FINALIZATION_CAPACITY")
            blocks = set(r.canary_blocks)
            blocks.add(min(r.clean_count // self.policy.E, 2) + 1)
            self.store.db.execute(
                "UPDATE admission_trials SET finalized_beacon=?,evidence_hash=? WHERE epoch=?",
                (now, digest(expected), final.w),
            )
            self.store.db.execute(
                "UPDATE admission_trial_results SET outcome='MATCH' WHERE epoch=?", (final.w,)
            )
            self.store.db.execute(
                "UPDATE admissions_v2 SET "
                "clean_count=clean_count+1,canary_blocks=?,seed_beacon=? WHERE "
                "admission_id=?",
                (",".join(map(str, sorted(blocks))), now + 1, admission_id),
            )
            return self.graduate(r.hotkey, now=now)

    def lock(self, raw: bytes, *, now: int) -> str:
        self._beacon(now)
        env = parse_envelope(raw)
        if env.type != "EscrowLock":
            raise AdmissionError("LOCK_TYPE")
        op = EscrowLock.model_validate(env.body)
        with self.escrow.tx():
            r = self.store.by_id(op.admission_id or "")
            Intake(self.escrow.run_id, {"EscrowLock": lambda s: s == r.coldkey}).accept(raw, now)
            if op.owner != r.coldkey or op.kind != "LOCK_ADMISSION":
                raise AdmissionError("LOCK_OWNER_OR_KIND")
            self.store.enqueue_lock(op, env.signer)
        return self.store.reconcile_lock(op.operation_id).event_hash

    def status(self, hotkey: str, *, now: int) -> AdmissionStatus:
        self._beacon(now)
        r = self.store.record(hotkey)
        units, receipt = self.escrow.locked(r.admission_id, r.coldkey)
        p = self.escrow.policy
        denominator = p.s_lower_ppm * p.beta_ppm * 1_000_000
        required = max(
            p.S_min_units,
            self.escrow.manifest.training.verify.s_min_units(p.R_collectible_units),
            (p.G_max_units * 10**18 + denominator - 1) // denominator
            if denominator
            else p.G_max_units + units + 1,
        )
        funding = FundedStatus(
            self.escrow.run_id,
            hotkey,
            r.admission_id,
            p.ledger_mode,
            units,
            0,
            receipt,
            self.conservative_bound(),
        )
        live_phase = r.state == "ACTIVE" or (r.state == "PROBATION" and r.clean_count >= 4)
        eligible = (
            live_phase
            and not r.pending_dispute
            and now <= r.screen_until
            and units >= required
            and funding.conservative_bound
            and p.s_lower_ppm > 0
            and not self.store.pending_outbox(r.admission_id)
        )
        return AdmissionStatus(
            r, funding, nominal_q(r.state, r.clean_count), p.q_floor, eligible, not eligible
        )

    def graduate(self, hotkey: str, *, now: int) -> AdmissionStatus:
        self._beacon(now)
        with self.escrow.tx():
            s = self.status(hotkey, now=now)
            r = s.record
            if (
                r.state == "PROBATION"
                and r.clean_count >= self.policy.clean_finalizations
                and r.canary_blocks == (1, 2, 3)
                and s.eligible
            ):
                self.store.transition(r.admission_id, "ACTIVE", "GRADUATED_FULL_REPLAY", now)
            return self.status(hotkey, now=now)

    def rotate(self, raw: bytes, *, now: int) -> AdmissionRecord:
        self._beacon(now)
        env = parse_envelope(raw)
        if env.type != "RotateRequest":
            raise AdmissionError("ROTATION_TYPE")
        op = RotateRequest.model_validate(env.body)
        with self.escrow.tx():
            history = self.store.db.execute(
                "SELECT * FROM admission_history WHERE hotkey=?", (op.hotkey,)
            ).fetchone()
            if history is None:
                raise AdmissionError("UNKNOWN_ROTATION_HISTORY")
            Intake(self.escrow.run_id, {"RotateRequest": lambda s: s == history["coldkey"]}).accept(
                raw, now
            )
            key = f"rotate|{op.operation_id}"
            old = self.store.db.execute(
                "SELECT * FROM admission_reservations WHERE reservation=?", (key,)
            ).fetchone()
            if old:
                self.store.reserve(key, body_digest(env.body), old["receipt"])
                return self.store.by_id(history["admission_id"])
            r = self.store.record(op.hotkey)
            if self.store.db.execute(
                "SELECT 1 FROM admission_history WHERE hotkey=?", (op.new_hotkey,)
            ).fetchone():
                raise AdmissionError("NEW_HOTKEY_HISTORY")
            self.store.db.execute(
                "UPDATE admissions_v2 SET hotkey=? WHERE admission_id=? AND hotkey=?",
                (op.new_hotkey, r.admission_id, op.hotkey),
            )
            self.store.db.execute(
                "INSERT INTO admission_history VALUES(?,?,?)",
                (op.new_hotkey, r.admission_id, r.coldkey),
            )
            self.store.reserve(key, body_digest(env.body), r.admission_id)
            return self.store.by_id(r.admission_id)

    def suspend(
        self,
        hotkey: str,
        *,
        reason: str,
        now: int,
        pending_dispute: bool = False,
        dispute_id: str | None = None,
        evidence_hash: str | None = None,
    ) -> None:
        """Internal authenticated admin/dispute hook; no accounting release."""
        self._beacon(now)
        with self.escrow.tx():
            r = self.store.record(hotkey)
            if r.state == "BANNED":
                raise AdmissionError("TERMINAL_BANNED")
            if pending_dispute and (dispute_id is not None or evidence_hash is not None):
                if dispute_id is None or evidence_hash is None:
                    raise AdmissionError("INCOMPLETE_PENDING_REFERENCE")
                self.store.bind_pending(r, dispute_id, evidence_hash)
            self.store.transition(r.admission_id, "SUSPENDED", reason, now)
            if pending_dispute:
                self.store.db.execute(
                    "UPDATE admissions_v2 SET pending_dispute=1 WHERE admission_id=?",
                    (r.admission_id,),
                )

    def resume(self, hotkey: str, *, now: int) -> None:
        self._beacon(now)
        with self.escrow.tx():
            r = self.store.record(hotkey)
            if r.state != "SUSPENDED" or r.pending_dispute:
                raise AdmissionError("RESUME_STATE")
            self.store.transition(r.admission_id, "PROBATION", "RESUME_FRESH_PROBATION", now)
            self.store.db.execute(
                "UPDATE admission_trial_results SET outcome='INFRASTRUCTURE' "
                "WHERE epoch=(SELECT trial_epoch FROM admissions_v2 WHERE admission_id=?) "
                "AND outcome='OPEN'",
                (r.admission_id,),
            )
            self.store.db.execute(
                "UPDATE admissions_v2 SET challenge=NULL,proof_digest=NULL,reference_json=NULL,"
                "reference_commitment=NULL,screen_json=NULL,"
                "clean_count=0,canary_blocks='',screen_until=0,seed_beacon=? WHERE "
                "admission_id=?",
                (now + 1, r.admission_id),
            )

    def resolve_pending(self, hotkey: str, reference: str, *, now: int) -> None:
        """Resume only from L5's authenticated nonfraud settlement, preserving strikes."""
        self._beacon(now)
        with self.escrow.tx():
            r = self.store.record(hotkey)
            pending = self.store.db.execute(
                "SELECT * FROM admission_pending WHERE admission_id=? AND resolved=0",
                (r.admission_id,),
            ).fetchone()
            if pending is None or pending["dispute_id"] != reference:
                raise AdmissionError("PENDING_SETTLEMENT_REFERENCE")
            result = self.escrow.settlement(reference)
            if (
                r.state != "SUSPENDED"
                or not r.pending_dispute
                or result.unresolved
                or result.outcome == "FRAUD"
                or now < result.release_beacon
                or (
                    result.run_id,
                    result.admission_id,
                    result.coldkey,
                    result.dispute_id,
                    result.evidence_hash,
                )
                != (
                    self.escrow.run_id,
                    r.admission_id,
                    r.coldkey,
                    pending["dispute_id"],
                    pending["evidence_hash"],
                )
            ):
                raise AdmissionError("PENDING_SETTLEMENT")
            self.store.db.execute(
                "UPDATE admissions_v2 SET pending_dispute=0 WHERE admission_id=?", (r.admission_id,)
            )
            self.store.db.execute(
                "UPDATE admission_pending SET resolved=1 WHERE dispute_id=? AND resolved=0",
                (reference,),
            )
            self.resume(hotkey, now=now)

    def settle_fraud(self, hotkey: str, lock_id: str, *, now: int) -> None:
        """Only an authenticated, finalized proven training-fraud settlement bans."""
        self._beacon(now)
        with self.escrow.tx():
            r = self.store.record(hotkey)
            status = self.escrow.settlement(lock_id)
            lock_row = self.store.db.execute(
                "SELECT request FROM escrow_events WHERE operation_id=? AND kind='LOCK_ADMISSION'",
                (lock_id,),
            ).fetchone()
            if lock_row is None:
                raise AdmissionError("UNKNOWN_FRAUD_LOCK")
            op = EscrowLock.model_validate_json(lock_row["request"])
            if (
                op.admission_id != r.admission_id
                or op.owner != r.coldkey
                or status.unresolved
                or status.outcome != "FRAUD"
                or now < status.release_beacon
            ):
                raise AdmissionError("UNPROVEN_TRAINING_FRAUD")
            self.store.transition(r.admission_id, "BANNED", "FINALIZED_TRAINING_FRAUD", now)
