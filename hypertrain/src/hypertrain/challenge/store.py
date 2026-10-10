"""SQLite (WAL) coordinator state: runs, roster, drand rounds, round state machine, intake, audits.

Every deadline is a drand round number; the only clock is the latest operator-pushed, verified
drand round D. Ledger events (todo 3 Ledger) carry the drand chain time of D. Route and JSON
contract: docs/challenge-routes.md.
"""

from __future__ import annotations

import ast
import hashlib
import hmac
import json
import math
import secrets
import socket
import sqlite3
import struct
import tempfile
import threading
import time
from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
from dataclasses import asdict
from fractions import Fraction
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal

from pydantic import BaseModel, ConfigDict, JsonValue, StrictInt

if TYPE_CHECKING:
    from experiments.gpu_network_v2.orchestrate import NetworkRuntime
    from scripts.network_gpu_operation import Operation

    from hypertrain.gpu_ops.work_screen import IslandLaunch

from hypertrain.beacon import BeaconError, BeaconRound
from hypertrain.challenge.admission import Admission
from hypertrain.challenge.disputes_v2 import Contest, DisputesV2
from hypertrain.data.assignment import assign_round
from hypertrain.data.store import ObjectNotFound, StoreError
from hypertrain.data.store import Store as ObjectStore
from hypertrain.data.trial_assignment import trial_assignment_hash
from hypertrain.ledger import Ledger, LedgerError, Params
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    EscrowV2,
    FinalityEvidence,
    RewardWork,
    Settlement,
    ShadowReservationEvidence,
)
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import (
    EnvelopeError,
    ExpiredError,
    Intake,
    MalformedEnvelope,
    ReplayError,
    RunMismatchError,
    SignatureError,
    UnauthorizedSigner,
    body_digest,
    replay_key,
    seal,
)
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey, verify
from hypertrain.protocol.messages import (
    SS58,
    Accept,
    AuditChallenge,
    Bisect,
    Commit,
    DeltaManifest,
    Dispute,
    Finalize,
    LeafPreimage,
    ReplayVerdict,
    Resolution,
    Rollback,
    RoundOpen,
    RunManifest,
    StateServe,
    f32hex,
    f32val,
)
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    AggregationPolicyV2,
    AuditPolicyV2,
    DisputePolicyV2,
    EconomicsPolicyV2,
    Hex64,
    IslandJobV1,
    JoinChallenge,
    PolicyHashes,
    RunManifestV2,
    ShadowReplayReceiptV1,
    ShadowReservationRequestV1,
    ShadowReservationV1,
    ShadowRewardFinalizeV1,
    WireModel,
    WorkScreenV2,
)

NEVER = 2**53 - 1  # exp_drand of container-signed records
LEASE_ROUNDS = 600  # 30 min of 3 s drand rounds
ZERO_STATE = sha256_hex(b"ht-zero-outer-state")
NO_HONEYPOT = sha256_hex(b"ht-no-honeypot")
FAULTS = frozenset({"MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"})
MINER_TYPES = frozenset({"Accept", "Commit", "DeltaManifest", "StateServe", "Dispute", "Bisect"})
COORD_TYPES = ("RoundOpen", "Rollback", "Finalize")


class ShadowCustodyIntake(WireModel):
    job_hash: Hex64
    reference_publication_hash: Hex64
    miner_publication_hash: Hex64
    reference_result_hash: Hex64
    miner_result_hash: Hex64
    reference_operation_hash: Hex64
    miner_operation_hash: Hex64


class ShadowReservationIntake(WireModel):
    finalize: envelope_v2.EnvelopeV2
    commit: envelope_v2.EnvelopeV2
    custody: ShadowCustodyIntake


class ShadowAcceptanceIntake(WireModel):
    reservation_hash: Hex64
    audit_challenge: envelope_v2.EnvelopeV2
    verdict: envelope_v2.EnvelopeV2
    auditor_receipt: envelope_v2.EnvelopeV2
    reward: ShadowRewardFinalizeV1


class ShadowOperatorRecord(BaseModel):
    """Private strict addressed authority, not a public protocol or qualification."""

    model_config = ConfigDict(extra="forbid", strict=True, frozen=True)
    domain: Literal["shadow-operator-admission-v1"]
    run_id: Hex64
    owner: SS58
    admission_hash: Hex64
    manifest_hash: Hex64
    economics_policy_hash: Hex64
    graph_hash: Hex64
    workload: dict[str, JsonValue]
    backend_qualification_authority_hash: Hex64
    source_map_hash: Hex64
    profile_hash: Hex64
    hosts: dict[str, JsonValue]
    budget: dict[str, JsonValue]
    accepted_beacon: StrictInt
    cutoff_unix: StrictInt
    expires_beacon: StrictInt


def shadow_operator_receipt(
    raw: bytes, owner: str, run_id: str, accepted_at: int
) -> tuple[ShadowOperatorRecord, str, dict[str, JsonValue]]:
    """Authenticate ORIGINAL receipt beacon on intake/recovery, never current expiry renewal."""
    from hypertrain.protocol.messages import Receipt

    payload = envelope_v2.load_json(raw)
    if set(payload) != {"record", "receipt"}:
        raise ChallengeError(403, "operator payload fields differ")
    record = ShadowOperatorRecord.model_validate(payload["record"])
    receipt = envelope_v2.Intake(run_id, {"Receipt": owner.__eq__}).accept(
        canonicalize(payload["receipt"]), accepted_at
    )
    if type(accepted_at) is not int or accepted_at < 1:
        raise ChallengeError(403, "operator retained verified beacon invalid")
    env = envelope_v2.parse_envelope(canonicalize(payload["receipt"]))
    digest = sha256_hex(canonicalize(record.model_dump(mode="json")))
    if (
        not isinstance(receipt, Receipt)
        or record.owner != owner
        or record.run_id != run_id
        or receipt.commit_hash != digest
        or not receipt.received_round == record.accepted_beacon == accepted_at
        or env.exp_drand != record.expires_beacon
        or accepted_at > record.expires_beacon
    ):
        raise ChallengeError(403, "operator original receipt beacon/binding differs")
    return record, digest, payload


def shadow_operator_expected(record: ShadowOperatorRecord, expected: dict, clock: float) -> None:
    if (
        canonicalize(record.model_dump(mode="json")) != canonicalize(expected)
        or clock >= record.cutoff_unix
        or record.budget.get("scope") != "FULL116_CONTINUATION"
    ):
        raise ChallengeError(403, "operator graph/workload/budget/source/cutoff differs")


_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS beacon(round INTEGER PRIMARY KEY, signature TEXT NOT NULL,
  randomness TEXT NOT NULL, bls INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS runs(run_id TEXT PRIMARY KEY, manifest TEXT NOT NULL,
  envelope TEXT NOT NULL, status TEXT NOT NULL, config TEXT);
CREATE TABLE IF NOT EXISTS roster(run_id TEXT, hotkey TEXT, bond INTEGER NOT NULL,
  probation INTEGER NOT NULL, cluster TEXT, region TEXT, flags TEXT NOT NULL,
  admitted_w INTEGER, removed INTEGER NOT NULL DEFAULT 0, blacklisted INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(run_id, hotkey));
CREATE TABLE IF NOT EXISTS rounds(run_id TEXT, w INTEGER, body TEXT NOT NULL,
  envelope TEXT NOT NULL, base INTEGER NOT NULL, reasons TEXT NOT NULL,
  applied INTEGER NOT NULL DEFAULT 0, selected TEXT, rollback TEXT, finalize TEXT,
  final_at INTEGER, theta_start TEXT, v0 TEXT, PRIMARY KEY(run_id, w));
CREATE TABLE IF NOT EXISTS miners(run_id TEXT, w INTEGER, hotkey TEXT, status TEXT NOT NULL,
  received_round INTEGER, accept TEXT, commit_env TEXT, receipt TEXT, delta TEXT,
  selected INTEGER NOT NULL DEFAULT 0, verdict TEXT, preimages TEXT, ef_in TEXT, rerun TEXT,
  PRIMARY KEY(run_id, w, hotkey));
CREATE TABLE IF NOT EXISTS seen(key TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, run_id TEXT NOT NULL, w INTEGER NOT NULL,
  target TEXT NOT NULL, challenge TEXT NOT NULL, challenge_hash TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL, lease TEXT, lease_expires INTEGER, attempts INTEGER NOT NULL DEFAULT 0,
  result TEXT, verdict TEXT, auditor TEXT, fail_reason TEXT);
CREATE TABLE IF NOT EXISTS state_serves(challenge_hash TEXT, t INTEGER, envelope TEXT NOT NULL,
  blob TEXT NOT NULL, PRIMARY KEY(challenge_hash, t));
CREATE TABLE IF NOT EXISTS forfeits(run_id TEXT, w INTEGER, hotkey TEXT, envelope TEXT NOT NULL,
  PRIMARY KEY(run_id, w, hotkey));
CREATE TABLE IF NOT EXISTS disputes(id TEXT PRIMARY KEY, run_id TEXT NOT NULL, w INTEGER NOT NULL,
  hotkey TEXT NOT NULL, auditor TEXT NOT NULL, job_id TEXT NOT NULL, action TEXT NOT NULL,
  state TEXT NOT NULL, envelope TEXT NOT NULL, resolution TEXT);
CREATE TABLE IF NOT EXISTS bisects(dispute_id TEXT, seq INTEGER, envelope TEXT NOT NULL,
  PRIMARY KEY(dispute_id, seq));
CREATE TABLE IF NOT EXISTS honeypots(run_id TEXT, commitment TEXT, reveal TEXT,
  PRIMARY KEY(run_id, commitment));
"""


class _ServiceCapacityV1(BaseModel):
    """Internal owner admission, not a public wire type or a CPU runtime benchmark."""

    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    version: StrictInt
    profile_id: Literal["tiny-cpu-service-32-v1"]
    run_id: str
    backend_binding_hash: str
    implementation_hash: str
    max_complete_roster: StrictInt
    memory_reservation_bytes: StrictInt
    max_outer_work_units: StrictInt
    max_audit_step_units: StrictInt
    max_repair_step_units: StrictInt
    max_object_bytes: StrictInt
    max_tape_bytes: StrictInt


class ChallengeError(Exception):
    def __init__(self, status: int, detail: str) -> None:
        super().__init__(detail)
        self.status = status
        self.detail = detail


_ENVELOPE_STATUS: list[tuple[type[EnvelopeError], int]] = [
    (SignatureError, 401),
    (UnauthorizedSigner, 403),
    (ReplayError, 409),
    (ExpiredError, 400),
    (RunMismatchError, 400),
    (MalformedEnvelope, 400),
]


def _dumps(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def audit_selected(run_id: str, w: int, drand_sig: bytes, hotkey: str, q_hex: str) -> bool:
    """sel = sha256('ht-audit'|run_id(32B)|u64be(w)|drand_sig[d_audit]|hotkey utf8) < q*2^256.

    q is the exact f32 value of q_i, so the comparison is integer-exact.
    """
    digest = hashlib.sha256(
        b"ht-audit" + bytes.fromhex(run_id) + struct.pack(">Q", w) + drand_sig + hotkey.encode()
    ).digest()
    q = Fraction(f32val(q_hex))
    return int.from_bytes(digest, "big") * q.denominator < q.numerator * 2**256


def assignment_hash(run_id: str, w: int, slot: int, samples: tuple[int, ...]) -> str:
    """sha256('ht-assignment-v1' | run_id(32B) | u64be(w) | u64be(slot) | u64le(sample)*)."""
    return sha256_hex(
        b"ht-assignment-v1"
        + bytes.fromhex(run_id)
        + struct.pack(">QQ", w, slot)
        + b"".join(struct.pack("<Q", i) for i in samples)
    )


def rerun_message(run_id: str, w: int, hotkey: str, rerun_leaves_root: str) -> bytes:
    """sr25519 preimage of a miner's TRANSIENT rerun claim (no protocol message type exists)."""
    return f"hypertrain/1|Rerun|{run_id}|{w}|{hotkey}|{rerun_leaves_root}".encode()


def _merge_answers(answers: list[bytes]) -> bytes:
    """One get_weights answer over per-run ledgers: each run owns 10^6 of full_share_mass.

    ponytail: equal emission share per run and no cross-run 65,536-hotkey cap; add owner
    weights per run / a merged cap when runs differ in size.
    """
    from hypertrain.ledger.journal import canonical

    parsed = [json.loads(a) for a in answers]
    weights: dict[str, float] = {}
    for a in parsed:
        for hotkey, units in a["weights"].items():
            weights[hotkey] = weights.get(hotkey, 0.0) + units
    first = parsed[0]
    meta = dict(first["metadata"])
    for key in (
        "units_paid",
        "units_burned_this_epoch",
        "hotkeys_capped",
        "ledger_minted",
        "ledger_burned",
        "ledger_paid",
        "ledger_pending",
    ):
        meta[key] = sum(a["metadata"][key] for a in parsed)
    meta["reason"] = next((r for a in parsed if (r := a["metadata"]["reason"]) != "ok"), "ok")
    meta["runs"] = len(parsed)
    return canonical(
        {
            **first,
            "weights": {h: weights[h] for h in sorted(weights)},
            "full_share_mass": sum(a["full_share_mass"] for a in parsed),
            "metadata": meta,
        }
    )


class ChallengeStore:
    def __init__(
        self,
        state_dir: Path,
        params: Params,
        coord: Keypair | None,
        owner_hotkey: str | None,
        verify_beacon: Callable[[Mapping[str, Any]], BeaconRound],
        objects: ObjectStore,
        *,
        _role_launch: (
            Callable[[str, IslandJobV1, Path, Mapping[str, Any]], IslandLaunch] | None
        ) = None,
    ) -> None:
        state_dir.mkdir(parents=True, exist_ok=True)
        self.params = params
        self.coord = coord
        self.owner_hotkey = owner_hotkey
        self.verify_beacon = verify_beacon
        self.objects = objects
        self._role_launch = _role_launch
        self._lock = threading.RLock()
        self._db = sqlite3.connect(
            state_dir / "challenge.db", check_same_thread=False, isolation_level=None
        )
        self._db.row_factory = sqlite3.Row
        self._db.execute("PRAGMA journal_mode=WAL")
        self._db.execute("PRAGMA synchronous=FULL")
        self._db.executescript(_SCHEMA)
        self.ledger = Ledger(state_dir / "ledger", params)
        self._intakes: dict[str, Intake] = {}
        self.state_dir = state_dir
        self.clock: Callable[[], float] = time.time
        self._services_v2: dict[str, tuple[Any, Any, Any]] = {}
        self.beacon_arrived = threading.Condition()
        self._lease_guards_v2: dict[str, Any] = {}
        self._ipc_v2: socket.socket | None = None
        self._ipc_path_v2: Path | None = None
        self._ipc_thread_v2: threading.Thread | None = None
        self._db.executescript("""
          CREATE TABLE IF NOT EXISTS accepted_v2(
            key TEXT PRIMARY KEY, digest TEXT NOT NULL, envelope BLOB NOT NULL,
            accepted_beacon INTEGER NOT NULL, receipt TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS records_v2(
            run_id TEXT, kind TEXT, id TEXT, data TEXT NOT NULL,
            PRIMARY KEY(run_id,kind,id));
          CREATE TABLE IF NOT EXISTS audit_leases_v2(
            id TEXT PRIMARY KEY, run_id TEXT, w INTEGER, hotkey TEXT,
            challenge TEXT, created INTEGER, absolute INTEGER, state TEXT,
            attempts INTEGER NOT NULL DEFAULT 0, auditor TEXT, nonce TEXT,
            expires INTEGER, tried TEXT NOT NULL DEFAULT '[]', reservation TEXT,
            step_budget INTEGER, result TEXT);
          CREATE TABLE IF NOT EXISTS weights_runs(epoch INTEGER PRIMARY KEY, answer BLOB NOT NULL);
        """)
        # Further runs live in isolated sub-stores; the root keeps the first (legacy) run.
        self._children: dict[str, ChallengeStore] = {}
        runs_dir = state_dir / "runs"
        for db_path in sorted(runs_dir.glob("*/challenge.db")) if runs_dir.is_dir() else ():
            child = self._open_child(db_path.parent)
            ids = [r[0] for r in child._db.execute("SELECT run_id FROM runs")]
            if not ids:  # aborted creation: nothing committed; removed on the next create
                child.close()
                continue
            if ids != [db_path.parent.name]:
                raise ChallengeError(503, f"run sub-store {db_path.parent.name} is inconsistent")
            self._children[ids[0]] = child

    @contextmanager
    def _tx(self, *, notify: bool = False) -> Iterator[sqlite3.Connection]:
        with self._lock:
            self._db.execute("BEGIN IMMEDIATE")
            try:
                yield self._db
            except BaseException:
                self._db.execute("ROLLBACK")
                raise
            self._db.execute("COMMIT")
        if notify:
            self._notify_processes_v2()

    def healthy(self) -> bool:
        try:
            with self._tx() as db:
                db.execute("INSERT OR REPLACE INTO meta VALUES('health', '1')")
        except sqlite3.Error:
            return False
        return True

    def close_v2_notifications(self) -> None:
        if self._ipc_v2 is not None:
            self._ipc_v2.sendto(b"stop", str(self._ipc_path_v2))
            if self._ipc_thread_v2 is not None:
                self._ipc_thread_v2.join(timeout=5)
            self._ipc_v2.close()
            assert self._ipc_path_v2 is not None
            self._ipc_path_v2.unlink(missing_ok=True)
            self._ipc_v2 = None
        for child in self._children.values():
            child.close_v2_notifications()

    def close(self) -> None:
        self.close_v2_notifications()
        for child in self._children.values():
            child._db.close()
        self._db.close()

    def stores(self) -> list[ChallengeStore]:
        return [self, *self._children.values()]

    def for_run(self, run_id: str) -> ChallengeStore:
        """Store hosting run_id; unknown ids resolve to the root, which answers 404."""
        return self._children.get(run_id, self)

    def for_job(self, job_id: str) -> ChallengeStore:
        for store in self.stores():
            with store._lock:
                if store._db.execute("SELECT 1 FROM jobs WHERE id=?", (job_id,)).fetchone():
                    return store
        return self

    def lease_any(self) -> dict[str, Any] | None:
        return next((job for s in self.stores() if (job := s.lease()) is not None), None)

    def get_object_any(self, sha: str) -> bytes:
        """Worker-token object read: the auditor's leased job may belong to any run."""
        for store in self._children.values():
            try:
                return store.objects.get(sha)
            except (ObjectNotFound, StoreError, ValueError):
                continue
        return self.get_object(sha)

    def _open_child(self, directory: Path) -> ChallengeStore:
        from hypertrain.data.store import LocalFSStore

        # ponytail: sub-run objects are always local; add an S3 prefix per run if needed.
        child = ChallengeStore(
            directory,
            self.params,
            self.coord,
            self.owner_hotkey,
            self.verify_beacon,
            LocalFSStore(directory / "objects"),
            _role_launch=self._role_launch,
        )
        child.clock = lambda: self.clock()
        lock = directory / "capacity-runtime.lock"
        if not lock.is_symlink():  # one host-wide capacity lock across all hosted runs
            lock.unlink(missing_ok=True)
            lock.symlink_to(self.state_dir.resolve() / "capacity-runtime.lock")
        return child

    def _create_child(
        self, run_id: str, pinned: tuple[str, ...], create: Callable[[ChallengeStore], Any]
    ) -> Any:
        """New run in runs/<run_id>/: own SQLite, ledger, objects, replay set and beacons."""
        import shutil

        with self._lock:
            hosted = self._db.execute("SELECT 1 FROM runs WHERE run_id=?", (run_id,)).fetchone()
            if hosted is not None or run_id in self._children:
                raise ChallengeError(409, "run already exists in this challenge state")
            directory = self.state_dir / "runs" / run_id
            shutil.rmtree(directory, ignore_errors=True)  # only an aborted, uncommitted create
            child = self._open_child(directory)
            try:
                with child._tx() as db:
                    db.executemany(
                        "INSERT OR IGNORE INTO beacon VALUES(?, ?, ?, ?)",
                        [tuple(r) for r in self._db.execute("SELECT * FROM beacon")],
                    )
                qualification = self._db.execute(
                    "SELECT * FROM records_v2 WHERE run_id=? AND kind='qualification'", (run_id,)
                ).fetchall()
                authorities = [json.loads(r["data"])["authority_hash"] for r in qualification]
                for sha in (*pinned, *authorities):
                    try:
                        child.objects.put(self.objects.get(sha))
                    except ObjectNotFound:
                        continue  # the child's own validation reports it
                with child._tx() as db:
                    db.executemany(
                        "INSERT INTO records_v2 VALUES(?, ?, ?, ?)",
                        [tuple(r) for r in qualification],
                    )
                result = create(child)
            except BaseException:
                child.close()
                shutil.rmtree(directory, ignore_errors=True)
                raise
            self._children[run_id] = child
            return result

    def _meta(self, db: sqlite3.Connection, key: str, default: int) -> int:
        row = db.execute("SELECT value FROM meta WHERE key=?", (key,)).fetchone()
        return int(row["value"]) if row else default

    def _set_meta(self, db: sqlite3.Connection, key: str, value: int) -> None:
        db.execute("INSERT OR REPLACE INTO meta VALUES(?, ?)", (key, str(value)))

    def _coord(self) -> Keypair:
        if self.coord is None:
            raise ChallengeError(503, "coordinator key is not configured")
        return self.coord

    def _now(self, db: sqlite3.Connection) -> int:
        row = db.execute("SELECT MAX(round) AS r FROM beacon").fetchone()
        return int(row["r"] or 0)

    def _sig(self, db: sqlite3.Connection, rnd: int) -> bytes:
        row = db.execute("SELECT signature FROM beacon WHERE round=?", (rnd,)).fetchone()
        if row is None:
            raise ChallengeError(409, f"drand round {rnd} has not been pushed")
        return bytes.fromhex(row["signature"])

    def _known_sig(self, db: sqlite3.Connection, rnd: int) -> str | None:
        row = db.execute("SELECT signature FROM beacon WHERE round=?", (rnd,)).fetchone()
        return None if row is None else str(row["signature"])

    def _run(self, db: sqlite3.Connection, run_id: str) -> tuple[sqlite3.Row, RunManifest]:
        row = db.execute("SELECT * FROM runs WHERE run_id=?", (run_id,)).fetchone()
        if row is None:
            raise ChallengeError(404, "unknown run")
        if json.loads(row["manifest"]).get("manifest_version") == 2:
            raise ChallengeError(409, "route and stored contract version differ")
        return row, RunManifest.model_validate_json(row["manifest"])

    def _round(self, db: sqlite3.Connection, run_id: str, w: int) -> tuple[sqlite3.Row, RoundOpen]:
        row = db.execute("SELECT * FROM rounds WHERE run_id=? AND w=?", (run_id, w)).fetchone()
        if row is None:
            raise ChallengeError(404, "unknown round")
        return row, RoundOpen.model_validate_json(row["body"])

    def _chain_time(self, manifest: RunManifest, rnd: int) -> int:
        return manifest.beacon.genesis_time + (max(rnd, 1) - 1) * manifest.beacon.period

    def _event_at(self, db: sqlite3.Connection, manifest: RunManifest, rnd: int) -> int:
        """Ledger event time: drand chain time, never before the last event or answered epoch_at."""
        at = max(
            self._chain_time(manifest, rnd),
            self._meta(db, "last_answer_at", -1) + 1,
            self._meta(db, "last_event_at", 0),
        )
        self._set_meta(db, "last_event_at", at)
        return at

    def _intake(self, db: sqlite3.Connection, run_id: str, manifest: RunManifest) -> Intake:
        intake = self._intakes.get(run_id)
        if intake is None:
            auditors = set(manifest.auditors)
            coord = manifest.coord_pubkey
            allowed: dict[str, Callable[[str], bool]] = {
                "ReplayVerdict": auditors.__contains__,
                "Resolution": auditors.__contains__,
                **{t: coord.__eq__ for t in COORD_TYPES},
            }
            prefix = _dumps([run_id])[:-1] + ","
            seen = {
                tuple(json.loads(r["key"]))
                for r in db.execute("SELECT key FROM seen WHERE key LIKE ?", (prefix + "%",))
            }
            intake = Intake(run_id, allowed, seen)
            self._intakes[run_id] = intake
        return intake

    def _accept_signed(
        self, db: sqlite3.Connection, run_id: str, env: Any, expected: str
    ) -> tuple[RunManifest, BaseModel, tuple[str, ...]]:
        _, manifest = self._run(db, run_id)
        if not isinstance(env, Mapping) or env.get("type") != expected:
            raise ChallengeError(400, f"expected a signed {expected} envelope")
        intake = self._intake(db, run_id, manifest)
        try:
            model = intake.accept(env, self._now(db))
        except EnvelopeError as error:
            status = next(s for cls, s in _ENVELOPE_STATUS if isinstance(error, cls))
            raise ChallengeError(status, str(error)) from None
        return manifest, model, replay_key(expected, run_id, env["signer"], model)

    def _signed(
        self,
        run_id: str,
        env: Any,
        expected: str,
        handler: Callable[[sqlite3.Connection, RunManifest, Any, Mapping[str, Any]], Any],
    ) -> Any:
        """Verify a signed envelope, run the handler in one transaction, persist the replay key.

        A handler rejection releases the replay key so a corrected resend is not blocked.
        """
        with self._tx() as db:
            manifest, model, key = self._accept_signed(db, run_id, env, expected)
            try:
                result = handler(db, manifest, model, env)
                db.execute("INSERT INTO seen VALUES(?)", (_dumps(list(key)),))
            except BaseException:
                self._intakes[run_id].seen.discard(key)
                raise
            return result

    def push_beacon(self, payload: Mapping[str, Any]) -> dict[str, Any]:
        try:
            br = self.verify_beacon(payload)
        except BeaconError as error:
            raise ChallengeError(400, f"beacon round rejected: {error}") from None
        with self._tx() as db:
            known = db.execute("SELECT * FROM beacon WHERE round=?", (br.round,)).fetchone()
            if known is not None and known["signature"] != br.signature:
                raise ChallengeError(409, f"conflicting payload for drand round {br.round}")
            if known is None:
                db.execute(
                    "INSERT INTO beacon VALUES(?, ?, ?, ?)",
                    (br.round, br.signature, br.randomness, int(br.bls_verified)),
                )
            self._advance(db)
            result = {"round": br.round, "latest": self._now(db)}
        with self.beacon_arrived:
            self.beacon_arrived.notify_all()
        for guard in self._lease_guards_v2.values():
            guard.advance(br.round)
        for run_id, (escrow, _, _) in self._services_v2.items():
            with self._tx():
                origins = self._db.execute(
                    "SELECT o.origin FROM escrow_origins o JOIN escrow_units u "
                    "ON o.origin=u.origin WHERE o.mature_at<=? "
                    "AND u.bucket='reward_pending' AND u.units>0",
                    (br.round,),
                ).fetchall()
                for origin in origins:
                    try:
                        escrow.mature(
                            sha256_hex(f"ht-mature/2|{run_id}|{origin[0]}".encode()),
                            (origin[0],),
                            br.round,
                        )
                    except EscrowError as error:
                        if error.code not in (
                            "DISPUTED_OR_UNVESTED",
                            "IMMATURE_OR_UNKNOWN_ORIGIN",
                            "ALREADY_MATURED",
                        ):
                            raise
                self._expire_leases_v2(run_id, br.round)
        for _, _, disputes in self._services_v2.values():
            disputes.notify()
        self._notify_processes_v2()
        for child in self._children.values():
            child.push_beacon(payload)
        return result

    def latest_beacon(self) -> dict[str, Any]:
        with self._lock:
            row = self._db.execute("SELECT * FROM beacon ORDER BY round DESC LIMIT 1").fetchone()
        if row is None:
            return {"round": 0}
        return {
            "round": row["round"],
            "signature": row["signature"],
            "randomness": row["randomness"],
        }

    def create_run(self, env: Any) -> dict[str, Any]:
        with self._lock:
            return self._create_run(env)

    def _create_run(self, env: Any) -> dict[str, Any]:
        if self.owner_hotkey is None:
            raise ChallengeError(503, "owner hotkey is not configured")
        coord = self._coord()
        if not isinstance(env, Mapping) or env.get("type") != "RunManifest":
            raise ChallengeError(400, "expected a signed RunManifest envelope")
        intake = Intake(str(env.get("run_id")), {"RunManifest": self.owner_hotkey.__eq__})
        with self._tx() as db:
            try:
                manifest = intake.accept(env, self._now(db))
            except EnvelopeError as error:
                status = next(s for cls, s in _ENVELOPE_STATUS if isinstance(error, cls))
                raise ChallengeError(status, str(error)) from None
            assert isinstance(manifest, RunManifest)
            run_id = manifest.run_id()
            if run_id != env["run_id"]:
                raise ChallengeError(400, "run_id must be sha256(JCS(manifest body))")
            if manifest.coord_pubkey != coord.ss58:
                raise ChallengeError(422, "manifest coord_pubkey is not this container's K_coord")
            p = self.params
            mv = manifest.verify
            if (manifest.budget.epochs_per_round, mv.E_vest_rounds, mv.forgive_per_epoch) != (
                p.epochs_per_round,
                p.vest_rounds,
                p.forgive_per_epoch,
            ):
                raise ChallengeError(422, "manifest budget/vesting must match the ledger params")
            hosted = db.execute("SELECT 1 FROM runs").fetchone() is not None
            if not hosted:
                db.execute(
                    "INSERT INTO runs VALUES(?, ?, ?, 'created', NULL)",
                    (run_id, manifest.model_dump_json(), _dumps(dict(env))),
                )
        if hosted:  # every run has its own ledger, round sequence and replay set
            result: dict[str, Any] = self._create_child(run_id, (), lambda c: c.create_run(env))
            return result
        return {"run_id": run_id, "status": "created"}

    def configure(self, run_id: str, config: dict[str, int]) -> dict[str, Any]:
        with self._tx() as db:
            row, _ = self._run(db, run_id)
            if row["status"] == "running":
                raise ChallengeError(409, "pause the run before reconfiguring it")
            db.execute("UPDATE runs SET config=? WHERE run_id=?", (_dumps(config), run_id))
        return {"run_id": run_id, "config": config}

    def set_paused(self, run_id: str, paused: bool) -> dict[str, Any]:
        with self._tx() as db:
            row, manifest = self._run(db, run_id)
            if paused:
                db.execute("UPDATE runs SET status='paused' WHERE run_id=?", (run_id,))
                return {"run_id": run_id, "status": "paused"}
            if row["config"] is None:
                raise ChallengeError(409, "configure the run before unpausing it")
            if self._now(db) < 1:
                raise ChallengeError(409, "no drand round has been pushed yet")
            db.execute("UPDATE runs SET status='running' WHERE run_id=?", (run_id,))
            if db.execute("SELECT 1 FROM rounds WHERE run_id=?", (run_id,)).fetchone() is None:
                self._open_round_zero(db, run_id, manifest)
            return {"run_id": run_id, "status": "running"}

    def set_roster(self, run_id: str, hotkey: str, entry: dict[str, Any] | None) -> dict[str, Any]:
        with self._tx() as db:
            self._run(db, run_id)
            if entry is None:
                db.execute(
                    "UPDATE roster SET removed=1 WHERE run_id=? AND hotkey=?", (run_id, hotkey)
                )
                return {"hotkey": hotkey, "removed": True}
            db.execute(
                "INSERT INTO roster(run_id, hotkey, bond, probation, cluster, region, flags) "
                "VALUES(?, ?, ?, ?, ?, ?, ?) ON CONFLICT(run_id, hotkey) DO UPDATE SET "
                "bond=excluded.bond, probation=excluded.probation, cluster=excluded.cluster, "
                "region=excluded.region, flags=excluded.flags, removed=0",
                (
                    run_id,
                    hotkey,
                    int(entry["bond"]),
                    int(entry["probation"]),
                    entry["cluster"],
                    entry["region"],
                    _dumps(sorted(entry["flags"])),
                ),
            )
            return {"hotkey": hotkey, **entry}

    def set_honeypot(self, run_id: str, commitment: str) -> dict[str, Any]:
        with self._tx() as db:
            self._run(db, run_id)
            db.execute("INSERT OR IGNORE INTO honeypots VALUES(?, ?, NULL)", (run_id, commitment))
            db.execute(
                "INSERT OR REPLACE INTO meta VALUES(?, ?)", (f"honeypot:{run_id}", commitment)
            )
        return {"run_id": run_id, "honeypot_commit": commitment}

    def reveal_honeypot(
        self, run_id: str, commitment: str, members: list[dict[str, str]], salt: str
    ) -> dict[str, Any]:
        from hypertrain.auditor.honeypot import Honeypot, verify_reveal

        pots = [Honeypot(m["hotkey"], m["mode"]) for m in members]  # type: ignore[arg-type]
        if not verify_reveal(commitment, pots, bytes.fromhex(salt)):
            raise ChallengeError(422, "reveal does not match the commitment")
        with self._tx() as db:
            self._run(db, run_id)
            row = db.execute(
                "SELECT reveal FROM honeypots WHERE run_id=? AND commitment=?", (run_id, commitment)
            ).fetchone()
            if row is None:
                raise ChallengeError(404, "unknown honeypot commitment")
            if row["reveal"] is not None:
                raise ChallengeError(409, "commitment already revealed")
            reveal = {"members": sorted(members, key=lambda m: m["hotkey"]), "salt": salt}
            db.execute(
                "UPDATE honeypots SET reveal=? WHERE run_id=? AND commitment=?",
                (_dumps(reveal), run_id, commitment),
            )
        return {"commitment": commitment, "verified": True}

    def _template(
        self, db: sqlite3.Connection, run_id: str, manifest: RunManifest, w: int, d_open: int
    ) -> tuple[dict[str, Any], dict[str, list[str]]]:
        row, _ = self._run(db, run_id)
        cfg = json.loads(row["config"] or "null")
        if cfg is None:
            raise ChallengeError(409, "run is not configured")
        mv, t = manifest.verify, manifest.verify.T
        faulted = {
            r["cluster"]
            for r in db.execute(
                "SELECT cluster FROM roster WHERE run_id=? AND blacklisted=1 "
                "AND cluster IS NOT NULL",
                (run_id,),
            )
        }
        members = db.execute(
            "SELECT * FROM roster WHERE run_id=? AND removed=0 AND blacklisted=0 ORDER BY hotkey",
            (run_id,),
        ).fetchall()
        roster: list[dict[str, Any]] = []
        reasons: dict[str, list[str]] = {}
        for slot, m in enumerate(members):
            why: list[str] = []
            admitted = w if m["admitted_w"] is None else m["admitted_w"]
            if m["probation"] or (not m["bond"] and w - admitted < mv.probation_rounds):
                why.append("probation")
            if {"outlier", "copy"} & set(json.loads(m["flags"])):
                why.append("outlier")
            if m["cluster"] is not None and m["cluster"] in faulted:
                why.append("cluster")
            reasons[m["hotkey"]] = why or ["random"]
            q = f32hex(1.0) if why else mv.q_base
            roster.append({"hotkey": m["hotkey"], "slot": slot, "q_i": q})
        d_assign = d_open + t.assign_after_open
        d_commit = d_assign + cfg["train_rounds"]
        d_audit = d_commit + t.audit_after_commit
        d_upload = d_commit + t.upload_after_commit
        commit = db.execute(
            "SELECT value FROM meta WHERE key=?", (f"honeypot:{run_id}",)
        ).fetchone()
        body = {
            "w": w,
            "roster": roster,
            "roster_hash": sha256_hex(canonicalize(roster, allow_float=False)),
            "honeypot_commit": commit["value"] if commit else NO_HONEYPOT,
            "d_open": d_open,
            "d_assign": d_assign,
            "d_commit": d_commit,
            "d_audit": d_audit,
            "d_upload": d_upload,
            "d_final": max(d_audit, d_upload) + cfg["final_after_upload"],
        }
        return body, reasons

    def _batch(self, manifest: RunManifest) -> int:
        inner = manifest.inner
        return inner.micro_batch * inner.grad_accum * inner.H

    def _next_base(self, manifest: RunManifest, base: int, prev_slots: int, slots: int) -> int:
        n, batch = manifest.dataset.n_samples, self._batch(manifest)
        nxt = base + prev_slots * batch
        if slots * batch > n:
            raise ChallengeError(409, "roster x batch exceeds the dataset size")
        if slots and (nxt + slots * batch - 1) // n != nxt // n:
            nxt = (nxt // n + 1) * n
        return nxt

    def _store_round(
        self,
        db: sqlite3.Connection,
        run_id: str,
        env: Mapping[str, Any],
        body: RoundOpen,
        base: int,
        reasons: dict[str, list[str]],
    ) -> None:
        db.execute(
            "INSERT INTO rounds(run_id, w, body, envelope, base, reasons) VALUES(?, ?, ?, ?, ?, ?)",
            (run_id, body.w, body.model_dump_json(), _dumps(dict(env)), base, _dumps(reasons)),
        )
        for entry in body.roster:
            db.execute(
                "UPDATE roster SET admitted_w=? WHERE run_id=? AND hotkey=? AND admitted_w IS NULL",
                (body.w, run_id, entry.hotkey),
            )
            db.execute(
                "INSERT INTO miners(run_id, w, hotkey, status) VALUES(?, ?, ?, 'ASSIGNED')",
                (run_id, body.w, entry.hotkey),
            )

    def _open_round_zero(self, db: sqlite3.Connection, run_id: str, manifest: RunManifest) -> None:
        tmpl, reasons = self._template(db, run_id, manifest, 0, self._now(db) + 1)
        if not tmpl["roster"]:
            raise ChallengeError(409, "admit at least one miner before unpausing")
        body = RoundOpen.model_validate(
            {
                **tmpl,
                "prev_final_hash": manifest.init_state_hash,
                "theta_hash": manifest.init_state_hash,
                "outer_state_hash": ZERO_STATE,
                "center_hash": ZERO_STATE,
            }
        )
        base = self._next_base(manifest, 0, 0, len(body.roster))
        env = seal(self._coord(), "RoundOpen", run_id, body, NEVER)
        self._store_round(db, run_id, env, body, base, reasons)

    def round_state(self, db: sqlite3.Connection, row: sqlite3.Row, body: RoundOpen) -> str:
        d = self._now(db)
        if row["finalize"] is not None:
            final_round = self.params.round_of(row["final_at"])
            now_round = self.params.round_of(self._chain_time_run(db, row["run_id"], d))
            if now_round >= final_round + self.params.vest_rounds:
                return "RELEASED"
            return "VESTING" if now_round > final_round else "FINAL"
        if db.execute(
            "SELECT 1 FROM disputes WHERE run_id=? AND w=? AND state='open'",
            (row["run_id"], body.w),
        ).fetchone():
            return "DISPUTE"
        if row["applied"]:
            nxt = db.execute(
                "SELECT body FROM rounds WHERE run_id=? AND w=?", (row["run_id"], body.w + 1)
            ).fetchone()
            d_assign_next = RoundOpen.model_validate_json(nxt["body"]).d_assign if nxt else 0
            return "AUDIT" if d >= d_assign_next else "APPLIED"
        for state, deadline in (
            ("UPLOAD_CLOSED", body.d_upload),
            ("COMMIT_CLOSED", body.d_commit),
        ):
            if d >= deadline:
                return state
        if d > body.d_assign:
            return "TRAINING"
        return "ASSIGNED" if d >= body.d_assign else "OPEN"

    def _chain_time_run(self, db: sqlite3.Connection, run_id: str, rnd: int) -> int:
        _, manifest = self._run(db, run_id)
        return self._chain_time(manifest, rnd)

    def _assignment(
        self, db: sqlite3.Connection, run_id: str, manifest: RunManifest, row: sqlite3.Row
    ) -> tuple[tuple[int, ...], ...]:
        body = RoundOpen.model_validate_json(row["body"])
        a = assign_round(
            bytes.fromhex(run_id),
            body.w,
            self._sig(db, body.d_assign),
            n_samples=manifest.dataset.n_samples,
            n_slots=len(body.roster),
            batch=self._batch(manifest),
            base_w=row["base"],
            unit=manifest.dataset.assign_unit or 1,
        )
        return a.slices

    def _advance(self, db: sqlite3.Connection) -> None:
        """Audit selection for every round whose d_audit beacon is known (idempotent)."""
        d = self._now(db)
        for row in db.execute("SELECT * FROM rounds WHERE selected IS NULL").fetchall():
            body = RoundOpen.model_validate_json(row["body"])
            if d < body.d_audit:
                continue
            sig = db.execute(
                "SELECT signature FROM beacon WHERE round=?", (body.d_audit,)
            ).fetchone()
            if sig is None:
                continue
            self._select(db, row, body, bytes.fromhex(sig["signature"]))

    def _select(
        self, db: sqlite3.Connection, row: sqlite3.Row, body: RoundOpen, sig: bytes
    ) -> None:
        run_id = row["run_id"]
        _, manifest = self._run(db, run_id)
        reasons = json.loads(row["reasons"])
        q = {e.hotkey: e.q_i for e in body.roster}
        committed = db.execute(
            "SELECT hotkey, preimages FROM miners WHERE run_id=? AND w=? "
            "AND status IN ('COMMITTED','UPLOADED') "
            "ORDER BY hotkey",
            (run_id, body.w),
        ).fetchall()
        segments_mode = manifest.inner.state_policy == "carry"
        picked = [
            m for m in committed if audit_selected(run_id, body.w, sig, m["hotkey"], q[m["hotkey"]])
        ]
        if segments_mode and self._now(db) < body.d_upload:
            if any(m["preimages"] is None for m in picked):
                return  # segments are drawn from committed norms: wait for leaves or d_upload
        chosen: list[str] = []
        for m in picked:
            hk = m["hotkey"]
            chosen.append(hk)
            why = list(reasons[hk])
            segments: list[list[int]] = []
            if segments_mode:
                segments = self._segments(manifest, run_id, body.w, sha256_hex(sig), m)
                why.append("final")
            challenge = {
                "w": body.w,
                "target": hk,
                "beacon_round": body.d_audit,
                "beacon_sig_sha256": sha256_hex(sig),
                "mode": "segments" if segments_mode else "full",
                "segments": segments,
                "reasons": why,
                "serve_deadline": body.d_audit + manifest.verify.T.serve_deadline,
            }
            AuditChallenge.model_validate(challenge)
            env = seal(self._coord(), "AuditChallenge", run_id, challenge, NEVER)
            db.execute(
                "INSERT INTO jobs(id, run_id, w, target, challenge, challenge_hash, state) "
                "VALUES(?, ?, ?, ?, ?, ?, 'queued')",
                (
                    "a_" + secrets.token_hex(8),
                    run_id,
                    body.w,
                    hk,
                    _dumps(env),
                    body_digest(challenge),
                ),
            )
            db.execute(
                "UPDATE miners SET selected=1 WHERE run_id=? AND w=? AND hotkey=?",
                (run_id, body.w, hk),
            )
        db.execute(
            "UPDATE rounds SET selected=? WHERE run_id=? AND w=?", (_dumps(chosen), run_id, body.w)
        )

    def _segments(
        self, manifest: RunManifest, run_id: str, w: int, sig_sha256: str, m: sqlite3.Row
    ) -> list[list[int]]:
        """Leaf windows [i, i+1] from auditor.replay.select_segments (the auditor re-derives
        them): k drand-keyed + final always + Q_top largest committed norms. Without stored
        preimages (none by d_upload) the norms are zeros, so top-Q falls back to lowest indices.
        """
        from hypertrain.auditor.replay import committed_norms, select_segments

        u = manifest.inner.H // manifest.inner.J
        if m["preimages"] is None:
            norms = [0.0] * u
        else:
            pres = [LeafPreimage.model_validate(x) for x in json.loads(m["preimages"])]
            norms = committed_norms(pres)
        v = manifest.verify
        segs = select_segments(run_id, w, sig_sha256, m["hotkey"], norms, v.k_segments, v.Q_top)
        return [[a, b] for a, b in segs]

    def _miner(self, db: sqlite3.Connection, run_id: str, w: int, hotkey: str) -> sqlite3.Row:
        row = db.execute(
            "SELECT * FROM miners WHERE run_id=? AND w=? AND hotkey=?", (run_id, w, hotkey)
        ).fetchone()
        if row is None:
            raise ChallengeError(403, "hotkey is not on this round's roster")
        return row

    def accept(self, run_id: str, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Accept, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            row, body = self._round(db, run_id, m.w)
            miner = self._miner(db, run_id, m.w, m.hotkey)
            d = self._now(db)
            if not body.d_assign <= d < body.d_commit:
                raise ChallengeError(409, "Accept is only open from d_assign until d_commit")
            if miner["status"] != "ASSIGNED":
                raise ChallengeError(409, f"miner is {miner['status']}")
            ref = manifest.reference_spec
            if m.image_digest != ref.image_digest:
                raise ChallengeError(422, "image_digest is not the reference image")
            if ref.driver_allowlist and m.driver_version not in ref.driver_allowlist:
                raise ChallengeError(422, "driver_version is not allowlisted")
            slot = next(e.slot for e in body.roster if e.hotkey == m.hotkey)
            samples = self._assignment(db, run_id, manifest, row)[slot]
            if m.assignment_hash != assignment_hash(run_id, m.w, slot, samples):
                raise ChallengeError(422, "assignment_hash does not match the derived assignment")
            db.execute(
                "UPDATE miners SET status='ACCEPTED', accept=? WHERE run_id=? AND w=? AND hotkey=?",
                (_dumps(dict(env)), run_id, m.w, m.hotkey),
            )
            return {"w": m.w, "hotkey": m.hotkey, "status": "ACCEPTED", "slot": slot}

        result: dict[str, Any] = self._signed(run_id, env, "Accept", handle)
        return result

    def commit(self, run_id: str, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Commit, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            _, body = self._round(db, run_id, m.w)
            miner = self._miner(db, run_id, m.w, m.hotkey)
            if miner["status"] != "ACCEPTED":
                raise ChallengeError(409, f"miner is {miner['status']}, Accept first")
            if m.n_leaves != manifest.inner.H // manifest.inner.J + 1:
                raise ChallengeError(422, "n_leaves must be H/J + 1")
            received = self._now(db)
            receipt = {
                "w": m.w,
                "commit_hash": body_digest(env["body"]),
                "received_round": received,
            }
            signed = seal(self._coord(), "Receipt", run_id, receipt, NEVER)
            status = "EXCLUDED" if received >= body.d_audit else "COMMITTED"
            db.execute(
                "UPDATE miners SET status=?, received_round=?, commit_env=?, receipt=? "
                "WHERE run_id=? AND w=? AND hotkey=?",
                (status, received, _dumps(dict(env)), _dumps(signed), run_id, m.w, m.hotkey),
            )
            if status == "COMMITTED":
                try:
                    self.ledger.commit(m.w, m.hotkey, self._event_at(db, manifest, received))
                except LedgerError as error:
                    raise ChallengeError(409, f"ledger rejected the commit: {error}") from None
            return {"status": status, "receipt": signed}

        result: dict[str, Any] = self._signed(run_id, env, "Commit", handle)
        return result

    def delta(self, run_id: str, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: DeltaManifest, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            _, body = self._round(db, run_id, m.w)
            miner = self._miner(db, run_id, m.w, m.hotkey)
            if miner["status"] != "COMMITTED":
                raise ChallengeError(409, f"miner is {miner['status']}")
            if self._now(db) >= body.d_upload:
                raise ChallengeError(409, "upload window closed at d_upload")
            commit = Commit.model_validate(json.loads(miner["commit_env"])["body"])
            if (m.delta_hash, m.size) != (commit.delta_hash, commit.delta_bytes):
                raise ChallengeError(422, "delta_hash/size differ from the Commit")
            db.execute(
                "UPDATE miners SET status='UPLOADED', delta=? WHERE run_id=? AND w=? AND hotkey=?",
                (_dumps(dict(env)), run_id, m.w, m.hotkey),
            )
            return {"w": m.w, "hotkey": m.hotkey, "status": "UPLOADED"}

        result: dict[str, Any] = self._signed(run_id, env, "DeltaManifest", handle)
        return result

    def upload_url(self, run_id: str, w: int, hotkey: str, sha: str) -> dict[str, Any]:
        with self._tx() as db:
            _, body = self._round(db, run_id, w)
            miner = self._miner(db, run_id, w, hotkey)
            if miner["status"] != "COMMITTED" or self._now(db) >= body.d_upload:
                raise ChallengeError(409, "uploads need a COMMITTED miner before d_upload")
            commit = Commit.model_validate(json.loads(miner["commit_env"])["body"])
            if commit.delta_hash != sha:
                raise ChallengeError(422, "sha256 is not the committed delta_hash")
        return {"method": "PUT", "sha256": sha, "url": self.objects.presign(sha, "PUT")}

    def state_serve(self, run_id: str, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: StateServe, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            job = db.execute(
                "SELECT * FROM jobs WHERE run_id=? AND challenge_hash=?", (run_id, m.challenge_hash)
            ).fetchone()
            if job is None or job["target"] != m.hotkey:
                raise ChallengeError(404, "no audit challenge for this hotkey")
            challenge = AuditChallenge.model_validate(json.loads(job["challenge"])["body"])
            if challenge.mode != "segments":
                raise ChallengeError(409, "full-mode audits need no StateServe")
            if self._now(db) > challenge.serve_deadline:
                raise ChallengeError(409, "serve_deadline passed")
            blob = m.uri.rsplit("/", 1)[-1]
            if self._served_root(blob) != m.tensor_root:
                raise ChallengeError(422, "tensor_root is not replay.tensor_root of the blob")
            db.execute(
                "INSERT OR REPLACE INTO state_serves VALUES(?, ?, ?, ?)",
                (m.challenge_hash, m.t, _dumps(dict(env)), blob),
            )
            return {"challenge_hash": m.challenge_hash, "t": m.t, "stored": True}

        result: dict[str, Any] = self._signed(run_id, env, "StateServe", handle)
        return result

    def _served_root(self, blob_sha: str) -> str | None:
        """tensor_root = hypertrain.auditor.replay.tensor_root of the uploaded state blob."""
        from hypertrain.auditor.replay import tensor_root, unpack_state

        theta, st = unpack_state(self.get_object(blob_sha))
        return None if st is None else tensor_root(theta, st)

    def dispute(self, run_id: str, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Dispute, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            job = next(
                (
                    j
                    for j in db.execute(
                        "SELECT * FROM jobs WHERE run_id=? AND target=? AND verdict IS NOT NULL",
                        (run_id, m.hotkey),
                    )
                    if body_digest(json.loads(j["verdict"])["body"]) == m.verdict_hash
                ),
                None,
            )
            if job is None:
                raise ChallengeError(404, "no verdict with this hash against this hotkey")
            dispute_id = body_digest(env["body"])
            state = "open" if m.action == "contest" else "accepted"
            db.execute(
                "INSERT INTO disputes VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
                (
                    dispute_id,
                    run_id,
                    job["w"],
                    m.hotkey,
                    job["auditor"],
                    job["id"],
                    m.action,
                    state,
                    _dumps(dict(env)),
                ),
            )
            return {"dispute_id": dispute_id, "state": state}

        result: dict[str, Any] = self._signed(run_id, env, "Dispute", handle)
        return result

    def _open_dispute(self, db: sqlite3.Connection, run_id: str, dispute_id: str) -> sqlite3.Row:
        row = db.execute(
            "SELECT * FROM disputes WHERE id=? AND run_id=?", (dispute_id, run_id)
        ).fetchone()
        if row is None or row["state"] != "open":
            raise ChallengeError(409, "dispute is not open")
        return row

    def bisect(self, run_id: str, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Bisect, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            row = self._open_dispute(db, run_id, m.dispute_id)
            if env["signer"] != m.party or m.party not in (row["hotkey"], row["auditor"]):
                raise ChallengeError(403, "only the disputing miner or auditor may bisect")
            seq = db.execute(
                "SELECT COUNT(*) AS n FROM bisects WHERE dispute_id=?", (m.dispute_id,)
            ).fetchone()["n"]
            db.execute(
                "INSERT INTO bisects VALUES(?, ?, ?)", (m.dispute_id, seq, _dumps(dict(env)))
            )
            return {"dispute_id": m.dispute_id, "seq": seq}

        result: dict[str, Any] = self._signed(run_id, env, "Bisect", handle)
        return result

    def resolution(self, run_id: str, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Resolution, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            row = self._open_dispute(db, run_id, m.dispute_id)
            if env["signer"] == row["auditor"]:
                raise ChallengeError(403, "the referee must not be the disputed auditor")
            if m.loser not in (row["hotkey"], row["auditor"]):
                raise ChallengeError(422, "loser must be the miner or the auditor")
            db.execute(
                "UPDATE disputes SET state='resolved', resolution=? WHERE id=?",
                (_dumps(dict(env)), m.dispute_id),
            )
            out: dict[str, Any] = {"dispute_id": m.dispute_id, "loser": m.loser}
            finalized = db.execute(
                "SELECT finalize FROM rounds WHERE run_id=? AND w=?", (run_id, row["w"])
            ).fetchone()["finalize"]
            if m.loser == row["hotkey"] and finalized is not None:
                at = self._event_at(db, manifest, self._now(db))
                try:
                    out["faulted"] = self.ledger.fault(run_id, row["w"], row["hotkey"], at)
                except LedgerError as error:
                    raise ChallengeError(409, f"ledger rejected the fault: {error}") from None
                db.execute(
                    "UPDATE roster SET blacklisted=1 WHERE run_id=? AND hotkey=?",
                    (run_id, row["hotkey"]),
                )
                self._reseal_forfeit(
                    db, run_id, row["w"], row["hotkey"], "DISPUTE_LOST", [body_digest(env["body"])]
                )
            if m.loser == row["auditor"]:
                db.execute(
                    "UPDATE miners SET status='MATCH' WHERE run_id=? AND w=? AND hotkey=?",
                    (run_id, row["w"], row["hotkey"]),
                )
                if finalized is not None:
                    at = self._event_at(db, manifest, self._now(db))
                    try:
                        out["credited"] = self.ledger.vindicate(run_id, row["w"], row["hotkey"], at)
                    except LedgerError as error:
                        raise ChallengeError(409, f"ledger rejected the credit: {error}") from None
                db.execute(
                    "UPDATE roster SET blacklisted=0 WHERE run_id=? AND hotkey=?",
                    (run_id, row["hotkey"]),
                )
                db.execute(
                    "DELETE FROM forfeits WHERE run_id=? AND w=? AND hotkey=?",
                    (run_id, row["w"], row["hotkey"]),
                )
            return out

        result: dict[str, Any] = self._signed(run_id, env, "Resolution", handle)
        return result

    def lease(self) -> dict[str, Any] | None:
        with self._tx() as db:
            self._advance(db)
            d = self._now(db)
            db.execute(
                "UPDATE jobs SET state='queued', lease=NULL "
                "WHERE state='leased' AND lease_expires<?",
                (d,),
            )
            job = next(
                (
                    j
                    for j in db.execute("SELECT * FROM jobs WHERE state='queued' ORDER BY w, rowid")
                    if self._replayable(db, j)
                ),
                None,
            )
            if job is None:
                return None
            lease = secrets.token_hex(16)
            db.execute(
                "UPDATE jobs SET state='leased', lease=?, lease_expires=?, attempts=attempts+1 "
                "WHERE id=?",
                (lease, d + LEASE_ROUNDS, job["id"]),
            )
            round_row, body = self._round(db, job["run_id"], job["w"])
            _, manifest = self._run(db, job["run_id"])
            miner = self._miner(db, job["run_id"], job["w"], job["target"])
            serves = [
                json.loads(r["envelope"])
                for r in db.execute(
                    "SELECT envelope FROM state_serves WHERE challenge_hash=? ORDER BY t",
                    (job["challenge_hash"],),
                )
            ]
            slot = next(e.slot for e in body.roster if e.hotkey == job["target"])
            preimages = json.loads(miner["preimages"])
            commit_env = json.loads(miner["commit_env"])
            return {
                "manifest": manifest.body(),
                "commit_envelope": commit_env,
                "challenge_envelope": json.loads(job["challenge"]),
                "leaves": [LeafPreimage.model_validate(x).digest() for x in preimages],
                "preimages": preimages,
                "assignment": {
                    "sample_ids": list(
                        self._assignment(db, job["run_id"], manifest, round_row)[slot]
                    ),
                    "global_step0": job["w"] * manifest.inner.H,
                },
                "theta_start_sha256": round_row["theta_start"],
                "ef_in_sha256": miner["ef_in"],
                "v0_sha256": round_row["v0"],
                "id": job["id"],
                "lease": lease,
                "lease_expires_round": d + LEASE_ROUNDS,
                "run_id": job["run_id"],
                "w": job["w"],
                "target": job["target"],
                "challenge": json.loads(job["challenge"])["body"],
                "challenge_hash": job["challenge_hash"],
                "round_open": json.loads(round_row["envelope"]),
                "commit": commit_env["body"],
                "delta_manifest": json.loads(miner["delta"]) if miner["delta"] else None,
                "state_serves": serves,
            }

    def _leased(self, db: sqlite3.Connection, job_id: str, lease: str) -> sqlite3.Row:
        job = db.execute("SELECT * FROM jobs WHERE id=?", (job_id,)).fetchone()
        if job is None:
            raise ChallengeError(404, "unknown job")
        if job["state"] != "leased" or job["lease"] != lease:
            raise ChallengeError(409, "lease is not current")
        if job["lease_expires"] < self._now(db):
            raise ChallengeError(409, "lease expired")
        return job

    def _replayable(self, db: sqlite3.Connection, job: sqlite3.Row) -> bool:
        round_row, _ = self._round(db, job["run_id"], job["w"])
        miner = self._miner(db, job["run_id"], job["w"], job["target"])
        return round_row["theta_start"] is not None and miner["preimages"] is not None

    def serves(self, job_id: str, lease: str) -> dict[str, Any]:
        with self._tx() as db:
            job = self._leased(db, job_id, lease)
            rows = db.execute(
                "SELECT envelope, blob FROM state_serves WHERE challenge_hash=? ORDER BY t",
                (job["challenge_hash"],),
            ).fetchall()
            return {
                "now_round": self._now(db),
                "serves": [
                    {"serve": json.loads(r["envelope"])["body"], "blob_sha256": r["blob"]}
                    for r in rows
                ],
            }

    def get_object(self, sha: str) -> bytes:
        try:
            return self.objects.get(sha)
        except ObjectNotFound:
            raise ChallengeError(404, "unknown object") from None
        except (StoreError, ValueError) as error:
            raise ChallengeError(400, f"bad object key: {error}") from None

    def _require_object(self, sha: str) -> None:
        self.get_object(sha)

    def set_round_state(
        self, run_id: str, w: int, theta_start: str, v0: str | None
    ) -> dict[str, Any]:
        for sha in (theta_start, v0):
            if sha is not None:
                self._require_object(sha)
        with self._tx() as db:
            row, _ = self._round(db, run_id, w)
            if row["theta_start"] is not None and row["theta_start"] != theta_start:
                raise ChallengeError(409, "round start state is already published")
            db.execute(
                "UPDATE rounds SET theta_start=?, v0=? WHERE run_id=? AND w=?",
                (theta_start, v0, run_id, w),
            )
        return {"w": w, "theta_start_sha256": theta_start, "v0_sha256": v0}

    def leaves(
        self, run_id: str, w: int, hotkey: str, preimages: list[Any], ef_in: str | None
    ) -> dict[str, Any]:
        """Leaf preimages are self-authenticating: their Merkle root must equal leaves_root."""
        try:
            pres = [LeafPreimage.model_validate(x) for x in preimages]
        except ValueError:
            raise ChallengeError(422, "invalid LeafPreimage") from None
        if ef_in is not None:
            self._require_object(ef_in)
        with self._tx() as db:
            _, body = self._round(db, run_id, w)
            miner = self._miner(db, run_id, w, hotkey)
            if miner["status"] not in ("COMMITTED", "UPLOADED") or self._now(db) >= body.d_upload:
                raise ChallengeError(409, "leaves need a committed miner before d_upload")
            commit = Commit.model_validate(json.loads(miner["commit_env"])["body"])
            if len(pres) != commit.n_leaves or any((x.run_id, x.w) != (run_id, w) for x in pres):
                raise ChallengeError(422, "preimages must be n_leaves leaves of this run/round")
            root = MerkleTree([bytes.fromhex(x.digest()) for x in pres]).root.hex()
            if root != commit.leaves_root:
                raise ChallengeError(422, "preimages do not hash to the committed leaves_root")
            db.execute(
                "UPDATE miners SET preimages=?, ef_in=? WHERE run_id=? AND w=? AND hotkey=?",
                (_dumps([x.model_dump(mode="json") for x in pres]), ef_in, run_id, w, hotkey),
            )
        return {"w": w, "hotkey": hotkey, "leaves_root": root}

    def rerun(self, run_id: str, w: int, hotkey: str, root: str, sig: str) -> dict[str, Any]:
        """Miner's own rerun leaves_root after a MISMATCH (auditor.replay.classify_mismatch)."""
        from hypertrain.auditor.replay import classify_mismatch

        try:
            ok = verify(
                decode_hotkey(hotkey), rerun_message(run_id, w, hotkey, root), bytes.fromhex(sig)
            )
        except (KeyError_, ValueError):
            ok = False
        if not ok:
            raise ChallengeError(401, "rerun signature does not verify")
        with self._tx() as db:
            _, manifest = self._run(db, run_id)
            row, _ = self._round(db, run_id, w)
            miner = self._miner(db, run_id, w, hotkey)
            if row["finalize"] is not None or miner["status"] != "MISMATCH":
                raise ChallengeError(409, "rerun is only open for a MISMATCH before finalize")
            if miner["rerun"] is not None:
                raise ChallengeError(409, "rerun already submitted")
            verdict = ReplayVerdict.model_validate(json.loads(miner["verdict"])["body"])
            commit = Commit.model_validate(json.loads(miner["commit_env"])["body"])
            transients = db.execute(
                "SELECT COUNT(*) AS n FROM miners WHERE run_id=? AND hotkey=? "
                "AND status='TRANSIENT'",
                (run_id, hotkey),
            ).fetchone()["n"]
            outcome = classify_mismatch(
                verdict, commit.leaves_root, root, transients, manifest.verify.forgive_per_epoch
            )
            db.execute(
                "UPDATE miners SET rerun=? WHERE run_id=? AND w=? AND hotkey=?",
                (_dumps({"leaves_root": root, "classification": outcome}), run_id, w, hotkey),
            )
            if outcome == "TRANSIENT":
                db.execute(
                    "UPDATE miners SET status='TRANSIENT' WHERE run_id=? AND w=? AND hotkey=?",
                    (run_id, w, hotkey),
                )
                db.execute(
                    "UPDATE roster SET blacklisted=0 WHERE run_id=? AND hotkey=?", (run_id, hotkey)
                )
                db.execute(
                    "DELETE FROM forfeits WHERE run_id=? AND w=? AND hotkey=?", (run_id, w, hotkey)
                )
        return {"w": w, "hotkey": hotkey, "classification": outcome}

    def heartbeat(self, job_id: str, lease: str) -> dict[str, Any]:
        with self._tx() as db:
            self._leased(db, job_id, lease)
            expires = self._now(db) + LEASE_ROUNDS
            db.execute("UPDATE jobs SET lease_expires=? WHERE id=?", (expires, job_id))
        return {"id": job_id, "lease_expires_round": expires}

    def fail(self, job_id: str, lease: str, reason: str, retry: bool) -> dict[str, Any]:
        with self._tx() as db:
            self._leased(db, job_id, lease)
            state = "queued" if retry else "failed"
            db.execute(
                "UPDATE jobs SET state=?, lease=NULL, fail_reason=? WHERE id=?",
                (state, reason, job_id),
            )
        return {"id": job_id, "state": state}

    def complete(self, job_id: str, lease: str, env: Any) -> dict[str, Any]:
        with self._lock:
            job = self._db.execute("SELECT run_id FROM jobs WHERE id=?", (job_id,)).fetchone()
        if job is None:
            raise ChallengeError(404, "unknown job")
        run_id = job["run_id"]

        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Any, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            row = self._leased(db, job_id, lease)
            if m.challenge_hash != row["challenge_hash"]:
                raise ChallengeError(422, "verdict challenge_hash is not this job's")
            db.execute(
                "UPDATE jobs SET state='done', lease=NULL, result=?, verdict=?, auditor=? "
                "WHERE id=?",
                (m.result, _dumps(dict(env)), env["signer"], job_id),
            )
            db.execute(
                "UPDATE miners SET status=?, verdict=? WHERE run_id=? AND w=? AND hotkey=?",
                (m.result, _dumps(dict(env)), run_id, row["w"], row["target"]),
            )
            out: dict[str, Any] = {"id": job_id, "state": "done", "result": m.result}
            if m.result in FAULTS:
                out["forfeit"] = self._forfeit(db, manifest, row, m.result, env)
            return out

        result: dict[str, Any] = self._signed(run_id, env, "ReplayVerdict", handle)
        return result

    def _forfeit(
        self,
        db: sqlite3.Connection,
        manifest: RunManifest,
        job: sqlite3.Row,
        result: str,
        verdict_env: Mapping[str, Any],
    ) -> dict[str, Any]:
        hotkey = job["target"]
        evidence = [job["challenge_hash"], body_digest(verdict_env["body"])]
        db.execute(
            "UPDATE roster SET blacklisted=1 WHERE run_id=? AND hotkey=?", (job["run_id"], hotkey)
        )
        # the ledger burns nothing before finalize: the provisional Forfeit claims 0 units
        from hypertrain.auditor.replay import forfeit_for

        cause = forfeit_for(job["w"], hotkey, result, evidence, 0, 0).cause
        return self._seal_forfeit(db, job["run_id"], job["w"], hotkey, cause, evidence, (0, 0))

    def _seal_forfeit(
        self,
        db: sqlite3.Connection,
        run_id: str,
        w: int,
        hotkey: str,
        cause: str,
        evidence: list[str],
        burned: tuple[int, int],
    ) -> dict[str, Any]:
        """K_coord Forfeit claiming exactly the (round slice, escrow) units the ledger burned."""
        body = {
            "w": w,
            "hotkey": hotkey,
            "cause": cause,
            "evidence": evidence,
            "round_reward_burned": burned[0],
            "escrow_burned_units": burned[1],
            "blacklist": True,
            "debt_units": 0,
        }
        env = seal(self._coord(), "Forfeit", run_id, body, NEVER)
        db.execute(
            "INSERT OR REPLACE INTO forfeits VALUES(?, ?, ?, ?)",
            (run_id, w, hotkey, _dumps(env)),
        )
        return env

    def _reseal_forfeit(
        self,
        db: sqlite3.Connection,
        run_id: str,
        w: int,
        hotkey: str,
        cause: str | None = None,
        extra: list[str] | None = None,
    ) -> None:
        row = db.execute(
            "SELECT envelope FROM forfeits WHERE run_id=? AND w=? AND hotkey=?", (run_id, w, hotkey)
        ).fetchone()
        old = json.loads(row["envelope"])["body"] if row else {"cause": cause, "evidence": []}
        evidence = old["evidence"] + [e for e in extra or [] if e not in old["evidence"]]
        burned = self.ledger.burned_for_fault(w, hotkey)
        self._seal_forfeit(db, run_id, w, hotkey, cause or old["cause"], evidence, burned)

    def inputs(self, run_id: str, w: int, d_open: int | None) -> dict[str, Any]:
        with self._tx() as db:
            self._advance(db)
            row, body = self._round(db, run_id, w)
            _, manifest = self._run(db, run_id)
            miners = [
                self._miner_view(m)
                for m in db.execute(
                    "SELECT * FROM miners WHERE run_id=? AND w=? ORDER BY hotkey", (run_id, w)
                )
            ]
            out: dict[str, Any] = {
                "run_id": run_id,
                "w": w,
                "state": self.round_state(db, row, body),
                "now_round": self._now(db),
                "round_open": json.loads(row["envelope"]),
                "base": row["base"],
                "miners": miners,
                "selected": json.loads(row["selected"]) if row["selected"] else None,
                "rollback": json.loads(row["rollback"]) if row["rollback"] else None,
            }
            nxt = db.execute(
                "SELECT 1 FROM rounds WHERE run_id=? AND w=?", (run_id, w + 1)
            ).fetchone()
            if nxt is None:
                start = max(self._now(db), body.d_upload) if d_open is None else d_open
                out["next_round_template"], _ = self._template(db, run_id, manifest, w + 1, start)
            return out

    def _miner_view(self, m: sqlite3.Row) -> dict[str, Any]:
        return {
            "hotkey": m["hotkey"],
            "status": m["status"],
            "selected": bool(m["selected"]),
            "received_round": m["received_round"],
            "commit": json.loads(m["commit_env"]) if m["commit_env"] else None,
            "receipt": json.loads(m["receipt"]) if m["receipt"] else None,
            "delta_manifest": json.loads(m["delta"]) if m["delta"] else None,
            "verdict": json.loads(m["verdict"]) if m["verdict"] else None,
        }

    def aggregate(self, run_id: str, w: int, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: RoundOpen, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            run, _ = self._run(db, run_id)
            if run["status"] != "running":
                raise ChallengeError(409, "run is not running")
            if m.w != w + 1:
                raise ChallengeError(422, "aggregate must open round w+1")
            row, body = self._round(db, run_id, w)
            d = self._now(db)
            if d < body.d_upload:
                raise ChallengeError(409, "round w uploads are still open")
            if m.d_open < d:
                raise ChallengeError(409, "d_open is in the past")
            tmpl, reasons = self._template(db, run_id, manifest, m.w, m.d_open)
            got = m.model_dump(mode="json")
            if {k: got[k] for k in tmpl} != tmpl:
                raise ChallengeError(422, "RoundOpen differs from next_round_template")
            base = self._next_base(manifest, row["base"], len(body.roster), len(m.roster))
            self._store_round(db, run_id, env, m, base, reasons)
            db.execute("UPDATE rounds SET applied=1 WHERE run_id=? AND w=?", (run_id, w))
            return {"w": w, "state": "APPLIED", "opened": m.w}

        result: dict[str, Any] = self._signed(run_id, env, "RoundOpen", handle)
        return result

    def rollback(self, run_id: str, w: int, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Rollback, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            row, _ = self._round(db, run_id, w)
            if m.w != w or not row["applied"] or row["finalize"] is not None:
                raise ChallengeError(409, "rollback needs an applied, unfinalized round w")
            committed = {
                r["hotkey"]
                for r in db.execute(
                    "SELECT hotkey FROM miners WHERE run_id=? AND w=? "
                    "AND received_round IS NOT NULL AND status!='EXCLUDED'",
                    (run_id, w),
                )
            }
            if not set(m.excluded) <= committed:
                raise ChallengeError(422, "excluded must be committed miners of round w")
            db.execute(
                "UPDATE rounds SET rollback=? WHERE run_id=? AND w=?",
                (_dumps(dict(env)), run_id, w),
            )
            return {"w": w, "excluded": list(m.excluded)}

        result: dict[str, Any] = self._signed(run_id, env, "Rollback", handle)
        return result

    def _ledger_verdict(self, m: sqlite3.Row, excluded: set[str]) -> str:
        status = m["status"]
        if status in FAULTS:
            return "FAULT"
        if status == "TRANSIENT":
            return "TRANSIENT"
        if m["delta"] is None:
            return "NO_UPLOAD"
        if status == "MATCH":
            return "MATCH"
        if m["selected"]:
            if m["hotkey"] in excluded:
                # ponytail: unresolved at d_final -> excluded, unpaid, no burn; "pay on
                # vindication" after finality needs a ledger credit event (todo 3 API).
                return "NO_UPLOAD"
            raise ChallengeError(409, f"audit of {m['hotkey']} is unresolved and not rolled back")
        return "UNSAMPLED"

    def finalize(self, run_id: str, w: int, env: Any) -> dict[str, Any]:
        def handle(
            db: sqlite3.Connection, manifest: RunManifest, m: Finalize, env: Mapping[str, Any]
        ) -> dict[str, Any]:
            self._advance(db)
            row, body = self._round(db, run_id, w)
            if m.w != w or row["finalize"] is not None:
                raise ChallengeError(409, "round already finalized or w mismatch")
            if self._now(db) < body.d_final or row["selected"] is None:
                raise ChallengeError(409, "d_final not reached")
            prev = db.execute(
                "SELECT finalize FROM rounds WHERE run_id=? AND w=?", (run_id, w - 1)
            ).fetchone()
            if w > 0 and (prev is None or prev["finalize"] is None):
                raise ChallengeError(409, "rounds finalize in order")
            excluded: set[str] = set()
            if row["rollback"]:
                excluded = set(json.loads(row["rollback"])["body"]["excluded"])
            open_disputes = {
                r["hotkey"]
                for r in db.execute(
                    "SELECT hotkey FROM disputes WHERE run_id=? AND w=? AND state='open'",
                    (run_id, w),
                )
            }
            if not open_disputes <= excluded:
                raise ChallengeError(409, "open disputes must be rolled back before finalize")
            miners = db.execute(
                "SELECT * FROM miners WHERE run_id=? AND w=? AND received_round IS NOT NULL "
                "AND status!='EXCLUDED' ORDER BY hotkey",
                (run_id, w),
            ).fetchall()
            verdicts = {
                mm["hotkey"]: (
                    "NO_UPLOAD"
                    if mm["hotkey"] in open_disputes
                    else self._ledger_verdict(mm, excluded)
                )
                for mm in miners
            }
            included = sorted(h for h, v in verdicts.items() if v in ("MATCH", "UNSAMPLED"))
            if list(m.included) != included:
                raise ChallengeError(422, f"included must be {included}")
            at = self._event_at(db, manifest, self._now(db))
            tokens = {mm["hotkey"]: json.loads(mm["commit_env"])["body"]["tokens"] for mm in miners}
            try:
                for hk in sorted(verdicts):
                    self.ledger.verdict(w, hk, verdicts[hk], tokens[hk], 0, at)
                self.ledger.finalize(w, at)
            except LedgerError as error:
                raise ChallengeError(409, f"ledger rejected finalize: {error}") from None
            for hk in sorted(h for h, v in verdicts.items() if v == "FAULT"):
                self._reseal_forfeit(db, run_id, w, hk)
            db.execute(
                "UPDATE rounds SET finalize=?, final_at=? WHERE run_id=? AND w=?",
                (_dumps(dict(env)), at, run_id, w),
            )
            return {"w": w, "state": "FINAL", "verdicts": verdicts}

        result: dict[str, Any] = self._signed(run_id, env, "Finalize", handle)
        return result

    def runs(self) -> list[dict[str, Any]]:
        with self._lock:
            rows = self._db.execute("SELECT run_id, status FROM runs").fetchall()
        own = [{"run_id": r["run_id"], "status": r["status"]} for r in rows]
        return own + [r for child in self._children.values() for r in child.runs()]

    def run_status(self, run_id: str) -> dict[str, Any]:
        with self._tx() as db:
            self._advance(db)
            row, _ = self._run(db, run_id)
            rounds = []
            for r in db.execute("SELECT * FROM rounds WHERE run_id=? ORDER BY w", (run_id,)):
                body = RoundOpen.model_validate_json(r["body"])
                rounds.append({"w": body.w, "state": self.round_state(db, r, body)})
            roster = [
                {
                    "hotkey": r["hotkey"],
                    "bond": bool(r["bond"]),
                    "probation": bool(r["probation"]),
                    "cluster": r["cluster"],
                    "region": r["region"],
                    "flags": json.loads(r["flags"]),
                    "admitted_w": r["admitted_w"],
                    "removed": bool(r["removed"]),
                    "blacklisted": bool(r["blacklisted"]),
                }
                for r in db.execute(
                    "SELECT * FROM roster WHERE run_id=? ORDER BY hotkey", (run_id,)
                )
            ]
            return {
                "run_id": run_id,
                "status": row["status"],
                "config": json.loads(row["config"] or "null"),
                "manifest_envelope": json.loads(row["envelope"]),
                "now_round": self._now(db),
                "rounds": rounds,
                "roster": roster,
            }

    def round_view(self, run_id: str, w: int) -> dict[str, Any]:
        with self._tx() as db:
            self._advance(db)
            row, body = self._round(db, run_id, w)
            _, manifest = self._run(db, run_id)
            state = self.round_state(db, row, body)
            assignment = None
            if self._now(db) >= body.d_assign:
                slices = self._assignment(db, run_id, manifest, row)
                assignment = [
                    {
                        "hotkey": e.hotkey,
                        "slot": e.slot,
                        "samples": list(slices[e.slot]),
                        "assignment_hash": assignment_hash(run_id, w, e.slot, slices[e.slot]),
                    }
                    for e in body.roster
                ]
            miners = [
                self._miner_view(m)
                for m in db.execute(
                    "SELECT * FROM miners WHERE run_id=? AND w=? ORDER BY hotkey", (run_id, w)
                )
            ]
            jobs = [
                {
                    "id": j["id"],
                    "target": j["target"],
                    "state": j["state"],
                    "result": j["result"],
                    "challenge": json.loads(j["challenge"]),
                }
                for j in db.execute(
                    "SELECT * FROM jobs WHERE run_id=? AND w=? ORDER BY target", (run_id, w)
                )
            ]
            forfeits = [
                json.loads(f["envelope"])
                for f in db.execute(
                    "SELECT envelope FROM forfeits WHERE run_id=? AND w=? ORDER BY hotkey",
                    (run_id, w),
                )
            ]
            return {
                "run_id": run_id,
                "w": w,
                "state": state,
                "now_round": self._now(db),
                "round_open": json.loads(row["envelope"]),
                "base": row["base"],
                "assignment": assignment,
                "audit_beacon_round": body.d_audit,
                "audit_beacon_signature": self._known_sig(db, body.d_audit),
                "selected": json.loads(row["selected"]) if row["selected"] else None,
                "miners": miners,
                "jobs": jobs,
                "forfeits": forfeits,
                "rollback": json.loads(row["rollback"]) if row["rollback"] else None,
                "finalize": json.loads(row["finalize"]) if row["finalize"] else None,
            }

    def auditor_stats(self, run_id: str) -> dict[str, Any]:
        with self._lock:
            db = self._db
            _, manifest = self._run(db, run_id)
            honeypots: dict[str, str] = {}
            for h in db.execute("SELECT reveal FROM honeypots WHERE run_id=?", (run_id,)):
                if h["reveal"]:
                    for member in json.loads(h["reveal"])["members"]:
                        honeypots[member["hotkey"]] = member["mode"]
            epochs: dict[int, dict[str, int]] = {}
            for j in db.execute(
                "SELECT j.*, r.body FROM jobs j JOIN rounds r ON r.run_id=j.run_id AND r.w=j.w "
                "WHERE j.run_id=? AND j.result IS NOT NULL",
                (run_id,),
            ):
                d_audit = RoundOpen.model_validate_json(j["body"]).d_audit
                epoch = self.params.epoch_of(self._chain_time(manifest, d_audit))
                e = epochs.setdefault(
                    epoch,
                    {
                        "audits": 0,
                        "catches": 0,
                        "hp_bad": 0,
                        "hp_bad_caught": 0,
                        "hp_honest": 0,
                        "hp_honest_flagged": 0,
                    },
                )
                caught = j["result"] in FAULTS
                e["audits"] += 1
                e["catches"] += caught
                mode = honeypots.get(j["target"])
                if mode is not None and mode != "honest":
                    e["hp_bad"] += 1
                    e["hp_bad_caught"] += caught
                elif mode == "honest":
                    e["hp_honest"] += 1
                    e["hp_honest_flagged"] += caught

        def rate(num: int, den: int) -> float | None:
            return num / den if den else None

        return {
            "run_id": run_id,
            "epochs": [
                {
                    "epoch": k,
                    **v,
                    "honeypot_catch_rate": rate(v["hp_bad_caught"], v["hp_bad"]),
                    "honeypot_false_positive_rate": rate(v["hp_honest_flagged"], v["hp_honest"]),
                }
                for k, v in sorted(epochs.items())
            ],
        }

    def weights(self, epoch: int, epoch_at: int | None, computed_at: int) -> bytes:
        if epoch_at is None:
            epoch_at = self.params.genesis_unix + (epoch + 1) * self.params.epoch_seconds
        with self._tx() as db:
            stored = db.execute(
                "SELECT answer FROM weights_runs WHERE epoch=?", (epoch,)
            ).fetchone()
            if stored is not None:
                return bytes(stored[0])
            legacy = epoch in self.ledger._engine.answers  # answered before multi-run hosting
            answer = self.ledger.get_weights(epoch, epoch_at, computed_at)
            if legacy:
                return answer
            last = self._meta(db, "last_answer_at", -1)
            self._set_meta(db, "last_answer_at", max(last, epoch_at))
            answers = [answer] + [
                c.weights(epoch, epoch_at, computed_at) for c in self._children.values()
            ]
            merged = answer if len(answers) == 1 else _merge_answers(answers)
            db.execute("INSERT INTO weights_runs VALUES(?, ?)", (epoch, merged))
        return merged

    def _run_v2(self, run_id: str) -> RunManifestV2:
        row = self._db.execute("SELECT manifest FROM runs WHERE run_id=?", (run_id,)).fetchone()
        if row is None:
            raise ChallengeError(404, "unknown run")
        data = envelope_v2.load_json(row[0])
        if not isinstance(data, dict) or data.get("manifest_version") != 2:
            raise ChallengeError(409, "route and stored contract version differ")
        return RunManifestV2.model_validate(data)

    def _record_v2(self, run_id: str, kind: str, identity: str) -> dict[str, Any]:
        row = self._db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
            (run_id, kind, identity),
        ).fetchone()
        if row is None:
            raise ChallengeError(409, f"missing accepted {kind}: {identity}")
        value = json.loads(row[0])
        if not isinstance(value, dict):
            raise ChallengeError(409, "invalid persisted record")
        return value

    def _put_record_v2(
        self, run_id: str, kind: str, identity: str, data: Mapping[str, JsonValue]
    ) -> None:
        self._db.execute(
            "INSERT OR REPLACE INTO records_v2 VALUES(?,?,?,?)",
            (run_id, kind, identity, canonicalize(dict(data), allow_float=False).decode()),
        )

    def _beacon_v2(self, rnd: int) -> BeaconRound:
        from hypertrain.beacon import BeaconUnavailable

        row = self._db.execute("SELECT * FROM beacon WHERE round=?", (rnd,)).fetchone()
        if row is None:
            raise BeaconUnavailable(f"round {rnd} not pushed")
        return BeaconRound(row[0], row[1], row[2], bool(row[3]))

    def _snapshot_current_v2(self, manifest: RunManifestV2) -> None:
        """Read freshness gate for every eligibility/lease deadline boundary."""
        now = self._now(self._db)
        emitted = (
            int(self.clock()) - manifest.training.beacon.genesis_time
        ) // manifest.training.beacon.period + 1
        if now < 1 or now < emitted - 2:
            raise ChallengeError(503, "beacon infrastructure paused")

    def _fresh_v2(self, manifest: RunManifestV2, operation: str, rnd: int | None = None) -> int:
        from hypertrain.beacon.core import BeaconUnavailable
        from hypertrain.beacon.freshness import FreshnessGate

        now = self._now(self._db)
        pin = now if rnd is None else rnd
        if pin < 1:
            raise ChallengeError(503, "no verified beacon")
        gate = FreshnessGate(self._db, manifest.run_id())
        gate.pin(operation, pin)
        if manifest.network.economics_policy_hash == "0" * 64:
            raise ChallengeError(409, "economics policy missing")
        policy = EconomicsPolicyV2.model_validate_json(
            self.objects.get(manifest.network.economics_policy_hash)
        )
        if policy.ledger_mode == "test":
            # Test-only functional CPU evidence; never a production BLS qualification.
            b = self._beacon_v2(pin)
            emitted = (int(self.clock()) - manifest.training.beacon.genesis_time) // 3 + 1
            if now < emitted - 2:
                self._db.execute(
                    "UPDATE beacon_pins_v2 SET paused=1,reason='STALE' "
                    "WHERE run_id=? AND operation_id=?",
                    (manifest.run_id(), operation),
                )
                raise ChallengeError(503, "beacon infrastructure paused")
            self._db.execute(
                "UPDATE beacon_pins_v2 SET signature=?,signature_hash=?,paused=0,reason=NULL "
                "WHERE run_id=? AND operation_id=?",
                (b.signature, sha256_hex(bytes.fromhex(b.signature)), manifest.run_id(), operation),
            )
        else:
            store = self

            class AcceptedBeacon:
                def get(self, round: int) -> BeaconRound:
                    return store._beacon_v2(round)

                def round_at_or_after(self, t: int) -> int:
                    from hypertrain.beacon.core import round_at_or_after

                    return round_at_or_after(t, manifest.training.beacon.genesis_time, 3)

            try:
                gate.refresh(
                    operation,
                    AcceptedBeacon(),
                    now_seconds=int(self.clock()),
                    genesis_seconds=manifest.training.beacon.genesis_time,
                    period_seconds=manifest.training.beacon.period,
                    max_lag_rounds=2,
                )
            except BeaconUnavailable:
                raise ChallengeError(
                    503, "beacon infrastructure paused; pinned seed retained"
                ) from None
        return now

    def create_run_v2(self, raw: bytes) -> dict[str, JsonValue]:
        with self._lock:  # one authority lock across the hosted check and the insert
            return self._create_run_v2(raw)

    def _create_run_v2(self, raw: bytes) -> dict[str, JsonValue]:
        if self.owner_hotkey is None:
            raise ChallengeError(503, "owner hotkey is not configured")
        env = envelope_v2.parse_envelope(raw)
        if env.type != "RunManifestV2":
            raise ChallengeError(400, "expected RunManifestV2")
        hosted = False
        with self._tx():
            model = envelope_v2.Intake(
                env.run_id, {"RunManifestV2": self.owner_hotkey.__eq__}
            ).accept(raw, self._now(self._db))
            assert isinstance(model, RunManifestV2)
            if model.training.coord_pubkey != self._coord().ss58:
                raise ChallengeError(403, "wrong coordinator")
            if self._db.execute("SELECT 1 FROM runs").fetchone():
                hosted = True
        if hosted:
            pinned = (
                *(getattr(model.network, f) for f in PolicyHashes.model_fields),
                model.network.relay_registry_hash,
            )
            child_result: dict[str, JsonValue] = self._create_child(
                model.run_id(), pinned, lambda c: c.create_run_v2(raw)
            )
            return child_result
        with self._tx():
            for name, cls in (
                ("admission_policy_hash", AdmissionPolicyV2),
                ("economics_policy_hash", EconomicsPolicyV2),
                ("aggregation_policy_hash", AggregationPolicyV2),
                ("audit_policy_hash", AuditPolicyV2),
                ("dispute_policy_hash", DisputePolicyV2),
            ):
                policy = cls.model_validate_json(self.objects.get(getattr(model.network, name)))
                if name == "audit_policy_hash":
                    assert isinstance(policy, AuditPolicyV2)
                    policy.validate_training(model.training)
            from hypertrain.protocol.relay_messages import RelayRegistryV1

            RelayRegistryV1.model_validate_json(self.objects.get(model.network.relay_registry_hash))
            if len(set(model.training.auditors)) < 2:
                raise ChallengeError(422, "two distinct audit reference identities required")
            if model.training.inner.state_policy == "derived":
                raise ChallengeError(
                    422, "v2 derived-state influence requires authenticated v0 integration"
                )
            self._db.execute(
                "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
                (model.run_id(), model.model_dump_json(), raw.decode()),
            )
        self._services(run_id=model.run_id())
        return {"run_id": model.run_id(), "status": "created", "contract": 2}

    def _owner_v2(self, run_id: str, hotkey: str) -> str:
        row = self._db.execute(
            "SELECT coldkey FROM admissions_v2 WHERE run_id=? AND hotkey=?", (run_id, hotkey)
        ).fetchone()
        if row is None:
            raise ChallengeError(403, "hotkey is not current accepted identity")
        return str(row[0])

    def _assignment_v2(self, run_id: str, w: int, hotkey: str) -> tuple[int, ...]:
        self.require_roster_v2(run_id, w)
        value = self._record_v2(run_id, "assignment", f"{w}:{hotkey}")
        samples = value["samples"]
        assert isinstance(samples, list) and all(type(i) is int for i in samples)
        return tuple(int(str(i)) for i in samples)

    def _settlement_v2(self, run_id: str, reference: str) -> Settlement:
        disputes = self._services_v2.get(run_id)
        if disputes is not None:
            row = self._db.execute(
                "SELECT turn FROM disputes_v2 WHERE id=? OR json_extract(turn,'$.lock_id')=?",
                (reference, reference),
            ).fetchone()
            if row:
                return disputes[2].settlement(reference)
        row = self._db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='settlement' AND id=?",
            (run_id, reference),
        ).fetchone()
        if row is None:
            raise EscrowError("UNACCEPTED_SETTLEMENT")
        return Settlement.model_validate_json(row[0])

    def _finality_replay_v2(self, run_id: str, evidence: FinalityEvidence) -> str:
        if evidence.shadow:
            return self._shadow_replay_v2(run_id, evidence)
        record = self._record_v2(run_id, "replayed-finality", str(evidence.finalize.body["w"]))
        if (
            record["finalize_hash"] != body_digest(evidence.finalize.body)
            or record["tape_hash"] != evidence.tape_hash
            or record["finalized_beacon"] != evidence.finalized_beacon
            or record["shadow"] != evidence.shadow
        ):
            raise EscrowError("FINALITY_ACCEPTANCE_LINKAGE")
        from hypertrain.aggregator.checkpoint import network_inputs
        from hypertrain.aggregator.tape_v2 import TapeV2, replay_tape

        manifest = self._run_v2(run_id)
        w = int(str(evidence.finalize.body["w"]))
        self.require_roster_v2(run_id, w)
        applied = self._record_v2(run_id, "applied", str(w))
        inputs = network_inputs(self._record_v2(run_id, "tape-inputs", str(w))["inputs"])
        tape = TapeV2.from_bytes(self.objects.get(evidence.tape_hash))
        replayed = replay_tape(
            self.objects,
            tape,
            manifest,
            AggregationPolicyV2.model_validate_json(
                self.objects.get(manifest.network.aggregation_policy_hash)
            ),
            self.objects.get(manifest.network.economics_policy_hash),
            signer=self._coord().ss58,
            w=w,
            prev_state=str(applied["prev_state"]),
            predecessor_tape_hash=str(applied["predecessor_tape_hash"]),
            inputs=inputs,
            reference_reward_units=self._services_v2[run_id][0].policy.R_collectible_units,
        )
        from hypertrain.aggregator.core import th

        if th(replayed.theta) != record["theta_hash"]:
            raise EscrowError("FINALITY_TAPE_REPLAY_MISMATCH")
        return str(record["theta_hash"])

    def _bootstrap_backend_v2(
        self,
        manifest: RunManifestV2,
        configs: tuple[Path, Path],
        results: tuple[Path, Path],
        raw: bytes,
    ) -> None:
        """Internal owner-reviewed CUDA evidence; install before run/service creation."""
        from hypertrain.gpu_ops import network_qualification as engine
        from hypertrain.gpu_ops.journal import fsha
        from hypertrain.protocol.messages import Receipt

        ref = manifest.training.reference_spec
        config_bytes = tuple(p.read_bytes() for p in configs)
        result_bytes = tuple(p.read_bytes() for p in results)
        evidence: dict[str, JsonValue] = {
            "run_id": manifest.run_id(),
            "backend": "cuda",
            "reference_hash": sha256_hex(canonicalize(ref.model_dump(mode="json"))),
            "layout_hash": ref.layout.model_dump_json(),
            "configs": [sha256_hex(data) for data in config_bytes],
            "results": [sha256_hex(data) for data in result_bytes],
            "summaries": [
                fsha(path.parent / f"round-{round_index}/published/rank-{rank}/summary.json")
                for path in results
                for round_index in range(2)
                for rank in range(ref.layout.n_gpus)
            ],
        }
        digest = sha256_hex(canonicalize(evidence))
        receipt = envelope_v2.Intake(
            manifest.run_id(), {"Receipt": lambda h: h == self.owner_hotkey}
        ).accept(raw, self._now(self._db))
        env = envelope_v2.parse_envelope(raw)
        if (
            env.type != "Receipt"
            or not isinstance(receipt, Receipt)
            or (receipt.commit_hash != digest or receipt.received_round != self._now(self._db))
        ):
            raise ChallengeError(403, "reviewed backend receipt differs from exact evidence")
        for config_path, result_path in zip(configs, results, strict=True):
            cfg = engine.Qualification.model_validate_json(config_path.read_bytes())
            job = engine.check_inputs(cfg)
            result = engine.Result.model_validate_json(result_path.read_bytes())
            for round_index in range(2):
                for rank in range(ref.layout.n_gpus):
                    summary = json.loads(
                        (
                            result_path.parent
                            / f"round-{round_index}/published"
                            / f"rank-{rank}/summary.json"
                        ).read_bytes()
                    )
                    if summary.get("backend") != "cuda":
                        raise ChallengeError(403, "CPU artifacts cannot qualify CUDA execution")
            if (
                job.manifest != manifest
                or result.config_sha256 != fsha(config_path)
                or result.profile_sha256 != cfg.profile_sha256
                or result.sources != cfg.sources
                or (result.instance_id, result.machine_id, result.role)
                != (cfg.instance_id, cfg.machine_id, cfg.role)
                or result.environment.image_digest != ref.image_digest
                or any(d not in ref.driver_allowlist for d in result.environment.drivers)
                or any(s != ref.sm_count for s in result.environment.sm_counts)
                or not Path(result.environment.torch_path).is_relative_to("/venv/main")
            ):
                raise ChallengeError(403, "reviewed CUDA runtime/reference binding differs")
        engine.compare(list(results))  # Rehash all rescued rank/round/checkpoint bytes.
        summaries = [
            fsha(path.parent / f"round-{round_index}/published/rank-{rank}/summary.json")
            for path in results
            for round_index in range(2)
            for rank in range(ref.layout.n_gpus)
        ]
        if evidence["summaries"] != summaries:
            raise ChallengeError(409, "qualification summary custody changed during bootstrap")
        with self._tx():
            row = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='qualification' AND id=?",
                (manifest.run_id(), ref.image_digest),
            ).fetchone()
            if row is not None:
                if json.loads(row[0])["authority_hash"] != digest:
                    raise ChallengeError(409, "immutable backend qualification conflicts")
                return
            if (
                manifest.run_id() in self._services_v2
                or self._db.execute(
                    "SELECT 1 FROM runs WHERE run_id=?", (manifest.run_id(),)
                ).fetchone()
                or self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='execution-backend'",
                    (manifest.run_id(),),
                ).fetchone()
            ):
                raise ChallengeError(409, "backend qualification must precede run services")
            for path, data in zip(
                (*configs, *results), (*config_bytes, *result_bytes), strict=True
            ):
                if path.read_bytes() != data:
                    raise ChallengeError(409, "qualification custody changed during bootstrap")
                self.objects.put(data)
            authority = self.objects.put(canonicalize(evidence))
            record = {
                **evidence,
                "authority_hash": authority,
                "receipt": env.model_dump(mode="json"),
            }
            self._put_record_v2(manifest.run_id(), "qualification", ref.image_digest, record)

    def _backend_v2(self, manifest: RunManifestV2) -> Literal["cpu", "cuda"]:
        """Resolve frozen run execution authority independently of monetary policy."""
        with self._lock:
            ref = manifest.training.reference_spec
            row = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='qualification' AND id=?",
                (manifest.run_id(), ref.image_digest),
            ).fetchone()
            authority = None
            if row is not None:
                record = json.loads(row[0])
                authority = str(record["authority_hash"])
                evidence = envelope_v2.load_json(self.objects.get(authority))
                env = envelope_v2.parse_envelope(record["receipt"])
                if (
                    sha256_hex(canonicalize(evidence)) != authority
                    or any(record.get(k) != v for k, v in evidence.items())
                    or evidence.get("run_id") != manifest.run_id()
                    or evidence.get("backend") != "cuda"
                    or evidence.get("reference_hash")
                    != sha256_hex(canonicalize(ref.model_dump(mode="json")))
                    or evidence.get("layout_hash") != ref.layout.model_dump_json()
                    or env.type != "Receipt"
                    or env.run_id != manifest.run_id()
                    or env.signer != self.owner_hotkey
                    or env.body["commit_hash"] != authority
                    or not envelope_v2.verify_envelope(record["receipt"])
                ):
                    raise ChallengeError(403, "backend qualification authority differs")
                backend: Literal["cpu", "cuda"] = "cuda"
            else:
                econ = EconomicsPolicyV2.model_validate_json(
                    self.objects.get(manifest.network.economics_policy_hash)
                )
                if econ.ledger_mode != "test":
                    raise ChallengeError(409, "production CUDA qualification required")
                backend = "cpu"
            selection: dict[str, JsonValue] = {"backend": backend, "authority_hash": authority}
            frozen = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? "
                "AND kind='execution-backend' AND id='run'",
                (manifest.run_id(),),
            ).fetchone()
            if frozen is not None and json.loads(frozen[0]) != selection:
                raise ChallengeError(409, "frozen execution backend cannot change")
            if frozen is None:
                self._put_record_v2(manifest.run_id(), "execution-backend", "run", selection)
            return backend

    def _prefix_secret_v2(self) -> bytes:
        secret = hmac.digest(self._coord()._secret, b"hypertrain/ip-prefix/2", "sha256")
        namespace = sha256_hex(secret)
        with self._tx() as db:
            row = db.execute(
                "SELECT value FROM meta WHERE key='admission-prefix-namespace-v2'"
            ).fetchone()
            if row is not None:
                if row["value"] != namespace:
                    raise ChallengeError(503, "admission prefix namespace differs")
            else:
                quota = db.execute(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='admission_quota'"
                ).fetchone()
                if (
                    quota is not None
                    and db.execute(
                        "SELECT 1 FROM admission_quota WHERE identity LIKE 'ip:%' LIMIT 1"
                    ).fetchone()
                ):
                    raise ChallengeError(503, "legacy admission prefix namespace unavailable")
                db.execute(
                    "INSERT INTO meta VALUES('admission-prefix-namespace-v2', ?)", (namespace,)
                )
        return secret

    def _services(self, run_id: str) -> tuple[EscrowV2, Admission, DisputesV2]:
        with self._lock:
            existing = self._services_v2.get(run_id)
            if existing is not None:
                self._backend_v2(self._run_v2(run_id))
                return existing
            manifest = self._run_v2(run_id)
            backend = self._backend_v2(manifest)
            n = manifest.network
            econ = EconomicsPolicyV2.model_validate_json(self.objects.get(n.economics_policy_hash))
            escrow = EscrowV2(
                self._db,
                manifest,
                econ,
                auditors=frozenset(manifest.training.auditors),
                replay_finality=lambda e: self._finality_replay_v2(run_id, e),
                settlement=lambda ref: self._settlement_v2(run_id, ref),
                owner_of=lambda hot: self._owner_v2(run_id, hot),
                assignment_of=lambda w, hot: self._assignment_v2(run_id, w, hot),
                shadow_assignment_of=lambda binding: self._shadow_assignment_v2(run_id, binding),
                shadow_settlement=lambda origin: self._shadow_settlement_v2(run_id, origin),
            )
            escrow.mutex = self._lock  # One SQLite authority lock, including event acknowledgments.

            def qualify(screen: WorkScreenV2) -> bool:
                if screen.layout != manifest.training.reference_spec.layout or (
                    screen.image_digest != manifest.training.reference_spec.image_digest
                ):
                    return False
                if backend == "cpu":
                    return True
                try:
                    record = self._record_v2(run_id, "qualification", screen.image_digest)
                except ChallengeError:
                    return False
                return record.get("layout_hash") == screen.layout.model_dump_json() and (
                    record.get("reference_hash")
                    == sha256_hex(
                        canonicalize(manifest.training.reference_spec.model_dump(mode="json"))
                    )
                )

            policy = AdmissionPolicyV2.model_validate_json(
                self.objects.get(n.admission_policy_hash)
            )
            admission = Admission(
                escrow,
                policy,
                self._coord(),
                beacon=self._beacon_v2,
                stage_reference=lambda c, samples, epoch: self.stage_trial_v2(
                    run_id, c, samples, epoch
                ),
                objects=self.objects,
                ip_secret=self._prefix_secret_v2(),
                qualified=qualify,
                conservative_bound=lambda: (
                    econ.ledger_mode == "test"
                    or (
                        self._db.execute(
                            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='economic-bound'",
                            (run_id,),
                        ).fetchone()
                        is not None
                    )
                ),
                backend=backend,
            )
            dispute_policy = DisputePolicyV2.model_validate_json(
                self.objects.get(n.dispute_policy_hash)
            )
            disputes = DisputesV2(
                escrow,
                dispute_policy,
                self._coord(),
                contest_of=lambda h: Contest.model_validate(self._record_v2(run_id, "contest", h)),
                owner_of=lambda hot: self._owner_v2(run_id, hot),
                span=lambda contest, level, ctx: self._dispute_span_v2(run_id, contest, level, ctx),
            )
            self._services_v2[run_id] = (escrow, admission, disputes)
            self._start_notifications_v2()
            return escrow, admission, disputes

    def _start_notifications_v2(self) -> None:
        """Explicit datagram IPC; durable SQLite events are truth, no event polling."""
        import os

        if self._ipc_v2 is not None:
            return
        directory = Path(tempfile.gettempdir()) / (
            "ht-notify-" + sha256_hex(str(self.state_dir.resolve()).encode())[:20]
        )
        directory.mkdir(mode=0o700, exist_ok=True)
        if directory.stat().st_uid != os.getuid() or directory.stat().st_mode & 0o077:
            raise ChallengeError(503, "unsafe notification directory")
        path = directory / f"{os.getpid()}-{secrets.token_hex(4)}.sock"
        ipc = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        ipc.bind(str(path))
        self._ipc_v2, self._ipc_path_v2 = ipc, path

        def receive() -> None:
            while True:
                message = ipc.recv(64)
                if message == b"stop":
                    return
                with self._lock:
                    now = self._now(self._db)
                    for guard in tuple(self._lease_guards_v2.values()):
                        guard.advance(now)
                    disputes = [s[2] for s in self._services_v2.values()]
                for dispute in disputes:
                    dispute.notify()
                with self.beacon_arrived:
                    self.beacon_arrived.notify_all()

        self._ipc_thread_v2 = threading.Thread(target=receive, daemon=True, name="ht-v2-notify")
        self._ipc_thread_v2.start()

    def _notify_processes_v2(self) -> None:
        if self._ipc_v2 is None or self._ipc_path_v2 is None:
            return
        for path in self._ipc_path_v2.parent.glob("*.sock"):
            if path == self._ipc_path_v2:
                continue
            try:
                self._ipc_v2.sendto(b"committed", str(path))
            except (FileNotFoundError, ConnectionRefusedError):
                path.unlink(missing_ok=True)

    def stage_trial_v2(
        self, run_id: str, challenge: JoinChallenge, samples: tuple[int, ...], epoch: int
    ) -> tuple[IslandJobV1, Path]:
        import torch

        import hypertrain.trainer  # noqa: F401
        from hypertrain.auditor.replay import AnchorCache, pack_state
        from hypertrain.trainer.config import TrainConfig
        from hypertrain.trainer.model import init_params

        manifest = self._run_v2(run_id)
        cfg = TrainConfig.from_manifest_v2(manifest)
        theta = init_params(cfg.model)
        start_state = pack_state(theta)
        ef_in = pack_state({k: torch.zeros_like(v) for k, v in theta.items()})
        if cfg.inner.state_policy == "carry":
            admission = self._services(run_id)[1].store.by_id(challenge.admission_id)
            genesis = AnchorCache().genesis(manifest, admission.hotkey, theta)
            start_state = pack_state(genesis.theta, genesis.state)
            ef_in = pack_state(genesis.ef)
        directory = self.state_dir / "trials-v2" / challenge.nonce
        directory.mkdir(parents=True, exist_ok=True)
        dataset = self._record_v2(run_id, "dataset", "inputs")
        raw = self.objects.get(str(dataset["samples_hash"]))
        proofs = json.loads(self.objects.get(str(dataset["proofs_hash"])))
        assert isinstance(proofs, list)
        width = (cfg.model.seq_len + 1) * (
            2 if manifest.training.dataset.sample_format.startswith("u16") else 4
        )
        if (
            len(raw) != manifest.training.dataset.n_samples * width
            or len(proofs) != manifest.training.dataset.n_samples
        ):
            raise ChallengeError(422, "dataset object dimensions differ from signed manifest")
        inputs = {
            "start_state": start_state,
            "ef_in": ef_in,
            "v0": pack_state({}),
            "samples": b"".join(raw[i * width : (i + 1) * width] for i in samples),
            "sample_proofs": canonicalize([proofs[i] for i in samples]),
        }
        for name, value in inputs.items():
            path = directory / name
            if path.exists() and path.read_bytes() != value:
                raise ChallengeError(409, "immutable trial inputs changed")
            path.write_bytes(value)
        deadline = manifest.training.beacon.genesis_time + (challenge.deadline_beacon - 1) * 3
        return IslandJobV1(
            job_version=1,
            run_id=run_id,
            w=epoch,
            manifest=manifest,
            sample_ids=list(samples),
            global_step0=0,
            start_state_sha256=sha256_hex(inputs["start_state"]),
            ef_in_sha256=sha256_hex(inputs["ef_in"]),
            v0_sha256=sha256_hex(inputs["v0"]),
            object_paths={name: name for name in inputs},
            deadline=deadline,
        ), directory

    def _dispute_span_v2(
        self, run_id: str, contest: Contest, level: Any, ctx: tuple[int, ...]
    ) -> int:
        from hypertrain.auditor.island_bisect import IslandParty
        from hypertrain.protocol.messages_v2 import IslandJobV1

        geometry = self._record_v2(run_id, "trace-geometry", contest.verdict_hash)
        job = IslandJobV1.model_validate(geometry["job"])
        if job.run_id != run_id or job.w != contest.w:
            raise ChallengeError(409, "accepted trace geometry cross-run/round")
        party = IslandParty.published(contest.miner, job, Path(str(geometry["directory"])))
        return party.span(level, ctx)

    def admission_v2(
        self, run_id: str, action: str, raw: bytes = b"", identity: str = "", ip_prefix: str = ""
    ) -> dict[str, Any]:
        escrow, admission, disputes = self._services(run_id)
        failure: ChallengeError | None = None
        result: dict[str, Any] = {}
        with self._tx():
            manifest = self._run_v2(run_id)
            try:
                now = self._fresh_v2(manifest, f"admission:{action}:{self._now(self._db)}")
            except ChallengeError as error:
                failure = error  # Persist infrastructure pause, do not roll it back.
            if failure is None:
                match action:
                    case "join":
                        result = asdict(admission.join(raw, now=now, ip_prefix=ip_prefix))
                    case "challenge":
                        result = admission.challenge(identity, now=now)
                    case "status":
                        result = asdict(admission.status(identity, now=now))
                        screen = self._db.execute(
                            "SELECT screen_json FROM admissions_v2 WHERE hotkey=?", (identity,)
                        ).fetchone()
                        result["work_screen_hash"] = (
                            WorkScreenV2.model_validate_json(screen[0]).digest()
                            if screen and screen[0]
                            else None
                        )
                    case "rotate":
                        result = asdict(admission.rotate(raw, now=now))
                    case "recover":
                        env = envelope_v2.parse_envelope(raw)
                        from hypertrain.protocol.messages import Receipt

                        auth = self._authenticated_v2(run_id, raw, "Receipt", env.signer)
                        assert isinstance(auth, Receipt)
                        row = self._db.execute(
                            "SELECT p.dispute_id FROM admission_pending p JOIN admissions_v2 a "
                            "ON a.admission_id=p.admission_id WHERE a.hotkey=? AND p.resolved=0",
                            (env.signer,),
                        ).fetchone()
                        if row is None or row[0] != auth.commit_hash:
                            raise ChallengeError(
                                403, "recovery reference differs from exact current pending dispute"
                            )
                        admission.resolve_pending(env.signer, auth.commit_hash, now=now)
                        result = asdict(admission.status(env.signer, now=now))
                    case "lock":
                        env = envelope_v2.parse_envelope(raw)
                        from hypertrain.protocol.messages_v2 import EscrowLock

                        op = EscrowLock.model_validate(env.body)
                        if op.kind == "LOCK_ADMISSION":
                            result = {"event_hash": admission.lock(raw, now=now)}
                        else:
                            self._authenticated_v2(run_id, raw, "EscrowLock", op.owner)
                            result = escrow.lock(op, signer=env.signer).body()
                    case "transfer" | "release":
                        from hypertrain.protocol.messages_v2 import EscrowRelease, EscrowTransfer

                        env = envelope_v2.parse_envelope(raw)
                        expected = "EscrowTransfer" if action == "transfer" else "EscrowRelease"
                        model = self._authenticated_v2(run_id, raw, expected, env.signer)
                        if action == "transfer":
                            assert isinstance(model, EscrowTransfer)
                            result = escrow.transfer(model, signer=env.signer).body()
                        else:
                            assert isinstance(model, EscrowRelease)
                            result = escrow.release(model, signer=env.signer, now_beacon=now).body()
                    case "dispute" | "bisect" | "resolution" | "state-serve":
                        methods = {
                            "dispute": disputes.open,
                            "bisect": disputes.bisect,
                            "resolution": disputes.resolution,
                            "state-serve": disputes.state_serve,
                        }
                        result = methods[action](raw, now=now).model_dump(mode="json")
                        if action == "dispute":
                            admission.suspend(
                                result["contest"]["miner"],
                                now=now,
                                reason="PENDING_DISPUTE",
                                pending_dispute=True,
                                dispute_id=result["dispute_id"],
                                evidence_hash=result["contest"]["evidence_hash"],
                            )
                        if action in ("dispute", "resolution"):
                            w = result["contest"]["w"]
                            finality = self._record_v2(run_id, "finality", str(w))
                            pending = self._db.execute(
                                "SELECT COUNT(*) FROM disputes_v2 "
                                "WHERE json_extract(turn,'$.contest.w')=? "
                                "AND json_extract(turn,'$.resolution') IS NULL",
                                (w,),
                            ).fetchone()[0]
                            finality["unresolved_disputes"] = pending
                            self._put_record_v2(run_id, "finality", str(w), finality)
                        if action == "resolution":
                            disputes.settle_lock(result["dispute_id"], now=now)
                            self._put_record_v2(
                                run_id,
                                "resolution",
                                result["dispute_id"],
                                envelope_v2.parse_envelope(raw).model_dump(mode="json"),
                            )
                    case _:
                        raise ChallengeError(404, "unknown v2 operation")
        if failure is not None:
            raise failure
        disputes.notify()  # Only after outer acceptance transaction commits.
        self._notify_processes_v2()
        return result

    def _authenticated_v2(self, run_id: str, raw: bytes, expected: str, signer: str) -> BaseModel:
        env = envelope_v2.parse_envelope(raw)
        if env.type != expected:
            raise ChallengeError(400, f"expected {expected}")
        model = envelope_v2.Intake(run_id, {expected: signer.__eq__}).accept(
            raw, self._now(self._db)
        )
        key = "auth:" + _dumps(list(envelope_v2.replay_key(expected, run_id, env.signer, model)))
        digest = body_digest(env.body)
        known = self._db.execute("SELECT digest FROM accepted_v2 WHERE key=?", (key,)).fetchone()
        if known is not None and known[0] != digest:
            raise ChallengeError(409, "conflicting signed operation nonce")
        if known is None:
            self._db.execute(
                "INSERT INTO accepted_v2 VALUES(?,?,?,?,?)",
                (key, digest, raw, self._now(self._db), "{}"),
            )
        return model

    def trial_proof_v2(self, run_id: str, proof: bytes, screen: bytes) -> dict[str, Any]:
        with self._tx():
            _, admission, _ = self._services(run_id)
            now = self._fresh_v2(self._run_v2(run_id), f"proof:{self._now(self._db)}")
            return asdict(admission.proof(proof, screen, now=now))

    def retain_runtime_custody_v2(
        self,
        run_id: str,
        runtime: NetworkRuntime,
        spec: Operation,
        directory: Path,
        *,
        tree: Path,
        cancel: threading.Event | None = None,
    ) -> dict[str, str]:
        """Internal trusted-runtime intake; no HTTP route or production authority."""
        with self._tx():
            _, admission, _ = self._services(run_id)
            return admission.retain_runtime_custody(
                runtime, spec, directory, tree=tree, now=self._now(self._db), cancel=cancel
            )

    def trial_reference_v2(
        self, run_id: str, admission_id: str, *, retain_shadow_custody: bool = False
    ) -> dict[str, str]:
        from hypertrain.auditor.worker import LeaseGuard

        _, admission, _ = self._services(run_id)
        with self._lock:
            now = self._fresh_v2(
                self._run_v2(run_id), f"reference:{admission_id}:{self._now(self._db)}"
            )
            if self._role_launch is None:
                value = admission.prepare_reference(
                    admission_id, now=now, retain_shadow_custody=retain_shadow_custody
                )
            else:
                current = admission.store.db.execute(
                    "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
                ).fetchone()
                if current is None or current["challenge"] is None:
                    raise ChallengeError(409, "reference lacks accepted challenge")
                challenge = JoinChallenge.model_validate_json(current["challenge"])
                cutoff = (
                    self._run_v2(run_id).training.beacon.genesis_time
                    + (challenge.deadline_beacon - 1) * 3
                )
                guard = LeaseGuard(challenge.deadline_beacon, challenge.deadline_beacon, cutoff)
                key = f"reference:{admission_id}:{challenge.nonce}"

                def trusted(
                    job: IslandJobV1, directory: Path, c: JoinChallenge, epoch: int
                ) -> IslandLaunch:
                    assert self._role_launch is not None
                    if c != challenge or job.deadline != cutoff:
                        raise ChallengeError(409, "reference launch subject changed")
                    return self._role_launch(
                        "reference",
                        job,
                        directory,
                        {
                            "run_id": run_id,
                            "admission_id": admission_id,
                            "epoch": epoch,
                            "challenge": c.body(),
                            "challenge_hash": c.digest(),
                            "challenge_envelope": json.loads(
                                admission.store.db.execute(
                                    "SELECT receipt FROM admission_reservations "
                                    "WHERE reservation=?",
                                    (f"challenge|{admission_id}|{c.nonce}",),
                                ).fetchone()[0]
                            ),
                            "hotkey": current["hotkey"],
                        },
                    )

                self._lease_guards_v2[key] = guard
                try:
                    with guard:
                        guard.advance(now)
                        guard.check()
                        value = admission.prepare_reference(
                            admission_id,
                            now=now,
                            trusted_launch=trusted,
                            cancel=guard.cancelled,
                            retain_shadow_custody=retain_shadow_custody,
                        )
                        guard.check()
                finally:
                    self._lease_guards_v2.pop(key, None)
        return {"reference_commitment": value}

    def trial_finalize_v2(self, run_id: str, admission_id: str, raw: bytes) -> dict[str, Any]:
        with self._tx():
            _, admission, _ = self._services(run_id)
            now = self._fresh_v2(self._run_v2(run_id), f"trial-final:{self._now(self._db)}")
            return asdict(admission.finalize_trial(admission_id, raw, now=now))

    def _shadow_trial_v2(
        self, run_id: str, admission_id: str, raw: bytes, *, accepted_at: int | None = None
    ) -> tuple:
        """Revalidate original accepted custody without reserving or issuing."""
        from hypertrain.auditor.replay import unpack_state
        from hypertrain.miner.island_launch import confined, validate_artifacts
        from hypertrain.protocol.messages_v2 import CommitV2, WorkProof
        from hypertrain.trainer.compress import state_hash

        request = ShadowReservationIntake.model_validate(envelope_v2.load_json(raw))
        escrow, admission, _ = self._services(run_id)
        with self._lock:
            now = self._now(self._db) if accepted_at is None else accepted_at
            row = self._db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=? AND run_id=?",
                (admission_id, run_id),
            ).fetchone()
            if row is None:
                raise ChallengeError(409, "shadow admission missing")
            final = envelope_v2.Intake(run_id, {"Finalize": self._coord().ss58.__eq__}).accept(
                request.finalize.model_dump(mode="json"), now
            )
            commit = envelope_v2.Intake(run_id, {"CommitV2": row["hotkey"].__eq__}).accept(
                request.commit.model_dump(mode="json"), now
            )
            if not isinstance(final, Finalize) or not isinstance(commit, CommitV2):
                raise ChallengeError(422, "expected original Finalize and CommitV2")
            trial = self._db.execute(
                "SELECT * FROM admission_trials WHERE epoch=? AND admission_id=?",
                (final.w, admission_id),
            ).fetchone()
            result = self._db.execute(
                "SELECT * FROM admission_trial_results WHERE epoch=?", (final.w,)
            ).fetchone()
            accepted_final = self._db.execute(
                "SELECT digest FROM admission_reservations WHERE reservation=?",
                (f"trial-final|{admission_id}|{final.w}",),
            ).fetchone()
            if (
                trial is None
                or trial["finalized_beacon"] is None
                or result is None
                or accepted_final is None
                or accepted_final[0] != body_digest(final.model_dump(mode="json"))
                or result["reference"] is None
                or result["proof"] is None
                or result["screen"] is None
            ):
                raise ChallengeError(409, "shadow original accepted trial missing")
            challenge = JoinChallenge.model_validate_json(result["challenge"])
            challenge_receipt = self._db.execute(
                "SELECT receipt FROM admission_reservations WHERE reservation=?",
                (f"challenge|{admission_id}|{challenge.nonce}",),
            ).fetchone()
            if (
                challenge_receipt is None
                or envelope_v2.Intake(run_id, {"JoinChallenge": self._coord().ss58.__eq__}).accept(
                    challenge_receipt[0], trial["received_beacon"]
                )
                != challenge
            ):
                raise ChallengeError(409, "shadow original signed challenge differs")
            reference = WorkProof.model_validate_json(result["reference"])
            proof_env = envelope_v2.parse_envelope(result["proof"])
            proof = envelope_v2.Intake(run_id, {"WorkProof": row["hotkey"].__eq__}).accept(
                proof_env.model_dump(mode="json"), trial["finalized_beacon"]
            )
            screen = WorkScreenV2.model_validate(
                envelope_v2.Intake(run_id, {"WorkScreenV2": row["hotkey"].__eq__})
                .accept(result["screen"], trial["finalized_beacon"])
                .model_dump(mode="json")
            )
            job = IslandJobV1.model_validate_json(self.objects.get(request.custody.job_hash))
            if (
                job.manifest != escrow.manifest
                or job.run_id != run_id
                or job.w != final.w
                or challenge.admission_id != admission_id
                or challenge.nonce != trial["nonce"]
                or challenge.manifest_hash != run_id
                or proof != reference
                or job.deadline
                != escrow.manifest.training.beacon.genesis_time
                + (challenge.deadline_beacon - 1) * 3
                or job.manifest.training.reference_spec.layout != challenge.layout
                or trial["evidence_hash"] != reference.digest()
                or reference.challenge_hash != challenge.digest()
                or screen.challenge_hash != challenge.digest()
                or screen.nonce != challenge.nonce
                or not admission.qualified(screen)
                or challenge.assignment_hash
                != trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids))
                or commit.hotkey != row["hotkey"]
                or commit.w != final.w
                or final.included != [row["hotkey"]]
                or final.entitlements_root != reference.digest()
                or commit.leaves_root != reference.leaves_root
                or commit.delta_hash != reference.delta_hash
            ):
                raise ChallengeError(409, "shadow original work binding differs")
            from hypertrain.data.trial_assignment import trial_samples

            if tuple(job.sample_ids) != trial_samples(
                job.manifest,
                admission_id,
                challenge.nonce,
                self._beacon_v2(challenge.seed_beacon),
            ):
                raise ChallengeError(409, "shadow original ordered samples differ")
            commit.validate_assignment(job.manifest, len(job.sample_ids))
            records = []
            for prefix, operation in (("reference", "reference"), ("miner", "probe")):
                operation_hash = getattr(request.custody, prefix + "_operation_hash")
                accepted = self._record_v2(run_id, "shadow-execution", operation_hash)
                operation_body = envelope_v2.load_json(self.objects.get(operation_hash))
                if operation_body != {k: v for k, v in accepted.items() if k != "directory"}:
                    raise ChallengeError(409, "shadow accepted operation object differs")
                publication_hash = getattr(request.custody, prefix + "_publication_hash")
                result_hash = getattr(request.custody, prefix + "_result_hash")
                if (
                    accepted["operation"] != operation
                    or accepted["run_id"] != run_id
                    or accepted["admission_id"] != admission_id
                    or accepted["epoch"] != job.w
                    or accepted["challenge_hash"] != challenge.digest()
                    or accepted["job_hash"] != request.custody.job_hash
                    or accepted["publication_hash"] != publication_hash
                    or accepted["result_hash"] != result_hash
                    or accepted["backend"] != admission.backend
                    or accepted["accepted_beacon"] > trial["finalized_beacon"]
                ):
                    raise ChallengeError(409, "shadow accepted execution binding differs")
                publication = envelope_v2.load_json(self.objects.get(publication_hash))
                original_result = envelope_v2.load_json(self.objects.get(result_hash))
                if (
                    original_result["job_hash"] != request.custody.job_hash
                    or original_result["publication_hash"] != publication_hash
                    or WorkProof.model_validate(original_result["proof"]) != reference
                    or operation == "probe"
                    and original_result.get("commit_hash")
                    != body_digest(commit.model_dump(mode="json"))
                ):
                    raise ChallengeError(409, "shadow original result differs")
                retained_screen = WorkScreenV2.model_validate(original_result["screen"])
                if not admission.qualified(retained_screen) or (
                    retained_screen.challenge_hash != challenge.digest()
                    or retained_screen.admission_id != admission_id
                    or retained_screen.nonce != challenge.nonce
                    or retained_screen.artifact_hashes
                    != [a.sha256 for a in reference.artifact_refs]
                ):
                    raise ChallengeError(409, "shadow retained qualified screen differs")
                if operation == "probe":
                    original_commit = envelope_v2.Intake(
                        run_id, {"CommitV2": row["hotkey"].__eq__}
                    ).accept(
                        canonicalize(original_result["commit_envelope"]), trial["finalized_beacon"]
                    )
                    if original_commit != commit:
                        raise ChallengeError(409, "shadow original accepted miner commit differs")
                directory = Path(accepted["directory"])
                if directory.is_symlink() or not directory.resolve().is_relative_to(
                    self.state_dir.resolve()
                ):
                    raise ChallengeError(409, "shadow accepted publication outside store custody")
                actual_files = {}
                for path in directory.rglob("*"):
                    if path.is_symlink() or not (path.is_file() or path.is_dir()):
                        raise ChallengeError(409, "shadow publication special file")
                    if path.is_file():
                        data = confined(directory, str(path.relative_to(directory))).read_bytes()
                        actual_files[str(path.relative_to(directory))] = {
                            "sha256": sha256_hex(data),
                            "bytes": len(data),
                        }
                if actual_files != publication:
                    raise ChallengeError(409, "shadow original publication changed")
                for metadata in publication.values():
                    assert isinstance(metadata, dict) and isinstance(metadata["sha256"], str)
                    data = self.objects.get(metadata["sha256"])
                    if sha256_hex(data) != metadata["sha256"] or len(data) != metadata["bytes"]:
                        raise ChallengeError(409, "shadow addressed publication changed")
                artifacts = validate_artifacts(job, directory)
                if tuple(retained_screen.rank_results) != artifacts.ranks:
                    raise ChallengeError(409, "shadow original rank screen differs")
                if any(
                    envelope_v2.load_json(
                        confined(directory, f"rank-{rank}/summary.json").read_bytes()
                    )["backend"]
                    != admission.backend
                    for rank in range(len(artifacts.ranks))
                ):
                    raise ChallengeError(409, "shadow original rank backend differs")
                summary = envelope_v2.load_json((directory / "rank-0/summary.json").read_bytes())
                commitments = summary["commitments"]
                assert isinstance(commitments, dict)
                if (
                    summary["backend"] != admission.backend
                    or commitments["final_theta_hash"] != commit.final_theta_hash
                    or commitments["ef_out_hash"] != commit.ef_out_hash
                    or artifacts.delta.stat().st_size != commit.delta_bytes
                    or state_hash(unpack_state(artifacts.state.read_bytes())[0])
                    != final.final_theta_hash_w1
                    or state_hash(
                        unpack_state(
                            confined(directory, job.object_paths["start_state"]).read_bytes()
                        )[0]
                    )
                    != challenge.theta_hash
                    or state_hash(
                        unpack_state(confined(directory, job.object_paths["ef_in"]).read_bytes())[0]
                    )
                    != commit.ef_in_hash
                ):
                    raise ChallengeError(409, "shadow final commitment differs")
                records.append(accepted)
            if (
                request.custody.reference_operation_hash == request.custody.miner_operation_hash
                or records[0]["directory"] == records[1]["directory"]
                or records[0]["accepted_beacon"] > records[1]["accepted_beacon"]
            ):
                raise ChallengeError(409, "shadow independent execution missing")
            graph_hash = body_digest({"manifest": escrow.manifest.body()})
            if escrow.policy.ledger_mode == "test":
                economic_hash = escrow.policy.digest()
            else:
                authority = self._shadow_operator_v2(run_id, historical=accepted_at is not None)
                identities = authority["workload"]["identities"]
                identity = next(
                    (
                        i
                        for i in identities
                        if (i["hotkey"], i["coldkey"]) == (row["hotkey"], row["coldkey"])
                    ),
                    None,
                )
                if identity is None or trial["finalized_beacon"] > authority["expires_beacon"]:
                    raise ChallengeError(403, "operator reservation roster/time differs")
                for accepted in records:
                    if (
                        accepted.get("operator_authority_hash") != authority["hash"]
                        or accepted.get("graph_hash") != authority["graph_hash"]
                        or accepted.get("source_map_hash") != authority["source_map_hash"]
                        or accepted.get("host") != authority["hosts"][identity["role"]]
                        or type(accepted.get("completed_unix")) is not int
                        or accepted["completed_unix"] > authority["cutoff_unix"]
                    ):
                        raise ChallengeError(403, "operator original execution custody differs")
                graph_hash, economic_hash = authority["graph_hash"], authority["hash"]
            binding = ShadowReservationRequestV1(
                run_id=run_id,
                admission_id=admission_id,
                hotkey=row["hotkey"],
                coldkey=row["coldkey"],
                trial_epoch=final.w,
                commit_hash=body_digest(commit.model_dump(mode="json")),
                finalize_hash=body_digest(final.model_dump(mode="json")),
                custody_hash=request.custody.digest(),
                graph_hash=graph_hash,
                economic_admission_hash=economic_hash,
                nonce=challenge.nonce,
                assignment_hash=challenge.assignment_hash,
                finalized_beacon=trial["finalized_beacon"],
                mature_at=trial["finalized_beacon"] + escrow.manifest.training.verify.E_vest_rounds,
            )
            return binding, request, job, dict(trial), dict(result), records, final

    def shadow_reservation_v2(self, run_id: str, admission_id: str, raw: bytes) -> dict[str, Any]:
        """Authenticate original accepted work; reserve only, never issue units."""
        binding, request, job, trial, result, records, final = self._shadow_trial_v2(
            run_id, admission_id, raw
        )
        escrow, admission, _ = self._services(run_id)
        with self._lock:
            now = self._now(self._db)
            row = admission.store.by_id(admission_id)
            semantic_key = f"shadow-reserve|{run_id}|{admission_id}|{final.w}"
            snapshots = [
                ("SELECT * FROM admission_trials WHERE epoch=?", final.w, dict(trial)),
                ("SELECT * FROM admission_trial_results WHERE epoch=?", final.w, dict(result)),
            ]
            store = self

            class ReservationEscrow(EscrowV2):
                _reserving_shadow: bool = False

                def reserve_shadow(
                    self,
                    request: ShadowReservationRequestV1,
                    evidence: ShadowReservationEvidence,
                ) -> ShadowReservationV1:
                    # This disposable instance alone enables the guard for this one call.
                    self._reserving_shadow = True
                    try:
                        return super().reserve_shadow(request, evidence)
                    finally:
                        self._reserving_shadow = False

                @contextmanager
                def tx(self) -> Iterator[sqlite3.Connection]:
                    # Constructor/verification use original tx semantics, without freshness writes.
                    with super().tx() as db:
                        if not self._reserving_shadow:
                            yield db
                            return
                        for query, value, expected in snapshots:
                            current_row = db.execute(query, (value,)).fetchone()
                            if current_row is None or dict(current_row) != expected:
                                raise ChallengeError(
                                    409, "shadow accepted trial changed before reservation"
                                )
                        for accepted in records:
                            operation_hash = body_digest(
                                {k: v for k, v in accepted.items() if k != "directory"}
                            )
                            if (
                                store._record_v2(run_id, "shadow-execution", operation_hash)
                                != accepted
                            ):
                                raise ChallengeError(
                                    409, "shadow execution changed before reservation"
                                )
                        old = db.execute(
                            "SELECT digest FROM admission_reservations WHERE reservation=?",
                            (semantic_key,),
                        ).fetchone()
                        if old is None:
                            if result["outcome"] != "MATCH":
                                raise ChallengeError(
                                    409, "shadow new reservation requires original MATCH"
                                )
                            store._fresh_v2(
                                self.manifest, f"shadow-reservation:{admission_id}:{now}"
                            )
                            current = admission.store.by_id(admission_id)
                            first = db.execute(
                                "SELECT MIN(epoch) FROM admission_trials WHERE admission_id=? "
                                "AND finalized_beacon IS NOT NULL",
                                (admission_id,),
                            ).fetchone()[0]
                            if (
                                current.clean_count != 1
                                or first != final.w
                                or current.pending_dispute
                                or db.execute(
                                    "SELECT 1 FROM admission_pending WHERE admission_id=? "
                                    "AND resolved=0",
                                    (admission_id,),
                                ).fetchone()
                                is not None
                            ):
                                raise ChallengeError(
                                    409, "shadow new reservation requires settled first trial"
                                )
                        yield db
                        if old is None:
                            store._put_record_v2(
                                run_id,
                                "shadow-custody",
                                semantic_key,
                                {
                                    "request": request.model_dump(mode="json"),
                                    "accepted_at": now,
                                },
                            )

            intake = ReservationEscrow(
                self._db,
                escrow.manifest,
                escrow.policy,
                auditors=escrow.auditors,
                replay_finality=escrow.replay_finality,
                settlement=escrow.settlement,
                owner_of=escrow.owner_of,
                assignment_of=escrow.assignment_of,
            )
            intake.mutex = self._lock
            reserved = intake.reserve_shadow(
                binding,
                ShadowReservationEvidence(
                    finalize=request.finalize,
                    commit=request.commit,
                    owner=row.coldkey,
                    sample_ids=tuple(job.sample_ids),
                    custody_hash=request.custody.digest(),
                    reference_final_state_hash=final.final_theta_hash_w1,
                ),
            )
            return {
                "reservation": reserved.body(),
                "reservation_hash": reserved.digest(),
                "shadow_ordinal": reserved.shadow_ordinal,
            }

    def _shadow_binding_v2(self, run_id: str, reservation_hash: str) -> ShadowReservationV1:
        rows = self._db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='shadow-reservation'",
            (run_id,),
        ).fetchall()
        for row in rows:
            binding = ShadowReservationV1.model_validate(json.loads(row[0])["binding"])
            if binding.digest() == reservation_hash:
                semantic = self._db.execute(
                    "SELECT receipt FROM admission_reservations WHERE reservation=?",
                    (f"shadow-reserve|{run_id}|{binding.admission_id}|{binding.trial_epoch}",),
                ).fetchone()
                if (
                    semantic is None
                    or ShadowReservationV1.model_validate_json(semantic[0]) != binding
                ):
                    raise ChallengeError(409, "shadow semantic reservation differs")
                return binding
        raise ChallengeError(409, "shadow reservation missing")

    def _shadow_custody_v2(self, run_id: str, binding: ShadowReservationV1) -> tuple:
        record = self._record_v2(
            run_id,
            "shadow-custody",
            f"shadow-reserve|{run_id}|{binding.admission_id}|{binding.trial_epoch}",
        )
        validated = self._shadow_trial_v2(
            run_id,
            binding.admission_id,
            canonicalize(record["request"]),
            accepted_at=record["accepted_at"],
        )
        if validated[0].body() != binding.model_dump(mode="json", exclude={"shadow_ordinal"}):
            raise ChallengeError(409, "shadow original reservation custody differs")
        return validated

    def _shadow_assignment_v2(self, run_id: str, binding: ShadowReservationV1) -> tuple[int, ...]:
        job = self._shadow_custody_v2(run_id, binding)[2]
        return tuple(job.sample_ids)

    def _shadow_status_v2(self, run_id: str, binding: ShadowReservationV1) -> Settlement:
        """Resolve current disputes from accepted store records, never supplied flags."""
        admission = self._services(run_id)[1].store.by_id(binding.admission_id)
        trial = self._db.execute(
            "SELECT outcome FROM admission_trial_results WHERE epoch=?", (binding.trial_epoch,)
        ).fetchone()
        if trial is None:
            raise EscrowError("SHADOW_TRIAL_MISSING")
        unresolved = admission.pending_dispute or trial[0] != "MATCH"
        fraud = admission.state == "BANNED" or trial[0] == "FRAUD"
        release = binding.mature_at
        closed = []
        seen = set()
        for row in self._db.execute(
            "SELECT * FROM admission_pending WHERE admission_id=?", (binding.admission_id,)
        ).fetchall():
            if not row["resolved"]:
                unresolved = True
                continue
            status = self._settlement_v2(run_id, row["dispute_id"])
            if (
                status.run_id,
                status.admission_id,
                status.coldkey,
                status.dispute_id,
                status.evidence_hash,
            ) != (
                run_id,
                binding.admission_id,
                binding.coldkey,
                row["dispute_id"],
                row["evidence_hash"],
            ):
                raise EscrowError("SHADOW_DISPUTE_LINKAGE")
            unresolved = unresolved or status.unresolved
            fraud = fraud or status.outcome == "FRAUD"
            release = max(release, status.release_beacon)
            closed.append([row["dispute_id"], status.closed_dispute_root])
            seen.add(row["dispute_id"])
        for row in self._db.execute(
            "SELECT id FROM disputes_v2 WHERE json_extract(turn,'$.contest.admission_id')=? "
            "OR (json_extract(turn,'$.contest.miner')=? AND json_extract(turn,'$.contest.w')=?)",
            (binding.admission_id, binding.hotkey, binding.trial_epoch),
        ).fetchall():
            if row[0] in seen:
                continue
            status = self._services(run_id)[2].settlement(row[0])
            unresolved = unresolved or status.unresolved
            fraud = fraud or status.outcome == "FRAUD"
            release = max(release, status.release_beacon)
            closed.append([row[0], status.closed_dispute_root])
        return Settlement(
            finality_hash=binding.finalize_hash,
            closed_dispute_root=sha256_hex(canonicalize(sorted(closed))),
            release_beacon=release,
            unresolved=unresolved,
            outcome="FRAUD" if fraud else "MATCH",
            run_id=run_id,
            admission_id=binding.admission_id,
            coldkey=binding.coldkey,
        )

    def _shadow_settlement_v2(self, run_id: str, origin: str) -> Settlement:
        row = self._db.execute(
            "SELECT reservation_hash FROM escrow_shadow_finalized WHERE origin=?", (origin,)
        ).fetchone()
        if row is None:
            raise EscrowError("SHADOW_FINALITY_MISSING")
        binding = self._shadow_binding_v2(run_id, row[0])
        self._record_v2(run_id, "shadow-evidence", binding.digest())
        self._shadow_custody_v2(run_id, binding)
        return self._shadow_status_v2(run_id, binding)

    def _shadow_anchor_v2(self, job: IslandJobV1, hotkey: str, directory: Path) -> str:
        """Derive original trial genesis anchor from actual start/optimizer/EF bytes."""
        from hypertrain.auditor.replay import AnchorCache, optimizer_hash, unpack_state
        from hypertrain.miner.island_launch import confined
        from hypertrain.protocol.messages_v2 import StartStateV2
        from hypertrain.trainer.compress import state_hash

        theta, carried = unpack_state(
            confined(directory, job.object_paths["start_state"]).read_bytes()
        )
        genesis = AnchorCache().genesis(job.manifest, hotkey, theta)
        if carried is not None and optimizer_hash(carried) != optimizer_hash(genesis.state):
            raise ChallengeError(409, "shadow original trial optimizer differs from genesis")
        ef, _ = unpack_state(confined(directory, job.object_paths["ef_in"]).read_bytes())
        if job.global_step0 != 0 or state_hash(ef) != state_hash(genesis.ef):
            raise ChallengeError(409, "shadow original trial EF/start differs from genesis")
        return StartStateV2(
            run_id=job.run_id,
            w=job.w,
            hotkey=hotkey,
            theta_hash=state_hash(theta),
            state_object_sha256=job.start_state_sha256,
            opt_state_hash=optimizer_hash(genesis.state),
            ef_object_sha256=job.ef_in_sha256,
            ef_hash=state_hash(ef),
            parent_anchor_hash=genesis.anchor_hash,
            global_step0=0,
            anchor_verdict_hash=genesis.proof_hash,
        ).digest()

    def _shadow_replay_v2(self, run_id: str, evidence: FinalityEvidence) -> str:
        """Read accepted signed receipt and revalidate the original independent publication."""
        signed = envelope_v2.parse_envelope(self.objects.get(evidence.tape_hash))
        receipt = ShadowReplayReceiptV1.model_validate(signed.body)
        binding = self._shadow_binding_v2(run_id, receipt.reservation_hash)
        record = self._record_v2(run_id, "shadow-evidence", binding.digest())
        if (
            record["auditor_receipt_hash"] != evidence.tape_hash
            or not evidence.shadow
            or evidence.finalized_beacon != binding.finalized_beacon
            or body_digest(evidence.finalize.body) != binding.finalize_hash
        ):
            raise EscrowError("SHADOW_REPLAY_LINKAGE")
        request = ShadowAcceptanceIntake.model_validate(
            {
                "reservation_hash": binding.digest(),
                "audit_challenge": envelope_v2.load_json(
                    self.objects.get(record["audit_challenge_hash"])
                ),
                "verdict": envelope_v2.load_json(self.objects.get(record["verdict_hash"])),
                "auditor_receipt": signed.model_dump(mode="json"),
                "reward": record["reward"],
            }
        )
        validated, _, _, _ = self._validate_shadow_acceptance_v2(
            run_id, binding, request, record["accepted_at"], historical=True
        )
        if (
            len(evidence.works) != 1
            or body_digest(evidence.works[0].commit.body) != binding.commit_hash
            or evidence.works[0].owner != binding.coldkey
            or evidence.works[0].sample_ids != tuple(validated[2].sample_ids)
            or body_digest(evidence.works[0].verdict.body) != body_digest(request.verdict.body)
            or body_digest(evidence.works[0].challenge.body)
            != body_digest(request.audit_challenge.body)
        ):
            raise EscrowError("SHADOW_REPLAY_WORK_LINKAGE")
        status = self._shadow_status_v2(run_id, binding)
        if status.unresolved or status.outcome == "FRAUD":
            raise EscrowError("SHADOW_UNSETTLED")
        # State comes from the independently executed reference, not a signed MATCH scalar.
        from hypertrain.auditor.replay import unpack_state
        from hypertrain.trainer.compress import state_hash

        reference = Path(validated[5][0]["directory"]) / "rank-0/state.safetensors"
        return state_hash(unpack_state(reference.read_bytes())[0])

    def _validate_shadow_acceptance_v2(
        self,
        run_id: str,
        binding: ShadowReservationV1,
        request: ShadowAcceptanceIntake,
        now: int,
        *,
        historical: bool = False,
    ) -> tuple:
        from hypertrain.protocol.messages_v2 import AuditChallengeV2, WorkProof

        manifest = self._run_v2(run_id)
        auditor = request.auditor_receipt.signer
        if auditor not in manifest.training.auditors or auditor == binding.hotkey:
            raise ChallengeError(403, "shadow distinct pinned auditor required")
        audit = envelope_v2.Intake(run_id, {"AuditChallengeV2": self._coord().ss58.__eq__}).accept(
            request.audit_challenge.model_dump(mode="json"), now
        )
        verdict = envelope_v2.Intake(run_id, {"ReplayVerdict": auditor.__eq__}).accept(
            request.verdict.model_dump(mode="json"), now
        )
        receipt = envelope_v2.Intake(run_id, {"ShadowReplayReceiptV1": auditor.__eq__}).accept(
            request.auditor_receipt.model_dump(mode="json"), now
        )
        if (
            not isinstance(audit, AuditChallengeV2)
            or not isinstance(verdict, ReplayVerdict)
            or not isinstance(receipt, ShadowReplayReceiptV1)
        ):
            raise ChallengeError(422, "shadow evidence types differ")
        self._services(run_id)[0]._authority(request.reward)
        validated = self._shadow_custody_v2(run_id, binding)
        _, original, job, trial, result, operations, final = validated
        challenge = JoinChallenge.model_validate_json(result["challenge"])
        signed_challenge = self._db.execute(
            "SELECT receipt FROM admission_reservations WHERE reservation=?",
            (f"challenge|{binding.admission_id}|{challenge.nonce}",),
        ).fetchone()[0]
        reference = WorkProof.model_validate_json(result["reference"])
        miner = WorkProof.model_validate(envelope_v2.parse_envelope(result["proof"]).body)
        expected = {
            "run_id": run_id,
            "admission_id": binding.admission_id,
            "hotkey": binding.hotkey,
            "coldkey": binding.coldkey,
            "trial_epoch": binding.trial_epoch,
            "shadow_ordinal": binding.shadow_ordinal,
            "reservation_hash": binding.digest(),
            "challenge_hash": challenge.digest(),
            "signed_challenge_hash": sha256_hex(
                canonicalize(envelope_v2.load_json(signed_challenge))
            ),
            "assignment_hash": binding.assignment_hash,
            "sample_ids_hash": sha256_hex(canonicalize(job.sample_ids)),
            **original.custody.body(),
            "reference_work_proof_hash": reference.digest(),
            "miner_work_proof_hash": miner.digest(),
            "commit_hash": binding.commit_hash,
            "audit_challenge_hash": body_digest(audit.model_dump(mode="json")),
            "verdict_hash": body_digest(verdict.model_dump(mode="json")),
            "trial_finalize_hash": binding.finalize_hash,
            "final_state_hash": final.final_theta_hash_w1,
            "backend_qualification_authority_hash": self._shadow_backend_authority_v2(
                run_id, historical=historical
            ),
            "completed_beacon": receipt.completed_beacon,
        }
        if receipt.body() != expected or (
            not binding.finalized_beacon <= receipt.completed_beacon <= now
            or (audit.w, audit.target) != (binding.trial_epoch, binding.hotkey)
            or audit.anchor_hash
            != self._shadow_anchor_v2(job, binding.hotkey, Path(operations[0]["directory"]))
            or audit.mode != "full"
            or audit.segments
            or audit.audit_mode != "anchored-full"
            or audit.beacon_round < challenge.seed_beacon
            or audit.beacon_round > receipt.completed_beacon
            or audit.beacon_sig_sha256
            != sha256_hex(bytes.fromhex(self._beacon_v2(audit.beacon_round).signature))
            or not receipt.completed_beacon <= audit.serve_deadline <= challenge.deadline_beacon
            or verdict.challenge_hash != body_digest(audit.model_dump(mode="json"))
            or verdict.result != "MATCH"
            or verdict.first_bad_leaf is not None
            or verdict.recomputed_leaves_root != reference.leaves_root
            or verdict.replay_env.image_digest != manifest.training.reference_spec.image_digest
            or verdict.replay_env.driver not in manifest.training.reference_spec.driver_allowlist
            or verdict.replay_env.sm_count != manifest.training.reference_spec.sm_count
            or request.reward.reservation_hash != binding.digest()
            or request.reward.shadow_ordinal != binding.shadow_ordinal
            or request.reward.w != binding.trial_epoch
            or request.reward.finalize_hash != binding.finalize_hash
            or request.reward.mature_at != binding.mature_at
        ):
            raise ChallengeError(409, "shadow authenticated evidence binding differs")
        return validated, audit, verdict, receipt

    def accept_shadow_operator_v2(self, graph_path: Path, runtime: NetworkRuntime) -> str:
        """Private genuine-runtime intake, atomic and immutable. No provider or kernels."""
        from experiments.gpu_network_v2.orchestrate import NetworkRuntime, reviewed_launch
        from scripts.network_service_proof import (
            ContinuationGraph,
            continuation_graph,
            continuation_graph_hash,
            continuation_operator_record,
            continuation_qualification,
        )

        if type(runtime) is not NetworkRuntime:
            raise ChallengeError(403, "operator genuine runtime required")
        untrusted = envelope_v2.load_json(graph_path.read_bytes())
        run_id = untrusted["run_id"]
        assert isinstance(run_id, str)
        owner = self.owner_hotkey
        assert owner is not None
        with self._tx():
            manifest = self._run_v2(run_id)
            now = self._now(self._db)
            if not self._beacon_v2(now).bls_verified:
                raise ChallengeError(403, "operator verified beacon required")
            graph = ContinuationGraph.model_validate(untrusted)
            assert graph.economic_authority is not None
            raw = graph.economic_authority.read_bytes()
            existing = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? "
                "AND kind='shadow-operator' AND id='run'",
                (run_id,),
            ).fetchone()
            if existing is not None:
                stored = json.loads(existing[0])
                record, digest, payload = shadow_operator_receipt(
                    raw, owner, run_id, stored["accepted_at"]
                )
                if stored["payload"] != payload or stored["hash"] != digest:
                    raise ChallengeError(409, "operator lost-response authority changed")
                if record.graph_hash != continuation_graph_hash(graph):
                    raise ChallengeError(409, "operator lost-response graph changed")
                for role, frozen in record.hosts.items():
                    assert isinstance(frozen, dict)
                    if runtime.owned_host(role) != {
                        key: frozen[key] for key in ("instance_id", "machine_id", "image_digest")
                    }:
                        raise ChallengeError(409, "operator lost-response host changed")
                self._shadow_operator_v2(run_id, historical=True)
                return digest
            graph = continuation_graph(graph_path, runtime, owner, run_id, manifest)
            record, digest, payload = shadow_operator_receipt(raw, owner, run_id, now)
            policy = EconomicsPolicyV2.model_validate_json(
                self.objects.get(manifest.network.economics_policy_hash)
            )
            if policy.ledger_mode != "production" or self._backend_v2(manifest) != "cuda":
                raise ChallengeError(409, "operator accepted production CUDA required")
            if record.budget.get("scope") != "FULL116_CONTINUATION":
                raise ChallengeError(403, "operator FIRST4 is not full116 authorization")
            launch_path = Path(str(record.budget.get("launch_path", "")))
            if (
                not launch_path.is_absolute()
                or launch_path.is_symlink()
                or (sha256_hex(launch_path.read_bytes()) != record.budget.get("launch_sha256"))
            ):
                raise ChallengeError(403, "operator original signed launch custody differs")
            checked = reviewed_launch(launch_path, owner, now, "continue")
            continuation_qualification(self, manifest, graph, runtime, checked)
            backend = self._record_v2(
                run_id, "qualification", manifest.training.reference_spec.image_digest
            )
            expected = continuation_operator_record(
                graph, runtime, checked, backend, now, record.expires_beacon, launch_path
            )
            shadow_operator_expected(record, expected, time.time())
            self._retain_shadow_operator_v2(run_id, payload, digest, now, graph_path)
            return digest

    def _retain_shadow_operator_v2(
        self,
        run_id: str,
        payload: dict[str, JsonValue],
        digest: str,
        accepted_at: int,
        graph_path: Path,
    ) -> None:
        """Persistence only; caller must complete genuine admission predicates first."""
        with self._lock:
            self._db.execute("SAVEPOINT shadow_operator_intake")
            try:
                self._retain_shadow_operator_record(
                    run_id, payload, digest, accepted_at, graph_path
                )
            except BaseException:
                self._db.execute("ROLLBACK TO shadow_operator_intake")
                raise
            finally:
                self._db.execute("RELEASE shadow_operator_intake")

    def _retain_shadow_operator_record(
        self,
        run_id: str,
        payload: dict[str, JsonValue],
        digest: str,
        accepted_at: int,
        graph_path: Path,
    ) -> None:
        with self._lock:
            old = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? "
                "AND kind='shadow-operator' AND id='run'",
                (run_id,),
            ).fetchone()
            body: dict[str, JsonValue] = {
                "payload": payload,
                "hash": digest,
                "accepted_at": accepted_at,
                "payload_hash": sha256_hex(canonicalize(payload)),
                "graph_path": str(graph_path),
            }
            if old is not None and json.loads(old[0]) != body:
                raise ChallengeError(409, "operator immutable retained authority conflicts")
            if old is not None:
                return
            if self.objects.put(canonicalize(payload)) != body["payload_hash"]:
                raise ChallengeError(403, "operator addressed payload hash differs")
            self._put_record_v2(
                run_id,
                "shadow-operator",
                "run",
                body,
            )

    def _shadow_operator_v2(self, run_id: str, *, historical: bool = False) -> dict:
        stored = self._record_v2(run_id, "shadow-operator", "run")
        owner = self.owner_hotkey
        assert owner is not None
        record, digest, _ = shadow_operator_receipt(
            canonicalize(stored["payload"]), owner, run_id, stored["accepted_at"]
        )
        if self.objects.get(stored["payload_hash"]) != canonicalize(stored["payload"]):
            raise ChallengeError(403, "operator addressed retained payload differs")
        from scripts.network_service_proof import ContinuationGraph, continuation_graph_hash

        graph = ContinuationGraph.model_validate_json(Path(stored["graph_path"]).read_bytes())
        if continuation_graph_hash(graph) != record.graph_hash:
            raise ChallengeError(403, "operator retained graph differs")
        for name, pin in graph.private_files.items():
            path = Path(name)
            if path.is_symlink() or sha256_hex(path.read_bytes()) != pin:
                raise ChallengeError(403, "operator retained private input differs")
        launch_path = record.budget["launch_path"]
        assert isinstance(launch_path, str)
        launch = envelope_v2.load_json(Path(launch_path).read_bytes())
        launch_record = launch["record"]
        assert isinstance(launch_record, dict)
        if (
            sha256_hex(canonicalize(launch_record)) != record.admission_hash
            or sha256_hex(Path(launch_path).read_bytes()) != record.budget["launch_sha256"]
        ):
            raise ChallengeError(403, "operator retained launch differs")
        budget_files, launch_files = record.budget["files"], launch_record["files"]
        assert isinstance(budget_files, dict) and isinstance(launch_files, dict)
        for name, digest_pin in budget_files.items():
            subject = launch_files[name]
            assert isinstance(subject, dict) and isinstance(subject["path"], str)
            path = Path(subject["path"])
            if path.is_symlink() or sha256_hex(path.read_bytes()) != digest_pin:
                raise ChallengeError(403, "operator retained input binding differs")
        assert isinstance(launch_record["tree"], str)
        tree = Path(launch_record["tree"])
        source_profile = launch_files["source_profile"]
        assert isinstance(source_profile, dict) and isinstance(source_profile["path"], str)
        for name, pin in graph.operation_sources.items():
            source = (
                Path(source_profile["path"])
                if name == "experiments/gpu_network_v2/profile.json"
                else tree / name
            )
            if source.is_symlink() or sha256_hex(source.read_bytes()) != pin:
                raise ChallengeError(403, "operator current operation source differs")
        manifest = self._run_v2(run_id)
        backend = self._record_v2(
            run_id, "qualification", manifest.training.reference_spec.image_digest
        )
        if (
            digest != stored["hash"]
            or record.manifest_hash != manifest.run_id()
            or record.economics_policy_hash != manifest.network.economics_policy_hash
            or self._backend_v2(manifest) != "cuda"
            or record.backend_qualification_authority_hash != backend["authority_hash"]
            or not historical
            and (self._now(self._db) > record.expires_beacon or time.time() >= record.cutoff_unix)
        ):
            raise ChallengeError(403, "operator retained qualification/cutoff differs")
        return {**record.model_dump(mode="json"), "hash": digest}

    def _shadow_backend_authority_v2(self, run_id: str, *, historical: bool = False) -> str:
        manifest = self._run_v2(run_id)
        if (
            self._backend_v2(manifest) != "cpu"
            or self._services(run_id)[0].policy.ledger_mode != "test"
        ):
            return self._shadow_operator_v2(run_id, historical=historical)[
                "backend_qualification_authority_hash"
            ]
        return body_digest(
            {
                "domain": "cpu-test-only",
                "reference": manifest.training.reference_spec.model_dump(mode="json"),
            }
        )

    def shadow_reward_v2(self, run_id: str, admission_id: str, raw: bytes) -> dict[str, Any]:
        """Accept original evidence and conserved issuance in one shared SQLite transaction."""
        request = ShadowAcceptanceIntake.model_validate(envelope_v2.load_json(raw))
        escrow, admission, _ = self._services(run_id)
        with self._tx():
            binding = self._shadow_binding_v2(run_id, request.reservation_hash)
            if binding.admission_id != admission_id:
                raise ChallengeError(409, "shadow acceptance admission differs")
            now = self._now(self._db)
            semantic = body_digest(
                {
                    "reservation_hash": binding.digest(),
                    "audit_challenge": request.audit_challenge.body,
                    "verdict": request.verdict.body,
                    "auditor_receipt": request.auditor_receipt.body,
                    "reward": request.reward.model_dump(mode="json", exclude={"authority_sig"}),
                }
            )
            previous = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='shadow-evidence' AND id=?",
                (run_id, binding.digest()),
            ).fetchone()
            if previous is not None:
                retained = json.loads(previous[0])
                if retained["semantic_digest"] != semantic or "response" not in retained:
                    raise ChallengeError(409, "shadow acceptance semantic conflict")
                try:
                    stored = ShadowAcceptanceIntake.model_validate(
                        {
                            "reservation_hash": binding.digest(),
                            "audit_challenge": envelope_v2.load_json(
                                self.objects.get(retained["audit_challenge_hash"])
                            ),
                            "verdict": envelope_v2.load_json(
                                self.objects.get(retained["verdict_hash"])
                            ),
                            "auditor_receipt": envelope_v2.load_json(
                                self.objects.get(retained["auditor_receipt_hash"])
                            ),
                            "reward": retained["reward"],
                        }
                    )
                except StoreError as error:
                    raise ChallengeError(
                        409, "shadow accepted evidence object unavailable"
                    ) from error
                validated, _, _, _ = self._validate_shadow_acceptance_v2(
                    run_id, binding, stored, retained["accepted_at"], historical=True
                )
                self._validate_shadow_acceptance_v2(
                    run_id, binding, request, retained["accepted_at"], historical=True
                )
                stored_semantic = body_digest(
                    {
                        "reservation_hash": binding.digest(),
                        "audit_challenge": stored.audit_challenge.body,
                        "verdict": stored.verdict.body,
                        "auditor_receipt": stored.auditor_receipt.body,
                        "reward": stored.reward.model_dump(mode="json", exclude={"authority_sig"}),
                    }
                )
                if (
                    stored_semantic != semantic
                    or stored.reward.tape_hash != retained["auditor_receipt_hash"]
                    or retained["response"]["authority_hash"] != retained["auditor_receipt_hash"]
                    or retained["response"]["reservation_hash"] != binding.digest()
                    or retained["response"]["shadow_ordinal"] != binding.shadow_ordinal
                ):
                    raise ChallengeError(409, "shadow accepted evidence semantic differs")
                finalized = self._db.execute(
                    "SELECT reservation_hash FROM escrow_shadow_finalized WHERE ordinal=?",
                    (binding.shadow_ordinal,),
                ).fetchone()
                if finalized is None or finalized[0] != binding.digest():
                    raise ChallengeError(409, "shadow accepted issuance missing")
                original = validated[1]
                recovered = escrow.reward_finalize(
                    request.reward,
                    FinalityEvidence(
                        original.finalize,
                        (
                            RewardWork(
                                original.commit,
                                stored.verdict,
                                stored.audit_challenge,
                                binding.coldkey,
                                tuple(validated[2].sample_ids),
                            ),
                        ),
                        retained["auditor_receipt_hash"],
                        True,
                        binding.finalized_beacon,
                        binding.mature_at,
                    ),
                )
                if recovered.body() != retained["response"]["escrow_receipt"]:
                    raise ChallengeError(409, "shadow accepted ledger receipt differs")
                return retained["response"]
            validated, _, _, _ = self._validate_shadow_acceptance_v2(run_id, binding, request, now)
            current = admission.store.by_id(admission_id)
            status = self._shadow_status_v2(run_id, binding)
            if (
                validated[4]["outcome"] != "MATCH"
                or current.clean_count != 1
                or status.unresolved
                or status.outcome == "FRAUD"
            ):
                raise ChallengeError(409, "shadow acceptance requires settled first trial")
            self._fresh_v2(escrow.manifest, f"shadow-acceptance:{binding.digest()}:{now}")
            receipt_hash = self.objects.put(
                canonicalize(request.auditor_receipt.model_dump(mode="json"))
            )
            if request.reward.tape_hash != receipt_hash:
                raise ChallengeError(409, "shadow reward original signed receipt differs")
            record: dict[str, JsonValue] = {
                "semantic_digest": semantic,
                "accepted_at": now,
                "audit_challenge_hash": self.objects.put(
                    canonicalize(request.audit_challenge.model_dump(mode="json"))
                ),
                "verdict_hash": self.objects.put(
                    canonicalize(request.verdict.model_dump(mode="json"))
                ),
                "auditor_receipt_hash": receipt_hash,
                "reward": request.reward.body(),
            }
            self._put_record_v2(run_id, "shadow-evidence", binding.digest(), record)
            original = validated[1]
            evidence = FinalityEvidence(
                original.finalize,
                (
                    RewardWork(
                        original.commit,
                        request.verdict,
                        request.audit_challenge,
                        binding.coldkey,
                        tuple(validated[2].sample_ids),
                    ),
                ),
                receipt_hash,
                True,
                binding.finalized_beacon,
                binding.mature_at,
            )
            ledger_receipt = escrow.reward_finalize(request.reward, evidence)
            response: dict[str, JsonValue] = {
                "reservation_hash": binding.digest(),
                "shadow_ordinal": binding.shadow_ordinal,
                "authority_hash": receipt_hash,
                "escrow_receipt": ledger_receipt.body(),
                "origin_ids": [origin for origin in request.reward.origin_ids],
                "mature_at": binding.mature_at,
            }
            self._put_record_v2(run_id, "shadow-settlement", binding.digest(), status.body())
            self._put_record_v2(
                run_id, "shadow-evidence", binding.digest(), {**record, "response": response}
            )
            return response

    def run_status_v2(self, run_id: str) -> dict[str, Any]:
        with self._lock:
            self._run_v2(run_id)
            row = self._db.execute("SELECT * FROM runs WHERE run_id=?", (run_id,)).fetchone()
            return {
                "run_id": run_id,
                "contract": 2,
                "status": row["status"],
                "manifest_envelope": json.loads(row["envelope"]),
                "now_round": self._now(self._db),
            }

    def configure_inputs_v2(self, run_id: str, data: Mapping[str, JsonValue]) -> dict[str, str]:
        """Administrative object linkage; samples still verify against the pinned dataset root."""
        with self._tx():
            self._run_v2(run_id)
            if set(data) != {"samples_hash", "proofs_hash"}:
                raise ChallengeError(422, "expected dataset object hashes")
            for value in data.values():
                self.objects.get(str(value))
            self._put_record_v2(run_id, "dataset", "inputs", data)
        return {"status": "configured"}

    def genesis_v2(self, run_id: str, raw: bytes) -> dict[str, JsonValue]:
        from hypertrain.protocol.messages_v2 import TestGenesis

        escrow, _, _ = self._services(run_id)
        with self._tx():
            model = self._authenticated_v2(run_id, raw, "TestGenesis", self._coord().ss58)
            assert isinstance(model, TestGenesis)
            receipt = escrow.test_genesis(model)
            escrow.mature(
                sha256_hex(b"ht-genesis-mature|" + run_id.encode()),
                tuple(o.origin_id for o in model.origins),
                self._now(self._db),
            )
            return receipt.body()

    def _service_profile_v2(self, run_id: str) -> _ServiceCapacityV1:
        """Derive the sole admitted tiny shape; never allocate tensors."""
        manifest = self._run_v2(run_id)
        m, inner = manifest.training.model, manifest.training.inner
        layout = manifest.training.reference_spec.layout
        if (
            (
                m.arch,
                m.compute_dtype,
                m.n_layers,
                m.d_model,
                m.n_heads,
                m.n_kv_heads,
                m.d_ff,
                m.vocab,
                m.seq_len,
                m.param_count,
            )
            != ("decoder", "fp32", 1, 8, 2, 2, 8, 16, 2, 728)
            or (layout.pp, layout.n_gpus, layout.dp_size, layout.ep_size, layout.zero1)
            != (1, 1, 1, 1, False)
            or (
                inner.H,
                inner.J,
                inner.micro_batch,
                inner.grad_accum,
                inner.opt,
                inner.state_policy,
            )
            != (2, 1, 1, 1, "adamw", "carry")
            or (manifest.training.outer.opt != "nesterov")
        ):
            raise ChallengeError(409, "SERVICE_PROFILE_GEOMETRY")
        if self._backend_v2(manifest) != "cpu":
            raise ChallengeError(409, "SERVICE_PROFILE_BACKEND")
        from hypertrain.aggregator.core import get_object
        from hypertrain.protocol.messages_v2 import AdmissionPolicyV2, AggregationPolicyV2
        from hypertrain.trainer.config import TrainConfig
        from hypertrain.trainer.model import param_shapes

        policy = AggregationPolicyV2.model_validate_json(
            get_object(self.objects, manifest.network.aggregation_policy_hash)
        )
        if policy.cclip_iters != 1:
            raise ChallengeError(409, "SERVICE_PROFILE_CCLIP")
        admission_policy = AdmissionPolicyV2.model_validate_json(
            get_object(self.objects, manifest.network.admission_policy_hash)
        )
        if admission_policy.artifact_limits.max_object_bytes < 65536:
            raise ChallengeError(409, "SERVICE_PROFILE_OBJECT_LIMIT")
        shapes = param_shapes(TrainConfig.from_manifest_v2(manifest).model)
        if sum(math.prod(shape) for shape in shapes.values()) != 728 or len(shapes) != 12:
            raise ChallengeError(409, "SERVICE_PROFILE_SHAPES")
        paths = (
            "aggregator/core.py",
            "aggregator/tape_v2.py",
            "aggregator/rollback_v2.py",
            "trainer/compress.py",
            "trainer/config.py",
            "trainer/model.py",
            "miner/island_launch.py",
            "aggregator/capacity_worker.py",
            "auditor/replay.py",
        )
        source = Path(__file__).resolve().parents[1]
        implementation = self._service_implementation_v2(source, paths)
        backend = self._record_v2(run_id, "execution-backend", "run")
        return _ServiceCapacityV1(
            version=1,
            profile_id="tiny-cpu-service-32-v1",
            run_id=run_id,
            backend_binding_hash=sha256_hex(canonicalize(backend)),
            implementation_hash=implementation,
            max_complete_roster=32,
            memory_reservation_bytes=1 << 30,
            max_outer_work_units=50_000_000,
            max_audit_step_units=512,
            max_repair_step_units=1024,
            max_object_bytes=65536,
            max_tape_bytes=1 << 20,
        )

    @staticmethod
    def _service_implementation_v2(source: Path, paths: tuple[str, ...]) -> str:
        """Bind capacity semantics, not unrelated shared-store methods or line numbers."""
        tree = ast.parse((source / "challenge/store.py").read_text())
        owned = {
            "_ServiceCapacityV1",
            "_service_profile_v2",
            "_service_implementation_v2",
            "bootstrap_service_capacity_v2",
            "_service_admission_v2",
            "require_roster_v2",
            "_service_boundary_v2",
            "_service_bytes_v2",
            "_service_charge_v2",
            "_backend_v2",
            "open_round_v2",
            "_assignment_v2",
            "round_view_v2",
            "training_v2",
            "verified_inputs_v2",
            "aggregate_v2",
            "_capacity_outer_v2",
            "_prepare_capacity_genesis_v2",
            "island_job_v2",
            "execute_audit_v2",
            "referee_v2",
            "rollback_v2",
            "rollback_preview_v2",
            "_rollback_context_v2",
            "schedule_audits_v2",
            "lease_v2",
            "complete_audit_v2",
            "finalize_v2",
        }
        nodes = [
            node
            for node in ast.walk(tree)
            if isinstance(node, (ast.ClassDef, ast.FunctionDef)) and node.name in owned
        ]
        code = {node.name: ast.dump(node, include_attributes=False) for node in nodes}
        if set(code) != owned:
            raise ChallengeError(409, "SERVICE_CAPACITY_IMPLEMENTATION")
        code.update({path: sha256_hex((source / path).read_bytes()) for path in paths})
        return sha256_hex(canonicalize(code))

    def _service_boundary_v2(
        self,
        run_id: str,
        w: int | None = None,
        *,
        heavy: bool = False,
        roster: list[Any] | None = None,
    ) -> dict[str, Any] | None:
        """Original complete roster first; accounting cannot grant heavy execution."""
        estimate = self.require_roster_v2(run_id, w, roster=roster)
        if heavy and estimate is not None:
            raise ChallengeError(
                503, "SERVICE_RUNTIME_NOT_ENFORCED: OS and codec preflight pending"
            )
        return estimate

    def _service_bytes_v2(
        self, run_id: str, w: int, raw: bytes, *, kind: Literal["object", "tape", "context"]
    ) -> None:
        """Actual bytes, never a declared length or decoded tensor estimate."""
        if self._service_boundary_v2(run_id, w) is None:
            return
        profile, _ = self._service_admission_v2(run_id)
        limit = profile.max_object_bytes if kind == "object" else profile.max_tape_bytes
        if len(raw) > limit:
            raise ChallengeError(413, f"SERVICE_{kind.upper()}_BYTES")

    def _service_charge_v2(
        self,
        run_id: str,
        w: int,
        operation: str,
        attempt: int,
        *,
        kind: Literal["audit", "outer", "repair"],
        units: int,
    ) -> bool:
        """Committed debit: True grants attempt, False never grants duplicate execution."""
        if (
            not operation
            or len(operation.encode()) > 256
            or type(attempt) is not int
            or attempt not in (1, 2)
            or type(units) is not int
            or units < 1
        ):
            raise ChallengeError(422, "SERVICE_CHARGE_BOUNDS")
        if self._db.in_transaction:
            raise ChallengeError(409, "SERVICE_CHARGE_REQUIRES_COMMITTED_TRANSACTION")
        with self._tx():
            preparation = operation == "genesis" and w == -1
            reservation = (
                self._record_v2(run_id, "genesis-resource", "run") if preparation else None
            )
            if (
                self._service_boundary_v2(
                    run_id,
                    None if preparation else w,
                    roster=reservation["roster"] if reservation else None,
                )
                is None
            ):
                return True
            if reservation is None:
                reservation = self._record_v2(run_id, "round-resource", str(w))
            if self._now(self._db) > reservation["deadline"]:
                raise ChallengeError(409, "SERVICE_RESOURCE_DEADLINE")
            profile, digest = self._service_admission_v2(run_id)
            charge: dict[str, JsonValue] = {
                "w": w,
                "operation": operation,
                "attempt": attempt,
                "kind": kind,
                "units": units,
                "profile_hash": digest,
            }
            identity = sha256_hex(
                canonicalize({"w": w, "operation": operation, "attempt": attempt})
            )
            row = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='service-charge' AND id=?",
                (run_id, identity),
            ).fetchone()
            if row is not None:
                if json.loads(row[0]) != charge:
                    raise ChallengeError(409, "SERVICE_CHARGE_CONFLICT")
                return False
            limit = {
                "audit": profile.max_audit_step_units,
                "outer": profile.max_outer_work_units,
                "repair": profile.max_repair_step_units,
            }[kind]
            spent = 0
            for row in self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='service-charge'", (run_id,)
            ).fetchall():
                prior = json.loads(row[0])
                if prior["kind"] == kind:
                    spent += prior["units"]
            if spent + units > limit:
                raise ChallengeError(409, "SERVICE_WORK_BUDGET_EXHAUSTED")
            self._put_record_v2(run_id, "service-charge", identity, charge)
            return True

    def bootstrap_service_capacity_v2(self, run_id: str, body: bytes, raw: bytes) -> str:
        """Trusted operator bootstrap: owner Receipt signs exact immutable profile."""
        from hypertrain.protocol.messages import Receipt

        with self._tx():
            try:
                profile = _ServiceCapacityV1.model_validate(
                    envelope_v2.load_json(body, max_bytes=16384)
                )
            except ValueError as exc:
                raise ChallengeError(422, "SERVICE_PROFILE_BODY") from exc
            if (
                profile != self._service_profile_v2(run_id)
                or canonicalize(profile.model_dump(mode="json")) != body
            ):
                raise ChallengeError(409, "SERVICE_PROFILE_BINDING")
            digest = sha256_hex(body)
            env = envelope_v2.parse_envelope(raw)
            if (env.run_id, env.type, env.signer) != (run_id, "Receipt", self.owner_hotkey):
                raise ChallengeError(403, "SERVICE_PROFILE_ROLE")
            existing = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? "
                "AND kind='service-admission' AND id='run'",
                (run_id,),
            ).fetchone()
            if existing is not None:
                stored = json.loads(existing[0])
                if stored["hash"] != digest or stored["receipt"] != env.model_dump(mode="json"):
                    raise ChallengeError(409, "SERVICE_PROFILE_IMMUTABLE")
                self._service_admission_v2(run_id)
                return digest
            if self.owner_hotkey is None:
                raise ChallengeError(409, "SERVICE_PROFILE_OWNER")
            model = self._authenticated_v2(run_id, raw, "Receipt", self.owner_hotkey)
            if not isinstance(model, Receipt) or (
                model.w,
                model.received_round,
                model.commit_hash,
            ) != (0, self._now(self._db), digest):
                raise ChallengeError(409, "SERVICE_PROFILE_RECEIPT")
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round' LIMIT 1", (run_id,)
            ).fetchone():
                raise ChallengeError(409, "SERVICE_PROFILE_BEFORE_OPEN")
            self.objects.put(body)
            self._put_record_v2(
                run_id,
                "service-admission",
                "run",
                {
                    "hash": digest,
                    "body": profile.model_dump(mode="json"),
                    "receipt": env.model_dump(mode="json"),
                    "accepted": model.received_round,
                },
            )
            return digest

    def _prepare_capacity_genesis_v2(
        self, run_id: str, raw: bytes, *, cancel: threading.Event | None = None
    ) -> dict[str, Any]:
        """Owner bootstrap admission plus signed original opening intent; never opens a round."""
        import os
        import sys

        from hypertrain.aggregator.capacity_worker import (
            GenesisRequest,
            _directory_fd,
            _write_at,
            collect_genesis,
        )
        from hypertrain.data.store import LocalFSStore
        from hypertrain.miner.island_launch import CapacityAttempt, run_capacity_argv
        from hypertrain.protocol.messages_v2 import RoundOpenV2

        with self._tx():
            manifest = self._run_v2(run_id)
            profile, digest = self._service_admission_v2(run_id)
            opening = self._authenticated_v2(
                run_id, raw, "RoundOpenV2", manifest.training.coord_pubkey
            )
            assert isinstance(opening, RoundOpenV2)
            now = self._now(self._db)
            if (
                opening.w != 0
                or opening.d_final <= now
                or not isinstance(self.objects, LocalFSStore)
            ):
                raise ChallengeError(409, "SERVICE_GENESIS_PRECONDITION")
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round'", (run_id,)
            ).fetchone():
                raise ChallengeError(409, "SERVICE_PROFILE_BEFORE_OPEN")
            estimate = self._service_boundary_v2(run_id, roster=opening.roster)
            assert estimate is not None
            admission = self._record_v2(run_id, "service-admission", "run")
            envelope = json.loads(
                self._db.execute("SELECT envelope FROM runs WHERE run_id=?", (run_id,)).fetchone()[
                    0
                ]
            )
            request = GenesisRequest(
                manifest=manifest,
                profile=profile.model_dump(mode="json"),
                manifest_envelope=envelope,
                receipt=admission["receipt"],
                opening=envelope_v2.parse_envelope(raw).model_dump(mode="json"),
                accepted=admission["accepted"],
            )
            request.validate_authority()
            deadline = manifest.training.beacon.genesis_time + (opening.d_final - 1) * 3
            if time.time() >= deadline or cancel is not None and cancel.is_set():
                raise ChallengeError(409, "SERVICE_GENESIS_DEADLINE_CANCEL")
            directory = self.state_dir / "capacity-genesis" / run_id / "1"
            with _directory_fd(directory, create=True) as root:
                _write_at(root, Path("request.json"), canonicalize(request.body()))
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='genesis-resource'", (run_id,)
            ).fetchone():
                raise ChallengeError(409, "SERVICE_GENESIS_ATTEMPT_RECORDED")
            self._put_record_v2(
                run_id,
                "genesis-resource",
                "run",
                {
                    **estimate,
                    "roster": opening.body()["roster"],
                    "deadline": opening.d_final,
                    "request_hash": request.digest(),
                },
            )
        identity = sha256_hex(
            canonicalize({"run_id": run_id, "w": -1, "operation": "genesis", "attempt": 1})
        )
        capacity = CapacityAttempt(
            identity,
            digest,
            self.state_dir / "capacity-runtime.lock",
            lambda: self._service_charge_v2(
                run_id, -1, "genesis", 1, kind="outer", units=int(estimate["outer_work_units"])
            ),
            cpu_quota="50%",
        )
        run_capacity_argv(
            [
                sys.executable,
                "-m",
                "hypertrain.aggregator.capacity_worker",
                str(directory.absolute()),
            ],
            directory / "runtime",
            deadline,
            capacity,
            env=dict(
                PATH=os.defpath,
                OMP_NUM_THREADS="1",
                MKL_NUM_THREADS="1",
                CUBLAS_WORKSPACE_CONFIG=":4096:8",
            ),
            cancel=cancel,
        )
        result, objects = collect_genesis(directory, request)
        with self._tx():
            self._service_admission_v2(run_id)
            if (
                self._now(self._db) >= opening.d_final
                or cancel is not None
                and cancel.is_set()
                or self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round'", (run_id,)
                ).fetchone()
                or self._record_v2(run_id, "genesis-resource", "run")["request_hash"]
                != request.digest()
            ):
                raise ChallengeError(409, "SERVICE_GENESIS_STALE")
            for key, value in objects.items():
                if self.objects.put(value) != key:
                    raise ChallengeError(422, "SERVICE_GENESIS_OBJECT_HASH")
            record: dict[str, JsonValue] = {
                "profile_hash": digest,
                "request_hash": request.digest(),
                "result": result.body(),
            }
            self._put_record_v2(run_id, "genesis-prepared", "run", record)
            return record

    def _service_admission_v2(self, run_id: str) -> tuple[_ServiceCapacityV1, str]:
        from hypertrain.aggregator.core import get_object

        record = self._record_v2(run_id, "service-admission", "run")
        raw = get_object(self.objects, record["hash"])
        profile = _ServiceCapacityV1.model_validate_json(raw)
        env = envelope_v2.parse_envelope(record["receipt"])
        if (
            profile != self._service_profile_v2(run_id)
            or canonicalize(profile.model_dump(mode="json")) != raw
            or record["body"] != profile.model_dump(mode="json")
            or not envelope_v2.verify_envelope(record["receipt"])
            or (env.run_id, env.type, env.signer) != (run_id, "Receipt", self.owner_hotkey)
            or (env.body["w"], env.body["commit_hash"], env.body["received_round"])
            != (0, record["hash"], record["accepted"])
            or record["accepted"] > env.exp_drand
        ):
            raise ChallengeError(409, "SERVICE_PROFILE_AUTHORITY")
        return profile, record["hash"]

    def require_roster_v2(
        self, run_id: str, w: int | None = None, roster: list[Any] | None = None
    ) -> dict[str, Any] | None:
        """Complete roster guard before storage, assignment or synchronous arithmetic."""
        if roster is None:
            assert w is not None
            record = self._record_v2(run_id, "round", str(w))
            roster = record["body"]["roster"]
        n = len(roster)
        if n > 32:
            raise ChallengeError(409, "ROSTER_LIMIT_32")
        admitted = self._db.execute(
            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='service-admission' AND id='run'",
            (run_id,),
        ).fetchone()
        if n <= 16 and not admitted:
            return None
        if not admitted:
            raise ChallengeError(409, "ROSTER_LIMIT_16: pause complete roster until qualification")
        profile, digest = self._service_admission_v2(run_id)
        from hypertrain.trainer.config import TrainConfig
        from hypertrain.trainer.model import param_shapes

        manifest = self._run_v2(run_id)
        shapes = param_shapes(TrainConfig.from_manifest_v2(manifest).model)
        p, t = manifest.training.model.param_count, len(shapes)
        delta_bytes = 20 + sum(
            32
            + len(name.encode())
            + 8 * len(shape)
            + 4 * ((math.prod(shape) + 255) // 256)
            + math.prod(shape)
            for name, shape in shapes.items()
        )
        from hypertrain.aggregator.rollback_v2 import RecomputedWork
        from hypertrain.aggregator.tape_v2 import TapeInput

        entries = [e.model_dump(mode="json") if isinstance(e, BaseModel) else e for e in roster]
        h, key = "f" * 64, "x" * 50
        flag = {"a": key, "b": key, "overlap": "f" * 8, "cosine": "f" * 8}
        tape_shell = {
            "body": {
                "v": "ht-tape-v2",
                "arithmetic": "flat-cclip-cap-v1",
                "run_id": h,
                "w": 2**53 - 1,
                "policy_hash": h,
                "policy_hashes": {name: h for name in PolicyHashes.model_fields},
                "predecessor_tape_hash": h,
                "prev_state": h,
                "prev_hashes": {name: h for name in ("theta", "u", "center")},
                "inputs": [
                    {name: (entry if name == "roster" else h) for name in TapeInput.model_fields}
                    for entry in entries
                ],
                "input_root": h,
                "excluded": [{"hotkey": key, "reason": "INFRASTRUCTURE", "evidence_hash": h}] * n,
                "allocation": {
                    "policy_hash": h,
                    "entries": [
                        {
                            "hotkey": key,
                            "admission_id": h,
                            "probation": False,
                            "weight_units": 1 << 22,
                            "commit_hash": h,
                            "delta_manifest_hash": h,
                        }
                    ]
                    * n,
                },
                "preclipped": [key] * n,
                "copy_suspicion": [flag] * (n * (n - 1) // 2),
                "out_state": h,
                "out_hashes": {name: h for name in ("theta", "u", "center")},
            },
            "signer": key,
            "sig": "f" * 128,
        }
        repair_shell = {
            "body": {
                "v": "ht-rollback-replay/1",
                "purpose": "model-repair-no-reward",
                **{
                    name: h
                    for name in (
                        "source_tape_hash",
                        "qualification_hash",
                        "reference_hash",
                        "layout_hash",
                    )
                },
                "backend": "cuda",
                "arithmetic": tape_shell["body"],
                "recomputed": [
                    {
                        name: (
                            key
                            if name == "hotkey"
                            else (
                                [2**53 - 1] * 2
                                if name == "sample_ids"
                                else (2**53 - 1 if name == "global_step0" else h)
                            )
                        )
                        for name in RecomputedWork.model_fields
                    }
                ]
                * n,
            },
            "signer": key,
            "sig": "f" * 128,
        }
        tape_bytes = max(len(canonicalize(tape_shell)), len(canonicalize(repair_shell)))
        work = 2 * (n * (n - 1) // 2 * p + n * p * (p - 1).bit_length() + 2 * n * p)
        memory_estimate = (68 << 20) + (32 * n + 256) * p + 4096 * n * t
        if (
            delta_bytes > min(profile.max_object_bytes, 2 << 30)
            or 16 * p + 16384 > profile.max_object_bytes
            or tape_bytes > profile.max_tape_bytes
            or work > profile.max_outer_work_units
            or memory_estimate > profile.memory_reservation_bytes
        ):
            raise ChallengeError(409, "SERVICE_RESOURCE_ESTIMATE")
        estimate = {
            "profile_hash": digest,
            "roster_hash": sha256_hex(canonicalize(entries)),
            "count": n,
            "delta_bytes": delta_bytes,
            "tape_bytes_bound": tape_bytes,
            "outer_work_units": work,
            "memory_estimate_bytes": memory_estimate,
            "audit_steps": 8 * n,
            "repair_steps": 16 * n,
        }
        if w is not None:
            reservation = self._record_v2(run_id, "round-resource", str(w))
            if any(reservation.get(name) != value for name, value in estimate.items()):
                raise ChallengeError(409, "SERVICE_ROUND_RESOURCE_BINDING")
        return estimate

    def open_round_v2(self, run_id: str, raw: bytes) -> dict[str, Any]:
        from hypertrain.protocol.messages_v2 import RoundOpenV2, StartStateV2

        env = envelope_v2.parse_envelope(raw)
        body = RoundOpenV2.model_validate(env.body)
        resource = self._service_boundary_v2(run_id, roster=body.roster)
        if resource is not None:
            if not self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='genesis-prepared'", (run_id,)
            ).fetchone():
                self._service_boundary_v2(run_id, roster=body.roster, heavy=True)
            from hypertrain.aggregator.capacity_worker import (
                GenesisRequest,
                _directory_fd,
                _write_at,
                bounded_bytes,
                collect_genesis,
            )
            from hypertrain.challenge.finality_v2 import require_open
            from hypertrain.data.store import LocalFSStore

            with self._tx():
                manifest = self._run_v2(run_id)
                signed = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(raw, self._now(self._db))
                assert isinstance(signed, RoundOpenV2)
                require_open(run_id, signed.w, self._history_v2(run_id))
                self._snapshot_current_v2(manifest)
                now = self._now(self._db)
                if (
                    signed.w != 0
                    or signed.d_open <= now
                    or signed.d_final - signed.d_audit < 2
                    or signed.d_final > signed.d_audit + 200
                    or len(signed.roster) * manifest.training.batch_samples()
                    > manifest.training.dataset.n_samples
                    or signed.registry_epoch != self._registry_v2(manifest).epoch
                    or signed.policy_hashes.body()
                    != {k: getattr(manifest.network, k) for k in signed.policy_hashes.body()}
                    or signed.roster_hash != resource["roster_hash"]
                ):
                    raise ChallengeError(422, "SERVICE_PREPARED_OPEN_PRECONDITION")
                reservation = self._record_v2(run_id, "genesis-resource", "run")
                prepared = self._record_v2(run_id, "genesis-prepared", "run")
                if (
                    any(reservation.get(k) != v for k, v in resource.items())
                    or reservation["roster"] != signed.body()["roster"]
                    or reservation["deadline"] < signed.d_final
                    or prepared["profile_hash"] != resource["profile_hash"]
                    or prepared["request_hash"] != reservation["request_hash"]
                ):
                    raise ChallengeError(409, "SERVICE_PREPARED_OPEN_BINDING")
                identity = sha256_hex(canonicalize({"w": -1, "operation": "genesis", "attempt": 1}))
                charge = self._record_v2(run_id, "service-charge", identity)
                if charge != {
                    "w": -1,
                    "operation": "genesis",
                    "attempt": 1,
                    "kind": "outer",
                    "units": resource["outer_work_units"],
                    "profile_hash": resource["profile_hash"],
                }:
                    raise ChallengeError(409, "SERVICE_PREPARED_OPEN_CHARGE")
                directory = self.state_dir / "capacity-genesis" / run_id / "1"
                request = GenesisRequest.model_validate_json(
                    bounded_bytes(directory / "request.json", 1 << 20)
                )
                if request.digest() != prepared["request_hash"] or request.manifest != manifest:
                    raise ChallengeError(409, "SERVICE_PREPARED_OPEN_REQUEST")
                original_intent = RoundOpenV2.model_validate(request.opening["body"])
                if original_intent.model_dump(
                    exclude={
                        "theta_hash",
                        "outer_state_hash",
                        "center_hash",
                        "start_state_index_hash",
                    }
                ) != signed.model_dump(
                    exclude={
                        "theta_hash",
                        "outer_state_hash",
                        "center_hash",
                        "start_state_index_hash",
                    }
                ):
                    raise ChallengeError(409, "SERVICE_PROTECTED_OPEN_INTENT")
                result, objects = collect_genesis(directory, request)
                if (
                    result.body() != prepared["result"]
                    or signed.theta_hash != result.starts[0].theta_hash
                    or signed.outer_state_hash != result.outer_hashes["outer_state_hash"]
                    or signed.center_hash != result.outer_hashes["center_hash"]
                    or signed.start_state_index_hash
                    != sha256_hex(canonicalize([s.body() for s in result.starts]))
                ):
                    raise ChallengeError(422, "SERVICE_PREPARED_OPEN_STATE")
                if not isinstance(self.objects, LocalFSStore):
                    raise ChallengeError(503, "SERVICE_PREPARED_OPEN_LOCAL_STORE")
                for digest, payload in objects.items():
                    if bounded_bytes(self.objects._path(digest), 65536) != payload:
                        raise ChallengeError(422, "SERVICE_PREPARED_OPEN_OBJECT")
                runtime = directory / "runtime"
                if not (runtime / "capacity-cleaned.json").exists():
                    self._service_boundary_v2(run_id, roster=signed.roster, heavy=True)
                # Existing manager custody grants this exact preparation only, never heavy work.
                import os
                import sys

                from hypertrain.aggregator.weighted_v2 import Candidate, allocate_weights
                from hypertrain.auditor.replay import AnchorCache

                attempt = envelope_v2.load_json(
                    bounded_bytes(runtime / "capacity-attempt.json", 65536)
                )
                kernel = envelope_v2.load_json(
                    bounded_bytes(runtime / "capacity-observed.json", 65536)
                )
                completed = envelope_v2.load_json(
                    bounded_bytes(runtime / "capacity-result.json", 65536)
                )
                cleaned = envelope_v2.load_json(
                    bounded_bytes(runtime / "capacity-cleaned.json", 65536)
                )
                execution = envelope_v2.load_json(
                    bounded_bytes(runtime / "capacity-exec.json", 65536)
                )
                if type(attempt["deadline"]) is not int:
                    raise ChallengeError(409, "SERVICE_PROTECTED_OPEN_RUNTIME")
                attempt_identity = sha256_hex(
                    canonicalize({"run_id": run_id, "w": -1, "operation": "genesis", "attempt": 1})
                )
                if (
                    attempt["identity"] != attempt_identity
                    or attempt["profile_hash"] != prepared["profile_hash"]
                    or attempt["deadline"]
                    != manifest.training.beacon.genesis_time + (reservation["deadline"] - 1) * 3
                    or execution["deadline"] != attempt["deadline"]
                    or attempt.get("cpu_quota") != "50%"
                    or execution.get("cpu_quota") != attempt["cpu_quota"]
                    or execution["argv"]
                    != [
                        sys.executable,
                        "-m",
                        "hypertrain.aggregator.capacity_worker",
                        str(directory.absolute()),
                    ]
                    or execution["env"]
                    != {
                        "PATH": os.defpath,
                        "OMP_NUM_THREADS": "1",
                        "MKL_NUM_THREADS": "1",
                        "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
                    }
                    or kernel["memory.max"] != str(1 << 30)
                    or kernel["memory.swap.max"] != "0"
                    or kernel["cpu.max"] != "50000 100000"
                    or kernel["memory.oom.group"] != "1"
                    or kernel["affinity"] != [0, 1, 2, 3]
                    or kernel["nice"] != 19
                    or not str(kernel["cgroup"]).endswith("/" + str(attempt["unit"]))
                    or completed["observed"] != kernel
                    or not isinstance(completed["status"], str)
                    or not {"Result=success", "ExecMainStatus=0", "oom 0", "oom_kill 0"}
                    <= set(completed["status"].splitlines())
                    or cleaned
                    != {"units": [attempt["unit"], attempt["timer"], attempt["timer_service"]]}
                    or b"CAPACITY_GENESIS_COMPLETE"
                    not in bounded_bytes(runtime / "capacity-stdout.log", 65536)
                    or time.time() >= attempt["deadline"]
                ):
                    raise ChallengeError(409, "SERVICE_PROTECTED_OPEN_RUNTIME")
                if len(signed.roster) != 4 or len(result.starts) != 4:
                    self._service_boundary_v2(run_id, roster=signed.roster, heavy=True)
                escrow, admission, _ = self._services(run_id)
                for entry in signed.roster:
                    status = admission.status(entry.hotkey, now=now)
                    record = status.record
                    units, receipt = escrow.locked(record.admission_id, record.coldkey)
                    if (
                        not status.eligible
                        or (entry.admission_id, entry.coldkey_group, entry.state, entry.q_i)
                        != (record.admission_id, record.coldkey, record.state, f32hex(1.0))
                        or entry.eligible_weight != 4194304
                        or units <= 0
                        or (status.funding.locked_units, status.funding.lock_receipt_hash)
                        != (units, receipt)
                    ):
                        raise ChallengeError(403, "SERVICE_PROTECTED_OPEN_ELIGIBILITY")
                policy_bytes = bounded_bytes(
                    self.objects._path(manifest.network.aggregation_policy_hash), 65536
                )
                if sha256_hex(policy_bytes) != manifest.network.aggregation_policy_hash:
                    raise ChallengeError(422, "SERVICE_PROTECTED_OPEN_POLICY")
                allocate_weights(
                    [Candidate(r, "0" * 64, "0" * 64) for r in signed.roster],
                    AggregationPolicyV2.model_validate_json(policy_bytes),
                )
                if (
                    self._record_v2(run_id, "genesis-prepared", "run") != prepared
                    or self._record_v2(run_id, "genesis-resource", "run") != reservation
                    or self._service_admission_v2(run_id)[1] != prepared["profile_hash"]
                    or self._db.execute(
                        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round'", (run_id,)
                    ).fetchone()
                ):
                    raise ChallengeError(409, "SERVICE_PROTECTED_OPEN_CAS")
                self._fresh_v2(manifest, f"open-prepared:{signed.w}")
                for start in result.starts:
                    anchor_directory = (
                        self.state_dir / "anchors-v2" / start.hotkey / start.parent_anchor_hash
                    )
                    with _directory_fd(anchor_directory, create=True) as anchor_fd:
                        _write_at(anchor_fd, Path("state"), objects[start.state_object_sha256])
                        _write_at(anchor_fd, Path("ef"), objects[start.ef_object_sha256])
                        _write_at(
                            anchor_fd,
                            Path("metadata"),
                            canonicalize(
                                {
                                    "run_id": run_id,
                                    "hotkey": start.hotkey,
                                    "w": -1,
                                    "layout_hash": AnchorCache.layout_hash(manifest),
                                    "proof_hash": start.anchor_verdict_hash,
                                    "anchor_hash": start.parent_anchor_hash,
                                    "backend": "genesis",
                                    "state_sha256": start.state_object_sha256,
                                    "ef_sha256": start.ef_object_sha256,
                                }
                            ),
                        )
                self._put_record_v2(run_id, "round", str(signed.w), env.model_dump(mode="json"))
                self._put_record_v2(run_id, "round-meta", str(signed.w), {"base": 0})
                self._put_record_v2(
                    run_id,
                    "round-resource",
                    str(signed.w),
                    {
                        **resource,
                        "deadline": signed.d_final,
                        "max_attempts": 2,
                        "max_running_jobs": 2,
                        "memory_reservation_bytes": 1 << 30,
                    },
                )
                for start in result.starts:
                    self._put_record_v2(run_id, "start", f"{signed.w}:{start.hotkey}", start.body())
                    self._put_record_v2(
                        run_id,
                        "anchor",
                        f"-1:{start.hotkey}",
                        {
                            "path": str(
                                self.state_dir
                                / "anchors-v2"
                                / start.hotkey
                                / start.parent_anchor_hash
                            ),
                            "anchor_hash": start.parent_anchor_hash,
                            "proof_hash": start.anchor_verdict_hash,
                            "state_root": result.state_root,
                            "ef_hash": start.ef_hash,
                            "hotkey": start.hotkey,
                            "w": -1,
                            "backend": "genesis",
                        },
                    )
                self._put_record_v2(
                    run_id,
                    "finality",
                    str(signed.w),
                    {
                        "run_id": run_id,
                        "w": signed.w,
                        "applied": False,
                        "disposition": "PENDING",
                        "unresolved_disputes": 0,
                        "unresolved_audits": len(signed.roster),
                        "rollback_complete": False,
                        "state_hash": signed.theta_hash,
                        "resolution_hash": None,
                    },
                )
                self._db.execute("UPDATE runs SET status='running' WHERE run_id=?", (run_id,))
                return env.model_dump(mode="json")
        self._service_boundary_v2(run_id, roster=body.roster, heavy=True)
        import hypertrain.trainer  # noqa: F401
        from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
        from hypertrain.challenge.finality_v2 import require_open
        from hypertrain.trainer.compress import state_hash
        from hypertrain.trainer.config import TrainConfig
        from hypertrain.trainer.model import init_params

        escrow, admission, _ = self._services(run_id)
        with self._tx():
            manifest = self._run_v2(run_id)
            self._authenticated_v2(run_id, raw, "RoundOpenV2", manifest.training.coord_pubkey)
            prior = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='round' AND id=?",
                (run_id, str(body.w)),
            ).fetchone()
            if prior:
                if body_digest(json.loads(prior[0])["body"]) != body.digest():
                    raise ChallengeError(409, "conflicting round opening")
                return json.loads(prior[0])
            history = self._history_v2(run_id)
            require_open(run_id, body.w, history)
            resource = self.require_roster_v2(run_id, roster=body.roster)
            if resource is not None:
                if (
                    len(body.roster) * manifest.training.batch_samples()
                    > manifest.training.dataset.n_samples
                ):
                    raise ChallengeError(422, "service roster assignment capacity")
                active_resources = []
                for row in self._db.execute(
                    "SELECT id,data FROM records_v2 WHERE run_id=? AND kind='round-resource'",
                    (run_id,),
                ).fetchall():
                    if not self._db.execute(
                        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='finalize' AND id=?",
                        (run_id, row[0]),
                    ).fetchone():
                        active_resources.append(json.loads(row[1]))
                if (
                    sum(r["count"] for r in active_resources) + resource["count"] > 32
                    or sum(r["audit_steps"] for r in active_resources) + resource["audit_steps"]
                    > 512
                    or sum(r["repair_steps"] for r in active_resources) + resource["repair_steps"]
                    > 1024
                ):
                    raise ChallengeError(409, "SERVICE_RESOURCE_POOL_EXHAUSTED")
                if body.d_final > body.d_audit + 200:
                    raise ChallengeError(422, "service resource absolute horizon")
                resource["deadline"] = body.d_final
                resource["max_attempts"] = 2
                resource["max_running_jobs"] = 2
                resource["memory_reservation_bytes"] = 1 << 30
                self._put_record_v2(run_id, "round-resource", str(body.w), resource)
            now = self._fresh_v2(manifest, f"open:{body.w}")
            if body.d_open <= now or body.d_final - body.d_audit < 2:
                raise ChallengeError(422, "round schedule has no future replay budget")
            if body.policy_hashes.body() != {
                k: getattr(manifest.network, k) for k in body.policy_hashes.body()
            }:
                raise ChallengeError(422, "round policies differ from pinned wrapper")
            registry = self._registry_v2(manifest)
            if body.registry_epoch != registry.epoch:
                raise ChallengeError(422, "registry epoch differs")
            starts = []
            cache = AnchorCache()
            cfg = TrainConfig.from_manifest_v2(manifest)
            theta = init_params(cfg.model)
            if body.w:
                previous = self._record_v2(run_id, "applied", str(body.w - 1))
                from hypertrain.aggregator.core import load_state

                theta_np = load_state(self.objects, str(previous["out_state"])).theta
                import torch

                theta = {k: torch.from_numpy(v.copy()) for k, v in theta_np.items()}
            if body.theta_hash != state_hash(theta):
                raise ChallengeError(422, "public theta does not match authenticated predecessor")
            for entry in body.roster:
                status = admission.status(entry.hotkey, now=now)
                r = status.record
                if not status.eligible or (
                    entry.admission_id,
                    entry.coldkey_group,
                    entry.state,
                    entry.q_i,
                ) != (r.admission_id, r.coldkey, r.state, f32hex(1.0)):
                    raise ChallengeError(403, "roster lacks current funded admission authority")
                if entry.eligible_weight != 4194304:
                    raise ChallengeError(422, "live eligible ceiling must be Q/4")
                if body.w == 0:
                    anchor = cache.genesis(manifest, entry.hotkey, theta)
                else:
                    anchor = self._restore_anchor_v2(run_id, entry.hotkey, body.w - 1, cache)
                state_blob = pack_state(theta, anchor.state)
                ef_blob = pack_state(anchor.ef)
                start = StartStateV2(
                    run_id=run_id,
                    w=body.w,
                    hotkey=entry.hotkey,
                    theta_hash=state_hash(theta),
                    state_object_sha256=self.objects.put(state_blob),
                    opt_state_hash=optimizer_hash(anchor.state),
                    ef_object_sha256=self.objects.put(ef_blob),
                    ef_hash=state_hash(anchor.ef),
                    parent_anchor_hash=anchor.anchor_hash,
                    global_step0=anchor.state.step,
                    anchor_verdict_hash=anchor.proof_hash,
                )
                starts.append(start)
            if body.start_state_index_hash != sha256_hex(canonicalize([s.body() for s in starts])):
                raise ChallengeError(422, "start-state index differs from authenticated anchors")
            # Feasibility stays on authenticated opening, never join/status.
            from hypertrain.aggregator.weighted_v2 import Candidate, allocate_weights

            policy = AggregationPolicyV2.model_validate_json(
                self.objects.get(manifest.network.aggregation_policy_hash)
            )
            allocate_weights([Candidate(r, "0" * 64, "0" * 64) for r in body.roster], policy)
            base = 0
            if body.w:
                prev_round = self._record_v2(run_id, "round", str(body.w - 1))
                prev_base = int(str(self._record_v2(run_id, "round-meta", str(body.w - 1))["base"]))
                base = (
                    prev_base
                    + len(prev_round["body"]["roster"]) * manifest.training.batch_samples()
                )
                n = manifest.training.dataset.n_samples
                if (
                    base + len(body.roster) * manifest.training.batch_samples() - 1
                ) // n != base // n:
                    base = (base // n + 1) * n
            if (
                len(body.roster) * manifest.training.batch_samples()
                > manifest.training.dataset.n_samples
            ):
                raise ChallengeError(422, "complete roster exceeds dataset")
            self._put_record_v2(run_id, "round", str(body.w), env.model_dump(mode="json"))
            self._put_record_v2(run_id, "round-meta", str(body.w), {"base": base})
            for start in starts:
                self._put_record_v2(run_id, "start", f"{body.w}:{start.hotkey}", start.body())
                if body.w == 0:
                    anchor = cache.entries[(run_id, start.hotkey, -1, cache.layout_hash(manifest))]
                    self._persist_anchor_v2(manifest, anchor, cache)
            self._put_record_v2(
                run_id,
                "finality",
                str(body.w),
                {
                    "run_id": run_id,
                    "w": body.w,
                    "applied": False,
                    "disposition": "PENDING",
                    "unresolved_disputes": 0,
                    "unresolved_audits": len(body.roster),
                    "rollback_complete": False,
                    "state_hash": body.theta_hash,
                    "resolution_hash": None,
                },
            )
            self._db.execute("UPDATE runs SET status='running' WHERE run_id=?", (run_id,))
            return env.model_dump(mode="json")

    def _registry_v2(self, manifest: RunManifestV2) -> Any:
        from hypertrain.protocol.relay_messages import RelayRegistryV1

        current = self._db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='registry' AND id='current'",
            (manifest.run_id(),),
        ).fetchone()
        if current is not None:
            return RelayRegistryV1.model_validate(json.loads(current[0])["body"])
        return RelayRegistryV1.model_validate_json(
            self.objects.get(manifest.network.relay_registry_hash)
        )

    def rotate_registry_v2(self, run_id: str, raw: bytes) -> dict[str, Any]:
        from hypertrain.protocol import relay_envelope
        from hypertrain.protocol.relay_messages import RelayRegistryV1

        with self._tx():
            manifest = self._run_v2(run_id)
            old = self._registry_v2(manifest)
            candidate = relay_envelope.Intake(
                run_id, {"RelayRegistryV1": self._coord().ss58.__eq__}
            ).accept(raw, self._now(self._db))
            assert isinstance(candidate, RelayRegistryV1)
            if candidate.digest() == old.digest():
                return {"registry_hash": old.digest(), "epoch": old.epoch}
            if candidate.previous_registry_hash != old.digest() or candidate.epoch != old.epoch + 1:
                raise ChallengeError(409, "successor registry must preserve exact accepted lineage")
            now = self._now(self._db)
            for row in self._db.execute(
                "SELECT id,data FROM records_v2 WHERE run_id=? AND kind='grant'", (run_id,)
            ):
                grant = json.loads(row["data"])["body"]
                released = self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='relay-release' AND id=?",
                    (run_id, row["id"]),
                ).fetchone()
                if released is not None:
                    continue
                prior = next(s for s in old.specs if s.id == grant["relay_id"])
                successor = next((s for s in candidate.specs if s.id == prior.id), None)
                if successor is None or successor.https_url != prior.https_url:
                    raise ChallengeError(409, "unreleased custody requires original relay endpoint")
                for key in prior.pubkeys:
                    if key.valid_from_round <= now:
                        replacement = next(
                            (k for k in successor.pubkeys if k.key_id == key.key_id), None
                        )
                        if (
                            replacement is None
                            or replacement.pubkey != key.pubkey
                            or replacement.valid_from_round != key.valid_from_round
                            or replacement.valid_until_round < key.valid_until_round
                        ):
                            raise ChallengeError(409, "successor dropped an unreleased custody key")
            self.objects.put(canonicalize(candidate.body()))
            self._put_record_v2(run_id, "registry", old.digest(), old.body())
            self._put_record_v2(
                run_id,
                "registry",
                "current",
                relay_envelope.parse_envelope(raw).model_dump(mode="json"),
            )
            result = {"registry_hash": candidate.digest(), "epoch": candidate.epoch}
        self._notify_processes_v2()
        return result

    def relay_observer_v2(
        self, run_id: str, grant_hash: str, observer: str, raw: bytes | None = None
    ) -> dict[str, Any]:
        """Fresh signed challenge/response plus accepted reference bytes, not health flags."""
        from hypertrain.protocol.messages import Receipt
        from hypertrain.protocol.messages_v2 import WorkProof

        with self._tx():
            manifest = self._run_v2(run_id)
            now = self._fresh_v2(manifest, f"relay-observer:{grant_hash}:{observer}")
            grant = self._record_v2(run_id, "grant", grant_hash)["body"]
            if observer not in manifest.training.auditors or observer in {
                grant["hotkey"],
                self._coord().ss58,
            }:
                raise ChallengeError(403, "independent pinned observer required")
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='auditor-disabled' AND id=?",
                (run_id, observer),
            ).fetchone():
                raise ChallengeError(403, "disabled observer cannot establish health")
            reference = self._db.execute(
                "SELECT reference FROM admission_trial_results WHERE outcome='MATCH' "
                "AND reference IS NOT NULL ORDER BY epoch DESC LIMIT 1"
            ).fetchone()
            if reference is None:
                raise ChallengeError(409, "independent accepted reference unavailable")
            proof = WorkProof.model_validate_json(reference[0])
            for artifact in proof.artifact_refs:
                content = self.objects.get(artifact.sha256)
                if len(content) != artifact.size or sha256_hex(content) != artifact.sha256:
                    raise ChallengeError(409, "accepted reference bytes unhealthy")
            identity = f"{grant_hash}:{observer}"
            if raw is None:
                challenge = Receipt(
                    w=grant["w"], commit_hash=secrets.token_hex(32), received_round=now
                )
                signed = envelope_v2.seal(self._coord(), "Receipt", run_id, challenge, now + 10)
                self._put_record_v2(run_id, "relay-observer-challenge", identity, signed)
                result = signed
            else:
                accepted_challenge = self._record_v2(run_id, "relay-observer-challenge", identity)
                model = self._authenticated_v2(run_id, raw, "Receipt", observer)
                if (
                    model.model_dump(mode="json") != accepted_challenge["body"]
                    or now >= accepted_challenge["exp_drand"]
                ):
                    raise ChallengeError(409, "observer response differs from fresh challenge")
                result = {
                    "beacon": now,
                    "expires": now + 10,
                    "reference_hash": proof.digest(),
                    "response": envelope_v2.parse_envelope(raw).model_dump(mode="json"),
                }
                self._put_record_v2(run_id, "relay-observer-health", identity, result)
        self._notify_processes_v2()
        return result

    async def relay_control_v2(
        self, run_id: str, grant_hash: str, action: str, client: Any
    ) -> dict[str, Any]:
        from hypertrain.protocol import relay_envelope
        from hypertrain.protocol.relay_messages import (
            ChunkCustodyAck,
            CustodyRelease,
            RelayReceipt,
            RetentionExtension,
            RetentionExtensionAck,
            UploadGrant,
            extend_retention,
        )

        with self._tx():
            manifest = self._run_v2(run_id)
            self._snapshot_current_v2(manifest)
            grant = UploadGrant.model_validate(self._record_v2(run_id, "grant", grant_hash)["body"])
            accepted = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='relay-receipt' "
                "AND json_extract(data,'$.body.grant_hash')=?",
                (run_id, grant_hash),
            ).fetchone()
            if accepted is None:
                raise ChallengeError(409, "no accepted durable receipt custody to extend/release")
            original = relay_envelope.parse_envelope(json.loads(accepted[0]))
            receipt = RelayReceipt.model_validate(original.body)
            registry = self._registry_v2(manifest)
            spec = next(s for s in registry.specs if s.id == grant.relay_id)
            current = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='retention' AND id=?",
                (run_id, grant_hash),
            ).fetchone()
            lineage: dict[str, Any] = (
                json.loads(current[0])
                if current
                else {"hash": receipt.digest(), "seq": 0, "horizon": receipt.retain_until}
            )
            message: RetentionExtension | CustodyRelease
            now = self._now(self._db)
            if action == "extend" and now >= lineage["horizon"]:
                raise ChallengeError(409, "custody horizon already expired; rescue required")
            if action == "extend":
                unresolved = [
                    r[0]
                    for r in self._db.execute(
                        "SELECT id FROM disputes_v2 "
                        "WHERE json_extract(turn,'$.contest.w')=? "
                        "AND json_extract(turn,'$.resolution') IS NULL",
                        (grant.w,),
                    )
                ]
                horizon = max(
                    lineage["horizon"] + 100, now + manifest.training.verify.E_vest_rounds + 1440
                )
                message = RetentionExtension(
                    grant_hash=grant_hash,
                    custody_hashes=[receipt.digest()],
                    seq=lineage["seq"] + 1,
                    previous_retention_hash=lineage["hash"],
                    dispute_ids=unresolved,
                    issued_beacon=now,
                    retain_until=horizon,
                )
                route, type_name = f"/v1/uploads/{grant_hash}/retention", "RetentionExtension"
            elif action == "release":
                settled = self.relay_settlement_v2(run_id, grant_hash)
                if now < max(settled["vesting_beacon"], settled["closed_beacon"]):
                    raise ChallengeError(409, "vesting/dispute horizon not closed")
                grace = (
                    max(
                        settled["finality_beacon"],
                        settled["vesting_beacon"],
                        settled["closed_beacon"],
                    )
                    + 100
                )
                if now < grace:
                    raise ChallengeError(409, "relay settlement epoch grace has not elapsed")
                message = CustodyRelease(
                    grant_hash=grant_hash,
                    custody_hashes=[receipt.digest()],
                    retention_hash=lineage["hash"],
                    finality_hash=settled["finality_hash"],
                    closed_dispute_root=settled["closed_dispute_root"],
                    vesting_beacon=settled["vesting_beacon"],
                    release_beacon=now,
                )
                route, type_name = f"/v1/uploads/{grant_hash}/release", "CustodyRelease"
            else:
                raise ChallengeError(404, "unknown relay lifecycle operation")
            signed = relay_envelope.seal(
                self._coord(), type_name, run_id, message, max(now + 10, lineage["horizon"])
            )
            self._put_record_v2(run_id, "relay-control-request", message.digest(), signed)
        response = await client.post(spec.https_url + route, content=canonicalize(signed))
        response.raise_for_status()
        data = response.json()
        chunk_releases: list[JsonValue] = []
        if action == "release":
            accepted_custody = self._record_v2(run_id, "relay-custody", grant_hash)
            acknowledgments = [
                relay_envelope.parse_envelope(a) for a in accepted_custody["artifacts"]
            ]
            for ack_env in acknowledgments:
                if (
                    ack_env.run_id != run_id
                    or ack_env.signer != original.signer
                    or not relay_envelope.verify_envelope(ack_env.model_dump())
                ):
                    raise ChallengeError(403, "release chunk custody authority mismatch")
            chunks = [
                ChunkCustodyAck.model_validate(a.body)
                for a in acknowledgments
                if a.type == "ChunkCustodyAck"
            ]
            from hypertrain.protocol.relay_messages import UploadChunkManifest

            expected = UploadChunkManifest.model_validate(
                self._record_v2(run_id, "chunk-manifest", grant_hash)
            )
            if len(chunks) != len(expected.chunks) or any(
                (c.index, c.off, c.len, c.chunk_sha256) != (e.index, e.off, e.len, e.chunk_sha256)
                or c.grant_hash != grant_hash
                or c.key_id != receipt.key_id
                or c.retain_until < grant.retain_until
                for c, e in zip(sorted(chunks, key=lambda c: c.index), expected.chunks, strict=True)
            ):
                raise ChallengeError(409, "release requires exact original full chunk custody")
            assert isinstance(message, CustodyRelease)
            for chunk in chunks:
                release = message.model_copy(
                    update={
                        "custody_hashes": [chunk.digest()],
                        "retention_hash": chunk.digest(),
                    }
                )
                signed_chunk = relay_envelope.seal(
                    self._coord(),
                    "CustodyRelease",
                    run_id,
                    release,
                    max(now + 10, chunk.retain_until),
                )
                reply = await client.post(
                    spec.https_url + route, content=canonicalize(signed_chunk)
                )
                reply.raise_for_status()
                parsed = relay_envelope.parse_envelope(reply.json())
                if parsed.body != release.body() or parsed.signer != self._coord().ss58:
                    raise ChallengeError(
                        403, "chunk release response differs from signed authority"
                    )
                if not relay_envelope.verify_envelope(parsed.model_dump()):
                    raise ChallengeError(403, "chunk release signature invalid")
                chunk_releases.append({"request": signed_chunk, "response": reply.json()})
        with self._tx():
            latest = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='retention' AND id=?",
                (run_id, grant_hash),
            ).fetchone()
            if latest is not None and json.loads(latest[0])["hash"] != lineage["hash"]:
                raise ChallengeError(409, "retention changed during lifecycle request")
            self._snapshot_current_v2(manifest)
            if action == "extend":
                ack_env = relay_envelope.parse_envelope(data)
                if (
                    ack_env.type != "RetentionExtensionAck"
                    or ack_env.signer != original.signer
                    or ack_env.run_id != run_id
                    or not relay_envelope.verify_envelope(data)
                ):
                    raise ChallengeError(403, "retention acknowledgment authority mismatch")
                ack = RetentionExtensionAck.model_validate(ack_env.body)
                assert isinstance(message, RetentionExtension)
                effective = extend_retention(
                    message,
                    ack,
                    grant_hash=grant_hash,
                    custody_hashes=[receipt.digest()],
                    key_id=receipt.key_id,
                    previous_hash=lineage["hash"],
                    previous_seq=lineage["seq"],
                    previous_horizon=lineage["horizon"],
                )
                self._put_record_v2(
                    run_id,
                    "retention",
                    grant_hash,
                    {
                        "hash": message.digest(),
                        "seq": message.seq,
                        "horizon": effective,
                        "request": signed,
                        "ack": data,
                    },
                )
            else:
                release_env = relay_envelope.parse_envelope(data)
                if (
                    release_env.signer != self._coord().ss58
                    or release_env.run_id != run_id
                    or release_env.type != "CustodyRelease"
                    or release_env.body != message.body()
                    or not relay_envelope.verify_envelope(data)
                ):
                    raise ChallengeError(403, "release response differs from signed authority")
                if self.relay_settlement_v2(run_id, grant_hash) != settled:
                    raise ChallengeError(409, "settlement changed during release")
                self._put_record_v2(
                    run_id,
                    "relay-release",
                    grant_hash,
                    {"request": signed, "response": data, "chunks": chunk_releases},
                )
        self._notify_processes_v2()
        return data

    def relay_failure_v2(self, run_id: str, raw: bytes) -> dict[str, Any]:
        """Master checks original custody and two pinned observations; no miner slash."""
        from hypertrain.protocol import relay_envelope
        from hypertrain.protocol.relay_messages import (
            AcceptedUploadAck,
            ChunkCustodyAck,
            RelayFailureEvidence,
            RelayReceipt,
            UploadChunkManifest,
            UploadGrant,
            has_full_custody,
        )

        data = envelope_v2.load_json(raw)
        if set(data) != {"evidence", "artifacts", "observations"}:
            raise ChallengeError(422, "original signed failure artifacts and observers required")
        if (
            not isinstance(data["evidence"], dict)
            or not isinstance(data["artifacts"], list)
            or not isinstance(data["observations"], list)
        ):
            raise ChallengeError(422, "invalid failure envelope arrays")
        if any(not isinstance(a, dict) for a in [*data["artifacts"], *data["observations"]]):
            raise ChallengeError(422, "failure artifacts must be signed envelope objects")
        env = relay_envelope.parse_envelope(data["evidence"])
        artifacts = [relay_envelope.parse_envelope(canonicalize(a)) for a in data["artifacts"]]
        observers = [relay_envelope.parse_envelope(canonicalize(o)) for o in data["observations"]]
        with self._tx(notify=True):
            manifest = self._run_v2(run_id)
            self._snapshot_current_v2(manifest)
            evidence = RelayFailureEvidence.model_validate(env.body)
            grant = UploadGrant.model_validate(
                self._record_v2(run_id, "grant", evidence.grant_hash)["body"]
            )
            registry = self._registry_v2(manifest)
            spec = next(s for s in registry.specs if s.id == grant.relay_id)
            key = next((k for k in spec.pubkeys if k.key_id == evidence.key_id), None)
            if (
                key is None
                or env.type != "RelayFailureEvidence"
                or env.run_id != run_id
                or evidence.run_id != run_id
                or evidence.relay_id != grant.relay_id
                or env.signer != self._coord().ss58
                or (
                    not relay_envelope.verify_envelope(env.model_dump())
                    or self._now(self._db) >= env.exp_drand
                )
            ):
                raise ChallengeError(403, "failure authority/domain mismatch")
            if any(
                a.run_id != run_id or not relay_envelope.verify_envelope(a.model_dump())
                for a in artifacts
            ):
                raise ChallengeError(403, "unsigned or cross-run custody artifact")
            if evidence.evidence_hash != sha256_hex(
                canonicalize([a.model_dump(mode="json") for a in artifacts])
            ):
                raise ChallengeError(422, "failure artifact hash differs")
            authorized = {
                o.signer
                for o in observers
                if o.signer in manifest.training.auditors
                and o.run_id == run_id
                and o.type == "RelayFailureEvidence"
                and o.body == env.body
                and relay_envelope.verify_envelope(o.model_dump())
            }
            if evidence.result == "UNCONFIRMED_AVAILABILITY":
                self._put_record_v2(
                    run_id, "relay-failure", evidence.digest(), env.model_dump(mode="json")
                )
                self._put_record_v2(
                    run_id,
                    "relay-failure-artifacts",
                    evidence.digest(),
                    {"artifacts": data["artifacts"], "observations": data["observations"]},
                )
                return {"result": evidence.result, "disabled": False}
            if authorized != set(evidence.observer_ids) or len(authorized) != 2:
                raise ChallengeError(403, "failure requires two distinct pinned healthy observers")
            for observer, observed in zip(
                evidence.observer_ids, evidence.observed_beacons, strict=True
            ):
                health = self._record_v2(
                    run_id, "relay-observer-health", f"{grant.digest()}:{observer}"
                )
                if health["beacon"] != observed or self._now(self._db) >= health["expires"]:
                    raise ChallengeError(
                        403, "observer has no current authenticated health evidence"
                    )
                if any(
                    o.signer == observer and o.exp_drand <= self._now(self._db) for o in observers
                ):
                    raise ChallengeError(403, "observer signature expired")
            assert key is not None
            if key.pubkey in authorized:
                raise ChallengeError(403, "relay signing key cannot observe its own failure")
            custody = [a for a in artifacts if a.signer == key.pubkey]
            opportunity = next(
                (
                    AcceptedUploadAck.model_validate(a.body)
                    for a in custody
                    if a.type == "AcceptedUploadAck"
                    and AcceptedUploadAck.model_validate(a.body).digest()
                    == evidence.upload_ack_hash
                ),
                None,
            )
            if opportunity is None:
                raise ChallengeError(422, "exact signed custody opportunity missing")
            if not key.valid_from_round <= opportunity.accepted_beacon < key.valid_until_round:
                raise ChallengeError(403, "custody signed outside original key validity")
            if opportunity.key_id != evidence.key_id:
                raise ChallengeError(422, "failure key differs from original signed custody")
            chunks = [
                ChunkCustodyAck.model_validate(a.body)
                for a in custody
                if a.type == "ChunkCustodyAck"
                and ChunkCustodyAck.model_validate(a.body).digest() in evidence.chunk_ack_hashes
            ]
            receipt = next(
                (
                    RelayReceipt.model_validate(a.body)
                    for a in custody
                    if a.type == "RelayReceipt"
                    and RelayReceipt.model_validate(a.body).digest() == evidence.receipt_hash
                ),
                None,
            )
            chunk_manifest = UploadChunkManifest.model_validate(
                self._record_v2(run_id, "chunk-manifest", grant.digest())
            )
            assignment = self._record_v2(run_id, "relay-assignment", f"{grant.w}:{grant.hotkey}")[
                "body"
            ]
            if (
                evidence.assignment_hash != body_digest(assignment)
                or evidence.chunk_manifest_hash != grant.chunk_manifest_hash
            ):
                raise ChallengeError(422, "failure differs from accepted assignment/chunk scope")
            if evidence.result != "CONTRACTUAL_SERVICE_FAILURE" or evidence.request is not None:
                raise ChallengeError(
                    409, "failure mode requires independently staged retrieval payload evidence"
                )
            if not has_full_custody(grant, chunk_manifest, opportunity, chunks, receipt):
                raise ChallengeError(
                    422, "prefix/opportunity cannot establish full custody failure"
                )
            if set(evidence.chunk_ack_hashes) != {c.digest() for c in chunks}:
                raise ChallengeError(422, "failure includes unproved chunk custody")
            if min(evidence.observed_beacons) <= opportunity.service_deadline or max(
                evidence.observed_beacons
            ) > self._now(self._db):
                raise ChallengeError(422, "failure observed before signed service cutoff or future")
            horizon = (
                receipt.retain_until if receipt is not None else min(c.retain_until for c in chunks)
            )
            lineage = (
                receipt.digest()
                if receipt is not None
                else sha256_hex(canonicalize(sorted(c.digest() for c in chunks)))
            )
            record = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='retention' AND id=?",
                (run_id, grant.digest()),
            ).fetchone()
            if record is not None:
                lineage, horizon = json.loads(record[0])["hash"], json.loads(record[0])["horizon"]
            if evidence.retention_hash != lineage or max(evidence.observed_beacons) > horizon:
                raise ChallengeError(422, "failure outside acknowledged exact retention horizon")
            accepted = self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='master-acceptance' "
                "AND json_extract(data,'$.body.w')=? AND json_extract(data,'$.body.hotkey')=?",
                (run_id, grant.w, grant.hotkey),
            ).fetchone()
            if accepted:
                raise ChallengeError(
                    409, "completed master acceptance contradicts completion failure"
                )
            self._put_record_v2(
                run_id, "relay-failure", evidence.digest(), env.model_dump(mode="json")
            )
            self._put_record_v2(
                run_id,
                "relay-failure-artifacts",
                evidence.digest(),
                {"artifacts": data["artifacts"], "observations": data["observations"]},
            )
            self._put_record_v2(
                run_id,
                "relay-disabled",
                f"{grant.relay_id}:{grant.assignment_epoch}:{evidence.key_id}",
                {"key_id": evidence.key_id, "evidence_hash": evidence.digest()},
            )
            result = {"result": evidence.result, "disabled": True, "miner_penalty": 0}
        self._notify_processes_v2()
        return result

    async def relay_fallback_v2(
        self, run_id: str, failure_hash: str, client: Any
    ) -> dict[str, Any]:
        """Rescue exact signed bytes; fallback grants never renew the original cutoff."""
        from hypertrain.data.stream_store import spool
        from hypertrain.protocol import relay_envelope
        from hypertrain.protocol.relay_messages import (
            AcceptedUploadAck,
            ChunkCustodyAck,
            RelayAssignment,
            RelayFailureEvidence,
            RelayReceipt,
            RetrievalRequest,
            RetrievalResponse,
            UploadChunkManifest,
            UploadGrant,
            has_full_custody,
        )
        from hypertrain.relay.client import RelayClient

        with self._tx(notify=True):
            manifest = self._run_v2(run_id)
            self._snapshot_current_v2(manifest)
            failure = RelayFailureEvidence.model_validate(
                self._record_v2(run_id, "relay-failure", failure_hash)["body"]
            )
            grant_env = self._record_v2(run_id, "grant", failure.grant_hash)
            grant = UploadGrant.model_validate(grant_env["body"])
            assignment_env = self._record_v2(
                run_id, "relay-assignment", f"{grant.w}:{grant.hotkey}"
            )
            assignment = RelayAssignment.model_validate(assignment_env["body"])
            registry = self._registry_v2(manifest)
            now = self._now(self._db)
            choices = [
                s
                for s in registry.specs
                if s.id in assignment.fallback_ids
                and any(k.valid_from_round <= now < k.valid_until_round for k in s.pubkeys)
                and self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='relay-draining' AND id=?",
                    (run_id, s.id),
                ).fetchone()
                is None
                and self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? "
                    "AND kind='relay-disabled' AND id LIKE ?",
                    (run_id, s.id + ":%"),
                ).fetchone()
                is None
            ]
            if now < grant.exp_drand and choices:
                successor = UploadGrant.model_validate(
                    {
                        **grant.body(),
                        "relay_id": choices[0].id,
                        "nonce": sha256_hex(
                            canonicalize([grant.digest(), failure_hash, choices[0].id])
                        ),
                    }
                )
                signed = relay_envelope.seal(
                    self._coord(), "UploadGrant", run_id, successor, successor.exp_drand
                )
                self._put_record_v2(run_id, "grant", successor.digest(), signed)
                self._put_record_v2(
                    run_id,
                    "chunk-manifest",
                    successor.digest(),
                    self._record_v2(run_id, "chunk-manifest", grant.digest()),
                )
                result: dict[str, Any] = {
                    "grant": signed,
                    "assignment": assignment_env,
                    "rescued": False,
                }
                self._put_record_v2(run_id, "relay-fallback", failure_hash, result)
                return result
            if failure.result != "CONTRACTUAL_SERVICE_FAILURE":
                raise ChallengeError(409, "no timely fallback or authenticated full custody rescue")
            artifacts = self._record_v2(run_id, "relay-failure-artifacts", failure_hash)[
                "artifacts"
            ]
            original = [relay_envelope.parse_envelope(a) for a in artifacts]
            chunk_manifest = UploadChunkManifest.model_validate(
                self._record_v2(run_id, "chunk-manifest", grant.digest())
            )
            opportunity = next(
                AcceptedUploadAck.model_validate(a.body)
                for a in original
                if a.type == "AcceptedUploadAck" and body_digest(a.body) == failure.upload_ack_hash
            )
            scopes: list[tuple[str, int, list[str], str | None, str]] = []
            chunks = sorted(
                [
                    (ChunkCustodyAck.model_validate(a.body), a.signer)
                    for a in original
                    if a.type == "ChunkCustodyAck"
                    and ChunkCustodyAck.model_validate(a.body).digest() in failure.chunk_ack_hashes
                ],
                key=lambda item: item[0].index,
            )
            if len({c.index for c, _ in chunks}) != len(chunks) or any(
                c.index >= len(chunk_manifest.chunks)
                or (c.off, c.len, c.chunk_sha256)
                != (
                    chunk_manifest.chunks[c.index].off,
                    chunk_manifest.chunks[c.index].len,
                    chunk_manifest.chunks[c.index].chunk_sha256,
                )
                or c.grant_hash != grant.digest()
                or c.upload_ack_hash != opportunity.digest()
                or c.key_id != failure.key_id
                for c, _ in chunks
            ):
                raise ChallengeError(422, "contradictory original chunk rescue custody")
            if failure.receipt_hash is not None:
                receipts = [
                    a
                    for a in original
                    if a.type == "RelayReceipt" and body_digest(a.body) == failure.receipt_hash
                ]
                if len(receipts) != 1:
                    raise ChallengeError(422, "exact original complete rescue receipt required")
                receipt_env = receipts[0]
                receipt = RelayReceipt.model_validate(receipt_env.body)
                if not has_full_custody(grant, chunk_manifest, opportunity, [], receipt):
                    raise ChallengeError(422, "partial receipt cannot authorize complete rescue")
                scopes = [(grant.delta_hash, grant.size, [], receipt.digest(), receipt_env.signer)]
            else:
                if not has_full_custody(grant, chunk_manifest, opportunity, [c for c, _ in chunks]):
                    raise ChallengeError(422, "complete original chunk rescue coverage required")
                scopes = [
                    (c.chunk_sha256, c.len, [c.digest()], None, signer) for c, signer in chunks
                ]
            retention = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='retention' AND id=?",
                (run_id, grant.digest()),
            ).fetchone()
            requests = []
            for sha, size, chunk_hashes, receipt_hash, signer in scopes:
                lineage = chunk_hashes[0] if chunk_hashes else str(receipt_hash)
                horizon = grant.retain_until
                if retention:
                    record = json.loads(retention[0])
                    lineage, horizon = record["hash"], record["horizon"]
                if now >= horizon:
                    raise ChallengeError(409, "original rescue custody horizon expired")
                request = RetrievalRequest(
                    request_id=secrets.token_hex(32),
                    nonce=secrets.token_hex(32),
                    relay_id=grant.relay_id,
                    key_id=failure.key_id,
                    assignment_hash=assignment.digest(),
                    grant_hash=grant.digest(),
                    custody_ack_hashes=chunk_hashes,
                    receipt_hash=receipt_hash,
                    retention_hash=lineage,
                    object_or_chunk_hash=sha,
                    size=size,
                    requested_beacon=now,
                    deadline_beacon=min(now + 10, horizon),
                )
                signed_request = relay_envelope.parse_envelope(
                    relay_envelope.seal(
                        self._coord(), "RetrievalRequest", run_id, request, request.deadline_beacon
                    )
                )
                self._put_record_v2(
                    run_id, "retrieval", request.digest(), signed_request.model_dump(mode="json")
                )
                requests.append((signed_request, signer))
        relay = RelayClient(registry, client)
        recovered = bytearray()
        responses = []
        for request_raw, signer in requests:
            with spool() as payload:
                response = await relay.retrieval(request_raw, payload, relay_public=signer)
                if RetrievalResponse.model_validate(response.body).status != "SERVED":
                    raise ChallengeError(409, "original acknowledged bytes unavailable for rescue")
                payload.seek(0)
                recovered.extend(payload.read())
                responses.append((request_raw, response))
        with self._tx():
            self._snapshot_current_v2(manifest)
            if len(recovered) != grant.size or sha256_hex(bytes(recovered)) != grant.delta_hash:
                raise ChallengeError(422, "rescued original bytes differ from signed commit")
            self.objects.put(bytes(recovered))
            for request_raw, response in responses:
                self._put_record_v2(
                    run_id,
                    "retrieval-response",
                    body_digest(request_raw.body),
                    response.model_dump(mode="json"),
                )
            result = {
                "rescued": True,
                "delta_hash": grant.delta_hash,
                "grant": grant_env,
                "assignment": assignment_env,
                "master_acceptance": None,
            }
            self._put_record_v2(run_id, "relay-fallback", failure_hash, result)
        self._notify_processes_v2()
        return result

    def relay_drain_v2(self, run_id: str, relay_id: str) -> dict[str, Any]:
        """Remove from new assignments, retain original custody/endpoint authority."""
        with self._tx():
            manifest = self._run_v2(run_id)
            self._snapshot_current_v2(manifest)
            if not any(s.id == relay_id for s in self._registry_v2(manifest).specs):
                raise ChallengeError(404, "relay absent from accepted registry")
            result: dict[str, Any] = {"relay_id": relay_id, "draining_beacon": self._now(self._db)}
            self._put_record_v2(run_id, "relay-draining", relay_id, result)
        self._notify_processes_v2()
        return result

    def _history_v2(self, run_id: str) -> list[Any]:
        from hypertrain.challenge.finality_v2 import RoundStatus

        return [
            RoundStatus(**json.loads(r[0]))
            for r in self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='finality' "
                "ORDER BY CAST(id AS INTEGER)",
                (run_id,),
            )
        ]

    def _persist_anchor_v2(self, manifest: RunManifestV2, anchor: Any, cache: Any) -> None:
        from hypertrain.auditor.replay import tensor_root
        from hypertrain.trainer.compress import state_hash

        if anchor.backend not in ("genesis", self._backend_v2(manifest)):
            raise ChallengeError(409, "anchor backend differs from frozen run execution")
        path = cache.persist(self.state_dir / "anchors-v2" / anchor.hotkey, anchor)
        self._put_record_v2(
            manifest.run_id(),
            "anchor",
            f"{anchor.w}:{anchor.hotkey}",
            {
                "path": str(path),
                "anchor_hash": anchor.anchor_hash,
                "proof_hash": anchor.proof_hash,
                "state_root": tensor_root(anchor.theta, anchor.state),
                "ef_hash": state_hash(anchor.ef),
                "hotkey": anchor.hotkey,
                "w": anchor.w,
                "backend": anchor.backend,
            },
        )

    def _restore_anchor_v2(self, run_id: str, hotkey: str, w: int, cache: Any) -> Any:
        record = self._record_v2(run_id, "anchor", f"{w}:{hotkey}")
        if record["backend"] not in ("genesis", self._backend_v2(self._run_v2(run_id))):
            raise ChallengeError(409, "anchor backend differs from frozen run execution")
        return cache.restore(
            Path(str(record["path"])),
            self._run_v2(run_id),
            str(record["anchor_hash"]),
            str(record["proof_hash"]),
            str(record["state_root"]),
            str(record["ef_hash"]),
            expected_hotkey=hotkey,
            expected_round=w,
            expected_backend=str(record["backend"]),
        )

    def round_view_v2(self, run_id: str, w: int) -> dict[str, Any]:
        self._service_boundary_v2(run_id, w)
        from hypertrain.protocol.messages_v2 import RoundOpenV2

        with self._tx():
            manifest = self._run_v2(run_id)
            env = self._record_v2(run_id, "round", str(w))
            body = RoundOpenV2.model_validate(env["body"])
            self.require_roster_v2(run_id, roster=body.roster)
            self._snapshot_current_v2(manifest)
            if self._now(self._db) >= body.d_assign:
                self._fresh_v2(manifest, f"assignment:{w}", body.d_assign)
                assignment = assign_round(
                    bytes.fromhex(run_id),
                    w,
                    self._sig(self._db, body.d_assign),
                    n_samples=manifest.training.dataset.n_samples,
                    n_slots=len(body.roster),
                    batch=manifest.training.batch_samples(),
                    base_w=int(str(self._record_v2(run_id, "round-meta", str(w))["base"])),
                    unit=manifest.training.dataset.assign_unit or 1,
                )
                for e in body.roster:
                    samples = assignment.slices[e.slot]
                    self._put_record_v2(
                        run_id,
                        "assignment",
                        f"{w}:{e.hotkey}",
                        {
                            "hotkey": e.hotkey,
                            "slot": e.slot,
                            "samples": list(samples),
                            "assignment_hash": assignment_hash(run_id, w, e.slot, samples),
                        },
                    )
            assignments = [
                json.loads(r[0])
                for r in self._db.execute(
                    "SELECT data FROM records_v2 WHERE run_id=? "
                    "AND kind='assignment' AND id LIKE ?",
                    (run_id, f"{w}:%"),
                )
            ]
            return {
                "round_open": env,
                "assignment": assignments,
                "now_round": self._now(self._db),
                "starts": [
                    self._record_v2(run_id, "start", f"{w}:{e.hotkey}") for e in body.roster
                ],
            }

    def training_v2(self, run_id: str, action: str, raw: bytes) -> dict[str, Any]:
        from hypertrain.protocol.messages_v2 import AcceptV2, CommitV2, DeltaManifestV2, RoundOpenV2

        env = envelope_v2.parse_envelope(raw)
        expected = {"accept": "AcceptV2", "commit": "CommitV2", "delta": "DeltaManifestV2"}[action]
        with self._tx():
            manifest = self._run_v2(run_id)
            w = int(str(env.body["w"]))
            self.require_roster_v2(run_id, w)
            self._service_bytes_v2(run_id, w, raw, kind="object")
            self._owner_v2(run_id, env.signer)
            model = self._authenticated_v2(run_id, raw, expected, env.signer)
            body = RoundOpenV2.model_validate(self._record_v2(run_id, "round", str(w))["body"])
            assignment = self._record_v2(run_id, "assignment", f"{w}:{env.signer}")
            now = self._fresh_v2(manifest, f"{action}:{w}:{self._now(self._db)}")
            reservation = _dumps(list(envelope_v2.replay_key(expected, run_id, env.signer, model)))
            old = self._db.execute(
                "SELECT * FROM accepted_v2 WHERE key=?", (reservation,)
            ).fetchone()
            digest = body_digest(env.body)
            if old:
                if old["digest"] != digest:
                    raise ChallengeError(409, "conflicting semantic replay")
                return json.loads(old["receipt"])
            if action in ("accept", "commit") and not body.d_assign <= now < body.d_commit:
                raise ChallengeError(409, "commit window closed")
            if action == "delta" and now >= body.d_upload:
                raise ChallengeError(409, "upload window closed")
            if action == "accept":
                assert isinstance(model, AcceptV2)
                model.validate_manifest(manifest)
                if model.assignment_hash != assignment["assignment_hash"]:
                    raise ChallengeError(422, "assignment differs")
                _, admission, _ = self._services(run_id)
                status = admission.status(env.signer, now=now)
                row = self._db.execute(
                    "SELECT screen_json FROM admissions_v2 WHERE hotkey=?", (env.signer,)
                ).fetchone()
                if (
                    not status.eligible
                    or model.work_screen_hash != WorkScreenV2.model_validate_json(row[0]).digest()
                ):
                    raise ChallengeError(403, "screen or funded admission stale")
            elif action == "commit":
                assert isinstance(model, CommitV2)
                self._record_v2(run_id, "accept", f"{w}:{env.signer}")
                model.validate_assignment(manifest, len(self._assignment_v2(run_id, w, env.signer)))
                start = self._record_v2(run_id, "start", f"{w}:{env.signer}")
                if model.ef_in_hash != start["ef_hash"]:
                    raise ChallengeError(422, "commit EF differs from authenticated anchor")
            else:
                assert isinstance(model, DeltaManifestV2)
                commit = CommitV2.model_validate(
                    self._record_v2(run_id, "commit", f"{w}:{env.signer}")["body"]
                )
                acceptance = self._record_v2(
                    run_id, "master-acceptance", model.master_acceptance_hash
                )
                if (model.delta_hash, model.size, acceptance["body"]["delta_hash"]) != (
                    commit.delta_hash,
                    commit.delta_bytes,
                    commit.delta_hash,
                ):
                    raise ChallengeError(422, "delta/master acceptance differs from commit")
                resource = self._service_boundary_v2(run_id, w)
                if resource is not None:
                    from hypertrain.data.store import LocalFSStore

                    profile, _ = self._service_admission_v2(run_id)
                    if model.size > profile.max_object_bytes:
                        raise ChallengeError(413, "SERVICE_OBJECT_BYTES")
                    if not isinstance(self.objects, LocalFSStore):
                        raise ChallengeError(503, "SERVICE_BOUNDED_GET_REQUIRED")
                    with self.objects._path(model.delta_hash).open("rb") as source:
                        payload = source.read(model.size + 1)
                else:
                    payload = self.objects.get(model.delta_hash)
                self._service_bytes_v2(run_id, w, payload, kind="object")
                if (
                    len(payload) != model.size
                    or (resource is not None and sha256_hex(payload) != model.delta_hash)
                    or any(
                        sha256_hex(payload[c.off : c.off + c.len]) != c.sha256 for c in model.chunks
                    )
                ):
                    raise ChallengeError(422, "original payload chunks mismatch")
                if resource is not None:
                    from hypertrain.trainer.compress import MAGIC, validate_payload
                    from hypertrain.trainer.config import TrainConfig
                    from hypertrain.trainer.model import param_shapes

                    config = TrainConfig.from_manifest_v2(manifest)
                    try:
                        codec = validate_payload(
                            payload,
                            param_shapes(config.model),
                            max_payload_bytes=profile.max_object_bytes,
                        )
                    except ValueError as exc:
                        raise ChallengeError(422, "SERVICE_DELTA_PAYLOAD") from exc
                    if codec != config.compress.codec or model.format != MAGIC[codec].decode():
                        raise ChallengeError(422, "SERVICE_DELTA_CODEC")
                self.objects.put(canonicalize(model.model_dump(mode="json")))
            receipt = envelope_v2.seal(
                self._coord(),
                "Receipt",
                run_id,
                {"w": w, "commit_hash": digest, "received_round": now},
                NEVER,
            )
            self._put_record_v2(run_id, action, f"{w}:{env.signer}", env.model_dump(mode="json"))
            self.objects.put(canonicalize(env.body))
            self._db.execute(
                "INSERT INTO accepted_v2 VALUES(?,?,?,?,?)",
                (reservation, digest, raw, now, _dumps(receipt)),
            )
            return receipt

    def leaves_v2(self, run_id: str, raw: bytes) -> dict[str, str]:
        """Signed immutable leaf object descriptor; leaves themselves stay content-addressed."""
        env = envelope_v2.parse_envelope(raw)
        from hypertrain.protocol.messages_v2 import CommitV2, WorkProof

        with self._tx():
            model = self._authenticated_v2(run_id, raw, "WorkProof", env.signer)
            assert isinstance(model, WorkProof)
            matches = self._db.execute(
                "SELECT id,data FROM records_v2 WHERE run_id=? AND kind='commit' AND id LIKE ?",
                (run_id, "%:" + env.signer),
            ).fetchall()
            found = [
                r
                for r in matches
                if body_digest(json.loads(r["data"])["body"]) == model.challenge_hash
            ]
            if len(found) != 1:
                raise ChallengeError(422, "leaf publication must bind accepted commit digest")
            commit = CommitV2.model_validate(json.loads(found[0]["data"])["body"])
            if (model.leaves_root, model.delta_hash) != (commit.leaves_root, commit.delta_hash):
                raise ChallengeError(422, "leaf root differs from commit")
            leaves = json.loads(self.objects.get(model.artifact_refs[-1].sha256))
            assert isinstance(leaves, list)
            parsed = [LeafPreimage.model_validate(p) for p in leaves]
            if (
                len(parsed) != commit.n_leaves
                or any(p.run_id != run_id or p.w != commit.w for p in parsed)
                or (
                    MerkleTree([bytes.fromhex(p.digest()) for p in parsed]).root.hex()
                    != commit.leaves_root
                )
            ):
                raise ChallengeError(422, "leaf object does not match accepted commit")
            identity = f"{commit.w}:{env.signer}"
            self._put_record_v2(
                run_id, "leaves", identity, {"object": model.artifact_refs[-1].sha256}
            )
            return {"leaves_root": commit.leaves_root}

    def schedule_audits_v2(self, run_id: str, w: int) -> list[str]:
        from hypertrain.protocol.messages_v2 import AuditChallengeV2, CommitV2, RoundOpenV2

        with self._tx():
            manifest = self._run_v2(run_id)
            resource = self._service_boundary_v2(run_id, w)
            if resource is not None:
                from hypertrain.aggregator.capacity_worker import bounded_bytes
                from hypertrain.data.store import LocalFSStore
                from hypertrain.protocol.messages_v2 import StartStateV2

                now = self._now(self._db)
                opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(self._record_v2(run_id, "round", str(w))), now)
                assert isinstance(opening, RoundOpenV2)
                if (
                    opening.w != w
                    or opening.roster_hash != resource["roster_hash"]
                    or opening.policy_hashes.body()
                    != {k: getattr(manifest.network, k) for k in opening.policy_hashes.body()}
                ):
                    raise ChallengeError(409, "SERVICE_SCHEDULE_ROUND_AUTHORITY")
                for entry in opening.roster:
                    identity = f"{w}:{entry.hotkey}"
                    signed_commit = self._record_v2(run_id, "commit", identity)
                    commit_model = envelope_v2.Intake(
                        run_id, {"CommitV2": entry.hotkey.__eq__}
                    ).accept(canonicalize(signed_commit), now)
                    assert isinstance(commit_model, CommitV2)
                    commit_model.validate_assignment(manifest, manifest.training.batch_samples())
                    start_model = StartStateV2.model_validate(
                        self._record_v2(run_id, "start", identity)
                    )
                    if (
                        commit_model.w != w
                        or commit_model.hotkey != entry.hotkey
                        or (start_model.run_id, start_model.w, start_model.hotkey)
                        != (run_id, w, entry.hotkey)
                    ):
                        raise ChallengeError(409, "SERVICE_SCHEDULE_INPUT_AUTHORITY")
                    key = _dumps(
                        list(envelope_v2.replay_key("CommitV2", run_id, entry.hotkey, commit_model))
                    )
                    accepted_commit = self._db.execute(
                        "SELECT digest,envelope,receipt FROM accepted_v2 WHERE key=?", (key,)
                    ).fetchone()
                    if (
                        accepted_commit is None
                        or accepted_commit["digest"] != body_digest(signed_commit["body"])
                        or envelope_v2.parse_envelope(accepted_commit["envelope"]).model_dump(
                            mode="json"
                        )
                        != signed_commit
                        or not envelope_v2.verify_envelope(json.loads(accepted_commit["receipt"]))
                    ):
                        raise ChallengeError(409, "SERVICE_SCHEDULE_ACCEPTED_COMMIT")
                    intake_receipt = envelope_v2.parse_envelope(
                        json.loads(accepted_commit["receipt"])
                    )
                    if (
                        intake_receipt.type != "Receipt"
                        or intake_receipt.run_id != run_id
                        or intake_receipt.signer != manifest.training.coord_pubkey
                        or intake_receipt.body["w"] != w
                        or intake_receipt.body["commit_hash"] != accepted_commit["digest"]
                    ):
                        raise ChallengeError(409, "SERVICE_SCHEDULE_ACCEPTANCE_RECEIPT")
                if not isinstance(self.objects, LocalFSStore):
                    raise ChallengeError(503, "SERVICE_SCHEDULE_LOCAL_STORE")
                policy_bytes = bounded_bytes(
                    self.objects._path(manifest.network.audit_policy_hash), 65536
                )
                if sha256_hex(policy_bytes) != manifest.network.audit_policy_hash:
                    raise ChallengeError(422, "SERVICE_SCHEDULE_POLICY_HASH")
                audit_policy = AuditPolicyV2.model_validate_json(policy_bytes)
                audit_policy.validate_training(manifest.training)
                self._service_boundary_v2(run_id, w, heavy=True)
            body = RoundOpenV2.model_validate(self._record_v2(run_id, "round", str(w))["body"])
            self.require_roster_v2(run_id, roster=body.roster)
            now = self._fresh_v2(manifest, f"audit:{w}", body.d_audit)
            if now < body.d_audit:
                raise ChallengeError(409, "audit beacon not ready")
            policy = AuditPolicyV2.model_validate_json(
                self.objects.get(manifest.network.audit_policy_hash)
            )
            active = self._db.execute(
                "SELECT COUNT(*) FROM audit_leases_v2 WHERE state IN ('queued','running')"
            ).fetchone()[0]
            missing = []
            for entry in body.roster:
                identity = f"{w}:{entry.hotkey}"
                commit_env = self._record_v2(run_id, "commit", identity)
                commit = CommitV2.model_validate(commit_env["body"])
                job_id = sha256_hex(f"ht-audit-job/2|{run_id}|{w}|{entry.hotkey}".encode())
                if not self._db.execute(
                    "SELECT 1 FROM audit_leases_v2 WHERE id=?", (job_id,)
                ).fetchone():
                    missing.append((entry, identity, job_id, commit))
            if active + len(missing) > policy.max_queued_jobs:
                raise ChallengeError(503, "audit capacity unavailable; no new reservation")
            absolute = min(body.d_final, body.d_audit + 200)
            if absolute <= now + 1:
                raise ChallengeError(503, "audit absolute horizon exhausted")
            result = []
            for entry, identity, job_id, _commit in missing:
                start = self._record_v2(run_id, "start", identity)
                challenge = AuditChallengeV2(
                    w=w,
                    target=entry.hotkey,
                    beacon_round=body.d_audit,
                    beacon_sig_sha256=sha256_hex(self._sig(self._db, body.d_audit)),
                    mode="full",
                    segments=[],
                    reasons=["random", "final"],
                    serve_deadline=absolute,
                    anchor_hash=sha256_hex(canonicalize(start)),
                    audit_mode="anchored-full",
                )
                signed = envelope_v2.seal(
                    self._coord(), "AuditChallengeV2", run_id, challenge, absolute
                )
                reservation = sha256_hex(f"ht-audit-reserve/2|{job_id}".encode())
                self._db.execute(
                    "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,challenge,created,absolute,"
                    "state,reservation,step_budget) "
                    "VALUES(?,?,?,?,?,?,?,'queued',?,?)",
                    (
                        job_id,
                        run_id,
                        w,
                        entry.hotkey,
                        _dumps(signed),
                        body.d_audit,
                        absolute,
                        reservation,
                        policy.max_steps_per_attempt,
                    ),
                )
                self._put_record_v2(
                    run_id,
                    "audit-reservation",
                    reservation,
                    {
                        "attempts": 2,
                        "steps_per_attempt": 2 * manifest.training.inner.H,
                        "absolute": absolute,
                        "auditors": list(manifest.training.auditors[:2]),
                    },
                )
                result.append(job_id)
            return result

    def lease_v2(self, run_id: str, raw: bytes) -> dict[str, Any] | None:
        from hypertrain.protocol.messages import Receipt
        from hypertrain.protocol.messages_v2 import ArtifactRef, AuditJobV2, StartStateV2

        env = envelope_v2.parse_envelope(raw)
        with self._tx():
            manifest = self._run_v2(run_id)
            if env.signer not in manifest.training.auditors:
                raise ChallengeError(403, "auditor not allowlisted")
            admitted = self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='service-admission' AND id='run'",
                (run_id,),
            ).fetchone()
            if admitted:
                from hypertrain.aggregator.capacity_worker import bounded_bytes
                from hypertrain.data.store import LocalFSStore
                from hypertrain.protocol.messages_v2 import AuditChallengeV2, RoundOpenV2

                self._service_admission_v2(run_id)
                now = self._now(self._db)
                envelope_v2.Intake(run_id, {"Receipt": env.signer.__eq__}).accept(raw, now)
                queued = self._db.execute(
                    "SELECT * FROM audit_leases_v2 WHERE run_id=? "
                    "AND state='queued' ORDER BY created,id LIMIT 1",
                    (run_id,),
                ).fetchone()
                if queued is None:
                    raise ChallengeError(
                        503, "SERVICE_RUNTIME_NOT_ENFORCED: no authenticated queued lease"
                    )
                resource = self._service_boundary_v2(run_id, queued["w"])
                assert resource is not None
                opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(self._record_v2(run_id, "round", str(queued["w"]))), now)
                assert isinstance(opening, RoundOpenV2)
                challenge = envelope_v2.Intake(
                    run_id, {"AuditChallengeV2": manifest.training.coord_pubkey.__eq__}
                ).accept(queued["challenge"].encode(), now)
                assert isinstance(challenge, AuditChallengeV2)
                original_start = StartStateV2.model_validate(
                    self._record_v2(run_id, "start", f"{queued['w']}:{queued['hotkey']}")
                )
                if (
                    opening.w != queued["w"]
                    or opening.roster_hash != resource["roster_hash"]
                    or queued["hotkey"] not in {r.hotkey for r in opening.roster}
                    or (challenge.w, challenge.target) != (queued["w"], queued["hotkey"])
                    or challenge.anchor_hash != original_start.digest()
                    or challenge.serve_deadline != queued["absolute"]
                    or now >= queued["absolute"]
                    or queued["attempts"] >= 2
                    or queued["step_budget"] != 2 * manifest.training.inner.H
                ):
                    raise ChallengeError(409, "SERVICE_LEASE_AUTHORITY")
                signed_commit = self._record_v2(
                    run_id, "commit", f"{queued['w']}:{queued['hotkey']}"
                )
                from hypertrain.protocol.messages_v2 import CommitV2

                commit_model = envelope_v2.Intake(
                    run_id, {"CommitV2": queued["hotkey"].__eq__}
                ).accept(canonicalize(signed_commit), now)
                assert isinstance(commit_model, CommitV2)
                commit_model.validate_assignment(manifest, manifest.training.batch_samples())
                if (
                    commit_model.w != queued["w"]
                    or commit_model.hotkey != queued["hotkey"]
                    or original_start.run_id != run_id
                    or original_start.w != queued["w"]
                    or original_start.hotkey != queued["hotkey"]
                ):
                    raise ChallengeError(409, "SERVICE_LEASE_COMMIT_AUTHORITY")
                if not isinstance(self.objects, LocalFSStore):
                    raise ChallengeError(503, "SERVICE_LEASE_LOCAL_STORE")
                leaves_key = str(
                    self._record_v2(run_id, "leaves", f"{queued['w']}:{queued['hotkey']}")["object"]
                )
                leaves_bytes = bounded_bytes(self.objects._path(leaves_key), 65536)
                if sha256_hex(leaves_bytes) != leaves_key:
                    raise ChallengeError(422, "SERVICE_LEASE_LEAVES_HASH")
                from hypertrain.protocol.messages import LeafPreimage

                leaves_json = json.loads(leaves_bytes)
                if (
                    not isinstance(leaves_json, list)
                    or len(leaves_json)
                    != manifest.training.inner.H // manifest.training.inner.J + 1
                ):
                    raise ChallengeError(422, "SERVICE_LEASE_LEAVES_SIZE")
                for leaf in leaves_json:
                    p = LeafPreimage.model_validate(leaf)
                    if p.run_id != run_id or p.w != queued["w"]:
                        raise ChallengeError(422, "SERVICE_LEASE_LEAVES_AUTHORITY")
                hashes = [
                    bytes.fromhex(LeafPreimage.model_validate(p).digest()) for p in leaves_json
                ]
                if MerkleTree(hashes).root.hex() != commit_model.leaves_root or [
                    p["t"] for p in leaves_json
                ] != list(range(0, manifest.training.inner.H + 1, manifest.training.inner.J)):
                    raise ChallengeError(422, "SERVICE_LEASE_LEAVES_COMMITMENT")
                self._service_boundary_v2(run_id, queued["w"], heavy=True)
            model = self._authenticated_v2(run_id, raw, "Receipt", env.signer)
            assert isinstance(model, Receipt)
            now = self._fresh_v2(manifest, f"lease:{self._now(self._db)}")
            self._expire_leases_v2(run_id, now)
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='auditor-disabled' AND id=?",
                (run_id, env.signer),
            ).fetchone():
                raise ChallengeError(403, "silent auditor disabled pending review")
            if self._db.execute(
                "SELECT 1 FROM audit_leases_v2 WHERE auditor=? AND state='running'", (env.signer,)
            ).fetchone():
                raise ChallengeError(409, "one live lease per auditor")
            if (
                self._db.execute(
                    "SELECT COUNT(*) FROM audit_leases_v2 WHERE state='running'"
                ).fetchone()[0]
                >= 2
            ):
                return None
            rows = self._db.execute(
                "SELECT * FROM audit_leases_v2 WHERE run_id=? "
                "AND state='queued' ORDER BY created,id",
                (run_id,),
            ).fetchall()
            row = next(
                (
                    r
                    for r in rows
                    if env.signer not in json.loads(r["tried"]) and env.signer != r["hotkey"]
                ),
                None,
            )
            if row is None:
                return None
            nonce = secrets.token_hex(32)
            expires = min(now + 100, row["absolute"])
            identity = f"{row['w']}:{row['hotkey']}"
            start = StartStateV2.model_validate(self._record_v2(run_id, "start", identity))
            commit_env = self._record_v2(run_id, "commit", identity)
            leaves_record = self._record_v2(run_id, "leaves", identity)
            leaves = json.loads(self.objects.get(str(leaves_record["object"])))
            ef = self.objects.get(start.ef_object_sha256)
            from hypertrain.auditor.replay import pack_state

            v0_hash = self.objects.put(pack_state({}))
            job = AuditJobV2(
                run_id=run_id,
                job_id=row["id"],
                auditor_id=env.signer,
                attempt=row["attempts"] + 1,
                lease_nonce=nonce,
                lease_expires=expires,
                absolute_deadline=row["absolute"],
                reservation_id=row["reservation"],
                replay_step_budget=row["step_budget"],
                anchor_age=0,
                manifest=manifest,
                challenge_envelope=json.loads(row["challenge"]),
                commit_envelope=commit_env,
                sample_ids=list(self._assignment_v2(run_id, row["w"], row["hotkey"])),
                start_state=start,
                preimages=leaves,
                ef_in=ArtifactRef(sha256=start.ef_object_sha256, size=len(ef)),
                v0=ArtifactRef(sha256=v0_hash, size=len(self.objects.get(v0_hash))),
                created_beacon=row["created"],
            )
            tried = json.loads(row["tried"]) + [env.signer]
            self._db.execute(
                "UPDATE audit_leases_v2 SET state='running',attempts=attempts+1,"
                "auditor=?,nonce=?,expires=?,tried=? WHERE id=?",
                (env.signer, nonce, expires, _dumps(tried), row["id"]),
            )
            self._put_record_v2(run_id, "audit-job", row["id"], job.body())
            return envelope_v2.seal(self._coord(), "AuditJobV2", run_id, job, expires)

    def _expire_leases_v2(self, run_id: str, now: int) -> None:
        rows = self._db.execute(
            "SELECT * FROM audit_leases_v2 WHERE run_id=? AND state IN ('running','queued')",
            (run_id,),
        ).fetchall()
        for row in rows:
            if row["state"] == "exhausted":
                continue
            if row["state"] == "running" and now >= row["expires"]:
                self._put_record_v2(
                    run_id,
                    "auditor-disabled",
                    row["auditor"],
                    {"job_id": row["id"], "reason": "LEASE_EXPIRED"},
                )
                state = "exhausted" if row["attempts"] >= 2 or now >= row["absolute"] else "queued"
                self._db.execute(
                    "UPDATE audit_leases_v2 SET state=?,result='AUDIT_INFRASTRUCTURE' WHERE id=?",
                    (state, row["id"]),
                )
                if row["id"] in self._lease_guards_v2:
                    self._lease_guards_v2[row["id"]].revoke()
            elif now >= row["absolute"]:
                self._db.execute(
                    "UPDATE audit_leases_v2 SET state='exhausted',"
                    "result='AUDIT_INFRASTRUCTURE' WHERE id=?",
                    (row["id"],),
                )
            current = self._db.execute(
                "SELECT state FROM audit_leases_v2 WHERE id=?", (row["id"],)
            ).fetchone()[0]
            if current == "exhausted":
                self._put_record_v2(
                    run_id,
                    "excluded",
                    f"{row['w']}:{row['hotkey']}",
                    {"reason": "INFRASTRUCTURE", "evidence_hash": sha256_hex(row["id"].encode())},
                )
                finality = self._record_v2(run_id, "finality", str(row["w"]))
                finality["disposition"] = "EXCLUDED"
                finality["resolution_hash"] = sha256_hex(row["id"].encode())
                finality["rollback_complete"] = not finality["applied"]
                finality["unresolved_audits"] = max(0, int(str(finality["unresolved_audits"])) - 1)
                self._put_record_v2(run_id, "finality", str(row["w"]), finality)

    def island_job_v2(self, run_id: str, w: int, hotkey: str) -> dict[str, Any]:
        from hypertrain.auditor.replay import pack_state, unpack_state
        from hypertrain.gpu_ops.journal import durable_write
        from hypertrain.protocol.messages_v2 import IslandJobV1, StartStateV2

        with self._tx():
            manifest = self._run_v2(run_id)
            resource = self._service_boundary_v2(run_id, w)
            if resource is not None:
                from hypertrain.aggregator.capacity_worker import (
                    GenesisRequest,
                    bounded_bytes,
                    collect_genesis,
                )
                from hypertrain.data.store import LocalFSStore
                from hypertrain.protocol.messages_v2 import RoundOpenV2
                from hypertrain.trainer.config import TrainConfig
                from hypertrain.trainer.model import param_shapes

                if not isinstance(self.objects, LocalFSStore):
                    raise ChallengeError(503, "SERVICE_JOB_LOCAL_STORE")
                opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(
                    canonicalize(self._record_v2(run_id, "round", str(w))), self._now(self._db)
                )
                assert isinstance(opening, RoundOpenV2)
                if (
                    opening.w != w
                    or w != 0
                    or opening.roster_hash != resource["roster_hash"]
                    or hotkey not in {r.hotkey for r in opening.roster}
                    or opening.policy_hashes.body()
                    != {k: getattr(manifest.network, k) for k in opening.policy_hashes.body()}
                ):
                    raise ChallengeError(409, "SERVICE_JOB_ROSTER_AUTHORITY")
                self._owner_v2(run_id, hotkey)
                start = StartStateV2.model_validate(
                    self._record_v2(run_id, "start", f"{w}:{hotkey}")
                )
                if (start.run_id, start.w, start.hotkey) != (run_id, w, hotkey):
                    raise ChallengeError(409, "SERVICE_JOB_START_AUTHORITY")
                prepared = self._record_v2(run_id, "genesis-prepared", "run")
                reservation = self._record_v2(run_id, "genesis-resource", "run")
                directory = self.state_dir / "capacity-genesis" / run_id / "1"
                request = GenesisRequest.model_validate_json(
                    bounded_bytes(directory / "request.json", 1 << 20)
                )
                if (
                    prepared["profile_hash"] != resource["profile_hash"]
                    or request.manifest != manifest
                    or request.digest() != prepared["request_hash"]
                    or reservation["request_hash"] != request.digest()
                    or reservation["roster"] != opening.body()["roster"]
                ):
                    raise ChallengeError(409, "SERVICE_JOB_PREPARED_BINDING")
                if (
                    any(reservation.get(k) != v for k, v in resource.items())
                    or reservation["deadline"] < opening.d_final
                ):
                    raise ChallengeError(409, "SERVICE_JOB_RESOURCE_BINDING")
                charge_id = sha256_hex(
                    canonicalize({"w": -1, "operation": "genesis", "attempt": 1})
                )
                charge = self._record_v2(run_id, "service-charge", charge_id)
                if charge != {
                    "w": -1,
                    "operation": "genesis",
                    "attempt": 1,
                    "kind": "outer",
                    "units": resource["outer_work_units"],
                    "profile_hash": resource["profile_hash"],
                }:
                    raise ChallengeError(409, "SERVICE_JOB_CHARGE_BINDING")
                result, prepared_objects = collect_genesis(directory, request)
                if (
                    result.body() != prepared["result"]
                    or start not in result.starts
                    or opening.theta_hash != start.theta_hash
                    or opening.outer_state_hash != result.outer_hashes["outer_state_hash"]
                    or opening.center_hash != result.outer_hashes["center_hash"]
                    or opening.start_state_index_hash
                    != sha256_hex(canonicalize([s.body() for s in result.starts]))
                ):
                    raise ChallengeError(409, "SERVICE_JOB_PREPARED_START")

                def read(key: str) -> bytes:
                    assert isinstance(self.objects, LocalFSStore)
                    raw = bounded_bytes(self.objects._path(key), 65536)
                    if sha256_hex(raw) != key:
                        raise ChallengeError(422, "SERVICE_JOB_OBJECT_HASH")
                    return raw

                state, ef = read(start.state_object_sha256), read(start.ef_object_sha256)
                if (
                    state != prepared_objects[start.state_object_sha256]
                    or ef != prepared_objects[start.ef_object_sha256]
                ):
                    raise ChallengeError(422, "SERVICE_JOB_PREPARED_OBJECT")
                shapes = param_shapes(TrainConfig.from_manifest_v2(manifest).model)
                # Header-only safetensors preflight: no tensor allocation or alternate decoder.
                for raw, optimizer in ((state, True), (ef, False)):
                    if len(raw) < 8:
                        raise ChallengeError(422, "SERVICE_JOB_STATE_HEADER")
                    header_size = struct.unpack_from("<Q", raw)[0]
                    if header_size > len(raw) - 8:
                        raise ChallengeError(422, "SERVICE_JOB_STATE_HEADER")
                    header = envelope_v2.load_json(raw[8 : 8 + header_size], max_bytes=65536)
                    expected = {
                        f"{part}/{name}": list(shape)
                        for part in (("theta", "m", "v") if optimizer else ("theta",))
                        for name, shape in shapes.items()
                    }
                    if optimizer:
                        expected["step"] = []
                    if not isinstance(header, dict) or set(header) != set(expected):
                        raise ChallengeError(422, "SERVICE_JOB_STATE_NAMES")
                    offsets: list[tuple[int, int]] = []
                    for name, shape in expected.items():
                        info = header[name]
                        if (
                            not isinstance(info, dict)
                            or set(info) != {"dtype", "shape", "data_offsets"}
                            or info["shape"] != shape
                            or not isinstance(info["shape"], list)
                            or any(type(dim) is not int for dim in info["shape"])
                            or info["dtype"] != ("I64" if name == "step" else "F32")
                        ):
                            raise ChallengeError(422, "SERVICE_JOB_STATE_SHAPE")
                        span = info["data_offsets"]
                        if (
                            not isinstance(span, list)
                            or len(span) != 2
                            or type(span[0]) is not int
                            or type(span[1]) is not int
                            or not 0 <= span[0] <= span[1] <= len(raw) - 8 - header_size
                            or span[1] - span[0] != (8 if name == "step" else 4 * math.prod(shape))
                        ):
                            raise ChallengeError(422, "SERVICE_JOB_STATE_OFFSETS")
                        offsets.append((span[0], span[1]))
                    cursor = 0
                    for begin, end in sorted(offsets):
                        if begin != cursor:
                            raise ChallengeError(422, "SERVICE_JOB_STATE_OFFSETS")
                        cursor = end
                    if cursor != len(raw) - 8 - header_size:
                        raise ChallengeError(422, "SERVICE_JOB_STATE_OFFSETS")
                samples = self._assignment_v2(run_id, w, hotkey)
                if len(samples) != manifest.training.batch_samples() or any(
                    not 0 <= i < manifest.training.dataset.n_samples for i in samples
                ):
                    raise ChallengeError(422, "SERVICE_JOB_ASSIGNMENT")
                dataset = self._record_v2(run_id, "dataset", "inputs")
                data = read(str(dataset["samples_hash"]))
                proofs = json.loads(read(str(dataset["proofs_hash"])))
                width = (manifest.training.model.seq_len + 1) * (
                    2 if manifest.training.dataset.sample_format.startswith("u16") else 4
                )
                if (
                    len(data) != width * manifest.training.dataset.n_samples
                    or not isinstance(proofs, list)
                    or len(proofs) != manifest.training.dataset.n_samples
                ):
                    raise ChallengeError(422, "SERVICE_JOB_DATASET")
                # Read/preflight only until all consumers qualify: never stage or publish a job.
                self._service_boundary_v2(run_id, w, heavy=True)
            self._owner_v2(run_id, hotkey)
            samples = self._assignment_v2(run_id, w, hotkey)
            start = StartStateV2.model_validate(self._record_v2(run_id, "start", f"{w}:{hotkey}"))
            body = self._record_v2(run_id, "round", str(w))["body"]
            directory = self.state_dir / "jobs-v2" / str(w) / hotkey
            directory.mkdir(parents=True, exist_ok=True)
            state = self.objects.get(start.state_object_sha256)
            if manifest.training.inner.state_policy != "carry":
                state = pack_state(unpack_state(state)[0])
            inputs = {
                "start_state": state,
                "ef_in": self.objects.get(start.ef_object_sha256),
                "v0": pack_state({}),
            }
            dataset = self._record_v2(run_id, "dataset", "inputs")
            raw = self.objects.get(str(dataset["samples_hash"]))
            proofs = json.loads(self.objects.get(str(dataset["proofs_hash"])))
            width = (manifest.training.model.seq_len + 1) * (
                2 if manifest.training.dataset.sample_format.startswith("u16") else 4
            )
            inputs.update(
                samples=b"".join(raw[i * width : (i + 1) * width] for i in samples),
                sample_proofs=canonicalize([proofs[i] for i in samples]),
            )
            hashes = {}
            for name, value in inputs.items():
                durable_write(directory / name, value)
                hashes[name] = self.objects.put(value)
            job = IslandJobV1(
                job_version=1,
                run_id=run_id,
                w=w,
                manifest=manifest,
                sample_ids=list(samples),
                global_step0=start.global_step0,
                start_state_sha256=hashes["start_state"],
                ef_in_sha256=hashes["ef_in"],
                v0_sha256=hashes["v0"],
                object_paths={k: k for k in inputs},
                deadline=manifest.training.beacon.genesis_time + (int(body["d_commit"]) - 1) * 3,
            )
            self._put_record_v2(run_id, "island-job", f"{w}:{hotkey}", job.body())
            return {"job": job.body(), "objects": hashes}

    def execute_audit_v2(self, run_id: str, job_id: str, raw: bytes) -> dict[str, Any]:
        """Independent service execution; auditor signature cannot manufacture MATCH."""
        import hypertrain.trainer  # noqa: F401
        from hypertrain.auditor.replay import AnchorCache
        from hypertrain.auditor.worker import LeaseGuard, execute_island_audit
        from hypertrain.gpu_ops.journal import durable_write
        from hypertrain.protocol.messages import ReplayEnv
        from hypertrain.protocol.messages_v2 import AuditJobV2

        env = envelope_v2.parse_envelope(raw)
        with self._tx():
            manifest = self._run_v2(run_id)
            backend = self._backend_v2(manifest)
            row = self._db.execute(
                "SELECT * FROM audit_leases_v2 WHERE id=? AND run_id=?", (job_id, run_id)
            ).fetchone()
            if row is None or row["state"] != "running" or row["auditor"] != env.signer:
                raise ChallengeError(403, "audit request lacks owned live lease")
            request = self._authenticated_v2(run_id, raw, "Receipt", row["auditor"])
            assert hasattr(request, "commit_hash")
            if request.commit_hash != row["nonce"]:
                raise ChallengeError(403, "audit lease nonce differs")
            now = self._fresh_v2(manifest, f"audit-execute:{job_id}:{self._now(self._db)}")
            job = AuditJobV2.model_validate(self._record_v2(run_id, "audit-job", job_id))
            job.validate_embedded(now)
            resource = self._service_boundary_v2(run_id, row["w"])
            if resource is not None:
                from hypertrain.aggregator.capacity_worker import bounded_bytes
                from hypertrain.data.store import LocalFSStore
                from hypertrain.protocol.messages import Receipt
                from hypertrain.protocol.messages_v2 import IslandJobV1, RoundOpenV2
                from hypertrain.trainer.config import TrainConfig
                from hypertrain.trainer.model import param_shapes

                if not isinstance(self.objects, LocalFSStore):
                    raise ChallengeError(503, "SERVICE_AUDIT_LOCAL_STORE")
                opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(self._record_v2(run_id, "round", str(row["w"]))), now)
                assert isinstance(opening, RoundOpenV2)
                if (
                    job.run_id != run_id
                    or job.manifest != manifest
                    or job.job_id != job_id
                    or job.auditor_id != row["auditor"]
                    or job.lease_nonce != row["nonce"]
                    or job.lease_expires != row["expires"]
                    or job.absolute_deadline != row["absolute"]
                    or job.attempt != row["attempts"]
                    or job.reservation_id != row["reservation"]
                    or job.replay_step_budget > row["step_budget"]
                    or job.start_state.w != row["w"]
                    or job.start_state.hotkey != row["hotkey"]
                    or now >= min(row["expires"], row["absolute"])
                    or opening.w != row["w"]
                    or opening.roster_hash != resource["roster_hash"]
                    or row["hotkey"] not in {entry.hotkey for entry in opening.roster}
                    or env.signer not in manifest.training.auditors
                    or opening.policy_hashes.body()
                    != {k: getattr(manifest.network, k) for k in opening.policy_hashes.body()}
                    or job.start_state.body()
                    != self._record_v2(run_id, "start", f"{row['w']}:{row['hotkey']}")
                ):
                    raise ChallengeError(409, "SERVICE_AUDIT_LEASE_BINDING")
                geometry = IslandJobV1.model_validate(
                    self._record_v2(run_id, "island-job", f"{row['w']}:{row['hotkey']}")
                )
                deadline = (
                    manifest.training.beacon.genesis_time
                    + (min(row["expires"], row["absolute"]) - 1) * 3
                )
                accepted = self._record_v2(run_id, "audit-geometry", f"{job_id}:{row['nonce']}")
                receipt = envelope_v2.Intake(
                    run_id, {"Receipt": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(accepted["receipt"]), now)
                assert isinstance(receipt, Receipt)
                if (
                    receipt.w != row["w"]
                    or receipt.commit_hash != accepted["descriptor_hash"]
                    or receipt.received_round > now
                ):
                    raise ChallengeError(409, "SERVICE_AUDIT_DESCRIPTOR_RECEIPT")

                def read_object(key: str, limit: int = 65536) -> bytes:
                    assert isinstance(self.objects, LocalFSStore)
                    data = bounded_bytes(self.objects._path(key), limit)
                    if sha256_hex(data) != key:
                        raise ChallengeError(422, "SERVICE_AUDIT_OBJECT_HASH")
                    return data

                descriptor_bytes = read_object(str(accepted["descriptor_hash"]), 1 << 20)
                preflight_descriptor = envelope_v2.load_json(descriptor_bytes, max_bytes=1 << 20)
                target = self.state_dir / "audit-geometry-v2" / job_id / row["nonce"]
                if (
                    not isinstance(preflight_descriptor, dict)
                    or canonicalize(preflight_descriptor) != descriptor_bytes
                    or preflight_descriptor
                    != {
                        "job": geometry.model_copy(update={"deadline": deadline}).body(),
                        "audit_job_hash": job.digest(),
                        "lease_nonce": row["nonce"],
                        "expires": row["expires"],
                        "absolute": row["absolute"],
                        "directory": str(target),
                    }
                    or geometry.manifest != manifest
                    or geometry.run_id != run_id
                    or geometry.w != row["w"]
                    or geometry.sample_ids != job.sample_ids
                    or geometry.start_state_sha256 != job.start_state.state_object_sha256
                    or geometry.ef_in_sha256 != job.ef_in.sha256
                    or geometry.v0_sha256 != job.v0.sha256
                    or set(geometry.object_paths)
                    != {"start_state", "ef_in", "v0", "samples", "sample_proofs"}
                    or any(
                        Path(p).is_absolute() or ".." in Path(p).parts
                        for p in geometry.object_paths.values()
                    )
                ):
                    raise ChallengeError(409, "SERVICE_AUDIT_DESCRIPTOR_BINDING")
                anchor_record = self._record_v2(run_id, "anchor", f"{row['w'] - 1}:{row['hotkey']}")
                if (
                    anchor_record["hotkey"] != row["hotkey"]
                    or anchor_record["w"] != row["w"] - 1
                    or anchor_record["anchor_hash"] != job.start_state.parent_anchor_hash
                    or anchor_record["proof_hash"] != job.start_state.anchor_verdict_hash
                    or anchor_record["ef_hash"] != job.start_state.ef_hash
                    or anchor_record["backend"] not in ("genesis", backend)
                ):
                    raise ChallengeError(409, "SERVICE_AUDIT_ANCHOR_BINDING")
                anchor_directory = self.state_dir / "anchors-v2" / row["hotkey"]
                path = Path(str(anchor_record["path"]))
                if not path.is_relative_to(anchor_directory):
                    raise ChallengeError(409, "SERVICE_AUDIT_ANCHOR_PATH")
                anchor_metadata = json.loads(bounded_bytes(path / "metadata", 65536))
                if not isinstance(anchor_metadata, dict) or anchor_metadata != {
                    "run_id": run_id,
                    "hotkey": row["hotkey"],
                    "w": row["w"] - 1,
                    "layout_hash": AnchorCache.layout_hash(manifest),
                    "proof_hash": anchor_record["proof_hash"],
                    "anchor_hash": anchor_record["anchor_hash"],
                    "backend": anchor_record["backend"],
                    "state_sha256": job.start_state.state_object_sha256,
                    "ef_sha256": job.ef_in.sha256,
                }:
                    raise ChallengeError(409, "SERVICE_AUDIT_ANCHOR_METADATA")
                inputs = {
                    "start_state": read_object(job.start_state.state_object_sha256),
                    "ef_in": read_object(job.ef_in.sha256),
                    "v0": read_object(job.v0.sha256),
                }
                if (
                    bounded_bytes(path / "state", 65536) != inputs["start_state"]
                    or bounded_bytes(path / "ef", 65536) != inputs["ef_in"]
                ):
                    raise ChallengeError(422, "SERVICE_AUDIT_ANCHOR_BYTES")
                if len(inputs["ef_in"]) != job.ef_in.size or len(inputs["v0"]) != job.v0.size:
                    raise ChallengeError(422, "SERVICE_AUDIT_REF_SIZE")
                shapes = param_shapes(TrainConfig.from_manifest_v2(manifest).model)
                for name, data in inputs.items():
                    if len(data) < 8:
                        raise ChallengeError(422, "SERVICE_AUDIT_STATE_HEADER")
                    size = struct.unpack_from("<Q", data)[0]
                    if size > len(data) - 8:
                        raise ChallengeError(422, "SERVICE_AUDIT_STATE_HEADER")
                    header = envelope_v2.load_json(data[8 : 8 + size], max_bytes=65536)
                    expected = {
                        f"{part}/{tensor}": list(shape)
                        for part in (("theta", "m", "v") if name == "start_state" else ("theta",))
                        for tensor, shape in shapes.items()
                    }
                    if name == "start_state":
                        expected["step"] = []
                    if name == "v0" and header == {}:
                        expected = {}
                    if not isinstance(header, dict) or set(header) != set(expected):
                        raise ChallengeError(422, "SERVICE_AUDIT_STATE_NAMES")
                    spans: list[tuple[int, int]] = []
                    for tensor, shape in expected.items():
                        info = header[tensor]
                        if (
                            not isinstance(info, dict)
                            or set(info) != {"shape", "dtype", "data_offsets"}
                            or info["shape"] != shape
                            or not isinstance(info["shape"], list)
                            or any(type(dim) is not int for dim in info["shape"])
                            or info["dtype"] != ("I64" if tensor == "step" else "F32")
                        ):
                            raise ChallengeError(422, "SERVICE_AUDIT_STATE_SHAPE")
                        span = info["data_offsets"]
                        if (
                            not isinstance(span, list)
                            or len(span) != 2
                            or type(span[0]) is not int
                            or type(span[1]) is not int
                            or not 0 <= span[0] <= span[1] <= len(data) - 8 - size
                            or span[1] - span[0]
                            != (8 if tensor == "step" else 4 * math.prod(shape))
                        ):
                            raise ChallengeError(422, "SERVICE_AUDIT_STATE_OFFSETS")
                        spans.append((span[0], span[1]))
                    cursor = 0
                    for begin, end in sorted(spans):
                        if begin != cursor:
                            raise ChallengeError(422, "SERVICE_AUDIT_STATE_OFFSETS")
                        cursor = end
                    if cursor != len(data) - 8 - size:
                        raise ChallengeError(422, "SERVICE_AUDIT_STATE_OFFSETS")
                dataset = self._record_v2(run_id, "dataset", "inputs")
                samples_data = read_object(str(dataset["samples_hash"]))
                proofs = json.loads(read_object(str(dataset["proofs_hash"])))
                width = (manifest.training.model.seq_len + 1) * (
                    2 if manifest.training.dataset.sample_format.startswith("u16") else 4
                )
                if (
                    len(samples_data) != width * manifest.training.dataset.n_samples
                    or not isinstance(proofs, list)
                    or len(proofs) != manifest.training.dataset.n_samples
                    or any(not 0 <= i < manifest.training.dataset.n_samples for i in job.sample_ids)
                ):
                    raise ChallengeError(422, "SERVICE_AUDIT_DATASET")
                inputs.update(
                    samples=b"".join(
                        samples_data[i * width : (i + 1) * width] for i in job.sample_ids
                    ),
                    sample_proofs=canonicalize([proofs[i] for i in job.sample_ids]),
                )
                miner = self.state_dir / "jobs-v2" / str(row["w"]) / row["hotkey"]
                for name, relative in geometry.object_paths.items():
                    if bounded_bytes(miner / relative, 65536) != inputs[name]:
                        raise ChallengeError(422, "SERVICE_AUDIT_GEOMETRY_BYTES")
                self._service_boundary_v2(run_id, row["w"], heavy=True)
            deadline = (
                manifest.training.beacon.genesis_time
                + (min(row["expires"], row["absolute"]) - 1) * 3
            )
            guard = LeaseGuard(row["expires"], row["absolute"], deadline)
            self._lease_guards_v2[job_id] = guard  # Subscribe before triggering execution.
            cache = AnchorCache()
            self._restore_anchor_v2(run_id, row["hotkey"], row["w"] - 1, cache)
            start = job.start_state
            directory = self.state_dir / "audit-v2" / job_id / row["nonce"]
            directory.mkdir(parents=True, exist_ok=True)
            inputs = {
                "start_state": self.objects.get(start.state_object_sha256),
                "ef_in": self.objects.get(start.ef_object_sha256),
                "v0": self.objects.get(job.v0.sha256),
            }
            dataset = self._record_v2(run_id, "dataset", "inputs")
            samples_raw = self.objects.get(str(dataset["samples_hash"]))
            proofs = json.loads(self.objects.get(str(dataset["proofs_hash"])))
            assert isinstance(proofs, list)
            width = (manifest.training.model.seq_len + 1) * (
                2 if manifest.training.dataset.sample_format.startswith("u16") else 4
            )
            inputs["samples"] = b"".join(
                samples_raw[i * width : (i + 1) * width] for i in job.sample_ids
            )
            inputs["sample_proofs"] = canonicalize([proofs[i] for i in job.sample_ids])
            for name, value in inputs.items():
                durable_write(directory / name, value)
        try:
            with guard:
                from hypertrain.protocol.messages_v2 import IslandJobV1

                with self._lock:
                    geometry_job = IslandJobV1.model_validate(
                        self._record_v2(
                            run_id, "island-job", f"{job.start_state.w}:{job.start_state.hotkey}"
                        )
                    ).model_copy(update={"deadline": deadline})
                miner_directory = (
                    self.state_dir / "jobs-v2" / str(job.start_state.w) / job.start_state.hotkey
                )
                geometry_directory = self.state_dir / "audit-geometry-v2" / job_id / row["nonce"]
                geometry_directory.mkdir(parents=True, exist_ok=True)
                for relative in geometry_job.object_paths.values():
                    durable_write(
                        geometry_directory / relative, (miner_directory / relative).read_bytes()
                    )
                descriptor: dict[str, JsonValue] = {
                    "job": geometry_job.body(),
                    "audit_job_hash": job.digest(),
                    "lease_nonce": row["nonce"],
                    "expires": row["expires"],
                    "absolute": row["absolute"],
                    "directory": str(geometry_directory),
                }
                descriptor_hash = self.objects.put(canonicalize(descriptor))
                with self._tx():
                    key = f"{job_id}:{row['nonce']}"
                    old = self._db.execute(
                        "SELECT data FROM records_v2 WHERE run_id=? "
                        "AND kind='audit-geometry' AND id=?",
                        (run_id, key),
                    ).fetchone()
                    if old is not None and json.loads(old[0])["descriptor_hash"] != descriptor_hash:
                        raise ChallengeError(409, "immutable audit geometry descriptor conflicts")
                    if old is None:
                        self._put_record_v2(
                            run_id,
                            "audit-geometry",
                            key,
                            {
                                "descriptor_hash": descriptor_hash,
                                "receipt": envelope_v2.seal(
                                    self._coord(),
                                    "Receipt",
                                    run_id,
                                    {
                                        "w": row["w"],
                                        "commit_hash": descriptor_hash,
                                        "received_round": now,
                                    },
                                    NEVER,
                                ),
                            },
                        )
                guard.check()
                outcome, artifacts = execute_island_audit(
                    job,
                    directory,
                    cache,
                    guard,
                    now_round=now,
                    deadline_unix=deadline,
                    backend=backend,
                    publication_job=geometry_job,
                    publication_directory=geometry_directory,
                    launch=(
                        self._role_launch(
                            "audit",
                            geometry_job,
                            geometry_directory,
                            {
                                "run_id": run_id,
                                "job_id": job_id,
                                "audit_job": job.body(),
                                "descriptor_hash": descriptor_hash,
                                "descriptor": descriptor,
                                "descriptor_receipt": self._record_v2(
                                    run_id, "audit-geometry", f"{job_id}:{row['nonce']}"
                                )["receipt"],
                                "hotkey": row["hotkey"],
                                "auditor": row["auditor"],
                            },
                        )
                        if self._role_launch is not None
                        else None
                    ),
                )
                from hypertrain.auditor.island_bisect import IslandParty

                if artifacts.directory != geometry_directory / "published":
                    raise ChallengeError(409, "independent audit publication directory differs")
                IslandParty.published(row["hotkey"], geometry_job, artifacts.directory)
                guard.check()
            with self._tx():
                current = self._db.execute(
                    "SELECT * FROM audit_leases_v2 WHERE id=?", (job_id,)
                ).fetchone()
                fresh = self._fresh_v2(manifest, f"audit-result:{job_id}:{self._now(self._db)}")
                if (
                    current["nonce"] != job.lease_nonce
                    or current["auditor"] != env.signer
                    or (
                        current["state"] != "running"
                        or fresh >= current["expires"]
                        or fresh >= current["absolute"]
                    )
                ):
                    raise ChallengeError(409, "late or revoked audit result cannot settle lease")
                verdict = ReplayVerdict(
                    challenge_hash=body_digest(
                        envelope_v2.parse_envelope(job.challenge_envelope).body
                    ),
                    result=outcome.result,
                    first_bad_leaf=outcome.first_bad_leaf,
                    recomputed_leaves_root=outcome.recomputed_leaves_root,
                    replay_env=ReplayEnv(
                        image_digest=manifest.training.reference_spec.image_digest,
                        driver=manifest.training.reference_spec.driver_allowlist[0],
                        gpu_uuid_sha256="0" * 64,
                        sm_count=max(1, manifest.training.reference_spec.sm_count),
                    ),
                )
                # Store independently executed evidence. Caller signs this exact verdict separately.
                identity = f"{job.start_state.w}:{job.start_state.hotkey}"
                self._put_record_v2(
                    run_id, "executed-verdict", job_id, verdict.model_dump(mode="json")
                )
                self._put_record_v2(
                    run_id, "audit-artifacts", job_id, {"directory": str(artifacts.directory)}
                )
                if outcome.result == "MATCH":
                    anchor = cache.entries[
                        (
                            run_id,
                            job.start_state.hotkey,
                            job.start_state.w,
                            cache.layout_hash(manifest),
                        )
                    ]
                    self._persist_anchor_v2(manifest, anchor, cache)
                return {
                    "verdict": verdict.model_dump(mode="json"),
                    "job_id": job_id,
                    "lease_nonce": job.lease_nonce,
                    "identity": identity,
                }
        finally:
            self._lease_guards_v2.pop(job_id, None)

    def complete_audit_v2(self, run_id: str, job_id: str, raw: bytes) -> dict[str, str]:
        from hypertrain.challenge.trust_v2 import ReplayEvidence
        from hypertrain.protocol.messages_v2 import AuditJobV2, CommitV2

        with self._tx():
            row = self._db.execute(
                "SELECT * FROM audit_leases_v2 WHERE id=? AND run_id=?", (job_id, run_id)
            ).fetchone()
            if row is None:
                raise ChallengeError(404, "unknown audit lease")
            manifest = self._run_v2(run_id)
            resource = self._service_boundary_v2(run_id, row["w"])
            if resource is not None:
                from hypertrain.aggregator.capacity_worker import bounded_bytes
                from hypertrain.data.store import LocalFSStore
                from hypertrain.protocol.messages_v2 import IslandJobV1, RoundOpenV2

                now = self._now(self._db)
                signed_result = envelope_v2.Intake(
                    run_id, {"ReplayVerdict": row["auditor"].__eq__}
                ).accept(raw, now)
                assert isinstance(signed_result, ReplayVerdict)
                opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(self._record_v2(run_id, "round", str(row["w"]))), now)
                assert isinstance(opening, RoundOpenV2)
                original_job = AuditJobV2.model_validate(
                    self._record_v2(run_id, "audit-job", job_id)
                )
                original_job.validate_embedded(now)
                original_challenge = envelope_v2.parse_envelope(original_job.challenge_envelope)
                original_verdict = self._record_v2(run_id, "executed-verdict", job_id)
                if (
                    opening.w != row["w"]
                    or opening.roster_hash != resource["roster_hash"]
                    or row["hotkey"] not in {r.hotkey for r in opening.roster}
                    or original_job.manifest != manifest
                    or original_job.run_id != run_id
                    or original_job.job_id != job_id
                    or original_job.auditor_id != row["auditor"]
                    or original_job.lease_nonce != row["nonce"]
                    or original_job.lease_expires != row["expires"]
                    or original_job.absolute_deadline != row["absolute"]
                    or original_job.start_state.hotkey != row["hotkey"]
                    or original_job.start_state.w != row["w"]
                    or row["state"] != "running"
                    or now >= min(row["expires"], row["absolute"])
                    or signed_result.model_dump(mode="json") != original_verdict
                    or signed_result.challenge_hash != body_digest(original_challenge.body)
                ):
                    raise ChallengeError(409, "SERVICE_COMPLETE_LEASE_RESULT")
                original_geometry = IslandJobV1.model_validate(
                    self._record_v2(run_id, "island-job", f"{row['w']}:{row['hotkey']}")
                )
                accepted_descriptor = self._record_v2(
                    run_id, "audit-geometry", f"{job_id}:{row['nonce']}"
                )
                descriptor_receipt = envelope_v2.parse_envelope(accepted_descriptor["receipt"])
                if (
                    descriptor_receipt.type != "Receipt"
                    or descriptor_receipt.run_id != run_id
                    or descriptor_receipt.signer != manifest.training.coord_pubkey
                    or descriptor_receipt.body["w"] != row["w"]
                    or descriptor_receipt.body["commit_hash"]
                    != accepted_descriptor["descriptor_hash"]
                    or not envelope_v2.verify_envelope(accepted_descriptor["receipt"])
                ):
                    raise ChallengeError(409, "SERVICE_COMPLETE_DESCRIPTOR_SIGNATURE")
                if not isinstance(self.objects, LocalFSStore):
                    raise ChallengeError(503, "SERVICE_COMPLETE_LOCAL_STORE")
                descriptor_data = bounded_bytes(
                    self.objects._path(str(accepted_descriptor["descriptor_hash"])), 1 << 20
                )
                output_directory = self.state_dir / "audit-geometry-v2" / job_id / row["nonce"]
                job_deadline = (
                    manifest.training.beacon.genesis_time
                    + (min(row["expires"], row["absolute"]) - 1) * 3
                )
                if sha256_hex(descriptor_data) != accepted_descriptor[
                    "descriptor_hash"
                ] or envelope_v2.load_json(descriptor_data, max_bytes=1 << 20) != {
                    "job": original_geometry.model_copy(update={"deadline": job_deadline}).body(),
                    "audit_job_hash": original_job.digest(),
                    "lease_nonce": row["nonce"],
                    "expires": row["expires"],
                    "absolute": row["absolute"],
                    "directory": str(output_directory),
                }:
                    raise ChallengeError(409, "SERVICE_COMPLETE_DESCRIPTOR_BINDING")
                # Metadata read only; IslandParty/tensors and settlement stay denied.
                bounded_bytes(output_directory / "published/rank-0/trace.json", 65536)
                self._service_boundary_v2(run_id, row["w"], heavy=True)
            verdict = self._authenticated_v2(run_id, raw, "ReplayVerdict", row["auditor"])
            assert isinstance(verdict, ReplayVerdict)
            expected = self._record_v2(run_id, "executed-verdict", job_id)
            if verdict.model_dump(mode="json") != expected:
                raise ChallengeError(422, "signed verdict differs from independent real replay")
            now = self._fresh_v2(self._run_v2(run_id), f"complete:{job_id}:{self._now(self._db)}")
            if row["state"] == "completed":
                if row["result"] != verdict.result:
                    raise ChallengeError(409, "audit replay conflicts")
                return {"result": row["result"]}
            if row["state"] != "running" or now >= row["expires"] or now >= row["absolute"]:
                raise ChallengeError(409, "audit result outside owned immutable lease")
            job = AuditJobV2.model_validate(self._record_v2(run_id, "audit-job", job_id))
            commit = CommitV2.model_validate(job.commit_envelope["body"])
            identity = f"{row['w']}:{row['hotkey']}"
            assignment = self._record_v2(run_id, "assignment", identity)
            evidence = ReplayEvidence(
                run_id,
                row["w"],
                row["hotkey"],
                str(assignment["assignment_hash"]),
                job.start_state.digest(),
                commit.leaves_root,
                commit.delta_hash,
                commit.final_theta_hash,
                commit.ef_in_hash,
                commit.ef_out_hash,
                verdict.digest() if hasattr(verdict, "digest") else body_digest(expected),
                verdict.result,
                "anchored-full",
            )
            self._put_record_v2(run_id, "replay", identity, asdict(evidence))
            self._put_record_v2(
                run_id, "verdict", identity, envelope_v2.parse_envelope(raw).model_dump(mode="json")
            )
            self._db.execute(
                "UPDATE audit_leases_v2 SET state='completed',result=? WHERE id=?",
                (verdict.result, job_id),
            )
            # Derived from independently executed published trace, never client geometry.
            from hypertrain.auditor.island_bisect import IslandParty
            from hypertrain.protocol.messages_v2 import IslandJobV1

            geometry_job = IslandJobV1.model_validate(
                self._record_v2(run_id, "island-job", identity)
            )
            accepted_geometry = self._record_v2(
                run_id, "audit-geometry", f"{job_id}:{row['nonce']}"
            )
            descriptor_hash = str(accepted_geometry["descriptor_hash"])
            descriptor_bytes = self.objects.get(descriptor_hash)
            descriptor = envelope_v2.load_json(descriptor_bytes)
            receipt = envelope_v2.parse_envelope(accepted_geometry["receipt"])
            deadline = (
                job.manifest.training.beacon.genesis_time
                + (min(row["expires"], row["absolute"]) - 1) * 3
            )
            geometry_directory = self.state_dir / "audit-geometry-v2" / job_id / row["nonce"]
            if (
                sha256_hex(descriptor_bytes) != descriptor_hash
                or receipt.type != "Receipt"
                or receipt.run_id != run_id
                or receipt.signer != self._coord().ss58
                or not envelope_v2.verify_envelope(accepted_geometry["receipt"])
                or receipt.body["commit_hash"] != descriptor_hash
                or receipt.body["w"] != row["w"]
                or descriptor
                != {
                    "job": geometry_job.model_copy(update={"deadline": deadline}).body(),
                    "audit_job_hash": job.digest(),
                    "lease_nonce": row["nonce"],
                    "expires": row["expires"],
                    "absolute": row["absolute"],
                    "directory": str(geometry_directory),
                }
            ):
                raise ChallengeError(409, "accepted audit geometry authority differs")
            geometry_job = IslandJobV1.model_validate(descriptor["job"])
            geometry_directory = geometry_directory / "published"
            if not (geometry_directory / "rank-0/trace.json").is_file():
                raise ChallengeError(
                    503, "qualified immutable same-layout trace geometry unavailable"
                )
            party = IslandParty.published(row["hotkey"], geometry_job, geometry_directory)
            layer_span = max(
                party.span("layer", (window,)) for window in range(1, party.H // party.J + 1)
            )
            op_span = max(
                party.span("op", (window, layer))
                for window in range(1, party.H // party.J + 1)
                for layer in range(party.span("layer", (window,)))
            )
            policy = DisputePolicyV2.model_validate_json(
                self.objects.get(job.manifest.network.dispute_policy_hash)
            )
            referee = next(
                (
                    h
                    for h in policy.referees
                    if h not in (row["hotkey"], row["auditor"], self._coord().ss58)
                ),
                None,
            )
            if referee is None:
                raise ChallengeError(503, "independent referee unavailable")
            admission = self._services(run_id)[1].store.record(row["hotkey"])
            contest = Contest(
                verdict_hash=body_digest(expected),
                challenge_hash=verdict.challenge_hash,
                w=row["w"],
                miner=row["hotkey"],
                auditor=row["auditor"],
                referee=referee,
                step_span=party.H // party.J,
                layer_span=layer_span,
                op_span=op_span,
                admission_id=admission.admission_id,
                coldkey=admission.coldkey,
                evidence_hash=body_digest(expected),
            )
            self._put_record_v2(run_id, "contest", contest.verdict_hash, contest.body())
            self._put_record_v2(
                run_id,
                "trace-geometry",
                contest.verdict_hash,
                {"job": geometry_job.body(), "directory": str(geometry_directory)},
            )
            finality = self._record_v2(run_id, "finality", str(row["w"]))
            finality["unresolved_audits"] = max(0, int(str(finality["unresolved_audits"])) - 1)
            if verdict.result != "MATCH":
                self._put_record_v2(
                    run_id,
                    "excluded",
                    identity,
                    {"reason": "UNVERIFIED", "evidence_hash": body_digest(expected)},
                )
            self._put_record_v2(run_id, "finality", str(row["w"]), finality)
            return {"result": verdict.result}

    def verified_inputs_v2(self, run_id: str, w: int) -> list[Any]:
        resource = self._service_boundary_v2(run_id, w)
        if resource is not None:
            from hypertrain.aggregator.capacity_worker import bounded_bytes
            from hypertrain.challenge.trust_v2 import ReplayEvidence as AcceptedReplay
            from hypertrain.data.store import LocalFSStore
            from hypertrain.protocol.messages_v2 import (
                AuditJobV2,
                CommitV2,
                DeltaManifestV2,
                EscrowLock,
                IslandJobV1,
                RoundOpenV2,
            )

            manifest = self._run_v2(run_id)
            now = self._now(self._db)
            opening = envelope_v2.Intake(
                run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
            ).accept(canonicalize(self._record_v2(run_id, "round", str(w))), now)
            assert isinstance(opening, RoundOpenV2)
            if (
                opening.w != w
                or opening.roster_hash != resource["roster_hash"]
                or opening.policy_hashes.body()
                != {k: getattr(manifest.network, k) for k in opening.policy_hashes.body()}
            ):
                raise ChallengeError(409, "SERVICE_INPUT_ROUND_AUTHORITY")
            if not isinstance(self.objects, LocalFSStore):
                raise ChallengeError(503, "SERVICE_INPUT_LOCAL_STORE")
            payloads = []
            for entry in opening.roster:
                identity = f"{w}:{entry.hotkey}"
                # No exclusions silently shorten the complete original roster on this path.
                commit_envelope = self._record_v2(run_id, "commit", identity)
                delta_envelope = self._record_v2(run_id, "delta", identity)
                accepted_commit = envelope_v2.Intake(
                    run_id, {"CommitV2": entry.hotkey.__eq__}
                ).accept(canonicalize(commit_envelope), now)
                accepted_delta = envelope_v2.Intake(
                    run_id, {"DeltaManifestV2": entry.hotkey.__eq__}
                ).accept(canonicalize(delta_envelope), now)
                assert isinstance(accepted_commit, CommitV2) and isinstance(
                    accepted_delta, DeltaManifestV2
                )
                accepted_commit.validate_assignment(manifest, manifest.training.batch_samples())
                if (
                    accepted_commit.w != w
                    or accepted_commit.hotkey != entry.hotkey
                    or accepted_delta.w != w
                    or accepted_delta.hotkey != entry.hotkey
                    or (accepted_delta.delta_hash, accepted_delta.size)
                    != (accepted_commit.delta_hash, accepted_commit.delta_bytes)
                ):
                    raise ChallengeError(409, "SERVICE_INPUT_COMMIT_AUTHORITY")
                for kind, wire, typed in (
                    ("CommitV2", commit_envelope, accepted_commit),
                    ("DeltaManifestV2", delta_envelope, accepted_delta),
                ):
                    intake_key = _dumps(
                        list(envelope_v2.replay_key(kind, run_id, entry.hotkey, typed))
                    )
                    intake = self._db.execute(
                        "SELECT * FROM accepted_v2 WHERE key=?", (intake_key,)
                    ).fetchone()
                    if intake is None:
                        raise ChallengeError(409, "SERVICE_INPUT_ORIGINAL_INTAKE")
                    receipt = envelope_v2.parse_envelope(json.loads(intake["receipt"]))
                    if (
                        intake["digest"] != body_digest(wire["body"])
                        or envelope_v2.parse_envelope(intake["envelope"]).model_dump(mode="json")
                        != wire
                        or receipt.type != "Receipt"
                        or receipt.run_id != run_id
                        or receipt.signer != manifest.training.coord_pubkey
                        or receipt.body["w"] != w
                        or receipt.body["commit_hash"] != intake["digest"]
                        or receipt.body["received_round"] != intake["accepted_beacon"]
                        or not envelope_v2.verify_envelope(json.loads(intake["receipt"]))
                    ):
                        raise ChallengeError(409, "SERVICE_INPUT_INTAKE_AUTHORITY")
                replay = AcceptedReplay(**self._record_v2(run_id, "replay", identity))
                signed_verdict = self._record_v2(run_id, "verdict", identity)
                matched = envelope_v2.Intake(
                    run_id, {"ReplayVerdict": lambda key: key in manifest.training.auditors}
                ).accept(canonicalize(signed_verdict), now)
                assert isinstance(matched, ReplayVerdict)
                lease = self._db.execute(
                    "SELECT * FROM audit_leases_v2 WHERE run_id=? AND w=? AND hotkey=?",
                    (run_id, w, entry.hotkey),
                ).fetchone()
                if lease is None:
                    raise ChallengeError(409, "SERVICE_INPUT_MATCH_LEASE")
                audit_job = AuditJobV2.model_validate(
                    self._record_v2(run_id, "audit-job", lease["id"])
                )
                if (
                    matched.result != "MATCH"
                    or matched.first_bad_leaf is not None
                    or lease["state"] != "completed"
                    or lease["result"] != "MATCH"
                    or lease["auditor"] != signed_verdict["signer"]
                    or audit_job.auditor_id != lease["auditor"]
                    or audit_job.run_id != run_id
                    or audit_job.manifest != manifest
                    or audit_job.job_id != lease["id"]
                    or audit_job.lease_nonce != lease["nonce"]
                    or audit_job.lease_expires != lease["expires"]
                    or audit_job.absolute_deadline != lease["absolute"]
                    or audit_job.start_state.hotkey != entry.hotkey
                    or audit_job.start_state.w != w
                    or audit_job.commit_envelope != commit_envelope
                    or matched.challenge_hash
                    != body_digest(envelope_v2.parse_envelope(audit_job.challenge_envelope).body)
                    or matched.recomputed_leaves_root != accepted_commit.leaves_root
                    or self._record_v2(run_id, "executed-verdict", lease["id"])
                    != matched.model_dump(mode="json")
                    or (replay.run_id, replay.w, replay.hotkey, replay.result, replay.audit_mode)
                    != (run_id, w, entry.hotkey, "MATCH", "anchored-full")
                    or replay.verdict_hash != body_digest(signed_verdict["body"])
                    or replay.anchor_hash != audit_job.start_state.digest()
                    or (
                        replay.leaves_root,
                        replay.delta_hash,
                        replay.final_theta_hash,
                        replay.ef_in_hash,
                        replay.ef_out_hash,
                    )
                    != (
                        accepted_commit.leaves_root,
                        accepted_commit.delta_hash,
                        accepted_commit.final_theta_hash,
                        accepted_commit.ef_in_hash,
                        accepted_commit.ef_out_hash,
                    )
                    or replay.assignment_hash
                    != self._record_v2(run_id, "assignment", identity)["assignment_hash"]
                ):
                    raise ChallengeError(409, "SERVICE_INPUT_MATCH_AUTHORITY")
                descriptor_record = self._record_v2(
                    run_id, "audit-geometry", f"{lease['id']}:{lease['nonce']}"
                )
                descriptor_receipt = envelope_v2.parse_envelope(descriptor_record["receipt"])
                if (
                    descriptor_receipt.type != "Receipt"
                    or descriptor_receipt.run_id != run_id
                    or descriptor_receipt.signer != manifest.training.coord_pubkey
                    or descriptor_receipt.body["w"] != w
                    or descriptor_receipt.body["commit_hash"]
                    != descriptor_record["descriptor_hash"]
                    or not envelope_v2.verify_envelope(descriptor_record["receipt"])
                ):
                    raise ChallengeError(409, "SERVICE_INPUT_DESCRIPTOR_SIGNATURE")
                # Existing ledger/admission predicates own origin-backed influence.
                escrow, admission, _ = self._services(run_id)
                funding_status = admission.status(entry.hotkey, now=now)
                owner = self._owner_v2(run_id, entry.hotkey)
                locked, lock_receipt = escrow.locked(entry.admission_id, owner)
                if (
                    not funding_status.eligible
                    or funding_status.record.admission_id != entry.admission_id
                    or funding_status.record.coldkey != owner
                    or (
                        funding_status.funding.run_id,
                        funding_status.funding.hotkey,
                        funding_status.funding.admission_id,
                    )
                    != (run_id, entry.hotkey, entry.admission_id)
                    or funding_status.funding.locked_units != locked
                    or funding_status.funding.lock_receipt_hash != lock_receipt
                    or locked <= 0
                    or not funding_status.funding.conservative_bound
                ):
                    raise ChallengeError(403, "SERVICE_INPUT_FUNDING_AUTHORITY")
                origins = self._db.execute(
                    "SELECT u.origin,u.units,o.origin AS issued_origin,e.request "
                    "FROM escrow_units u "
                    "LEFT JOIN escrow_origins o ON o.origin=u.origin "
                    "JOIN escrow_events e ON e.operation_id=u.ref AND e.kind='LOCK_ADMISSION' "
                    "WHERE u.owner=? AND u.bucket='admission_locked' AND u.units>0 "
                    "AND json_extract(e.request,'$.admission_id')=?",
                    (owner, entry.admission_id),
                ).fetchall()
                if (
                    not origins
                    or sum(row["units"] for row in origins) != locked
                    or any(row["issued_origin"] is None for row in origins)
                ):
                    raise ChallengeError(403, "SERVICE_INPUT_FUNDING_ORIGINS")
                for origin in origins:
                    lock = EscrowLock.model_validate_json(origin["request"])
                    if (
                        lock.owner != owner
                        or lock.admission_id != entry.admission_id
                        or origin["origin"] not in lock.origin_ids
                    ):
                        raise ChallengeError(403, "SERVICE_INPUT_FUNDING_ORIGINS")
                payloads.append((accepted_delta, descriptor_record, audit_job))
            for delta, descriptor_record, audit_job in payloads:
                descriptor_bytes = bounded_bytes(
                    self.objects._path(str(descriptor_record["descriptor_hash"])), 1 << 20
                )
                descriptor = envelope_v2.load_json(descriptor_bytes, max_bytes=1 << 20)
                geometry = IslandJobV1.model_validate(
                    self._record_v2(
                        run_id,
                        "island-job",
                        f"{audit_job.start_state.w}:{audit_job.start_state.hotkey}",
                    )
                )
                deadline = (
                    manifest.training.beacon.genesis_time
                    + (min(audit_job.lease_expires, audit_job.absolute_deadline) - 1) * 3
                )
                directory = (
                    self.state_dir / "audit-geometry-v2" / audit_job.job_id / audit_job.lease_nonce
                )
                if (
                    sha256_hex(descriptor_bytes) != descriptor_record["descriptor_hash"]
                    or descriptor
                    != {
                        "job": geometry.model_copy(update={"deadline": deadline}).body(),
                        "audit_job_hash": audit_job.digest(),
                        "lease_nonce": audit_job.lease_nonce,
                        "expires": audit_job.lease_expires,
                        "absolute": audit_job.absolute_deadline,
                        "directory": str(directory),
                    }
                    or geometry.manifest != manifest
                    or geometry.run_id != run_id
                    or geometry.w != w
                    or geometry.sample_ids != audit_job.sample_ids
                    or geometry.start_state_sha256 != audit_job.start_state.state_object_sha256
                    or geometry.ef_in_sha256 != audit_job.ef_in.sha256
                    or geometry.v0_sha256 != audit_job.v0.sha256
                ):
                    raise ChallengeError(409, "SERVICE_INPUT_DESCRIPTOR_BINDING")
                raw_delta = bounded_bytes(self.objects._path(delta.delta_hash), 65536)
                if sha256_hex(raw_delta) != delta.delta_hash or len(raw_delta) != delta.size:
                    raise ChallengeError(422, "SERVICE_INPUT_DELTA_HASH")
                from hypertrain.trainer.compress import validate_payload
                from hypertrain.trainer.config import TrainConfig
                from hypertrain.trainer.model import param_shapes

                config = TrainConfig.from_manifest_v2(manifest)
                if (
                    validate_payload(raw_delta, param_shapes(config.model), max_payload_bytes=65536)
                    != config.compress.codec
                ):
                    raise ChallengeError(422, "SERVICE_INPUT_DELTA_CODEC")
            self._service_boundary_v2(run_id, w, heavy=True)
        self._service_boundary_v2(run_id, w)
        from hypertrain.aggregator.tape_v2 import VerifiedInput
        from hypertrain.challenge.trust_v2 import ReplayEvidence, SettlementStatus
        from hypertrain.protocol.messages_v2 import CommitV2, DeltaManifestV2, RoundOpenV2

        self.require_roster_v2(run_id, w)
        _, admission, disputes = self._services(run_id)
        body = RoundOpenV2.model_validate(self._record_v2(run_id, "round", str(w))["body"])
        now = self._now(self._db)
        works = []
        for entry in body.roster:
            identity = f"{w}:{entry.hotkey}"
            excluded = self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='excluded' AND id=?",
                (run_id, identity),
            ).fetchone()
            if excluded:
                continue
            status = admission.status(entry.hotkey, now=now)
            if not status.eligible or status.record.admission_id != entry.admission_id:
                raise ChallengeError(403, "current admission funding/identity no longer eligible")
            pending = (
                self._db.execute(
                    "SELECT 1 FROM disputes_v2 WHERE miner=? "
                    "AND json_extract(turn,'$.resolution') IS NULL",
                    (entry.hotkey,),
                ).fetchone()
                is not None
            )
            settlement = SettlementStatus(
                run_id,
                w,
                entry.hotkey,
                pending,
                False,
                sha256_hex(canonicalize([run_id, w, entry.hotkey, status.record.hotkey, pending])),
            )
            works.append(
                VerifiedInput(
                    entry,
                    CommitV2.model_validate(self._record_v2(run_id, "commit", identity)["body"]),
                    DeltaManifestV2.model_validate(
                        self._record_v2(run_id, "delta", identity)["body"]
                    ),
                    ReplayEvidence(**self._record_v2(run_id, "replay", identity)),
                    status.funding,
                    settlement,
                    str(self._record_v2(run_id, "assignment", identity)["assignment_hash"]),
                    status.record.clean_count,
                )
            )
        return works

    def _capacity_outer_v2(self, run_id: str, w: int) -> dict[str, Any]:
        """Internal bounded adapter; public heavy gate remains independently closed."""
        import os
        import sys

        from hypertrain.aggregator.capacity_worker import (
            OuterInput,
            OuterRequest,
            bounded_bytes,
            collect_result,
        )
        from hypertrain.aggregator.tape_v2 import TapeV2
        from hypertrain.challenge.finality_v2 import require_apply
        from hypertrain.data.store import LocalFSStore
        from hypertrain.gpu_ops.journal import durable_write
        from hypertrain.miner.island_launch import CapacityAttempt, run_capacity_argv
        from hypertrain.protocol.envelope_v2 import tape_signing_message

        with self._tx():
            manifest = self._run_v2(run_id)
            resource = self._service_boundary_v2(run_id, w)
            if resource is None or not isinstance(self.objects, LocalFSStore):
                raise ChallengeError(503, "SERVICE_CAPACITY_OUTER_AUTHORITY")
            _, profile_hash = self._service_admission_v2(run_id)
            self._fresh_v2(manifest, f"aggregate:{w}:{self._now(self._db)}")
            require_apply(run_id, w, [s for s in self._history_v2(run_id) if s.w < w])
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='applied' AND id=?",
                (run_id, str(w)),
            ).fetchone():
                raise ChallengeError(409, "SERVICE_CAPACITY_OUTER_ALREADY_APPLIED")
            opening = self._record_v2(run_id, "round", str(w))
            works = self.verified_inputs_v2(run_id, w)
            inputs = [OuterInput.model_validate(self._serialize_input_v2(x)) for x in works]
            prior = self._record_v2(run_id, "applied", str(w - 1)) if w else None
            prev_state = str(prior["out_state"]) if prior else None
            predecessor = str(prior["tape_hash"]) if prior else "0" * 64
            objects = {
                manifest.network.aggregation_policy_hash,
                manifest.network.economics_policy_hash,
                *(x.commit.delta_hash for x in inputs),
                *(sha256_hex(canonicalize(x.commit.model_dump(mode="json"))) for x in inputs),
                *(
                    sha256_hex(canonicalize(x.delta_manifest.model_dump(mode="json")))
                    for x in inputs
                ),
            }
            if prev_state:
                objects.add(prev_state)
            request = OuterRequest(
                manifest=manifest,
                w=w,
                original_roster=opening["body"]["roster"],
                inputs=inputs,
                prev_state=prev_state,
                predecessor_tape_hash=predecessor,
                reference_reward_units=self._services(run_id)[0].policy.R_collectible_units,
                objects=sorted(objects),
            )
            snapshot = canonicalize(request.body())
            OuterRequest.parse(snapshot)
            directory = self.state_dir / "capacity-outer" / run_id / str(w) / "1"
            directory.mkdir(parents=True, exist_ok=True)
            if (directory / "request.json").exists():
                raise ChallengeError(409, "SERVICE_CAPACITY_OUTER_ATTEMPT_RECORDED")
            staged = LocalFSStore(directory / "objects")
            for key in request.objects:
                payload = bounded_bytes(self.objects._path(key), 65536)
                if sha256_hex(payload) != key:
                    raise ChallengeError(422, "SERVICE_CAPACITY_OUTER_OBJECT_HASH")
                staged.put(payload)
            durable_write(directory / "request.json", snapshot)
            reservation = self._record_v2(run_id, "round-resource", str(w))
            deadline = (
                manifest.training.beacon.genesis_time + (int(reservation["deadline"]) - 1) * 3
            )
        identity = sha256_hex(
            canonicalize({"run_id": run_id, "w": w, "operation": "outer", "attempt": 1})
        )
        capacity = CapacityAttempt(
            identity,
            profile_hash,
            self.state_dir / "capacity-runtime.lock",
            lambda: self._service_charge_v2(
                run_id, w, "outer", 1, kind="outer", units=int(resource["outer_work_units"])
            ),
        )
        env = dict(
            os.environ, OMP_NUM_THREADS="1", MKL_NUM_THREADS="1", CUBLAS_WORKSPACE_CONFIG=":4096:8"
        )
        run_capacity_argv(
            [
                sys.executable,
                "-m",
                "hypertrain.aggregator.capacity_worker",
                str(directory.resolve()),
            ],
            directory / "runtime",
            deadline,
            capacity,
            env=env,
        )
        body, output_objects = collect_result(directory, request)
        with self._tx():
            self._service_boundary_v2(run_id, w)
            self._fresh_v2(manifest, f"aggregate-result:{w}:{self._now(self._db)}")
            require_apply(run_id, w, [s for s in self._history_v2(run_id) if s.w < w])
            current = [
                OuterInput.model_validate(self._serialize_input_v2(x))
                for x in self.verified_inputs_v2(run_id, w)
            ]
            if (
                current != inputs
                or self._record_v2(run_id, "round", str(w)) != opening
                or (w and self._record_v2(run_id, "applied", str(w - 1)) != prior)
                or self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='applied' AND id=?",
                    (run_id, str(w)),
                ).fetchone()
            ):
                raise ChallengeError(409, "SERVICE_CAPACITY_OUTER_STALE")
            signing_key = self._coord()
            tape = TapeV2(
                body=body,
                signer=signing_key.ss58,
                sig=signing_key.sign(tape_signing_message(body.body(), run_id)).hex(),
            )
            for digest, payload in output_objects.items():
                if self.objects.put(payload) != digest:
                    raise ChallengeError(422, "SERVICE_CAPACITY_OUTER_OUTPUT_HASH")
            tape_hash = self.objects.put(tape.to_bytes())
            applied = {
                "tape_hash": tape_hash,
                "out_state": body.out_state,
                "theta_hash": body.out_hashes["theta_hash"],
                "prev_state": body.prev_state,
                "predecessor_tape_hash": predecessor,
            }
            self._put_record_v2(run_id, "applied", str(w), applied)
            self._put_record_v2(
                run_id, "tape-inputs", str(w), {"inputs": [x.body() for x in inputs]}
            )
            finality = self._record_v2(run_id, "finality", str(w))
            finality["applied"], finality["state_hash"] = True, applied["theta_hash"]
            self._put_record_v2(run_id, "finality", str(w), finality)
            return {"tape": tape.body_json(), **applied}

    def aggregate_v2(self, run_id: str, w: int) -> dict[str, Any]:
        preflight_resource = self._service_boundary_v2(run_id, w)
        if preflight_resource is not None:
            from hypertrain.aggregator.capacity_worker import bounded_bytes
            from hypertrain.aggregator.tape_v2 import TapeV2
            from hypertrain.data.store import LocalFSStore
            from hypertrain.protocol.envelope_v2 import tape_signing_message
            from hypertrain.protocol.messages_v2 import RoundOpenV2

            manifest = self._run_v2(run_id)
            if not isinstance(self.objects, LocalFSStore):
                raise ChallengeError(503, "SERVICE_AGGREGATE_LOCAL_STORE")
            opening = envelope_v2.Intake(
                run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
            ).accept(canonicalize(self._record_v2(run_id, "round", str(w))), self._now(self._db))
            assert isinstance(opening, RoundOpenV2)
            if opening.w != w or opening.roster_hash != preflight_resource["roster_hash"]:
                raise ChallengeError(409, "SERVICE_AGGREGATE_ROUND_AUTHORITY")
            if opening.policy_hashes.body() != {
                k: getattr(manifest.network, k) for k in opening.policy_hashes.body()
            }:
                raise ChallengeError(409, "SERVICE_INPUT_ROUND_AUTHORITY")
            prior_tape = None
            for round_w in range(max(0, w - 1), w + 1):
                applied_row = self._db.execute(
                    "SELECT data FROM records_v2 WHERE run_id=? AND kind='applied' AND id=?",
                    (run_id, str(round_w)),
                ).fetchone()
                if applied_row is None:
                    if round_w < w:
                        raise ChallengeError(409, "SERVICE_AGGREGATE_PREDECESSOR_MISSING")
                    continue
                applied_metadata = json.loads(applied_row[0])
                tape_bytes = bounded_bytes(
                    self.objects._path(str(applied_metadata["tape_hash"])), 1 << 20
                )
                if sha256_hex(tape_bytes) != applied_metadata["tape_hash"]:
                    raise ChallengeError(422, "SERVICE_AGGREGATE_TAPE_HASH")
                signed_tape = TapeV2.from_bytes(tape_bytes)
                tape_body = signed_tape.body
                tape_opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(
                    canonicalize(self._record_v2(run_id, "round", str(round_w))),
                    self._now(self._db),
                )
                assert isinstance(tape_opening, RoundOpenV2)
                if (
                    signed_tape.signer != manifest.training.coord_pubkey
                    or tape_body.run_id != run_id
                    or tape_body.w != round_w
                    or not verify(
                        decode_hotkey(signed_tape.signer),
                        tape_signing_message(tape_body.body(), run_id),
                        bytes.fromhex(signed_tape.sig),
                    )
                    or tape_body.prev_state != applied_metadata["prev_state"]
                    or tape_body.out_state != applied_metadata["out_state"]
                    or tape_body.predecessor_tape_hash != applied_metadata["predecessor_tape_hash"]
                    or tape_body.out_hashes["theta_hash"] != applied_metadata["theta_hash"]
                    or tape_body.policy_hash != manifest.network.aggregation_policy_hash
                    or tape_body.policy_hashes.body() != tape_opening.policy_hashes.body()
                    or {entry.roster.hotkey for entry in tape_body.inputs}
                    | {entry.hotkey for entry in tape_body.excluded}
                    != {entry.hotkey for entry in tape_opening.roster}
                    or any(entry.roster not in tape_opening.roster for entry in tape_body.inputs)
                    or (round_w == 0 and tape_body.predecessor_tape_hash != "0" * 64)
                ):
                    raise ChallengeError(409, "SERVICE_AGGREGATE_TAPE_AUTHORITY")
                if round_w == w - 1:
                    prior_tape = (applied_metadata["tape_hash"], tape_body.out_state)
                elif w and (tape_body.predecessor_tape_hash, tape_body.prev_state) != prior_tape:
                    raise ChallengeError(409, "SERVICE_AGGREGATE_TAPE_LINEAGE")
            self._service_boundary_v2(run_id, w, heavy=True)
            self.verified_inputs_v2(run_id, w)
        resource = self._service_boundary_v2(run_id, w, heavy=True)
        if resource is not None:
            return self._capacity_outer_v2(run_id, w)
        import hypertrain.trainer  # noqa: F401
        from hypertrain.aggregator.core import OuterState
        from hypertrain.aggregator.tape_v2 import make_tape, replay_tape
        from hypertrain.challenge.finality_v2 import require_apply
        from hypertrain.trainer.config import TrainConfig
        from hypertrain.trainer.model import init_params

        with self._tx():
            manifest = self._run_v2(run_id)
            self.require_roster_v2(run_id, w)
            self._fresh_v2(manifest, f"aggregate:{w}:{self._now(self._db)}")
            existing = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='applied' AND id=?",
                (run_id, str(w)),
            ).fetchone()
            if existing is not None:
                from hypertrain.aggregator.tape_v2 import TapeV2

                saved = json.loads(existing[0])
                if saved.get("kind") == "repair":
                    from hypertrain.aggregator.rollback_v2 import RepairTape

                    repair = RepairTape.from_bytes(self.objects.get(saved["tape_hash"]))
                    return {"repair": repair.model_dump(mode="json"), **saved}
                tape = TapeV2.from_bytes(self.objects.get(saved["tape_hash"]))
                return {"tape": tape.body_json(), **saved}
            history = [s for s in self._history_v2(run_id) if s.w < w]
            require_apply(run_id, w, history)
            inputs = self.verified_inputs_v2(run_id, w)
            policy = AggregationPolicyV2.model_validate_json(
                self.objects.get(manifest.network.aggregation_policy_hash)
            )
            economics = self.objects.get(manifest.network.economics_policy_hash)
            if w:
                previous = self._record_v2(run_id, "applied", str(w - 1))
                prev_state, predecessor = str(previous["out_state"]), str(previous["tape_hash"])
            else:
                theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
                prev_state = self.objects.put(
                    OuterState.init(
                        {k: v.detach().cpu().numpy() for k, v in theta.items()}
                    ).to_bytes()
                )
                predecessor = "0" * 64
            tape = make_tape(
                self.objects,
                manifest,
                policy,
                economics,
                self._coord(),
                w=w,
                prev_state=prev_state,
                predecessor_tape_hash=predecessor,
                inputs=inputs,
                reference_reward_units=self._services(run_id)[0].policy.R_collectible_units,
            )
            self.require_roster_v2(run_id, w)
            replay_tape(
                self.objects,
                tape,
                manifest,
                policy,
                economics,
                signer=self._coord().ss58,
                w=w,
                prev_state=prev_state,
                predecessor_tape_hash=predecessor,
                inputs=inputs,
                reference_reward_units=self._services(run_id)[0].policy.R_collectible_units,
            )
            tape_hash = self.objects.put(tape.to_bytes())
            applied = {
                "tape_hash": tape_hash,
                "out_state": tape.body.out_state,
                "theta_hash": tape.body.out_hashes["theta_hash"],
                "prev_state": prev_state,
                "predecessor_tape_hash": predecessor,
            }
            self._put_record_v2(run_id, "applied", str(w), applied)
            # Persist historical authority facts, never replace with future released locks.
            self._put_record_v2(
                run_id,
                "tape-inputs",
                str(w),
                {"inputs": [self._serialize_input_v2(x) for x in inputs]},
            )
            finality = self._record_v2(run_id, "finality", str(w))
            finality["applied"], finality["state_hash"] = True, applied["theta_hash"]
            self._put_record_v2(run_id, "finality", str(w), finality)
            return {"tape": tape.body_json(), **applied}

    @staticmethod
    def _serialize_input_v2(work: Any) -> dict[str, Any]:
        return {
            "roster": work.roster.body(),
            "commit": work.commit.model_dump(mode="json"),
            "delta_manifest": work.delta_manifest.model_dump(mode="json"),
            "replay": asdict(work.replay),
            "funding": asdict(work.funding),
            "settlement": asdict(work.settlement),
            "assignment_hash": work.assignment_hash,
            "clean_finalizations": work.clean_finalizations,
        }

    def finalize_v2(self, run_id: str, w: int, raw: bytes) -> dict[str, Any]:
        from hypertrain.challenge.finality_v2 import RoundStatus, settle
        from hypertrain.ledger.escrow_v2 import RewardWork, authority_message, reward_origin_id
        from hypertrain.protocol.messages_v2 import RewardFinalize

        with self._tx():
            manifest = self._run_v2(run_id)
            if self._service_boundary_v2(run_id, w) is not None:
                final_request = envelope_v2.Intake(
                    run_id, {"Finalize": manifest.training.coord_pubkey.__eq__}
                ).accept(raw, self._now(self._db))
                assert isinstance(final_request, Finalize)
                original_applied = self._record_v2(run_id, "applied", str(w))
                from hypertrain.protocol.messages_v2 import RoundOpenV2

                final_opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(
                    canonicalize(self._record_v2(run_id, "round", str(w))), self._now(self._db)
                )
                assert isinstance(final_opening, RoundOpenV2)
                if (
                    final_request.w != w
                    or final_request.final_theta_hash_w1 != original_applied["theta_hash"]
                    or original_applied.get("kind") == "repair"
                    or final_request.included
                    != sorted(entry.hotkey for entry in final_opening.roster)
                ):
                    raise ChallengeError(409, "SERVICE_FINALIZE_AUTHORITY")
                self.aggregate_v2(run_id, w)
                self._service_boundary_v2(run_id, w, heavy=True)
            model = self._authenticated_v2(run_id, raw, "Finalize", manifest.training.coord_pubkey)
            assert isinstance(model, Finalize)
            previous_final = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='finalize' AND id=?",
                (run_id, str(w)),
            ).fetchone()
            if previous_final is not None:
                if json.loads(previous_final[0])["body"] != model.model_dump(mode="json"):
                    raise ChallengeError(409, "conflicting finalized origin authority")
                return self._record_v2(run_id, "finalize-result", str(w))
            self.require_roster_v2(run_id, w)
            applied = self._record_v2(run_id, "applied", str(w))
            if applied.get("kind") == "repair":
                raise ChallengeError(
                    409, "repair evidence is model-only; cannot mint miner rewards"
                )
            status = RoundStatus(**self._record_v2(run_id, "finality", str(w)))
            included = sorted(x.roster.hotkey for x in self.verified_inputs_v2(run_id, w))
            if model.w != w or model.included != included:
                raise ChallengeError(
                    422, "finalized included set differs from full accepted authority"
                )
            now = self._fresh_v2(manifest, f"finalize:{w}:{self._now(self._db)}")
            final_hash = body_digest(model.model_dump(mode="json"))
            settled = settle(status, model, final_hash)
            self._put_record_v2(run_id, "finality", str(w), asdict(settled))
            self._put_record_v2(
                run_id, "finalize", str(w), envelope_v2.parse_envelope(raw).model_dump(mode="json")
            )
            self._put_record_v2(
                run_id,
                "replayed-finality",
                str(w),
                {
                    "finalize_hash": final_hash,
                    "tape_hash": str(applied["tape_hash"]),
                    "finalized_beacon": now,
                    "shadow": False,
                    "theta_hash": str(applied["theta_hash"]),
                },
            )
            works = []
            for hotkey in included:
                identity = f"{w}:{hotkey}"
                row = self._db.execute(
                    "SELECT challenge FROM audit_leases_v2 WHERE run_id=? AND w=? AND hotkey=?",
                    (run_id, w, hotkey),
                ).fetchone()
                works.append(
                    RewardWork(
                        envelope_v2.parse_envelope(self._record_v2(run_id, "commit", identity)),
                        envelope_v2.parse_envelope(self._record_v2(run_id, "verdict", identity)),
                        envelope_v2.parse_envelope(json.loads(row[0])),
                        self._owner_v2(run_id, hotkey),
                        self._assignment_v2(run_id, w, hotkey),
                    )
                )
            escrow = self._services(run_id)[0]
            vesting = now + manifest.training.verify.E_vest_rounds
            evidence = FinalityEvidence(
                envelope_v2.parse_envelope(raw),
                tuple(works),
                str(applied["tape_hash"]),
                False,
                now,
                vesting,
            )
            total = sum(int(str(item.commit.body["tokens"])) for item in works)
            units = {
                item.commit.signer: escrow.policy.round_reward_units
                * int(str(item.commit.body["tokens"]))
                // total
                for item in works
            }
            remainder = escrow.policy.round_reward_units - sum(units.values())
            for hot in included[:remainder]:
                units[hot] += 1
            origins = [
                {
                    "origin_id": reward_origin_id(run_id, w, h, final_hash),
                    "owner": self._owner_v2(run_id, h),
                    "units": units[h],
                    "mature_at": vesting,
                }
                for h in included
                if units[h]
            ]
            reward = RewardFinalize(
                run_id=run_id,
                w=w,
                finalize_hash=final_hash,
                tape_hash=str(applied["tape_hash"]),
                verdict_root=sha256_hex(
                    canonicalize([body_digest(item.verdict.body) for item in works])
                ),
                allocation_hash=sha256_hex(canonicalize(origins)),
                budget_units=escrow.policy.round_reward_units,
                origin_ids=[str(o["origin_id"]) for o in origins],
                mature_at=vesting,
                authority_sig="0" * 128,
            )
            reward = reward.model_copy(
                update={"authority_sig": self._coord().sign(authority_message(reward)).hex()}
            )
            receipt = escrow.reward_finalize(reward, evidence)
            for origin in origins:
                self._put_record_v2(
                    run_id,
                    "settlement",
                    str(origin["origin_id"]),
                    Settlement(
                        finality_hash=final_hash,
                        closed_dispute_root=sha256_hex(canonicalize([])),
                        release_beacon=vesting,
                        unresolved=False,
                        outcome="MATCH",
                    ).body(),
                )
            answer: dict[str, JsonValue] = {
                "status": "SETTLED",
                "reward_receipt": receipt.body(),
                "theta_hash": model.final_theta_hash_w1,
            }
            self._put_record_v2(run_id, "finalize-result", str(w), answer)
            return answer

    def relay_assignment_v2(self, run_id: str, w: int, hotkey: str) -> dict[str, Any]:
        from hypertrain.protocol import relay_envelope
        from hypertrain.protocol.relay_messages import RelayAssignment, RelayNetworkManifest

        with self._tx():
            manifest = self._run_v2(run_id)
            self.require_roster_v2(run_id, w)
            self._owner_v2(run_id, hotkey)
            self._record_v2(run_id, "assignment", f"{w}:{hotkey}")
            registry = self._registry_v2(manifest)
            network = RelayNetworkManifest(
                run_id=run_id,
                base_manifest_hash=manifest.training.training_hash(),
                registry_hash=registry.digest(),
                specs_hash=sha256_hex(canonicalize([s.body() for s in registry.specs])),
                assignment_policy="master-observed-median3-v1",
            )
            prior = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='relay-assignment' AND id=?",
                (run_id, f"{w}:{hotkey}"),
            ).fetchone()
            if prior is not None:
                return {
                    "assignment": json.loads(prior[0]),
                    "registry": registry.body(),
                    "network_manifest": self._record_v2(run_id, "relay-network", "current"),
                }
            choices = sorted(registry.specs, key=lambda s: (s.region, s.id))
            disabled = {
                r[0]
                for r in self._db.execute(
                    "SELECT id FROM records_v2 WHERE run_id=? AND kind='relay-disabled'", (run_id,)
                )
            }
            now = self._now(self._db)
            draining = {
                r[0]
                for r in self._db.execute(
                    "SELECT id FROM records_v2 WHERE run_id=? AND kind='relay-draining'", (run_id,)
                )
            }
            choices = [
                s
                for s in choices
                if s.id not in draining
                and any(
                    k.valid_from_round <= now < k.valid_until_round
                    and f"{s.id}:{registry.epoch}:{k.key_id}" not in disabled
                    for k in s.pubkeys
                )
            ]
            if not choices:
                raise ChallengeError(503, "no approved healthy relay; direct rescue required")
            body = self._record_v2(run_id, "round", str(w))["body"]
            assignment = RelayAssignment(
                w=w,
                hotkey=hotkey,
                primary_id=choices[0].id,
                fallback_ids=[s.id for s in choices[1:]],
                assignment_epoch=registry.epoch,
                manifest_hash=network.digest(),
                exp_drand=int(str(body["d_upload"])),
            )
            env = relay_envelope.seal(
                self._coord(), "RelayAssignment", run_id, assignment, assignment.exp_drand
            )
            self._put_record_v2(run_id, "relay-assignment", f"{w}:{hotkey}", env)
            self._put_record_v2(
                run_id,
                "relay-network",
                "current",
                relay_envelope.seal(self._coord(), "RelayNetworkManifest", run_id, network, NEVER),
            )
            return {
                "assignment": env,
                "registry": registry.body(),
                "network_manifest": self._record_v2(run_id, "relay-network", "current"),
            }

    def relay_settlement_v2(self, run_id: str, grant_hash: str) -> dict[str, Any]:
        """Authenticated control-plane adapter for L4 retention release, not client facts."""
        from hypertrain.relay.core import Settlement as RelaySettlement

        with self._lock:
            grant = self._record_v2(run_id, "grant", grant_hash)["body"]
            w = int(grant["w"])
            status = self._record_v2(run_id, "finality", str(w))
            if (
                status["disposition"] != "SETTLED"
                or status["unresolved_audits"]
                or status["unresolved_disputes"]
            ):
                raise ChallengeError(409, "relay custody remains pinned by unresolved finality")
            accepted = self._record_v2(run_id, "replayed-finality", str(w))
            turns = [
                json.loads(r[0])
                for r in self._db.execute(
                    "SELECT turn FROM disputes_v2 WHERE json_extract(turn,'$.contest.w')=?", (w,)
                )
            ]
            if any(t["resolution"] is None or t["resolved_beacon"] is None for t in turns):
                raise ChallengeError(409, "relay custody remains dispute pinned")
            closed = [t["resolution"] for t in turns]
            settled = RelaySettlement(
                str(accepted["finalize_hash"]),
                int(accepted["finalized_beacon"]),
                sha256_hex(canonicalize(closed)),
                max([int(accepted["finalized_beacon"]), *[t["resolved_beacon"] for t in turns]]),
                int(accepted["finalized_beacon"])
                + self._run_v2(run_id).training.verify.E_vest_rounds,
            )
            return asdict(settled)

    def dispute_state_v2(self, run_id: str, dispute_id: str, checkpoint: int) -> dict[str, Any]:
        _, _, disputes = self._services(run_id)
        with self._tx():
            self._snapshot_current_v2(self._run_v2(run_id))
            event = disputes.state_request(dispute_id, checkpoint, now=self._now(self._db))
        disputes.notify()
        self._notify_processes_v2()
        return event.model_dump(mode="json")

    def referee_v2(self, run_id: str, dispute_id: str) -> dict[str, Any]:
        """Internal full anchored referee execution; HTTP never supplies replay truth."""
        import shutil

        from hypertrain.auditor.island_bisect import IslandParty, adjudicate
        from hypertrain.auditor.replay import AnchorCache
        from hypertrain.auditor.worker import LeaseGuard
        from hypertrain.miner.island_launch import launch_island
        from hypertrain.protocol.messages_v2 import IslandJobV1

        _, _, disputes = self._services(run_id)
        with self._tx():
            manifest = self._run_v2(run_id)
            backend = self._backend_v2(manifest)
            self._snapshot_current_v2(manifest)
            turn = disputes.get(dispute_id)
            if turn.resolution is not None or self._now(self._db) >= turn.absolute_deadline:
                raise ChallengeError(409, "referee outside unresolved absolute horizon")
            resource = self._service_boundary_v2(run_id, turn.contest.w)
            if resource is not None:
                from hypertrain.aggregator.capacity_worker import bounded_bytes
                from hypertrain.challenge.disputes_v2 import WatchEvent, authentic
                from hypertrain.data.store import LocalFSStore
                from hypertrain.protocol.messages import Receipt, ReplayVerdict
                from hypertrain.protocol.messages_v2 import RoundOpenV2
                from hypertrain.trainer.config import TrainConfig
                from hypertrain.trainer.model import param_shapes

                now = self._now(self._db)
                opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(self._record_v2(run_id, "round", str(turn.contest.w))), now)
                assert isinstance(opening, RoundOpenV2)
                event_row = self._db.execute(
                    "SELECT event FROM dispute_events_v2 "
                    "WHERE json_extract(event,'$.turn.dispute_id')=? ORDER BY cursor DESC LIMIT 1",
                    (dispute_id,),
                ).fetchone()
                event = WatchEvent.model_validate_json(event_row[0]) if event_row else None
                if (
                    turn.run_id != run_id
                    or turn.dispute_id != dispute_id
                    or event is None
                    or not authentic(event)
                    or event.signer != manifest.training.coord_pubkey
                    or event.turn != turn
                    or event.issued_beacon > now
                    or opening.w != turn.contest.w
                    or opening.roster_hash != resource["roster_hash"]
                    or turn.contest.miner not in {r.hotkey for r in opening.roster}
                    or turn.contest.auditor not in manifest.training.auditors
                ):
                    raise ChallengeError(409, "SERVICE_REFEREE_TURN_AUTHORITY")
                claim = self._record_v2(run_id, "verdict", f"{turn.contest.w}:{turn.contest.miner}")
                verdict = envelope_v2.Intake(
                    run_id, {"ReplayVerdict": turn.contest.auditor.__eq__}
                ).accept(canonicalize(claim), now)
                assert isinstance(verdict, ReplayVerdict)
                if (
                    body_digest(claim["body"]) != turn.contest.verdict_hash
                    or verdict.challenge_hash != turn.contest.challenge_hash
                ):
                    raise ChallengeError(409, "SERVICE_REFEREE_EVIDENCE_AUTHORITY")
                metadata = self._record_v2(run_id, "trace-geometry", turn.contest.verdict_hash)
                preflight_job = IslandJobV1.model_validate(metadata["job"])
                directory = self.state_dir / "referee-v2" / dispute_id / turn.transcript_hash
                deadline = manifest.training.beacon.genesis_time + (turn.absolute_deadline - 1) * 3
                expected_descriptor = {
                    "job": preflight_job.model_copy(update={"deadline": deadline}).body(),
                    "historical_geometry_hash": body_digest(metadata),
                    "dispute_id": dispute_id,
                    "transcript_hash": turn.transcript_hash,
                    "seq": turn.seq,
                    "referee": turn.contest.referee,
                    "absolute_deadline": turn.absolute_deadline,
                    "backend": backend,
                }
                current_descriptor = self._record_v2(
                    run_id, "referee-job", f"{dispute_id}:{turn.transcript_hash}"
                )
                receipt = envelope_v2.Intake(
                    run_id, {"Receipt": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(current_descriptor["receipt"]), now)
                assert isinstance(receipt, Receipt)
                if (
                    receipt.w != turn.contest.w
                    or receipt.commit_hash != current_descriptor["descriptor_hash"]
                    or receipt.received_round > now
                    or preflight_job.manifest != manifest
                    or preflight_job.run_id != run_id
                    or preflight_job.w != turn.contest.w
                    or set(preflight_job.object_paths)
                    != {"start_state", "ef_in", "v0", "samples", "sample_proofs"}
                    or any(
                        Path(p).is_absolute() or ".." in Path(p).parts
                        for p in preflight_job.object_paths.values()
                    )
                ):
                    raise ChallengeError(409, "SERVICE_REFEREE_DESCRIPTOR_AUTHORITY")
                if not isinstance(self.objects, LocalFSStore):
                    raise ChallengeError(503, "SERVICE_REFEREE_LOCAL_STORE")
                descriptor_bytes = bounded_bytes(
                    self.objects._path(str(current_descriptor["descriptor_hash"])), 1 << 20
                )
                if (
                    sha256_hex(descriptor_bytes) != current_descriptor["descriptor_hash"]
                    or envelope_v2.load_json(descriptor_bytes, max_bytes=1 << 20)
                    != expected_descriptor
                ):
                    raise ChallengeError(409, "SERVICE_REFEREE_DESCRIPTOR_BINDING")
                from hypertrain.protocol.messages_v2 import StartStateV2

                original_start = StartStateV2.model_validate(
                    self._record_v2(run_id, "start", f"{turn.contest.w}:{turn.contest.miner}")
                )
                anchor_record = self._record_v2(
                    run_id, "anchor", f"{turn.contest.w - 1}:{turn.contest.miner}"
                )
                if (
                    original_start.run_id != run_id
                    or original_start.w != turn.contest.w
                    or original_start.hotkey != turn.contest.miner
                    or original_start.state_object_sha256 != preflight_job.start_state_sha256
                    or original_start.ef_object_sha256 != preflight_job.ef_in_sha256
                    or anchor_record["hotkey"] != turn.contest.miner
                    or anchor_record["w"] != turn.contest.w - 1
                    or anchor_record["anchor_hash"] != original_start.parent_anchor_hash
                    or anchor_record["proof_hash"] != original_start.anchor_verdict_hash
                    or anchor_record["backend"] not in ("genesis", backend)
                ):
                    raise ChallengeError(409, "SERVICE_REFEREE_ANCHOR_AUTHORITY")
                published = Path(str(metadata["directory"]))
                root = self.state_dir / "audit-geometry-v2"
                if (
                    not published.is_relative_to(root)
                    or ".." in published.parts
                    or published.name != "published"
                ):
                    raise ChallengeError(409, "SERVICE_REFEREE_PATH")
                shapes = param_shapes(TrainConfig.from_manifest_v2(manifest).model)
                for name, key in (
                    ("start_state", preflight_job.start_state_sha256),
                    ("ef_in", preflight_job.ef_in_sha256),
                    ("v0", preflight_job.v0_sha256),
                ):
                    data = bounded_bytes(published.parent / preflight_job.object_paths[name], 65536)
                    if sha256_hex(data) != key:
                        raise ChallengeError(422, "SERVICE_REFEREE_OBJECT_HASH")
                    if len(data) < 8 or struct.unpack_from("<Q", data)[0] > len(data) - 8:
                        raise ChallengeError(422, "SERVICE_REFEREE_STATE_HEADER")
                    size = struct.unpack_from("<Q", data)[0]
                    header = envelope_v2.load_json(data[8 : 8 + size], max_bytes=65536)
                    expected = {
                        f"{part}/{tensor}": list(shape)
                        for part in (("theta", "m", "v") if name == "start_state" else ("theta",))
                        for tensor, shape in shapes.items()
                    }
                    if name == "start_state":
                        expected["step"] = []
                    if name == "v0":
                        expected = {}
                    if not isinstance(header, dict) or set(header) != set(expected):
                        raise ChallengeError(422, "SERVICE_REFEREE_STATE_NAMES")
                    spans: list[tuple[int, int]] = []
                    for tensor, shape in expected.items():
                        info = header[tensor]
                        if (
                            not isinstance(info, dict)
                            or set(info) != {"dtype", "shape", "data_offsets"}
                            or info["shape"] != shape
                            or not isinstance(info["shape"], list)
                            or any(type(dim) is not int for dim in info["shape"])
                            or info["dtype"] != ("I64" if tensor == "step" else "F32")
                        ):
                            raise ChallengeError(422, "SERVICE_REFEREE_STATE_SHAPE")
                        span = info["data_offsets"]
                        if (
                            not isinstance(span, list)
                            or len(span) != 2
                            or type(span[0]) is not int
                            or type(span[1]) is not int
                            or not 0 <= span[0] <= span[1] <= len(data) - 8 - size
                            or span[1] - span[0]
                            != (8 if tensor == "step" else 4 * math.prod(shape))
                        ):
                            raise ChallengeError(422, "SERVICE_REFEREE_STATE_OFFSETS")
                        spans.append((span[0], span[1]))
                    cursor = 0
                    for begin, end in sorted(spans):
                        if begin != cursor:
                            raise ChallengeError(422, "SERVICE_REFEREE_STATE_OFFSETS")
                        cursor = end
                    if cursor != len(data) - 8 - size:
                        raise ChallengeError(422, "SERVICE_REFEREE_STATE_OFFSETS")
                dataset = self._record_v2(run_id, "dataset", "inputs")
                for key in (dataset["samples_hash"], dataset["proofs_hash"]):
                    data = bounded_bytes(self.objects._path(str(key)), 65536)
                    if sha256_hex(data) != key:
                        raise ChallengeError(422, "SERVICE_REFEREE_DATASET_HASH")
                for name in ("samples", "sample_proofs"):
                    bounded_bytes(published.parent / preflight_job.object_paths[name], 65536)
                from hypertrain.auditor.island_bisect import TraceEntry

                for rank in range(manifest.training.reference_spec.layout.n_gpus):
                    trace_data = json.loads(
                        bounded_bytes(published / f"rank-{rank}" / "trace.json", 65536)
                    )
                    if not isinstance(trace_data, list) or len(trace_data) > 4096:
                        raise ChallengeError(422, "SERVICE_REFEREE_TRACE_SIZE")
                    for entry in trace_data:
                        trace_entry = TraceEntry.model_validate(entry)
                        if (
                            trace_entry.rank != rank
                            or not 0 <= trace_entry.step <= manifest.training.inner.H
                            or any(
                                dim < 0
                                or dim
                                > max(
                                    manifest.training.model.seq_len,
                                    manifest.training.model.d_model,
                                    manifest.training.model.vocab,
                                )
                                * 4
                                for dim in trace_entry.shape
                            )
                            or math.prod(trace_entry.shape) * 4 > 65536
                        ):
                            raise ChallengeError(422, "SERVICE_REFEREE_TRACE_SHAPE")
                # Trace decoding, anchor restore and replay stay beyond the profile guard.
                self._service_boundary_v2(run_id, turn.contest.w, heavy=True)
            geometry = self._record_v2(run_id, "trace-geometry", turn.contest.verdict_hash)
            job = IslandJobV1.model_validate(geometry["job"])
            if job.run_id != run_id or job.w != turn.contest.w or job.manifest != manifest:
                raise ChallengeError(409, "historical referee geometry subject differs")
            self._restore_anchor_v2(run_id, turn.contest.miner, turn.contest.w - 1, AnchorCache())
            miner = IslandParty.published(turn.contest.miner, job, Path(str(geometry["directory"])))
            original = Path(str(geometry["directory"])).parent
            # Published claim must match the independently accepted ordinary replay.
            accepted = self._record_v2(run_id, "verdict", f"{turn.contest.w}:{turn.contest.miner}")[
                "body"
            ]
            directory = self.state_dir / "referee-v2" / dispute_id / turn.transcript_hash
            directory.mkdir(parents=True, exist_ok=True)
            for relative in job.object_paths.values():
                source = original / relative
                target = directory / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
            snapshot = turn.transcript_hash
            deadline = manifest.training.beacon.genesis_time + (turn.absolute_deadline - 1) * 3
            replay_job = job.model_copy(update={"deadline": deadline})
            descriptor: dict[str, JsonValue] = {
                "job": replay_job.body(),
                "historical_geometry_hash": body_digest(geometry),
                "dispute_id": dispute_id,
                "transcript_hash": snapshot,
                "seq": turn.seq,
                "referee": turn.contest.referee,
                "absolute_deadline": turn.absolute_deadline,
                "backend": backend,
            }
            descriptor_hash = self.objects.put(canonicalize(descriptor))
            key = f"{dispute_id}:{snapshot}"
            old = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='referee-job' AND id=?",
                (run_id, key),
            ).fetchone()
            if old is not None and json.loads(old[0])["descriptor_hash"] != descriptor_hash:
                raise ChallengeError(409, "immutable referee descriptor conflicts")
            if old is not None:
                receipt_raw = json.loads(old[0])["receipt"]
                receipt = envelope_v2.parse_envelope(receipt_raw)
                if (
                    receipt.type != "Receipt"
                    or receipt.run_id != run_id
                    or receipt.signer != self._coord().ss58
                    or receipt.body["commit_hash"] != descriptor_hash
                    or receipt.body["w"] != turn.contest.w
                    or not envelope_v2.verify_envelope(receipt_raw)
                ):
                    raise ChallengeError(409, "accepted referee descriptor signature differs")
            if old is None:
                self._put_record_v2(
                    run_id,
                    "referee-job",
                    key,
                    {
                        "descriptor_hash": descriptor_hash,
                        "receipt": envelope_v2.seal(
                            self._coord(),
                            "Receipt",
                            run_id,
                            {
                                "w": turn.contest.w,
                                "commit_hash": descriptor_hash,
                                "received_round": self._now(self._db),
                            },
                            NEVER,
                        ),
                    },
                )
            guard = LeaseGuard(turn.absolute_deadline, turn.absolute_deadline, deadline)
            self._lease_guards_v2[key] = guard
        try:
            with guard:
                guard.check()
                launch = (
                    self._role_launch(
                        "referee",
                        replay_job,
                        directory,
                        {
                            "run_id": run_id,
                            "dispute_id": dispute_id,
                            "turn": turn.model_dump(mode="json"),
                            "descriptor_hash": descriptor_hash,
                            "descriptor": descriptor,
                            "descriptor_receipt": self._record_v2(run_id, "referee-job", key)[
                                "receipt"
                            ],
                            "historical_geometry": geometry,
                        },
                    )
                    if self._role_launch is not None
                    else launch_island
                )
                artifacts = launch(
                    replay_job, directory, backend=backend, trace=True, cancel=guard.cancelled
                )
                guard.check()
        finally:
            self._lease_guards_v2.pop(key, None)
        referee = IslandParty.published(turn.contest.referee, replay_job, artifacts.directory)
        # MATCH ordinary audit already authenticated these full trajectory commitments.
        auditor = IslandParty.published(turn.contest.auditor, replay_job, artifacts.directory)
        if turn.level == "op" and turn.paused and turn.interval[1] - turn.interval[0] == 1:
            from hypertrain.auditor.island_bisect import RefereeEvidence
            from hypertrain.protocol.messages_v2 import BisectV2

            with self._lock:
                answers = [
                    BisectV2.model_validate_json(r[0])
                    for r in self._db.execute(
                        "SELECT body FROM dispute_entries_v2 WHERE id=? ORDER BY seq DESC LIMIT 2",
                        (dispute_id,),
                    )
                ]
            if (
                len(answers) != 2
                or {a.party for a in answers} != {turn.contest.miner, turn.contest.auditor}
                or any(
                    a.level != "op" or a.ctx != turn.ctx or a.interval != turn.interval
                    for a in answers
                )
            ):
                raise ChallengeError(409, "final op lacks both exact signed claimant outputs")
            lo, hi = turn.interval
            at = [lo, -(-(lo + hi) // 2), hi]
            truth = referee.hashes("op", tuple(turn.ctx), at)
            if any(a.hashes[0] != truth[0] for a in answers):
                raise ChallengeError(
                    409, "independent full replay does not reproduce agreed op input"
                )
            wrong = [a.party for a in answers if a.hashes[-1] != truth[-1]]
            if len(wrong) != 1:
                raise ChallengeError(409, "independent op cannot identify one wrong claimant")
            evidence = RefereeEvidence(
                dispute_id=dispute_id,
                transcript_hash=snapshot,
                reason="FRAUD",
                loser=wrong[0],
                predecessor_hash=referee.state_root(turn.ctx[0] - 1),
                inputs_hash=truth[0],
                output_hash=truth[-1],
                op_spec=referee.op_name(tuple(turn.ctx), hi),
            )
        else:
            if accepted["result"] != "MATCH":
                raise ChallengeError(
                    409, "mismatch contest must reach exact signed final-op outputs"
                )
            evidence = adjudicate((dispute_id, snapshot), (miner, auditor), referee)
        with self._tx():
            self._snapshot_current_v2(manifest)
            current = disputes.get(dispute_id)
            if (
                current.transcript_hash != snapshot
                or self._now(self._db) >= current.absolute_deadline
            ):
                raise ChallengeError(409, "referee transcript changed during independent execution")
            digest = disputes.register_evidence(evidence)
            self._put_record_v2(
                run_id,
                "referee-artifacts",
                digest,
                {
                    "descriptor_hash": descriptor_hash,
                    "directory": str(artifacts.directory),
                    "historical_geometry_hash": body_digest(geometry),
                },
            )
        return {"evidence_hash": digest, "evidence": evidence.body()}

    def dispute_timeout_v2(self, run_id: str, dispute_id: str, raw: bytes) -> dict[str, Any]:
        from hypertrain.challenge.disputes_v2 import Availability

        data = envelope_v2.load_json(raw)
        if (
            set(data) != {"witnesses"}
            or not isinstance(data["witnesses"], list)
            or len(data["witnesses"]) != 2
        ):
            raise ChallengeError(422, "two original signed permitted witness observations required")
        proofs = tuple(Availability.model_validate(x) for x in data["witnesses"])
        _, _, disputes = self._services(run_id)
        with self._tx():
            self._snapshot_current_v2(self._run_v2(run_id))
            result = disputes.timeout(dispute_id, (proofs[0], proofs[1]), now=self._now(self._db))
            if result.resolution is not None:
                disputes.settle_lock(dispute_id, now=self._now(self._db))
                w = result.contest.w
                status = self._record_v2(run_id, "finality", str(w))
                status["unresolved_disputes"] = self._db.execute(
                    "SELECT COUNT(*) FROM disputes_v2 "
                    "WHERE json_extract(turn,'$.contest.w')=? "
                    "AND json_extract(turn,'$.resolution') IS NULL",
                    (w,),
                ).fetchone()[0]
                self._put_record_v2(run_id, "finality", str(w), status)
        disputes.notify()
        self._notify_processes_v2()
        return result.body()

    def rollback_v2(self, run_id: str, raw: bytes) -> dict[str, Any]:
        """Bounded real inner recomputation, fresh replay, then one atomic repair commit."""
        from dataclasses import replace

        from hypertrain.aggregator.rollback_v2 import execute_repair_context
        from hypertrain.auditor.replay import AnchorCache
        from hypertrain.challenge.finality_v2 import RoundStatus, exclude, finish_rollback
        from hypertrain.protocol.messages import Rollback

        with self._tx():
            manifest = self._run_v2(run_id)
            model = self._authenticated_v2(run_id, raw, "Rollback", self._coord().ss58)
            assert isinstance(model, Rollback)
            if self._service_boundary_v2(run_id, model.w) is not None:
                preflight_first = self._record_v2(run_id, "applied", str(model.w))
                self._rollback_context_v2(
                    run_id,
                    model.w,
                    preflight_first["prev_state"],
                    preflight_first["predecessor_tape_hash"],
                    [],
                    body_digest(model.model_dump(mode="json")),
                    profile_request=raw,
                )
            existing = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='rollback' AND id=?",
                (run_id, str(model.w)),
            ).fetchone()
            if existing:
                saved = json.loads(existing[0])
                if saved["request"]["body"] != model.model_dump(mode="json"):
                    raise ChallengeError(409, "conflicting rollback")
                return saved["result"]
            self._snapshot_current_v2(manifest)
            statuses = []
            for w in (model.w, model.w + 1):
                self.require_roster_v2(run_id, w)
                status = RoundStatus(**self._record_v2(run_id, "finality", str(w)))
                statuses.append(status)
                if status.disposition == "SETTLED":
                    raise ChallengeError(409, "finalized late fraud is money-only")
                if not status.applied or status.unresolved_audits or status.unresolved_disputes:
                    raise ChallengeError(409, "rollback requires applied closed audit/dispute work")
            if model.recomputed != ["agg_w", "step_w", "agg_w1", "step_w1"]:
                raise ChallengeError(422, "rollback requires complete inner and outer work")
            old = self._record_v2(run_id, "applied", str(model.w + 1))
            if old.get("kind") == "repair":
                raise ChallengeError(
                    409, "already repaired history requires exact original rollback"
                )
            if model.old_theta_hash_w2 != old["theta_hash"]:
                raise ChallengeError(409, "rollback original theta subject differs")
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round' "
                "AND CAST(id AS INTEGER)>?",
                (run_id, model.w + 1),
            ).fetchone():
                raise ChallengeError(
                    409, "rollback cannot rewrite beyond speculative two-round window"
                )
            cause = self._rollback_exclusions_v2(run_id, model)
            snapshot_before_reservation = self._rollback_snapshot_v2(run_id)
            operation = body_digest(model.model_dump(mode="json"))
            reservation = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='repair-reservation' AND id=?",
                (run_id, operation),
            ).fetchone()
            if reservation is None:
                active = self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='repair-reservation' "
                    "AND json_extract(data,'$.state')='RUNNING'",
                    (run_id,),
                ).fetchone()
                if active:
                    raise ChallengeError(409, "repair reservation already occupied")
                now = self._now(self._db)
                self._put_record_v2(
                    run_id,
                    "repair-reservation",
                    operation,
                    {
                        "state": "RUNNING",
                        "created": now,
                        "absolute": now + 200,
                        "max_attempts": 2,
                        "source_revision": snapshot_before_reservation,
                        "attempts": 1,
                        "steps": 2
                        * manifest.training.inner.H
                        * sum(
                            len(self._record_v2(run_id, "tape-inputs", str(round_w))["inputs"])
                            - len(model.excluded)
                            for round_w in (model.w, model.w + 1)
                        ),
                    },
                )
            else:
                reserved = json.loads(reservation[0])
                if reserved["source_revision"] != snapshot_before_reservation:
                    raise ChallengeError(409, "rollback reserved source revision changed")
                if self._now(self._db) >= reserved["absolute"] or reserved["attempts"] >= 2:
                    raise ChallengeError(409, "repair reservation exhausted")
                reserved["attempts"] += 1
                self._put_record_v2(run_id, "repair-reservation", operation, reserved)
            snapshot = self._rollback_snapshot_v2(run_id)
            first_applied = self._record_v2(run_id, "applied", str(model.w))
            first_context = self._rollback_context_v2(
                run_id,
                model.w,
                first_applied["prev_state"],
                first_applied["predecessor_tape_hash"],
                cause,
                operation,
            )
        first = execute_repair_context(
            self.objects,
            manifest,
            first_context,
            self._coord().ss58,
            key=self._coord(),
        )
        checked_first = execute_repair_context(
            self.objects,
            manifest,
            first_context,
            self._coord().ss58,
            tape=first.tape,
        )
        first_hash = self.objects.put(first.tape.to_bytes())
        with self._tx():
            if self._rollback_snapshot_v2(run_id) != snapshot:
                raise ChallengeError(409, "rollback authority revision changed")
            second_context = self._rollback_context_v2(
                run_id,
                model.w + 1,
                first.tape.body.arithmetic.out_state,
                first_hash,
                cause,
                operation,
                anchors=checked_first.anchors,
            )
        second = execute_repair_context(
            self.objects,
            manifest,
            second_context,
            self._coord().ss58,
            key=self._coord(),
        )
        checked_second = execute_repair_context(
            self.objects,
            manifest,
            second_context,
            self._coord().ss58,
            tape=second.tape,
        )
        second_hash = self.objects.put(second.tape.to_bytes())
        if (model.new_theta_hash_w2, model.new_outer_state_hash) != (
            checked_second.tape.body.arithmetic.out_hashes["theta_hash"],
            checked_second.tape.body.arithmetic.out_state,
        ):
            raise ChallengeError(422, "signed rollback outputs differ from actual repaired work")
        with self._tx():
            self._snapshot_current_v2(manifest)
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='rollback' AND id=?",
                (run_id, str(model.w)),
            ).fetchone():
                return self._record_v2(run_id, "rollback", str(model.w))["result"]
            reserved = self._record_v2(run_id, "repair-reservation", operation)
            if self._now(self._db) >= reserved["absolute"]:
                raise ChallengeError(409, "repair absolute deadline expired")
            if self._rollback_snapshot_v2(run_id) != snapshot:
                raise ChallengeError(409, "rollback authority revision changed")
            for result, context, tape_hash, status in (
                (checked_first, first_context, first_hash, statuses[0]),
                (checked_second, second_context, second_hash, statuses[1]),
            ):
                body = result.tape.body.arithmetic
                original = self._record_v2(run_id, "applied", str(body.w))
                self._put_record_v2(run_id, "original-applied", str(body.w), original)
                self._put_record_v2(run_id, "repair-context", str(body.w), context)
                for e in cause:
                    self._put_record_v2(run_id, "excluded", f"{body.w}:{e['hotkey']}", e)
                applied = {
                    "tape_hash": tape_hash,
                    "out_state": body.out_state,
                    "theta_hash": body.out_hashes["theta_hash"],
                    "prev_state": body.prev_state,
                    "predecessor_tape_hash": body.predecessor_tape_hash,
                    "kind": "repair",
                }
                self._put_record_v2(run_id, "applied", str(body.w), applied)
                cache = AnchorCache()
                for anchor in result.anchors:
                    cache.entries[(run_id, anchor.hotkey, body.w, anchor.layout_hash)] = anchor
                    old_anchor = self._record_v2(run_id, "anchor", f"{body.w}:{anchor.hotkey}")
                    self._put_record_v2(
                        run_id, "original-anchor", f"{body.w}:{anchor.hotkey}", old_anchor
                    )
                    self._persist_anchor_v2(manifest, anchor, cache)
                if status.w == model.w:
                    excluded_status = exclude(status, model.cause_hashes[0])
                    repaired = finish_rollback(
                        excluded_status,
                        model,
                        model.new_theta_hash_w2,
                        model.new_outer_state_hash,
                    )
                    repaired = replace(repaired, state_hash=body.out_hashes["theta_hash"])
                else:
                    repaired = replace(
                        status,
                        disposition="EXCLUDED",
                        rollback_complete=True,
                        resolution_hash=model.cause_hashes[0],
                        state_hash=body.out_hashes["theta_hash"],
                    )
                self._put_record_v2(run_id, "finality", str(body.w), asdict(repaired))
            result_data: dict[str, Any] = {
                "status": "REPAIRED",
                "tape_hashes": [first_hash, second_hash],
                "theta_hash": model.new_theta_hash_w2,
                "outer_state_hash": model.new_outer_state_hash,
                "reward_units": 0,
            }
            self._put_record_v2(
                run_id,
                "rollback",
                str(model.w),
                {
                    "request": envelope_v2.parse_envelope(raw).model_dump(mode="json"),
                    "result": result_data,
                },
            )
            reserved["state"] = "COMPLETE"
            self._put_record_v2(run_id, "repair-reservation", operation, reserved)
        return result_data

    def _rollback_snapshot_v2(self, run_id: str) -> str:
        """Revision digest excludes only reservation bookkeeping, not live source authorities."""
        records = self._db.execute(
            "SELECT kind,id,data FROM records_v2 WHERE run_id=? AND kind!='repair-reservation' "
            "ORDER BY kind,id",
            (run_id,),
        ).fetchall()
        admissions = self._db.execute("SELECT * FROM admissions_v2 ORDER BY hotkey").fetchall()
        disputes = self._db.execute("SELECT * FROM disputes_v2 ORDER BY id").fetchall()
        intake = self._db.execute(
            "SELECT key,digest,envelope,accepted_beacon,receipt FROM accepted_v2 ORDER BY key"
        ).fetchall()
        origins = self._db.execute(
            "SELECT * FROM escrow_units ORDER BY origin,owner,bucket,ref"
        ).fetchall()
        return sha256_hex(
            canonicalize(
                [
                    [
                        [
                            {"bytes": value.hex()} if isinstance(value, bytes) else value
                            for value in row
                        ]
                        for row in rows
                    ]
                    for rows in (records, admissions, disputes, intake, origins)
                ]
            )
        )

    def _rollback_exclusions_v2(self, run_id: str, model: Any) -> list[dict[str, Any]]:
        """Only accepted independent dispute resolutions justify removing applied contributions."""
        evidence = []
        for hotkey in model.excluded:
            rows = self._db.execute(
                "SELECT turn FROM disputes_v2 WHERE miner=? AND json_extract(turn,'$.contest.w')=?",
                (hotkey, model.w),
            ).fetchall()
            matched = []
            for row in rows:
                turn = json.loads(row[0])
                resolution = turn["resolution"]
                if resolution is None:
                    continue
                digest = body_digest(resolution)
                if digest not in model.cause_hashes:
                    continue
                if (
                    turn["contest"]["miner"] != hotkey
                    or resolution["reason"]
                    not in (
                        "FRAUD",
                        "WITHHELD",
                        "PARTY_TIMEOUT",
                    )
                    or resolution["loser"] != hotkey
                ):
                    raise ChallengeError(409, "rollback cause does not exclude this miner")
                signed = self._record_v2(run_id, "resolution", turn["dispute_id"])
                policy = DisputePolicyV2.model_validate_json(
                    self.objects.get(self._run_v2(run_id).network.dispute_policy_hash)
                )
                if (
                    not envelope_v2.verify_envelope(signed)
                    or signed["body"] != resolution
                    or (
                        signed["signer"] != turn["contest"]["referee"]
                        or signed["signer"] not in policy.referees
                        or signed["type"] != "ResolutionV2"
                        or signed["run_id"] != run_id
                        or turn["run_id"] != run_id
                    )
                ):
                    raise ChallengeError(409, "rollback cause lacks original referee signature")
                independent = self._db.execute(
                    "SELECT body FROM dispute_evidence_v2 WHERE hash=?",
                    (resolution["evidence_hash"],),
                ).fetchone()
                if independent is None:
                    raise ChallengeError(409, "rollback cause lacks independent referee work")
                matched.append(digest)
            if len(matched) != 1:
                raise ChallengeError(409, "rollback exclusion lacks exact accepted referee cause")
            evidence.append({"hotkey": hotkey, "reason": "FRAUD", "evidence_hash": matched[0]})
        if {e["evidence_hash"] for e in evidence} != set(model.cause_hashes):
            raise ChallengeError(409, "rollback cause set differs")
        return evidence

    def rollback_preview_v2(self, run_id: str, raw: bytes) -> dict[str, Any]:
        """Compute a bounded repair proposal; cannot change applied/finality/reward records."""
        from hypertrain.aggregator.rollback_v2 import execute_repair_context
        from hypertrain.protocol.messages import Rollback

        with self._tx():
            manifest = self._run_v2(run_id)
            model = envelope_v2.Intake(
                run_id,
                {"Rollback": self._coord().ss58.__eq__},
            ).accept(raw, self._now(self._db))
            assert isinstance(model, Rollback)
            if self._service_boundary_v2(run_id, model.w) is not None:
                first = self._record_v2(run_id, "applied", str(model.w))
                self._rollback_context_v2(
                    run_id,
                    model.w,
                    first["prev_state"],
                    first["predecessor_tape_hash"],
                    [],
                    body_digest(model.model_dump(mode="json")),
                    profile_request=raw,
                )
            causes = self._rollback_exclusions_v2(run_id, model)
            operation = sha256_hex(
                b"repair-preview|" + bytes.fromhex(body_digest(model.model_dump(mode="json")))
            )
            now = self._now(self._db)
            previous_preview = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='repair-preview' AND id=?",
                (run_id, operation),
            ).fetchone()
            if previous_preview:
                return json.loads(previous_preview[0])
            reserved = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='repair-reservation' AND id=?",
                (run_id, operation),
            ).fetchone()
            if reserved:
                raise ChallengeError(409, "repair preview reservation consumed")
            self._put_record_v2(
                run_id,
                "repair-reservation",
                operation,
                {
                    "state": "PREVIEW",
                    "created": now,
                    "absolute": now + 200,
                    "max_attempts": 1,
                    "attempts": 1,
                },
            )
            revision = self._rollback_snapshot_v2(run_id)
            first = self._record_v2(run_id, "applied", str(model.w))
            context = self._rollback_context_v2(
                run_id,
                model.w,
                first["prev_state"],
                first["predecessor_tape_hash"],
                causes,
                operation,
            )
        computed = execute_repair_context(
            self.objects, manifest, context, self._coord().ss58, key=self._coord()
        )
        first_hash = self.objects.put(computed.tape.to_bytes())
        with self._tx():
            if self._rollback_snapshot_v2(run_id) != revision:
                raise ChallengeError(409, "repair preview source revision changed")
            context = self._rollback_context_v2(
                run_id,
                model.w + 1,
                computed.tape.body.arithmetic.out_state,
                first_hash,
                causes,
                operation,
                anchors=computed.anchors,
            )
        result = execute_repair_context(
            self.objects, manifest, context, self._coord().ss58, key=self._coord()
        )
        answer = {
            "new_theta_hash_w2": result.tape.body.arithmetic.out_hashes["theta_hash"],
            "new_outer_state_hash": result.tape.body.arithmetic.out_state,
        }
        with self._tx():
            if self._rollback_snapshot_v2(run_id) != revision:
                raise ChallengeError(409, "repair preview source revision changed")
            self._put_record_v2(run_id, "repair-preview", operation, answer)
        return answer

    def _rollback_context_v2(
        self,
        run_id: str,
        w: int,
        prev_state: str,
        predecessor: str,
        excluded: list[dict[str, Any]],
        operation: str,
        *,
        anchors: Any = None,
        profile_request: bytes | None = None,
    ) -> dict[str, Any]:
        from hypertrain.aggregator.checkpoint import network_inputs
        from hypertrain.aggregator.tape_v2 import TapeV2
        from hypertrain.auditor.replay import AnchorCache, pack_state, tensor_root
        from hypertrain.protocol.messages_v2 import AuditJobV2
        from hypertrain.trainer.compress import state_hash

        manifest = self._run_v2(run_id)
        resource = self._service_boundary_v2(run_id, w)
        if resource is not None:
            from hypertrain.aggregator.capacity_worker import bounded_bytes
            from hypertrain.auditor.island_bisect import RefereeEvidence
            from hypertrain.challenge.disputes_v2 import Turn
            from hypertrain.data.store import LocalFSStore
            from hypertrain.protocol.envelope_v2 import tape_signing_message
            from hypertrain.protocol.messages import Rollback
            from hypertrain.protocol.messages_v2 import RoundOpenV2
            from hypertrain.trainer.config import TrainConfig
            from hypertrain.trainer.model import param_shapes

            if not isinstance(self.objects, LocalFSStore):
                raise ChallengeError(503, "SERVICE_ROLLBACK_LOCAL_STORE")
            now = self._now(self._db)
            if profile_request is None:
                raise ChallengeError(409, "SERVICE_ROLLBACK_REQUEST_REQUIRED")
            rollback_request = envelope_v2.Intake(
                run_id, {"Rollback": manifest.training.coord_pubkey.__eq__}
            ).accept(profile_request, now)
            assert isinstance(rollback_request, Rollback)
            if (
                rollback_request.w != w
                or body_digest(rollback_request.model_dump(mode="json")) != operation
                or rollback_request.recomputed != ["agg_w", "step_w", "agg_w1", "step_w1"]
            ):
                raise ChallengeError(409, "SERVICE_ROLLBACK_REQUEST_AUTHORITY")
            policy_raw = bounded_bytes(
                self.objects._path(manifest.network.dispute_policy_hash), 65536
            )
            if sha256_hex(policy_raw) != manifest.network.dispute_policy_hash:
                raise ChallengeError(422, "SERVICE_ROLLBACK_POLICY_HASH")
            policy = DisputePolicyV2.model_validate_json(policy_raw)
            causes = []
            for hotkey in rollback_request.excluded:
                rows = self._db.execute(
                    "SELECT turn FROM disputes_v2 WHERE miner=? "
                    "AND json_extract(turn,'$.contest.w')=?",
                    (hotkey, w),
                ).fetchall()
                matches = []
                for row in rows:
                    accepted_turn = Turn.model_validate_json(row[0])
                    resolution = accepted_turn.resolution
                    if (
                        resolution is None
                        or resolution.digest() not in rollback_request.cause_hashes
                    ):
                        continue
                    signed_resolution = self._record_v2(
                        run_id, "resolution", accepted_turn.dispute_id
                    )
                    resolved = envelope_v2.Intake(
                        run_id, {"ResolutionV2": accepted_turn.contest.referee.__eq__}
                    ).accept(canonicalize(signed_resolution), now)
                    evidence_row = self._db.execute(
                        "SELECT body FROM dispute_evidence_v2 WHERE hash=?",
                        (resolution.evidence_hash,),
                    ).fetchone()
                    if evidence_row is None:
                        raise ChallengeError(409, "SERVICE_ROLLBACK_EVIDENCE_MISSING")
                    evidence = RefereeEvidence.model_validate_json(evidence_row[0])
                    if (
                        accepted_turn.run_id != run_id
                        or accepted_turn.contest.miner != hotkey
                        or accepted_turn.contest.referee not in policy.referees
                        or resolved != resolution
                        or resolution.dispute_id != accepted_turn.dispute_id
                        or resolution.transcript_hash != accepted_turn.transcript_hash
                        or resolution.reason not in ("FRAUD", "WITHHELD", "PARTY_TIMEOUT")
                        or resolution.loser != hotkey
                        or evidence.digest() != resolution.evidence_hash
                        or evidence.dispute_id != accepted_turn.dispute_id
                        or evidence.transcript_hash != resolution.transcript_hash
                        or evidence.reason != resolution.reason
                        or evidence.loser != hotkey
                    ):
                        raise ChallengeError(409, "SERVICE_ROLLBACK_EVIDENCE_AUTHORITY")
                    artifact = self._record_v2(
                        run_id, "referee-artifacts", resolution.evidence_hash
                    )
                    current_descriptor = self._record_v2(
                        run_id,
                        "referee-job",
                        f"{accepted_turn.dispute_id}:{accepted_turn.transcript_hash}",
                    )
                    signed_receipt = envelope_v2.parse_envelope(current_descriptor["receipt"])
                    if (
                        artifact["descriptor_hash"] != current_descriptor["descriptor_hash"]
                        or signed_receipt.type != "Receipt"
                        or signed_receipt.run_id != run_id
                        or signed_receipt.signer != manifest.training.coord_pubkey
                        or signed_receipt.body["commit_hash"]
                        != current_descriptor["descriptor_hash"]
                        or not envelope_v2.verify_envelope(current_descriptor["receipt"])
                    ):
                        raise ChallengeError(409, "SERVICE_ROLLBACK_DESCRIPTOR_AUTHORITY")
                    descriptor_raw = bounded_bytes(
                        self.objects._path(str(current_descriptor["descriptor_hash"])), 1 << 20
                    )
                    descriptor = envelope_v2.load_json(descriptor_raw, max_bytes=1 << 20)
                    if (
                        sha256_hex(descriptor_raw) != current_descriptor["descriptor_hash"]
                        or descriptor["dispute_id"] != accepted_turn.dispute_id
                        or descriptor["transcript_hash"] != accepted_turn.transcript_hash
                        or descriptor["referee"] != accepted_turn.contest.referee
                    ):
                        raise ChallengeError(409, "SERVICE_ROLLBACK_DESCRIPTOR_BINDING")
                    matches.append(resolution.digest())
                if len(matches) != 1:
                    raise ChallengeError(409, "SERVICE_ROLLBACK_CAUSE_AUTHORITY")
                causes.extend(matches)
            if set(causes) != set(rollback_request.cause_hashes):
                raise ChallengeError(409, "SERVICE_ROLLBACK_CAUSE_SET")
            previous_tape = None
            for round_w in (w, w + 1):
                estimate = self._service_boundary_v2(run_id, round_w)
                assert estimate is not None
                opening = envelope_v2.Intake(
                    run_id, {"RoundOpenV2": manifest.training.coord_pubkey.__eq__}
                ).accept(canonicalize(self._record_v2(run_id, "round", str(round_w))), now)
                assert isinstance(opening, RoundOpenV2)
                if opening.w != round_w or opening.roster_hash != estimate["roster_hash"]:
                    raise ChallengeError(409, "SERVICE_ROLLBACK_ROSTER")
                original = self._record_v2(run_id, "applied", str(round_w))
                if (
                    round_w == w + 1
                    and original["theta_hash"] != rollback_request.old_theta_hash_w2
                ):
                    raise ChallengeError(409, "SERVICE_ROLLBACK_OLD_THETA")
                tape_raw = bounded_bytes(self.objects._path(str(original["tape_hash"])), 1 << 20)
                if sha256_hex(tape_raw) != original["tape_hash"]:
                    raise ChallengeError(422, "SERVICE_ROLLBACK_TAPE_HASH")
                signed_tape = TapeV2.from_bytes(tape_raw)
                tape_body = signed_tape.body
                if (
                    signed_tape.signer != manifest.training.coord_pubkey
                    or tape_body.run_id != run_id
                    or tape_body.w != round_w
                    or not verify(
                        decode_hotkey(signed_tape.signer),
                        tape_signing_message(tape_body.body(), run_id),
                        bytes.fromhex(signed_tape.sig),
                    )
                    or tape_body.policy_hash != manifest.network.aggregation_policy_hash
                    or tape_body.policy_hashes.body() != opening.policy_hashes.body()
                    or tape_body.prev_state != original["prev_state"]
                    or tape_body.predecessor_tape_hash != original["predecessor_tape_hash"]
                    or tape_body.out_state != original["out_state"]
                    or tape_body.out_hashes["theta_hash"] != original["theta_hash"]
                    or {x.roster.hotkey for x in tape_body.inputs}
                    | {x.hotkey for x in tape_body.excluded}
                    != {x.hotkey for x in opening.roster}
                    or any(x.roster not in opening.roster for x in tape_body.inputs)
                    or (
                        previous_tape is None
                        and (
                            tape_body.prev_state != prev_state
                            or tape_body.predecessor_tape_hash != predecessor
                        )
                    )
                    or (
                        previous_tape is not None
                        and (
                            tape_body.prev_state != previous_tape[1]
                            or tape_body.predecessor_tape_hash != previous_tape[0]
                        )
                    )
                ):
                    raise ChallengeError(409, "SERVICE_ROLLBACK_TAPE_AUTHORITY")
                previous_tape = (original["tape_hash"], tape_body.out_state)
                data = bounded_bytes(self.objects._path(tape_body.prev_state), 65536)
                if sha256_hex(data) != tape_body.prev_state:
                    raise ChallengeError(422, "SERVICE_ROLLBACK_STATE_HASH")
                if len(data) < 8 or struct.unpack_from("<Q", data)[0] > len(data) - 8:
                    raise ChallengeError(422, "SERVICE_ROLLBACK_STATE_HEADER")
                size = struct.unpack_from("<Q", data)[0]
                header = envelope_v2.load_json(data[8 : 8 + size], max_bytes=65536)
                shapes = param_shapes(TrainConfig.from_manifest_v2(manifest).model)
                expected = {
                    f"{part}/{name}": list(shape)
                    for part in ("theta", "u", "center")
                    for name, shape in shapes.items()
                }
                if not isinstance(header, dict) or set(header) != set(expected):
                    raise ChallengeError(422, "SERVICE_ROLLBACK_STATE_NAMES")
                spans: list[tuple[int, int]] = []
                for name, shape in expected.items():
                    info = header[name]
                    if (
                        not isinstance(info, dict)
                        or set(info) != {"dtype", "shape", "data_offsets"}
                        or info["shape"] != shape
                        or not isinstance(info["shape"], list)
                        or any(type(dim) is not int for dim in info["shape"])
                        or info["dtype"] != "F32"
                    ):
                        raise ChallengeError(422, "SERVICE_ROLLBACK_STATE_SHAPE")
                    span = info["data_offsets"]
                    if (
                        not isinstance(span, list)
                        or len(span) != 2
                        or type(span[0]) is not int
                        or type(span[1]) is not int
                        or not 0 <= span[0] <= span[1] <= len(data) - 8 - size
                        or span[1] - span[0] != 4 * math.prod(shape)
                    ):
                        raise ChallengeError(422, "SERVICE_ROLLBACK_STATE_OFFSETS")
                    spans.append((span[0], span[1]))
                cursor = 0
                for begin, end in sorted(spans):
                    if begin != cursor:
                        raise ChallengeError(422, "SERVICE_ROLLBACK_STATE_OFFSETS")
                    cursor = end
                if cursor != len(data) - 8 - size:
                    raise ChallengeError(422, "SERVICE_ROLLBACK_STATE_OFFSETS")
            dataset = self._record_v2(run_id, "dataset", "inputs")
            for key in (dataset["samples_hash"], dataset["proofs_hash"]):
                data = bounded_bytes(self.objects._path(str(key)), 65536)
                if sha256_hex(data) != key:
                    raise ChallengeError(422, "SERVICE_ROLLBACK_DATASET_HASH")
            self._service_boundary_v2(run_id, w, heavy=True)
        history = []
        if w > 0 and anchors is None:
            if w > 16:
                raise ChallengeError(
                    409, "bounded authenticated predecessor replay exceeds16rounds"
                )
            for prior_w in range(w):
                applied = self._record_v2(run_id, "applied", str(prior_w))
                entry = {
                    **applied,
                    "round_open": self._record_v2(run_id, "round", str(prior_w)),
                    "inputs": self._record_v2(run_id, "tape-inputs", str(prior_w))["inputs"],
                    "reference_reward_units": self._services(run_id)[0].policy.R_collectible_units,
                }
                if applied.get("kind") == "repair":
                    context = self._record_v2(run_id, "repair-context", str(prior_w))
                    entry["repair_context"] = context
                    entry["inputs"] = [s["work"] for s in context["body"]["sources"]]
                else:
                    jobs = []
                    for work in entry["inputs"]:
                        identity = f"{prior_w}:{work['roster']['hotkey']}"
                        replay = self._record_v2(run_id, "replay", identity)
                        rows = self._db.execute(
                            "SELECT data FROM records_v2 WHERE run_id=? AND kind='audit-job' "
                            "AND json_extract(data,'$.start_state.w')=? "
                            "AND json_extract(data,'$.start_state.hotkey')=?",
                            (run_id, prior_w, work["roster"]["hotkey"]),
                        ).fetchall()
                        matching = [
                            json.loads(r[0])
                            for r in rows
                            if AuditJobV2.model_validate(json.loads(r[0])).start_state.digest()
                            == replay["anchor_hash"]
                        ]
                        if len(matching) != 1:
                            raise ChallengeError(409, "authenticated predecessor audit job missing")
                        jobs.extend(matching)
                    entry["audit_jobs"] = jobs
                history.append(entry)
        source = self._record_v2(run_id, "applied", str(w))
        tape = TapeV2.from_bytes(self.objects.get(source["tape_hash"]))
        historical = self._record_v2(run_id, "tape-inputs", str(w))["inputs"]
        excluded_ids = {e["hotkey"] for e in excluded}
        if len(excluded_ids) != len(excluded) or not excluded_ids <= {
            x["roster"]["hotkey"] for x in historical
        }:
            raise ChallengeError(409, "rollback exclusion set not in original accepted inputs")
        retained = [x for x in historical if x["roster"]["hotkey"] not in excluded_ids]
        # Validate feasibility before launching any work; never silently truncate.
        from hypertrain.aggregator.weighted_v2 import Candidate, allocate_weights

        aggregation = AggregationPolicyV2.model_validate_json(
            self.objects.get(manifest.network.aggregation_policy_hash)
        )
        works = network_inputs(retained)
        allocate_weights([Candidate(x.roster, "0" * 64, "0" * 64) for x in works], aggregation)
        supplied = {a.hotkey: a for a in anchors} if anchors is not None else {}
        items = []
        backend = self._backend_v2(manifest)
        for work, data in zip(works, retained, strict=True):
            hotkey = work.roster.hotkey
            current = self._services(run_id)[1].status(hotkey, now=self._now(self._db))
            if not current.eligible or current.record.admission_id != work.roster.admission_id:
                raise ChallengeError(409, "repair retained contributor no longer funded/eligible")
            anchor = supplied.get(hotkey)
            if anchor is None:
                anchor = self._restore_anchor_v2(run_id, hotkey, w - 1, AnchorCache())
                start = self._record_v2(run_id, "start", f"{w}:{hotkey}")
                if (start["parent_anchor_hash"], start["anchor_verdict_hash"]) != (
                    anchor.anchor_hash,
                    anchor.proof_hash,
                ):
                    raise ChallengeError(409, "repair original carried anchor authority differs")
            import torch

            if any(
                not bool(torch.isfinite(x).all())
                for values in (
                    anchor.theta,
                    anchor.state.m,
                    anchor.state.v,
                    anchor.ef,
                )
                for x in values.values()
            ):
                raise ChallengeError(409, "nonfinite authenticated repair anchor")
            envelopes, receipts = {}, []
            for kind, obj in (("commit", work.commit), ("delta", work.delta_manifest)):
                envelope = self._record_v2(run_id, kind, f"{w}:{hotkey}")
                msg = "CommitV2" if kind == "commit" else "DeltaManifestV2"
                reservation = _dumps(list(envelope_v2.replay_key(msg, run_id, hotkey, obj)))
                row = self._db.execute(
                    "SELECT * FROM accepted_v2 WHERE key=?", (reservation,)
                ).fetchone()
                if row is None or row["digest"] != body_digest(envelope["body"]):
                    raise ChallengeError(409, "repair source lacks original durable intake")
                receipt = json.loads(row["receipt"])
                if (
                    not envelope_v2.verify_envelope(receipt)
                    or receipt["signer"] != self._coord().ss58
                    or (
                        receipt["type"] != "Receipt"
                        or receipt["run_id"] != run_id
                        or receipt["body"]["commit_hash"] != row["digest"]
                        or receipt["body"]["w"] != w
                        or receipt["body"]["received_round"] != row["accepted_beacon"]
                    )
                ):
                    raise ChallengeError(409, "repair source intake receipt authority differs")
                original = envelope_v2.parse_envelope(row["envelope"]).model_dump(mode="json")
                if original != envelope:
                    raise ChallengeError(409, "repair source original envelope changed")
                envelopes[kind] = self.objects.put(canonicalize(original))
                receipts.append(receipt)
            items.append(
                {
                    "work": data,
                    "commit_envelope_hash": envelopes["commit"],
                    "delta_envelope_hash": envelopes["delta"],
                    "acceptance_hash": sha256_hex(canonicalize(receipts)),
                    "samples": list(self._assignment_v2(run_id, w, hotkey)),
                    "anchor": {
                        "hotkey": hotkey,
                        "w": w - 1,
                        "layout_hash": anchor.layout_hash,
                        "proof_hash": anchor.proof_hash,
                        "anchor_hash": anchor.anchor_hash,
                        "backend": anchor.backend,
                        "state_object": self.objects.put(pack_state(anchor.theta, anchor.state)),
                        "ef_object": self.objects.put(pack_state(anchor.ef)),
                        "state_root": tensor_root(anchor.theta, anchor.state),
                        "ef_hash": state_hash(anchor.ef),
                    },
                    "receipts": receipts,
                }
            )
        ref = manifest.training.reference_spec
        reference_hash = sha256_hex(canonicalize(ref.model_dump(mode="json")))
        selection = self._record_v2(run_id, "execution-backend", "run")
        if selection["backend"] != backend:
            raise ChallengeError(409, "repair immutable backend authority differs")
        qualification: dict[str, Any] = {"execution_backend": selection}
        if backend == "cuda":
            reviewed = json.loads(self.objects.get(selection["authority_hash"]))
            if (
                reviewed.get("reference_hash") != reference_hash
                or reviewed.get("layout_hash") != ref.layout.model_dump_json()
            ):
                raise ChallengeError(409, "repair reviewed qualification subject differs")
            qualification["reviewed_qualification"] = reviewed
        repair_reservation = self._record_v2(run_id, "repair-reservation", operation)
        dataset = self._record_v2(run_id, "dataset", "inputs")
        exclusion_proofs = []
        for exclusion in excluded:
            row = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='resolution' "
                "AND json_extract(data,'$.body.loser')=?",
                (run_id, exclusion["hotkey"]),
            ).fetchall()
            signed = next(
                json.loads(r[0])
                for r in row
                if body_digest(json.loads(r[0])["body"]) == exclusion["evidence_hash"]
            )
            independent = self._db.execute(
                "SELECT body FROM dispute_evidence_v2 WHERE hash=?",
                (signed["body"]["evidence_hash"],),
            ).fetchone()
            exclusion_proofs.append(
                {"resolution": signed, "referee_evidence": json.loads(independent[0])}
            )
        body = {
            "v": "ht-rollback-context/1",
            "run_id": run_id,
            "w": w,
            "manifest": manifest.body(),
            "history": history,
            "source_tape_hash": source["tape_hash"],
            "prev_state": prev_state,
            "predecessor_tape_hash": predecessor,
            "sources": items,
            "excluded": excluded + [e.body() for e in tape.body.excluded],
            "exclusion_proofs": exclusion_proofs,
            "qualification_hash": sha256_hex(canonicalize(qualification)),
            "qualification": qualification,
            "reference_hash": reference_hash,
            "layout_hash": AnchorCache.layout_hash(manifest),
            "backend": backend,
            "reference_reward_units": self._services(run_id)[0].policy.R_collectible_units,
            "samples_hash": dataset["samples_hash"],
            "proofs_hash": dataset["proofs_hash"],
            "reservation": repair_reservation,
            "timeout_seconds": max(
                1, min(600, (repair_reservation["absolute"] - self._now(self._db)) * 3)
            ),
        }
        message = b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(body)).encode()
        return {
            "body": body,
            "signer": self._coord().ss58,
            "sig": self._coord().sign(message).hex(),
        }

    def upload_grant_v2(
        self, run_id: str, raw: bytes, commit_hash: str | None = None
    ) -> dict[str, Any]:
        from hypertrain.protocol import relay_envelope
        from hypertrain.protocol.messages_v2 import CommitV2
        from hypertrain.protocol.relay_messages import (
            CHUNK_BYTES,
            RelayAssignment,
            UploadChunk,
            UploadChunkManifest,
            UploadGrant,
        )

        # UploadChunkManifest is miner-signed in its relay domain, not a trust flag.
        env = relay_envelope.parse_envelope(raw)
        if commit_hash is not None and (
            len(commit_hash) != 64 or any(c not in "0123456789abcdef" for c in commit_hash)
        ):
            raise ChallengeError(422, "commit subject must be canonical body digest")
        with self._tx():
            manifest = self._run_v2(run_id)
            self._owner_v2(run_id, env.signer)
            chunk = relay_envelope.Intake(
                run_id, {"UploadChunkManifest": env.signer.__eq__}
            ).accept(raw, self._now(self._db))
            assert isinstance(chunk, UploadChunkManifest)
            assignments = self._db.execute(
                "SELECT id,data FROM records_v2 WHERE run_id=? "
                "AND kind='relay-assignment' AND id LIKE ?",
                (run_id, "%:" + env.signer),
            ).fetchall()
            eligible = []
            for row in assignments:
                assignment_env = relay_envelope.parse_envelope(json.loads(row["data"]))
                a = RelayAssignment.model_validate(assignment_env.body)
                if (
                    assignment_env.run_id != run_id
                    or assignment_env.type != "RelayAssignment"
                    or assignment_env.signer != self._coord().ss58
                    or not relay_envelope.verify_envelope(assignment_env.model_dump())
                    or a.hotkey != env.signer
                    or row["id"] != f"{a.w}:{env.signer}"
                    or assignment_env.exp_drand != a.exp_drand
                ):
                    raise ChallengeError(409, "relay assignment differs from accepted work subject")
                if self._now(self._db) >= a.exp_drand:
                    continue
                committed = self._db.execute(
                    "SELECT data FROM records_v2 WHERE run_id=? AND kind='commit' AND id=?",
                    (run_id, f"{a.w}:{env.signer}"),
                ).fetchone()
                if committed is None:
                    continue
                commit_env = envelope_v2.parse_envelope(json.loads(committed[0]))
                if commit_hash is not None and body_digest(commit_env.body) != commit_hash:
                    continue
                committed_work = CommitV2.model_validate(commit_env.body)
                if (
                    commit_env.run_id != run_id
                    or commit_env.signer != env.signer
                    or commit_env.type != "CommitV2"
                    or not envelope_v2.verify_envelope(commit_env.model_dump())
                    or committed_work.w != a.w
                    or committed_work.hotkey != env.signer
                ):
                    raise ChallengeError(409, "commit differs from signed relay assignment subject")
                if chunk.size != committed_work.delta_bytes:
                    continue
                try:
                    original = self.objects.get(committed_work.delta_hash)
                except ObjectNotFound:
                    continue
                if len(original) != chunk.size or sha256_hex(original) != committed_work.delta_hash:
                    raise ChallengeError(409, "accepted commit original delta corrupt")
                expected = UploadChunkManifest(
                    size=len(original),
                    chunks=[
                        UploadChunk(
                            index=i,
                            off=off,
                            len=len(original[off : off + CHUNK_BYTES]),
                            chunk_sha256=sha256_hex(original[off : off + CHUNK_BYTES]),
                        )
                        for i, off in enumerate(range(0, len(original), CHUNK_BYTES))
                    ],
                )
                if expected == chunk:
                    eligible.append(a)
            if len(eligible) != 1:
                raise ChallengeError(409, "one current signed relay assignment required")
            assignment = eligible[0]
            registry = self._registry_v2(manifest)
            spec = next(s for s in registry.specs if s.id == assignment.primary_id)
            now = self._now(self._db)
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='relay-draining' AND id=?",
                (run_id, spec.id),
            ).fetchone() is not None or not any(
                k.valid_from_round <= now < k.valid_until_round
                and self._db.execute(
                    "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='relay-disabled' AND id=?",
                    (run_id, f"{spec.id}:{assignment.assignment_epoch}:{k.key_id}"),
                ).fetchone()
                is None
                for k in spec.pubkeys
            ):
                raise ChallengeError(
                    409, "original relay drained or key disabled; use signed fallback"
                )
            commit = self._record_v2(run_id, "commit", f"{assignment.w}:{env.signer}")["body"]
            if chunk.size != commit["delta_bytes"]:
                raise ChallengeError(422, "grant size differs from accepted commit")
            grant = UploadGrant(
                w=assignment.w,
                hotkey=env.signer,
                relay_id=assignment.primary_id,
                assignment_epoch=assignment.assignment_epoch,
                delta_hash=str(commit["delta_hash"]),
                size=chunk.size,
                chunk_manifest_hash=chunk.chunk_manifest_hash(),
                retain_until=assignment.exp_drand
                + manifest.training.verify.E_vest_rounds
                + manifest.training.verify.T.dispute_per_level * 20
                + 100,
                nonce=sha256_hex(canonicalize([run_id, assignment.body(), chunk.body()])),
                exp_drand=assignment.exp_drand,
            )
            signed = relay_envelope.seal(
                self._coord(), "UploadGrant", run_id, grant, grant.exp_drand
            )
            self._put_record_v2(run_id, "grant", grant.digest(), signed)
            self._put_record_v2(run_id, "chunk-manifest", grant.digest(), chunk.body())
            return signed

    async def relay_receipt_v2(self, raw: bytes, client: Any) -> dict[str, Any]:
        import base64

        from hypertrain.data.stream_store import spool
        from hypertrain.protocol import relay_envelope
        from hypertrain.protocol.relay_messages import RelayReceipt, RetrievalRequest, UploadGrant
        from hypertrain.relay.client import RelayClient, receipt_verified

        env = relay_envelope.parse_envelope(raw)
        run_id = env.run_id
        receipt = RelayReceipt.model_validate(env.body)
        with self._tx():
            manifest = self._run_v2(run_id)
            grant = UploadGrant.model_validate(
                self._record_v2(run_id, "grant", receipt.grant_hash)["body"]
            )
            registry = self._registry_v2(manifest)
            body = self._record_v2(run_id, "round", str(grant.w))["body"]
            now = self._fresh_v2(
                manifest, f"relay-receipt:{receipt.digest()}:{self._now(self._db)}"
            )
            deadline = int(str(body["d_upload"]))
            receipt_verified(
                env,
                registry=registry,
                relay_id=grant.relay_id,
                run_id=run_id,
                grant=grant,
                received_before=deadline,
            )
            assignment = self._record_v2(run_id, "relay-assignment", f"{grant.w}:{grant.hotkey}")[
                "body"
            ]
            request = RetrievalRequest(
                request_id=secrets.token_hex(32),
                nonce=secrets.token_hex(32),
                relay_id=grant.relay_id,
                key_id=receipt.key_id,
                assignment_hash=body_digest(assignment),
                grant_hash=grant.digest(),
                custody_ack_hashes=[],
                receipt_hash=receipt.digest(),
                retention_hash=receipt.digest(),
                object_or_chunk_hash=grant.delta_hash,
                size=grant.size,
                requested_beacon=now,
                deadline_beacon=min(now + 10, receipt.retain_until),
            )
            request_raw = relay_envelope.parse_envelope(
                relay_envelope.seal(
                    self._coord(), "RetrievalRequest", run_id, request, request.deadline_beacon
                )
            )
            self._put_record_v2(
                run_id, "retrieval", request.digest(), request_raw.model_dump(mode="json")
            )
        spec = next(s for s in registry.specs if s.id == grant.relay_id)
        capability = base64.b64encode(
            canonicalize(self._record_v2(run_id, "grant", grant.digest()))
        ).decode()
        custody_response = await client.get(
            spec.https_url + f"/v1/uploads/{grant.digest()}/acks",
            headers={"X-Upload-Grant": capability},
        )
        custody_response.raise_for_status()
        custody = custody_response.json()
        relay = RelayClient(registry, client)
        with spool() as payload:
            response = await relay.retrieval(request_raw, payload, relay_public=env.signer)
            payload.seek(0)
            original = payload.read()
        with self._tx():
            current = self._fresh_v2(
                manifest, f"relay-accept:{receipt.digest()}:{self._now(self._db)}"
            )
            if (
                current >= deadline
                or sha256_hex(original) != grant.delta_hash
                or len(original) != grant.size
            ):
                raise ChallengeError(
                    422, "master independently retrieved mismatched or late payload"
                )
            self.objects.put(original)
            from hypertrain.protocol.relay_messages import MasterAcceptanceV2

            acceptance = MasterAcceptanceV2(
                w=grant.w,
                hotkey=grant.hotkey,
                delta_hash=grant.delta_hash,
                receipt_hash=receipt.digest(),
                received_drand=current,
            )
            signed = relay_envelope.seal(
                self._coord(), "MasterAcceptanceV2", run_id, acceptance, grant.retain_until
            )
            self._put_record_v2(
                run_id, "relay-receipt", receipt.digest(), env.model_dump(mode="json")
            )
            self._put_record_v2(run_id, "relay-custody", grant.digest(), {"artifacts": custody})
            self._put_record_v2(
                run_id, "retrieval-response", request.digest(), response.model_dump(mode="json")
            )
            self._put_record_v2(run_id, "master-acceptance", acceptance.digest(), signed)
            return signed
