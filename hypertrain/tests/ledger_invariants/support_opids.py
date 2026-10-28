"""Helpers of the opids package: client events that take an operation id, seeded collateral and
the dispute world whose settlement lookup is the store's own. Not collected: no test_ prefix."""

from __future__ import annotations

import dataclasses
from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
from pathlib import Path
from types import ModuleType
from typing import Any

from pydantic import ValidationError

from env_pin_fixtures import honest_env
from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.beacon.core import FixtureBeacon
from hypertrain.challenge.admission import Admission
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.disputes_v2 import Contest, DisputeError, DisputesV2, Turn
from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import Params
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowReceipt, EscrowV2, Settlement, digest
from hypertrain.miner.admission import sign_join
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    DisputePolicyV2,
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    EscrowTransfer,
    HardwareHint,
    ResolutionV2,
    RunManifestV2,
)

from .support import H, bare_escrow

PREFIX_SEED = b"li-opids-prefix-key"
RECIPIENT = Keypair(b"\x0d" * 32)  # receives the units of a client transfer; no fixture funds it


def fresh_id(label: str) -> str:
    """An operation id nobody derives: it cannot collide with a service-derived one."""
    return sha256_hex(f"li-opids|{label}".encode())


class StepRefused(AssertionError):
    """The service refused a step it takes for its own account."""


class DefenceMissing(AssertionError):
    """A refusal the protocol must hold did not happen."""


def service_step[T](step: Callable[[], T]) -> T:
    """Run a step the service takes for its own account.

    A refusal is reported as `StepRefused` carrying its code, so a known hole is told apart from
    a broken control (a plain AssertionError) and the failure names its reason."""
    try:
        return step()
    except (EscrowError, DisputeError) as error:
        raise StepRefused(f"the service step was refused: {error.code}") from error


def dispute_code(call: Callable[[], object]) -> str:
    """Code of the DisputeError `call` raises; any other outcome fails the test."""
    try:
        call()
    except DisputeError as error:
        return error.code
    except Exception as error:
        raise AssertionError(
            f"expected a DisputeError, got {type(error).__name__}: {error}"
        ) from error
    raise AssertionError("expected a DisputeError, nothing was raised")


class ModelRefused(Exception):
    """The client event model refused the operation id before any ledger saw the event.

    The ids a server derives may live in a key space no client id can name (ADR 0014, part 1: the
    prefix `srv:` is not a Hex64), so signing one is refused by validation: still a refusal."""


def refused(call: Callable[[], object]) -> bool:
    """True when the ledger, the dispute or admission service or the event model refuses `call`,
    False when it is accepted."""
    try:
        call()
    except (EscrowError, DisputeError, AdmissionError, ModelRefused):
        return True
    return False


def client_model[T](build: Callable[[], T]) -> T:
    """Build a client event; a refusal of its operation id by the model is `ModelRefused`, any
    other invalid field is a bug of the test and stays a ValidationError."""
    try:
        return build()
    except ValidationError as error:
        if {issue["loc"] for issue in error.errors()} != {("operation_id",)}:
            raise
        raise ModelRefused(str(error)) from error


def must_refuse(call: Callable[[], object], what: str) -> None:
    if not refused(call):
        raise DefenceMissing(what)


def self_transfer(owner: str, operation_id: str, origin: str, units: int = 1) -> EscrowTransfer:
    """A net-zero client event: the owner sends `units` of `origin` to itself."""
    return EscrowTransfer(
        operation_id=operation_id,
        owner=owner,
        recipient=owner,
        units=units,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )


def transfer_to_recipient(
    owner: str, operation_id: str, origin: str, units: int = 1
) -> EscrowTransfer:
    """A client event any funded key can sign: the owner sends `units` of `origin` to a key that
    holds nothing. The ledger refuses a transfer to oneself (SELF_TRANSFER), so the cheapest event
    a bystander can sign moves a unit away."""
    return EscrowTransfer(
        operation_id=operation_id,
        owner=owner,
        recipient=RECIPIENT.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )


def contest_lock(
    owner: str, operation_id: str, origin: str, dispute_id: str = H, units: int = 1
) -> EscrowLock:
    """A bond lock any funded key can sign: it needs no admission."""
    return EscrowLock(
        operation_id=operation_id,
        owner=owner,
        units=units,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )


def client_event(
    escrow: EscrowV2, kind: str, key: Keypair, operation_id: str, origin: str
) -> EscrowReceipt:
    """What a funded key can sign on the escrow routes, under an id of its choice."""
    if kind == "transfer":
        event = client_model(lambda: transfer_to_recipient(key.ss58, operation_id, origin))
        return escrow.transfer(event, signer=key.ss58)
    assert kind == "lock"
    return escrow.lock(
        client_model(lambda: contest_lock(key.ss58, operation_id, origin)), signer=key.ss58
    )


