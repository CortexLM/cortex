"""Conserved origin units. SQLite event journal and receipts commit with bucket changes.

No legacy entitlement import, external deposit, client mint or client slash exists.
The service supplies authenticated finality/replay and settlement from its own records.
"""

from __future__ import annotations

import json
import sqlite3
import threading
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, assert_never

from pydantic import BaseModel

from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.envelope_v2 import EnvelopeV2, Intake
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import decode_hotkey, verify
from hypertrain.protocol.messages import SS58, Finalize, ReplayVerdict
from hypertrain.protocol.messages_v2 import (
    U64,
    AuditChallengeV2,
    CommitV2,
    EconomicsPolicyV2,
    EscrowLock,
    EscrowOperation,
    EscrowReceipt,
    EscrowRelease,
    EscrowTransfer,
    Hex64,
    OriginAllocation,
    RewardFinalize,
    RunManifestV2,
    ShadowReservationRequestV1,
    ShadowReservationV1,
    ShadowRewardFinalizeV1,
    TestGenesis,
    WireModel,
)


class EscrowError(ValueError):
    def __init__(self, code: str) -> None:
        self.code = code
        super().__init__(code)


def digest(model: BaseModel) -> str:
    return body_digest(model.model_dump(mode="json"))


def authority_message(model: TestGenesis | RewardFinalize | ShadowRewardFinalizeV1) -> bytes:
    """Origin authority signature is independent of the enclosing envelope signer."""
    return (
        b"hypertrain/origin/2|"
        + type(model).__name__.encode()
        + b"|"
        + model.run_id.encode()
        + b"|"
        + body_digest(model.model_dump(mode="json", exclude={"authority_sig"})).encode()
    )


def genesis_allocation_hash(allocations: Sequence[tuple[str, int, int]]) -> str:
    """Hash owner/units/maturity first; origin IDs bind this hash without recursion."""
    return sha256_hex(canonicalize([list(a) for a in allocations], allow_float=False))


def genesis_origin_id(run_id: str, allocation_hash: str, index: int) -> str:
    return sha256_hex(f"ht-origin-genesis-v2|{run_id}|{allocation_hash}|{index}".encode())


def reward_origin_id(run_id: str, w: int, hotkey: str, finalize_hash: str) -> str:
    return sha256_hex(f"ht-origin-reward-v2|{run_id}|{w}|{hotkey}|{finalize_hash}".encode())


def shadow_origin_id(reservation: ShadowReservationV1) -> str:
    return sha256_hex(
        (
            f"ht-origin-shadow-v1|{reservation.run_id}|{reservation.shadow_ordinal}|"
            f"{reservation.trial_epoch}|{reservation.hotkey}|{reservation.finalize_hash}"
        ).encode()
    )


@dataclass(frozen=True, slots=True)
class RewardWork:
    """Accepted signed work, with ownership/assignment fetched from service state."""

    commit: EnvelopeV2
    verdict: EnvelopeV2
    challenge: EnvelopeV2
    owner: str
    sample_ids: tuple[int, ...]


@dataclass(frozen=True, slots=True)
class FinalityEvidence:
    finalize: EnvelopeV2
    works: tuple[RewardWork, ...]
    tape_hash: str
    shadow: bool
    finalized_beacon: int
    vesting_beacon: int


@dataclass(frozen=True, slots=True)
class ShadowReservationEvidence:
    """Original trial custody validated by trusted intake before auditor signing.

    The store resolves custody_hash from accepted original operation records.
    This input contains no acceptance receipt, auditor verdict or replay result.
    """

    finalize: EnvelopeV2
    commit: EnvelopeV2
    owner: str
    sample_ids: tuple[int, ...]
    custody_hash: str
    reference_final_state_hash: str


class Settlement(WireModel):
    """Internal L0/L5 acceptance record, never a client supplied unlock flag."""

    finality_hash: Hex64
    closed_dispute_root: Hex64
    release_beacon: U64
    unresolved: bool
    outcome: Literal["MATCH", "FRAUD", "INFRASTRUCTURE"]
    run_id: Hex64 | None = None
    admission_id: Hex64 | None = None
    coldkey: SS58 | None = None
    dispute_id: Hex64 | None = None
    evidence_hash: Hex64 | None = None


@dataclass(frozen=True, slots=True)
class Balances:
    issued: int
    available: int
    reward_pending: int
    admission_locked: int
    dispute_locked: int
    paid: int
    burned: int

    def conserved(self) -> bool:
        return self.issued == sum(
            (
                self.available,
                self.reward_pending,
                self.admission_locked,
                self.dispute_locked,
                self.paid,
                self.burned,
            )
        )


