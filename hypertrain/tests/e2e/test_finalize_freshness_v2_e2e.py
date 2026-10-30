"""S21 on the real routes: an audited round's aggregate and finalize survive a short beacon pause.

Run 0ef600b4, rounds 0 and 1 (30/10, lot hotfix-finalize-freshness, D118): the 5 audits MATCH
and the aggregate stored, the finalize answered 503 "beacon infrastructure paused" after 2 to 5
minutes, its gate judging after the replay the lag that replay had made (8 to 11 rounds); a
retried aggregate answered 503 seven seconds in, on a flicker of 3 rounds at its entry. Both kinds
now tolerate up to RESULT_PAUSE_ROUNDS on an audited round whose tape is stored. The finalize's
gate still comes after its replay (finalize_v2 is part of the implementation hash): a replay that
keeps the pusher out longer than the cap still answers 503 (the xfail below). One real round of
five members; the wall clock is moved, the beacon is not pushed.
"""

from __future__ import annotations

import json

import pytest
from test_rollback_service_v2 import (  # noqa: F401
    active_network,
    execute_round,
    fixtures,
)

from hypertrain.challenge.exit_v2 import MAX_LAG_ROUNDS, RESULT_PAUSE_ROUNDS
from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol.messages import Finalize

CAP = MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS
FLICKER = MAX_LAG_ROUNDS + 1  # what the S21 base beacon showed several times a minute


def wall_at(network, number: int) -> None:
    """The master's wall clock at the emission of round `number`, nothing pushed."""
    genesis = network.manifest.training.beacon.genesis_time
    network.store.clock = lambda: genesis + (number - 1) * 3


def record(network, kind: str, identity: str = "0"):
    row = network.store._db.execute(
        "SELECT data FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
        (network.manifest.run_id(), kind, identity),
    ).fetchone()
    return None if row is None else row[0]


def aggregate(network):
    return network.client.post(network.url + "/admin/rounds/0/aggregate", headers=fixtures.admin())


def finalize_round(network, w: int, applied, expiry: int = 100000):
    final = Finalize(
        w=w,
        final_theta_hash_w1=applied["theta_hash"],
        included=sorted(k.ss58 for k in fixtures.HOT),
        entitlements_root="0" * 64,
    )
    return network.client.post(
        network.url + f"/admin/rounds/{w}/finalize",
        json=type(network).signed(network, fixtures.COORD, "Finalize", final, expiry),
        headers=fixtures.admin(),
    )


def finalize(network, applied, expiry: int = 100000):
    return finalize_round(network, 0, applied, expiry)


@pytest.fixture(scope="module")
def audited(active_network):  # noqa: F811
    """Round 0 executed, its five audits MATCH, not yet aggregated."""
    execute_round(active_network, 0, aggregate=False)
    return active_network


def test_s21_an_aggregate_retried_on_a_flicker_answers_its_stored_tape(audited, monkeypatch):
    network = audited
    view = network.now
    # A first aggregate judges its members at the gate's round, which must have been pushed: on
    # a flicker it is still refused, at entry, before it computes anything.
    computed = []
    real = ChallengeStore._aggregate_compute_v2

    def compute(store, plan, w):
        computed.append(w)
        return real(store, plan, w)

    monkeypatch.setattr(ChallengeStore, "_aggregate_compute_v2", compute)
    wall_at(network, view + FLICKER)
    first = aggregate(network)
    assert (first.status_code, first.json()["detail"]) == (503, "beacon infrastructure paused")
    assert computed == [] and record(network, "applied") is None
    # Fresh: the tape is computed and stored.
    wall_at(network, view)
    stored = aggregate(network)
    assert stored.status_code == 200, stored.text
    assert computed == [0]
    # The S21 retries (round 1, attempts 2 and 3): the same stored tape, on a flicker and at the
    # cap, nothing computed again; one round past the cap is a real outage and refused.
    for lag in (FLICKER, CAP):
        wall_at(network, view + lag)
        again = aggregate(network)
        assert (again.status_code, again.json()) == (200, stored.json()), again.text
    wall_at(network, view + CAP + 1)
    outage = aggregate(network)
    assert (outage.status_code, outage.json()["detail"]) == (503, "beacon infrastructure paused")
    assert computed == [0]


