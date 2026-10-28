"""Finality on the real v2 service: finalize, open, aggregate and rollback of a staged run.

`Stage` (support_finality) is built once per module: five ACTIVE identities, round 0 audited with
one forged commit (four MATCH, one MISMATCH) and aggregated, round 1 aggregated for the four honest
miners. Every test starts from a byte-identical copy of that database and pins a refusal by its
exception type and text and by an unchanged database. A defence that does not hold today is a
strict xfail that names the invariant.
"""

from __future__ import annotations

import json
import sqlite3
from collections import Counter
from collections.abc import Callable, Iterator
from typing import Any

import pytest

from hypertrain.aggregator.core import FinalityError
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowReceipt, EscrowV2
from hypertrain.protocol.envelope import ExpiredError, UnauthorizedSigner, body_digest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair

from . import support_exit_roster as X
from .support_finality import ANOTHER, FINAL, Stage, build_stage, refusal

# -- the staged service ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def frozen(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Stage]:
    yield from build_stage(tmp_path_factory)


@pytest.fixture
def stage(frozen: Stage) -> Stage:
    frozen.reset()
    return frozen


INCLUDED_DIFFERS = "finalized included set differs from full accepted authority"
STRANGER = Keypair(bytes([0x66]) * 32).ss58


def honest_hotkeys(stage: Stage) -> list[str]:
    return sorted(key.ss58 for key in stage.honest)


def credits(stage: Stage) -> Counter[tuple[int, str]]:
    return Counter(
        (int(row[0]), str(row[1]))
        for row in stage.rows(
            "SELECT reward_round, hotkey FROM escrow_origins WHERE reward_round IS NOT NULL"
        )
    )


def finalize_effects(stage: Stage) -> list[tuple[str, str, str]]:
    return stage.records(
        "finality", "finalize", "replayed-finality", "finalize-result", "settlement"
    )


def nothing_was_finalized(stage: Stage) -> None:
    assert stage.rows("SELECT COUNT(*) FROM escrow_finalized") == [(0,)]
    assert stage.rows("SELECT COUNT(*) FROM escrow_events WHERE kind='REWARD_FINALIZE'") == [(0,)]
    assert credits(stage) == Counter()
    assert stage.record("finality", "0")["disposition"] == "PENDING"
    assert stage.records("finalize", "finalize-result", "replayed-finality", "settlement") == []
    assert not stage.store._db.in_transaction


# -- FIN-01: only the coordinator settles, and only the audited set ------------------------------

FINALIZE_REFUSED = [
    pytest.param(
        lambda s: (0, s.signed_finalize(signer=s.honest[0])),
        (UnauthorizedSigner, None, "signer is not authorized for message role"),
        id="a miner signs",
    ),
    pytest.param(
        lambda s: (0, s.signed_finalize(signer=s.fixtures.AUDITORS[0])),
        (UnauthorizedSigner, None, "signer is not authorized for message role"),
        id="an auditor signs",
    ),
    pytest.param(
        lambda s: (0, s.signed_finalize(expiry=1)),
        (ExpiredError, None, "envelope expired"),
        id="the envelope has expired",
    ),
    pytest.param(
        lambda s: (0, s.signed_finalize(included=honest_hotkeys(s)[:3])),
        (ChallengeError, 422, INCLUDED_DIFFERS),
        id="an audited miner is left out",
    ),
    pytest.param(
        lambda s: (0, s.signed_finalize(included=sorted([*honest_hotkeys(s), s.forger.ss58]))),
        (ChallengeError, 422, INCLUDED_DIFFERS),
        id="the excluded miner is added",
    ),
    pytest.param(
        lambda s: (0, s.signed_finalize(included=sorted([*honest_hotkeys(s), STRANGER]))),
        (ChallengeError, 422, INCLUDED_DIFFERS),
        id="a stranger is added",
    ),
    pytest.param(
        lambda s: (0, s.signed_finalize(1)),
        (ChallengeError, 422, INCLUDED_DIFFERS),
        id="the body of another round",
    ),
    pytest.param(
        lambda s: (0, s.signed_finalize(final_theta_hash_w1=ANOTHER)),
        (FinalityError, None, "round cannot settle"),
        id="another applied theta",
    ),
]


