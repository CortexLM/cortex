"""Helpers private to the shadow-reward package of the ledger-invariants lot. Not collected.

The ledger half builds a bare EscrowV2 around the real w=0 work of `real_reward` and signs shadow
rewards the way the fixture module does. The service half re-derives the signature chain of a
shadow acceptance after a test changed one field of it, so a refusal is the refusal of that
field and not of a stale hash or signature.
"""

from __future__ import annotations

import copy
import hashlib
import sqlite3
from dataclasses import dataclass, replace
from types import ModuleType
from typing import Any

import httpx
from pydantic import create_model

from hypertrain.ledger.escrow_v2 import (
    EscrowReceipt,
    EscrowV2,
    FinalityEvidence,
    authority_message,
)
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    RewardFinalize,
    RotateRequest,
    ShadowRewardFinalizeV1,
)
from hypertrain.protocol.messages_v2 import (
    ShadowReservationV1 as Reservation,
)

from .support import Real, bare_escrow, error_code, match_settlement, table_names

UNIT = 1_000_000  # round_reward_units of the fixture policy
MATURE_AT = 20  # vesting round of the fixture reward


def admission_id(index: int) -> str:
    return format(index + 1, "02x") * 32


def resign_shadow(
    fixture: ModuleType, reward: ShadowRewardFinalizeV1, **changes: Any
) -> ShadowRewardFinalizeV1:
    """`reward` with `changes` applied and the authority signature made again by COORD."""
    unsigned = ShadowRewardFinalizeV1.model_validate(
        {**reward.body(), **changes, "authority_sig": "0" * 128}
    )
    signature = fixture.COORD.sign(authority_message(unsigned)).hex()
    return unsigned.model_copy(update={"authority_sig": signature})


@dataclass
class Engine:
    """A bare ledger holding the real work of `real_reward`, for shadow reservations and rewards."""

    fixture: ModuleType
    escrow: EscrowV2
    evidence: FinalityEvidence
    live: RewardFinalize
    setup: Any

    @property
    def facts(self) -> FinalityEvidence:
        return replace(self.evidence, shadow=True)

    def reserve(self, index: int = 0) -> Reservation:
        return self.fixture.reserve_actual_shadow(self.escrow, self.evidence, admission_id(index))

    def reward(self, binding: Reservation) -> ShadowRewardFinalizeV1:
        return self.fixture.sign_actual_shadow(self.live, binding)

    def origin(self, binding: Reservation) -> str:
        """The origin id the ledger issues for `binding`, as the reward authority signs it.

        The ledger refuses a reward whose origin ids differ from its own derivation, so the id of
        a signed reward is that derivation whatever key space the ledger derives it in.
        """
        return self.reward(binding).origin_ids[0]

    def accept(self, binding: Reservation) -> EscrowReceipt:
        return self.escrow.reward_finalize(self.reward(binding), self.facts)

    def events(self, kind: str | None = None) -> int:
        query, args = "SELECT COUNT(*) FROM escrow_events", ()
        if kind is not None:
            query, args = query + " WHERE kind=?", (kind,)
        return int(self.escrow.db.execute(query, args).fetchone()[0])

    def marker(self) -> tuple[Any, ...]:
        """Everything a refused operation must leave alone."""
        db = self.escrow.db
        return (
            self.escrow.balances(),
            self.events(),
            db.execute("SELECT COUNT(*) FROM admission_reservations").fetchone()[0],
            self.escrow._snapshot(),
        )

    def limit_to(self, awards: int) -> None:
        """Pin the issuance cap at `awards` rounds: a test-owned stand-in for a larger policy."""
        policy = self.escrow.policy
        self.escrow.policy = policy.model_copy(
            update={"max_total_issuance": awards * policy.round_reward_units}
        )

    def pending(self) -> int:
        return int(
            self.escrow.db.execute(
                "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-reservation' "
                "AND json_extract(data,'$.accepted') IS NULL"
            ).fetchone()[0]
        )


def engine(fixture: ModuleType, real: Real, *, awards: int | None = None) -> Engine:
    """An in-memory ledger whose settlement accepts the fixture Finalize as a clean MATCH."""
    setup, evidence, live = real
    escrow = bare_escrow(
        fixture,
        setup,
        settlement=lambda _reference: match_settlement(live.finalize_hash, MATURE_AT),
    )
    built = Engine(fixture, escrow, evidence, live, setup)
    if awards is not None:
        built.limit_to(awards)
    return built


def file_engine(fixture: ModuleType, real: Real, path: Any) -> Engine:
    """The same ledger on a file, so a test can reopen it."""
    from .support import known_replay

    setup, evidence, live = real
    escrow = fixture.ledger(
        setup,
        path,
        settlement=lambda _reference: match_settlement(live.finalize_hash, MATURE_AT),
        replay=known_replay(fixture),
    )
    return Engine(fixture, escrow, evidence, live, setup)


refusal_code = error_code  # the lot's helper under the name the tests of this package use


def as_live_class(fixture: ModuleType, reward: ShadowRewardFinalizeV1) -> ShadowRewardFinalizeV1:
    """`reward` carrying the reward authority's signature over its own body as a live reward.

    A stand-in class with the shadow fields and the name RewardFinalize gives the message the live
    domain would sign for this body, so only the domain separation of `authority_message` can
    refuse it.
    """
    live_named = create_model("RewardFinalize", __base__=ShadowRewardFinalizeV1)
    stand_in = live_named.model_validate(reward.model_dump())
    signature = fixture.COORD.sign(authority_message(stand_in)).hex()
    return reward.model_copy(update={"authority_sig": signature})


