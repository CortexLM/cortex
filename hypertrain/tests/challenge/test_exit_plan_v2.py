"""The general exit is pure beacon-round arithmetic: the bounds, the planner, the blame rule."""

from __future__ import annotations

from dataclasses import replace
from typing import get_args

import pytest

from hypertrain.challenge import exit_v2 as x
from hypertrain.challenge.admission import SeizureReason

S = x.Schedule(now=5, d_commit=30, d_audit=31, d_upload=150, d_final=231, lease_rounds=100)
LONG = replace(S, d_upload=400, d_final=1000)  # a gap wider than two lease windows
ROSTER = tuple(x.Member(f"h{i}", f"a{i}") for i in range(6))


def keys(*numbers: int) -> frozenset[str]:
    return frozenset(f"h{i}" for i in numbers)


def snap(*, at: int = 5, roster=ROSTER, **kw) -> x.RoundSnapshot:
    base = dict(
        w=0,
        schedule=replace(S, now=at),
        roster=roster,
        committed=frozenset(),
        excluded=frozenset(),
        audited=frozenset(),
        delivered=frozenset(),
        uploaded=frozenset(),
        applied=False,
        served=True,
        served_upload=True,
        has_leases=True,
        carries=lambda hotkeys: len(hotkeys) >= 4,
    )
    return x.RoundSnapshot(**{**base, **kw})


def test_every_wait_has_a_bound_no_later_than_the_second_audit_window():
    assert set(x.BOUNDS) == set(x.Wait)
    for wait, bound in x.BOUNDS.items():
        assert S.now <= bound(S) <= S.d_audit + 4 * S.lease_rounds + 1, wait


def test_each_bound_is_a_round_schedule_value():
    due = {wait: bound(LONG) for wait, bound in x.BOUNDS.items()}
    assert due[x.Wait.COMMIT] == 30
    assert due[x.Wait.DELTA] == 400
    assert due[x.Wait.LEAVES] == min(1000, 31 + 2 * 100)
    assert due[x.Wait.AUDIT_WINDOW] == min(1000, 31 + 2 * 100) - 1
    assert due[x.Wait.AUDIT_LEASE] == 31 + 4 * 100
    assert due[x.Wait.DISPUTE] == 31 + 4 * 100 + 1
    assert due[x.Wait.MEMBER_STATE] == 1000
    at_the_door = (x.Wait.INFEASIBLE, x.Wait.ROTATION, x.Wait.OWN_CONTEST, x.Wait.FINALIZE_ORDER)
    assert [due[wait] for wait in at_the_door] == [LONG.now] * 4


def test_nothing_is_planned_before_the_commit_window_closes_or_for_an_applied_round():
    assert x.plan_round(snap(at=29, committed=keys(0, 1, 2, 3))) == x.RoundPlan((), None)
    assert x.plan_round(snap(at=30, committed=keys(0, 1, 2, 3), applied=True)) == x.RoundPlan(
        (), None
    )


def test_two_silent_of_six_are_excluded_at_d_commit_and_the_round_goes_on():
    plan = x.plan_round(snap(at=30, committed=keys(0, 1, 2, 3)))
    assert [
        (e.hotkey, e.admission_id, e.reason, e.stage, e.blame, e.record) for e in plan.exclusions
    ] == [
        ("h4", "a4", "NO_SHOW", "commit", "MINER", True),
        ("h5", "a5", "NO_SHOW", "commit", "MINER", True),
    ]
    assert plan.retire is None


def test_a_member_already_excluded_is_not_excluded_twice():
    plan = x.plan_round(snap(at=30, committed=keys(0, 1, 2, 3), excluded=keys(4, 5)))
    assert plan == x.RoundPlan((), None)


def test_two_silent_of_five_are_booked_but_leave_no_membership_fact():
    plan = x.plan_round(snap(at=30, roster=ROSTER[:5], committed=keys(0, 1, 2)))
    assert plan.retire == "INFEASIBLE"
    assert [(e.hotkey, e.blame, e.record) for e in plan.exclusions] == [
        ("h3", "MINER", False),
        ("h4", "MINER", False),
    ]


def test_an_earlier_exclusion_counts_against_the_survivors():
    plan = x.plan_round(
        snap(at=30, roster=ROSTER[:5], committed=keys(0, 1, 2, 3), excluded=keys(3))
    )
    assert plan.retire == "INFEASIBLE"
    assert [e.hotkey for e in plan.exclusions] == ["h4"]


@pytest.mark.parametrize(
    ("stage", "served", "committed_any", "delivered_any", "uploaded", "blame"),
    [
        ("commit", True, True, False, False, "MINER"),
        ("commit", False, True, False, False, "INFRASTRUCTURE"),
        ("commit", True, False, False, False, "INFRASTRUCTURE"),
        ("delta", True, True, True, False, "MINER"),
        ("delta", False, True, True, False, "INFRASTRUCTURE"),
        ("delta", True, True, False, False, "INFRASTRUCTURE"),
        ("delta", True, True, True, True, "INFRASTRUCTURE"),
        ("leaves", True, True, True, False, "INFRASTRUCTURE"),
        ("leaves", False, False, False, False, "INFRASTRUCTURE"),
    ],
    ids=[
        "commit-served",
        "commit-paused",
        "commit-nobody-committed",
        "delta-served",
        "delta-paused",
        "delta-nobody-delivered",
        "delta-the-master-holds-an-upload-receipt",
        "leaves-the-master-lost-them",
        "leaves-whatever-the-window",
    ],
)
def test_only_a_witnessed_silence_is_the_miners(
    stage, served, committed_any, delivered_any, uploaded, blame
):
    got = x.classify(
        stage,
        served=served,
        committed_any=committed_any,
        delivered_any=delivered_any,
        uploaded=uploaded,
    )
    assert got == blame


