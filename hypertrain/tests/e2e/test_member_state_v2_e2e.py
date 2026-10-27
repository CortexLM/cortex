"""A roster member whose identity or eligibility changes after the round opened (R-C2).

One test per state, on five ACTIVE identities (see challenge/five_identity_network_v2.py).
"""

from __future__ import annotations

import pytest
import test_rollback_service_v2 as rollback

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.protocol.messages import Finalize

fixtures = rollback.fixtures


@pytest.fixture
def n(tmp_path_factory):
    """Five ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures)


def admission_id(n, hotkey):
    return n.store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (hotkey,)
    ).fetchone()[0]


def finalize(n, w, applied, members):
    final = Finalize(
        w=w,
        final_theta_hash_w1=applied["theta_hash"],
        included=sorted(k.ss58 for k in members),
        entitlements_root="0" * 64,
    )
    return n.client.post(
        n.url + f"/admin/rounds/{w}/finalize",
        json=n.signed(fixtures.COORD, "Finalize", final),
        headers=fixtures.admin(),
    )


def paid(n, w):
    rows = n.store._db.execute(
        "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=? ORDER BY hotkey", (w,)
    )
    return [tuple(r) for r in rows]


def test_a_sanction_after_the_aggregate_does_not_stop_the_finalize(n):
    # Given: round 0 applied with four inputs (the fifth member never committed), then the bond
    # of an input is seized, as the no-show exclusion of the NEXT round does to a member that is
    # on both rosters. The live set is now three, the roster five, the applied tape four.
    run = n.manifest.run_id()
    silent, sanctioned = fixtures.HOT[4], fixtures.HOT[0]
    members = fixtures.HOT[:4]
    applied = rollback.execute_round(n, 0, silent=(silent,))
    burned = n.store._services(run)[1].seize_bond(
        admission_id(n, sanctioned.ss58), reason="NO_SHOW", evidence_hash="ab" * 32, now=n.now
    )
    assert burned == 1000
    # When
    settled = finalize(n, 0, applied, members)
    # Then: the round settles and pays exactly the inputs of the applied tape, the sanctioned
    # member included.
    assert settled.status_code == 200, settled.text
    tape_inputs = n.store._record_v2(run, "tape-inputs", "0")["inputs"]
    inputs = sorted(item["roster"]["hotkey"] for item in tape_inputs)
    assert inputs == sorted(k.ss58 for k in members)
    assert paid(n, 0) == [(h, 250000) for h in inputs]