def slow_replays(network, monkeypatch, *, to: int) -> list[int]:
    """The S21 replay: while it runs no beacon lands and the wall clock reaches round `to`."""
    replays: list[int] = []
    real = ChallengeStore._run_finality_replay_v2

    def replay(hosted, job):
        replays.append(job.w)
        answer = real(hosted, job)
        wall_at(network, to)
        return answer

    monkeypatch.setattr(ChallengeStore, "_run_finality_replay_v2", replay)
    return replays


def aggregated(network, view: int):
    wall_at(network, view)
    stored = aggregate(network)  # computed here unless the aggregate test left it
    assert stored.status_code == 200, stored.text
    return stored.json()


def settled_at(network, w: int) -> tuple[int, set[int]]:
    """The beacon round w closed at and the release beacon of every origin it paid."""
    store, run = network.store, network.manifest.run_id()
    closure = store._record_v2(run, "replayed-finality", str(w))
    releases = {
        json.loads(data)["release_beacon"]
        for (data,) in store._db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='settlement'", (run,)
        )
    }
    return closure["finalized_beacon"], releases


def test_a_finalize_past_the_cap_or_expired_where_it_would_settle_is_refused(audited):
    network = audited
    view = network.now
    applied = aggregated(network, view)
    # A real outage: refused, nothing written.
    wall_at(network, view + CAP + 1)
    outage = finalize(network, applied)
    assert (outage.status_code, outage.json()["detail"]) == (503, "beacon infrastructure paused")
    # Expired at the round it would settle at, though not at the stale view: the ledger accepts
    # the Finalize again at that round (EscrowV2.reward_finalize) and refuses it.
    wall_at(network, view + 10)
    expired = finalize(network, applied, expiry=view + 4)
    assert (expired.status_code, expired.json()["detail"]) == (409, "envelope expired")
    assert record(network, "finalize") is None


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="remaining 503 of the hash-neutral lot: finalize_v2 judges its gate after its replay, "
    "so a replay that keeps the pusher out longer than RESULT_PAUSE_ROUNDS (150 s) is refused; "
    "the gate at entry moves the implementation hash (branch hotfix-finalize-entry-gate)",
)
def test_a_finalize_whose_own_replay_outlasts_the_cap_settles(audited, monkeypatch):
    network = audited
    view = network.now
    applied = aggregated(network, view)
    network.store._finality_verdicts_v2.clear()  # no verdict kept by an earlier test: it replays
    replays = slow_replays(network, monkeypatch, to=view + CAP + 1)
    settled = finalize(network, applied)
    assert replays == [0]
    assert settled.status_code == 200, settled.text  # today: 503 "beacon infrastructure paused"


def test_s21_a_finalize_whose_replay_ends_inside_the_cap_settles(audited, monkeypatch):
    # S21 round 0: fresh at entry, the replay kept the pusher out and the gate after it saw the
    # view 8 (attempt 2) or 11 (attempt 1) rounds behind.
    network = audited
    view = network.now
    applied = aggregated(network, view)
    network.store._finality_verdicts_v2.clear()  # no verdict kept by an earlier test: it replays
    replays = slow_replays(network, monkeypatch, to=view + 11)
    settled = finalize(network, applied)
    assert settled.status_code == 200, settled.text
    assert settled.json()["status"] == "SETTLED"
    assert replays == [0]
    # Settled at the round a healthy view held when the gate was judged, never at the stale one.
    judged = view + 11 - MAX_LAG_ROUNDS
    vesting = judged + network.manifest.training.verify.E_vest_rounds
    assert settled_at(network, 0) == (judged, {vesting})
    # Retried during a long outage: the stored answer, as before.
    wall_at(network, view + 10 * CAP)
    assert finalize(network, applied).json() == settled.json()
    assert replays == [0]


def test_a_finalize_on_a_flicker_settles_at_the_round_a_healthy_view_holds(audited):
    # Round 1, aggregated on a fresh view, finalized on a view one round past the bound.
    network = audited
    wall_at(network, network.now)
    applied = execute_round(network, 1)
    view = network.now
    wall_at(network, view + FLICKER)
    settled = finalize_round(network, 1, applied)
    assert settled.status_code == 200, settled.text
    assert settled.json()["status"] == "SETTLED"
    # Judged at emitted - MAX_LAG_ROUNDS, a round not pushed yet: never at the stale view.
    judged = view + FLICKER - MAX_LAG_ROUNDS
    assert network.store._now(network.store._db) == view < judged
    vesting = judged + network.manifest.training.verify.E_vest_rounds
    finalized, releases = settled_at(network, 1)
    assert finalized == judged and vesting in releases
