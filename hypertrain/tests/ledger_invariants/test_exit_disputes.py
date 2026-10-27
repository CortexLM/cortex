"""A contest must always have a bounded exit that needs nobody's cooperation (D39, exit_disputes).

Every test states a defence of the dispute path: an open contest is closed by the beacon without
the late party, the finality hold it raises always equals the number of open disputes of its round,
its bond comes back, and a contest never wedges the round of the miner who opened it. The world is
the v2 service of `support.service_world`; the audit's records are seeded the way
`complete_audit_v2` writes them, everything after them goes through the routes.

Holes are strict xfails that assert the desired defence after a control that reaches the point
under test. Lens ids are candidates until a test below says otherwise; a hole that rests on a
stand-in for the service, or on a policy the maintainers have not decided, says [candidate].
"""

from __future__ import annotations

import asyncio
import threading
from collections.abc import Iterator
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from dataclasses import replace
from pathlib import Path
from typing import Any

import anyio
import pytest
from fastapi.testclient import TestClient

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.disputes_v2 import (
    EXCLUSION_ABANDONED,
    EXCLUSION_FROZEN,
    EXCLUSION_HORIZON,
)
from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import DisputeV2

from . import support_exit_disputes as X
from .support import World, service_world

# How the server closes a contest nobody can decide: an infrastructure exclusion, no loser.
CLOSED = ("INFRASTRUCTURE", None)
FINAL_OP = {"paused": True, "level": "op", "ctx": [1, 0], "interval": (1, 2)}


@pytest.fixture
def world(tmp_path: Path) -> Iterator[World]:
    with service_world(tmp_path) as built:
        yield built


def tag_of(world: World, dispute_id: str) -> str:
    """The evidence tag of the server's own closure of `dispute_id`."""
    resolution = X.disputes(world).get(dispute_id).resolution
    assert resolution is not None
    body = world.store._db.execute(
        "SELECT body FROM dispute_evidence_v2 WHERE hash=?", (resolution.evidence_hash,)
    ).fetchone()[0]
    return RefereeEvidence.model_validate_json(body).op_spec


def horizon_of(world: World, dispute_id: str) -> tuple[int, int, int]:
    """(turn_deadline, grace end, absolute_deadline) of an open dispute, in beacon rounds."""
    turn = X.disputes(world).get(dispute_id)
    grace = world.network.manifest.training.verify.T.dispute_per_level
    return turn.turn_deadline, turn.turn_deadline + grace, turn.absolute_deadline


def freeze(world: World, dispute_id: str, **update: object) -> None:
    """Put an open dispute into a turn shape no route builds cheaply (a paused final op)."""
    d = X.disputes(world)
    with world.store._tx():
        d._save(d.get(dispute_id).model_copy(update=update))


# --- the bounded exit (DIA-14, DIB-03, FIN-20, DS-03; DIA-15) ---------------------------------


