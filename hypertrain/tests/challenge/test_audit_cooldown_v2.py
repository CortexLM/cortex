"""The auditor cooldown schedule is pure beacon-round arithmetic."""

from __future__ import annotations

from hypertrain.challenge.audit_cooldown_v2 import cooling_until, next_record, others_free


def record(previous, now, *, free=True):
    return next_record(
        previous,
        job_id="aa" * 32,
        cause="AUDITOR_SILENT",
        now=now,
        lease_rounds=100,
        absolute_rounds=200,
        free=free,
    )


def test_the_cooldown_is_one_lease_then_two_and_stays_at_two():
    first = record(None, 1000)
    second = record(first, first["until"] + 10)
    third = record(second, second["until"] + 10)
    assert [(r["strikes"], r["until"] - r["since"]) for r in (first, second, third)] == [
        (1, 100),
        (2, 200),
        (3, 200),
    ]


def test_a_strike_is_forgotten_after_one_quiet_lease():
    first = record(None, 1000)
    assert record(first, first["until"] + 99)["strikes"] == 2
    assert record(first, first["until"] + 100)["strikes"] == 1


def test_the_cooldown_ends_exactly_at_until():
    first = record(None, 1000)
    assert cooling_until(first, first["until"] - 1) == first["until"]
    assert cooling_until(first, first["until"]) is None
    assert cooling_until(None, 5) is None


def test_the_last_free_auditor_is_recorded_but_not_refused():
    last = record(None, 1000, free=False)
    assert (last["strikes"], cooling_until(last, 1000)) == (1, None)
    assert record(last, 1001)["strikes"] == 2  # the strike still counts when others are back


def test_a_configured_auditor_that_never_polls_still_counts_as_free():
    auditors = ["a", "b", "ghost"]
    a_cooling = {"a": record(None, 1000)}
    assert others_free(auditors, "b", a_cooling, 1000)  # the ghost: b is refused as well
    assert not others_free(["a", "b"], "b", a_cooling, 1000)  # two configured: b is never refused
    assert others_free(["a", "b"], "b", a_cooling, a_cooling["a"]["until"])
