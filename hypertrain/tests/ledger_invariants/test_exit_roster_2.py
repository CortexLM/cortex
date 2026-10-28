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
from dataclasses import asdict
from pathlib import Path
from typing import Any

import pytest

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.relay.core import Settlement

from . import support_exit_roster as X

FUNDED = X.FUNDED
LOST = "LEAF_OBJECT_LOST"  # the code of the 503 and the result of the job it parks (D83)


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

    # they are left out for the audit nobody serves, not for silence
    X.deliver_deltas(rnd, rnd.members)

    X.run_out_audit_windows(rnd)
    X.walk(rnd, rnd.network.now + 1)  # whatever retries a sanction gets another turn

    assert [row["state"] for row in rnd.lease_rows() if row["hotkey"] == rnd.hotkey(head)] == [
        "exhausted"
    ]
    assert X.faults_booked(rnd) == []
    assert [X.standing(rnd, i) for i in rnd.members] == [FUNDED] * len(rnd.members)


def ask_a_lease(rnd: X.Round, tag: int, auditor: int = 0) -> Any:
    return X.post_lease_quietly(rnd, X.lease_ask(rnd, X.service().AUDITORS[auditor], tag))


def jobs(rnd: X.Round) -> dict[str, tuple[str, str | None]]:
    """hotkey -> (state, result) of the audit job of every member."""
    return {row["hotkey"]: (row["state"], row["result"]) for row in rnd.lease_rows()}


def lose_and_keep(rnd: X.Round, index: int) -> bytes:
    """The master's store loses the leaf object of member `index`; returns its bytes."""
    key = rnd.record("leaves", rnd.identity(index))["object"]
    data: bytes = rnd.store.objects.get(key)
    X.lose_leaves(rnd, index)
    return data


# Closed by v3-d82-burn-pending-reaudit (D83): a leaf object the master lost is a named 503.
def test_a_leaf_object_the_master_lost_is_refused_by_name_and_the_audits_behind_it_lease(
    queue: X.Round,
) -> None:
    """NSH-03 (D83): the leaf object of the job at the head of the queue is gone from the master's
    store. That is the master's fault: the ask is refused 503 with a code of its own, never a bare
    500; the refusal is journaled and is not undone with the transaction that refuses it; the job
    is given up without a verdict and without blaming its member, so the next ask is served the
    job behind it."""
    rnd = queue
    head, behind = rnd.queue_order()[:2]
    X.lose_leaves(rnd, head)
    X.control(
        [row["state"] for row in rnd.lease_rows()] == ["queued"] * len(rnd.members),
        "nobody leased anything: the lost object is at the head of a full queue",
    )

    refused = ask_a_lease(rnd, 1)
    served = ask_a_lease(rnd, 2)

    assert refused.status_code == 503, refused.text[:200]
    assert refused.json()["detail"] == f"{LOST}: round {rnd.w} job {rnd.job_id(head)}"
    assert rnd.record("leaf-object-lost", rnd.identity(head)) == {
        "job_id": rnd.job_id(head),
        "object": rnd.record("leaves", rnd.identity(head))["object"],
        "at": rnd.network.now,
    }
    assert jobs(rnd)[rnd.hotkey(head)] == ("exhausted", LOST)
    books = X.books(rnd)
    assert (books["excluded"], books["exit-sanction"], books["exit-failed"]) == ({}, [], [])
    assert served.status_code == 200, served.text[:200]
    assert served.json()["body"]["start_state"]["hotkey"] == rnd.hotkey(behind)


def test_a_lost_leaf_object_retires_the_round_at_the_next_push(queue: X.Round) -> None:
    """NSH-03 (D83): the round cannot be audited whole, so it is retired as a null round at the
    first push after the loss was found, not left to starve until its audit windows end. Its
    survivors are every member (the master lost the object, the members did what they owed), the
    lease that was running and the jobs that were not served are cancelled, the job that was
    refused keeps its cause, and the relay's custody of an upload of the round is released."""
    rnd = queue
    first, lost = rnd.queue_order()[:2]
    grant, horizon = "cd" * 32, 900
    X.lose_leaves(rnd, lost)
    X.receipt_of(rnd, grant, horizon)
    with pytest.raises(ChallengeError, match="pinned by unresolved finality"):
        rnd.store.relay_settlement_v2(rnd.run_id, grant)
    X.control(ask_a_lease(rnd, 1).status_code == 200, "the job ahead of the lost one is leased")
    X.control(rnd.finality()["disposition"] == "PENDING", "the round is pending before the loss")
    ask_a_lease(rnd, 2, auditor=1)  # the second auditor reaches the lost job

    X.walk(rnd, rnd.network.now + 1)  # one push, not a jump

    status = rnd.finality()
    assert (status["disposition"], status["unresolved_audits"]) == ("RETIRED", 0)
    retired = rnd.record("retired", str(rnd.w))
    assert retired["cause"] == LOST
    assert retired["survivors"] == sorted(rnd.hotkey(i) for i in rnd.members)
    assert rnd.store._round_closure_v2(rnd.run_id, rnd.w) == retired["evidence_hash"]
    assert jobs(rnd) == {
        rnd.hotkey(i): ("exhausted", LOST if i == lost else "ROUND_RETIRED") for i in rnd.members
    }
    closed = Settlement(
        retired["evidence_hash"], horizon, sha256_hex(canonicalize([])), horizon, horizon
    )
    assert rnd.store.relay_settlement_v2(rnd.run_id, grant) == asdict(closed)