def seed_available(escrow: EscrowV2, origin: str, owner: str, units: int) -> None:
    """Matured collateral held by `owner`, as a production bootstrap leaves it: no hotkey, never
    payable. Written directly because the bootstrap allocation a production fixture pins cannot
    be signed again here; the engine's own conservation check still runs on every event."""
    escrow.db.execute("INSERT INTO escrow_origins VALUES(?,?,?,NULL,NULL)", (origin, units, 1))
    escrow.db.execute(
        "INSERT INTO escrow_units VALUES(?,?,'available','',?)", (origin, owner, units)
    )


def collateral_escrow(fixture: ModuleType, setup: Any, **options: Any) -> tuple[EscrowV2, str]:
    """A production ledger on which OTHER holds 1,000 available units: the funded bystander."""
    escrow = bare_escrow(fixture, setup, **options)
    origin = fresh_id("collateral-origin")
    seed_available(escrow, origin, fixture.OTHER.ss58, 1000)
    return escrow, origin


@dataclasses.dataclass
class DisputeWorld:
    """A test-mode ledger and a DisputesV2 on the connection of one ChallengeStore, so that the
    escrow's settlement callback is `ChallengeStore._settlement_v2` and its dispute-first lookup.
    COLD (20,000) and OTHER (10,000) hold available units; HOT is owned by COLD, MINER_TWO by OTHER.
    """

    store: ChallengeStore
    escrow: EscrowV2
    disputes: DisputesV2
    fixture: ModuleType
    referee: Keypair
    next_referee: Keypair
    miner_two: Keypair
    run_id: str
    origins: tuple[str, str]  # genesis origins of COLD and OTHER
    contests: dict[str, Contest]
    admission: Admission | None = None  # the production resolver of admission locks, when asked

    def join(self, hotkey: Keypair, coldkey: Keypair, request_id: str) -> str:
        """Admit `hotkey` under `coldkey` through the service (needs `admission=True`);
        returns the admission id."""
        assert self.admission is not None, "dispute_world(..., admission=True) installs it"
        policy = self.admission.policy
        request = sign_join(
            hotkey,
            coldkey,
            run_id=self.run_id,
            request_id=request_id,
            expires_beacon=10000,
            policy_hash=digest(policy),
            hardware_hint=HardwareHint(device_name="advisory", device_count=1, driver="cpu-test"),
            env=honest_env(),
        )
        record = self.admission.join(canonicalize(request.body()), now=1, ip_prefix="loopback")
        return record.admission_id

    def add_contest(self, verdict_hash: str, miner: Keypair) -> str:
        """Register the accepted verdict `disputes.open` reads; returns the dispute id."""
        self.contests[verdict_hash] = Contest(
            verdict_hash=verdict_hash,
            challenge_hash="22" * 32,
            w=0,
            miner=miner.ss58,
            auditor=self.fixture.AUDITOR.ss58,
            referee=self.referee.ss58,
            step_span=2,
            layer_span=2,
            op_span=2,
        )
        return sha256_hex(canonicalize([self.run_id, verdict_hash, miner.ss58]))

    def bond(
        self, owner: Keypair, operation_id: str, dispute_id: str, origin: str, units: int = 100
    ) -> EscrowLock:
        lock = contest_lock(owner.ss58, operation_id, origin, dispute_id, units)
        self.escrow.lock(lock, signer=owner.ss58)
        return lock

    def open(self, miner: Keypair, verdict_hash: str, bond_lock: str, now: int) -> Turn:
        request = DisputeV2(
            hotkey=miner.ss58, verdict_hash=verdict_hash, action="contest", bond_lock=bond_lock
        )
        raw = canonicalize(seal(miner, "DisputeV2", self.run_id, request, 100000))
        return self.disputes.open(raw, now=now)

    def resolve_match(self, turn: Turn, now: int) -> Turn:
        """The referee's MATCH resolution on evidence the service registered itself."""
        evidence = RefereeEvidence(
            dispute_id=turn.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason="MATCH",
            loser=None,
            predecessor_hash="11" * 32,
            inputs_hash="22" * 32,
            output_hash="33" * 32,
            op_spec="full-match",
        )
        self.disputes.register_evidence(evidence)
        resolution = ResolutionV2(
            dispute_id=turn.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason="MATCH",
            loser=None,
            evidence_hash=evidence.digest(),
        )
        raw = canonicalize(seal(self.referee, "ResolutionV2", self.run_id, resolution, 100000))
        return self.disputes.resolution(raw, now=now)

    def release_of(
        self, owner: Keypair, lock: EscrowLock, status: Turn, operation_id: str, origin: str
    ) -> EscrowRelease:
        """The release a lock owner signs when it quotes the closed dispute `status`."""
        assert status.resolution is not None
        return EscrowRelease(
            operation_id=operation_id,
            owner=owner.ss58,
            units=lock.units,
            origin_ids=[origin],
            admission_id=lock.admission_id,
            dispute_id=lock.dispute_id,
            lock_id=lock.operation_id,
            finality_hash=status.resolution.digest(),
            closed_dispute_root=status.transcript_hash,
        )


