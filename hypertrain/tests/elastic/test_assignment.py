"""Shard planning: deterministic, each pending shard to at most one member, within capacity."""

from collections import Counter

from hypothesis import given, settings
from hypothesis import strategies as st

from hypertrain.elastic.assignment import plan, score

members = st.dictionaries(st.text("abcdef", min_size=1, max_size=3), st.integers(0, 3), max_size=8)


@settings(max_examples=300, deadline=None)
@given(seed=st.binary(max_size=16), pending=st.lists(st.integers(0, 40), max_size=30), free=members)
def test_plan_gives_each_pending_shard_to_one_member_within_its_capacity(
    seed: bytes, pending: list[int], free: dict[str, int]
) -> None:
    out = plan(seed, pending, free)
    assert set(out) <= set(pending)
    load = Counter(out.values())
    assert all(load[m] <= free[m] for m in load)
    assert len(out) == min(len(set(pending)), sum(n for n in free.values() if n > 0))
    assert plan(seed, sorted(pending, reverse=True), dict(reversed(list(free.items())))) == out


def test_the_score_is_the_documented_formula() -> None:
    # sha256("ht-elastic-assign-v1|" || seed || shard as 8 big-endian bytes || member): an
    # auditor recomputes the plan from this, so the digests are pinned, not derived
    assert score(b"seed", 7, "alice").hex() == (
        "449f5ab7960986fcc770c38c1643941bb464e34119f28dd74492eaee63edab77"
    )
    assert score(b"seed", 7, "bob").hex() == (
        "218bd3ae5fa7d10829a7d6ae17314339fbf371434ce00455ba274bf7b484330b"
    )
    assert plan(b"seed", [7], {"alice": 1, "bob": 1}) == {7: "alice"}  # the highest score wins


def test_every_member_gets_a_share_of_the_shards() -> None:
    members = {f"m{i}": 64 for i in range(4)}  # room for all 64 shards on any one of them
    load = Counter(plan(b"seed", range(64), members).values())
    assert set(load) == set(members)
    assert max(load.values()) <= 32  # twice the mean of 16


def test_a_departure_moves_only_the_shards_of_the_member_that_left() -> None:
    room = {m: 64 for m in "abcd"}
    before = plan(b"seed", range(64), room)
    after = plan(b"seed", range(64), {m: n for m, n in room.items() if m != "d"})
    assert set(before.values()) == set(room)  # d held shards, so the check is not empty
    assert {s: m for s, m in before.items() if m != "d"}.items() <= after.items()
