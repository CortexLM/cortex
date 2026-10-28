"""Virtual-time model of the real `hypertrain.beacon.push.Pusher` against a held store lock.

A remote reference holds the lock of the challenge store that hosts the run
(`challenge/store.py` `trial_reference_v2`). This drives the unmodified `Pusher.run` through such an
outage on a clock that only moves when the pusher or the model says so, so a lock of minutes runs
in milliseconds and the result does not depend on the machine. `simulate` runs the production
`Pusher` (head first) unless told otherwise; `AscendingPusher` is the one it replaced.

Two layouts, because the challenge has two (`ChallengeStore._children`):

- `root`: the run lives in the root store. `GET /v1/beacon/latest` and `POST /v1/admin/beacon`
  both need the lock the reference holds.
- `child`: the run was created while the root already hosted one, so it lives in a child store with
  its own lock (S13's run did). The root answers `latest` at once and inserts a pushed round at
  once, then `push_beacon` hands the round to the child (the fan-out at the end of `push_beacon`),
  which waits for its lock. The pusher sees the root's rounds; the gate and the seeds read the
  child's table.

The constants are the code's own, the costs are S7 / S13 measurements on s100 (docs/adr/0012):

- the drand quicknet period is 3 s and the freshness gate refuses at `now < emitted - 2`
  (`store.py` `_snapshot_current_v2`);
- the pusher's HTTP client times out after 10 s (`beacon/__main__.py`);
- `push_beacon` verifies the round's BLS signature before it takes the lock, and a timed-out request
  is still applied by the service once the lock is free (checked with uvicorn and a thread pool);
- a round costs the pusher a relay fetch plus a BLS verify (about 1.2 s) and the service another
  verify (about 1.2 s; the child's second verify of the same round is a cache hit, because
  `drand._pairing_ok` is an `lru_cache`). The sum is one S7 measurement and the least certain
  number here: recovery after a lock is very sensitive to it (ADR 0012, "Sensitivity").
"""

from __future__ import annotations

import bisect
import contextlib
import io
import itertools
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Literal
from unittest import mock

import httpx

import hypertrain.beacon.push as push

PERIOD = 3.0
LAG_BOUND = 2
TIMEOUT = 10.0
STEP = 0.1

Layout = Literal["root", "child"]


@dataclass(frozen=True)
class Costs:
    pusher: float = 1.2  # relay fetch and BLS verify in the pusher, per round
    service: float = 1.2  # BLS verify in the service, before the lock


MEASURED = Costs()


@dataclass(frozen=True)
class Outcome:
    exit_code: int | None  # 0: still running at the horizon, 1: gave up (--max-failures)
    timeouts: int  # requests the pusher abandoned after TIMEOUT
    holes: int  # rounds between the first and the last one the store holds that it never got
    gate_closed_after_lock: float  # seconds the freshness gate refused after the lock was freed
    final_lag: int


class _Round:
    def __init__(self, number: int) -> None:
        self.round, self.signature, self.randomness = number, "aa", "bb"


class _Response:
    def __init__(self, body: dict[str, Any]) -> None:
        self.status_code, self.is_success, self.text, self._body = 200, True, "", body

    def json(self) -> dict[str, Any]:
        return self._body


class _Interrupt(KeyboardInterrupt):
    """Raised by the model's sleep once the horizon is reached: Pusher.run returns 0 on it."""