class EscrowV2:
    """Mutable accounting service; BEGIN IMMEDIATE serializes independent processes.

    Pass the service SQLite connection to share eligibility/rotation transactions.
    Independent replay and settlement callbacks are mandatory authority boundaries.
    """

    def __init__(
        self,
        database: Path | sqlite3.Connection,
        manifest: RunManifestV2,
        policy: EconomicsPolicyV2,
        *,
        auditors: frozenset[str],
        replay_finality: Callable[[FinalityEvidence], str],
        settlement: Callable[[str], Settlement],
        owner_of: Callable[[str], str],
        assignment_of: Callable[[int, str], tuple[int, ...]],
        shadow_assignment_of: Callable[[ShadowReservationV1], tuple[int, ...]] | None = None,
        shadow_settlement: Callable[[str], Settlement] | None = None,
    ) -> None:
        if digest(policy) != manifest.network.economics_policy_hash:
            raise EscrowError("ECONOMICS_POLICY_HASH")
        if policy.round_reward_units != manifest.training.budget.epochs_per_round * 1_000_000:
            raise EscrowError("MANIFEST_REWARD_BUDGET")
        self.manifest, self.policy, self.run_id = manifest, policy, manifest.run_id()
        if policy.reward_authority != manifest.training.coord_pubkey or auditors != frozenset(
            manifest.training.auditors
        ):
            raise EscrowError("PINNED_AUTHORITIES")
        self.auditors, self.replay_finality = auditors, replay_finality
        self.settlement, self.owner_of, self.assignment_of = settlement, owner_of, assignment_of
        self.shadow_assignment_of, self.shadow_settlement = shadow_assignment_of, shadow_settlement
        self.mutex = threading.RLock()
        match database:
            case Path():
                self.db = sqlite3.connect(database, isolation_level=None, check_same_thread=False)
            case sqlite3.Connection():
                self.db = database
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
          CREATE TABLE IF NOT EXISTS escrow_config(run_id TEXT PRIMARY KEY, policy TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS escrow_origins(
            origin TEXT PRIMARY KEY, issued INTEGER NOT NULL, mature_at INTEGER NOT NULL,
            reward_round INTEGER, hotkey TEXT);
          CREATE TABLE IF NOT EXISTS escrow_units(
            origin TEXT, owner TEXT, bucket TEXT, ref TEXT, units INTEGER NOT NULL CHECK(units>=0),
            PRIMARY KEY(origin,owner,bucket,ref));
          CREATE TABLE IF NOT EXISTS escrow_events(
            seq INTEGER PRIMARY KEY, operation_id TEXT UNIQUE, kind TEXT NOT NULL,
            request TEXT NOT NULL, prev_hash TEXT NOT NULL, event_hash TEXT NOT NULL,
            receipt TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS escrow_finalized(w INTEGER PRIMARY KEY, finalize_hash TEXT);
          CREATE TABLE IF NOT EXISTS escrow_shadow_finalized(
            ordinal INTEGER PRIMARY KEY, trial_epoch INTEGER, finalize_hash TEXT,
            reservation_hash TEXT UNIQUE, origin TEXT UNIQUE);
          CREATE TABLE IF NOT EXISTS admission_reservations(
            reservation TEXT PRIMARY KEY, digest TEXT NOT NULL, receipt TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS records_v2(
            run_id TEXT, kind TEXT, id TEXT, data TEXT NOT NULL,
            PRIMARY KEY(run_id,kind,id));
          CREATE TABLE IF NOT EXISTS escrow_snapshots(
            operation_id TEXT PRIMARY KEY, state TEXT NOT NULL);
        """)
        with self.tx():
            rows = self.db.execute("SELECT * FROM escrow_config").fetchall()
            if rows and (
                len(rows) != 1
                or rows[0]["run_id"] != self.run_id
                or rows[0]["policy"] != policy.model_dump_json()
            ):
                raise EscrowError("LEDGER_MODE_OR_RUN_CHANGED")
            self.db.execute(
                "INSERT OR IGNORE INTO escrow_config VALUES(?,?)",
                (self.run_id, policy.model_dump_json()),
            )
        self.verify()

    @contextmanager
    def tx(self) -> Iterator[sqlite3.Connection]:
        with self.mutex:
            nested = self.db.in_transaction
            self.db.execute("SAVEPOINT escrow" if nested else "BEGIN IMMEDIATE")
            try:
                yield self.db
            finally:
                # Commit only on normal exit; exception state is captured by contextlib.
                import sys

                failed = sys.exc_info()[0] is not None
                if nested:
                    if failed:
                        self.db.execute("ROLLBACK TO escrow")
                    self.db.execute("RELEASE escrow")
                else:
                    self.db.execute("ROLLBACK" if failed else "COMMIT")

    def _existing(self, operation_id: str, request: str) -> EscrowReceipt | None:
        row = self.db.execute(
            "SELECT request,receipt FROM escrow_events WHERE operation_id=?", (operation_id,)
        ).fetchone()
        if row is None:
            return None
        if row["request"] != request:
            raise EscrowError("OPERATION_CONFLICT")
        return EscrowReceipt.model_validate_json(row["receipt"])

    def _event(self, operation_id: str, kind: str, request: str, owner: str) -> EscrowReceipt:
        self._conservation()
        last = self.db.execute(
            "SELECT seq,event_hash FROM escrow_events ORDER BY seq DESC LIMIT 1"
        ).fetchone()
        seq, prev = (last["seq"] + 1, last["event_hash"]) if last else (0, "0" * 64)
        state = self._snapshot()
        event_hash = sha256_hex(
            canonicalize([self.run_id, seq, prev, operation_id, kind, request, state])
        )
        b = self.balances(owner)
        receipt = EscrowReceipt(
            operation_id=operation_id,
            event_hash=event_hash,
            ledger_mode=self.policy.ledger_mode,
            available_units=b.available,
            admission_locked_units=b.admission_locked,
            dispute_locked_units=b.dispute_locked,
        )
        self.db.execute(
            "INSERT INTO escrow_events VALUES(?,?,?,?,?,?,?)",
            (seq, operation_id, kind, request, prev, event_hash, receipt.model_dump_json()),
        )
        self.db.execute("INSERT INTO escrow_snapshots VALUES(?,?)", (operation_id, state))
        return receipt

    def _authority(self, model: TestGenesis | RewardFinalize | ShadowRewardFinalizeV1) -> None:
        if model.run_id != self.run_id or not verify(
            decode_hotkey(self.policy.reward_authority),
            authority_message(model),
            bytes.fromhex(model.authority_sig),
        ):
            raise EscrowError("ORIGIN_AUTHORITY")

    def _issue(self, origin: OriginAllocation, w: int | None, hotkey: str | None) -> None:
        if self.db.execute(
            "SELECT 1 FROM escrow_origins WHERE origin=?", (origin.origin_id,)
        ).fetchone():
            raise EscrowError("DUPLICATE_ORIGIN")
        self.db.execute(
            "INSERT INTO escrow_origins VALUES(?,?,?,?,?)",
            (origin.origin_id, origin.units, origin.mature_at, w, hotkey),
        )
        self._credit(origin.origin_id, origin.owner, "reward_pending", "", origin.units)

    def test_genesis(self, genesis: TestGenesis) -> EscrowReceipt:
        self._authority(genesis)
        with self.tx():
            request = genesis.model_dump_json(exclude={"authority_sig"})
            operation = sha256_hex(f"ht-genesis-v2|{self.run_id}".encode())
            old = self._existing(operation, request)
            if old is not None:
                return old
            if (
                self.policy.ledger_mode != "test"
                or self.db.execute("SELECT 1 FROM escrow_events LIMIT 1").fetchone()
            ):
                raise EscrowError("TEST_GENESIS_FORBIDDEN")
            alloc = [(o.owner, o.units, o.mature_at) for o in genesis.origins]
            if (
                genesis.allocation_hash != self.policy.genesis_allocation_hash
                or (genesis_allocation_hash(alloc) != genesis.allocation_hash)
                or genesis.total_units > self.policy.max_total_issuance
            ):
                raise EscrowError("GENESIS_ALLOCATION")
            for i, o in enumerate(genesis.origins):
                if o.origin_id != genesis_origin_id(self.run_id, genesis.allocation_hash, i):
                    raise EscrowError("GENESIS_ORIGIN_DOMAIN")
                self._issue(o, None, None)
            return self._event(operation, "TEST_GENESIS", request, genesis.origins[0].owner)

    def reward_allocations(
        self, reward: RewardFinalize | ShadowRewardFinalizeV1, evidence: FinalityEvidence
    ) -> tuple[OriginAllocation, ...]:
        """Revalidate actual signed finality/work. Never mint from a canary PASS flag."""
        self._authority(reward)
        if (
            evidence.finalize.type != "Finalize"
            or evidence.finalize.signer != self.policy.reward_authority
        ):
            raise EscrowError("FINALITY_AUTHORITY")
        final = Intake(
            self.run_id, {"Finalize": lambda s: s == self.policy.reward_authority}
        ).accept(evidence.finalize.model_dump(mode="json"), evidence.finalized_beacon)
        assert isinstance(final, Finalize)
        if (reward.finalize_hash, reward.w, reward.tape_hash) != (
            body_digest(evidence.finalize.body),
            final.w,
            evidence.tape_hash,
        ) or reward.mature_at < max(evidence.vesting_beacon, evidence.finalized_beacon):
            raise EscrowError("FINALITY_BINDING_OR_VESTING")
        reservation = None
        match reward:
            case ShadowRewardFinalizeV1():
                if not evidence.shadow:
                    raise EscrowError("REWARD_DOMAIN")
                reservation = self._shadow_reservation(reward)
                if (
                    evidence.finalized_beacon != reservation.finalized_beacon
                    or reward.w != reservation.trial_epoch
                    or reward.mature_at != reservation.mature_at
                    or final.included != [reservation.hotkey]
                ):
                    raise EscrowError("SHADOW_FINALITY_BINDING")
                if self.shadow_assignment_of is None or self.shadow_settlement is None:
                    raise EscrowError("SHADOW_AUTHORITY_UNAVAILABLE")
            case RewardFinalize():
                if evidence.shadow:
                    raise EscrowError("REWARD_DOMAIN")
            case unreachable:
                assert_never(unreachable)
        if self.replay_finality(evidence) != final.final_theta_hash_w1:
            raise EscrowError("FINALITY_REPLAY")
        work: dict[str, tuple[str, int]] = {}
        verdicts = []
        for item in evidence.works:

            def commit_authority(signer: str, expected: str = item.commit.signer) -> bool:
                return signer == expected

            c = Intake(self.run_id, {"CommitV2": commit_authority}).accept(
                item.commit.model_dump(mode="json"), evidence.finalized_beacon
            )
            v = Intake(self.run_id, {"ReplayVerdict": lambda s: s in self.auditors}).accept(
                item.verdict.model_dump(mode="json"), evidence.finalized_beacon
            )
            challenge = Intake(
                self.run_id, {"AuditChallengeV2": lambda s: s == self.policy.reward_authority}
            ).accept(item.challenge.model_dump(mode="json"), evidence.finalized_beacon)
            if not isinstance(c, CommitV2) or not isinstance(v, ReplayVerdict):
                raise EscrowError("REWARD_WORK_TYPE")
            if (
                not isinstance(challenge, AuditChallengeV2)
                or (challenge.w, challenge.target) != (c.w, c.hotkey)
                or v.challenge_hash != digest(challenge)
            ):
                raise EscrowError("REWARD_AUDIT_BINDING")
            c.validate_assignment(self.manifest, len(item.sample_ids))
            if (
                c.w != final.w
                or c.hotkey in work
                or c.hotkey not in final.included
                or item.sample_ids
                != (
                    self.shadow_assignment_of(reservation)
                    if reservation is not None and self.shadow_assignment_of is not None
                    else self.assignment_of(final.w, c.hotkey)
                )
                or item.owner != self.owner_of(c.hotkey)
                or item.verdict.signer == c.hotkey
                or v.result != "MATCH"
                or v.first_bad_leaf is not None
                or v.recomputed_leaves_root != c.leaves_root
                or v.replay_env.image_digest != self.manifest.training.reference_spec.image_digest
                or v.replay_env.driver not in self.manifest.training.reference_spec.driver_allowlist
            ):
                raise EscrowError("UNVERIFIED_REWARD_WORK")
            work[c.hotkey] = item.owner, c.tokens
            if reservation is not None and (
                digest(c) != reservation.commit_hash or item.owner != reservation.coldkey
            ):
                raise EscrowError("SHADOW_WORK_BINDING")
            verdicts.append(digest(v))
        if (
            not work
            or set(work) != set(final.included)
            or len(set(final.included)) != len(final.included)
        ):
            raise EscrowError("NO_VERIFIED_WORK_OR_EXCLUSIONS")
        if sha256_hex(canonicalize(verdicts)) != reward.verdict_root:
            raise EscrowError("VERDICT_ROOT")
        if reward.budget_units != self.policy.round_reward_units:
            raise EscrowError("ROUND_BUDGET")
        ordered = sorted(work, key=lambda h: h.encode())
        total = sum(work[h][1] for h in ordered)
        units = {h: reward.budget_units * work[h][1] // total for h in ordered}
        remainder = reward.budget_units - sum(units.values())
        for h in ordered[:remainder]:
            units[h] += 1
        origins = tuple(
            OriginAllocation(
                origin_id=(
                    shadow_origin_id(reservation)
                    if reservation is not None
                    else reward_origin_id(self.run_id, reward.w, h, reward.finalize_hash)
                ),
                owner=work[h][0],
                units=units[h],
                mature_at=reward.mature_at,
            )
            for h in ordered
            if units[h]
        )
        if reward.origin_ids != [o.origin_id for o in origins] or reward.allocation_hash != (
            sha256_hex(canonicalize([o.model_dump(mode="json") for o in origins]))
        ):
            raise EscrowError("REWARD_ALLOCATION")
        return origins

    def reserve_shadow(
        self, request: ShadowReservationRequestV1, evidence: ShadowReservationEvidence
    ) -> ShadowReservationV1:
        """Trusted intake reserves one immutable ordinal before external reward signing."""
        key = f"shadow-reserve|{self.run_id}|{request.admission_id}|{request.trial_epoch}"
        if self.db.in_transaction:
            raise EscrowError("SHADOW_RESERVATION_REQUIRES_COMMIT")
        with self.tx():
            if request.run_id != self.run_id:
                raise EscrowError("SHADOW_RESERVATION_DOMAIN")
            final = Intake(self.run_id, {"Finalize": self.policy.reward_authority.__eq__}).accept(
                evidence.finalize.model_dump(mode="json"), request.finalized_beacon
            )
            if not isinstance(final, Finalize) or (
                evidence.finalize.type != "Finalize"
                or final.w != request.trial_epoch
                or digest(final) != request.finalize_hash
                or final.included != [request.hotkey]
                or request.mature_at
                != (request.finalized_beacon + self.manifest.training.verify.E_vest_rounds)
                or evidence.custody_hash != request.custody_hash
            ):
                raise EscrowError("SHADOW_RESERVATION_BINDING")
            commit = Intake(self.run_id, {"CommitV2": request.hotkey.__eq__}).accept(
                evidence.commit.model_dump(mode="json"), request.finalized_beacon
            )
            if not isinstance(commit, CommitV2) or (
                evidence.commit.type != "CommitV2"
                or commit.w != final.w
                or commit.hotkey != request.hotkey
                or digest(commit) != request.commit_hash
                or evidence.owner != request.coldkey
            ):
                raise EscrowError("SHADOW_RESERVATION_WORK")
            commit.validate_assignment(self.manifest, len(evidence.sample_ids))
            semantic = digest(request)
            old = self.db.execute(
                "SELECT digest,receipt FROM admission_reservations WHERE reservation=?", (key,)
            ).fetchone()
            if old is not None:
                if old["digest"] != semantic:
                    raise EscrowError("SHADOW_RESERVATION_CONFLICT")
                return ShadowReservationV1.model_validate_json(old["receipt"])
            if (
                self.owner_of(commit.hotkey) != request.coldkey
                or evidence.reference_final_state_hash != final.final_theta_hash_w1
                or commit.final_theta_hash != final.final_theta_hash_w1
            ):
                raise EscrowError("SHADOW_RESERVATION_WORK")
            pending = self.db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='shadow-reservation' "
                "AND json_extract(data,'$.accepted') IS NULL",
                (self.run_id,),
            ).fetchone()
            if pending is not None:
                raise EscrowError("SHADOW_RESERVATION_PENDING")
            last = self.db.execute("SELECT MAX(ordinal) FROM escrow_shadow_finalized").fetchone()[0]
            ordinal = last + 1 if last is not None else 0
            if ordinal >= self.policy.shadow_bootstrap_rounds:
                raise EscrowError("BOOTSTRAP_FINISHED")
            existing = self.db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='shadow-reservation' AND id=?",
                (self.run_id, str(ordinal)),
            ).fetchone()
            if existing is not None:
                raise EscrowError("SHADOW_RESERVATION_CONFLICT")
            if (
                self.balances().issued + self.policy.round_reward_units
                > self.policy.max_total_issuance
            ):
                raise EscrowError("TOTAL_ISSUANCE_BUDGET")
            binding = ShadowReservationV1.model_validate(
                {**request.body(), "shadow_ordinal": ordinal}
            )
            self.db.execute(
                "INSERT INTO admission_reservations VALUES(?,?,?)",
                (key, semantic, binding.model_dump_json()),
            )
            self.db.execute(
                "INSERT INTO records_v2 VALUES(?,?,?,?)",
                (
                    self.run_id,
                    "shadow-reservation",
                    str(ordinal),
                    canonicalize({"binding": binding.body(), "accepted": None}).decode(),
                ),
            )
            return binding

    def _shadow_reservation(self, reward: ShadowRewardFinalizeV1) -> ShadowReservationV1:
        row = self.db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='shadow-reservation' AND id=?",
            (self.run_id, str(reward.shadow_ordinal)),
        ).fetchone()
        if row is None:
            raise EscrowError("SHADOW_RESERVATION_REQUIRED")
        binding = ShadowReservationV1.model_validate(json.loads(row["data"])["binding"])
        if (digest(binding), binding.trial_epoch, binding.finalize_hash, binding.mature_at) != (
            reward.reservation_hash,
            reward.w,
            reward.finalize_hash,
            reward.mature_at,
        ):
            raise EscrowError("SHADOW_RESERVATION_CONFLICT")
        return binding

    def reward_finalize(
        self, reward: RewardFinalize | ShadowRewardFinalizeV1, evidence: FinalityEvidence
    ) -> EscrowReceipt:
        with self.tx():
            # L0 callbacks read this same BEGIN IMMEDIATE snapshot; no authority
            # read may race ownership/finality revocation before bucket mutation.
            match reward:
                case ShadowRewardFinalizeV1():
                    operation = sha256_hex(
                        (
                            f"ht-shadow-finalize-v1|{self.run_id}|{reward.shadow_ordinal}|"
                            f"{reward.reservation_hash}"
                        ).encode()
                    )
                    self._authority(reward)
                    self._shadow_reservation(reward)
                    if not evidence.shadow:
                        raise EscrowError("REWARD_DOMAIN")
                case RewardFinalize():
                    operation = sha256_hex(
                        f"ht-reward-finalize-v2|{self.run_id}|{reward.w}".encode()
                    )
                case unreachable:
                    assert_never(unreachable)
            request = reward.model_dump_json(exclude={"authority_sig"})
            # Shadow replay authenticates the immutable operation before new authority predicates.
            origins = None
            match reward:
                case ShadowRewardFinalizeV1():
                    pass
                case RewardFinalize():
                    origins = self.reward_allocations(reward, evidence)
                case unreachable:
                    assert_never(unreachable)
            old = self._existing(operation, request)
            if old is not None:
                return old
            if origins is None:
                origins = self.reward_allocations(reward, evidence)
            match reward:
                case ShadowRewardFinalizeV1():
                    last = self.db.execute(
                        "SELECT MAX(ordinal) FROM escrow_shadow_finalized"
                    ).fetchone()[0]
                    number = reward.shadow_ordinal
                case RewardFinalize():
                    last = self.db.execute("SELECT MAX(w) FROM escrow_finalized").fetchone()[0]
                    number = reward.w
                case unreachable:
                    assert_never(unreachable)
            if number != (last + 1 if last is not None else 0):
                raise EscrowError("FINALIZE_ORDER")
            pending_units = 0
            match reward:
                case ShadowRewardFinalizeV1():
                    pass
                case RewardFinalize():
                    pending_units = (
                        self.policy.round_reward_units
                        * self.db.execute(
                            "SELECT COUNT(*) FROM records_v2 WHERE run_id=? "
                            "AND kind='shadow-reservation' "
                            "AND json_extract(data,'$.accepted') IS NULL",
                            (self.run_id,),
                        ).fetchone()[0]
                    )
                case unreachable:
                    assert_never(unreachable)
            if (
                self.balances().issued + pending_units + reward.budget_units
                > self.policy.max_total_issuance
            ):
                raise EscrowError("TOTAL_ISSUANCE_BUDGET")
            match reward:
                case ShadowRewardFinalizeV1():
                    binding = self._shadow_reservation(reward)
                    by_origin = {shadow_origin_id(binding): binding.hotkey}
                case RewardFinalize():
                    by_origin = {
                        reward_origin_id(
                            self.run_id,
                            reward.w,
                            CommitV2.model_validate(item.commit.body).hotkey,
                            reward.finalize_hash,
                        ): CommitV2.model_validate(item.commit.body).hotkey
                        for item in evidence.works
                    }
                case unreachable:
                    assert_never(unreachable)
            for o in origins:
                self._issue(o, reward.w, by_origin[o.origin_id])
            match reward:
                case ShadowRewardFinalizeV1():
                    self.db.execute(
                        "INSERT INTO escrow_shadow_finalized VALUES(?,?,?,?,?)",
                        (
                            reward.shadow_ordinal,
                            reward.w,
                            reward.finalize_hash,
                            reward.reservation_hash,
                            origins[0].origin_id,
                        ),
                    )
                    receipt = self._event(
                        operation, "SHADOW_REWARD_FINALIZE", request, origins[0].owner
                    )
                    binding = self._shadow_reservation(reward)
                    self.db.execute(
                        "UPDATE records_v2 SET data=? WHERE run_id=? "
                        "AND kind='shadow-reservation' AND id=?",
                        (
                            canonicalize(
                                {"binding": binding.body(), "accepted": receipt.body()}
                            ).decode(),
                            self.run_id,
                            str(reward.shadow_ordinal),
                        ),
                    )
                    return receipt
                case RewardFinalize():
                    self.db.execute(
                        "INSERT INTO escrow_finalized VALUES(?,?)", (reward.w, reward.finalize_hash)
                    )
                    return self._event(operation, "REWARD_FINALIZE", request, origins[0].owner)
                case unreachable:
                    assert_never(unreachable)

    def _credit(self, origin: str, owner: str, bucket: str, ref: str, units: int) -> None:
        self.db.execute(
            """INSERT INTO escrow_units VALUES(?,?,?,?,?) ON CONFLICT(origin,owner,bucket,ref)
                        DO UPDATE SET units=units+excluded.units""",
            (origin, owner, bucket, ref, units),
        )

    def _move(
        self,
        op: EscrowOperation,
        source: str,
        destination: str,
        *,
        source_ref: str = "",
        destination_ref: str = "",
        recipient: str | None = None,
    ) -> None:
        rows = self.db.execute(
            "SELECT * FROM escrow_units WHERE owner=? AND bucket=? AND ref=?",
            (op.owner, source, source_ref),
        ).fetchall()
        selected = sorted(
            (r for r in rows if r["origin"] in op.origin_ids), key=lambda r: r["origin"]
        )
        if {r["origin"] for r in selected} != set(op.origin_ids) or sum(
            r["units"] for r in selected
        ) < op.units:
            raise EscrowError("UNFUNDED_OR_UNKNOWN_ORIGIN")
        left = op.units
        for row in selected:
            take = min(left, row["units"])
            self.db.execute(
                "UPDATE escrow_units SET units=units-? WHERE origin=? AND owner=? "
                "AND bucket=? AND ref=?",
                (take, row["origin"], op.owner, source, source_ref),
            )
            self._credit(row["origin"], recipient or op.owner, destination, destination_ref, take)
            left -= take

    def _owner_operation(self, op: EscrowOperation, signer: str) -> EscrowReceipt | None:
        if signer != op.owner:
            raise EscrowError("OWNER_AUTHORITY")
        return self._existing(op.operation_id, op.model_dump_json())

    def lock(self, op: EscrowLock, *, signer: str) -> EscrowReceipt:
        with self.tx():
            old = self._owner_operation(op, signer)
            if old is not None:
                return old
            bucket = "admission_locked" if op.kind == "LOCK_ADMISSION" else "dispute_locked"
            self._move(op, "available", bucket, destination_ref=op.operation_id)
            return self._event(op.operation_id, op.kind, op.model_dump_json(), op.owner)

    def transfer(self, op: EscrowTransfer, *, signer: str) -> EscrowReceipt:
        with self.tx():
            old = self._owner_operation(op, signer)
            if old is not None:
                return old
            self._move(op, "available", "available", recipient=op.recipient)
            return self._event(op.operation_id, "TRANSFER", op.model_dump_json(), op.owner)

    def mature(
        self, operation_id: str, origin_ids: tuple[str, ...], now_beacon: int
    ) -> EscrowReceipt:
        """Server-only vesting; disputed reward origins remain pending."""
        if not origin_ids or len(set(origin_ids)) != len(origin_ids) or type(now_beacon) is not int:
            raise EscrowError("MATURITY_INPUT")
        request = canonicalize(list(origin_ids)).decode()
        with self.tx():
            old = self._existing(operation_id, request)
            if old is not None:
                return old
            owner = ""
            for origin in origin_ids:
                record = self.db.execute(
                    "SELECT * FROM escrow_origins WHERE origin=?", (origin,)
                ).fetchone()
                if record is None or now_beacon < record["mature_at"]:
                    raise EscrowError("IMMATURE_OR_UNKNOWN_ORIGIN")
                if record["reward_round"] is not None:
                    shadow = self.db.execute(
                        "SELECT finalize_hash FROM escrow_shadow_finalized WHERE origin=?",
                        (origin,),
                    ).fetchone()
                    if shadow is not None:
                        if self.shadow_settlement is None:
                            raise EscrowError("SHADOW_AUTHORITY_UNAVAILABLE")
                        status, finality = self.shadow_settlement(origin), shadow
                    else:
                        status = self.settlement(origin)
                        finality = self.db.execute(
                            "SELECT finalize_hash FROM escrow_finalized WHERE w=?",
                            (record["reward_round"],),
                        ).fetchone()
                    if (
                        status.unresolved
                        or status.outcome == "FRAUD"
                        or now_beacon < status.release_beacon
                        or finality is None
                        or status.finality_hash != finality["finalize_hash"]
                    ):
                        raise EscrowError("DISPUTED_OR_UNVESTED")
                rows = self.db.execute(
                    "SELECT * FROM escrow_units WHERE origin=? AND "
                    "bucket='reward_pending' AND units>0",
                    (origin,),
                ).fetchall()
                if not rows:
                    raise EscrowError("ALREADY_MATURED")
                for row in rows:
                    owner = row["owner"]
                    self.db.execute(
                        "UPDATE escrow_units SET units=0 WHERE origin=? AND owner=? "
                        "AND bucket='reward_pending'",
                        (origin, owner),
                    )
                    self._credit(origin, owner, "available", "", row["units"])
            return self._event(operation_id, "MATURE", request, owner)

    def release(self, op: EscrowRelease, *, signer: str, now_beacon: int) -> EscrowReceipt:
        with self.tx():
            old = self._owner_operation(op, signer)
            if old is not None:
                return old
            status = self.settlement(op.lock_id)
            if (
                status.unresolved
                or status.outcome == "FRAUD"
                or now_beacon < status.release_beacon
                or (op.finality_hash, op.closed_dispute_root)
                != (status.finality_hash, status.closed_dispute_root)
            ):
                raise EscrowError("DISPUTED_OR_UNVESTED")
            lock = self.db.execute(
                "SELECT request,kind FROM escrow_events WHERE operation_id=?", (op.lock_id,)
            ).fetchone()
            if lock is None or lock["kind"] not in ("LOCK_ADMISSION", "LOCK_CONTEST"):
                raise EscrowError("UNKNOWN_LOCK")
            original = EscrowLock.model_validate_json(lock["request"])
            if (op.admission_id, op.dispute_id, op.owner) != (
                original.admission_id,
                original.dispute_id,
                original.owner,
            ):
                raise EscrowError("LOCK_REFERENCE")
            source = "admission_locked" if original.kind == "LOCK_ADMISSION" else "dispute_locked"
            self._move(op, source, "available", source_ref=op.lock_id)
            return self._event(op.operation_id, "RELEASE", op.model_dump_json(), op.owner)

    def slash(
        self,
        op: EscrowRelease,
        *,
        authority: str,
        now_beacon: int,
        referee: str | None = None,
        referee_cost: int = 0,
        max_referee_cost: int = 0,
    ) -> EscrowReceipt:
        """Internal proven-fraud settlement, no client slash route; funded cost then burn."""
        if (
            authority != self.policy.reward_authority
            or min(referee_cost, max_referee_cost) < 0
            or referee_cost > min(max_referee_cost, op.units)
        ):
            raise EscrowError("SLASH_AUTHORITY_OR_COST")
        with self.tx():
            request = canonicalize(
                [op.model_dump(mode="json"), referee, referee_cost, max_referee_cost]
            ).decode()
            old = self._existing(op.operation_id, request)
            if old is not None:
                return old
            status = self.settlement(op.lock_id)
            if (
                status.unresolved
                or status.outcome != "FRAUD"
                or now_beacon < status.release_beacon
                or (op.finality_hash, op.closed_dispute_root)
                != (status.finality_hash, status.closed_dispute_root)
            ):
                raise EscrowError("UNSETTLED_SLASH")
            row = self.db.execute(
                "SELECT request,kind FROM escrow_events WHERE operation_id=?", (op.lock_id,)
            ).fetchone()
            if row is None or row["kind"] not in ("LOCK_ADMISSION", "LOCK_CONTEST"):
                raise EscrowError("UNKNOWN_LOCK")
            original = EscrowLock.model_validate_json(row["request"])
            if (op.admission_id, op.dispute_id, op.owner) != (
                original.admission_id,
                original.dispute_id,
                original.owner,
            ):
                raise EscrowError("LOCK_REFERENCE")
            source = "admission_locked" if original.kind == "LOCK_ADMISSION" else "dispute_locked"
            if referee_cost:
                if (
                    original.kind != "LOCK_CONTEST"
                    or referee is None
                    or not self.balances(referee).issued
                ):
                    raise EscrowError("UNFUNDED_REFEREE")
                cost = EscrowOperation.model_validate(
                    {
                        **op.model_dump(mode="json", include=set(EscrowOperation.model_fields)),
                        "units": referee_cost,
                    }
                )
                self._move(cost, source, "available", source_ref=op.lock_id, recipient=referee)
            if op.units > referee_cost:
                burn = EscrowOperation.model_validate(
                    {
                        **op.model_dump(mode="json", include=set(EscrowOperation.model_fields)),
                        "units": op.units - referee_cost,
                    }
                )
                self._move(burn, source, "burned", source_ref=op.lock_id)
            if original.kind == "LOCK_ADMISSION":
                for row in self.db.execute(
                    "SELECT * FROM escrow_units WHERE owner=? AND "
                    "bucket='reward_pending' AND units>0",
                    (op.owner,),
                ).fetchall():
                    self.db.execute(
                        "UPDATE escrow_units SET units=0 WHERE origin=? AND owner=? "
                        "AND bucket='reward_pending'",
                        (row["origin"], op.owner),
                    )
                    self._credit(row["origin"], op.owner, "burned", "", row["units"])
            return self._event(op.operation_id, "SLASH", request, op.owner)

    def pay(self, op: EscrowOperation, *, signer: str) -> EscrowReceipt:
        """Terminal unit sink, not a cash payout or redemption adapter."""
        with self.tx():
            old = self._owner_operation(op, signer)
            if old is not None:
                return old
            self._move(op, "available", "paid")
            return self._event(op.operation_id, "PAY", op.model_dump_json(), op.owner)

    def balances(self, owner: str | None = None) -> Balances:
        query = "SELECT bucket,SUM(units) total FROM escrow_units"
        rows = self.db.execute(
            query + (" WHERE owner=?" if owner else "") + " GROUP BY bucket",
            (owner,) if owner else (),
        ).fetchall()
        buckets = {r["bucket"]: r["total"] for r in rows}
        issued = (
            self.db.execute("SELECT COALESCE(SUM(issued),0) FROM escrow_origins").fetchone()[0]
            if owner is None
            else sum(buckets.values())
        )
        return Balances(
            issued,
            *(
                buckets.get(b, 0)
                for b in (
                    "available",
                    "reward_pending",
                    "admission_locked",
                    "dispute_locked",
                    "paid",
                    "burned",
                )
            ),
        )

    def locked(self, admission_id: str, owner: str) -> tuple[int, str]:
        units, hashes = 0, []
        for r in self.db.execute("SELECT * FROM escrow_events WHERE kind='LOCK_ADMISSION'"):
            op = EscrowLock.model_validate_json(r["request"])
            if op.admission_id == admission_id and op.owner == owner:
                amount = self.db.execute(
                    "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE "
                    "bucket='admission_locked' AND ref=? AND owner=?",
                    (op.operation_id, owner),
                ).fetchone()[0]
                if amount:
                    units += amount
                    hashes.append(r["event_hash"])
        return units, sha256_hex(canonicalize(hashes)) if hashes else "0" * 64

    def _conservation(self) -> None:
        for row in self.db.execute(
            "SELECT o.origin,o.issued,COALESCE(SUM(u.units),0) held FROM escrow_origins o "
            "LEFT JOIN escrow_units u ON o.origin=u.origin GROUP BY o.origin"
        ):
            if row["issued"] != row["held"]:
                raise EscrowError("ORIGIN_CONSERVATION")
        if not self.balances().conserved():
            raise EscrowError("CONSERVATION")

    def verify(self) -> None:
        """Check canonical chain/receipts and origin conservation after restart."""
        for expected, row in enumerate(
            self.db.execute("SELECT * FROM escrow_shadow_finalized ORDER BY ordinal")
        ):
            if row["ordinal"] != expected:
                raise EscrowError("SHADOW_FINALIZE_ORDER")
            record = self.db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='shadow-reservation' AND id=?",
                (self.run_id, str(expected)),
            ).fetchone()
            if record is None:
                raise EscrowError("SHADOW_RESERVATION_REQUIRED")
            data = json.loads(record["data"])
            binding = ShadowReservationV1.model_validate(data["binding"])
            if (
                digest(binding) != row["reservation_hash"]
                or binding.finalize_hash != row["finalize_hash"]
                or binding.trial_epoch != row["trial_epoch"]
                or shadow_origin_id(binding) != row["origin"]
                or data["accepted"] is None
            ):
                raise EscrowError("SHADOW_PERSISTED_BINDING")
            operation = sha256_hex(
                f"ht-shadow-finalize-v1|{self.run_id}|{expected}|{digest(binding)}".encode()
            )
            event = self.db.execute(
                "SELECT receipt FROM escrow_events WHERE operation_id=?", (operation,)
            ).fetchone()
            if event is None or json.loads(event["receipt"]) != data["accepted"]:
                raise EscrowError("SHADOW_PERSISTED_RECEIPT")
        prev = "0" * 64
        last_state = None
        for i, row in enumerate(self.db.execute("SELECT * FROM escrow_events ORDER BY seq")):
            snap = self.db.execute(
                "SELECT state FROM escrow_snapshots WHERE operation_id=?", (row["operation_id"],)
            ).fetchone()
            if snap is None:
                raise EscrowError("MISSING_EVENT_SNAPSHOT")
            last_state = snap["state"]
            actual = sha256_hex(
                canonicalize(
                    [
                        self.run_id,
                        i,
                        prev,
                        row["operation_id"],
                        row["kind"],
                        row["request"],
                        last_state,
                    ]
                )
            )
            receipt = EscrowReceipt.model_validate_json(row["receipt"])
            if (
                row["seq"] != i
                or row["prev_hash"] != prev
                or row["event_hash"] != actual
                or receipt.event_hash != actual
                or receipt.operation_id != row["operation_id"]
                or receipt.ledger_mode != self.policy.ledger_mode
            ):
                raise EscrowError("EVENT_CHAIN")
            prev = actual
        if last_state is not None and last_state != self._snapshot():
            raise EscrowError("PERSISTED_STATE_MISMATCH")
        self._conservation()

    def _snapshot(self) -> str:
        origins = [list(r) for r in self.db.execute("SELECT * FROM escrow_origins ORDER BY origin")]
        units = [
            list(r)
            for r in self.db.execute("SELECT * FROM escrow_units ORDER BY origin,owner,bucket,ref")
        ]
        finals = [list(r) for r in self.db.execute("SELECT * FROM escrow_finalized ORDER BY w")]
        shadow = [
            list(r)
            for r in self.db.execute("SELECT * FROM escrow_shadow_finalized ORDER BY ordinal")
        ]
        if shadow:
            return canonicalize([origins, units, finals, shadow]).decode()
        return canonicalize([origins, units, finals]).decode()
