"""What the exit of a round owes the roster member who caused it, and what it owes the others (D47).

Package exit_roster, second module. The first module pins that no member can freeze a round; this
one pins the accounts the exit settles once a member is out: the admission of a member that never
showed is struck and its bond burned, a fault of the master's own sanctions nobody, a member that
changes after the round opened cannot take the round with it, and a convicted member is not paid
while the others are. The rules are those of ADR 0011 Part 8 and of Forge's decisions D47, D59 and
D63. The roster contract (a rotation while a round lists the admission, a lapsed screen, a
rotated hotkey) is in v3; the sanction of a no-show, the refusal of the contest of a verdict that
upholds the work and the payment of the others when a member is convicted came with
audit-integrity-2.

Every test states what happens to the standing of an admission (state, strikes, bond locked and
burned), to a status code or to who is paid, never how the master gets there: the sanction may be
booked by the first call that sees the silence or by the beacon, a refusal may come from the
admission or from the store. A hole is a strict xfail that asserts the desired defence after a
control the current code satisfies; each reason ends with the lot that will flip it.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from . import support_exit_roster as X

FUNDED = ("ACTIVE", 0, 1000, 0)  # state, strikes, bond locked, units burned


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


def second_no_show(rnd: X.Round, member: int, screens: int) -> None:
    """`member` is silent a second time, for the evidence of another round, `screens` work screens
    after the first one, having been put back on a roster in between (its trials are not under
    test here)."""
    X.seize(rnd, member, "NO_SHOW", FIRST_NO_SHOW)
    X.control(
        X.standing(rnd, member)[2:] == (0, 1000), "the first no-show burned the bond of the member"
    )
    X.back_on_a_roster(rnd, member)
    rnd.network.push(rnd.network.now + screens * X.screen_rounds(rnd))
    X.seize(rnd, member, "NO_SHOW", b"exit-roster|no-show|round 1")


# Closed by audit-integrity-2 (567fa939, D63): the first no-show books a strike and burns the bond.
def test_the_first_no_show_of_an_admission_costs_it_a_strike_and_its_bond(silent: X.Round) -> None:
    rnd = silent
    X.control(X.standing(rnd, rnd.quiet) == FUNDED, "the quiet member is a funded ACTIVE admission")

    others = silence_of_one_member(rnd)

    assert X.standing(rnd, rnd.quiet) == ("PROBATION", 1, 0, 1000)
    assert [X.standing(rnd, i) for i in others] == [FUNDED] * len(others)


# Closed by audit-integrity-2 (567fa939, D63): the first no-show is a fresh probation.
def test_the_first_no_show_of_an_admission_puts_it_on_probation(silent: X.Round) -> None:
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


# Closed by audit-integrity-2 (567fa939, 4909bf35, D63): the second one inside the window suspends.
def test_a_second_no_show_inside_the_window_suspends_the_admission(silent: X.Round) -> None:
    rnd, member = silent, X.UNROSTERED
    X.control(X.standing(rnd, member) == FUNDED, "the member is a funded ACTIVE admission")

    second_no_show(rnd, member, screens=1)
    X.seize(rnd, member, "NO_SHOW", FIRST_NO_SHOW)  # a retry books nothing

    assert X.standing(rnd, member) == ("SUSPENDED", 2, 0, 1000)


# Closed by audit-integrity-2 (5adfb637, D63): a suspension keeps its state and reason.
def test_a_no_show_adds_its_strike_to_a_suspension_and_keeps_its_reason(silent: X.Round) -> None:
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


# Closed by audit-integrity-2 (567fa939, 4909bf35, D63): only a no-show inside the window repeats.
def test_a_no_show_long_after_the_last_one_is_a_first_one_again(silent: X.Round) -> None:
    rnd, member = silent, X.UNROSTERED
    X.control(X.standing(rnd, member) == FUNDED, "the member is a funded ACTIVE admission")

    second_no_show(rnd, member, screens=100)

    assert X.standing(rnd, member) == ("PROBATION", 2, 0, 1000)
    assert X.admission_record(rnd, member)["reason"] == "NO_SHOW"


# A fault of the master's own is nobody's strike (D47, D63)


def test_a_loss_of_leaves_on_the_master_sanctions_nobody(queue: X.Round) -> None:
    """NSH-04: the leaf object member `head` published is gone from the master's store, so no
    auditor can be given its job; the member did what it owed (the master holds the record of its
    publication) and every window ends without a verdict. The job is given up, nobody is struck,
    nobody loses a bond, and whatever the exit records for the members it drops blames the
    infrastructure, never a silence or a failed audit. How the round then closes (each member
    excluded, or the round retired as a whole when nobody is left to carry it) is not under test."""
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
    assert set(rnd.excluded().values()) <= {"INFRASTRUCTURE"}
    assert [X.standing(rnd, i) for i in rnd.members] == [FUNDED] * len(rnd.members)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="NSH-03: lease_v2 reads the leaf object of the job at the head of the queue and a "
    "store that lost it raises ObjectNotFound there, so every auditor's ask is a 500 and the jobs "
    "behind it wait for their windows to end [fix-lot: none (no design yet)]",
)
def test_a_leaf_object_the_master_lost_does_not_starve_the_audits_behind_it(queue: X.Round) -> None:
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


# Closed by audit-integrity-2 (b0ac0ab5, D47): a verdict that upholds the work is not contested.
def test_a_miner_cannot_contest_a_verdict_that_upholds_its_work(tmp_path: Path) -> None:
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


# Closed by audit-integrity-2 (6d935814, D59): a Finalize lists the unconvicted and pays them.
def test_a_banned_admission_is_not_paid_and_the_others_are(tmp_path: Path) -> None:
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
