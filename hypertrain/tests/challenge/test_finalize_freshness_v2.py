"""The beacon gate of an audited round's aggregate and finalize tolerates a short pause (hotfix 3).

S21 (run 0ef600b4): round 0's finalize answered 503 "beacon infrastructure paused" twice, after
2 min 14 s and 5 min, and round 1's three times: once at the end of a 4 min 31 s finalize, twice
on an aggregate refused 7 s after entry. The view was 3 to 11 rounds behind the emitted round: the
call's own work kept the pusher out (finalize), or the base beacon flickered to 3 (aggregate), and
`_fresh_v2` held both kinds to MAX_LAG_ROUNDS = 2. An aggregate or finalize of a round whose
audits are all terminal (and, for finalize, whose aggregate is stored) is now judged like a lease
or an audit result: up to RESULT_PAUSE_ROUNDS further behind, at the round a healthy view holds.
Here the gate alone; tests/e2e/test_finalize_freshness_v2_e2e.py drives the routes.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import pytest

from hypertrain.challenge.exit_v2 import MAX_LAG_ROUNDS, RESULT_PAUSE_ROUNDS
from hypertrain.challenge.store import ChallengeError

from .audit_lease_fixture_v2 import network  # noqa: F401  (pytest fixture import)
from .test_audit_freshness_pause_v2 import (
    QUICKNET,
    production,  # noqa: F401  (pytest fixture import)
    production_pin,
    production_wall,
    wall_at,
)

CAP = MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS


def audited(store, run: str, *, w: int = 0, aggregated: bool = True) -> None:
    """Round w audited, every job terminal (a MATCH and an exhausted one), its tape stored."""
    for i, state in enumerate(("completed", "exhausted")):
        store._db.execute(
            "INSERT INTO audit_leases_v2(id,run_id,w,state) VALUES(?,?,?,?)",
            (f"job-{w}-{i}", run, w, state),
        )
    if aggregated:
        store._put_record_v2(run, "applied", str(w), {"theta_hash": "a" * 64})


@pytest.fixture
def lagging(network):
    """A running run, round 0 audited and aggregated, the view at `network.now`."""
    store, run = network.store, network.manifest.run_id()
    store._db.execute("UPDATE runs SET status='running'")
    audited(store, run)
    return network


def pin(store, operation):
    return tuple(
        store._db.execute(
            "SELECT paused,reason FROM beacon_pins_v2 WHERE operation_id=?", (operation,)
        ).fetchone()
    )


@pytest.mark.parametrize("lag", [MAX_LAG_ROUNDS + 1, 11, CAP])
@pytest.mark.parametrize("kind", ["aggregate", "finalize"])
def test_an_audited_round_is_aggregated_and_finalized_on_a_lagging_view(lagging, kind, lag):
    # The S21 views: a flicker of 3 at an aggregate's entry, 11 at a finalize's end, and the cap.
    store, now = lagging.store, lagging.now
    wall_at(lagging, now + lag)
    operation = f"{kind}:0:{now}"
    # Judged at the round a healthy view holds, never at the stale one and never later.
    assert store._fresh_v2(lagging.manifest, operation) == now + lag - MAX_LAG_ROUNDS
    assert pin(store, operation) == (0, None)


@pytest.mark.parametrize(
    ("kind", "fault"),
    [
        *(
            (kind, fault)
            for kind in ("aggregate", "finalize")
            for fault in (
                "past the cap",
                "an audit running",
                "an audit queued",
                "no audit job",
                "only another round audited",
                "an admitted run",
                "a paused run",
            )
        ),
        ("finalize", "not aggregated"),
        ("aggregate-result", "not a tolerated kind"),
    ],
)
def test_a_lagging_aggregate_or_finalize_is_refused_without_its_conditions(lagging, kind, fault):
    store, run, now = lagging.store, lagging.manifest.run_id(), lagging.now
    lag = CAP + 1 if fault == "past the cap" else MAX_LAG_ROUNDS + 1
    if fault.startswith("an audit"):
        state = fault.rpartition(" ")[2]
        store._db.execute("UPDATE audit_leases_v2 SET state=? WHERE id='job-0-1'", (state,))
    elif fault == "no audit job":
        store._db.execute("DELETE FROM audit_leases_v2")
    elif fault == "only another round audited":
        store._db.execute("UPDATE audit_leases_v2 SET w=1")
    elif fault == "an admitted run":
        store._put_record_v2(run, "service-admission", "run", {"probe": True})
    elif fault == "a paused run":
        store._db.execute("UPDATE runs SET status='paused'")
    elif fault == "not aggregated":
        store._db.execute("DELETE FROM records_v2 WHERE kind='applied'")
    wall_at(lagging, now + lag)
    operation = f"{kind}:0:{now}"
    with pytest.raises(ChallengeError) as refused:
        store._fresh_v2(lagging.manifest, operation)
    assert (refused.value.status, refused.value.detail) == (503, "beacon infrastructure paused")
    assert pin(store, operation) == (1, "STALE")


@pytest.fixture
def production_round(production):
    """The production store of hotfix 2, its one lease now the terminal audit of round 0."""
    store, manifest, now = production
    store._db.execute("UPDATE audit_leases_v2 SET w=0, state='completed'")
    store._put_record_v2(manifest.run_id(), "applied", "0", {"theta_hash": "a" * 64})
    return production


@pytest.mark.parametrize("kind", ["aggregate", "finalize"])
def test_production_takes_a_lagging_audited_round_on_its_bls_verified_pinned_round(
    production_round, kind
):
    store, manifest, now = production_round
    production_wall(store, manifest, now + CAP)
    operation = f"{kind}:0:{now}"
    assert store._fresh_v2(manifest, operation) == now + RESULT_PAUSE_ROUNDS
    assert production_pin(store, operation) == 0


@pytest.mark.parametrize("fault", ["forged signature", "fixture signature", "past the cap"])
@pytest.mark.parametrize("kind", ["aggregate", "finalize"])
def test_production_refuses_a_lagging_audited_round_without_a_verified_pin(
    production_round, kind, fault
):
    store, manifest, now = production_round
    lag = CAP + 1 if fault == "past the cap" else MAX_LAG_ROUNDS + 1
    if fault == "forged signature":
        signature = QUICKNET[1]["signature"]
        forged = signature[:-2] + ("00" if signature[-2:] != "00" else "01")
        store._db.execute("UPDATE beacon SET signature=? WHERE round=?", (forged, now))
    elif fault == "fixture signature":
        store._db.execute("UPDATE beacon SET signature=?,bls=0 WHERE round=?", ("aa" * 48, now))
    production_wall(store, manifest, now + lag)
    operation = f"{kind}:0:{now}"
    with pytest.raises(ChallengeError) as refused:
        store._fresh_v2(manifest, operation)
    assert (refused.value.status, refused.value.detail) == (
        503,
        "beacon infrastructure paused; pinned seed retained",
    )
    assert production_pin(store, operation) == 1