def no_trial_staged(*_: object) -> Any:
    raise AssertionError("no trial is staged in this world")


@contextmanager
def dispute_world(
    root: Path, fixture: ModuleType, *, admission: bool = False
) -> Iterator[DisputeWorld]:
    """`fixture` is the lot's tests/ledger/test_escrow_v2.py module (the `li_escrow` fixture).

    `admission=True` also builds the `Admission` service on the same ledger, as the store does:
    it installs the resolver that settles an admission lock (ADR 0010, part 6)."""
    referee, next_referee = Keypair(b"\x71" * 32), Keypair(b"\x72" * 32)
    policy = DisputePolicyV2(
        referees=[referee.ss58, next_referee.ss58],
        max_open=32,
        max_transcript_entries=256,
        max_entry_bytes=65536,
        fanout=2,
        max_referee_reassignments=1,
        max_referee_cost_units=100,
    )
    setup = fixture.setup()
    body = setup.manifest.body()
    body["training"]["inner"].update(H=2, J=1)
    body["network"]["dispute_policy_hash"] = policy.digest()
    setup = dataclasses.replace(setup, manifest=RunManifestV2.model_validate(body))
    run_id = setup.manifest.run_id()
    store = ChallengeStore(
        root / "state",
        Params("hypertrain", setup.manifest.training.beacon.genesis_time - 600, 300, 1, 1),
        None,
        None,
        lambda payload: None,
        LocalFSStore(root / "objects"),
    )
    miner_two = Keypair(b"\x0c" * 32)
    owners: Mapping[str, str] = {
        fixture.HOT.ss58: fixture.COLD.ss58,
        miner_two.ss58: fixture.OTHER.ss58,
    }
    contests: dict[str, Contest] = {}
    try:
        escrow: EscrowV2 = fixture.ledger(
            setup, store._db, settlement=lambda reference: store._settlement_v2(run_id, reference)
        )
        escrow.mutex = store._lock
        store._db.execute(
            "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
            (run_id, setup.manifest.model_dump_json(), "{}"),
        )
        origins = fixture.fund(escrow)
        service = (
            Admission(
                escrow,
                setup.admission_policy,
                fixture.COORD,
                beacon=FixtureBeacon(current=100000).get,
                stage_reference=no_trial_staged,
                objects=LocalFSStore(root / "admission-objects"),
                ip_secret=PREFIX_SEED,
                qualified=lambda _: True,
                conservative_bound=lambda: True,
                backend="cpu",
            )
            if admission
            else None
        )
        disputes = DisputesV2(
            escrow,
            policy,
            fixture.COORD,
            contest_of=lambda verdict: contests[verdict],
            owner_of=lambda hotkey: owners[hotkey],
            span=lambda contest, level, ctx: 2,
        )
        store._services_v2[run_id] = (escrow, None, disputes)
        yield DisputeWorld(
            store,
            escrow,
            disputes,
            fixture,
            referee,
            next_referee,
            miner_two,
            run_id,
            origins,
            contests,
            service,
        )
    finally:
        store.close()


def seed_reward_origin(
    world: DisputeWorld, origin: str, owner: str, units: int, mature_at: int, finalize_hash: str
) -> None:
    """The rows `reward_finalize` and the service's finalize leave for one live reward origin
    (pending units, round 0 finalized, the accepted settlement record), written directly: the
    real reward of the lot is bound to a manifest that pins no dispute policy."""
    db = world.escrow.db
    db.execute("INSERT INTO escrow_origins VALUES(?,?,?,0,?)", (origin, units, mature_at, "hot"))
    db.execute("INSERT INTO escrow_units VALUES(?,?,'reward_pending','',?)", (origin, owner, units))
    db.execute("INSERT INTO escrow_finalized VALUES(0,?)", (finalize_hash,))
    world.store._put_record_v2(
        world.run_id,
        "settlement",
        origin,
        Settlement(
            finality_hash=finalize_hash,
            closed_dispute_root=sha256_hex(canonicalize([])),
            release_beacon=mature_at,
            unresolved=False,
            outcome="MATCH",
        ).body(),
    )
