"""Silent roster members are sanctioned at the bound of their wait and the run goes on."""

from __future__ import annotations

import pytest
import test_rollback_service_v2 as rollback
from test_no_show_resume_v2 import aggregate, audit_results, excluded, finalize

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.challenge.store import ChallengeError

fixtures = rollback.fixtures


@pytest.fixture
def six_network(tmp_path_factory):
    """Six ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures, size=6)


def standing(network, hotkey):
    row = network.store._db.execute(
        "SELECT state,reason,strikes FROM admissions_v2 WHERE hotkey=?", (hotkey,)
    ).fetchone()
    return tuple(row)


def test_two_silent_of_six_the_next_round_runs_with_the_four(six_network, monkeypatch):
    n = six_network
    run = n.manifest.run_id()
    silent, survivors = fixtures.HOT[4:], fixtures.HOT[:4]
    escrow = n.store._services(run)[0]
    burned = escrow.balances().burned
    jump, first_exclusion = n.push, []

    def walk(number):  # the helper of the round jumps; a real pusher walks the rounds
        for rnd in range(n.now + 1, number + 1):
            jump(rnd)
            if not first_exclusion and excluded(n):
                first_exclusion.append(rnd)

    monkeypatch.setattr(n, "push", walk)

    opening = rollback.execute_round(n, 0, silent=tuple(silent), aggregate=False)

    # The push of d_commit left them out: long before the audits, and before anybody called them.
    assert first_exclusion == [opening.d_commit] and opening.d_commit < opening.d_audit
    assert {k: (v["reason"], v["stage"]) for k, v in excluded(n).items()} == {
        f"0:{h.ss58}": ("NO_SHOW", "commit") for h in silent
    }
    assert [standing(n, h.ss58) for h in silent] == [("PROBATION", "NO_SHOW", 1)] * 2
    assert escrow.balances().burned - burned == 2000 and escrow.balances().conserved()
    assert audit_results(n) == ["MATCH"] * 4
    applied = aggregate(n, 0)
    assert applied.status_code == 200, applied.text
    assert finalize(n, 0, applied.json(), survivors)["status"] == "SETTLED"
    paid = n.store._db.execute(
        "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=0 ORDER BY hotkey"
    ).fetchall()
    assert [tuple(r) for r in paid] == [(k, 250000) for k in sorted(s.ss58 for s in survivors)]

    second = rollback.execute_round(n, 1, hotkeys=survivors)
    assert finalize(n, 1, second, survivors)["status"] == "SETTLED"


def test_a_member_that_has_no_anchor_of_the_round_before_cannot_be_rostered(six_network):
    # Given: round 0 settled with four of the six ACTIVE identities; the fifth never held an anchor
    # (a member that comes in later is the unsolved Q3 of ADR 0011)
    n = six_network
    four, late = fixtures.HOT[:4], fixtures.HOT[4]
    first = rollback.execute_round(n, 0, hotkeys=four)
    assert finalize(n, 0, first, four)["status"] == "SETTLED"

    # When / Then: it cannot be on the roster of round 1 (the opening restores the anchor of the
    # round before for every entry), so a retirement, which carries exactly those anchors, never
    # meets a member without one; the run goes on with the four
    with pytest.raises(ChallengeError, match="missing accepted anchor"):
        rollback.execute_round(n, 1, hotkeys=[*four, late], open_only=True)
    assert rollback.execute_round(n, 1, hotkeys=four, open_only=True).status_code == 200