def test_the_commit_stage_reads_the_assignment_window_and_the_zero_commit_rule():
    paused = x.plan_round(snap(at=30, committed=keys(0), served=False, served_upload=True))
    nobody = x.plan_round(snap(at=30))
    served = x.plan_round(snap(at=30, committed=keys(0), served=True, served_upload=False))
    assert {e.blame for e in paused.exclusions} == {"INFRASTRUCTURE"}
    assert {e.blame for e in nobody.exclusions} == {"INFRASTRUCTURE"}
    assert {e.blame for e in served.exclusions} == {"MINER"}


@pytest.mark.parametrize(
    ("kw", "blame"),
    [
        ({"delivered": keys(0, 1, 2, 3, 4)}, "MINER"),
        ({"delivered": keys(0, 1, 2, 3, 4), "served_upload": False}, "INFRASTRUCTURE"),
        ({"delivered": frozenset()}, "INFRASTRUCTURE"),
        ({"delivered": keys(0, 1, 2, 3, 4), "uploaded": keys(5)}, "INFRASTRUCTURE"),
        ({"delivered": keys(0, 1, 2, 3, 4), "uploaded": keys(0)}, "MINER"),
    ],
    ids=["served", "paused", "nobody-delivered", "upload-evidence", "evidence-of-another-member"],
)
def test_the_delta_stage_excludes_an_audited_member_without_a_delta(kw, blame):
    everyone = keys(*range(6))
    plan = x.plan_round(snap(at=150, committed=everyone, audited=everyone, **kw))
    assert [(e.hotkey, e.stage, e.blame) for e in plan.exclusions if e.hotkey == "h5"] == [
        ("h5", "delta", blame)
    ]
    assert all(e.stage == "delta" for e in plan.exclusions)


def test_the_delta_stage_leaves_a_member_whose_audit_is_still_queued_to_its_audit():
    plan = x.plan_round(
        snap(
            at=150, committed=keys(*range(6)), audited=keys(0, 1, 2, 3), delivered=keys(0, 1, 2, 3)
        )
    )
    assert plan == x.RoundPlan((), None)


def test_members_that_cannot_deliver_any_more_leave_too_few_before_their_audits_end():
    plan = x.plan_round(snap(at=150, committed=keys(*range(6)), delivered=keys(0, 1, 2)))
    assert plan.retire == "INFEASIBLE"
    assert plan.exclusions == ()


def test_a_round_whose_audit_window_passed_without_jobs_is_marked():
    everyone = keys(*range(6))
    kw = {"committed": everyone, "delivered": everyone}
    last = S.absolute - 1
    assert x.plan_round(snap(at=last, has_leases=False, **kw)).retire == "AUDIT_WINDOW_MISSED"
    assert x.plan_round(snap(at=last - 1, has_leases=False, **kw)).retire is None
    assert x.plan_round(snap(at=last, has_leases=True, **kw)).retire is None


def test_a_missed_audit_window_books_the_blame_without_membership_facts():
    five = keys(0, 1, 2, 3, 4)
    plan = x.plan_round(snap(at=S.absolute - 1, has_leases=False, committed=five, delivered=five))
    assert plan.retire == "AUDIT_WINDOW_MISSED"
    assert [(e.hotkey, e.record) for e in plan.exclusions] == [("h5", False)]


def test_every_exit_the_planner_or_the_lease_expiry_books_has_a_seizure_reason():
    everyone = keys(*range(6))
    commit = x.plan_round(snap(at=30, committed=keys(0, 1, 2, 3))).exclusions
    delta = x.plan_round(
        snap(at=150, committed=everyone, audited=everyone, delivered=keys(0, 1, 2, 3, 4))
    ).exclusions
    booked = {(e.reason, e.stage) for e in commit + delta} | {("NO_SHOW", "leaves")}
    assert booked == set(x.SANCTION)
    assert set(x.SANCTION.values()) <= set(get_args(SeizureReason))


@pytest.mark.parametrize(
    ("previous", "emitted", "span"),
    [
        (10, 12, None),
        (10, 13, (10, 10)),
        (10, 20, (10, 17)),
        (0, 50, None),
    ],
    ids=["on-time", "first-refused-view", "caught-up-late", "no-beacon-yet"],
)
def test_a_push_shows_a_pause_only_when_the_master_refused_its_callers(previous, emitted, span):
    assert x.pause_span(previous, emitted) == span


@pytest.mark.parametrize(
    ("spans", "is_served"),
    [
        ([], True),
        ([(95, 99)], True),
        ([(95, 100)], False),
        ([(129, 140)], False),
        ([(130, 140)], True),
        ([(10, 20), (125, 126)], False),
    ],
    ids=[
        "no-pause",
        "ends-before",
        "ends-on-the-first-round",
        "starts-in",
        "starts-after",
        "one-of-two",
    ],
)
def test_a_window_is_served_unless_a_refused_view_falls_inside_it(spans, is_served):
    assert x.served(spans, 100, 130) is is_served