def test_the_retirement_for_a_lost_leaf_object_sanctions_nobody_and_moves_no_unit(
    queue: X.Round,
) -> None:
    """NSH-03 (D83): the member whose object is gone, and the others, keep their standing, their
    strikes and their bond once the round is retired; no member is excluded, no sanction is
    booked, no step fails silently and the ledger writes no event. The exit then gets two more
    turns, the way a sanction that only the beacon books would."""
    rnd = queue
    head = rnd.queue_order()[0]
    clean = X.books(rnd)
    X.control(
        clean["standing"] == [FUNDED] * len(rnd.members) and clean["excluded"] == {},
        "four funded ACTIVE members and nobody out of the round",
    )
    X.lose_leaves(rnd, head)

    ask_a_lease(rnd, 1)
    X.walk(rnd, rnd.network.now + 1)
    assert rnd.finality()["disposition"] == "RETIRED"  # without it nothing below could be booked
    X.walk(rnd, rnd.network.now + 2)

    assert X.books(rnd) == clean
    escrow = rnd.store._services(rnd.run_id)[0]
    assert escrow.balances().conserved()
    escrow.verify()


def test_the_round_after_one_with_a_lost_leaf_object_opens_and_its_audits_lease(
    queue: X.Round,
) -> None:
    """NSH-03 (D83): other rounds keep leasing. The retirement carries the roster's anchors, so
    the next round opens on them, and nothing of the retired round stands in the way of the jobs
    of the next one."""
    rnd = queue
    X.lose_leaves(rnd, rnd.queue_order()[0])
    ask_a_lease(rnd, 1)
    X.walk(rnd, rnd.network.now + 1)

    answer, following = X.open_after(rnd, rnd.members)

    assert answer.status_code == 200, answer.text[:200]
    assert following is not None
    for index in following.members:
        X.join_forged(following, index)
    X.control(X.schedule(following).status_code == 200, "the audits of the next round are queued")
    served = ask_a_lease(following, 1)
    assert served.status_code == 200, served.text[:200]


def test_a_restored_leaf_object_puts_its_job_back_and_the_round_goes_on(queue: X.Round) -> None:
    """NSH-03 (D83): the loss may be healed before the push that would retire the round (an
    operator restores the object, or the miner publishes it again): the job is queued again,
    nothing is retired and the audit of the member is served."""
    rnd = queue
    head = rnd.queue_order()[0]
    data = lose_and_keep(rnd, head)
    X.control(ask_a_lease(rnd, 1).status_code == 503, "the first ask finds the loss")
    assert rnd.store.objects.put(data) == rnd.record("leaves", rnd.identity(head))["object"]

    X.walk(rnd, rnd.network.now + 1)

    assert rnd.finality()["disposition"] == "PENDING"
    assert jobs(rnd)[rnd.hotkey(head)] == ("queued", None)
    again = ask_a_lease(rnd, 2)
    assert again.status_code == 200, again.text[:200]
    assert again.json()["body"]["start_state"]["hotkey"] == rnd.hotkey(head)


def test_a_restored_leaf_object_does_not_revive_a_job_of_a_retired_round(queue: X.Round) -> None:
    """NSH-03 (D83): a retired round that an open dispute keeps in the reaper's sight takes no
    job back: nothing of it may be leased any more."""
    rnd = queue
    head = rnd.queue_order()[0]
    data = lose_and_keep(rnd, head)
    ask_a_lease(rnd, 1)
    X.walk(rnd, rnd.network.now + 1)
    X.control(rnd.finality()["disposition"] == "RETIRED", "the loss retired the round")
    with rnd.store._tx():
        status = rnd.finality()
        status["unresolved_disputes"] = 1
        rnd.store._put_record_v2(rnd.run_id, "finality", str(rnd.w), status)
    rnd.store.objects.put(data)

    X.walk(rnd, rnd.network.now + 1)

    assert jobs(rnd)[rnd.hotkey(head)] == ("exhausted", LOST)


def test_a_lost_leaf_object_waits_for_the_round_before_it_and_is_retired_when_that_one_closes(
    tmp_path: Path,
) -> None:
    """NSH-03 (D83): a round is retired only once every round before it is closed (ADR 0011
    Part 8). The refusal is the exit waiting, not forgetting: the loss stays on record, the
    jobs of the round that were not lost keep leasing meanwhile, and the first push after the
    round before settles retires it."""
    with X.audited_round(tmp_path, 5) as before:
        theta = str(X.aggregate(before).json()["theta_hash"])
        answer, rnd = X.open_after(before, before.members)
        X.control(answer.status_code == 200 and rnd is not None, answer.text[:200])
        assert rnd is not None
        for index in rnd.members:
            X.join_forged(rnd, index)
        X.control(X.schedule(rnd).status_code == 200, "the audits of the next round are queued")
        X.control(
            before.finality()["disposition"] == "PENDING" and before.finality()["applied"],
            "the round before is applied and not settled: it is still open",
        )
        X.lose_leaves(rnd, rnd.queue_order()[0])

        refused = ask_a_lease(rnd, 1)
        X.walk(rnd, rnd.network.now + 1)

        assert refused.status_code == 503, refused.text[:200]
        assert rnd.finality()["disposition"] == "PENDING"
        assert "an earlier round is not closed" in rnd.record("round-exit", str(rnd.w))["waiting"]
        assert ask_a_lease(rnd, 2).status_code == 200
        settled = before.network.client.post(**X.finalize_request(before, before.members, theta))
        X.control(settled.status_code == 200, settled.text[:200])

        X.walk(rnd, rnd.network.now + 1)

        assert rnd.finality()["disposition"] == "RETIRED"


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
