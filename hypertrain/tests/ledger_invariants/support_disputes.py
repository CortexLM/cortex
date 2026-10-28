"""Builders private to the disputes package of the ledger-invariants lot. Not collected.

`Rig` is a bare DisputesV2 over a private in-memory EscrowV2: the state machine, its escrow and
nothing else. The HTTP helpers seed what the audit would have written (the accepted contest
record) on a `service_world` and go through the real routes for everything else.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import sys
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from types import ModuleType
from typing import Any

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.disputes_v2 import (
    Availability,
    Contest,
    DisputeError,
    DisputesV2,
    EventAck,
    Turn,
    signed_message,
)
from hypertrain.ledger.escrow_v2 import FinalityEvidence, digest, reward_origin_id
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    REQUEST_HORIZON,
    BisectV2,
    DisputePolicyV2,
    DisputeV2,
    DisputeWatchRequest,
    EscrowLock,
    OriginAllocation,
    ResolutionV2,
    RewardFinalize,
    RunManifestV2,
)

from .support import Real, World, bare_escrow, load_fixture_module, resign_reward

VERDICT = "11" * 32
AGREE = ["11" * 32] * 3
MINER_SAYS = ["11" * 32, "22" * 32, "33" * 32]
AUDITOR_SAYS = ["11" * 32, "aa" * 32, "bb" * 32]
FAR = 100_000  # envelope expiry round: far beyond any round a test reaches
BOND = 100  # the policy's max_referee_cost_units, so the smallest accepted bond

TABLES = (
    "disputes_v2",
    "dispute_entries_v2",
    "dispute_events_v2",
    "dispute_acks_v2",
    "dispute_evidence_v2",
    "dispute_states_v2",
    "dispute_receipts_v2",
    "escrow_events",
    "escrow_units",
)


def raised[E: Exception](call: Callable[[], object], kind: type[E]) -> E:
    """The `kind` exception `call` raises. Any other outcome fails: a test pins what refused."""
    try:
        call()
    except kind as error:
        return error
    except Exception as error:
        raise AssertionError(
            f"expected {kind.__name__}, got {type(error).__name__}: {error}"
        ) from error
    raise AssertionError(f"expected {kind.__name__}, nothing was raised")


def refusal(call: Callable[[], object]) -> str:
    """Code of the DisputeError `call` raises."""
    return raised(call, DisputeError).code


def table_digest(db: sqlite3.Connection, tables: tuple[str, ...] = TABLES) -> str:
    """One hash over every row of the dispute and escrow tables: equal means nothing moved."""
    hasher = hashlib.sha256()
    for name in tables:
        rows = sorted(repr(tuple(row)) for row in db.execute(f'SELECT * FROM "{name}"'))
        hasher.update(repr((name, rows)).encode())
    return hasher.hexdigest()


def settlement_events(db: sqlite3.Connection, lock_id: str) -> list[str]:
    """Kinds (RELEASE or SLASH) of the escrow events that dispose of the bond `lock_id`, in order.

    The operation id of such an event is derived by the service, and how it is derived is not a
    property of the bond, so the events are found by the lock they name."""
    kinds = []
    for kind, request in db.execute(
        "SELECT kind,request FROM escrow_events WHERE kind IN ('RELEASE','SLASH') ORDER BY seq"
    ):
        body = json.loads(request)
        operation = body[0] if isinstance(body, list) else body  # a slash wraps its release
        if operation["lock_id"] == lock_id:
            kinds.append(str(kind))
    return kinds


def reward_origin_of(escrow: Any, run: str, w: int, hotkey: str, finalize_hash: str) -> str:
    """The origin id `escrow` issues for this reward: its own derivation where the ledger keys
    the ids it derives, the plain function of the run, the round, the hotkey and the finalize
    hash else. A test reads the id from here and never computes it."""
    if hasattr(escrow, "reward_origin"):
        return str(escrow.reward_origin(w, hotkey, finalize_hash))
    derive: Callable[..., str] = reward_origin_id
    return derive(run, w, hotkey, finalize_hash)


def signed_variant_on(
    escrow: Any, fixture: ModuleType, real: Real, **final_changes: Any
) -> tuple[RewardFinalize, FinalityEvidence]:
    """A COORD-signed Finalize/RewardFinalize pair differing from the fixture's Finalize, whose
    origin is the one `escrow` would issue: the single audited worker's split is recomputed, so a
    refused variant is refused for its Finalize content and not for a malformed allocation."""
    setup, evidence, reward = real
    run_id = setup.manifest.run_id()
    final = Finalize.model_validate({**evidence.finalize.body, **final_changes})
    envelope = EnvelopeV2.model_validate(seal(fixture.COORD, "Finalize", run_id, final, 200))
    work = evidence.works[0]
    origin = OriginAllocation(
        origin_id=reward_origin_of(escrow, run_id, reward.w, work.commit.signer, digest(final)),
        owner=work.owner,
        units=reward.budget_units,
        mature_at=reward.mature_at,
    )
    changed = resign_reward(
        fixture,
        reward,
        finalize_hash=digest(final),
        origin_ids=[origin.origin_id],
        allocation_hash=sha256_hex(canonicalize([origin.body()])),
    )
    return changed, replace(evidence, finalize=envelope)


@dataclass
class Rig:
    f: ModuleType
    d: DisputesV2
    ref: Keypair
    next_ref: Keypair
    origin: str
    other_origin: str
    contests: dict[str, Contest] = field(default_factory=dict)
    owners: dict[str, str] = field(default_factory=dict)
    locks: int = 0

    @property
    def db(self) -> sqlite3.Connection:
        return self.d.db

    @property
    def escrow(self) -> Any:
        return self.d.escrow

    def state(self) -> str:
        return table_digest(self.db)

    def miner(self, index: int) -> Keypair:
        """A further audited miner; its coldkey is COLD, which holds the bonds."""
        return Keypair(bytes([150 + index]) * 32)

    def contest(
        self, verdict: str = VERDICT, miner: Keypair | None = None, **updates: Any
    ) -> Contest:
        key = miner or self.f.HOT
        made = Contest(
            verdict_hash=verdict,
            challenge_hash="22" * 32,
            w=0,
            miner=key.ss58,
            auditor=self.f.AUDITOR.ss58,
            referee=self.ref.ss58,
            step_span=2,
            layer_span=2,
            op_span=2,
        )
        made = made.model_copy(update=updates)
        self.contests[verdict] = made
        return made

    def dispute_id(self, verdict: str = VERDICT) -> str:
        made = self.contests[verdict]
        return sha256_hex(canonicalize([self.d.run_id, verdict, made.miner]))

    def lock(
        self,
        verdict: str = VERDICT,
        *,
        units: int = BOND,
        owner: str | None = None,
        kind: str = "LOCK_CONTEST",
        dispute_id: str | None = None,
        fund: bool = True,
    ) -> EscrowLock:
        """A bond the owner signed for `verdict`'s dispute (or for another id, or an admission)."""
        self.locks += 1
        who = owner or self.f.COLD.ss58
        admission = kind == "LOCK_ADMISSION"
        made = EscrowLock(
            operation_id=f"{0x1000 + self.locks:064x}",
            owner=who,
            units=units,
            origin_ids=[self.origin if who == self.f.COLD.ss58 else self.other_origin],
            admission_id=H_ADMISSION if admission else None,
            dispute_id=None if admission else dispute_id or self.dispute_id(verdict),
            kind="LOCK_ADMISSION" if admission else "LOCK_CONTEST",
        )
        if fund:
            signer = self.f.COLD if who == self.f.COLD.ss58 else self.f.OTHER
            self.escrow.lock(made, signer=signer.ss58)
        return made

    def request(
        self,
        key: Keypair,
        verdict: str,
        bond: str | None,
        *,
        action: str = "contest",
        hotkey: str | None = None,
    ) -> bytes:
        body = DisputeV2(
            hotkey=hotkey or key.ss58, verdict_hash=verdict, action=action, bond_lock=bond
        )
        return canonicalize(seal(key, "DisputeV2", self.d.run_id, body, FAR))

    def open(
        self, verdict: str = VERDICT, *, miner: Keypair | None = None, now: int = 1, **updates: Any
    ) -> tuple[Turn, bytes]:
        """Register the contest record, fund its bond, and open: the happy path."""
        key = miner or self.f.HOT
        self.contest(verdict, key, **updates)
        bond = self.lock(verdict)
        raw = self.request(key, verdict, bond.operation_id)
        return self.d.open(raw, now=now), raw

    def say(
        self, dispute_id: str, key: Keypair, hashes: list[str], now: int = 2, **override: Any
    ) -> tuple[Turn, bytes]:
        """`key` answers the current turn of `dispute_id`; `override` breaks one binding."""
        raw = self.answer(dispute_id, key, hashes, **override)
        return self.d.bisect(raw, now=now), raw

    def answer(self, dispute_id: str, key: Keypair, hashes: list[str], **override: Any) -> bytes:
        turn = self.d.get(dispute_id)
        fields: dict[str, Any] = dict(
            dispute_id=dispute_id,
            seq=turn.seq,
            level=turn.level,
            ctx=turn.ctx,
            interval=turn.interval,
            N=2,
            hashes=hashes,
            party=key.ss58,
            previous_transcript_hash=turn.transcript_hash,
        )
        fields.update(override)
        model = BisectV2.model_validate(fields)
        return canonicalize(seal(key, "BisectV2", self.d.run_id, model, FAR))

    def evidence(
        self, turn: Turn, reason: str = "MATCH", loser: str | None = None, op: str = "verified"
    ) -> RefereeEvidence:
        return RefereeEvidence(
            dispute_id=turn.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason=reason,  # type: ignore[arg-type]
            loser=loser,
            predecessor_hash="11" * 32,
            inputs_hash="22" * 32,
            output_hash="33" * 32,
            op_spec=op,
        )

    def resolve(
        self,
        turn: Turn,
        evidence: RefereeEvidence,
        *,
        key: Keypair | None = None,
        now: int = 3,
        **override: Any,
    ) -> Turn:
        """The referee signs a resolution that cites `evidence` (registered by the caller)."""
        return self.d.resolution(
            self.signed_resolution(turn, evidence, key=key, **override), now=now
        )

    def signed_resolution(
        self, turn: Turn, evidence: RefereeEvidence, *, key: Keypair | None = None, **override: Any
    ) -> bytes:
        fields: dict[str, Any] = dict(
            dispute_id=evidence.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason=evidence.reason,
            loser=evidence.loser,
            evidence_hash=evidence.digest(),
        )
        fields.update(override)
        signer = key or self.ref
        return canonicalize(
            seal(signer, "ResolutionV2", self.d.run_id, ResolutionV2.model_validate(fields), FAR)
        )

    def close(
        self,
        turn: Turn,
        reason: str,
        loser: str | None,
        *,
        now: int = 3,
        referee_cost: int = 0,
    ) -> Turn:
        """What the store does for a referee resolution: decision and bond in one transaction."""
        evidence = self.evidence(turn, reason, loser)
        self.d.register_evidence(evidence)
        with self.escrow.tx():
            closed = self.resolve(turn, evidence, now=now)
            self.d.settle_lock(turn.dispute_id, now=now, referee_cost=referee_cost)
        return closed

    def ack(
        self,
        key: Keypair,
        now: int = 2,
        *,
        index: int = -1,
        signer: Keypair | None = None,
        **over: Any,
    ) -> EventAck:
        """An EventAck of an event addressed to `key`, signed by `signer` (default `key`)."""
        event = self.d.events(key.ss58, 0)[index]
        fields: dict[str, Any] = dict(
            run_id=self.d.run_id,
            cursor=event.cursor,
            event_hash=event.digest(),
            party=key.ss58,
            received_beacon=now,
            sig="",
        )
        fields.update(over)
        made = EventAck.model_validate(fields)
        return made.model_copy(update={"sig": (signer or key).sign(signed_message(made)).hex()})

    def availability(
        self, turn: Turn, keys: tuple[Keypair, Keypair], now: int, **override: Any
    ) -> tuple[Availability, Availability]:
        proofs = []
        for key in keys:
            fields: dict[str, Any] = dict(
                run_id=self.d.run_id,
                dispute_id=turn.dispute_id,
                seq=turn.seq,
                transcript_hash=turn.transcript_hash,
                observed_beacon=now,
                service_healthy=True,
                beacon_healthy=True,
                reference_healthy=True,
                signer=key.ss58,
                sig="",
            )
            fields.update(override)
            proof = Availability.model_validate(fields)
            proofs.append(proof.model_copy(update={"sig": key.sign(signed_message(proof)).hex()}))
        return proofs[0], proofs[1]