def db_digest(db: sqlite3.Connection) -> str:
    """Hash of every row of every table of one SQLite connection."""
    hasher = hashlib.sha256()
    for name in table_names(db):
        rows = sorted(repr(tuple(row)) for row in db.execute(f'SELECT * FROM "{name}"'))
        hasher.update(repr((name, rows)).encode())
    return hasher.hexdigest()


def reauthenticate(
    n: Any,
    body: dict[str, Any],
    *,
    audit: dict[str, Any] | None = None,
    verdict: dict[str, Any] | None = None,
    receipt: dict[str, Any] | None = None,
    reward: dict[str, Any] | None = None,
    receipt_signer: Any = None,
    verdict_signer: Any = None,
) -> dict[str, Any]:
    """A copy of a shadow-acceptance body with the given fields changed and every hash and
    signature that depends on them made again, so only those fields differ from the original.

    The signers are the in-memory test keys of the network fixture: the helper isolates which
    check refused a change, it cannot sign for a deployed service."""
    from hypertrain.protocol.messages_v2 import ShadowRewardFinalizeV1 as Reward

    from .support import SERVICE_FIXTURE, load_fixture_module

    service = load_fixture_module(*SERVICE_FIXTURE)
    out = copy.deepcopy(body)
    out["audit_challenge"]["body"].update(audit or {})
    out["verdict"]["body"]["challenge_hash"] = body_digest(out["audit_challenge"]["body"])
    out["verdict"]["body"].update(verdict or {})
    out["auditor_receipt"]["body"]["audit_challenge_hash"] = out["verdict"]["body"][
        "challenge_hash"
    ]
    out["auditor_receipt"]["body"]["verdict_hash"] = body_digest(out["verdict"]["body"])
    out["auditor_receipt"]["body"].update(receipt or {})
    out["audit_challenge"] = n.signed(
        service.COORD, "AuditChallengeV2", out["audit_challenge"]["body"]
    )
    out["verdict"] = n.signed(
        verdict_signer or service.AUDITORS[0], "ReplayVerdict", out["verdict"]["body"]
    )
    out["auditor_receipt"] = n.signed(
        receipt_signer or service.AUDITORS[0],
        "ShadowReplayReceiptV1",
        out["auditor_receipt"]["body"],
    )
    out["reward"]["tape_hash"] = sha256_hex(canonicalize(out["auditor_receipt"]))
    out["reward"]["verdict_root"] = sha256_hex(canonicalize([body_digest(out["verdict"]["body"])]))
    out["reward"].update(reward or {})
    signed = Reward.model_validate({**out["reward"], "authority_sig": "0" * 128})
    out["reward"]["authority_sig"] = service.COORD.sign(authority_message(signed)).hex()
    return out


def push_round(n: Any, number: int) -> httpx.Response:
    """Post drand round `number` and return the answer, whatever it is."""
    from .support import SERVICE_FIXTURE, load_fixture_module

    service = load_fixture_module(*SERVICE_FIXTURE)
    n.now = number
    n.store.clock = lambda: n.manifest.training.beacon.genesis_time + (number - 1) * 3
    return n.client.post(
        "/v1/admin/beacon", json=service.fixture_beacon(number), headers=service.admin()
    )


def open_challenge(n: Any, identity: str) -> httpx.Response:
    """GET the trial challenge of `identity` as its own hotkey.

    The route asks the admission's hotkey for a signature (RTA-04, ADR 0010); an older service
    answered an unsigned request, so a network fixture without `challenge` is asked that way.
    """
    if hasattr(n, "challenge"):
        return n.challenge(identity)
    return n.client.get(n.url + f"/join/{identity}/challenge")


def rotate_admission(n: Any, cold: Any, old: Any, new: Any, operation_id: str = "df" * 32) -> Any:
    """POST the rotation of `old` to `new` sealed by `cold`, with the new hotkey's consent when
    the service asks for it (RTA-01, ADR 0010), and return the answer."""
    from hypertrain.miner import admission as client

    run_id, expires = n.manifest.run_id(), 1000
    if "new_hot_sig" in RotateRequest.model_fields:
        request = client.sign_rotation_consent(
            new,
            run_id=run_id,
            coldkey=cold.ss58,
            hotkey=old.ss58,
            operation_id=operation_id,
            expires_beacon=expires,
        )
    else:
        request = RotateRequest.model_validate(
            {"hotkey": old.ss58, "new_hotkey": new.ss58, "operation_id": operation_id}
        )
    return n.client.post(
        n.url + f"/admission/{old.ss58}/rotate",
        json=client.sign_rotation(cold, run_id, request, expires),
    )


def add_due_origin(
    escrow: EscrowV2, owner: str, units: int, mature_at: int, origin: str = "bb" * 32
) -> str:
    """A pending origin that vests with no settlement (like a bootstrap or genesis origin).

    It stands for any other origin the beacon route has to mature in the same transaction as a
    shadow origin.
    """
    with escrow.tx():
        escrow.db.execute(
            "INSERT INTO escrow_origins VALUES(?,?,?,?,?)", (origin, units, mature_at, None, None)
        )
        escrow.db.execute(
            "INSERT INTO escrow_units VALUES(?,?,?,?,?)",
            (origin, owner, "reward_pending", "", units),
        )
    return origin
