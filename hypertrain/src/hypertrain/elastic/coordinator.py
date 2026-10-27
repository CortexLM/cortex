"""Coordinator of one elastic run: membership, shard holders, checkpoint heads and credit.

Every mutating entry point and settle() first lapse expired leases (open_round refuses a bad call
before it sweeps), so no decision rests on a dead lease, and end the lease of any holder that made
no accepted link within `progress_ms` (counted per member from its last accepted link, or from
when it started holding work; a replacement host restarts that window once per link, not at every
rejoin): a host that heartbeats but cannot upload, or restarts in a loop, never stalls a round.
With capacity above 1 that window is shared by a member's shards, so a member that advances one of
them could keep another for ever while a live member that holds nothing waits; a shard that was
assigned or last advanced `progress_ms` ago is therefore handed to such a member when its holder
holds two or more shards and is not working it (a Move, not a departure; a host works its first
shard first), and a holder with nobody to hand it to keeps it. The holder's window restarts at
each Move, since the coordinator took work from it, so a host that never works its first shard
outlives its window by at most capacity - 1 more. holder() and links() are pure reads and may
show a lapsed holder until the next such call. A shard has at most one holder, a (member,
token) pair, and only that holder extends the shard's head, by a link whose parent is the head
(compare-and-set). Credit is the number of steps each accepted link adds, paid to the owner of the
member that produced it. Leaving, lapsing, stalling or being replaced costs a member nothing it
had accepted; only a link that fails the audit is penalised, and its member is revoked.

No link is credited without an audit: `audit` is required and runs inside commit(). A replay of
a whole link may be too slow for that call, so the audit can be a cheap check and the full replay
a later job; when it finds the link invalid, withdraw() voids the link and every link built on
it, within the round, and the shard resumes from the link before. Otherwise accepted links are
never undone.

Trust: `admit` decides which host key may join as a member (in production, a server key that the
owner's coldkey granted for this run, `protocol.server_keys.SignedGrant`). The caller must
authenticate every heartbeat and commit as signed by that member's registered key; tokens fence,
they are not credentials. The leave is verified here because it is kept as evidence.

The coordinator is single-threaded state: production serialises calls (the master's database,
ADR 0003, which rejects a Durable Object for it). This library persists only the token mark
(`tokens`): a restarted coordinator never reissues a token, so a signed leave or a stale host of
the old process names no live lease, and every host joins again. A log restored from an older
copy would reissue them, so a leave also names the epoch the coordinator drew at its start (ADR
0010): the leave of an earlier start is refused whatever token the log hands out again.
Rebuilding the heads, credits and revocations is the wiring's job.
"""

from __future__ import annotations

import re
import struct
from collections import Counter
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from enum import StrEnum

from pydantic import ValidationError

from hypertrain.elastic.assignment import plan
from hypertrain.elastic.checkpoint import CheckpointManager, CheckpointManifest, WorkUnit
from hypertrain.elastic.clock import Clock
from hypertrain.elastic.membership import Lease, LeaseError, Member, Membership, Status
from hypertrain.elastic.notice import LeaveNotice, NoticeError, SignedLeave, verify_leave
from hypertrain.elastic.store import CheckpointStore
from hypertrain.elastic.tokens import TokenLog
from hypertrain.protocol.keys import KeyError_, decode_hotkey

# An audit returns False only for a verified forgery, and raises when it cannot decide, so that
# nothing changes and nobody is penalised. It runs in the serialised section with `admit` and the
# store reads of check_link: all three must finish well under ttl_ms minus the heartbeat
# interval, or every lease lapses while they run. A slower check belongs to withdraw().
Audit = Callable[[WorkUnit, CheckpointManifest], bool]
Admit = Callable[[str, str, str], bool]  # (member, owner, signer) -> may join
MEMBER_ID = re.compile(r"[A-Za-z0-9_.:-]{1,128}")


class CoordinatorError(Exception):
    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code


class Exit(StrEnum):
    NOTICE = "notice"  # signed leave notice under a live lease
    LAPSE = "lapse"  # lease ran out: hard kill, host loss or partition
    STALLED = "stalled"  # no accepted link within progress_ms; its lease is ended
    REPLACED = "replaced"  # the same member id joined from another host
    REVOKED = "revoked"  # one of its links failed the audit, or was withdrawn after it