H_ADMISSION = "ad" * 32  # admission id of a LOCK_ADMISSION made only to be refused as a bond


def make_rig(
    fixture: ModuleType, *, coordinator_allowlisted: bool = False, pin: tuple[Keypair, ...] = ()
) -> Rig:
    """DisputesV2 over a funded in-memory EscrowV2 (COLD 20000, OTHER 10000 units).

    Mirrors tests/challenge/test_disputes_v2.service, except that the contest records, the
    owners and the miners are the rig's, so a test can hold several disputes.
    """
    ref, next_ref = Keypair(b"\x71" * 32), Keypair(b"\x72" * 32)
    policy = DisputePolicyV2(
        referees=[
            ref.ss58,
            next_ref.ss58,
            *([fixture.COORD.ss58] if coordinator_allowlisted else []),
            *(key.ss58 for key in pin),
        ],
        max_open=32,
        max_transcript_entries=256,
        max_entry_bytes=65536,
        fanout=2,
        max_referee_reassignments=1,
        max_referee_cost_units=BOND,
    )
    setup = fixture.setup()
    body = setup.manifest.body()
    body["training"]["inner"].update(H=2, J=1)
    body["network"]["dispute_policy_hash"] = policy.digest()
    setup = replace(setup, manifest=RunManifestV2.model_validate(body))
    escrow = bare_escrow(fixture, setup)
    origin, other_origin = fixture.fund(escrow)
    rig = Rig(fixture, None, ref, next_ref, origin, other_origin)  # type: ignore[arg-type]

    def contest_of(verdict: str) -> Contest:
        return rig.contests[verdict]

    rig.d = DisputesV2(
        escrow,
        policy,
        fixture.COORD,
        contest_of=contest_of,
        owner_of=lambda hotkey: rig.owners.get(hotkey, fixture.COLD.ss58),
        span=lambda contest, level, ctx: 2,
    )
    escrow.settlement = rig.d.settlement
    return rig


