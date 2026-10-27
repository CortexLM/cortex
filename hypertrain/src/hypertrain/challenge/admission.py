"""Bounded, persisted, shadow admission backed by actual full-round references."""

from __future__ import annotations

import hmac
import json
import secrets
import threading
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Literal

from pydantic import JsonValue, TypeAdapter

if TYPE_CHECKING:
    from experiments.gpu_network_v2.orchestrate import NetworkRuntime
    from scripts.network_gpu_operation import Operation

from hypertrain.beacon.core import BeaconRound
from hypertrain.challenge.admission_store import AdmissionError, AdmissionRecord, AdmissionStore
from hypertrain.challenge.finality_v2 import RoundStatus
from hypertrain.challenge.trust_v2 import FundedStatus, nominal_q
from hypertrain.data.store import (
    Store,
    StoreError,
    _stream,
    get_large,
    parse_index,
    put_large,
)
from hypertrain.data.trial_assignment import trial_assignment_hash, trial_samples
from hypertrain.gpu_ops.work_screen import IslandLaunch, screen_work
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2, Settlement, digest
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.envelope_v2 import (
    Intake,
    load_json,
    parse_envelope,
    seal,
    verify_join,
    verify_rotate,
)
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    PART_BYTES,
    AdmissionPolicyV2,
    ArtifactRef,
    EscrowLock,
    EscrowReceipt,
    EscrowRelease,
    Hex64,
    IslandJobV1,
    JoinChallenge,
    JoinRequest,
    RotateRequest,
    WorkProof,
    WorkScreenV2,
)

SeizureReason = Literal["TRAINING_FRAUD", "NO_SHOW", "AUDIT_MISMATCH"]


@dataclass(frozen=True)
class _Standing:
    """What a MATCH settlement of any lock of one admission depends on, apart from the lock."""

    unresolved: bool
    rounds: list[list[object]]
    origins: list[str]
    disputes: list[str]
    release_beacon: int


def _logical(objects: Store, ref: ArtifactRef, code: str) -> tuple[str, int, bool]:
    """(whole sha256, logical size, indexed) of `ref`; ref.size is the stored object length."""
    try:
        raw = objects.get(ref.sha256)
        index = parse_index(raw)
    except StoreError:
        raise AdmissionError(code) from None
    if len(raw) != ref.size:
        raise AdmissionError(code)
    return (ref.sha256, ref.size, False) if index is None else (index.sha256, index.size, True)


def _verify_ref(objects: Store, ref: ArtifactRef, cap: int) -> tuple[str, int]:
    """Caps bind each stored object (ref and every part); the logical size is bounded by
    PartIndexV1 (<= MAX_PARTS * PART_BYTES). Parts stream through hashing, never assembled."""
    if ref.size > cap:
        raise AdmissionError("PROOF_OBJECT_LIMIT")
    whole, size, indexed = _logical(objects, ref, "PROOF_OBJECT_HASH")
    if indexed:
        if PART_BYTES > cap:
            raise AdmissionError("PROOF_OBJECT_LIMIT")
        try:
            _stream(ref.sha256, objects.get, lambda _part: None)  # part lengths + whole sha
        except StoreError:
            raise AdmissionError("PROOF_OBJECT_HASH") from None
    return whole, size


