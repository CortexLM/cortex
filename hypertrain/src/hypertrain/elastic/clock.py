"""Time source of the elastic core: integer milliseconds, injectable, never a wall-clock date.

Leases are judged by the coordinator's own clock only, so clock skew between hosts never
decides an expiry. The simulator drives every component with one FakeClock.
"""

from __future__ import annotations

import time
from typing import Protocol


class Clock(Protocol):
    def now_ms(self) -> int: ...


class MonotonicClock:
    def now_ms(self) -> int:
        return time.monotonic_ns() // 1_000_000


class FakeClock:
    """A clock that moves only when told to, and never backwards."""

    def __init__(self, start_ms: int = 0) -> None:
        if start_ms < 0:
            raise ValueError("start_ms must be >= 0")
        self._now = start_ms

    def now_ms(self) -> int:
        return self._now

    def advance(self, ms: int) -> None:
        if ms < 0:
            raise ValueError("a clock never moves backwards")
        self._now += ms

    def set(self, now_ms: int) -> None:
        if now_ms < self._now:
            raise ValueError("a clock never moves backwards")
        self._now = now_ms
