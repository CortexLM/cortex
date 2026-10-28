"""A member kept out of a round by its own contest is sanctioned once, not twice (IMP-4, D86).

ADR 0011 Part 8, row 8: "the member's own state; never a second sanction". A roster member that
opens the contest of its own audit verdict is suspended (`PENDING_DISPUTE`) and cannot Accept. The
exit reaper still leaves it out of the round at the bound of the commit window, and books the
silence as a NO_SHOW. The NO_SHOW sanction then waits for the contest to end:

- the member lost it (a referee finding against it, a lapsed turn): the contest sanctioned it
  already and its absence is covered, no second bond is burned and no second strike is added;
- it won it, a closure ended it without a loser, or it never had one: NO_SHOW applies as D63 says;
- the contest is still open at the bound of the wait, or the round is closed: the hold is dropped
  and the exclusion stays the infrastructure's.

The contest is the real one: funded and opened through the routes, answered at step level, closed
by the beacon or by the referee's resolution. Only the audit's verdict records are seeded.
"""

# ruff: noqa: F811  (the `six_network` fixture parameter shadows the imported fixture)
from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

from hypertrain.challenge import exit_v2
from hypertrain.challenge.exit_v2 import Wait
from hypertrain.challenge.store import ChallengeStore
from ledger_invariants import support_exit_disputes as X

from . import test_service_network_v2 as fixtures
from .five_identity_network_v2 import push_through
from .test_exit_reaper_v2 import commit_records, rows_of, six_network, standing  # noqa: F401
from .test_round_refusals_v2 import open_round, round_zero

CONTESTANT, SILENT = 4, 5  # slots of the six identities: the member with a contest, a plain one


class Round:
    """Round 0 opened for real on six ACTIVE identities, four of which commit; the contestant and
    one plain member stay silent. The contestant opens its own contest before the commit bound."""

    def __init__(self, n, monkeypatch) -> None:
        monkeypatch.setattr(X, "service", lambda: fixtures)  # six identities, not the lot's four
        self.n = n
        self.run = n.manifest.run_id()
        self.world = SimpleNamespace(network=n, store=n.store, run_id=self.run)
        self.escrow = n.store._services(self.run)[0]
        self.hot = fixtures.HOT[CONTESTANT]
        self.opening = round_zero(n, fixtures.HOT)
        assert open_round(n, self.opening, n.client).status_code == 200
        commit_records(n, fixtures.HOT[:4])

    def contest(self) -> str:
        """The contestant's own contest of its audit verdict, funded and opened (SUSPENDED)."""
        w, index = self.world, CONTESTANT
        push_through(self.n, self.opening.d_commit - 2)
        dispute_id = X.seed_contest(w, index, w=0)
        with self.n.store._tx():  # round 0 is also the round it is silent in: no audit exclusion
            self.n.store._db.execute(
                "DELETE FROM records_v2 WHERE run_id=? AND kind='excluded' AND id=?",
                (self.run, f"0:{self.hot.ss58}"),
            )
        X.fund_contest(w, index)
        response = X.post_dispute(w, index)
        assert response.status_code == 200, response.text
        assert standing(self.n, self.hot.ss58) == ("SUSPENDED", "PENDING_DISPUTE", 0)
        return dispute_id

    def reach_commit_bound(self) -> None:
        push_through(self.n, self.opening.d_commit)

    def sanctions(self, index: int = CONTESTANT) -> list[str]:
        """The reasons the bond of the member was sanctioned for, in order."""
        rows = self.n.store._db.execute(
            "SELECT reason FROM admission_sanctions WHERE admission_id=("
            "SELECT admission_id FROM admissions_v2 WHERE hotkey=?) ORDER BY rowid",
            (fixtures.HOT[index].ss58,),
        )
        return [row[0] for row in rows]

    def row(self, index: int = CONTESTANT) -> dict:
        return json.loads(
            self.n.store._db.execute(
                "SELECT data FROM records_v2 WHERE kind='exit-sanction' AND id=?",
                (f"0:{fixtures.HOT[index].ss58}",),
            ).fetchone()[0]
        )

    def burned(self) -> int:
        return self.escrow.balances().burned

    def lose(self, dispute_id: str) -> None:
        """Both parties answer, then the referee finds the training of the contestant fraudulent."""
        X.pause_at_step(self.world, dispute_id)
        X.resolve(self.world, dispute_id, reason="FRAUD", loser=self.hot.ss58)


