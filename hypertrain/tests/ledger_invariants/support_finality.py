"""Builders private to the finality package. Not collected: no test_ prefix.

`Stage` is the real v2 service stopped between aggregate and finalize: five ACTIVE identities
(SQL-activated, as tests/e2e/test_dispute_exit_v2_e2e.py does), round 0 audited with one forged
commit (one MISMATCH, four MATCH) and aggregated, round 1 audited and aggregated for the four
honest miners. It costs about a minute, so a module builds it once and every test starts from a
byte-identical copy of its database. `Rounds` is the cheap counterpart for the ledger alone:
COORD-signed finalize evidence for any round number and any set of miners.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
from collections.abc import Callable, Iterator
from dataclasses import dataclass, replace
from pathlib import Path
from types import ModuleType
from typing import Any

import httpx

from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import Params
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    EscrowV2,
    FinalityEvidence,
    RewardWork,
    authority_message,
    digest,
    reward_origin_id,
)
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, Rollback
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    CommitV2,
    EconomicsPolicyV2,
    ReplayVerdict,
    ResolutionV2,
    RewardFinalize,
    RoundOpenV2,
    RunManifestV2,
)

from .support import Real, bare_escrow, load_fixture_module

ROLLBACK_FIXTURE = ("test_rollback_service_v2", "e2e/test_rollback_service_v2.py")
DISPUTE_EXIT_FIXTURE = ("test_dispute_exit_v2_e2e", "e2e/test_dispute_exit_v2_e2e.py")
FORGED_DELTA = "aa" * 32
ZERO = "0" * 64
FINAL = "ef" * 32
ANOTHER = "12" * 32


def refusal(call: Callable[[], object]) -> tuple[type[BaseException], int | None, str]:
    """(type, HTTP status if any, text) of the refusal `call` raises; no refusal fails."""
    try:
        call()
    except Exception as error:
        return type(error), getattr(error, "status", None), str(error)
    raise AssertionError("expected a refusal, nothing was raised")


@dataclass
class Stage:
    """The network, the frozen database and what the two rounds answered."""

    network: Any
    fixtures: ModuleType
    rollback: ModuleType
    snapshot: sqlite3.Connection
    now: int
    clock: Any
    forger: Any
    honest: list[Any]
    aggregated: list[dict[str, Any]]
    after_open: dict[str, Any]
    after_aggregate: dict[str, Any]

    @property
    def store(self) -> Any:
        return self.network.store

    @property
    def run_id(self) -> str:
        run_id: str = self.network.manifest.run_id()
        return run_id

    @property
    def coordinator(self) -> Keypair:
        key: Keypair = self.fixtures.COORD
        return key

    def reset(self) -> None:
        """Put the database, the beacon clock and the service cache back as frozen.

        The services are built here, outside any transaction: the first service-touching call
        of a handler would otherwise build them inside its own.
        """
        store = self.store
        with store._lock:
            self.snapshot.backup(store._db)
            store._services_v2.clear()
            self.network.now = self.now
            store.clock = self.clock
            store._services(self.run_id)

    def digest(self) -> str:
        """Hash of every row of every table of the store: a refusal must leave it unchanged."""
        hasher = hashlib.sha256()
        with self.store._lock:
            names = [
                row[0]
                for row in self.store._db.execute(
                    "SELECT name FROM sqlite_master WHERE type='table' "
                    "AND name NOT LIKE 'sqlite_%' ORDER BY name"
                )
            ]
            for name in names:
                rows = sorted(
                    repr(tuple(row)) for row in self.store._db.execute(f'SELECT * FROM "{name}"')
                )
                hasher.update(repr((name, rows)).encode())
        return hasher.hexdigest()

    def rows(self, sql: str, *args: Any) -> list[tuple[Any, ...]]:
        with self.store._lock:
            return [tuple(row) for row in self.store._db.execute(sql, args)]

    def record(self, kind: str, identity: str) -> dict[str, Any]:
        with self.store._lock:
            record: dict[str, Any] = self.store._record_v2(self.run_id, kind, identity)
        return record

    def records(self, *kinds: str) -> list[tuple[str, str, str]]:
        """(kind, id, data) of every stored record of those kinds."""
        marks = ",".join("?" * len(kinds))
        return [
            (str(k), str(i), str(d))
            for k, i, d in self.rows(
                f"SELECT kind,id,data FROM records_v2 WHERE run_id=? AND kind IN ({marks}) "
                "ORDER BY kind,id",
                self.run_id,
                *kinds,
            )
        ]

    def put(self, kind: str, identity: str, data: dict[str, Any]) -> None:
        """Overwrite a stored record: the test-owned stand-in for a state the service reached."""
        with self.store._tx():
            self.store._put_record_v2(self.run_id, kind, identity, data)

    def forget_nonce(self, message_type: str) -> None:
        """Drop the single-use reservations of one signed message type.

        Leaves the stored result in place, so the check behind the reservation is the only
        thing left between a conflicting request and the stored state.
        """
        with self.store._tx():
            self.store._db.execute(
                "DELETE FROM accepted_v2 WHERE key LIKE 'auth:%' "
                "AND json_extract(substr(key, 6), '$[1]')=?",
                (message_type,),
            )

    def finalize_body(self, w: int = 0, **changes: Any) -> dict[str, Any]:
        """The Finalize the coordinator would sign for round `w`."""
        body: dict[str, Any] = {
            "w": w,
            "final_theta_hash_w1": self.aggregated[w]["theta_hash"],
            "included": sorted(k.ss58 for k in self.honest),
            "entitlements_root": ZERO,
        }
        return body | changes

    def signed_finalize(
        self, w: int = 0, *, signer: Keypair | None = None, expiry: int = 10000, **changes: Any
    ) -> dict[str, Any]:
        envelope: dict[str, Any] = self.network.signed(
            signer or self.coordinator,
            "Finalize",
            Finalize(**self.finalize_body(w, **changes)),
            expiry,
        )
        return envelope

    def finalize(self, w: int = 0, **options: Any) -> dict[str, Any]:
        """Store-level finalize of the envelope `signed_finalize` builds."""
        answer: dict[str, Any] = self.store.finalize_v2(
            self.run_id, w, canonicalize(self.signed_finalize(w, **options))
        )
        return answer

    def post_finalize(self, w: int, envelope: dict[str, Any]) -> httpx.Response:
        return self.network.client.post(
            self.network.url + f"/admin/rounds/{w}/finalize",
            json=envelope,
            headers=self.fixtures.admin(),
        )

    def rollback_body(self, **changes: Any) -> Rollback:
        body: dict[str, Any] = {
            "w": 0,
            "excluded": [self.forger.ss58],
            "old_theta_hash_w2": self.aggregated[1]["theta_hash"],
            "new_theta_hash_w2": "cd" * 32,
            "new_outer_state_hash": "ef" * 32,
            "recomputed": ["agg_w", "step_w", "agg_w1", "step_w1"],
            "cause_hashes": ["ab" * 32],
        }
        return Rollback(**(body | changes))

    def signed_rollback(self, signer: Keypair | None = None, **changes: Any) -> bytes:
        envelope = self.network.signed(
            signer or self.coordinator, "Rollback", self.rollback_body(**changes)
        )
        return canonicalize(envelope)

    def seed_cause(
        self,
        *,
        loser: str | None = None,
        reason: str = "FRAUD",
        signer: Keypair | None = None,
        forge_signature: bool = False,
        evidence: bool = True,
    ) -> str:
        """Store a resolved contest of the forger's round-0 verdict, as the referee route would.

        Returns the cause hash a Rollback cites. The defaults are a resolution the referee signed,
        naming the forger as loser for FRAUD, with its evidence registered; each option breaks one
        of those. A test-owned stand-in: the contest itself is not played.
        """
        referee = self.fixtures.REFEREE
        dispute_id = sha256_hex(b"seeded dispute")
        resolution = ResolutionV2(
            dispute_id=dispute_id,
            transcript_hash=sha256_hex(b"seeded transcript"),
            reason=reason,
            loser=(
                None
                if reason in {"MATCH", "INFRASTRUCTURE", "REFEREE_FAILURE"}
                else (loser or self.forger.ss58)
            ),
            evidence_hash=sha256_hex(b"seeded evidence"),
        )
        envelope = self.network.signed(signer or referee, "ResolutionV2", resolution)
        if forge_signature:
            envelope["sig"] = referee.sign(b"another message").hex()
        body = resolution.model_dump(mode="json")
        turn = {
            "dispute_id": dispute_id,
            "run_id": self.run_id,
            "contest": {"miner": self.forger.ss58, "referee": referee.ss58, "w": 0},
            "resolution": body,
        }
        with self.store._tx():
            self.store._db.execute(
                "INSERT INTO disputes_v2(id, verdict, miner, turn, request) VALUES(?,?,?,?,?)",
                (dispute_id, sha256_hex(b"seeded verdict"), self.forger.ss58, json.dumps(turn), ""),
            )
            self.store._put_record_v2(self.run_id, "resolution", dispute_id, envelope)
            if evidence:
                self.store._db.execute(
                    "INSERT INTO dispute_evidence_v2 VALUES(?,?)",
                    (body["evidence_hash"], "{}"),
                )
        return body_digest(body)

    def signed_opening(self, w: int, **changes: Any) -> bytes:
        """A coordinator-signed RoundOpenV2 for round `w`, copied from the stored opening of
        round 1 with a schedule that lies in the future of the frozen beacon."""
        stored = self.record("round", "1")["body"]
        now = self.network.now
        body = stored | {
            "w": w,
            "d_open": now + 1,
            "d_assign": now + 2,
            "d_commit": now + 30,
            "d_audit": now + 31,
            "d_upload": now + 150,
            "d_final": now + 200,
        }
        envelope = self.network.signed(
            self.coordinator, "RoundOpenV2", RoundOpenV2.model_validate(body | changes)
        )
        return canonicalize(envelope)


def build_stage(tmp_path_factory: Any) -> Iterator[Stage]:
    rollback = load_fixture_module(*ROLLBACK_FIXTURE)
    dispute = load_fixture_module(*DISPUTE_EXIT_FIXTURE)
    fixtures = rollback.fixtures
    generator = dispute.activated_network.__wrapped__(tmp_path_factory)
    network = next(generator)
    run_id = network.manifest.run_id()
    snapshot = sqlite3.connect(":memory:", check_same_thread=False)
    try:
        forger, honest = fixtures.HOT[4], list(fixtures.HOT[:4])
        rollback.execute_round(network, 0, open_only=True)
        after_open = network.store._record_v2(run_id, "finality", "0")
        first = rollback.execute_round(
            network, 0, forge={forger.ss58: {"delta_hash": FORGED_DELTA}}
        )
        after_aggregate = network.store._record_v2(run_id, "finality", "0")
        second = rollback.execute_round(network, 1, hotkeys=honest)
        network.store._db.backup(snapshot)
        yield Stage(
            network,
            fixtures,
            rollback,
            snapshot,
            network.now,
            network.store.clock,
            forger,
            honest,
            [first, second],
            after_open,
            after_aggregate,
        )
    finally:
        snapshot.close()
        generator.close()


# -- ledger-only finalize evidence ------------------------------------------------------------

THETA = "cd" * 32  # what the replay stand-in answers for every synthetic tape
MINERS = [Keypair(bytes([0x20 + i]) * 32) for i in range(3)]


@dataclass(frozen=True)
class Round:
    reward: RewardFinalize
    evidence: FinalityEvidence
    origin_ids: tuple[str, ...]


class Rounds:
    """Finalize evidence for any (round, variant) over a setup widened to `cap` units.

    Derived from the real w=0 evidence of `real_reward`: the same commit, delta and leaves
    root, signed again by the same roles for another round number, another set of miners and
    another entitlements root. A variant is a different COORD-signed Finalize for the same
    round, which is what a repaired or re-signed finalize would be.
    """

    def __init__(self, fixture: ModuleType, real: Real, *, cap_rounds: int) -> None:
        setup, self.evidence0, _ = real
        policy = EconomicsPolicyV2.model_validate(
            {
                **setup.policy.body(),
                "max_total_issuance": cap_rounds * setup.policy.round_reward_units,
            }
        )
        body = setup.manifest.body()
        body["network"]["economics_policy_hash"] = digest(policy)
        self.fixture = fixture
        self.setup = replace(setup, policy=policy, manifest=RunManifestV2.model_validate(body))
        self.run_id = self.setup.manifest.run_id()
        self.budget = policy.round_reward_units
        self._built: dict[tuple[int, int, int, str | None], Round] = {}
        self._theta: dict[str, str] = {}
        self._deriver: EscrowV2 | None = None

    def escrow(self) -> EscrowV2:
        """A private ledger whose replay stand-in knows exactly the tapes built here."""

        def replay(evidence: FinalityEvidence) -> str:
            if evidence.tape_hash not in self._theta:
                raise EscrowError("NO_REPLAY_ACCEPTANCE")
            return self._theta[evidence.tape_hash]

        return bare_escrow(self.fixture, self.setup, replay=replay)

    def origin(self, w: int, hotkey: str, finalize_hash: str) -> str:
        """The id of the reward origin the ledger issues for (round, miner, finalize).

        Read from the ledger when it derives its own ids (keyed on a base that has an id key),
        from the plain function of the base that does not.
        """
        if self._deriver is None:
            self._deriver = bare_escrow(self.fixture, self.setup)
        derive = getattr(self._deriver, "reward_origin", None)
        if derive is not None:
            return str(derive(w, hotkey, finalize_hash))
        return reward_origin_id(self.run_id, w, hotkey, finalize_hash)

    def build(self, w: int, variant: int = 0, miners: int = 1, tamper: str | None = None) -> Round:
        key = (w, variant, miners, tamper)
        if key not in self._built:
            self._built[key] = self._build(w, variant, miners, tamper)
        return self._built[key]

    def _build(self, w: int, variant: int, count: int, tamper: str | None) -> Round:
        fixture, run_id = self.fixture, self.run_id
        base = self.evidence0.works[0]
        commit0 = CommitV2.model_validate(base.commit.body)
        challenge0 = AuditChallengeV2.model_validate(base.challenge.body)
        verdict0 = ReplayVerdict.model_validate(base.verdict.body)
        keys = [fixture.HOT, *MINERS][:count]
        challenger = MINERS[2] if tamper == "foreign challenge signer" else fixture.COORD
        works, verdicts, owners = [], [], {}
        for hot in keys:
            commit = commit0.model_copy(update={"w": w, "hotkey": hot.ss58})
            challenge = challenge0.model_copy(update={"w": w, "target": hot.ss58})
            verdict = verdict0.model_copy(update={"challenge_hash": digest(challenge)})
            auditor = fixture.AUDITOR
            if tamper is not None:
                verdict, auditor = self._tampered(verdict, tamper, hot)
            owner = fixture.COLD.ss58 if hot is fixture.HOT else fixture.OTHER.ss58
            owners[hot.ss58] = owner
            verdicts.append(digest(verdict))
            works.append(
                RewardWork(
                    EnvelopeV2.model_validate(seal(hot, "CommitV2", run_id, commit, 200)),
                    EnvelopeV2.model_validate(seal(auditor, "ReplayVerdict", run_id, verdict, 200)),
                    EnvelopeV2.model_validate(
                        seal(challenger, "AuditChallengeV2", run_id, challenge, 200)
                    ),
                    owner,
                    (0,),
                )
            )
        finalizer = MINERS[2] if tamper == "foreign finalize signer" else fixture.COORD
        final = Finalize(
            w=w,
            final_theta_hash_w1=THETA,
            included=sorted(k.ss58 for k in keys),
            entitlements_root=sha256_hex(f"variant|{variant}".encode()),
        )
        tape = sha256_hex(f"tape|{w}|{variant}|{count}|{tamper}".encode())
        self._theta[tape] = (
            sha256_hex(b"another theta") if tamper == "another replayed theta" else THETA
        )
        evidence = FinalityEvidence(
            EnvelopeV2.model_validate(seal(finalizer, "Finalize", run_id, final, 200)),
            tuple(works),
            tape,
            False,
            10,
            20,
        )
        hotkeys = sorted((k.ss58 for k in keys), key=str.encode)
        units = {h: self.budget // count for h in hotkeys}
        for h in hotkeys[: self.budget - sum(units.values())]:
            units[h] += 1
        origins = [
            {
                "origin_id": self.origin(w, h, digest(final)),
                "owner": owners[h],
                "units": units[h],
                "mature_at": 20,
            }
            for h in hotkeys
        ]
        reward = RewardFinalize(
            run_id=run_id,
            w=w,
            finalize_hash=digest(final),
            tape_hash=tape,
            verdict_root=sha256_hex(canonicalize(verdicts)),
            allocation_hash=sha256_hex(canonicalize(origins)),
            budget_units=self.budget,
            origin_ids=[str(o["origin_id"]) for o in origins],
            mature_at=20,
            authority_sig="0" * 128,
        )
        reward = RewardFinalize.model_validate(
            {**reward.body(), "authority_sig": fixture.COORD.sign(authority_message(reward)).hex()}
        )
        return Round(reward, evidence, tuple(reward.origin_ids))

    def _tampered(
        self, verdict: ReplayVerdict, tamper: str, miner: Keypair
    ) -> tuple[ReplayVerdict, Keypair]:
        """The verdict an audit would carry if it did not prove the commit, and its signer."""
        env = verdict.replay_env
        match tamper:
            case "mismatch":
                other = {"result": "MISMATCH"}
            case "bad leaf named":
                other = {"first_bad_leaf": 3}
            case "another leaves root":
                other = {"recomputed_leaves_root": sha256_hex(b"another leaves root")}
            case "foreign driver":
                other = {"replay_env": env.model_copy(update={"driver": "unlisted-driver"})}
            case "foreign image":
                other = {
                    "replay_env": env.model_copy(
                        update={"image_digest": "sha256:" + sha256_hex(b"image")}
                    )
                }
            case "another challenge":
                other = {"challenge_hash": sha256_hex(b"another challenge")}
            case "unpinned auditor":
                return verdict, MINERS[2]
            case "self audit":
                return verdict, miner
            case "foreign finalize signer" | "foreign challenge signer" | "another replayed theta":
                return verdict, self.fixture.AUDITOR
            case _:
                raise ValueError(tamper)
        return verdict.model_copy(update=other), self.fixture.AUDITOR


def plain_store(root: Path) -> ChallengeStore:
    """A ChallengeStore with no run: enough to exercise its transaction helper."""
    return ChallengeStore(
        root / "state",
        Params("hypertrain", 1_900_000_000, 300, 1, 1),
        None,
        None,
        lambda payload: None,
        LocalFSStore(root / "objects"),
    )


class FailingCommit:
    """The store's connection, whose first COMMIT fails the way a full disk makes it fail."""

    def __init__(self, real: sqlite3.Connection) -> None:
        self._real = real
        self.armed = True

    def execute(self, sql: str, *args: Any) -> Any:
        if self.armed and sql.strip().upper() == "COMMIT":
            self.armed = False
            raise sqlite3.OperationalError("database or disk is full")
        return self._real.execute(sql, *args)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._real, name)
