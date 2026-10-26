"""Drain on notice: flush a checkpoint if it fits before the deadline, then leave, once.

The miner loop calls check() between inner steps, so detection waits at most one poll interval
plus one step. The flush runs only when the time left covers the last measured flush plus a
margin; otherwise the host leaves at once and the shard resumes from its last accepted link.
The leave is sent even when the flush fails, and the flush error still propagates. A drain is
recorded only once the leave went out, so a failed leave is retried by the next check(); the
retried flush rewrites the same content-addressed objects and its commit is idempotent.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

from hypertrain.elastic.clock import Clock
from hypertrain.elastic.notice import Interruption, NoticeSource


@dataclass(frozen=True, slots=True)
class Drain:
    notice: Interruption
    flushed: bool
    started_ms: int
    finished_ms: int


class DrainHandler:
    def __init__(
        self,
        *,
        clock: Clock,
        source: NoticeSource,
        flush: Callable[[], object],
        leave: Callable[[Interruption], object],
        flush_ms: int,
        margin_ms: int,
    ) -> None:
        if flush_ms < 0 or margin_ms < 0:
            raise ValueError("flush_ms and margin_ms must be >= 0")
        self.clock = clock
        self.source = source
        self._flush = flush
        self._leave = leave
        self.flush_ms = flush_ms
        self.margin_ms = margin_ms
        self.drained: Drain | None = None

    def observe_flush(self, ms: int) -> None:
        """Record how long the last checkpoint write took; the next decision uses it."""
        self.flush_ms = ms

    def check(self) -> Drain | None:
        if self.drained is not None:
            return self.drained
        notice = self.source.poll()
        if notice is None:
            return None
        started = self.clock.now_ms()
        fits = notice.deadline_ms - started >= self.flush_ms + self.margin_ms
        flushed = False
        try:
            if fits:
                self._flush()
                flushed = True
        finally:
            self._leave(notice)
            self.drained = Drain(notice, flushed, started, self.clock.now_ms())
        return self.drained