@pytest.fixture
def case(six_network, monkeypatch):
    return Round(six_network, monkeypatch)


def test_a_silent_member_with_a_contest_open_is_left_out_but_not_sanctioned_yet(case):
    # Given: the contestant's contest is open when the commit window closes
    case.contest()
    before = case.burned()

    # When
    case.reach_commit_bound()

    # Then: both silent members are excluded from the round; the plain one is sanctioned at once,
    # the contestant's sanction waits for its contest
    excluded = case.n.store._db.execute(
        "SELECT id FROM records_v2 WHERE kind='excluded' "
        "AND json_extract(data,'$.reason')='NO_SHOW'"
    )
    assert sorted(row[0] for row in excluded) == sorted(
        f"0:{fixtures.HOT[i].ss58}" for i in (CONTESTANT, SILENT)
    )
    assert rows_of(case.n) == {
        f"0:{fixtures.HOT[CONTESTANT].ss58}": ("MINER", "PENDING"),
        f"0:{fixtures.HOT[SILENT].ss58}": ("MINER", "APPLIED"),
    }
    assert case.sanctions() == [] and case.sanctions(SILENT) == ["NO_SHOW"]
    assert standing(case.n, case.hot.ss58) == ("SUSPENDED", "PENDING_DISPUTE", 0)
    assert case.burned() - before == 1000  # the plain member's bond, not the contestant's


def test_a_contest_lost_after_the_exclusion_covers_the_absence_and_nothing_more_is_burned(case):
    # Given: the contestant is excluded with its contest open, its sanction held
    dispute_id = case.contest()
    case.reach_commit_bound()
    held = case.burned()

    # When: the referee finds against it, then the beacon goes on
    case.lose(dispute_id)
    lost = case.burned()
    push_through(case.n, case.n.now + 2)

    # Then: the contest sanctioned it once (banned, bond and contest bond burned); the absence is
    # covered by that contest and adds neither a sanction row, a strike nor a unit burned
    assert lost - held >= 1000
    assert standing(case.n, case.hot.ss58)[0] == "BANNED"
    assert case.sanctions() == ["TRAINING_FRAUD"]
    row = case.row()
    assert (row["reason"], row["state"], row["covered_by"]) == ("NO_SHOW", "WAIVED", dispute_id)
    assert case.burned() == lost
    assert case.escrow.balances().conserved()


def test_a_contest_lost_before_the_bound_covers_the_absence_from_the_start(case):
    # Given: the contest is lost before the commit window closes
    dispute_id = case.contest()
    case.lose(dispute_id)
    lost = case.burned()

    # When
    case.reach_commit_bound()

    # Then: one sanction, the contest's; the silence is booked and waived in the same push
    assert case.sanctions() == ["TRAINING_FRAUD"]
    assert case.row()["state"] == "WAIVED" and case.row()["covered_by"] == dispute_id
    assert case.burned() == lost + 1000  # only the plain member's bond


def test_a_contest_a_closure_ended_without_a_loser_leaves_the_no_show_to_apply(case):
    # Given: the contest is open at the bound, so the contestant's sanction is held
    dispute_id = case.contest()
    case.reach_commit_bound()
    before = case.burned()
    assert case.row()["state"] == "PENDING"

    # When: both parties answer honestly, the transcript pauses where no referee can score it,
    # and the beacon closes the dispute without a loser (the contestant resumes on probation)
    X.pause_at_step(case.world, dispute_id)
    push_through(case.n, case.n.now + 1)

    # Then: nobody lost, so D63 applies: a first NO_SHOW is a strike, the bond burns, and the
    # member is back on a fresh probation
    closed = X.disputes(case.world).get(dispute_id)
    assert (closed.resolution.reason, closed.resolution.loser) == ("INFRASTRUCTURE", None)
    assert case.row()["state"] == "APPLIED"
    assert case.sanctions() == ["NO_SHOW"]
    assert standing(case.n, case.hot.ss58) == ("PROBATION", "NO_SHOW", 1)
    assert case.burned() - before == 1000