@pytest.mark.parametrize(("build", "expected"), FINALIZE_REFUSED)
def test_finalize_refuses_everything_but_the_coordinators_complete_audited_set(
    stage: Stage,
    build: Callable[[Stage], tuple[int, dict[str, Any]]],
    expected: tuple[type[BaseException], int | None, str],
) -> None:
    route, envelope = build(stage)
    before = stage.digest()
    refused = refusal(lambda: stage.store.finalize_v2(stage.run_id, route, canonicalize(envelope)))
    assert refused == expected
    assert stage.digest() == before
    nothing_was_finalized(stage)
    # The refused round was not stuck: the corrected Finalize settles it.
    assert stage.finalize()["status"] == "SETTLED"


# -- FIN-02: the store settles only what settle() allows ------------------------------------------

STORE_REFUSED = [
    pytest.param({"unresolved_audits": 1}, "round cannot settle", id="an audit is open"),
    pytest.param({"unresolved_disputes": 1}, "round cannot settle", id="a dispute is open"),
    pytest.param(
        {"disposition": "EXCLUDED", "resolution_hash": FINAL},
        "round cannot settle",
        id="the round is excluded",
    ),
    pytest.param({"applied": False}, "round cannot settle", id="no tape was applied"),
    pytest.param(
        {"resolution_hash": FINAL}, "conflicting finality", id="another finalize was seen"
    ),
]


@pytest.mark.parametrize(("change", "message"), STORE_REFUSED)
def test_finalize_refuses_a_round_that_may_not_settle_and_writes_nothing(
    stage: Stage, change: dict[str, Any], message: str
) -> None:
    stage.put("finality", "0", stage.record("finality", "0") | change)
    before = stage.digest()
    assert refusal(stage.finalize) == (FinalityError, None, message)
    assert stage.digest() == before
    assert stage.rows("SELECT COUNT(*) FROM escrow_finalized") == [(0,)]
    assert credits(stage) == Counter()


def test_a_repair_is_model_history_and_is_never_finalized_for_a_reward(stage: Stage) -> None:
    stage.put("applied", "0", stage.record("applied", "0") | {"kind": "repair"})
    before = stage.digest()
    assert refusal(stage.finalize) == (
        ChallengeError,
        409,
        "repair evidence is model-only; cannot mint miner rewards",
    )
    assert stage.digest() == before


# -- FIN-03 / FIN-18: the audited miners are paid, nobody else ------------------------------------


def test_finalize_pays_the_four_audited_miners_and_nobody_excluded(stage: Stage) -> None:
    run, honest, forger = stage.run_id, honest_hotkeys(stage), stage.forger.ss58
    auditors = {key.ss58 for key in stage.fixtures.AUDITORS}
    for hotkey in [*honest, forger]:
        verdict = stage.record("verdict", f"0:{hotkey}")
        lease = stage.rows(
            "SELECT state, auditor FROM audit_leases_v2 WHERE run_id=? AND w=0 AND hotkey=?",
            run,
            hotkey,
        )
        assert verdict["signer"] in auditors and verdict["signer"] != hotkey
        assert lease == [("completed", verdict["signer"])]
        assert verdict["body"]["result"] == ("MISMATCH" if hotkey == forger else "MATCH")
    excluded = stage.records("excluded")
    assert [(kind, identity) for kind, identity, _ in excluded] == [("excluded", f"0:{forger}")]

    assert stage.finalize()["status"] == "SETTLED"

    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in honest})
    assert (
        sorted(stage.rows("SELECT issued FROM escrow_origins WHERE reward_round=0"))
        == [(250_000,)] * 4
    )
    escrow = stage.store._services(run)[0]
    assert escrow.balances(stage.fixtures.COLD[4].ss58).reward_pending == 0
    assert escrow.balances().reward_pending == escrow.policy.round_reward_units
    assert stage.records("excluded") == excluded
    # The same transaction left what a later vesting and a retry need: one settlement record
    # per reward origin and the stored result.
    origins = sorted(
        str(row[0]) for row in stage.rows("SELECT origin FROM escrow_origins WHERE reward_round=0")
    )
    assert [identity for _, identity, _ in stage.records("settlement")] == origins
    assert stage.record("finalize-result", "0")["status"] == "SETTLED"


def admission_of(stage: Stage, key: Keypair) -> dict[str, Any]:
    """The public admission route's answer for `key`: its record and whether it is eligible."""
    answer = stage.network.client.get(f"{stage.network.url}/admission/{key.ss58}")
    X.control(answer.status_code == 200, answer.text)
    body: dict[str, Any] = answer.json()
    return body