@pytest.mark.parametrize("settled", [False, True], ids=["round-pending", "round-settled"])
@pytest.mark.parametrize(
    "shape",
    ["never-answered", "acked-then-silent", "paused-step", "paused-final-op"],
)
def test_a_contest_nobody_can_decide_is_closed_by_the_beacon_and_frees_every_hold(
    world: World, shape: str, settled: bool
) -> None:
    """DIA-14 / DIB-03 / FIN-20 / DS-03: whatever the parties do or do not do, the open contest
    ends at a bounded beacon round, the bond is back in full, the hold on its round is gone and
    the round settles (pending round) or round 2 opens (settled round). Nobody is asked."""
    # Given: a contest of a round that is pending (applied, unsettled) or already settled.
    X.seed_round(
        world,
        0,
        applied=True,
        state_hash="aa" * 32,
        disposition="SETTLED" if settled else "PENDING",
    )
    X.seed_round(world, 1)
    escrow = world.store._services(world.run_id)[0]
    dispute_id = X.open_contest(world, 0)
    opened = escrow.balances()
    _, grace_end, horizon = horizon_of(world, dispute_id)
    excluded = world.store._record_v2(world.run_id, "excluded", f"0:{X.service().HOT[0].ss58}")
    close_at, cause = grace_end + 1, EXCLUSION_ABANDONED
    match shape:
        case "never-answered":
            pass
        case "acked-then-silent":
            X.acknowledge_turn(world, dispute_id)
        case "paused-step":
            X.pause_at_step(world, dispute_id)
            close_at, cause = world.network.now + 1, EXCLUSION_FROZEN
        case "paused-final-op":
            freeze(world, dispute_id, **FINAL_OP)
            close_at, cause = horizon + 1, EXCLUSION_HORIZON
    # Only a paused final op, which the referee can still score, waits for the horizon.
    assert (close_at < horizon) == (shape != "paused-final-op")
    # A pending round cannot settle while its contest is open. Whether a late contest may hold
    # the opening of round 2 of a settled round is FIN-11 below, so it is not asserted here.
    assert opened.dispute_locked == X.BOND
    assert settled or (X.unresolved_of(world) == 1 and X.hold_blocks(world, settled=False))
    # When / Then: one round before its closing round the contest is still open.
    if close_at - 1 > world.network.now:
        world.network.push(close_at - 1)
    assert X.disputes(world).get(dispute_id).resolution is None
    assert settled or X.hold_blocks(world, settled=False)
    world.network.push(close_at)
    # Then: closed with no loser, for the cause of its shape, nothing burned, hold released.
    closed = X.disputes(world).get(dispute_id)
    assert closed.resolution is not None
    assert (closed.resolution.reason, closed.resolution.loser) == CLOSED
    assert closed.resolved_beacon == close_at and tag_of(world, dispute_id) == cause
    assert escrow.balances() == replace(
        opened, available=opened.available + X.BOND, dispute_locked=0
    )
    assert escrow.balances().burned == 0 and X.miner_state(world, 0)[:2] == ("PROBATION", 0)
    assert X.unresolved_of(world) == 0 and not X.hold_blocks(world, settled=settled)
    assert world.store._record_v2(world.run_id, "excluded", f"0:{X.service().HOT[0].ss58}") == (
        excluded
    )
    assert world.count("dispute_acks_v2") == (1 if shape == "acked-then-silent" else 0)


def test_the_timeout_route_needs_the_contesters_receipt_but_the_beacon_exit_does_not(
    world: World,
) -> None:
    """DIA-15: PARTY_TIMEOUT is a conviction and keeps requiring the party's own signed receipt
    (NO_AUTHENTICATED_TURN_DELIVERY), but the exit of a contest whose party never acked does not:
    the operator's window ends at turn_deadline + T and the beacon closes the contest after it."""
    # Given: a funded contest whose contester never answers and never acknowledges.
    dispute_id = X.open_contest(world, 0)
    first, grace_end, _ = horizon_of(world, dispute_id)
    world.network.push(first + 1)
    before = world.db_digest()
    # When: the operator tries the witnessed timeout inside its window.
    refused = world.client.post(
        world.network.url + f"/admin/disputes/{dispute_id}/timeout",
        json=X.timeout_request(world, dispute_id),
        headers=world.admin,
    )
    # Then: refused for the missing receipt, state unchanged, no loser named.
    assert refused.status_code == 409 and "NO_AUTHENTICATED_TURN_DELIVERY" in refused.text
    assert world.db_digest() == before
    assert X.disputes(world).get(dispute_id).resolution is None
    # When: the last round of the operator's window passes, then the first one after it.
    world.network.push(grace_end)
    assert X.disputes(world).get(dispute_id).resolution is None
    world.network.push(grace_end + 1)
    # Then: the exit happened without a receipt, the contester is not convicted.
    closed = X.disputes(world).get(dispute_id)
    assert closed.resolution is not None and closed.resolution.loser is None
    assert world.count("dispute_acks_v2") == 0
    assert X.unresolved_of(world) == 0