def test_a_contest_refused_at_the_door_changes_nothing_for_the_absence(case):
    # Given: the contestant's contest of a verdict that upholds its work is refused (409)
    push_through(case.n, case.opening.d_commit - 2)
    X.seed_contest(case.world, CONTESTANT, w=0, mismatch=False)
    X.fund_contest(case.world, CONTESTANT)
    refused = X.post_dispute(case.world, CONTESTANT)
    assert refused.status_code == 409, refused.text
    before = case.burned()

    # When
    case.reach_commit_bound()

    # Then: it was never suspended and has no contest: a plain NO_SHOW, applied now
    assert case.row()["state"] == "APPLIED"
    assert case.sanctions() == ["NO_SHOW"]
    assert standing(case.n, case.hot.ss58) == ("PROBATION", "NO_SHOW", 1)
    assert case.burned() - before == 2000  # the contestant's bond and the plain member's


def test_a_contest_lost_before_the_round_opened_covers_no_absence_of_this_round(case):
    # Given: the contestant lost a contest, but the record says it ended before round 0 opened
    dispute_id = case.contest()
    case.lose(dispute_id)
    with case.n.store._tx():
        turn = json.loads(
            case.n.store._db.execute(
                "SELECT turn FROM disputes_v2 WHERE id=?", (dispute_id,)
            ).fetchone()[0]
        )
        turn["resolved_beacon"] = 0
        case.n.store._db.execute(
            "UPDATE disputes_v2 SET turn=? WHERE id=?", (json.dumps(turn), dispute_id)
        )

    # When
    case.reach_commit_bound()

    # Then: nothing ties this absence to that contest, so the NO_SHOW is its own sanction
    assert case.row()["state"] == "APPLIED" and "covered_by" not in case.row()
    assert case.sanctions() == ["TRAINING_FRAUD", "NO_SHOW"]


@pytest.mark.parametrize("ends", ["the-dispute-bound", "the-round-closes"])
def test_a_hold_is_dropped_at_the_bound_of_the_wait_and_sanctions_nobody(case, monkeypatch, ends):
    # Given: a contest nobody ever resolves (the dispute reaper is out), and its sanction held
    case.contest()
    monkeypatch.setattr(ChallengeStore, "_close_blocked_disputes_v2", lambda *a, **k: None)
    case.reach_commit_bound()
    bound = case.opening.d_commit + 5
    far = 10**6
    monkeypatch.setitem(
        exit_v2.BOUNDS, Wait.DISPUTE, lambda schedule: bound if ends == "the-dispute-bound" else far
    )
    before = case.burned()
    push_through(case.n, bound - 1)
    assert case.row()["state"] == "PENDING"

    # When: the bound is reached, or the round is closed by other means
    if ends == "the-round-closes":
        with case.n.store._tx():
            status = case.n.store._record_v2(case.run, "finality", "0")
            closed = {"disposition": "RETIRED", "unresolved_disputes": 0, "unresolved_audits": 0}
            case.n.store._put_record_v2(case.run, "finality", "0", {**status, **closed})
    push_through(case.n, bound)

    # Then: the exclusion stays, as the infrastructure's, with no sanction and no strike
    row = case.row()
    assert (row["state"], row["blame"]) == ("WAIVED", "INFRASTRUCTURE")
    assert case.sanctions() == []
    assert standing(case.n, case.hot.ss58) == ("SUSPENDED", "PENDING_DISPUTE", 0)
    assert case.burned() == before