def sanction(stage: Stage, key: Keypair, reason: str) -> int:
    """Book `reason` against the admission of `key` through the hook the audit side and the
    referee call (the units burned): the real seizure, not an edit of the admission row."""
    admission = stage.store._services(stage.run_id)[1]
    burned: int = admission.seize_bond(
        admission.store.record(key.ss58).admission_id,
        reason=reason,
        evidence_hash=sha256_hex(f"finality-2|{reason}|{key.ss58}".encode()),
        now=stage.network.now,
    )
    return burned


SANCTIONS = [
    pytest.param("NO_SHOW", id="the bond is burned"),
    pytest.param("AUDIT_MISMATCH", id="the admission is suspended"),
]


@pytest.mark.parametrize("reason", SANCTIONS)
def test_a_member_sanctioned_after_the_aggregation_is_paid_for_the_round_it_was_verified_in(
    stage: Stage, reason: str
) -> None:
    """The applied tape is the commitment: a member that was verified and is suspended, or loses
    its bond, between the aggregation and the finalize is paid for that round, because the
    sanction acts afterwards (ADR 0011 part 8, decision 23). Only a convicted coldkey is not."""
    member, cold = stage.honest[0], stage.fixtures.COLD[0].ss58
    escrow = stage.store._services(stage.run_id)[0]
    locked = escrow.balances(cold).admission_locked
    X.control(
        locked > 0 and admission_of(stage, member)["eligible"] is True,
        "the member is funded and eligible when the tape is applied",
    )
    burned = sanction(stage, member, reason)
    admission = admission_of(stage, member)
    X.control(
        burned > 0
        and escrow.balances(cold).admission_locked == 0
        and admission["eligible"] is False
        and admission["record"]["state"] != "BANNED",
        "the bond is burned and the member is no longer eligible, but not convicted",
    )
    X.control(
        reason != "AUDIT_MISMATCH" or admission["record"]["state"] == "SUSPENDED",
        "a confirmed audit mismatch suspends the admission",
    )
    issued = escrow.balances().issued

    response = stage.post_finalize(0, stage.signed_finalize())

    assert (response.status_code, response.json().get("status")) == (200, "SETTLED"), response.text
    units = escrow.policy.round_reward_units
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in honest_hotkeys(stage)})
    assert escrow.balances(cold).reward_pending == units // 4
    assert escrow.balances().issued == issued + units
    assert escrow.balances().conserved()


# Closed by audit-integrity-2 (6d935814, D59): a Finalize lists the unconvicted inputs only.
def test_a_miner_convicted_after_its_round_was_audited_is_never_credited_by_a_finalize(
    stage: Stage,
) -> None:
    """The ban is by coldkey: the convicted miner and every other admission of its coldkey are
    unpaid (the others are not banned), and the unconvicted inputs share the whole budget."""
    banned, sibling, *rest = stage.honest
    owner, sibling_owner = stage.fixtures.COLD[0].ss58, stage.fixtures.COLD[1].ss58
    # The sibling is run by the owner of the convicted miner: several admissions may share a
    # coldkey once the first is ACTIVE, and no cheap route joins one on the staged network.
    with stage.store._tx():
        stage.store._db.execute(
            "UPDATE admissions_v2 SET coldkey=? WHERE hotkey=?", (owner, sibling.ss58)
        )
    burned = sanction(stage, banned, "TRAINING_FRAUD")
    states = {key.ss58: admission_of(stage, key)["record"]["state"] for key in stage.honest}
    X.control(
        burned > 0
        and states
        == {banned.ss58: "BANNED", sibling.ss58: "ACTIVE"} | {key.ss58: "ACTIVE" for key in rest},
        "the miner is convicted and its sibling is not banned with it",
    )
    tape = sorted(item["roster"]["hotkey"] for item in stage.record("tape-inputs", "0")["inputs"])
    X.control(tape == honest_hotkeys(stage), "the tape applied the four audited miners")
    escrow = stage.store._services(stage.run_id)[0]
    issued, units = escrow.balances().issued, escrow.policy.round_reward_units
    unconvicted = sorted(key.ss58 for key in rest)

    # A Finalize that still lists the convicted miner or its sibling is refused, writing nothing.
    before = stage.digest()
    for listed in (tape, sorted([sibling.ss58, *unconvicted])):
        stale = stage.post_finalize(0, stage.signed_finalize(included=listed))
        detail = stale.json().get("detail")
        assert (stale.status_code, detail) == (422, INCLUDED_DIFFERS), stale.text
        assert stage.digest() == before
    settled = stage.post_finalize(0, stage.signed_finalize(included=unconvicted))

    assert (settled.status_code, settled.json().get("status")) == (200, "SETTLED"), settled.text
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in unconvicted})
    assert dict(stage.rows("SELECT hotkey, issued FROM escrow_origins WHERE reward_round=0")) == {
        hotkey: units // 2 for hotkey in unconvicted
    }
    for coldkey in (owner, sibling_owner):
        assert escrow.balances(coldkey).reward_pending == 0
    assert escrow.balances().issued == issued + units
    assert escrow.balances().conserved()
    assert admission_of(stage, banned)["record"]["state"] == "BANNED"
    assert admission_of(stage, sibling)["record"]["state"] == "ACTIVE"