def test_the_hold_of_a_round_is_the_count_of_its_open_disputes_after_every_closing_route(
    world: World,
) -> None:
    """DIA-16 / DIB-18 (and the bond half of DIA-14): after every route that opens or closes a
    dispute, finality[w] holds the number of open disputes of round w, recounted: two rounds never
    mix, a replayed resolution never subtracts twice, and the referee's route, the operator's
    timeout and the beacon exit each recount and take the contest bond out of dispute_locked.
    What becomes of the bond (returned or burned) is DIB-05's question, not asserted here."""
    # Given: rounds 0 and 1 and three miners with a funded contest each (two on round 0).
    fixture = X.service()
    escrow = world.store._services(world.run_id)[0]
    X.seed_round(world, 1)
    first = X.open_contest(world, 0)
    second = X.open_contest(world, 1)
    third = X.open_contest(world, 2, w=1)

    def state() -> tuple[int, int, int]:
        """(hold of round 0, hold of round 1, bonds still locked), the holds checked."""
        holds = (X.unresolved_of(world, 0), X.unresolved_of(world, 1))
        assert holds == (X.open_disputes_of(world, 0), X.open_disputes_of(world, 1))
        assert escrow.balances().conserved()
        return (*holds, escrow.balances().dispute_locked // X.BOND)

    assert state() == (2, 1, 3)
    # When: the referee decides the first one, and signs the same answer again.
    resolution = X.resolve(world, first, reason="MATCH", loser=None)
    assert state() == (1, 1, 2)
    assert X.post_resolution(world, resolution).status_code == 200
    assert state() == (1, 1, 2)
    # When: the operator times out the third one after its contester acknowledged and went silent.
    X.acknowledge_turn(world, third)
    world.network.push(X.disputes(world).get(third).turn_deadline + 1)
    response = world.client.post(
        world.network.url + f"/admin/disputes/{third}/timeout",
        json=X.timeout_request(world, third),
        headers=world.admin,
    )
    assert response.status_code == 200, response.text
    assert X.disputes(world).get(third).resolution.loser == fixture.HOT[2].ss58
    assert state() == (1, 0, 1)
    # When: the beacon closes the last one.
    world.network.push(horizon_of(world, second)[1] + 1)
    assert state() == (0, 0, 0)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-11: _recount_disputes_v2 rewrites finality[w] of a SETTLED round on a late "
    "contest (unresolved_disputes 0 -> 1), which blocks require_open(w+2) until it closes; "
    "ADR 0011 accepts this residual until contests are priced [candidate]",
)
def test_a_late_contest_never_reopens_a_settled_round(world: World) -> None:
    """FIN-11: a round that finality settled keeps its row; a contest filed on it afterwards is
    tracked apart and does not decide whether round 2 may open."""
    # Given: rounds 0 (settled) and 1 are in the history and round 2 may open.
    X.seed_round(world, 0, applied=True, disposition="SETTLED", resolution_hash="ab" * 32)
    X.seed_round(world, 1)
    settled_row = world.store._record_v2(world.run_id, "finality", "0")
    assert not X.hold_blocks(world, settled=True)
    # When: the miner contests its round-0 verdict (the e2e watch flow files exactly this).
    X.open_contest(world, 0)
    # Then
    assert world.store._record_v2(world.run_id, "finality", "0") == settled_row
    assert not X.hold_blocks(world, settled=True)


# --- a contest never wedges the round of its opener (DIB-04, DS-02) ---------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DIB-04+DS-02: a contest on a verdict that matched suspends its opener, and "
    "verified_inputs_v2 then raises 403 for the whole round",
)
def test_a_contest_on_a_matching_verdict_never_wedges_the_aggregation_of_its_round(
    world: World,
) -> None:
    """DIB-04 / DS-02: the opener of a contest is a roster member whose audit matched. Either the
    contest is refused, or the round can still be aggregated: verified_inputs_v2 skips or excludes
    it instead of raising for everybody. (That a closed contest returns the miner to a fresh
    probation, not to ACTIVE, is decided by ADR 0011 and pinned by tests/challenge/
    test_dispute_exit_v2.py::test_closed_contest_lifts_the_suspension.)"""
    # Given: an ACTIVE funded miner on the roster of round 0, eligible, nothing contested yet.
    X.activate(world, 0)
    X.seed_open_round(world, 0, [0])
    hot = X.service().HOT[0].ss58
    admission = world.store._services(world.run_id)[1]
    assert admission.status(hot, now=world.network.now).eligible
    # Control: the eligibility check passes, the call goes on to read the miner's missing commit.
    with pytest.raises(ChallengeError) as reached:
        world.store.verified_inputs_v2(world.run_id, 0)
    assert (reached.value.status, "missing accepted commit" in reached.value.detail) == (409, True)
    # When: the miner contests its MATCH verdict.
    X.seed_contest(world, 0, mismatch=False)
    X.fund_contest(world, 0)
    response = X.post_dispute(world, 0)
    # Then: refused with the miner untouched, or admitted with the round still aggregatable.
    if response.status_code != 200:
        assert X.miner_state(world, 0) == ("ACTIVE", 0, 12)
        return
    try:
        world.store.verified_inputs_v2(world.run_id, 0)
    except ChallengeError as error:
        assert error.status != 403, error.detail


