"""Builders private to the escrow_laws package of the ledger-invariants lot. Not collected.

They compose the lot's `support` with the fixture module of `tests/ledger/test_escrow_v2.py`:
every function takes that module as its first argument, so one loaded copy is used throughout.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass, replace
from types import ModuleType
from typing import Any

from hypertrain.challenge.disputes_v2 import DisputeError
from hypertrain.ledger.escrow_v2 import (
    EscrowV2,
    FinalityEvidence,
    Settlement,
    authority_message,
    digest,
    genesis_allocation_hash,
    genesis_origin_id,
    reward_origin_id,
)
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, ReplayVerdict
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    CommitV2,
    EscrowLock,
    EscrowOperation,
    EscrowRelease,
    EscrowTransfer,
    OriginAllocation,
    RewardFinalize,
    TestGenesis,
)

from .support import H, Real, bare_escrow

COLD_UNITS = 20_000
OTHER_UNITS = 10_000


@dataclass(frozen=True)
class Funded:
    """A test-mode ledger after genesis: COLD owns `cold`, OTHER owns `other`."""

    escrow: EscrowV2
    cold: str
    other: str


def signed_genesis(
    fixture: ModuleType,
    escrow: EscrowV2,
    *,
    key: Keypair | None = None,
    run_id: str | None = None,
) -> TestGenesis:
    """The policy-pinned genesis allocation, signed by `key` (default: the coordinator)."""
    allocation = [(fixture.COLD.ss58, COLD_UNITS, 1), (fixture.OTHER.ss58, OTHER_UNITS, 1)]
    pinned = genesis_allocation_hash(allocation)
    origins = [
        OriginAllocation(
            origin_id=genesis_origin_id(escrow.run_id, pinned, index),
            owner=owner,
            units=units,
            mature_at=at,
        )
        for index, (owner, units, at) in enumerate(allocation)
    ]
    unsigned = TestGenesis(
        run_id=run_id or escrow.run_id,
        allocation_hash=pinned,
        total_units=COLD_UNITS + OTHER_UNITS,
        origins=origins,
        authority_sig="0" * 128,
    )
    signature = (key or fixture.COORD).sign(authority_message(unsigned)).hex()
    return TestGenesis.model_validate({**unsigned.body(), "authority_sig": signature})


def funded(
    fixture: ModuleType,
    *,
    settlement: Callable[[str], Settlement] | Mapping[str, Settlement] | None = None,
    matured: bool = True,
) -> Funded:
    """Genesis on a private in-memory ledger; `matured=False` leaves the units reward_pending."""
    escrow = bare_escrow(fixture, fixture.setup(), settlement=settlement)
    genesis = signed_genesis(fixture, escrow)
    escrow.test_genesis(genesis)
    cold, other = (origin.origin_id for origin in genesis.origins)
    if matured:
        escrow.mature(H, (cold, other), 1)
    return Funded(escrow, cold, other)


def lock_op(
    fixture: ModuleType,
    origins: str | list[str],
    *,
    operation: str = "22" * 32,
    units: int = 1000,
    owner: str | None = None,
    kind: str = "LOCK_ADMISSION",
    reference: str = H,
) -> EscrowLock:
    """A lock of COLD; `reference` is its admission id, or its dispute id for a contest."""
    contest = kind == "LOCK_CONTEST"
    return EscrowLock(
        operation_id=operation,
        owner=owner or fixture.COLD.ss58,
        units=units,
        origin_ids=[origins] if isinstance(origins, str) else origins,
        admission_id=None if contest else reference,
        dispute_id=reference if contest else None,
        kind=kind,  # type: ignore[arg-type]
    )


def transfer_op(
    fixture: ModuleType,
    origin: str,
    *,
    operation: str = "33" * 32,
    units: int = 500,
    owner: str | None = None,
    recipient: str | None = None,
) -> EscrowTransfer:
    return EscrowTransfer(
        operation_id=operation,
        owner=owner or fixture.COLD.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
        recipient=recipient or fixture.OTHER.ss58,
    )


def pay_op(
    fixture: ModuleType,
    origin: str,
    *,
    operation: str = "55" * 32,
    units: int = 500,
    owner: str | None = None,
) -> EscrowOperation:
    return EscrowOperation(
        operation_id=operation,
        owner=owner or fixture.COLD.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )


def release_op(
    lock: EscrowLock,
    *,
    operation: str = "44" * 32,
    units: int | None = None,
    owner: str | None = None,
    admission_id: str | None = None,
    finality_hash: str = H,
    closed_dispute_root: str = H,
    lock_id: str | None = None,
) -> EscrowRelease:
    """A release of `lock` that matches it in every reference unless a keyword says otherwise."""
    return EscrowRelease(
        operation_id=operation,
        owner=owner or lock.owner,
        units=units or lock.units,
        origin_ids=lock.origin_ids,
        admission_id=admission_id if admission_id is not None else lock.admission_id,
        dispute_id=lock.dispute_id,
        lock_id=lock_id or lock.operation_id,
        finality_hash=finality_hash,
        closed_dispute_root=closed_dispute_root,
    )


def settled(
    outcome: str = "MATCH",
    *,
    unresolved: bool = False,
    release_beacon: int = 10,
    finality_hash: str = H,
    closed_dispute_root: str = H,
) -> Settlement:
    return Settlement(
        finality_hash=finality_hash,
        closed_dispute_root=closed_dispute_root,
        release_beacon=release_beacon,
        unresolved=unresolved,
        outcome=outcome,  # type: ignore[arg-type]
    )


def fingerprint(escrow: EscrowV2) -> tuple[Any, ...]:
    """Every unit row and every event (hash chain included): equal before and after a refusal."""
    units = escrow.db.execute(
        "SELECT * FROM escrow_units ORDER BY origin,owner,bucket,ref"
    ).fetchall()
    events = escrow.db.execute(
        "SELECT seq,operation_id,event_hash FROM escrow_events ORDER BY seq"
    ).fetchall()
    return tuple(map(tuple, units)), tuple(map(tuple, events))


def events(escrow: EscrowV2, kind: str | None = None) -> int:
    where, params = ("WHERE kind=?", (kind,)) if kind else ("", ())
    row = escrow.db.execute(f"SELECT COUNT(*) FROM escrow_events {where}", params).fetchone()
    return int(row[0])


def dispute_code(call: Callable[[], object]) -> str:
    """Code of the DisputeError `call` raises; any other outcome fails."""
    try:
        call()
    except DisputeError as error:
        return error.code
    except Exception as error:
        raise AssertionError(
            f"expected a DisputeError, got {type(error).__name__}: {error}"
        ) from error
    raise AssertionError("expected a DisputeError, nothing was raised")


def signed_by(reward: RewardFinalize, key: Keypair) -> RewardFinalize:
    """`reward` with its authority signature made by `key` instead of the coordinator."""
    signature = key.sign(authority_message(reward)).hex()
    return RewardFinalize.model_validate({**reward.body(), "authority_sig": signature})


def round_variant(
    fixture: ModuleType, real: Real, w: int
) -> tuple[RewardFinalize, FinalityEvidence]:
    """The fixture's single audited worker finalized for round `w` instead of round 0."""
    setup, evidence, reward = real
    run_id = setup.manifest.run_id()
    item = evidence.works[0]
    commit = CommitV2.model_validate({**item.commit.body, "w": w})
    challenge = AuditChallengeV2.model_validate({**item.challenge.body, "w": w})
    verdict = ReplayVerdict.model_validate(
        {**item.verdict.body, "challenge_hash": digest(challenge)}
    )
    final = Finalize.model_validate({**evidence.finalize.body, "w": w})
    work = replace(
        item,
        commit=EnvelopeV2.model_validate(seal(fixture.HOT, "CommitV2", run_id, commit, 200)),
        verdict=EnvelopeV2.model_validate(
            seal(fixture.AUDITOR, "ReplayVerdict", run_id, verdict, 200)
        ),
        challenge=EnvelopeV2.model_validate(
            seal(fixture.COORD, "AuditChallengeV2", run_id, challenge, 200)
        ),
    )
    changed = replace(
        evidence,
        finalize=EnvelopeV2.model_validate(seal(fixture.COORD, "Finalize", run_id, final, 200)),
        works=(work,),
    )
    origin = OriginAllocation(
        origin_id=reward_origin_id(run_id, w, fixture.HOT.ss58, digest(final)),
        owner=item.owner,
        units=reward.budget_units,
        mature_at=reward.mature_at,
    )
    unsigned = RewardFinalize.model_validate(
        {
            **reward.body(),
            "w": w,
            "finalize_hash": digest(final),
            "verdict_root": sha256_hex(canonicalize([digest(verdict)])),
            "allocation_hash": sha256_hex(canonicalize([origin.body()])),
            "origin_ids": [origin.origin_id],
            "authority_sig": "0" * 128,
        }
    )
    return signed_by(unsigned, fixture.COORD), changed