# Closed by audit-integrity-2 (6d935814, D59, D64-1): a Finalize that lists nobody settles.
def test_a_round_whose_inputs_are_all_convicted_settles_without_a_reward(stage: Stage) -> None:
    for key in stage.honest:
        sanction(stage, key, "TRAINING_FRAUD")
    X.control(
        {admission_of(stage, key)["record"]["state"] for key in stage.honest} == {"BANNED"},
        "every input of the applied tape is convicted",
    )
    escrow = stage.store._services(stage.run_id)[0]
    issued = escrow.balances().issued

    closed = stage.post_finalize(0, stage.signed_finalize(included=[]))

    assert (closed.status_code, closed.json().get("status")) == (200, "CLOSED_UNPAID"), closed.text
    assert stage.record("finality", "0")["disposition"] == "SETTLED"
    assert credits(stage) == Counter()
    assert stage.rows("SELECT COUNT(*) FROM escrow_finalized") == [(0,)]
    assert escrow.balances().issued == issued
    again = stage.post_finalize(0, stage.signed_finalize(included=[]))
    assert (again.status_code, again.json()) == (200, closed.json())


# -- FIN-04: finalize is one atomic unit ----------------------------------------------------------


def test_a_failure_inside_finalize_leaves_nothing_behind_and_the_retry_settles_once(
    stage: Stage,
) -> None:
    escrow = stage.store._services(stage.run_id)[0]
    real_owner = escrow.owner_of

    def failing(hotkey: str) -> str:
        raise EscrowError("INJECTED")

    # The failure comes from inside reward_finalize, after the SETTLED row, the finalize record
    # and the replayed-finality record were written by the same transaction.
    escrow.owner_of = failing
    before = stage.digest()
    assert refusal(stage.finalize) == (EscrowError, None, "INJECTED")
    assert stage.digest() == before
    nothing_was_finalized(stage)

    escrow.owner_of = real_owner
    stage.finalize()
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in honest_hotkeys(stage)})


def test_a_finalize_that_builds_its_services_inside_its_transaction_still_leaves_nothing(
    stage: Stage, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The condition of FIN-04 broken in a stand-in: the service cache is empty, as in a process
    # that has just restarted, and the ledger fails once it is reached. Today the first call
    # already fails while the ledger is built (OperationalError); a build that constructs its
    # services first reaches the ledger (EscrowError). Either way nothing may persist.
    def failing(self: EscrowV2, reward: Any, evidence: Any) -> EscrowReceipt:
        raise EscrowError("INJECTED")

    monkeypatch.setattr(EscrowV2, "reward_finalize", failing)
    stage.store._services_v2.clear()
    before = stage.digest()
    error = refusal(stage.finalize)
    assert error[0] in (EscrowError, sqlite3.OperationalError)
    assert stage.digest() == before
    nothing_was_finalized(stage)


# Closed by aggregate-lock-release (6d623b0d): the replay job reads the escrow policy through
# _services before the transaction opens, so a restarted store builds its services outside it.
def test_finalize_settles_as_the_first_request_after_a_restart(stage: Stage) -> None:
    assert stage.finalize()["status"] == "SETTLED"  # control: services already built
    stage.reset()
    stage.store._services_v2.clear()
    assert stage.finalize()["status"] == "SETTLED"


# -- FIN-06 / FIN-07: one credit per miner and round, rounds minted in order ----------------------


def test_a_round_is_credited_once_however_often_its_finalize_is_sent(stage: Stage) -> None:
    first = stage.finalize()
    effects, ledger = finalize_effects(stage), stage.digest()
    assert stage.finalize() == first
    assert finalize_effects(stage) == effects
    assert stage.digest() == ledger
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in honest_hotkeys(stage)})
    assert stage.rows("SELECT w FROM escrow_finalized") == [(0,)]

    other = {"entitlements_root": ANOTHER}
    assert refusal(lambda: stage.finalize(**other)) == (
        ChallengeError,
        409,
        "conflicting signed operation nonce",
    )
    # Second wall: with the single-use reservation gone, the stored finalize still decides.
    stage.forget_nonce("Finalize")
    assert refusal(lambda: stage.finalize(**other)) == (
        ChallengeError,
        409,
        "conflicting finalized origin authority",
    )
    assert stage.finalize() == first
    assert finalize_effects(stage) == effects
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in honest_hotkeys(stage)})