# --- the event long-poll and the dispute writes never wait for each other (DIA-22, DS-04) -----

PATIENCE = 4.0  # seconds a thread may wait for a lock before the test declares it stuck
START = 30.0  # seconds a started thread may take to reach the service on a loaded host


class PollerGate:
    """Test-owned stand-in for `EscrowV2.mutex`: the store's own lock, plus a signal when the
    poller asks for it and a bounded wait for the poller only, so a stuck pair can be released."""

    def __init__(self, inner: Any, attempted: threading.Event) -> None:
        self.inner, self.attempted = inner, attempted

    def __enter__(self) -> PollerGate:
        if threading.current_thread().name != "poller":
            self.inner.acquire()
            return self
        self.attempted.set()
        if not self.inner.acquire(timeout=PATIENCE):
            raise TimeoutError("the poller waited for the store lock while holding the condition")
        return self

    def __exit__(self, *exc: object) -> None:
        self.inner.release()


def in_thread(name: str, work) -> tuple[threading.Thread, dict[str, object]]:
    box: dict[str, object] = {}

    def run() -> None:
        try:
            box["result"] = work()
        except BaseException as error:  # reported to the test, never swallowed
            box["error"] = error

    thread = threading.Thread(target=run, name=name, daemon=True)
    thread.start()
    return thread, box


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DIA-22+DS-04: events() holds the dispute condition and asks for the store lock while "
    "a writer holds the store lock and notify()s: the pair waits for each other",
)
@pytest.mark.parametrize("write", ["open", "beacon-exit"])
def test_a_dispute_poll_and_a_dispute_write_never_wait_for_each_other(
    world: World, write: str
) -> None:
    """DIA-22 / DS-04: a long-poll that starts while a dispute write holds the store lock, and a
    write that notifies the pollers before it commits, both finish. The writes are the miner's
    contest and the beacon's own closure of a stalled one."""
    # Given: the write is ready (a funded contest, or a stalled one one round from its closure).
    store, hot = world.store, X.service().HOT[0]
    if write == "open":
        X.join(world, 0)
        X.seed_contest(world, 0)
        X.fund_contest(world, 0)
        dispute_id, method = X.dispute_id_of(world, 0), "open"
        request = canonicalize(
            world.network.signed(
                hot,
                "DisputeV2",
                DisputeV2(
                    hotkey=hot.ss58,
                    verdict_hash=X.verdict_of(0),
                    action="contest",
                    bond_lock=X.lock_id_of(0),
                ),
            )
        )
        act = lambda: store.admission_v2(world.run_id, "dispute", request)  # noqa: E731
    else:
        dispute_id, method = X.open_contest(world, 0), "exclude"
        grace_end = horizon_of(world, dispute_id)[1]
        world.network.push(grace_end)
        act = lambda: world.network.push(grace_end + 1)  # noqa: E731
    d = X.disputes(world)
    attempted, inside = threading.Event(), threading.Event()
    d.escrow.mutex = PollerGate(store._lock, attempted)
    real = getattr(d, method)

    def once_the_poll_is_waiting(*args: Any, **kwargs: Any) -> Any:
        inside.set()  # the store lock is held by the writer from here to its commit
        assert attempted.wait(START)  # the poller holds the condition and asks for the lock
        return real(*args, **kwargs)

    setattr(d, method, once_the_poll_is_waiting)
    # When: the write starts, and the poll starts inside its transaction.
    writer, written = in_thread("writer", act)
    assert inside.wait(START)
    poller, polled = in_thread("poller", lambda: d.events(hot.ss58, 0, timeout=5))
    writer.join(3 * PATIENCE)
    poller.join(3 * PATIENCE)
    # Then: neither is stuck, the write took effect and the poll saw the contest's event.
    assert not writer.is_alive() and not poller.is_alive()
    assert "error" not in polled, polled.get("error")
    assert "error" not in written, written.get("error")
    assert [event.turn.dispute_id for event in polled["result"]] == [dispute_id]
    assert (d.get(dispute_id).resolution is not None) == (write == "beacon-exit")