# --- HTTP: a real service_world with a seeded accepted contest ------------------------------

W = 0


@dataclass
class Contested:
    """A joined miner with the contest record its audit would have written."""

    world: World
    service: ModuleType
    index: int
    verdict: str

    @property
    def hot(self) -> Keypair:
        return self.service.HOT[self.index]  # type: ignore[no-any-return]

    @property
    def cold(self) -> Keypair:
        return self.service.COLD[self.index]  # type: ignore[no-any-return]

    @property
    def dispute_id(self) -> str:
        return sha256_hex(canonicalize([self.world.run_id, self.verdict, self.hot.ss58]))

    def post(self, path: str, key: Keypair, type_: str, body: Any, expiry: int = 10000) -> Any:
        net = self.world.network
        return self.world.client.post(
            net.url + path, json=net.signed(key, type_, body, expiry=expiry)
        )

    def lock(self, operation: str | None = None, units: int = BOND) -> EscrowLock:
        operation = operation or sha256_hex(b"bond|" + self.verdict.encode())
        store = self.world.store
        origin = store._db.execute(
            "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=?",
            (self.cold.ss58, units),
        ).fetchone()[0]
        return EscrowLock(
            operation_id=operation,
            owner=self.cold.ss58,
            units=units,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=self.dispute_id,
            kind="LOCK_CONTEST",
        )

    def fund(self, operation: str | None = None, units: int = BOND) -> EscrowLock:
        bond = self.lock(operation, units)
        response = self.post("/escrow/lock", self.cold, "EscrowLock", bond)
        assert response.status_code == 200, response.text
        return bond

    def open(self, bond: EscrowLock) -> Any:
        body = DisputeV2(
            hotkey=self.hot.ss58,
            verdict_hash=self.verdict,
            action="contest",
            bond_lock=bond.operation_id,
        )
        return self.post("/dispute", self.hot, "DisputeV2", body)

    def turn(self) -> Turn:
        return self.world.store._services(self.world.run_id)[2].get(self.dispute_id)

    def disputes(self) -> DisputesV2:
        return self.world.store._services(self.world.run_id)[2]


