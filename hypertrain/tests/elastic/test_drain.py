"""Drain on notice: flush only when it fits before the deadline, then always leave, once."""

import pytest

from hypertrain.data.store import StoreError
from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.drain import DrainHandler
from hypertrain.elastic.notice import FakeNoticeSource, Interruption

FLUSH_MS, MARGIN_MS = 5_000, 5_000


class Host:
    """Records what the handler asks for; a flush takes FLUSH_MS of the clock."""

    def __init__(self, clock: FakeClock) -> None:
        self.clock = clock
        self.events: list[tuple[str, int]] = []
        self.fail_flush = False
        self.fail_leaves = 0

    def flush(self) -> None:
        self.clock.advance(FLUSH_MS)
        if self.fail_flush:
            raise StoreError("upload failed")
        self.events.append(("flush", self.clock.now_ms()))

    def leave(self, notice: Interruption) -> None:
        if self.fail_leaves:
            self.fail_leaves -= 1
            raise ConnectionError("coordinator unreachable")
        assert self.clock.now_ms() <= notice.deadline_ms
        self.events.append(("leave", self.clock.now_ms()))


def handler(deadline_in_ms: int) -> tuple[DrainHandler, Host]:
    clock = FakeClock(10_000)
    notices = FakeNoticeSource(clock)
    notices.schedule(10_000, 10_000 + deadline_in_ms)
    host = Host(clock)
    drain = DrainHandler(
        clock=clock,
        source=notices,
        flush=host.flush,
        leave=host.leave,
        flush_ms=FLUSH_MS,
        margin_ms=MARGIN_MS,
    )
    return drain, host


def test_a_notice_with_time_left_flushes_then_leaves_before_the_deadline() -> None:
    drain, host = handler(120_000)
    done = drain.check()
    assert done is not None and done.flushed
    assert host.events == [("flush", 15_000), ("leave", 15_000)]


def test_a_late_notice_leaves_at_once_without_flushing() -> None:
    drain, host = handler(FLUSH_MS + MARGIN_MS - 1)
    done = drain.check()
    assert done is not None and not done.flushed
    assert host.events == [("leave", 10_000)]


def test_the_budget_follows_the_last_measured_flush() -> None:
    drain, host = handler(30_000)
    drain.observe_flush(40_000)
    drain.check()
    assert host.events == [("leave", 10_000)]


def test_a_failed_flush_still_leaves_and_raises() -> None:
    drain, host = handler(120_000)
    host.fail_flush = True
    with pytest.raises(StoreError):
        drain.check()
    assert host.events == [("leave", 15_000)]
    assert drain.drained is not None and not drain.drained.flushed


def test_a_failed_leave_is_retried_by_the_next_check_and_the_drain_runs_once() -> None:
    drain, host = handler(120_000)
    host.fail_leaves = 1
    with pytest.raises(ConnectionError):
        drain.check()
    assert drain.drained is None
    first = drain.check()
    assert first is not None and drain.check() is first
    assert first.flushed
    assert host.events == [("flush", 15_000), ("leave", 15_000)]
