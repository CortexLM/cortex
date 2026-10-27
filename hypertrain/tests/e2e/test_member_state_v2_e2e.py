"""A roster member whose identity or eligibility changes after the round opened (R-C2).

One test per state, on five ACTIVE identities (see challenge/five_identity_network_v2.py).
"""

from __future__ import annotations

import json

import pytest
import test_rollback_service_v2 as rollback

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.miner.admission import sign_rotation, sign_rotation_consent
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import DisputeV2, EscrowLock

fixtures = rollback.fixtures


@pytest.fixture
def n(tmp_path_factory):
    """Five ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures)


def admission_id(n, hotkey):
    return n.store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (hotkey,)
    ).fetchone()[0]


def standing(n, hotkey):
    row = n.store._db.execute(
        "SELECT state,strikes,pending_dispute FROM admissions_v2 WHERE hotkey=?", (hotkey,)
    ).fetchone()
    return tuple(row)


def excluded(n):
    return {
        r[0]: json.loads(r[1])
        for r in n.store._db.execute("SELECT id,data FROM records_v2 WHERE kind='excluded'")
    }


def aggregate(n, w=0):
    return n.client.post(n.url + f"/admin/rounds/{w}/aggregate", headers=fixtures.admin())


def set_screen(n, hotkey, until):
    with n.store._tx():
        n.store._db.execute(
            "UPDATE admissions_v2 SET screen_until=? WHERE hotkey=?", (until, hotkey)
        )


def rotate(n, index):
    """The owner of identity `index` rotates it to the registered, never-joined referee key."""
    run, hot, cold = n.manifest.run_id(), fixtures.HOT[index], fixtures.COLD[index]
    request = sign_rotation_consent(
        fixtures.REFEREE,
        run_id=run,
        coldkey=cold.ss58,
        hotkey=hot.ss58,
        operation_id="aa" * 32,
        expires_beacon=10000,
    )
    return n.client.post(
        n.url + f"/admission/{hot.ss58}/rotate", json=sign_rotation(cold, run, request, 10000)
    )


def hotkey_of(n, identity):
    return n.store._db.execute(
        "SELECT hotkey FROM admissions_v2 WHERE admission_id=?", (identity,)
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


def open_contest(n, hot, cold):
    """Fund a contest bond and open the member's contest of its own round 0 verdict."""
    contest = json.loads(
        n.store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=? "
            "AND json_extract(data,'$.w')=0",
            (hot.ss58,),
        ).fetchone()[0]
    )
    dispute_id = sha256_hex(canonicalize([n.manifest.run_id(), contest["verdict_hash"], hot.ss58]))
    origin = n.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (cold.ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="df" * 32,
        owner=cold.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    assert n.client.post(n.url + "/escrow/lock", json=n.signed(cold, "EscrowLock", lock)).is_success
    body = DisputeV2(
        hotkey=hot.ss58,
        verdict_hash=contest["verdict_hash"],
        action="contest",
        bond_lock=lock.operation_id,
    )
    return n.client.post(n.url + "/dispute", json=n.signed(hot, "DisputeV2", body))


def test_a_member_cannot_suspend_itself_by_contesting_its_own_match(n):
    # Given: round 0 audited, the verdict of the fifth member is MATCH.
    rollback.execute_round(n, 0, aggregate=False)
    hot, cold = fixtures.HOT[4], fixtures.COLD[4]
    # When: it funds and opens a contest of that verdict.
    contest = open_contest(n, hot, cold)
    # Then: refused before anything is written, and the round goes on with all five.
    assert (contest.status_code, "NOTHING_TO_CONTEST" in contest.text) == (409, True)
    assert standing(n, hot.ss58) == ("ACTIVE", 0, 0)
    applied = aggregate(n)
    assert applied.status_code == 200, applied.text
    assert finalize(n, 0, applied.json(), fixtures.HOT).status_code == 200


def test_a_member_whose_screen_lapsed_is_left_out_and_the_run_goes_on(n):
    # Given: round 0 audited; the screen of the fifth member ends before the aggregate.
    run = n.manifest.run_id()
    survivors, lapsed = fixtures.HOT[:4], fixtures.HOT[4]
    rollback.execute_round(n, 0, aggregate=False)
    set_screen(n, lapsed.ss58, n.now + 3)
    n.push(n.now + 10)
    # When
    applied = aggregate(n)
    # Then: it is left out with its cause and no sanction; the other four are aggregated and paid.
    assert applied.status_code == 200, applied.text
    assert excluded(n) == {
        f"0:{lapsed.ss58}": {
            "reason": "INELIGIBLE",
            "stage": "aggregate",
            "cause": "SCREEN_LAPSED",
            "evidence_hash": sha256_hex(
                f"ht-ineligible/1|{run}|0|{lapsed.ss58}|SCREEN_LAPSED".encode()
            ),
        }
    }
    assert standing(n, lapsed.ss58) == ("ACTIVE", 0, 0)
    assert finalize(n, 0, applied.json(), survivors).status_code == 200
    assert paid(n, 0) == [(h, 250000) for h in sorted(k.ss58 for k in survivors)]
    # And the run goes on: round 1 opens with the four.
    assert rollback.execute_round(n, 1, hotkeys=survivors, open_only=True).status_code == 200


def test_four_members_cannot_lose_one_but_the_round_waits_for_it(n):
    # Given: a roster of exactly four, one of which lapses after its audit.
    four = fixtures.HOT[:4]
    rollback.execute_round(n, 0, hotkeys=four, aggregate=False)
    set_screen(n, four[3].ss58, n.now + 3)
    n.push(n.now + 10)
    # When: the remaining three cannot carry the round.
    refused = aggregate(n)
    # Then: a named refusal that persists nothing (never a bare 500) ...
    assert (refused.status_code, "ROUND_INFEASIBLE" in refused.text) == (409, True)
    assert excluded(n) == {}
    # ... so the round resumes as soon as the member is eligible again.
    set_screen(n, four[3].ss58, 10**9)
    assert aggregate(n).status_code == 200


def test_a_member_cannot_rotate_while_its_round_is_open(n):
    # Given: round 0 audited, every verdict MATCH, and the fifth member's owner who would rotate.
    hot = fixtures.HOT[4].ss58
    identity = admission_id(n, hot)
    rollback.execute_round(n, 0, aggregate=False)
    # When / Then: the rotation is refused after the audits, and the aggregate still works ...
    refused = rotate(n, 4)
    assert (refused.status_code, "ROTATION_ROUND_OPEN" in refused.text) == (409, True)
    applied = aggregate(n)
    assert applied.status_code == 200, applied.text
    # ... it is refused after the aggregate too, and the round settles with all five ...
    refused = rotate(n, 4)
    assert (refused.status_code, "ROTATION_ROUND_OPEN" in refused.text) == (409, True)
    assert hotkey_of(n, identity) == hot
    assert finalize(n, 0, applied.json(), fixtures.HOT).status_code == 200
    # ... after which the owner may rotate.
    assert rotate(n, 4).status_code == 200
    assert hotkey_of(n, identity) == fixtures.REFEREE.ss58


@pytest.mark.parametrize(("margin", "refused"), [(-1, True), (0, False)], ids=["short", "exact"])
def test_a_roster_entry_must_keep_its_screen_until_the_round_ends(n, margin, refused):
    # Given: the screen of one member ends `margin` rounds from the d_final of the opening below
    # (execute_round opens with d_final = now + 200).
    set_screen(n, fixtures.HOT[4].ss58, n.now + 200 + margin)
    # When / Then: a screen that ends before the round's audit window does is refused at open;
    # one that ends exactly at d_final is accepted (eligible is `now <= screen_until`).
    if refused:
        with pytest.raises(AssertionError, match="roster screen ends before the round's audit"):
            rollback.execute_round(n, 0, open_only=True)
    else:
        assert rollback.execute_round(n, 0, open_only=True).status_code == 200