def _put_capped(objects: Store, data: bytes, cap: int) -> str:
    """put_large with the policy cap bound to every stored object (plain, part, index)."""

    def put(chunk: bytes) -> str:
        if len(chunk) > cap:
            raise AdmissionError("SHADOW_PUBLICATION_OBJECT_LIMIT")
        return objects.put(chunk)

    return put_large(data, put)


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
        escrow.admission_settlement = self.lock_settlement
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
            self.reap_dead_applications(now=now)
            if self.store.db.execute(
                "SELECT 1 FROM admission_history WHERE hotkey=?", (request.hotkey,)
            ).fetchone():
                raise AdmissionError("HOTKEY_HISTORY_EXISTS")
            if self.store.db.execute(
                "SELECT 1 FROM admissions_v2 WHERE coldkey=? AND state!='ACTIVE' "
                "AND NOT (state='SUSPENDED' AND reason='APPLIED_EXPIRED')",
                (request.coldkey,),
            ).fetchone():
                raise AdmissionError("COLDKEY_PROBATION_EXISTS")
            count = self.store.db.execute(
                "SELECT COUNT(*) FROM admissions_v2 WHERE state IN ('APPLIED','PROBATION')"
            ).fetchone()[0]
            if count >= self.policy.max_pending:
                raise AdmissionError("PENDING_CAPACITY")
            tag = self._prefix_tag(ip_prefix)
            if tag is not None and self._unfunded_in_prefix(tag) >= (
                self.policy.max_unfunded_per_prefix or 0
            ):
                raise AdmissionError("PREFIX_PENDING_CAPACITY")
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
            if tag is not None:
                self.store.db.execute(
                    "INSERT INTO admission_origin VALUES(?,?)", (admission_id, tag)
                )
            self.store.reserve(key, request_hash, admission_id)
            return self.store.by_id(admission_id)

    def _prefix_tag(self, ip_prefix: str) -> str | None:
        """Coordinator-keyed tag of the /24 or /64, only when the policy caps unfunded rows.

        Not bound to the quota epoch, so the cap survives the epoch rollover; keyed, so the
        stored tag does not reveal the prefix.
        """
        if self.policy.max_unfunded_per_prefix is None:
            return None
        message = f"admission-prefix|{self.escrow.run_id}|{ip_prefix}".encode()
        return hmac.digest(self.ip_secret, message, "sha256").hex()

    def _unfunded_in_prefix(self, tag: str) -> int:
        rows = self.store.db.execute(
            "SELECT a.admission_id,a.coldkey FROM admission_origin o "
            "JOIN admissions_v2 a ON a.admission_id=o.admission_id "
            "WHERE o.prefix_tag=? AND a.state='APPLIED'",
            (tag,),
        ).fetchall()
        return sum(self._unfunded(row["admission_id"], row["coldkey"]) for row in rows)

    def reap_dead_applications(self, *, now: int) -> list[str]:
        """Suspend as APPLIED_EXPIRED every APPLIED row that cannot progress and holds no stake.

        An APPLIED row is dead once `now > seed_beacon + challenge_rounds`: challenge() then
        refuses for ever, and only join, finalize_trial and resume write the seed. Without this
        the row keeps one of the max_pending places and its coldkey for good.
        The row, its transitions and its spent hotkey stay (no deletion, no new state). A row
        with a lock, or with one waiting in the outbox, is never touched: releasing stakes is the
        owner's. It runs at the start of join, not in challenge() or status(): a raising
        operation rolls its transaction back, and a GET must not write.
        """
        with self.escrow.tx():
            self.reap_expired_trials(now=now)
            dead = self.store.db.execute(
                "SELECT admission_id,coldkey FROM admissions_v2 "
                "WHERE state='APPLIED' AND seed_beacon+?<? ORDER BY admission_id",
                (self.policy.challenge_rounds, now),
            ).fetchall()
            reaped = []
            for row in dead:
                if self._unfunded(row["admission_id"], row["coldkey"]):
                    self.store.transition(row["admission_id"], "SUSPENDED", "APPLIED_EXPIRED", now)
                    reaped.append(str(row["admission_id"]))
            return reaped

    def _unfunded(self, admission_id: str, coldkey: str) -> bool:
        """No locked unit and no lock waiting in the outbox."""
        units, _ = self.escrow.locked(admission_id, coldkey)
        return units == 0 and not self.store.pending_outbox(admission_id)

    def reap_expired_trials(self, *, now: int) -> list[int]:
        """Close every OPEN trial with no accepted proof whose deadline has passed.

        Bookkeeping, not a sanction: nobody is suspended, no strike is added and no seed moves,
        so an honest miner that lost its window to an eviction or a beacon pause keeps its state.
        The deadline round itself still accepts a proof. A proven trial waits for its
        finalization, which may follow the deadline. The admission's state is ignored on purpose:
        a suspended, banned or deregistered holder keeps a trial place that no request can free.
        """
        with self.escrow.tx():
            epochs = [
                row[0]
                for row in self.store.db.execute(
                    "SELECT r.epoch FROM admission_trial_results r "
                    "JOIN admissions_v2 a ON a.trial_epoch=r.epoch "
                    "WHERE r.outcome='OPEN' AND a.proof_digest IS NULL "
                    "AND json_extract(r.challenge,'$.deadline_beacon')<? ORDER BY r.epoch",
                    (now,),
                )
            ]
            for epoch in epochs:
                self.store.db.execute(
                    "UPDATE admission_trial_results SET outcome='EXPIRED' WHERE epoch=?", (epoch,)
                )
            return epochs

    def challenge(self, admission_id: str, *, now: int) -> dict[str, JsonValue]:
        self._beacon(now)
        with self.escrow.tx():
            r = self.store.by_id(admission_id)
            row = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
            ).fetchone()
            if r.state not in ("APPLIED", "PROBATION", "ACTIVE") or r.pending_dispute:
                raise AdmissionError("CHALLENGE_STATE")
            self.reap_expired_trials(now=now)
            if row["challenge"]:
                c = JoinChallenge.model_validate_json(row["challenge"])
                trial = self.store.db.execute(
                    "SELECT outcome FROM admission_trial_results WHERE epoch=?",
                    (row["trial_epoch"],),
                ).fetchone()
                if trial and trial["outcome"] == "OPEN":
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
            logical = (sha256_hex(data), len(data))
            if (
                put_large(data, self.objects.put) != ref.sha256
                or logical != _logical(self.objects, ref, "REFERENCE_OBJECT_HASH")[:2]
            ):
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
                        (
                            publication["files"]["rank-0/" + name]["sha256"],
                            publication["files"]["rank-0/" + name]["bytes"],
                        )
                        != _logical(self.objects, ref, "SHADOW_RUNTIME_ACCEPTED_REFERENCE")[:2]
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
                _put_capped(self.objects, data, challenge.artifact_limits.max_object_bytes)
                # Custody `files` keep whole-file sha/bytes; the store holds plain or indexed refs.
                files[relative] = {"sha256": sha256_hex(data), "bytes": len(data)}
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
            if (sha256_hex(data), len(data)) != _logical(
                self.objects, ref, "SHADOW_PUBLICATION_PROOF"
            )[:2]:
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
            row = self.store.db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (r.admission_id,)
            ).fetchone()
            # The exact pair already accepted may be retried after its deadline, so a
            # finalization that failed after the proof can run; a new proof may not.
            replay = row["proof_digest"] == digest(proof)
            at = min(now, env.exp_drand, screen_env.exp_drand) if replay else now
            intake = Intake(
                self.escrow.run_id,
                {"WorkProof": lambda s: s == r.hotkey, "WorkScreenV2": lambda s: s == r.hotkey},
            )
            intake.accept(raw, at)
            intake.accept(screen_raw, at)
            screen = WorkScreenV2.model_validate(screen_env.body)
            if row["challenge"] is None or row["reference_json"] is None:
                raise AdmissionError("REFERENCE_NOT_COMMITTED")
            c = JoinChallenge.model_validate_json(row["challenge"])
            if (
                r.state not in ("APPLIED", "PROBATION", "ACTIVE")
                or r.pending_dispute
                or (now > c.deadline_beacon and not replay)
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
                _verify_ref(self.objects, a, c.artifact_limits.max_object_bytes)
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
            row = self.store.db.execute(
                "SELECT challenge,reference_commitment,trial_epoch,proof_digest "
                "FROM admissions_v2 WHERE admission_id=?",
                (r.admission_id,),
            ).fetchone()
            at = min(now, env.exp_drand) if row["proof_digest"] == digest(proof) else now
            Intake(self.escrow.run_id, {"WorkProof": r.hotkey.__eq__}).accept(raw, at)
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
            theta, _ = unpack_state(get_large(expected.artifact_refs[0].sha256, self.objects.get))
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

            theta, _ = unpack_state(get_large(state_ref.sha256, self.objects.get))
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
            if r.state == "BANNED":
                raise AdmissionError("LOCK_BANNED")
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
        """Move an admission to another hotkey, with the consent of both keys.

        The coldkey of the admission seals the envelope; the new hotkey signed the body
        (`new_hot_sig`), as a joining hotkey does. A new rotation, never a replay, is charged to
        `join_burst` per coldkey per `work_screen_epoch_rounds`; a banned admission cannot rotate.
        """
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
            if not verify_rotate(op, history["coldkey"], self.escrow.run_id, env.exp_drand):
                raise AdmissionError("ROTATION_CONSENT")
            key = f"rotate|{op.operation_id}"
            # sr25519 signatures are randomized: the replay digest leaves the consent out
            request_hash = body_digest(op.model_dump(mode="json", exclude={"new_hot_sig"}))
            old = self.store.db.execute(
                "SELECT * FROM admission_reservations WHERE reservation=?", (key,)
            ).fetchone()
            if old:
                self.store.reserve(key, request_hash, old["receipt"])
                return self.store.by_id(history["admission_id"])
            r = self.store.record(op.hotkey)
            if self.store.db.execute(
                "SELECT 1 FROM admission_history WHERE hotkey=?", (op.new_hotkey,)
            ).fetchone():
                raise AdmissionError("NEW_HOTKEY_HISTORY")
            if r.state == "BANNED":
                raise AdmissionError("TERMINAL_BANNED")
            self.store.quota(
                f"rotate:{r.coldkey}:{now // self.policy.work_screen_epoch_rounds}",
                now,
                0,
                self.policy.join_burst,
                code="ROTATION_QUOTA",
            )
            self.store.db.execute(
                "UPDATE admissions_v2 SET hotkey=? WHERE admission_id=? AND hotkey=?",
                (op.new_hotkey, r.admission_id, op.hotkey),
            )
            self.store.db.execute(
                "INSERT INTO admission_history VALUES(?,?,?)",
                (op.new_hotkey, r.admission_id, r.coldkey),
            )
            self.store.reserve(key, request_hash, r.admission_id)
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

    # The admission bond (ADR 0010, Part 6). A lock is settled by its own resolver, derived from
    # persisted rows on every call and installed on the ledger (EscrowV2.admission_settlement),
    # so release and slash agree with the admission state and a lock id cannot borrow the
    # settlement of a dispute or of a reward origin.

    def _bond_locks(self, admission_id: str) -> list[EscrowLock]:
        coldkey = self.store.by_id(admission_id).coldkey
        locks = (
            EscrowLock.model_validate_json(row["request"])
            for row in self.store.db.execute(
                "SELECT request FROM escrow_events WHERE kind='LOCK_ADMISSION' ORDER BY seq"
            )
        )
        return [
            lock for lock in locks if lock.admission_id == admission_id and lock.owner == coldkey
        ]

    def _residual(self, lock: EscrowLock) -> tuple[list[str], int]:
        rows = self.store.db.execute(
            "SELECT origin,units FROM escrow_units WHERE owner=? AND bucket='admission_locked' "
            "AND ref=? AND units>0 ORDER BY origin",
            (lock.owner, lock.operation_id),
        ).fetchall()
        return [row["origin"] for row in rows], sum(row["units"] for row in rows)

    def _bond_operation(self, label: str, lock_id: str) -> str:
        """Deterministic for retries, unguessable for an owner who would register it first."""
        return hmac.digest(self.ip_secret, f"ht-bond-{label}-v1|{lock_id}".encode(), "sha256").hex()

    def lock_settlement(self, lock_id: str, standing: _Standing | None = None) -> Settlement:
        """What may happen to one admission lock. Derived, never stored.

        FRAUD once the lock was seized (`seize_bond`). Otherwise MATCH, and unresolved until
        the admission is not banned and has no pending dispute or lock in the outbox, every
        round whose roster lists it is closed, none of its hotkeys has unvested rewards, and
        `release_beacon` is reached: the last maturity of its reward origins, and, while it is
        live (ACTIVE, or PROBATION with four clean trials), the end of its current screen, since
        the bond is what makes a live miner eligible.
        """
        row = self.store.db.execute(
            "SELECT request FROM escrow_events WHERE operation_id=? AND kind='LOCK_ADMISSION'",
            (lock_id,),
        ).fetchone()
        if row is None:
            raise EscrowError("UNKNOWN_LOCK")
        lock = EscrowLock.model_validate_json(row["request"])
        assert lock.admission_id is not None
        admission_id = lock.admission_id
        who = {"run_id": self.escrow.run_id, "admission_id": admission_id, "coldkey": lock.owner}
        seized = self.store.db.execute(
            "SELECT reason,evidence_hash,beacon FROM admission_seizures WHERE lock_id=?",
            (lock_id,),
        ).fetchone()
        if seized is not None:
            return Settlement(
                finality_hash=sha256_hex(
                    canonicalize(
                        ["ht-bond-seized-v1", lock_id, seized["reason"], seized["evidence_hash"]]
                    )
                ),
                closed_dispute_root=sha256_hex(canonicalize([seized["evidence_hash"]])),
                release_beacon=seized["beacon"],
                unresolved=False,
                outcome="FRAUD",
                evidence_hash=seized["evidence_hash"],
                **who,
            )
        st = standing or self._standing(admission_id)
        return Settlement(
            finality_hash=sha256_hex(
                canonicalize(["ht-bond-final-v1", lock_id, st.rounds, st.origins])
            ),
            closed_dispute_root=sha256_hex(canonicalize(["ht-bond-closed-v1", st.disputes])),
            release_beacon=st.release_beacon,
            unresolved=st.unresolved,
            outcome="MATCH",
            **who,
        )

    def _standing(self, admission_id: str) -> _Standing:
        """The admission-level reads of `lock_settlement`: one scan of the run's rounds, however
        many locks the admission holds."""
        r = self.store.by_id(admission_id)
        unresolved = r.state == "BANNED" or r.pending_dispute
        unresolved = unresolved or self.store.pending_outbox(admission_id)
        pending = self.store.db.execute(
            "SELECT dispute_id,resolved FROM admission_pending WHERE admission_id=? "
            "ORDER BY dispute_id",
            (admission_id,),
        ).fetchall()
        unresolved = unresolved or any(not p["resolved"] for p in pending)
        rounds: list[list[object]] = []
        for rostered in self.store.db.execute(
            "SELECT r.id FROM records_v2 r, json_each(json_extract(r.data,'$.body.roster')) j "
            "WHERE r.run_id=? AND r.kind='round' AND json_extract(j.value,'$.admission_id')=? "
            "ORDER BY CAST(r.id AS INTEGER)",
            (self.escrow.run_id, admission_id),
        ).fetchall():
            record = self.store.db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='finality' AND id=?",
                (self.escrow.run_id, rostered["id"]),
            ).fetchone()
            status = RoundStatus(**json.loads(record["data"])) if record else None
            unresolved = unresolved or status is None or not status.closed
            if status is not None:
                rounds.append([status.w, status.disposition, status.resolution_hash])
        origins = self.store.db.execute(
            "SELECT origin,mature_at FROM escrow_origins WHERE hotkey IN "
            "(SELECT hotkey FROM admission_history WHERE admission_id=?) ORDER BY origin",
            (admission_id,),
        ).fetchall()
        unvested = self.store.db.execute(
            "SELECT 1 FROM escrow_units u JOIN escrow_origins o ON o.origin=u.origin "
            "WHERE u.bucket='reward_pending' AND u.units>0 AND o.hotkey IN "
            "(SELECT hotkey FROM admission_history WHERE admission_id=?) LIMIT 1",
            (admission_id,),
        ).fetchone()
        release = max((o["mature_at"] for o in origins), default=0)
        if r.state == "ACTIVE" or (r.state == "PROBATION" and r.clean_count >= 4):
            release = max(release, r.screen_until + 1)
        return _Standing(
            unresolved=unresolved or unvested is not None,
            rounds=rounds,
            origins=[o["origin"] for o in origins],
            disputes=[p["dispute_id"] for p in pending],
            release_beacon=release,
        )

    def bond(self, admission_id: str, *, now: int) -> dict[str, JsonValue]:
        """The admission's locked bond and what the owner must sign to take it back (status)."""
        locks: list[JsonValue] = []
        standing = None
        for lock in self._bond_locks(admission_id):
            origins, units = self._residual(lock)
            if not units:
                continue
            standing = standing or self._standing(admission_id)
            s = self.lock_settlement(lock.operation_id, standing)
            locks.append(
                {
                    "lock_id": lock.operation_id,
                    "units": units,
                    "origin_ids": list(origins),
                    "outcome": s.outcome,
                    "releasable": not s.unresolved
                    and s.outcome != "FRAUD"
                    and now >= s.release_beacon,
                    "release_beacon": s.release_beacon,
                    "finality_hash": s.finality_hash,
                    "closed_dispute_root": s.closed_dispute_root,
                }
            )
        return {"locks": locks}

    def release_bond(self, admission_id: str, *, now: int) -> list[EscrowReceipt]:
        """Release every residual lock of the admission to its owner, or none.

        Internal, authenticated service hook; no route calls it. The owner's own way out is the
        signed `EscrowRelease` of `POST .../escrow/release` against the hashes of `bond`. Refused
        with BOND_NOT_RELEASABLE unless the derived settlement is MATCH and released.
        """
        self._beacon(now)
        receipts: list[EscrowReceipt] = []
        standing = None
        with self.escrow.tx():
            for lock in self._bond_locks(admission_id):
                origins, units = self._residual(lock)
                if not units:
                    continue
                standing = standing or self._standing(admission_id)
                s = self.lock_settlement(lock.operation_id, standing)
                if s.unresolved or s.outcome == "FRAUD" or now < s.release_beacon:
                    raise AdmissionError("BOND_NOT_RELEASABLE")
                op = EscrowRelease(
                    operation_id=self._bond_operation("release", lock.operation_id),
                    owner=lock.owner,
                    units=units,
                    origin_ids=origins,
                    admission_id=admission_id,
                    dispute_id=None,
                    lock_id=lock.operation_id,
                    finality_hash=s.finality_hash,
                    closed_dispute_root=s.closed_dispute_root,
                )
                receipts.append(self.escrow.release(op, signer=lock.owner, now_beacon=now))
        return receipts

    def seize_bond(
        self, admission_id: str, *, reason: SeizureReason, evidence_hash: str, now: int
    ) -> int:
        """Sanction an admission and burn its residual bond; returns the units burned.

        Internal, authenticated service hook (the coordinator is the ledger's reward authority);
        no route calls it. `evidence_hash` (64 hex) names what justifies it, and the pair
        (reason, evidence) is applied once: a retry burns nothing more and adds no strike. Every
        residual lock is recorded as seized, so its derived settlement is FRAUD (never
        releasable), and the burn goes through `EscrowV2.slash` under an HMAC operation id.
        Resolve the admission by id, never by hotkey, which a rotation changes.

        The caller decides whose fault it is. An auditor, infrastructure or referee-side fault
        never reaches this function: `settle_fraud` returns False for a verdict against the
        auditor, and the closures of ADR 0011 are not convictions.

        TRAINING_FRAUD (a referee verdict, see settle_fraud): also burns the owner's unvested
        rewards, as part of slashing a residual lock (after an earlier bond-only seizure there
        is none, and the rewards survive), and bans the admission (terminal; `lock` then
        answers LOCK_BANNED).
        AUDIT_MISMATCH (a round-stage audit mismatch the caller has confirmed as the miner's):
        suspends the admission, reason AUDIT_MISMATCH, adds one strike, and burns the bond alone.
        The coldkey stays bound by the one-non-ACTIVE-admission rule, as in probation, and the
        round's reward of the honest peers is not touched. NO_SHOW (a roster member that never
        committed): the bond alone; the admission stays, ineligible until it is funded again.
        """
        TypeAdapter(Hex64).validate_python(evidence_hash)
        if reason not in ("TRAINING_FRAUD", "NO_SHOW", "AUDIT_MISMATCH"):
            raise AdmissionError("SEIZURE_REASON")
        self._beacon(now)
        burned = 0
        with self.escrow.tx():
            r = self.store.by_id(admission_id)
            first = (
                self.store.db.execute(
                    "INSERT OR IGNORE INTO admission_sanctions VALUES(?,?,?,?)",
                    (admission_id, reason, evidence_hash, now),
                ).rowcount
                == 1
            )
            for lock in self._bond_locks(admission_id):
                origins, units = self._residual(lock)
                if not units:
                    continue
                self.store.db.execute(
                    "INSERT OR IGNORE INTO admission_seizures VALUES(?,?,?,?,?)",
                    (lock.operation_id, admission_id, reason, evidence_hash, now),
                )
                s = self.lock_settlement(lock.operation_id)
                op = EscrowRelease(
                    operation_id=self._bond_operation("slash", lock.operation_id),
                    owner=lock.owner,
                    units=units,
                    origin_ids=origins,
                    admission_id=admission_id,
                    dispute_id=None,
                    lock_id=lock.operation_id,
                    finality_hash=s.finality_hash,
                    closed_dispute_root=s.closed_dispute_root,
                )
                self.escrow.slash(
                    op,
                    authority=self.coord.ss58,
                    now_beacon=now,
                    burn_pending=reason == "TRAINING_FRAUD",
                )
                burned += units
            if reason == "TRAINING_FRAUD" and r.state != "BANNED":
                self.store.transition(admission_id, "BANNED", "FINALIZED_TRAINING_FRAUD", now)
            if reason == "AUDIT_MISMATCH" and first and r.state != "BANNED":
                self.store.transition(admission_id, "SUSPENDED", "AUDIT_MISMATCH", now)
                self.store.db.execute(
                    "UPDATE admissions_v2 SET strikes=strikes+1 WHERE admission_id=?",
                    (admission_id,),
                )
        return burned

    def settle_fraud(self, dispute_id: str, *, now: int) -> bool:
        """Seize and ban on a referee verdict of training fraud against the miner.

        Called with the dispute id when a resolution is accepted. Only an accepted, resolved
        dispute settlement whose outcome is FRAUD (reason FRAUD and loser == the miner of the
        contest) convicts. It returns False and changes nothing when there is no pending
        reference, the dispute is unresolved or the outcome is not FRAUD. The pending reference
        bound when the dispute opened must name the same admission, coldkey and evidence, and the
        release beacon must be reached, or UNPROVEN_TRAINING_FRAUD is raised and the resolution
        that called it is rolled back.
        """
        self._beacon(now)
        with self.escrow.tx():
            pending = self.store.db.execute(
                "SELECT * FROM admission_pending WHERE dispute_id=?", (dispute_id,)
            ).fetchone()
            if pending is None:
                return False
            s = self.escrow.settlement(dispute_id)
            if s.unresolved or s.outcome != "FRAUD" or s.admission_id is None:
                return False
            r = self.store.by_id(pending["admission_id"])
            if (s.run_id, s.admission_id, s.coldkey, s.dispute_id, s.evidence_hash) != (
                self.escrow.run_id,
                r.admission_id,
                r.coldkey,
                pending["dispute_id"],
                pending["evidence_hash"],
            ) or now < s.release_beacon:
                raise AdmissionError("UNPROVEN_TRAINING_FRAUD")
            self.seize_bond(
                r.admission_id,
                reason="TRAINING_FRAUD",
                evidence_hash=pending["evidence_hash"],
                now=now,
            )
            self.store.db.execute(
                "UPDATE admission_pending SET resolved=1 WHERE dispute_id=?", (dispute_id,)
            )
            return True