def test_rounds_are_minted_in_order_even_when_both_are_ready(stage: Stage) -> None:
    before = stage.digest()
    assert refusal(lambda: stage.finalize(1)) == (EscrowError, None, "FINALIZE_ORDER")
    assert stage.digest() == before
    assert stage.record("finality", "1")["disposition"] == "PENDING"

    stage.finalize(0)
    stage.finalize(1)
    assert stage.rows("SELECT w FROM escrow_finalized ORDER BY w") == [(0,), (1,)]
    assert credits(stage) == Counter(
        {(w, hotkey): 1 for w in (0, 1) for hotkey in honest_hotkeys(stage)}
    )


# -- FIN-10: a round's state only moves forward ---------------------------------------------------


def test_a_round_moves_forward_through_open_aggregate_and_finalize(stage: Stage) -> None:
    opened, aggregated = stage.after_open, stage.after_aggregate
    assert (opened["applied"], opened["disposition"], opened["unresolved_audits"]) == (
        False,
        "PENDING",
        5,
    )
    assert (aggregated["applied"], aggregated["disposition"], aggregated["unresolved_audits"]) == (
        True,
        "PENDING",
        0,
    )
    assert aggregated["state_hash"] == stage.aggregated[0]["theta_hash"] != opened["state_hash"]
    assert stage.record("finality", "0") == aggregated

    kept = stage.records("applied", "finality", "tape-inputs", "round", "round-meta", "start")
    replay = stage.store.aggregate_v2(stage.run_id, 0)
    assert replay["theta_hash"] == aggregated["state_hash"]
    assert (
        stage.records("applied", "finality", "tape-inputs", "round", "round-meta", "start") == kept
    )

    stage.finalize()
    settled = stage.record("finality", "0")
    assert settled == aggregated | {
        "disposition": "SETTLED",
        "resolution_hash": body_digest(stage.finalize_body(0)),
    }
    after = stage.records("applied", "finality", "tape-inputs", "finalize", "round", "start")
    assert stage.store.aggregate_v2(stage.run_id, 0)["theta_hash"] == aggregated["state_hash"]
    stage.finalize()
    assert (
        stage.records("applied", "finality", "tape-inputs", "finalize", "round", "start") == after
    )


# -- FIN-12 / FIN-13: the speculative window and the opening ---------------------------------------


def test_a_third_round_opens_only_after_the_first_is_settled(stage: Stage) -> None:
    before = stage.digest()
    assert refusal(lambda: stage.store.open_round_v2(stage.run_id, stage.signed_opening(2))) == (
        FinalityError,
        None,
        "w+2 blocked until settlement or exclusion plus rollback",
    )
    assert stage.digest() == before

    stage.finalize(0)
    opened = stage.rollback.execute_round(stage.network, 2, open_only=True)
    assert opened.status_code == 200, opened.text
    third = stage.record("finality", "2")
    assert (third["applied"], third["disposition"], third["unresolved_audits"]) == (
        False,
        "PENDING",
        4,
    )


