import hashlib

import pytest

from hypertrain.data.assignment import (
    FeistelPRP,
    ReconcileError,
    assign_key,
    assign_round,
    reconcile,
)

RUN = hashlib.sha256(b"run").digest()
SIG = b"\x11" * 48


def test_prp_bijection_exhaustive_4096() -> None:
    p = FeistelPRP(b"k", 4096)
    assert sorted(p(i) for i in range(4096)) == list(range(4096))
    assert [p(i) for i in range(16)] != list(range(16))


@pytest.mark.parametrize("n", [1, 2, 3, 5, 1000, 4097])
def test_prp_bijection_odd_sizes(n: int) -> None:
    p = FeistelPRP(b"k2", n)
    assert sorted(p(i) for i in range(n)) == list(range(n))


def test_prp_rejects_out_of_range() -> None:
    with pytest.raises(IndexError):
        FeistelPRP(b"k", 10)(10)
    with pytest.raises(ValueError):
        FeistelPRP(b"k", 0)


def test_slices_disjoint_64_slots_50_rounds() -> None:
    slots, batch, n = 64, 8, 64 * 8 * 50
    seen: set[int] = set()
    for w in range(50):
        a = assign_round(
            RUN,
            w,
            SIG + bytes([w]),
            n_samples=n,
            n_slots=slots,
            batch=batch,
            base_w=w * slots * batch,
        )
        for s in a.slices:
            assert len(s) == batch
            assert seen.isdisjoint(s)
            seen.update(s)
    assert seen == set(range(n))


def test_assignment_changes_with_drand_sig() -> None:
    kw = dict(n_samples=10_000, n_slots=4, batch=16, base_w=0)
    a = assign_round(RUN, 3, SIG, **kw)
    b = assign_round(RUN, 3, SIG[:-1] + b"\x12", **kw)
    assert a.slices != b.slices
    assert set().union(*a.slices) == set().union(*b.slices)  # same round block, new mapping
    assert assign_round(RUN, 3, SIG, **kw) == a
    assert assign_key(RUN, 3, SIG) != assign_key(RUN, 4, SIG)


def test_round_cannot_straddle_epoch() -> None:
    with pytest.raises(ValueError):
        assign_round(RUN, 0, SIG, n_samples=100, n_slots=2, batch=10, base_w=90)


def _a() -> object:
    return assign_round(RUN, 1, SIG, n_samples=1000, n_slots=3, batch=5, base_w=15)


def test_reconcile_accepts_honest() -> None:
    a = assign_round(RUN, 1, SIG, n_samples=1000, n_slots=3, batch=5, base_w=15)
    got = reconcile(a, {i: list(s) for i, s in enumerate(a.slices)}, generation=0)
    assert len(got) == 15


def test_reconcile_stale_generation() -> None:
    a = assign_round(RUN, 1, SIG, n_samples=1000, n_slots=3, batch=5, base_w=15)
    with pytest.raises(ReconcileError) as e:
        reconcile(a, {0: list(a.slices[0])}, generation=1)
    assert e.value.code == "STALE_GENERATION"


def test_reconcile_duplicate_exposure() -> None:
    a = assign_round(RUN, 1, SIG, n_samples=1000, n_slots=3, batch=5, base_w=15)
    dup = list(a.slices[0])
    dup[1] = dup[0]
    with pytest.raises(ReconcileError) as e:
        reconcile(a, {0: dup}, generation=0)
    assert e.value.code == "DUPLICATE_EXPOSURE"
    first = reconcile(a, {0: list(a.slices[0])}, generation=0)
    with pytest.raises(ReconcileError) as e:
        reconcile(a, {0: list(a.slices[0])}, generation=0, consumed=first)
    assert e.value.code == "DUPLICATE_EXPOSURE"


def test_reconcile_shard_overlap() -> None:
    a = assign_round(RUN, 1, SIG, n_samples=1000, n_slots=3, batch=5, base_w=15)
    stolen = list(a.slices[0][:4]) + [a.slices[1][0]]
    with pytest.raises(ReconcileError) as e:
        reconcile(a, {0: stolen}, generation=0)
    assert e.value.code == "SHARD_OVERLAP"


def test_reconcile_unassigned_and_count() -> None:
    a = assign_round(RUN, 1, SIG, n_samples=1000, n_slots=3, batch=5, base_w=15)
    outside = next(i for i in range(1000) if all(i not in s for s in a.slices))
    with pytest.raises(ReconcileError) as e:
        reconcile(a, {0: [*a.slices[0][:4], outside]}, generation=0)
    assert e.value.code == "ASSIGNMENT_VIOLATION"
    with pytest.raises(ReconcileError) as e:
        reconcile(a, {0: list(a.slices[0][:4])}, generation=0)
    assert e.value.code == "EXPOSURE_COUNT"