# --- what a contest settles for the contester (DIA-24, DIB-11, DIA-25, DIA-06, DIB-05) --------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DIA-24+DIB-11: a contest closed with the auditor as loser refunds the bond but never "
    "lifts the excluded/UNVERIFIED record the wrong verdict wrote (no deletion exists); the Lens "
    "calls the remedy a product decision [candidate]",
)
def test_a_contest_won_against_the_auditor_lifts_the_exclusion_it_proved_wrong(
    world: World,
) -> None:
    """DIA-24 / DIB-11: the referee finds the auditor wrong; the miner is made whole, not only
    its bond. The exclusion of (round, miner) that the verdict wrote is lifted."""
    # Given: a miner excluded by a non-MATCH verdict, contesting it with a funded bond.
    escrow = world.store._services(world.run_id)[0]
    hot = X.service().HOT[0].ss58
    dispute_id = X.open_contest(world, 0)
    assert world.count("records_v2", "kind='excluded' AND id=?", (f"0:{hot}",)) == 1
    # When: the referee convicts the auditor.
    X.resolve(world, dispute_id, reason="FRAUD", loser=X.service().AUDITORS[0].ss58)
    # Control: the bond is back in full and the hold is gone, so the resolution did run.
    assert escrow.balances().dispute_locked == 0 and escrow.balances().burned == 0
    assert X.unresolved_of(world) == 0
    # Then: the wrong verdict no longer excludes the miner from its round.
    assert world.count("records_v2", "kind='excluded' AND id=?", (f"0:{hot}",)) == 0


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DIA-25: a LOCK_CONTEST whose dispute never opened is not releasable: the settlement "
    "lookup finds neither a dispute nor a record (UNACCEPTED_SETTLEMENT), units stay locked",
)
def test_a_contest_bond_whose_dispute_never_opened_can_be_taken_back(world: World) -> None:
    """DIA-25: an owner who locked a bond for a contest that was refused (here: the verdict is
    unknown to the service) can get all its units back: the owner asks through the escrow release
    route, and whichever way the service returns them they end up available, none burned."""
    # Given: a funded contest lock, and an open that the service refuses.
    escrow = world.store._services(world.run_id)[0]
    X.join(world, 0)
    before = escrow.balances()
    lock = X.fund_contest(world, 0)
    assert escrow.balances().dispute_locked == X.BOND
    refused = X.post_dispute(world, 0)
    assert refused.status_code == 409 and world.count("disputes_v2") == 0
    # When: the owner releases the lock it signed.
    released = X.post_release(world, 0, lock)
    # Then: the owner is made whole.
    assert escrow.balances() == before, released.text


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DIA-06+DIB-05: open() takes a contest on a MATCH verdict (the contest record carries "
    "no verdict result) and suspends the miner; the e2e watch flow pins the permissive rule "
    "[candidate]",
)
def test_a_contest_on_a_matching_verdict_is_refused_before_it_suspends_anyone(
    world: World,
) -> None:
    """DIA-06 / DIB-05: a contest is admissible where it can be adverse to the audit (the verdict
    is not MATCH). A MATCH verdict's contest is refused and leaves the state as it was."""
    # Control: the same route admits the contest of a verdict that is not MATCH.
    X.open_contest(world, 1, mismatch=True)
    assert X.miner_state(world, 1)[:2] == ("SUSPENDED", 1)
    # Given: a funded contest of a MATCH verdict.
    X.join(world, 0)
    X.seed_contest(world, 0, mismatch=False)
    X.fund_contest(world, 0)
    before = world.db_digest()
    # When / Then
    response = X.post_dispute(world, 0)
    assert 400 <= response.status_code < 500, response.text
    assert world.db_digest() == before
    assert X.miner_state(world, 0)[:2] == ("APPLIED", 0)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DIB-05+DIA-06: a contest the referee closes MATCH (audit confirmed) returns the whole "
    "bond (settle_lock slashes only FRAUD against the miner), so a baseless contest is free; "
    "ADR 0011 leaves the pricing to a later decision [candidate]",
)
def test_a_baseless_contest_costs_the_contester_part_of_its_bond(world: World) -> None:
    """DIB-05 / DIA-06: the referee's replay confirms the audit; the contester ends with fewer
    units than it began with, whether the forfeited part is burned or paid to someone else.
    A service that refuses the contest of a MATCH verdict has no such contest to price, so the
    test ends there (DIA-06 above is then fixed and this marker has to go)."""
    # Given: a contest of a MATCH verdict, if the service takes it.
    escrow = world.store._services(world.run_id)[0]
    owner = X.service().COLD[0].ss58

    def owned() -> int:
        """Units of the contester that are available to it."""
        return int(
            world.store._db.execute(
                "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE owner=? "
                "AND bucket='available'",
                (owner,),
            ).fetchone()[0]
        )

    X.join(world, 0)
    before = owned()
    dispute_id = X.seed_contest(world, 0, mismatch=False)
    X.fund_contest(world, 0)
    opened = X.post_dispute(world, 0)
    if opened.status_code != 200:
        assert 400 <= opened.status_code < 500, opened.text
        return
    # When: the referee confirms the verdict.
    X.resolve(world, dispute_id, reason="MATCH", loser=None)
    # Control: the bond was settled (nothing is locked any more).
    assert escrow.balances().dispute_locked == 0 and X.unresolved_of(world) == 0
    # Then: the contester paid for it.
    assert owned() < before