def test_a_round_opening_is_immutable_idempotent_and_sequential(stage: Stage) -> None:
    run = stage.run_id
    stored = stage.record("round", "0")
    kept = stage.records("round", "round-meta", "start", "finality", "anchor")
    assert stage.store.open_round_v2(run, canonicalize(stored)) == stored
    assert stage.records("round", "round-meta", "start", "finality", "anchor") == kept

    changed = stage.signed_opening(0)
    before = stage.digest()
    assert refusal(lambda: stage.store.open_round_v2(run, changed)) == (
        ChallengeError,
        409,
        "conflicting signed operation nonce",
    )
    assert stage.digest() == before
    # Second wall: with the reservation gone, the stored opening still decides.
    stage.forget_nonce("RoundOpenV2")
    assert refusal(lambda: stage.store.open_round_v2(run, changed)) == (
        ChallengeError,
        409,
        "conflicting round opening",
    )
    assert stage.record("round", "0") == stored
    assert stage.records("round", "round-meta", "start", "finality", "anchor") == kept

    for w in (3, 5):
        assert refusal(lambda w=w: stage.store.open_round_v2(run, stage.signed_opening(w))) == (
            FinalityError,
            None,
            "opening requires complete sequential history",
        )
    # Past the window gate, an opening must start from the authenticated predecessor output.
    stage.finalize(0)
    before = stage.digest()
    assert refusal(lambda: stage.store.open_round_v2(run, stage.signed_opening(2))) == (
        ChallengeError,
        422,
        "public theta does not match authenticated predecessor",
    )
    assert stage.digest() == before


# -- FIN-14: a rollback needs the coordinator and an accepted referee cause ------------------------


def with_finality(w: int, **change: Any) -> Callable[[Stage], bytes]:
    def build(s: Stage) -> bytes:
        s.put("finality", str(w), s.record("finality", str(w)) | change)
        return s.signed_rollback()

    return build


def with_later_round(s: Stage) -> bytes:
    s.put("round", "2", s.record("round", "1"))
    return s.signed_rollback()


NOT_AUTHORIZED = (UnauthorizedSigner, None, "signer is not authorized for message role")
NOT_CLOSED = (ChallengeError, 409, "rollback requires applied closed audit/dispute work")
ROLLBACK_REFUSED = [
    pytest.param(
        lambda s: s.signed_rollback(signer=s.honest[0]), NOT_AUTHORIZED, id="a miner signs"
    ),
    pytest.param(
        lambda s: s.signed_rollback(signer=s.fixtures.AUDITORS[0]),
        NOT_AUTHORIZED,
        id="an auditor signs",
    ),
    pytest.param(with_finality(0, unresolved_disputes=1), NOT_CLOSED, id="a dispute is open"),
    pytest.param(with_finality(1, unresolved_audits=1), NOT_CLOSED, id="an audit is open"),
    pytest.param(with_finality(1, applied=False), NOT_CLOSED, id="the second round has no tape"),
    pytest.param(
        lambda s: s.signed_rollback(recomputed=["agg_w", "step_w"]),
        (ChallengeError, 422, "rollback requires complete inner and outer work"),
        id="a partial recomputation",
    ),
    pytest.param(
        lambda s: s.signed_rollback(old_theta_hash_w2=ANOTHER),
        (ChallengeError, 409, "rollback original theta subject differs"),
        id="another original theta",
    ),
    pytest.param(
        with_later_round,
        (ChallengeError, 409, "rollback cannot rewrite beyond speculative two-round window"),
        id="a later round exists",
    ),
    pytest.param(
        lambda s: s.signed_rollback(),
        (ChallengeError, 409, "rollback exclusion lacks exact accepted referee cause"),
        id="no referee resolution names the miner",
    ),
]


@pytest.mark.parametrize(("build", "expected"), ROLLBACK_REFUSED)
def test_a_rollback_without_the_coordinator_or_an_accepted_cause_changes_nothing(
    stage: Stage,
    build: Callable[[Stage], bytes],
    expected: tuple[type[BaseException], int | None, str],
) -> None:
    raw = build(stage)
    before = stage.digest()
    applied = stage.records("applied", "finality", "excluded", "repair-reservation", "rollback")
    assert refusal(lambda: stage.store.rollback_v2(stage.run_id, raw)) == expected
    assert stage.digest() == before
    assert stage.records("applied", "finality", "excluded", "repair-reservation", "rollback") == (
        applied
    )


def test_a_resolution_signed_by_the_referee_with_registered_evidence_is_an_accepted_cause(
    stage: Stage,
) -> None:
    cause = stage.seed_cause()
    accepted = stage.store._rollback_exclusions_v2(
        stage.run_id, stage.rollback_body(cause_hashes=[cause])
    )
    assert accepted == [{"hotkey": stage.forger.ss58, "reason": "FRAUD", "evidence_hash": cause}]