@dataclass(frozen=True, slots=True)
class Departure:
    member: str
    owner: str
    token: int
    kind: Exit
    at_ms: int
    released: tuple[int, ...]
    notice: LeaveNotice | None = None  # the verified leave, kept as evidence (NOTICE only)


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
class Move:
    """A shard handed from a live member that held two or more shards to one that held none."""

    shard: int
    from_member: str
    from_token: int
    to_member: str
    at_ms: int


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
    since_ms: int = 0  # when its holder got it, or last advanced it
    linked_ms: int = -1  # when its holder last advanced it; -1 for never

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
    moves: tuple[Move, ...]


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
        admit: Admit,
        tokens: TokenLog,
        audit: Audit,
        capacity: int = 1,
    ) -> None:
        if capacity < 1:
            raise ValueError("capacity must be >= 1")
        if not MEMBER_ID.fullmatch(run_id):
            raise ValueError("run_id must be an Identifier, as every manifest carries it")
        if progress_ms <= 0:
            raise ValueError("progress_ms must be > 0")
        self.progress_ms = progress_ms
        self.admit = admit
        self.run_id = run_id
        self.clock = clock
        self.membership = Membership(clock, ttl_ms, tokens)
        self.checkpoints = CheckpointManager(store)
        self.seed = seed
        self.capacity = capacity
        self.audit = audit
        self.round = -1
        self._shards: dict[int, _Shard] = {}
        self._departures: list[Departure] = []
        self._moves: list[Move] = []
        self._penalties: Counter[str] = Counter()
        # Retry answers, kept for the round they were given in and the round after it, so the
        # memory of a long run stays bounded by two rounds.
        self._left: dict[int, tuple[tuple[str, str], Departure]] = {}
        self._left_before: dict[int, tuple[tuple[str, str], Departure]] = {}
        self._accepted: dict[str, tuple[int, int, str]] = {}  # key -> (shard, step, member)
        self._accepted_before: dict[str, tuple[int, int, str]] = {}
        self._progress: dict[str, tuple[int, bool]] = {}  # member -> (window start, renewed)

    # ---- rounds ----

    def open_round(self, units: Sequence[WorkUnit]) -> Settlement | None:
        """Close the complete current round and open the next one; returns the closed round's
        final settlement, departures up to this call included (None before the first round)."""
        if not units:
            raise ValueError("a round needs at least one unit")
        if any(not s.done for s in self._shards.values()):
            raise CoordinatorError("ROUND_OPEN", f"round {self.round} is not complete")
        for u in units:  # a unit no manifest can bind would block every later round
            try:
                CheckpointManifest(
                    run_id=u.run_id,
                    policy=u.policy,
                    round=u.round,
                    shard=u.shard,
                    base=u.base,
                    parent=None,
                    step=u.steps,
                    state=u.base,
                    producer="open-round",
                    lease=1,
                )
            except ValidationError as e:
                raise ValueError(f"unit {u.shard}: {e.error_count()} invalid fields") from None
            if not self.checkpoints.store.exists(u.base):
                raise ValueError(f"base state {u.base} is not in the store")
        rounds = {u.round for u in units}
        if len(rounds) != 1 or rounds.pop() <= self.round:
            raise ValueError("units must share one round after the current one")
        if {u.run_id for u in units} != {self.run_id}:
            raise ValueError("units belong to another run")
        if len({u.shard for u in units}) != len(units):
            raise ValueError("duplicate shard")
        closed = self.settle() if self._shards else None
        self.round = units[0].round
        self._shards = {u.shard: _Shard(u) for u in sorted(units, key=lambda u: u.shard)}
        self._left_before, self._left = self._left, {}
        self._accepted_before, self._accepted = self._accepted, {}
        self._departures = []
        self._moves = []
        self._penalties = Counter()
        self._sweep()
        self._assign()
        return closed

    # ---- members ----

    def join(self, member: str, owner: str, signer: str) -> Grant:
        """Grant `member` a lease for host key `signer` if `admit` allows it; a live holder of
        the same id is replaced only by an admitted key of the same owner."""
        self._sweep()
        if not MEMBER_ID.fullmatch(member):
            raise LeaseError("BAD_MEMBER", member[:128])
        try:
            decode_hotkey(signer)
        except KeyError_:
            raise LeaseError("BAD_SIGNER", member) from None
        if not self.admit(member, owner, signer):
            raise LeaseError("NOT_ADMITTED", member)
        lease, replaced = self.membership.join(member, owner, signer)
        if replaced is not None:
            for s in self._held(replaced.token):
                s.holder = Holder(member, lease.token)
            window = self._progress.get(member)
            if window is not None and not window[1]:  # one fresh window per link, not per rejoin
                self._progress[member] = (self.clock.now_ms(), True)
            self._record(member, owner, replaced.token, Exit.REPLACED, ())
        self._assign()
        return self._grant(lease)

    def heartbeat(self, member: str, token: int) -> Grant:
        self._sweep()
        return self._grant(self.membership.renew(member, token))

    def leave(self, signed: SignedLeave) -> Departure:
        """Release the member's shards now. A leave already accepted under the same lease is
        answered again with its departure, in its own round and the next, so a retry after a
        lost answer is safe."""
        self._sweep()
        if signed.notice.run_id != self.run_id:  # cheap checks before the signature
            raise NoticeError(f"leave notice for run {signed.notice.run_id}")
        if signed.notice.epoch != self.membership.epoch:
            raise LeaseError("STALE_EPOCH", signed.notice.member)
        notice = verify_leave(signed)
        done = self._left.get(notice.lease) or self._left_before.get(notice.lease)
        if done is not None and done[0] == (notice.member, notice.signer):
            return done[1]
        if len(notice.heads) > len(self._shards):  # the notice is kept as evidence
            raise NoticeError("leave notice names more heads than the round has shards")
        m = self.membership.check(notice.member, notice.lease)
        if notice.signer != m.signer:
            raise NoticeError("leave notice signed by a key the member did not register")
        self.membership.leave(m.id, m.lease.token)
        departure = self._depart(m, Exit.NOTICE, notice=notice)
        self._left[notice.lease] = ((m.id, m.signer), departure)
        return departure

    def tick(self) -> list[Departure]:
        """Lapse and stall what is due and hand idle shards over. It returns the departures of
        this call only: the Moves, and the departures of sweeps that other entry points ran,
        are read from settle()."""
        return self._sweep()

    # ---- work ----

    def commit(self, member: str, token: int, shard: int, key: str) -> Accepted:
        """Accept `key` as the new head of `shard`. A link its member had accepted before is
        acknowledged again without new credit, even after that lease ended or its round closed
        (for one more round), so a retry after a lost answer is safe."""
        self._sweep()
        done = self._accepted.get(key) or self._accepted_before.get(key)
        if done is not None and (done[0], done[2]) == (shard, member):
            return Accepted(shard, key, done[1], True)
        m = self.membership.check(member, token)
        s = self._shards.get(shard)
        if s is None:
            raise CoordinatorError("UNKNOWN_SHARD", str(shard))
        if s.holder != Holder(member, token):
            raise CoordinatorError("NOT_HOLDER", f"shard {shard}")
        manifest = self.checkpoints.check_link(s.unit, s.head, key)
        if manifest.producer != member or manifest.lease != token:
            raise CoordinatorError("WRONG_PRODUCER", key)
        if not self.audit(s.unit, manifest):
            self._penalties[m.owner] += 1
            self.membership.revoke(member)
            self._depart(m, Exit.REVOKED)
            raise CoordinatorError("AUDIT_FAILED", key)
        s.links.append(Link(key, member, m.owner, manifest.step - s.step))
        self._accepted[key] = (shard, manifest.step, member)
        s.head, s.step = key, manifest.step
        s.since_ms = s.linked_ms = self.clock.now_ms()
        self._progress[member] = (self.clock.now_ms(), False)  # an accepted link restarts it
        if s.done:
            s.holder = None
            self._assign()
        return Accepted(shard, key, s.step, False)

    def withdraw(self, key: str) -> tuple[Link, ...]:
        """Void the accepted link `key` and every link built on it, because an audit that ran
        after commit() found it invalid; returns them, `key` first. Their credit goes with them
        and the shard's head goes back to the link before `key`, so the next holder resumes from
        a state that was not forged. The producer of `key` is penalised and revoked as for a
        failed audit; the producers of the links built on it are not, they were honest, and an
        honest holder of the shard gets a fresh window to restore. With a deferred audit an
        honest host that resumed from an unaudited head loses every link above a forged one, so
        the wiring either audits inside commit() or bounds how deep it lets work build on an
        unaudited link. Only a link of the open round can be withdrawn: a closed round's
        settlement was returned, and UNKNOWN_LINK also answers a link accepted two rounds ago,
        which the wiring must treat as too late, not as never accepted."""
        self._sweep()
        done = self._accepted.get(key)
        if done is None:
            raise CoordinatorError(
                "ROUND_CLOSED" if key in self._accepted_before else "UNKNOWN_LINK", key
            )
        s = self._shards[done[0]]
        first = next(i for i, link in enumerate(s.links) if link.key == key)
        voided = tuple(s.links[first:])
        del s.links[first:]
        for link in voided:
            del self._accepted[link.key]
        s.head = s.links[-1].key if s.links else None
        s.step = sum(link.steps for link in s.links)
        producer = voided[0]
        self._penalties[producer.owner] += 1
        m = self.membership.get(producer.member)
        assert m is not None  # it produced an accepted link
        live = m.status is Status.ACTIVE
        self.membership.revoke(m.id)
        if live:
            self._depart(m, Exit.REVOKED, assign=False)
        s.linked_ms = -1
        if s.holder is not None:  # the honest holder resumes from the link before: fresh clocks
            self._progress[s.holder.member] = (self.clock.now_ms(), False)
            s.since_ms = self.clock.now_ms()
        self._assign()  # a shard that was complete is pending again
        return voided

    def settle(self) -> Settlement:
        self._sweep()
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
            moves=tuple(self._moves),
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
        self,
        member: str,
        owner: str,
        token: int,
        kind: Exit,
        released: tuple[int, ...],
        notice: LeaveNotice | None = None,
    ) -> Departure:
        d = Departure(member, owner, token, kind, self.clock.now_ms(), released, notice)
        self._departures.append(d)
        return d

    def _depart(
        self, m: Member, kind: Exit, *, assign: bool = True, notice: LeaveNotice | None = None
    ) -> Departure:
        released = self._held(m.lease.token)
        for s in released:
            s.holder = None
        shards = tuple(s.unit.shard for s in released)
        d = self._record(m.id, m.owner, m.lease.token, kind, shards, notice)
        if assign:
            self._assign()
        return d

    def _sweep(self) -> list[Departure]:
        """Lapse expired leases, then stalled holders, and only then plan the freed shards, so
        no late member is handed a shard in the same sweep; last, hand idle shards over."""
        out = [self._depart(m, Exit.LAPSE, assign=False) for m in self.membership.expire()]
        now = self.clock.now_ms()
        late = {
            s.holder.member
            for s in self._shards.values()
            if s.holder is not None and now - self._progress[s.holder.member][0] >= self.progress_ms
        }
        out += [
            self._depart(self.membership.lapse(member), Exit.STALLED, assign=False)
            for member in sorted(late)
        ]
        if out:
            self._assign()
        self._hand_over_idle_shards()
        return out

    def _hand_over_idle_shards(self) -> None:
        """Give a shard that saw no accepted link within `progress_ms` to a live member that
        holds nothing: its holder keeps its other shards and is not stalled. With no such
        member, nothing moves. A member that holds a shard is taken to be working it, so shards
        never pass between busy members. A host works its shards in ascending order, as its
        grant lists them, so its first shard is taken only if the host advanced another one after
        it got, or last advanced, that first one (strictly after); a host never loses its last
        shard. The source's window restarts, once per Move; a replacement after it renews nothing
        until the host links again."""
        if self.capacity < 2:
            return
        now = self.clock.now_ms()
        held: dict[int, list[_Shard]] = {}  # each holder's shards, in shard order
        for s in self._shards.values():
            if s.holder is not None:
                held.setdefault(s.holder.token, []).append(s)
        idle = {m.id: m.lease.token for m in self.membership.active() if m.lease.token not in held}
        for shard, s in self._shards.items():
            holder = s.holder
            if not idle or holder is None or now - s.since_ms < self.progress_ms:
                continue
            mine = held[holder.token]
            if s is mine[0] and not any(t.linked_ms > s.since_ms for t in mine if t is not s):
                continue  # it works its first shard first, and has advanced no other
            to = plan(self._seed(), [shard], dict.fromkeys(idle, 1))[shard]
            self._progress[to] = (now, False)  # it starts holding work: its progress window opens
            s.holder, s.since_ms, s.linked_ms = Holder(to, idle.pop(to)), now, -1
            self._progress[holder.member] = (now, True)  # it lost work through no fault of its own
            mine.remove(s)
            self._moves.append(Move(shard, holder.member, holder.token, to, now))

    def _seed(self) -> bytes:
        return self.seed + struct.pack(">Q", self.round)

    def _assign(self) -> None:
        pending = [shard for shard, s in self._shards.items() if s.holder is None and not s.done]
        if not pending:
            return
        load = Counter(s.holder.token for s in self._shards.values() if s.holder is not None)
        live = {m.id: m.lease.token for m in self.membership.active()}
        free = {member: self.capacity - load[token] for member, token in live.items()}
        holding = {s.holder.member for s in self._shards.values() if s.holder is not None}
        for shard, member in plan(self._seed(), pending, free).items():
            self._shards[shard].holder = Holder(member, live[member])
            self._shards[shard].since_ms = self.clock.now_ms()
            self._shards[shard].linked_ms = -1
            if member not in holding:  # it starts holding work: its progress window opens
                self._progress[member] = (self.clock.now_ms(), False)
                holding.add(member)