# --- stuck contests cannot pin the queue (DS-06) ----------------------------------------------


def test_contests_nobody_answers_free_the_32_slot_queue_for_the_next_honest_contest(
    world: World,
) -> None:
    """DS-06: 32 funded contests that nobody answers fill the queue; the beacon closes them
    after their window and the 33rd contest is then accepted, so a stall cannot exclude others."""
    # Given: 32 open contests of distinct miners on round 0 and a 33rd one refused for room.
    X.seed_round(world, 0)
    d = X.disputes(world)
    table = X.synthetic_miners(world)
    ids = []
    for index in range(32):
        raw, dispute_id = X.queue_contest(world, table, index)
        assert d.open(raw, now=1).dispute_id == dispute_id
        ids.append(dispute_id)
    last, last_id = X.queue_contest(world, table, 32)
    assert X.error_code_of(lambda: d.open(last, now=1)) == "QUEUE_CAPACITY_EXCLUDE_WITHOUT_FRAUD"
    assert len(d.unresolved()) == 32
    # When: the window of the stuck contests is over.
    world.network.push(horizon_of(world, ids[0])[1] + 1)
    # Then: every one is closed without a loser, the hold is gone, the 33rd contest is taken.
    assert d.unresolved() == [] and X.unresolved_of(world) == 0
    assert all(d.get(i).resolution.loser is None for i in ids)
    escrow = world.store._services(world.run_id)[0]
    assert escrow.balances().burned == 0 and escrow.balances().dispute_locked == X.BOND
    assert d.open(last, now=world.network.now).dispute_id == last_id