class World:
    """The challenge service and the relays, on one virtual clock."""

    def __init__(
        self, lock: tuple[float, float] | None, costs: Costs, horizon: float, layout: Layout
    ) -> None:
        self.lock, self.costs, self.horizon, self.layout = lock, costs, horizon, layout
        self.now = 0.0
        self.timeouts = 0
        first = (0.0, self.emitted(0.0))
        self.root: list[tuple[float, int]] = [first]  # (time applied, round) in the root store
        # the store the run lives in: the root itself, or its child
        self.hosting = self.root if layout == "root" else [first]

    @staticmethod
    def emitted(t: float) -> int:
        return int(t // PERIOD) + 1

    def _free(self, t: float) -> float:
        return self.lock[1] if self.lock and self.lock[0] <= t < self.lock[1] else t

    def _wait_for_lock(self, started: float, ready: float) -> float:
        """When a request that needs the lock from `ready` on is served; a timeout if too late."""
        served = self._free(ready)
        if served - started > TIMEOUT:
            self.now = started + TIMEOUT
            self.timeouts += 1
            raise httpx.ReadTimeout("store lock held")
        return served

    def known(self, t: float) -> int:
        """The highest round the root holds at `t` (what GET /v1/beacon/latest reports)."""
        return max((number for when, number in self.root if when <= t), default=0)

    # the httpx.Client the pusher talks to
    def get(self, path: str) -> _Response:
        assert path == "/v1/beacon/latest"
        started = self.now
        # the root answers at once when the lock held is a child's
        served = self._wait_for_lock(started, started) if self.layout == "root" else started
        self.now = served + 0.01
        return _Response({"round": self.known(self.now)})

    def post(self, path: str, json: dict[str, Any], headers: dict[str, str]) -> _Response:
        assert path == "/v1/admin/beacon"
        started = self.now
        verified = started + self.costs.service
        applied = self._free(verified)  # when the store that hosts the run gets the round
        if self.layout == "root":
            self.root.append((applied, json["round"]))  # applied even if the client gave up
        else:
            self.root.append((verified, json["round"]))
            self.hosting.append((applied, json["round"]))
        self.now = self._wait_for_lock(started, verified) + 0.01
        return _Response({"round": json["round"]})

    def sleep(self, seconds: float) -> None:
        self.now += seconds
        if self.now >= self.horizon:
            raise _Interrupt

    def timeline(self) -> Callable[[float], int]:
        """Highest round the hosting store holds at a time: sort once, then bisect."""
        events = sorted(self.hosting)
        times = [when for when, _ in events]
        highest = list(itertools.accumulate((number for _, number in events), max))

        def at(t: float) -> int:
            index = bisect.bisect_right(times, t)
            return highest[index - 1] if index else 0

        return at


class Relays:
    def __init__(self, world: World) -> None:
        self.world = world

    def latest(self) -> _Round:
        self.world.now += self.world.costs.pusher
        return _Round(self.world.emitted(self.world.now))

    def get(self, number: int) -> _Round:
        self.world.now += self.world.costs.pusher
        return _Round(number)


class AscendingPusher(push.Pusher):
    """Model only: the pusher before ADR 0012 section 3, all missing rounds per tick, oldest first.

    Kept so that the S13 reproductions and the table of what that cadence cost still run against
    the code that produced them. The production `push.Pusher` pushes the head first.
    """

    def tick(self) -> dict[str, Any]:
        latest = self.relays.latest()
        known = self._known()
        pushed: list[int] = []
        for rnd in push.plan(known, latest.round, self.catch_up):
            br = latest if rnd == latest.round else self.relays.get(rnd)
            r = self.service.post(
                "/v1/admin/beacon",
                json={"round": br.round, "signature": br.signature, "randomness": br.randomness},
                headers={"Authorization": f"Bearer {self._token}"},
            )
            self._check(r, f"round {rnd}")
            pushed.append(rnd)
        return {"pushed": pushed, "latest": latest.round, "known": known}


def simulate(
    lock_seconds: float,
    *,
    interval: float,
    catch_up: int,
    max_failures: int,
    costs: Costs = MEASURED,
    layout: Layout = "root",
    offset: float = 0.0,
    pusher: type[push.Pusher] = push.Pusher,
    warmup: float = 60.0,
) -> Outcome:
    """Run the pusher through a lock of `lock_seconds` that starts `offset` s after the warm-up."""
    start = warmup + offset
    lock = (start, start + lock_seconds) if lock_seconds else None
    end = start + lock_seconds + 600 + 4 * lock_seconds
    world = World(lock, costs, end, layout)
    running = pusher(world, Relays(world), "token", catch_up)  # type: ignore[arg-type]
    quiet = io.StringIO()
    with (
        mock.patch.object(push.time, "sleep", world.sleep),
        contextlib.redirect_stdout(quiet),
        contextlib.redirect_stderr(quiet),
    ):
        code = running.run(interval, False, max_failures)
    stop = min(world.now, end)
    lock_end = lock[1] if lock else 0.0
    known = world.timeline()
    closed, at = 0.0, lock_end
    while at < stop:
        if known(at) < world.emitted(at) - LAG_BOUND:
            closed += STEP
        at += STEP
    held = {number for when, number in world.hosting if when <= stop}
    holes = sum(1 for number in range(min(held), max(held) + 1) if number not in held)
    return Outcome(code, world.timeouts, holes, round(closed, 1), world.emitted(stop) - max(held))