NOT_THE_LOSER = (ChallengeError, 409, "rollback cause does not exclude this miner")
NOT_THE_REFEREE = (ChallengeError, 409, "rollback cause lacks original referee signature")
CAUSE_REFUSED = [
    pytest.param({"loser": STRANGER}, NOT_THE_LOSER, id="the resolution names another loser"),
    pytest.param({"reason": "INFRASTRUCTURE"}, NOT_THE_LOSER, id="the resolution is no fault"),
    pytest.param({"forge_signature": True}, NOT_THE_REFEREE, id="the signature does not verify"),
    pytest.param(
        {"signer": Keypair(bytes([0x66]) * 32)}, NOT_THE_REFEREE, id="the referee did not sign"
    ),
    pytest.param(
        {"evidence": False},
        (ChallengeError, 409, "rollback cause lacks independent referee work"),
        id="no referee evidence is registered",
    ),
]


@pytest.mark.parametrize(("seed", "expected"), CAUSE_REFUSED)
def test_a_rollback_cause_the_referee_did_not_sign_with_evidence_is_refused(
    stage: Stage, seed: dict[str, Any], expected: tuple[type[BaseException], int | None, str]
) -> None:
    cause = stage.seed_cause(**seed)
    raw = stage.signed_rollback(cause_hashes=[cause])
    before = stage.digest()
    assert refusal(lambda: stage.store.rollback_v2(stage.run_id, raw)) == expected
    assert stage.digest() == before


def test_a_rollback_that_cites_a_cause_it_does_not_rest_on_is_refused(stage: Stage) -> None:
    # FIN-14: every cause a Rollback signs must be the accepted cause of an excluded miner.
    cause = stage.seed_cause()
    raw = stage.signed_rollback(cause_hashes=[cause, ANOTHER])
    before = stage.digest()
    assert refusal(lambda: stage.store.rollback_v2(stage.run_id, raw)) == (
        ChallengeError,
        409,
        "rollback cause set differs",
    )
    assert stage.digest() == before


# -- FIN-15: a settled round is never rewritten and no credit moves -------------------------------

SETTLE_FIRST = [
    pytest.param(lambda s: s.finalize(0), id="the first round is settled"),
    pytest.param(
        lambda s: s.put(
            "finality",
            "1",
            s.record("finality", "1") | {"disposition": "SETTLED", "resolution_hash": FINAL},
        ),
        id="the second round is settled",
    ),
]


@pytest.mark.parametrize("settle_one", SETTLE_FIRST)
def test_a_settled_round_is_never_rolled_back(
    stage: Stage, settle_one: Callable[[Stage], object]
) -> None:
    settle_one(stage)
    before = stage.digest()
    assert refusal(lambda: stage.store.rollback_v2(stage.run_id, stage.signed_rollback())) == (
        ChallengeError,
        409,
        "finalized late fraud is money-only",
    )
    assert stage.digest() == before


# -- FIN-16: a rollback is replay-safe ------------------------------------------------------------


def test_a_stored_rollback_is_replayed_and_a_different_body_for_the_round_is_refused(
    stage: Stage,
) -> None:
    # A stand-in for a completed repair: the stored request and result exist, no work is done.
    raw = stage.signed_rollback()
    result = {
        "status": "REPAIRED",
        "tape_hashes": [ANOTHER, FINAL],
        "theta_hash": "cd" * 32,
        "outer_state_hash": "ef" * 32,
        "reward_units": 0,
    }
    stage.put("rollback", "0", {"request": json.loads(raw), "result": result})
    other = stage.signed_rollback(new_theta_hash_w2=ANOTHER)
    before = stage.digest()
    assert refusal(lambda: stage.store.rollback_v2(stage.run_id, other)) == (
        ChallengeError,
        409,
        "conflicting rollback",
    )
    assert stage.digest() == before

    records = stage.rows("SELECT kind, id, data FROM records_v2 ORDER BY kind, id")
    assert stage.store.rollback_v2(stage.run_id, raw) == result
    assert stage.rows("SELECT kind, id, data FROM records_v2 ORDER BY kind, id") == records
    assert refusal(lambda: stage.store.rollback_v2(stage.run_id, other)) == (
        ChallengeError,
        409,
        "conflicting signed operation nonce",
    )
