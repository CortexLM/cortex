"""Coordinator of one elastic run: membership, shard holders, checkpoint heads and credit.

Every entry point first lapses expired leases, so no decision rests on a dead lease, and ends
the lease of any holder that made no accepted link within `progress_ms` of being given a shard
or of its last link: a host that heartbeats but cannot upload never stalls a round. A shard has
at most one holder, a (member, token) pair, and only that holder extends the shard's head, by a
link whose parent is the head (compare-and-set). Accepted links are never undone. Credit is the
number of steps each accepted link adds, paid to the owner of the member that produced it.
Leaving, lapsing, stalling or being replaced costs a member nothing it had accepted; only a link
that fails the audit is penalised, and its member is revoked.

The coordinator is single-threaded state: production serialises calls (the master's store or a
Durable Object, ADR 0003) and persists the ordered calls, which replay to the same state.
"""

from __future__ import annotations

import struct
from collections import Counter
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from enum import StrEnum

from hypertrain.elastic.assignment import plan
from hypertrain.elastic.checkpoint import CheckpointManager, CheckpointManifest, WorkUnit
from hypertrain.elastic.clock import Clock
from hypertrain.elastic.membership import Lease, Member, Membership
from hypertrain.elastic.notice import NoticeError, SignedLeave, verify_leave
from hypertrain.elastic.store import CheckpointStore

Audit = Callable[[WorkUnit, CheckpointManifest], bool]


class CoordinatorError(Exception):
    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code


class Exit(StrEnum):
    NOTICE = "notice"  # signed leave notice under a live lease
    LAPSE = "lapse"  # lease ran out: hard kill, host loss or partition
    STALLED = "stalled"  # no accepted link within progress_ms; its lease is ended
    REPLACED = "replaced"  # the same member id joined from another host
    REVOKED = "revoked"  # one of its links failed the audit


@dataclass(frozen=True, slots=True)
class Departure:
    member: str
    owner: str
    token: int
    kind: Exit
    at_ms: int
    released: tuple[int, ...]


@dataclass(frozen=True, slots=True)
class Assignment:
    unit: WorkUnit
    head: str | None
    step: int


@dataclass(frozen=True, slots=True)
class Grant:
    lease: Lease
    assignments: tuple[Assignment, ...]


@dataclass(frozen=True, slots=True)
class Accepted:
    shard: int
    key: str
    step: int
    duplicate: bool


@dataclass(frozen=True, slots=True)
class Link:
    key: str
    member: str
    owner: str
    steps: int


@dataclass(frozen=True, slots=True)
class Holder:
    member: str
    token: int


@dataclass(slots=True)
class _Shard:
    unit: WorkUnit
    holder: Holder | None = None
    head: str | None = None
    step: int = 0
    links: list[Link] = field(default_factory=list)
    accepted: dict[str, int] = field(default_factory=dict)
    since_ms: int = 0  # holder assigned or last link accepted

    @property
    def done(self) -> bool:
        return self.step == self.unit.steps


@dataclass(frozen=True, slots=True)
class Settlement:
    """`credits` and `penalties` are per owner; `heads` are the final links per shard."""

    round: int
    complete: bool
    heads: dict[int, str | None]
    credits: dict[str, int]
    penalties: dict[str, int]
    departures: tuple[Departure, ...]