# --- a failed rollback attempt never disables the next one (DIB-22, FIN-17, LV-04) ------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DIB-22+FIN-17+LV-04: a RUNNING repair reservation never leaves RUNNING (only "
    "COMPLETE writes it): once it is old, or the state moved, no rollback can start [candidate]",
)
@pytest.mark.parametrize("case", ["window-over", "state-moved"])
def test_a_failed_rollback_attempt_never_disables_the_next_one(world: World, case: str) -> None:
    """DIB-22 / FIN-17 / LV-04: the reservation of an attempt that never completed is released
    when its 200 beacon rounds are over or when the state moved (a retry starts over), so the
    rollback can still run. A different body for the same round cannot exist: its signed
    operation key is the round, so the retry is always the same request."""
    # Given: a crashed first attempt, and a gate that lets its own retry through (control).
    rig = X.rollback_rig(world)
    first = rig.request(1)
    rig.leave_running_reservation(first)
    assert rig.attempt(first) is None
    # When: its window is over, or an unrelated miner joins.
    if case == "state-moved":
        X.join(world, 1)
    else:
        world.network.push(world.network.now + 201)
    # Then: the gate does not refuse the retry.
    assert rig.attempt(first) is None


# --- a long-poll does not starve the service (DS-08) ------------------------------------------

POOL = 4  # worker threads of the scaled-down request pool (the service default is min(32, cpus+4))


@contextmanager
def shared_loop(world: World, workers: int) -> Iterator[TestClient]:
    """A client whose requests share one event loop whose thread pool has `workers` threads: the
    production pool scaled down, so a handful of long-polls exhausts it as 32 would the real one."""
    client = TestClient(world.app)
    with anyio.from_thread.start_blocking_portal(backend="asyncio") as portal:
        client.portal = portal

        async def shrink() -> None:
            asyncio.get_running_loop().set_default_executor(ThreadPoolExecutor(max_workers=workers))

        portal.call(shrink)
        yield client


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="DS-08: GET /disputes runs each long-poll (up to 30 s) on a thread of the shared "
    "request pool with no bound per party, so a few polls stall every other route [candidate]",
)
def test_waiting_dispute_polls_never_starve_the_other_routes(world: World) -> None:
    """DS-08: a watcher that polls for events does not keep the service from answering others.
    POOL polls of one party wait on an empty feed while a plain status request is made."""
    # Given: a registered miner whose feed is empty, and a pool that answers the status route.
    fixture, d = X.service(), X.disputes(world)
    X.join(world, 0)
    hot, url = fixture.HOT[0], world.network.url
    watch = {
        "x-dispute-signature": hot.sign(
            f"hypertrain/watch/2|{world.run_id}|{hot.ss58}".encode()
        ).hex()
    }
    waiting, answered = threading.Condition(), {}
    real_events = d.events

    def counted(*args: Any, **kwargs: Any) -> Any:
        with waiting:
            answered["entered"] = answered.get("entered", 0) + 1
            waiting.notify_all()
        return real_events(*args, **kwargs)

    d.events = counted  # type: ignore[method-assign]
    with shared_loop(world, POOL) as client:
        assert client.get(url).status_code == 200  # control: the route answers on an idle pool

        def poll(index: int) -> None:
            reply = client.get(
                url + "/disputes", params={"party": hot.ss58, "timeout": 25}, headers=watch
            )
            with waiting:
                answered[index] = reply.status_code
                waiting.notify_all()

        # When: POOL polls come in one after the other, each waits or is turned away.
        pollers = [threading.Thread(target=poll, args=(i,), daemon=True) for i in range(POOL)]
        for count, poller in enumerate(pollers, start=1):
            poller.start()
            with waiting:
                assert waiting.wait_for(
                    lambda count=count: (
                        answered.get("entered", 0) + sum(1 for k in answered if k != "entered")
                        >= count
                    ),
                    timeout=START,
                ), f"poll {count} never reached the service"
        probe_status: list[int] = []
        probe = threading.Thread(
            target=lambda: probe_status.append(client.get(url).status_code), daemon=True
        )
        probe.start()
        probe.join(PATIENCE)
        stuck = probe.is_alive()
        # The polls end when their party has an event: open its contest (a direct, unpooled call).
        X.seed_contest(world, 0)
        X.fund_contest(world, 0)
        assert X.post_dispute(world, 0).status_code == 200
        for thread in (*pollers, probe):
            thread.join(3 * PATIENCE)
        assert not any(t.is_alive() for t in (*pollers, probe))
    # Then: the status request was answered while the polls were still waiting.
    assert {answered[i] for i in range(POOL)} <= {200, 429}
    assert not stuck, "the status route waited for the polls"
