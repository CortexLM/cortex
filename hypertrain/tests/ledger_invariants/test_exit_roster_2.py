"""What the exit of a round owes the roster member who caused it, and what it owes the others (D47).

Package exit_roster, second module. The first module pins that no member can freeze a round; this
one pins the accounts the exit settles once a member is out: the admission of a member that never
showed is struck and its bond burned, a fault of the master's own sanctions nobody, a member that
changes after the round opened cannot take the round with it, and a convicted member is not paid
while the others are. The rules are those of ADR 0011 Part 8 and of Forge's decisions D47, D59 and
D63. The roster contract (a rotation while a round lists the admission, a lapsed screen, a
rotated hotkey), the sanction of a no-show, the refusal of the contest of a verdict that upholds
the work and the payment of the others when a member is convicted are pinned plainly. The one
strict xfail left is the audit queue that a leaf object the master lost starves (NSH-03).

Every test states what happens to the standing of an admission (state, strikes, bond locked and
burned), to a status code or to who is paid, never how the master gets there: the sanction may be
booked by the first call that sees the silence or by the beacon, a refusal may come from the
admission or from the store. A hole is a strict xfail that asserts the desired defence after a
control the current code satisfies; its reason ends with the lot that will flip it.

The strike window of a no-show is a rule of D63 that these tests use and do not measure: they ask
only that a second silence one beacon round after the first is inside it and that one a hundred
work screens after is outside it, which any window of a few screens that covers two
PROBATION -> ACTIVE ladders satisfies. Where the window ends is pinned by
tests/challenge/test_admission_bond_v2.py (a second no-show one round before the end of the
window suspends, one at its end is a fresh probation) and tests/challenge/test_strike_window_v2.py
(the window outlasts two ladders of the TEST and production policies); the lot does not mirror
them. The second silence finds the bond already burned by the first, so what the second tier
burns is not under test either.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from . import support_exit_roster as X

FUNDED = X.FUNDED


@pytest.fixture
def silent(tmp_path: Path) -> Iterator[X.Round]:
    with X.silent_round(tmp_path) as rnd:
        yield rnd


@pytest.fixture
def queue(tmp_path: Path) -> Iterator[X.Round]:
    with X.queued_round(tmp_path) as rnd:
        yield rnd


# The sanction of a member that never showed (D47, D63)


def silence_of_one_member(rnd: X.Round) -> list[int]:
    """The commit window of `rnd` ends round by round with its quiet member absent, the audits
    are scheduled, and whatever retries a sanction gets another turn. Returns the other members."""
    X.walk(rnd, rnd.opening.d_audit - 1)
    response = X.schedule(rnd)
    X.control(response.status_code == 200, response.text)
    X.control(
        rnd.excluded() == {rnd.hotkey(rnd.quiet): "NO_SHOW"},
        "the member that never committed is left out as a no-show, and only it",
    )
    X.walk(rnd, rnd.network.now + 1)
    return [i for i in rnd.members if i != rnd.quiet]


FIRST_NO_SHOW = b"exit-roster|no-show|round 0"


def second_no_show(rnd: X.Round, member: int, after: int) -> None:
    """`member` is silent a second time, for the evidence of another round, `after` beacon rounds
    after the first one, having been put back on a roster in between (its trials are not under
    test here)."""
    X.seize(rnd, member, "NO_SHOW", FIRST_NO_SHOW)
    X.control(
        X.standing(rnd, member)[2:] == (0, 1000), "the first no-show burned the bond of the member"
    )
    X.back_on_a_roster(rnd, member)
    rnd.network.push(rnd.network.now + after)
    X.seize(rnd, member, "NO_SHOW", b"exit-roster|no-show|round 1")


def test_the_first_no_show_of_an_admission_costs_it_a_strike_and_its_bond(silent: X.Round) -> None:
    """NSH-01 (D63): the first no-show of an admission costs it one strike and its bond and puts it
    on probation, not in a suspension; the standing of the other members is untouched. Without the
    cost an identity could idle in every round for nothing."""
    rnd = silent
    X.control(X.standing(rnd, rnd.quiet) == FUNDED, "the quiet member is a funded ACTIVE admission")

    others = silence_of_one_member(rnd)

    assert X.standing(rnd, rnd.quiet) == ("PROBATION", 1, 0, 1000)
    assert [X.standing(rnd, i) for i in others] == [FUNDED] * len(others)


def test_the_first_no_show_of_an_admission_puts_it_on_probation(silent: X.Round) -> None:
    """NSH-05 (D63): the first no-show of an admission puts it on a fresh probation (reason NO_SHOW,
    trials from clean_count 0); its way back is the trials."""
    rnd = silent
    record = X.admission_record(rnd, rnd.quiet)
    X.control(
        (record["state"], record["clean_count"]) == ("ACTIVE", 12) and record["screen_until"] > 0,
        "the quiet member is an ACTIVE admission with a work screen",
    )

    silence_of_one_member(rnd)

    record = X.admission_record(rnd, rnd.quiet)
    assert (record["state"], record["reason"]) == ("PROBATION", "NO_SHOW")
    assert (record["clean_count"], record["screen_until"]) == (0, 0)


def test_a_second_no_show_inside_the_window_suspends_the_admission(silent: X.Round) -> None:
    """NSH-02 (D63): a second no-show, for other evidence, inside the strike window adds a strike
    and suspends the admission. The second silence comes one beacon round after the first, which
    is inside any window the rule allows; where the window ends is not asserted here (module
    docstring)."""
    rnd, member = silent, X.UNROSTERED
    X.control(X.standing(rnd, member) == FUNDED, "the member is a funded ACTIVE admission")

    second_no_show(rnd, member, after=1)
    X.seize(rnd, member, "NO_SHOW", FIRST_NO_SHOW)  # a retry books nothing

    assert X.standing(rnd, member) == ("SUSPENDED", 2, 0, 1000)


def test_a_no_show_adds_its_strike_to_a_suspension_and_keeps_its_reason(silent: X.Round) -> None:
    """NSH-06 (D63): a no-show on a suspended admission adds its strike and keeps the state and the
    reason it was suspended for."""
    rnd, member = silent, X.UNROSTERED
    X.seize(rnd, member, "AUDIT_MISMATCH", b"exit-roster|mismatch|round 0")
    record = X.admission_record(rnd, member)
    X.control(
        (record["state"], record["reason"], record["strikes"])
        == ("SUSPENDED", "AUDIT_MISMATCH", 1),
        "the member is suspended for an audit mismatch, with one strike",
    )

    X.seize(rnd, member, "NO_SHOW", b"exit-roster|no-show|round 1")

    record = X.admission_record(rnd, member)
    assert (record["state"], record["reason"], record["strikes"]) == (
        "SUSPENDED",
        "AUDIT_MISMATCH",
        2,
    )


def test_a_no_show_long_after_the_last_one_is_a_first_one_again(silent: X.Round) -> None:
    """NSH-07 (D63): a no-show long after the last one is a first one again: another strike and a
    fresh probation, not a suspension."""
    rnd, member = silent, X.UNROSTERED
    X.control(X.standing(rnd, member) == FUNDED, "the member is a funded ACTIVE admission")

    second_no_show(rnd, member, after=100 * X.screen_rounds(rnd))

    assert X.standing(rnd, member) == ("PROBATION", 2, 0, 1000)
    assert X.admission_record(rnd, member)["reason"] == "NO_SHOW"


# A fault of the master's own is nobody's strike (D47, D63)


def test_a_loss_of_leaves_on_the_master_sanctions_nobody(queue: X.Round) -> None:
    """NSH-04: the leaf object member `head` published is gone from the master's store, so no
    auditor can be given its job; the member did what it owed (the master holds the record of its
    publication) and every window ends without a verdict. The job is given up, nobody is struck,
    nobody loses a bond, and no book of the run holds a fault against anybody: no exclusion for a
    silence or a failed audit, no exit sanction that blames a miner, no sanction applied by the
    admission service. How the round then closes (each member excluded, or the round retired as a
    whole when nobody is left to carry it) is not under test."""
    rnd = queue
    head = rnd.queue_order()[0]
    X.lose_leaves(rnd, head)
    X.control(
        rnd.has_record("leaves", rnd.identity(head)) and rnd.excluded() == {},
        "the master still holds the record of the publication and nobody is out of the round",
    )

    X.run_out_audit_windows(rnd)
    X.walk(rnd, rnd.network.now + 1)  # whatever retries a sanction gets another turn

    assert [row["state"] for row in rnd.lease_rows() if row["hotkey"] == rnd.hotkey(head)] == [
        "exhausted"
    ]
    assert X.faults_booked(rnd) == []
    assert [X.standing(rnd, i) for i in rnd.members] == [FUNDED] * len(rnd.members)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="NSH-03: lease_v2 skips a job whose leaves RECORD is missing (ADR 0011 Part 8, exit "
    "row 3) but reads the leaf object of a job whose record exists, and a store that lost the "
    "object raises ObjectNotFound there: every auditor's ask is a 500 and the jobs behind the "
    "head wait for their windows to end [fix-lot: none (no design yet)]",
)
def test_a_leaf_object_the_master_lost_does_not_starve_the_audits_behind_it(queue: X.Round) -> None:
    """NSH-03: the leaf object of the job at the head of the queue is gone from the master's store
    (its record of the publication stays): an auditor's ask is still served, with a job of another
    member, instead of failing for as long as the lost job is first in line."""
    rnd = queue
    head = rnd.queue_order()[0]
    X.lose_leaves(rnd, head)
    X.control(
        [row["state"] for row in rnd.lease_rows()] == ["queued"] * len(rnd.members),
        "nobody leased anything: the lost object is at the head of a full queue",
    )

    response = X.post_lease_quietly(rnd, X.lease_ask(rnd, X.service().AUDITORS[0], 1))

    assert response.status_code == 200, response.text[:200]
    assert response.json()["body"]["start_state"]["hotkey"] != rnd.hotkey(head)


# A roster is a contract until its round is closed (D47)


def test_an_admission_an_open_round_lists_cannot_rotate(queue: X.Round) -> None:
    rnd, member = queue, 0
    net = rnd.network
    path = net.url + "/admission/" + rnd.hotkey(member)
    before = net.client.get(path).json()
    X.control(
        before["record"]["state"] == "ACTIVE" and rnd.has_record("commit", rnd.identity(member)),
        "the member is a live admission with a commit in the open round",
    )

    response = net.client.post(path + "/rotate", json=X.rotation_request(rnd, member, X.SPARE))

    assert response.status_code == 409, response.text
    assert net.client.get(path).json() == before


def test_a_miner_cannot_contest_a_verdict_that_upholds_its_work(tmp_path: Path) -> None:
    """ROS-02 (D47): a miner cannot contest a verdict that upholds its work: the contest is refused,
    and it neither suspends the miner nor pins its round with a funded dispute nobody can win."""
    with X.audited_head(tmp_path) as (rnd, head):
        results = [row["result"] for row in rnd.lease_rows() if row["hotkey"] == rnd.hotkey(head)]
        X.control(
            results == ["MATCH"] and X.standing(rnd, head) == FUNDED and not X.disputes_opened(rnd),
            "the audit of the member says MATCH and nothing is contested",
        )

        response = X.contest(rnd, head)

        assert response.status_code == 409, response.text[:200]
        assert (X.standing(rnd, head), X.disputes_opened(rnd)) == (FUNDED, 0)


# A convicted member is not paid, and the others are (D59, D62)


def test_a_banned_admission_is_not_paid_and_the_others_are(tmp_path: Path) -> None:
    """ROS-03 (D59): after the conviction of one of the five members of a round, the Finalize of
    the four others is accepted and pays them, and the convicted member is paid nothing. It is the
    rule of the FIN-18+D59 test of test_finality_2.py, on a round of the roster fixtures whose five
    members really trained and were audited. That test is the stronger one: it also pins the ban
    by coldkey, the refusal of a Finalize that still lists the convicted, the split of the whole
    budget between the unconvicted and conservation. This one shows the rule holds end to end on a
    round nobody staged."""
    with X.audited_round(tmp_path, 5) as rnd:
        banned, others = 2, [0, 1, 3, 4]
        response = X.aggregate(rnd)
        X.control(response.status_code == 200, response.text)
        theta = str(response.json()["theta_hash"])
        X.control(
            [row["result"] for row in rnd.lease_rows()] == ["MATCH"] * 5
            and rnd.finality()["applied"],
            "all five members audited MATCH and the round is applied",
        )
        X.seize(rnd, banned, "TRAINING_FRAUD", b"exit-roster|conviction")
        X.control(X.standing(rnd, banned)[0] == "BANNED", "the referee's verdict banned the member")

        response = rnd.network.client.post(**X.finalize_request(rnd, others, theta))

        assert response.status_code == 200, response.text[:200]
        assert set(rnd.paid()) == {rnd.hotkey(i) for i in others}
        escrow = rnd.store._services(rnd.run_id)[0]
        assert escrow.balances(X.cold(banned).ss58).reward_pending == 0
        assert rnd.finality()["disposition"] == "SETTLED"


def test_a_member_whose_screen_lapsed_is_excluded_and_the_round_goes_on(tmp_path: Path) -> None:
    with X.audited_round(tmp_path, 5) as rnd:
        member = 1
        X.control(
            not rnd.finality()["applied"] and X.eligible(rnd, member) is True,
            "nothing is applied yet and the member is eligible",
        )
        with X.screen_expired(rnd, member):
            X.control(X.eligible(rnd, member) is False, "its work screen lapsed")

            response = X.aggregate(rnd)

            assert response.status_code == 200, response.text
            assert rnd.hotkey(member) in rnd.excluded()


def test_a_hotkey_rotated_after_the_audits_does_not_stall_the_round(tmp_path: Path) -> None:
    with X.audited_round(tmp_path, 5) as rnd:
        member, others = 1, [0, 2, 3, 4]
        X.control(not rnd.finality()["applied"], "nothing is applied yet")
        net = rnd.network

        rotated = net.client.post(
            net.url + "/admission/" + rnd.hotkey(member) + "/rotate",
            json=X.rotation_request(rnd, member, X.SPARE),
        )

        # Whether the rotation is accepted or refused, the round resolves for the others.
        assert rotated.status_code < 500, rotated.text
        response = X.aggregate(rnd)
        assert response.status_code == 200, response.text
        inputs = {item["roster"]["hotkey"] for item in response.json()["tape"]["body"]["inputs"]}
        assert inputs >= {rnd.hotkey(i) for i in others}
