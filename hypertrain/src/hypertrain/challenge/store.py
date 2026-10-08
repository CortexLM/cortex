"""SQLite (WAL) coordinator state: runs, roster, drand rounds, round state machine, intake, audits.

Every deadline is a drand round number; the only clock is the latest operator-pushed, verified
drand round D. Ledger events (todo 3 Ledger) carry the drand chain time of D. Route and JSON
contract: docs/challenge-routes.md.
"""

from __future__ import annotations

import hashlib
import json
import secrets
import sqlite3
import struct
import threading
from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
from fractions import Fraction
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from hypertrain.beacon import BeaconError, BeaconRound
from hypertrain.data.assignment import assign_round
from hypertrain.data.store import ObjectNotFound, StoreError
from hypertrain.data.store import Store as ObjectStore
from hypertrain.ledger import Ledger, LedgerError, Params
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

NEVER = 2**53 - 1  # exp_drand of container-signed records
LEASE_ROUNDS = 600  # 30 min of 3 s drand rounds
ZERO_STATE = sha256_hex(b"ht-zero-outer-state")
NO_HONEYPOT = sha256_hex(b"ht-no-honeypot")
FAULTS = frozenset({"MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"})
MINER_TYPES = frozenset({"Accept", "Commit", "DeltaManifest", "StateServe", "Dispute", "Bisect"})
COORD_TYPES = ("RoundOpen", "Rollback", "Finalize")

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


class ChallengeStore:
    def __init__(
        self,
        state_dir: Path,
        params: Params,
        coord: Keypair | None,
        owner_hotkey: str | None,
        verify_beacon: Callable[[Mapping[str, Any]], BeaconRound],
        objects: ObjectStore,
    ) -> None:
        state_dir.mkdir(parents=True, exist_ok=True)
        self.params = params
        self.coord = coord
        self.owner_hotkey = owner_hotkey
        self.verify_beacon = verify_beacon
        self.objects = objects
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

    @contextmanager
    def _tx(self) -> Iterator[sqlite3.Connection]:
        with self._lock:
            self._db.execute("BEGIN IMMEDIATE")
            try:
                yield self._db
            except BaseException:
                self._db.execute("ROLLBACK")
                raise
            self._db.execute("COMMIT")

    def healthy(self) -> bool:
        try:
            with self._tx() as db:
                db.execute("INSERT OR REPLACE INTO meta VALUES('health', '1')")
        except sqlite3.Error:
            return False
        return True

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
            return {"round": br.round, "latest": self._now(db)}

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
            if db.execute("SELECT 1 FROM runs").fetchone() is not None:
                # ponytail: one run per state dir because the ledger finalizes rounds 0,1,2..
                # globally; upgrade: a per-run ledger merged at get_weights.
                raise ChallengeError(409, "a run already exists in this challenge state")
            db.execute(
                "INSERT INTO runs VALUES(?, ?, ?, 'created', NULL)",
                (run_id, manifest.model_dump_json(), _dumps(dict(env))),
            )
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
        return [{"run_id": r["run_id"], "status": r["status"]} for r in rows]

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
            answer = self.ledger.get_weights(epoch, epoch_at, computed_at)
            last = self._meta(db, "last_answer_at", -1)
            self._set_meta(db, "last_answer_at", max(last, epoch_at))
        return answer
