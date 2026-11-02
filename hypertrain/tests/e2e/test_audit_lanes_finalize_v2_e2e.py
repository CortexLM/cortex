"""Three audit lanes and a signed first window, audited and finalized on the real routes.

The hash train of ADR 0017 joins two lots on one round: T3 reads the lanes (`max_running_jobs`)
and the first audit window (`first_window_rounds`) from the run's signed audit policy (ADR 0018);
T1 authenticates a Finalize and judges its beacon gate at entry, before the replay, and closes the
round at the round the replay ends. One real round of five members on a run that signs three
lanes, four auditors and a first window of 300 rounds under a gap cap of 400, opened at that gap:
300 is neither two leases (200) nor the gap cap or the round's gap (400), so a reader of another
field than the window fails.
"""

from __future__ import annotations

import pytest
import test_rollback_service_v2 as rollback
from test_finalize_freshness_v2_e2e import (
    CAP,
    aggregated,
    finalize_round,
    settled_at,
    slow_replays,
    wall_at,
    written,
)
from test_reaudit_failed_v2_e2e import committed_round, execute, lease, receipt, sign

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.challenge.exit_v2 import MAX_LAG_ROUNDS
from hypertrain.protocol.keys import Keypair

fixtures = rollback.fixtures
LANES, GAP, WINDOW = 3, 400, 300
# Fresh keys for the third and fourth auditor, as challenge/test_audit_liveness_v2.py signs them
EXTRA_AUDITORS = [Keypair(b"\x75" * 32), Keypair(b"\x76" * 32)]


@pytest.fixture
def n(tmp_path_factory, monkeypatch):
    """Five ACTIVE identities and four listed auditors; the audit policy signs the lanes, the gap
    cap and the first window."""
    monkeypatch.setattr(fixtures, "AUDITORS", [*fixtures.AUDITORS, *EXTRA_AUDITORS])
    for field, value in (
        ("max_running_jobs", LANES),
        ("gap_cap_rounds", GAP),
        ("first_window_rounds", WINDOW),
    ):
        monkeypatch.setitem(fixtures.AUDIT_POLICY_EXTRA, field, value)
    yield from five_identity_network(tmp_path_factory, fixtures)


def test_a_round_audited_on_three_lanes_in_its_first_window_settles_where_its_replay_ends(
    n, monkeypatch
):
    a, b, c, d = fixtures.AUDITORS
    committed_round(n, gap=GAP)
    scheduled = n.client.post(n.url + "/admin/rounds/0/audits", headers=fixtures.admin())
    assert scheduled.status_code == 200, scheduled.text
    d_audit = n.now
    # Every job's first window ends at d_audit + 300, a hundred rounds before d_final.
    absolute = n.store._db.execute(
        "SELECT absolute FROM audit_leases_v2 WHERE run_id=? AND w=0", (n.manifest.run_id(),)
    ).fetchall()
    assert [row[0] for row in absolute] == [d_audit + WINDOW] * len(fixtures.HOT)

    # Three lanes: three auditors hold a lease at once and the fourth is answered 204.
    running = [(key, lease(n, key, tag)) for tag, key in enumerate((a, b, c), 1)]
    full = n.client.post(n.url + "/worker/lease", json=receipt(n, d, f"{4:064x}"))
    assert full.status_code == 204, full.text
    verdicts = {}
    for key, job in running:
        verdicts[job["start_state"]["hotkey"]] = sign(n, key, job, execute(n, key, job))
    job = lease(n, d, 5)  # a lane is free again
    verdicts[job["start_state"]["hotkey"]] = sign(n, d, job, execute(n, d, job))
    # A lease taken late in the first window runs a full lease and ends with the window.
    n.push(d_audit + 150)
    job = lease(n, a, 6)
    assert (job["lease_expires"], job["absolute_deadline"]) == (d_audit + 250, d_audit + WINDOW)
    verdicts[job["start_state"]["hotkey"]] = sign(n, a, job, execute(n, a, job))
    assert verdicts == {hot.ss58: "MATCH" for hot in fixtures.HOT}

    view = n.now
    applied = aggregated(n, view)
    replay_ends = view + CAP + 10  # nothing is pushed while the replay runs
    replays = slow_replays(n, monkeypatch, to=replay_ends)
    before = written(n)
    # Refused at entry, before the gate and any replay: a Finalize a miner signed, on a view past
    # the cap, and one of the coordinator's that expired at the round the gate judges, though not
    # at the stale view.
    wall_at(n, view + CAP + 1)
    stranger = finalize_round(n, 0, applied, key=fixtures.HOT[0])
    wall_at(n, view + 10)
    expired = finalize_round(n, 0, applied, view + 4)
    assert [(r.status_code, r.json()["detail"]) for r in (stranger, expired)] == [
        (409, "signer is not authorized for message role"),
        (409, "envelope expired"),
    ]
    assert replays == [] and written(n) == before
    # Fresh at entry, then a replay that ends past the cap: settled, closed and vested from the
    # round the replay ended at, as a healthy view held it then, not from the entry round.
    wall_at(n, view)
    settled = finalize_round(n, 0, applied)
    assert settled.status_code == 200, settled.text
    assert settled.json()["status"] == "SETTLED"
    assert replays == [0]
    closed = replay_ends - MAX_LAG_ROUNDS
    vesting = closed + n.manifest.training.verify.E_vest_rounds
    assert settled_at(n, 0) == (closed, {vesting}), f"entry round {view}"
