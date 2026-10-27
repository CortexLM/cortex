"""One lock order for the dispute long poll and the transactions that wake it (Lens DSP-03 / F5)."""

from __future__ import annotations

import threading
from pathlib import Path

import pytest

from .test_disputes_v2 import fixtures, service

BOUND = 10  # seconds: far above what a free lock costs, far below a hung suite


class StoreLock:
    """The store lock as `ChallengeStore._lock` is for a hosted escrow, plus a flag raised when a
    thread other than the mutator starts to wait for it."""

    def __init__(self, inner, mutator: list[int]) -> None:
        self.inner, self.mutator = inner, mutator
        self.contended = threading.Event()

    def __enter__(self):
        if threading.get_ident() not in self.mutator:
            self.contended.set()
        return self.inner.__enter__()

    def __exit__(self, *exc):
        return self.inner.__exit__(*exc)


def test_a_mutator_that_notifies_under_the_store_lock_does_not_deadlock_a_parked_poller(
    tmp_path: Path,
) -> None:
    """A store transaction in flight wakes its waiters before it commits, which is what every
    `DisputesV2` mutator does when it is hosted. The long poll must be in the middle of asking
    for the store lock at that moment: it used to hold the condition while it did."""
    d, *_ = service(tmp_path)
    mutator_ids: list[int] = []
    lock = StoreLock(d.escrow.mutex, mutator_ids)
    d.escrow.mutex = lock
    polled: list[object] = []
    poller = threading.Thread(
        target=lambda: polled.extend(d.events(fixtures().HOT.ss58, 0, timeout=1)), daemon=True
    )

    def transaction() -> None:
        mutator_ids.append(threading.get_ident())
        with d.escrow.mutex:
            poller.start()
            assert lock.contended.wait(BOUND), "the poller never asked for the store lock"
            d.notify()

    mutator = threading.Thread(target=transaction, daemon=True)
    mutator.start()
    mutator.join(BOUND)
    poller.join(BOUND)

    assert not mutator.is_alive(), "the mutator waits for the condition the poller holds"
    assert not poller.is_alive(), "the poller waits for the store lock the mutator holds"
    assert polled == []


def test_a_poller_parked_on_the_condition_is_woken_by_a_mutator_holding_the_store_lock(
    tmp_path: Path,
) -> None:
    d, _, _, _, raw = service(tmp_path)
    polled: list[object] = []
    parked = threading.Event()

    def poll() -> None:
        parked.set()
        polled.extend(d.events(fixtures().HOT.ss58, 0, timeout=30))

    poller = threading.Thread(target=poll, daemon=True)
    poller.start()
    assert parked.wait(BOUND)
    with d.escrow.mutex:  # a hosted `open`: the store transaction is still open when it notifies
        d.open(raw, now=1)
    poller.join(BOUND)

    assert not poller.is_alive() and len(polled) == 1


def test_a_thread_that_holds_the_condition_may_not_ask_for_the_store_lock(tmp_path: Path) -> None:
    d, *_ = service(tmp_path)
    with d.escrow.mutex:
        d.notify()  # the permitted order: store lock, then condition
    with d._leaf(), pytest.raises(AssertionError, match="lock order"), d.escrow.mutex:
        pytest.fail("the inverted order was let through")
