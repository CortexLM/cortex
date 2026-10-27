"""An honest miner is not suspended because the operator's beacon pusher was late (D40, S13)."""

from __future__ import annotations

from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)


def standing(n, identity):
    row = n.store._db.execute(
        "SELECT state,reason,strikes FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()
    return tuple(row)


def test_a_trial_that_straddles_a_beacon_lag_suspends_nobody(network):  # noqa: F811
    """S13: two identities were lost because the operator changed the beacon cadence while their
    trials were open; the late catch-up of the pusher put the beacon past the trial deadline and
    the next request suspended them (CHALLENGE_EXPIRED, no way back)."""
    # Given: an applicant opens its trial (window 2 to 102) and the pusher goes down.
    identity = network.join(0).json()["admission_id"]
    network.push(2)
    assert network.challenge(identity).status_code == 200
    # When: the pusher catches up 130 rounds later and the miner, late through no fault of its
    # own, asks again.
    network.push(132)
    late = network.challenge(identity)
    # Then: the answer is a refusal, the trial is closed as EXPIRED, and the miner is neither
    # suspended nor struck.
    state, reason, strikes = standing(network, identity)
    assert (state, strikes) == ("APPLIED", 0) and reason != "CHALLENGE_EXPIRED"
    assert late.status_code == 409 and "CHALLENGE_EXPIRED" in late.text
    outcomes = network.store._db.execute("SELECT outcome FROM admission_trial_results").fetchall()
    assert [row[0] for row in outcomes] == ["EXPIRED"]