class Coordinator:
    def __init__(
        self,
        *,
        run_id: str,
        clock: Clock,
        store: CheckpointStore,
        ttl_ms: int,
        progress_ms: int,
        seed: bytes,
        capacity: int = 1,
        audit: Audit | None = None,
    ) -> None:
        if capacity < 1:
            raise ValueError("capacity must be >= 1")
        if progress_ms <= 0:
            raise ValueError("progress_ms must be > 0")
        self.progress_ms = progress_ms
        self.run_id = run_id
        self.clock = clock
        self.membership = Membership(clock, ttl_ms)
        self.checkpoints = CheckpointManager(store)
        self.seed = seed
        self.capacity = capacity
        self.audit = audit
        self.round = -1
        self._shards: dict[int, _Shard] = {}
        self._departures: list[Departure] = []
        self._penalties: Counter[str] = Counter()
        self._left: dict[int, tuple[tuple[str, str], Departure]] = {}

    # ---- rounds ----

    def open_round(self, units: Sequence[WorkUnit]) -> None:
        if not units:
            raise ValueError("a round needs at least one unit")
        if any(not s.done for s in self._shards.values()):
            raise CoordinatorError("ROUND_OPEN", f"round {self.round} is not complete")
        rounds = {u.round for u in units}
        if len(rounds) != 1 or rounds.pop() <= self.round:
            raise ValueError("units must share one round after the current one")
        if {u.run_id for u in units} != {self.run_id}:
            raise ValueError("units belong to another run")
        if len({u.shard for u in units}) != len(units):
            raise ValueError("duplicate shard")
        self.round = units[0].round
        self._shards = {u.shard: _Shard(u) for u in sorted(units, key=lambda u: u.shard)}
        self._departures = []
        self._penalties = Counter()
        self._sweep()
        self._assign()

    # ---- members ----

    def join(self, member: str, owner: str, signer: str) -> Grant:
        self._sweep()
        lease, replaced = self.membership.join(member, owner, signer)
        if replaced is not None:
            for s in self._held(replaced.token):
                s.holder, s.since_ms = Holder(member, lease.token), self.clock.now_ms()
            self._record(member, owner, replaced.token, Exit.REPLACED, ())
        self._assign()
        return self._grant(lease)

    def heartbeat(self, member: str, token: int) -> Grant:
        self._sweep()
        return self._grant(self.membership.renew(member, token))

    def leave(self, signed: SignedLeave) -> Departure:
        """Release the member's shards now. A leave already accepted under the same lease is
        answered again with its departure, so a retry after a lost answer is safe."""
        self._sweep()
        notice = verify_leave(signed)
        if notice.run_id != self.run_id:
            raise NoticeError(f"leave notice for run {notice.run_id}")
        done = self._left.get(notice.lease)
        if done is not None and done[0] == (notice.member, notice.signer):
            return done[1]
        m = self.membership.check(notice.member, notice.lease)
        if notice.signer != m.signer:
            raise NoticeError("leave notice signed by a key the member did not register")
        self.membership.leave(m.id, m.lease.token)
        departure = self._depart(m, Exit.NOTICE)
        self._left[notice.lease] = ((m.id, m.signer), departure)
        return departure

    def tick(self) -> list[Departure]:
        return self._sweep()

    # ---- work ----

    def commit(self, member: str, token: int, shard: int, key: str) -> Accepted:
        """Accept `key` as the new head of `shard`; a link accepted before is acknowledged
        again without new credit, so a retry after a lost answer is safe."""
        self._sweep()
        m = self.membership.check(member, token)
        s = self._shards.get(shard)
        if s is None:
            raise CoordinatorError("UNKNOWN_SHARD", str(shard))
        if key in s.accepted:
            return Accepted(shard, key, s.accepted[key], True)
        if s.holder != Holder(member, token):
            raise CoordinatorError("NOT_HOLDER", f"shard {shard}")
        manifest = self.checkpoints.check_link(s.unit, s.head, key)
        if manifest.producer != member or manifest.lease != token:
            raise CoordinatorError("WRONG_PRODUCER", key)
        if self.audit is not None and not self.audit(s.unit, manifest):
            self._penalties[m.owner] += 1
            self.membership.revoke(member)
            self._depart(m, Exit.REVOKED)
            raise CoordinatorError("AUDIT_FAILED", key)
        s.links.append(Link(key, member, m.owner, manifest.step - s.step))
        s.accepted[key] = manifest.step
        s.head, s.step, s.since_ms = key, manifest.step, self.clock.now_ms()
        if s.done:
            s.holder = None
            self._assign()
        return Accepted(shard, key, s.step, False)

    def settle(self) -> Settlement:
        credits: Counter[str] = Counter()
        for s in self._shards.values():
            for link in s.links:
                credits[link.owner] += link.steps
        return Settlement(
            round=self.round,
            complete=bool(self._shards) and all(s.done for s in self._shards.values()),
            heads={shard: s.head for shard, s in self._shards.items()},
            credits=dict(credits),
            penalties=dict(self._penalties),
            departures=tuple(self._departures),
        )

    def links(self, shard: int) -> tuple[Link, ...]:
        return tuple(self._shards[shard].links)

    def holder(self, shard: int) -> Holder | None:
        return self._shards[shard].holder

    # ---- internals ----

    def _held(self, token: int) -> list[_Shard]:
        return [
            s for s in self._shards.values() if s.holder is not None and s.holder.token == token
        ]

    def _grant(self, lease: Lease) -> Grant:
        held = self._held(lease.token)
        return Grant(lease, tuple(Assignment(s.unit, s.head, s.step) for s in held))

    def _record(
        self, member: str, owner: str, token: int, kind: Exit, released: tuple[int, ...]
    ) -> Departure:
        d = Departure(member, owner, token, kind, self.clock.now_ms(), released)
        self._departures.append(d)
        return d

    def _depart(self, m: Member, kind: Exit) -> Departure:
        released = self._held(m.lease.token)
        for s in released:
            s.holder = None
        d = self._record(m.id, m.owner, m.lease.token, kind, tuple(s.unit.shard for s in released))
        self._assign()
        return d

    def _sweep(self) -> list[Departure]:
        out = [self._depart(m, Exit.LAPSE) for m in self.membership.expire()]
        now = self.clock.now_ms()
        late = {
            s.holder.member
            for s in self._shards.values()
            if s.holder is not None and now - s.since_ms >= self.progress_ms
        }
        out += [
            self._depart(self.membership.lapse(member), Exit.STALLED) for member in sorted(late)
        ]
        return out

    def _assign(self) -> None:
        pending = [shard for shard, s in self._shards.items() if s.holder is None and not s.done]
        if not pending:
            return
        load = Counter(s.holder.token for s in self._shards.values() if s.holder is not None)
        live = {m.id: m.lease.token for m in self.membership.active()}
        free = {member: self.capacity - load[token] for member, token in live.items()}
        seed = self.seed + struct.pack(">Q", self.round)
        now = self.clock.now_ms()
        for shard, member in plan(seed, pending, free).items():
            self._shards[shard].holder, self._shards[shard].since_ms = (
                Holder(member, live[member]),
                now,
            )
