"""Shard planning: deterministic, each pending shard to at most one member, within capacity."""

from collections import Counter

from hypothesis import given, settings
from hypothesis import strategies as st

from hypertrain.elastic.assignment import plan

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