def contested(world: World, index: int = 0, verdict: str | None = None) -> Contested:
    """Join miner `index` and seed the accepted contest record of an adverse audit of round 0."""
    service = load_fixture_module("li_service_fixture", "challenge/test_service_network_v2.py")
    net = world.network
    verdict = verdict or sha256_hex(f"dsp-verdict-{index}".encode())
    assert net.join(index).status_code == 200
    store, run_id, inner = world.store, world.run_id, net.manifest.training.inner
    hot = service.HOT[index]
    admission_id = store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (hot.ss58,)
    ).fetchone()[0]
    contest = {
        "verdict_hash": verdict,
        "challenge_hash": sha256_hex(b"dsp-challenge"),
        "w": W,
        "miner": hot.ss58,
        "auditor": service.AUDITORS[0].ss58,
        "referee": service.REFEREE.ss58,
        "step_span": inner.H // inner.J,
        "layer_span": 2,
        "op_span": 2,
        "admission_id": admission_id,
        "coldkey": service.COLD[index].ss58,
        "evidence_hash": sha256_hex(b"dsp-evidence"),
        "auditor_coldkey": None,
    }
    with store._lock:
        store._put_record_v2(run_id, "contest", verdict, contest)
        if not store._db.execute(
            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='finality' AND id=?",
            (run_id, str(W)),
        ).fetchone():
            store._put_record_v2(run_id, "finality", str(W), {"w": W})
        store._put_record_v2(run_id, "excluded", f"{W}:{hot.ss58}", {"reason": "UNVERIFIED"})
    return Contested(world, service, index, verdict)


