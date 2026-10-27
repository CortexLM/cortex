"""A roster member that stops after the round opened no longer stops the run (AUD-06)."""

from __future__ import annotations

import json

import pytest
import test_rollback_service_v2 as rollback

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.challenge.finality_v2 import require_open
from hypertrain.protocol.messages import Finalize

fixtures = rollback.fixtures


@pytest.fixture
def activated_network(tmp_path_factory):
    """Five ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures)


def standing(network, hotkey):
    row = network.store._db.execute(
        "SELECT state,strikes,pending_dispute,clean_count FROM admissions_v2 WHERE hotkey=?",
        (hotkey,),
    ).fetchone()
    return tuple(row)


def excluded(network):
    return {
        r[0]: json.loads(r[1])
        for r in network.store._db.execute("SELECT id,data FROM records_v2 WHERE kind='excluded'")
    }


def audit_results(network):
    rows = network.store._db.execute("SELECT result FROM audit_leases_v2 WHERE state='completed'")
    return [r[0] for r in rows]


def finalize(network, w, applied, survivors):
    final = Finalize(
        w=w,
        final_theta_hash_w1=applied["theta_hash"],
        included=sorted(k.ss58 for k in survivors),
        entitlements_root="0" * 64,
    )
    response = network.client.post(
        network.url + f"/admin/rounds/{w}/finalize",
        json=network.signed(fixtures.COORD, "Finalize", final),
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    return response.json()


def test_round_proceeds_to_w_plus_2_after_a_member_never_commits(activated_network):
    n = activated_network
    run = n.manifest.run_id()
    silent, survivors = fixtures.HOT[4], fixtures.HOT[:4]
    before = standing(n, silent.ss58)

    first = rollback.execute_round(n, 0, silent=(silent,))

    assert audit_results(n) == ["MATCH"] * 4
    seen = excluded(n)
    assert {k: (v["reason"], v["stage"]) for k, v in seen.items()} == {
        f"0:{silent.ss58}": ("NO_SHOW", "commit")
    }
    status = n.store._record_v2(run, "finality", "0")
    assert (status["applied"], status["disposition"], status["unresolved_audits"]) == (
        True,
        "PENDING",
        0,
    )
    assert finalize(n, 0, first, survivors)["status"] == "SETTLED"
    paid = n.store._db.execute(
        "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=0 ORDER BY hotkey"
    ).fetchall()
    assert [tuple(r) for r in paid] == [(k, 250000) for k in sorted(s.ss58 for s in survivors)]
    assert standing(n, silent.ss58) == before

    second = rollback.execute_round(n, 1, hotkeys=survivors)
    assert finalize(n, 1, second, survivors)["status"] == "SETTLED"
    require_open(run, 2, n.store._history_v2(run))
    assert rollback.execute_round(n, 2, open_only=True).status_code == 200