def seed_rounds(
    world: World,
    *,
    applied: bool = True,
    settled: tuple[int, ...] = (),
    unresolved_audits: int = 0,
) -> None:
    """Rounds 0 and 1 as the service records an applied, closed pair (run it before `contested`).

    Only the records the rollback gate and the dispute recount read are written; the repair
    itself is never reached by a test that uses them.
    """
    store, run_id = world.store, world.run_id
    with store._lock:
        for w in (0, 1):
            store._put_record_v2(run_id, "round", str(w), {"body": {"roster": []}})
            store._put_record_v2(
                run_id,
                "finality",
                str(w),
                {
                    "run_id": run_id,
                    "w": w,
                    "applied": applied,
                    "disposition": "SETTLED" if w in settled else "PENDING",
                    "unresolved_disputes": 0,
                    "unresolved_audits": unresolved_audits,
                    "rollback_complete": False,
                    "state_hash": "aa" * 32,
                    "resolution_hash": None,
                },
            )


def rollback_raw(world: World, excluded: list[str], cause_hashes: list[str]) -> bytes:
    """A coordinator-signed Rollback of round 0 excluding `excluded` for `cause_hashes`."""
    from hypertrain.protocol.messages import Rollback

    service = load_fixture_module("li_service_fixture", "challenge/test_service_network_v2.py")
    body = Rollback(
        w=0,
        excluded=excluded,
        old_theta_hash_w2="aa" * 32,
        new_theta_hash_w2="bb" * 32,
        new_outer_state_hash="cc" * 32,
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=cause_hashes,
    )
    return canonicalize(seal(service.COORD, "Rollback", world.run_id, body, 10_000))


# The refusal of a watch request that the party did not sign.
WATCH_NEEDS_SIGNATURE = "party event access requires signature"


def watch_feed(world: World, party: Keypair, signer: Keypair | None) -> Any:
    """The dispute events of `party`, asked for with the signature of `signer` (none: no request).

    The one form the service reads is a signed DisputeWatchRequest in the body of a POST."""
    net = world.network
    url = net.url + "/disputes/watch"
    if signer is None:
        return world.client.post(url, json={})
    asked = DisputeWatchRequest(party=party.ss58, cursor=0, wait=0)
    signed = net.signed(signer, "DisputeWatchRequest", asked, expiry=net.now + REQUEST_HORIZON)
    return world.client.post(url, json=signed)


def header_signed_watch_feed(world: World, party: Keypair, signer: Keypair) -> Any:
    """The retired form of the same ask: a GET whose only proof is `signer`'s signature in the
    X-Dispute-Signature header, over a message that names no expiry."""
    message = f"hypertrain/watch/2|{world.run_id}|{party.ss58}".encode()
    return world.client.get(
        world.network.url + "/disputes?party=" + party.ss58,
        headers={"X-Dispute-Signature": signer.sign(message).hex()},
    )


def witnesses(
    world: World, turn: Turn, keys: tuple[Keypair, Keypair], now: int
) -> list[dict[str, Any]]:
    """Two signed healthy Availability observations of `turn` at beacon `now`."""
    made = []
    for key in keys:
        proof = Availability(
            run_id=world.run_id,
            dispute_id=turn.dispute_id,
            seq=turn.seq,
            transcript_hash=turn.transcript_hash,
            observed_beacon=now,
            service_healthy=True,
            beacon_healthy=True,
            reference_healthy=True,
            signer=key.ss58,
            sig="",
        )
        made.append(proof.model_copy(update={"sig": key.sign(signed_message(proof)).hex()}).body())
    return made


# --- a real forged-commit round: the e2e modules are borrowed by path -----------------------


def heavy_modules() -> tuple[ModuleType, ModuleType]:
    """tests/e2e/test_rollback_service_v2 and test_dispute_exit_v2_e2e, loaded once by path.

    The second imports the first by its plain name, so that name must resolve to the module
    that built the shared fixtures: the collected one when the e2e directory is part of the
    session, else the copy loaded here.
    """
    rollback = sys.modules.get("test_rollback_service_v2") or load_fixture_module(
        "li_rollback_e2e", "e2e/test_rollback_service_v2.py"
    )
    sys.modules.setdefault("test_rollback_service_v2", rollback)
    return rollback, load_fixture_module("li_exit_e2e", "e2e/test_dispute_exit_v2_e2e.py")
