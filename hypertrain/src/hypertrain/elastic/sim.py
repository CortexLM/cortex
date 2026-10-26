"""Deterministic eviction simulator of the elastic core (ADR 0002), without GPUs.

Hosts are 8-GPU units running a toy deterministic training workload against the real
Coordinator, CheckpointManager, DrainHandler and FakeNoticeSource on one FakeClock. Seeded fault
schedules take hosts away: a provider notice then a kill, a hard kill (the process dies and
restarts on the same host), a host loss (the machine is gone and a replacement is provisioned),
a network partition, a partition from the store only, a correlated regional loss and a capacity
drought. A run must end in the
state hash of the run without faults, with every accepted link in the final chains, no step
credited twice, every loss of progress within one checkpoint interval and no penalty for an
honest host.

    uv run --frozen python -m hypertrain.elastic.sim table
    uv run --frozen python -m hypertrain.elastic.sim soak --seeds 5000

A long soak runs disjoint seed ranges in parallel, one process each:

    for i in $(seq 0 15); do
      uv run --frozen python -m hypertrain.elastic.sim soak --seeds 2500 --start $((i * 2500)) &
    done; wait
"""

from __future__ import annotations

import argparse
import hashlib
import random
import struct
import sys
from collections import Counter
from collections.abc import Callable, Sequence
from dataclasses import dataclass, replace

from hypertrain.elastic.checkpoint import CheckpointError, CheckpointManifest, WorkUnit
from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.coordinator import (
    Assignment,
    Coordinator,
    CoordinatorError,
    Departure,
    Exit,
    Grant,
    Holder,
)
from hypertrain.elastic.drain import DrainHandler
from hypertrain.elastic.membership import Lease, LeaseError
from hypertrain.elastic.notice import (
    FakeNoticeSource,
    Interruption,
    LeaveNotice,
    ShardHead,
    sign_leave,
)
from hypertrain.elastic.store import MemoryCheckpointStore
from hypertrain.protocol.keys import Keypair

SIM_TAG = b"ht-elastic-sim-v1|"
RUN_ID = "elastic-sim"
POLICY = hashlib.sha256(SIM_TAG + b"policy").hexdigest()

# ---- toy workload: a hash chain per shard and round, an outer step over the shard finals ----


def genesis() -> bytes:
    return hashlib.sha256(SIM_TAG + b"genesis").digest()


def inner_step(state: bytes, round_: int, shard: int, step: int) -> bytes:
    """State after inner step `step` (0-based) of `shard` in `round_`."""
    return hashlib.sha256(
        SIM_TAG + b"step" + state + struct.pack(">QQQ", round_, shard, step)
    ).digest()


def outer_step(finals: Sequence[bytes]) -> bytes:
    return hashlib.sha256(SIM_TAG + b"outer" + b"".join(finals)).digest()


def replay(state: bytes, round_: int, shard: int, start: int, stop: int) -> bytes:
    for step in range(start, stop):
        state = inner_step(state, round_, shard, step)
    return state


def reference(rounds: int, shards: int, steps: int) -> bytes:
    """Final state of the same run on one host that is never interrupted."""
    state = genesis()
    for r in range(rounds):
        state = outer_step([replay(state, r, s, 0, steps) for s in range(shards)])
    return state


# ---- configuration and fault schedules ----


@dataclass(frozen=True, slots=True)
class SimConfig:
    """Durations are milliseconds and multiples of tick_ms. A host is one 8-GPU member."""

    hosts: int = 4
    regions: int = 2
    shards: int = 4
    steps: int = 60
    rounds: int = 2
    tick_ms: int = 1_000
    step_ms: int = 10_000
    ckpt_every: int = 6
    ckpt_ms: int = 5_000
    restore_ms: int = 5_000
    heartbeat_ms: int = 5_000
    ttl_ms: int = 20_000
    poll_ms: int = 5_000
    notice_ms: int = 120_000
    margin_ms: int = 5_000
    restart_ms: int = 30_000
    provision_ms: int = 300_000
    guard_ms: int = 20_000
    ack_loss_ppm: int = 0
    cheaters: tuple[int, ...] = ()
    gpus_per_host: int = 8
    capacity: int = 1  # shards a host may hold; it trains them one after the other

    def __post_init__(self) -> None:
        if min(self.hosts, self.regions, self.shards, self.steps, self.rounds) < 1:
            raise ValueError("hosts, regions, shards, steps and rounds must be >= 1")
        if not 1 <= self.ckpt_every <= self.steps:
            raise ValueError("ckpt_every must be in 1..steps")
        for name in (
            "step_ms",
            "ckpt_ms",
            "restore_ms",
            "heartbeat_ms",
            "ttl_ms",
            "poll_ms",
            "guard_ms",
        ):
            value = getattr(self, name)
            if value <= 0 or value % self.tick_ms:
                raise ValueError(f"{name} must be a positive multiple of tick_ms")
        if self.ttl_ms <= self.heartbeat_ms:
            raise ValueError("ttl_ms must exceed heartbeat_ms")

    @property
    def progress_ms(self) -> int:
        """The coordinator's progress deadline: twice the longest honest wait for one link."""
        return 2 * (
            self.heartbeat_ms + self.restore_ms + self.ckpt_every * self.step_ms + self.ckpt_ms
        )

    def horizon_ms(self) -> int:
        """Rough makespan without faults, the window in which faults are drawn."""
        waves = -(-self.shards // self.hosts)
        per_unit = self.steps * self.step_ms + -(-self.steps // self.ckpt_every) * self.ckpt_ms
        return self.rounds * waves * (per_unit + self.restore_ms + self.heartbeat_ms)


@dataclass(frozen=True, slots=True)
class Fault:
    """`notice`: interruption notice at `at_ms`, kill at `until_ms`. `kill`: the process dies
    and restarts on the same host. `loss`: the host is gone. `partition`: the host cannot reach
    the coordinator or the store until `until_ms`. `store-partition`: it reaches the coordinator
    but not the store."""

    at_ms: int
    kind: str
    slot: int
    until_ms: int = 0


@dataclass(frozen=True, slots=True)
class Schedule:
    name: str
    faults: tuple[Fault, ...] = ()
    droughts: tuple[tuple[int, int], ...] = ()


SCENARIOS = (
    "none",
    "notice",
    "short-notice",
    "kill",
    "loss",
    "partition",
    "store-partition",
    "region-notice",
    "region-loss",
    "drought",
    "churn",
)


def schedule(name: str, cfg: SimConfig, seed: int) -> Schedule:
    """The seeded fault schedule `name` for `cfg`."""
    rng = random.Random(f"{name}|{seed}")
    horizon = max(cfg.horizon_ms(), 2 * cfg.tick_ms)

    def when() -> int:
        return rng.randrange(1, horizon // cfg.tick_ms) * cfg.tick_ms

    def slot() -> int:
        return rng.randrange(cfg.hosts)

    def region() -> list[int]:
        r = rng.randrange(cfg.regions)
        return [s for s in range(cfg.hosts) if s % cfg.regions == r]

    def fault(kind: str, at: int, target: int) -> Fault:
        if kind == "notice":
            return Fault(at, kind, target, at + cfg.notice_ms)
        if kind == "short-notice":
            return Fault(at, "notice", target, at + cfg.tick_ms * rng.randrange(1, 4))
        if kind == "partition":
            span = rng.randrange(cfg.ttl_ms // cfg.tick_ms + 1, 4 * cfg.ttl_ms // cfg.tick_ms)
            return Fault(at, kind, target, at + span * cfg.tick_ms)
        if kind == "store-partition":
            span = rng.randrange(cfg.progress_ms // cfg.tick_ms, 3 * cfg.progress_ms // cfg.tick_ms)
            return Fault(at, kind, target, at + span * cfg.tick_ms)
        return Fault(at, kind, target)

    if name == "none":
        return Schedule(name)
    if name in ("notice", "short-notice", "kill", "loss", "partition", "store-partition"):
        return Schedule(name, (fault(name, when(), slot()),))
    if name in ("region-notice", "region-loss"):
        at, kind = when(), name.removeprefix("region-")
        return Schedule(name, tuple(fault(kind, at, s) for s in region()))
    if name == "drought":
        at = when()
        lost = tuple(fault("loss", at, s) for s in region())
        return Schedule(name, lost, ((at, at + 6 * cfg.provision_ms),))
    if name == "churn":
        kinds = ("notice", "short-notice", "kill", "loss", "partition", "store-partition")
        faults = sorted(
            (fault(rng.choice(kinds), when(), slot()) for _ in range(rng.randrange(2, 9))),
            key=lambda f: (f.at_ms, f.slot),
        )
        return Schedule(name, tuple(faults))
    raise ValueError(f"unknown scenario {name!r}")


# ---- results ----


@dataclass(frozen=True, slots=True)
class Loss:
    """Inner steps a host had computed past its last accepted link when it lost its shard."""

    slot: int
    kind: str
    steps: int
    at_ms: int


@dataclass(frozen=True, slots=True)
class SimResult:
    scenario: str
    seed: int
    completed: bool
    final_ok: bool
    makespan_ms: int
    computed_steps: int
    accepted_steps: int
    losses: tuple[Loss, ...]
    exits: dict[str, int]
    penalized: tuple[str, ...]
    drains: int
    drain_ms: int
    late_leaves: int
    resumes: int
    gpu_ms: int
    violations: tuple[str, ...]

    @property
    def wasted_steps(self) -> int:
        return self.computed_steps - self.accepted_steps

    @property
    def max_loss(self) -> int:
        return max((loss.steps for loss in self.losses), default=0)


# ---- hosts ----


@dataclass(slots=True)
class _Work:
    unit: WorkUnit
    step: int
    state: bytes
    committed: str | None
    committed_step: int
    acc_ms: int = 0
    forged: bool = False


class _Host:
    """The reference miner loop: join, heartbeat, poll the notice, restore, train, checkpoint
    every `ckpt_every` steps, commit, drain on notice. It drops its shard when its own lease
    guard runs out, as a miner must; `guard_ms` above `ttl_ms` models a host whose guard is
    wrong, so that only the coordinator's fencing stands between it and the shard."""

    def __init__(self, sim: Simulation, slot: int, incarnation: int) -> None:
        cfg = sim.cfg
        self.sim = sim
        self.slot = slot
        self.incarnation = incarnation
        self.member = f"m{slot:02d}"
        self.owner = f"miner-{slot:02d}"
        seed = hashlib.sha256(SIM_TAG + f"key|{slot}|{incarnation}".encode()).digest()
        self.key = Keypair(seed)
        self.alive = True
        self.started_ms = sim.clock.now_ms()
        self.lease: Lease | None = None
        self.last_ack_ms = 0
        self.next_hb_ms = 0
        self.next_poll_ms = 0
        self.work: _Work | None = None
        self.busy_until = 0
        self.busy_kind = ""
        self.on_free: Callable[[], None] | None = None
        self.unacked: tuple[str, int] | None = None
        self.leaving: Interruption | None = None
        self.flush_requested = False
        self.drain_started_ms = 0
        self.drain_partitioned = False
        self.left = False
        self.retired = False
        self.cheats = slot in cfg.cheaters
        self.notice = FakeNoticeSource(sim.clock)
        self.drain = DrainHandler(
            clock=sim.clock,
            source=self.notice,
            flush=self._flush,
            leave=self._leave,
            flush_ms=cfg.ckpt_ms,
            margin_ms=cfg.margin_ms,
        )

    @property
    def now(self) -> int:
        return self.sim.clock.now_ms()

    # ---- loop ----

    def tick(self) -> None:
        if not self.alive or self.left:
            return
        if self.leaving is not None and not self.sim.store_reachable(self.slot):
            self.drain_partitioned = True
        if self.busy_until and self.now >= self.busy_until:
            self._free()
        if self.lease is not None and self.now - self.last_ack_ms >= self.sim.cfg.guard_ms:
            self._drop("fenced")
        if self.lease is None:
            if self.leaving is not None:
                self._end_drain()
            elif self.sim.reachable(self.slot):
                self._join()
            return
        if self.now >= self.next_poll_ms:
            self.next_poll_ms = self.now + self.sim.cfg.poll_ms
            self.drain.check()
            if self.left:
                return
        if self.leaving is not None:
            if self.now >= self.next_hb_ms:
                self._heartbeat()  # the lease must outlive the flush
            if self.lease is not None and not self.busy_until:
                self._send_leave()
            return
        if self.unacked is not None:
            self._commit(*self.unacked)
        if self.lease is not None and self.now >= self.next_hb_ms:
            self._heartbeat()
        self._train()

    def _busy(self, ms: int, kind: str, then: Callable[[], None]) -> None:
        self.busy_until, self.busy_kind, self.on_free = self.now + ms, kind, then

    def _free(self) -> None:
        action = self.on_free
        self.busy_until, self.busy_kind, self.on_free = 0, "", None
        if action is not None:
            action()

    # ---- coordinator calls ----

    def _join(self) -> None:
        try:
            grant = self.sim.coord.join(self.member, self.owner, self.key.ss58)
        except LeaseError as e:
            if e.code != "REVOKED":
                raise
            self.die("revoked")
            return
        self.lease = grant.lease
        self.last_ack_ms = self.now
        self.next_hb_ms = self.now + self.sim.cfg.heartbeat_ms
        self._take(grant)

    def _heartbeat(self) -> None:
        if self.lease is None or not self.sim.reachable(self.slot):
            return
        self.next_hb_ms = self.now + self.sim.cfg.heartbeat_ms
        try:
            grant = self.sim.coord.heartbeat(self.member, self.lease.token)
        except LeaseError:
            self._drop("fenced")
            return
        self.last_ack_ms = self.now
        self._take(grant)

    def _take(self, grant: Grant) -> None:
        held = {a.unit.shard: a for a in grant.assignments}
        if self.work is not None and self.work.unit.shard not in held and self.unacked is None:
            self._lose("displaced")
        if self.work is None and held and not self.busy_until and self.leaving is None:
            self._restore(held[min(held)])

    def _restore(self, a: Assignment) -> None:
        def resumed() -> None:
            if not self.sim.store_reachable(self.slot):
                self._busy(self.sim.cfg.tick_ms, "restore", resumed)
                return
            step, state = self.sim.checkpoints.resume(a.unit, a.head)
            self.work = _Work(a.unit, step, state, a.head, step)
            self.sim.resumes += 1

        self._busy(self.sim.cfg.restore_ms, "restore", resumed)

    def _commit(self, key: str, step: int) -> None:
        w = self.work
        if w is None or self.lease is None:
            return
        if not self.sim.reachable(self.slot):
            self.unacked = (key, step)
            return
        try:
            self.sim.coord.commit(self.member, self.lease.token, w.unit.shard, key)
        except LeaseError:
            self._drop("fenced")
            return
        except CoordinatorError as e:
            if e.code == "AUDIT_FAILED":
                self.die("revoked")
            else:
                self._lose(f"refused:{e.code}")
            return
        except CheckpointError as e:
            self._lose(f"refused:{e.code}")
            return
        if self.sim.rng.randrange(1_000_000) < self.sim.cfg.ack_loss_ppm:
            self.unacked = (key, step)
            return
        if w.forged:
            self.sim.violations.append(f"slot {self.slot}: a forged link was accepted")
        self.unacked = None
        w.committed, w.committed_step = key, step
        if step == w.unit.steps:
            self.work = None
            self.next_hb_ms = self.now

    # ---- training and checkpoints ----

    def _train(self) -> None:
        w = self.work
        cfg = self.sim.cfg
        if w is None or self.busy_until or self.unacked is not None or w.step == w.unit.steps:
            return
        w.acc_ms += cfg.tick_ms
        if w.acc_ms < cfg.step_ms:
            return
        w.acc_ms = 0
        step = w.step
        if self.cheats and w.step == cfg.ckpt_every:
            step, w.forged = w.step + 1, True  # skips an inner step: the audit must catch it
        w.state = inner_step(w.state, w.unit.round, w.unit.shard, step)
        w.step += 1
        self.sim.computed += 1
        if w.step - w.committed_step >= cfg.ckpt_every or w.step == w.unit.steps:
            self._checkpoint()

    def _checkpoint(self) -> None:
        w, lease = self.work, self.lease
        if w is None or lease is None or w.step == w.committed_step:
            return
        unit, parent, step, state = w.unit, w.committed, w.step, w.state

        def written() -> None:
            if not self.sim.store_reachable(self.slot):
                self._busy(self.sim.cfg.tick_ms, "ckpt", written)
                return
            key = self.sim.checkpoints.save(unit, parent, step, state, self.member, lease.token)
            self._commit(key, step)

        self._busy(self.sim.cfg.ckpt_ms, "ckpt", written)

    # ---- drain ----

    def _flush(self) -> None:
        self.flush_requested = True
        if self.busy_kind == "restore":
            self.busy_until, self.busy_kind, self.on_free = 0, "", None
        if not self.busy_until:
            self._checkpoint()

    def _leave(self, notice: Interruption) -> None:
        self.leaving = notice
        self.drain_started_ms = self.now
        if not self.flush_requested:
            self.busy_until, self.busy_kind, self.on_free = 0, "", None
            self._send_leave()

    def _send_leave(self) -> None:
        """Send the signed leave, retried every tick while the coordinator is unreachable."""
        assert self.leaving is not None and self.lease is not None
        if not self.sim.reachable(self.slot):
            self.drain_partitioned = True
            return
        if self.unacked is not None:
            self._commit(*self.unacked)
        if self.lease is not None:
            w = self.work
            heads = [] if w is None else [ShardHead(shard=w.unit.shard, head=w.committed)]
            notice = LeaveNotice(
                run_id=RUN_ID,
                member=self.member,
                signer=self.key.ss58,
                lease=self.lease.token,
                reason="interruption",
                provider_time=self.leaving.provider_time,
                heads=heads,
            )
            try:
                self.sim.coord.leave(sign_leave(notice, self.key))
            except LeaseError:
                self.sim.late_leaves += 1  # the coordinator had already lapsed this lease
        self._end_drain()

    def _end_drain(self) -> None:
        took = self.now - self.drain_started_ms
        self.sim.drains += 1
        self.sim.drain_ms += took
        if took > self.sim.cfg.ckpt_ms and not self.drain_partitioned:
            self.sim.violations.append(f"drain of slot {self.slot} took over one checkpoint")
        self._lose("notice")
        self.left = True

    # ---- loss of work ----

    def _lose(self, kind: str) -> None:
        w = self.work
        if w is not None:
            self.sim.losses.append(Loss(self.slot, kind, w.step - w.committed_step, self.now))
        self.work = None
        self.unacked = None
        self.busy_until, self.busy_kind, self.on_free = 0, "", None

    def _drop(self, kind: str) -> None:
        self._lose(kind)
        self.lease = None

    def die(self, kind: str) -> None:
        if not self.alive:
            return
        if not self.left:
            self._lose(kind)
            if self.lease is not None:
                self.sim.dead_leases[self.lease.token] = self.now
        if kind == "notice" and self.leaving is not None and not self.left:
            if not self.drain_partitioned:
                self.sim.violations.append(f"slot {self.slot} died before its leave went out")
        self.alive = False
        self.retired = kind == "revoked"
        self.sim.gpu_ms += (self.now - self.started_ms) * self.sim.cfg.gpus_per_host


# ---- the simulation ----


@dataclass(frozen=True, slots=True, order=True)
class _Event:
    """`boot` starts a new host for `slot`; `evict` is the provider killing the noticed host
    `incarnation` at its deadline."""

    at_ms: int
    kind: str
    slot: int
    incarnation: int = 0


class Simulation:
    def __init__(self, cfg: SimConfig, plan: Schedule, seed: int) -> None:
        self.cfg = cfg
        self.plan = plan
        self.seed = seed
        self.rng = random.Random(f"acks|{seed}")
        self.clock = FakeClock()
        self.store = MemoryCheckpointStore()
        self.coord = Coordinator(
            run_id=RUN_ID,
            clock=self.clock,
            store=self.store,
            ttl_ms=cfg.ttl_ms,
            progress_ms=cfg.progress_ms,
            seed=SIM_TAG + struct.pack(">Q", seed),
            admit=self._admit,
            capacity=cfg.capacity,
            audit=self._audit,
        )
        self.checkpoints = self.coord.checkpoints
        self.hosts: dict[int, _Host] = {}
        self.granted: set[tuple[str, str, str]] = set()  # (member, owner, host key) issued
        self.incarnations: Counter[int] = Counter()
        self.events: list[_Event] = []
        self.partitions: dict[int, int] = {}
        self.store_partitions: dict[int, int] = {}
        self.cuts: dict[int, list[tuple[int, int]]] = {}  # slot -> partitions, kills, losses
        self.departures: list[Departure] = []
        self.losses: list[Loss] = []
        self.computed = 0
        self.accepted = 0
        self.drains = 0
        self.drain_ms = 0
        self.late_leaves = 0
        self.resumes = 0
        self.gpu_ms = 0
        self.exits: Counter[str] = Counter()
        self.penalized: set[str] = set()
        self.violations: list[str] = []
        self.noticed: set[tuple[int, int]] = set()  # (slot, incarnation) given a notice
        self.dead_leases: dict[int, int] = {}
        self.departed: set[int] = set()
        self.state = genesis()
        self._holders: dict[int, Holder | None] = {}

    def reachable(self, slot: int) -> bool:
        return self.clock.now_ms() >= self.partitions.get(slot, 0)

    def store_reachable(self, slot: int) -> bool:
        return self.reachable(slot) and self.clock.now_ms() >= self.store_partitions.get(slot, 0)

    def _audit(self, unit: WorkUnit, manifest: CheckpointManifest) -> bool:
        """Replay the link from its parent state: the auditor of the toy workload."""
        if manifest.parent is None:
            start, state = 0, self.store.get(unit.base)
        else:
            parent = self.checkpoints.manifest(manifest.parent)
            start, state = parent.step, self.store.get(parent.state)
        out = replay(state, unit.round, unit.shard, start, manifest.step)
        return hashlib.sha256(out).hexdigest() == manifest.state

    def _admit(self, member: str, owner: str, signer: str) -> bool:
        """Each miner grants a key to every host it boots; only those keys may join."""
        return (member, owner, signer) in self.granted

    def _boot(self, slot: int) -> None:
        self.incarnations[slot] += 1
        host = _Host(self, slot, self.incarnations[slot])
        self.granted.add((host.member, host.owner, host.key.ss58))
        self.hosts[slot] = host

    def _replace(self, slot: int, delay_ms: int, droughts: bool) -> None:
        at = self.clock.now_ms() + delay_ms
        for start, end in sorted(self.plan.droughts) if droughts else ():
            if start <= at < end:
                at = end
        self.events.append(_Event(at, "boot", slot))

    def _new_machine(self, slot: int) -> None:
        """A replacement is another machine: the old one's partitions do not follow it."""
        self.partitions.pop(slot, None)
        self.store_partitions.pop(slot, None)
        self._replace(slot, self.cfg.provision_ms, droughts=True)

    def _open(self, round_: int) -> None:
        base = self.store.put(self.state)
        units = [
            WorkUnit(RUN_ID, POLICY, round_, shard, base, self.cfg.steps)
            for shard in range(self.cfg.shards)
        ]
        self.coord.open_round(units)
        self._holders = {}

    def _fault(self, f: Fault) -> None:
        host = self.hosts[f.slot]
        if not host.alive:
            return
        if f.kind == "notice":
            if (f.slot, host.incarnation) in self.noticed:
                return  # a provider notices a host once
            self.noticed.add((f.slot, host.incarnation))
            host.notice.schedule(f.at_ms, f.until_ms)
            self.events.append(_Event(f.until_ms, "evict", f.slot, host.incarnation))
        elif f.kind == "kill":
            host.die("kill")
            self.cuts.setdefault(f.slot, []).append((f.at_ms, f.at_ms))
            self._replace(f.slot, self.cfg.restart_ms, droughts=False)
        elif f.kind == "loss":
            host.die("loss")
            self.cuts.setdefault(f.slot, []).append((f.at_ms, f.at_ms))
            self._new_machine(f.slot)
        elif f.kind == "partition":
            self.partitions[f.slot] = f.until_ms
            self.cuts.setdefault(f.slot, []).append((f.at_ms, f.until_ms))
        elif f.kind == "store-partition":
            self.store_partitions[f.slot] = f.until_ms
            self.cuts.setdefault(f.slot, []).append((f.at_ms, f.until_ms))
        else:
            raise ValueError(f"unknown fault {f.kind!r}")

    def _due(self) -> None:
        now = self.clock.now_ms()
        due = sorted(e for e in self.events if e.at_ms <= now)
        self.events = [e for e in self.events if e.at_ms > now]
        for e in due:
            host = self.hosts[e.slot]
            if e.kind == "evict" and host.alive and host.incarnation == e.incarnation:
                host.die("notice")
                self.cuts.setdefault(e.slot, []).append((e.at_ms, e.at_ms))
                self._new_machine(e.slot)
            elif e.kind == "boot" and not host.alive and not host.retired:
                self._boot(e.slot)

    def _check(self) -> None:
        """Per-tick invariants of the coordinator's view."""
        cfg, now = self.cfg, self.clock.now_ms()
        self.departed.update(d.token for d in self.coord.settle().departures)
        load: Counter[int] = Counter()
        for shard in range(cfg.shards):
            holder, before = self.coord.holder(shard), self._holders.get(shard)
            if holder is not None:
                load[holder.token] += 1
            if before is not None and holder is not None and holder.member != before.member:
                if before.token not in self.departed:
                    self.violations.append(f"shard {shard} moved from a live holder {before}")
            self._holders[shard] = holder
        for token, n in load.items():
            if n > self.coord.capacity:
                self.violations.append(f"lease {token} holds {n} shards")
        for token, died in self.dead_leases.items():
            if token not in self.departed and now - died > cfg.ttl_ms:
                self.violations.append(f"lease {token} outlived its host by more than ttl_ms")
                self.departed.add(token)

    def _close_round(self) -> None:
        settlement = self.coord.settle()
        r, cfg = settlement.round, self.cfg
        base = self.store.put(self.state)
        finals = []
        for shard in range(cfg.shards):
            unit = WorkUnit(RUN_ID, POLICY, r, shard, base, cfg.steps)
            head = settlement.heads[shard]
            chain = [m.state for m in self.checkpoints.chain(unit, head)]
            links = self.coord.links(shard)
            if [self.checkpoints.manifest(link.key).state for link in links] != chain:
                self.violations.append(f"round {r} shard {shard}: accepted link missing from chain")
            if sum(link.steps for link in links) != cfg.steps:
                self.violations.append(f"round {r} shard {shard}: steps credited != steps")
            finals.append(self.checkpoints.resume(unit, head)[1])
        if sum(settlement.credits.values()) != cfg.shards * cfg.steps:
            self.violations.append(f"round {r}: credits do not sum to the round's steps")
        self._tally()
        self.state = outer_step(finals)

    def _tally(self) -> None:
        settlement = self.coord.settle()
        self.accepted += sum(settlement.credits.values())
        self.exits.update(d.kind.value for d in settlement.departures)
        self.departures += settlement.departures
        self.penalized.update(settlement.penalties)

    def run(self) -> SimResult:
        cfg = self.cfg
        limit = 8 * cfg.horizon_ms() + 4 * cfg.provision_ms
        limit += sum(end - start for start, end in self.plan.droughts)
        faults = sorted(self.plan.faults, key=lambda f: (f.at_ms, f.slot))
        for slot in range(cfg.hosts):
            self._boot(slot)
        self._open(0)
        completed = False
        while self.clock.now_ms() <= limit:
            while faults and faults[0].at_ms <= self.clock.now_ms():
                self._fault(faults.pop(0))
            self._due()
            self.coord.tick()
            for slot in range(cfg.hosts):
                self.hosts[slot].tick()
            self._check()
            if self.coord.settle().complete:
                self._close_round()
                if self.coord.round + 1 == cfg.rounds:
                    completed = True
                    break
                self._open(self.coord.round + 1)
            self.clock.advance(cfg.tick_ms)
        if not completed:
            self._tally()
            self.violations.append("run did not complete")
        for host in self.hosts.values():
            host.die("end")
        cheaters = {f"miner-{s:02d}" for s in cfg.cheaters}
        if self.penalized - cheaters:
            self.violations.append(f"honest owners penalised: {sorted(self.penalized - cheaters)}")
        for loss in self.losses:
            if loss.steps > cfg.ckpt_every:
                self.violations.append(f"loss of {loss.steps} steps above ckpt_every: {loss}")
            if loss.kind == "displaced" or loss.kind.startswith("refused"):
                self.violations.append(f"the coordinator refused or moved live work: {loss}")
        # a holder hit by no fault in its window (no partition, kill, loss or eviction; a dead
        # host may stall before it lapses) must never be stalled
        for d in self.departures:
            window = (d.at_ms - cfg.progress_ms, d.at_ms)
            cuts = self.cuts.get(int(d.member[1:]), [])
            cut = any(a <= window[1] and b >= window[0] for a, b in cuts)
            if d.kind is Exit.STALLED and d.token not in self.dead_leases and not cut:
                self.violations.append(f"{d.member} stalled with no fault in its window")
        if self.computed - self.accepted > sum(loss.steps for loss in self.losses):
            self.violations.append("wasted steps not accounted for by any loss")
        final_ok = completed and self.state == reference(cfg.rounds, cfg.shards, cfg.steps)
        if completed and not final_ok:
            self.violations.append("final state differs from the uninterrupted run")
        return SimResult(
            scenario=self.plan.name,
            seed=self.seed,
            completed=completed,
            final_ok=final_ok,
            makespan_ms=self.clock.now_ms(),
            computed_steps=self.computed,
            accepted_steps=self.accepted,
            losses=tuple(self.losses),
            exits=dict(self.exits),
            penalized=tuple(sorted(self.penalized)),
            drains=self.drains,
            drain_ms=self.drain_ms,
            late_leaves=self.late_leaves,
            resumes=self.resumes,
            gpu_ms=self.gpu_ms,
            violations=tuple(self.violations),
        )


def simulate(cfg: SimConfig, name: str, seed: int) -> SimResult:
    return Simulation(cfg, schedule(name, cfg, seed), seed).run()


def random_config(rng: random.Random) -> SimConfig:
    """A small random run: the soak's and the property tests' configuration space."""
    steps = rng.randrange(4, 61)
    return SimConfig(
        hosts=rng.randrange(1, 9),
        regions=rng.randrange(1, 4),
        shards=rng.randrange(1, 9),
        steps=steps,
        rounds=rng.randrange(1, 4),
        ckpt_every=rng.randrange(1, steps + 1),
        restart_ms=rng.choice((5_000, 30_000)),
        guard_ms=rng.choice((20_000, 40_000)),
        ack_loss_ppm=rng.choice((0, 100_000, 300_000)),
        ckpt_ms=rng.choice((5_000, 15_000, 30_000)),
        ttl_ms=rng.choice((20_000, 60_000)),
        capacity=rng.choice((1, 2)),
    )


# ---- reporting ----


def table(results: Sequence[SimResult], cfg: SimConfig) -> str:
    """Markdown table: one row per run, waste in inner steps and GPU-hours."""
    step_gpu_ms = cfg.step_ms * cfg.gpus_per_host
    rows = [
        "| scenario | done | final = ref | exits (notice/lapse/stalled/replaced/revoked) "
        "| penalised | computed | accepted | wasted | max loss (bound) | drain s "
        "| makespan min | GPU-h | useful % |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for r in results:
        kinds = ("notice", "lapse", "stalled", "replaced", "revoked")
        exits = "/".join(str(r.exits.get(k, 0)) for k in kinds)
        useful = 100 * r.accepted_steps * step_gpu_ms / r.gpu_ms if r.gpu_ms else 0.0
        rows.append(
            f"| {r.scenario} | {'yes' if r.completed else 'NO'} | {'yes' if r.final_ok else 'NO'} "
            f"| {exits} | {', '.join(r.penalized) or '-'} | {r.computed_steps} "
            f"| {r.accepted_steps} | {r.wasted_steps} | {r.max_loss} ({cfg.ckpt_every}) "
            f"| {r.drain_ms / 1000:.0f} | {r.makespan_ms / 60_000:.1f} "
            f"| {r.gpu_ms / 3_600_000:.2f} | {useful:.1f} |"
        )
    return "\n".join(rows)


EXIT_KINDS = ("notice", "lapse", "stalled", "replaced", "revoked")


def aggregate(cfg: SimConfig, names: Sequence[str], seeds: Sequence[int]) -> tuple[str, bool]:
    """Markdown table of each scenario over `seeds`, and whether every run was clean.

    Exits are totals over the runs. Wasted steps are computed minus accepted inner steps; the
    max loss is the largest single loss against its bound (`ckpt_every`). Drain seconds,
    makespan and GPU-hours are means per run; GPU-hours are compared with the fault-free run of
    the same configuration."""
    base = simulate(cfg, "none", seeds[0]).gpu_ms
    rows = [
        "| scenario | runs | done, final = ref | violations "
        "| exits notice/lapse/stalled/replaced/revoked | penalised runs "
        "| wasted steps mean / max | max loss (bound) | drain s | makespan min | GPU-h (vs none) |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    clean = True
    for name in names:
        rs = [simulate(cfg, name, seed) for seed in seeds]
        clean = clean and not any(r.violations for r in rs)
        exits = "/".join(str(sum(r.exits.get(k, 0) for r in rs)) for k in EXIT_KINDS)
        wasted = [r.wasted_steps for r in rs]
        gpu_h = sum(r.gpu_ms for r in rs) / len(rs) / 3_600_000
        rows.append(
            f"| {name} | {len(rs)} | {sum(r.completed and r.final_ok for r in rs)} "
            f"| {sum(bool(r.violations) for r in rs)} | {exits} "
            f"| {sum(bool(r.penalized) for r in rs)} "
            f"| {sum(wasted) / len(rs):.1f} / {max(wasted)} "
            f"| {max(r.max_loss for r in rs)} ({cfg.ckpt_every}) "
            f"| {sum(r.drain_ms for r in rs) / len(rs) / 1000:.1f} "
            f"| {sum(r.makespan_ms for r in rs) / len(rs) / 60_000:.1f} "
            f"| {gpu_h:.2f} ({100 * (gpu_h * 3_600_000 / base - 1):+.0f} %) |"
        )
    return "\n".join(rows), clean


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m hypertrain.elastic.sim")
    sub = parser.add_subparsers(dest="command", required=True)
    tab = sub.add_parser("table", help="one run per scenario, as a markdown table")
    tab.add_argument("--seed", type=int, default=1)
    tab.add_argument("--hosts", type=int, default=SimConfig().hosts)
    tab.add_argument("--shards", type=int, default=SimConfig().shards)
    soak = sub.add_parser(
        "soak", help="every scenario over many random runs; exit 1 on a violation"
    )
    soak.add_argument("--seeds", type=int, default=200)
    soak.add_argument("--start", type=int, default=0)
    agg = sub.add_parser("aggregate", help="each scenario over many seeds (the ADR's tables)")
    agg.add_argument("--seeds", type=int, default=200)
    agg.add_argument("--hosts", type=int, default=SimConfig().hosts)
    agg.add_argument("--shards", type=int, default=SimConfig().shards)
    agg.add_argument("--ckpt-every", type=int, default=SimConfig().ckpt_every)
    agg.add_argument("--cheater", type=int, help="slot of a host that forges one link")
    agg.add_argument("--scenarios", nargs="+", choices=SCENARIOS, default=list(SCENARIOS))
    args = parser.parse_args(argv)
    if args.command == "aggregate":
        cfg = SimConfig(hosts=args.hosts, shards=args.shards, ckpt_every=args.ckpt_every)
        if args.cheater is not None:
            cfg = replace(cfg, cheaters=(args.cheater,))
        text, clean = aggregate(cfg, args.scenarios, range(args.seeds))
        print(text)
        return 0 if clean else 1
    if args.command == "table":
        cfg = SimConfig(hosts=args.hosts, shards=args.shards)
        results = [simulate(cfg, name, args.seed) for name in SCENARIOS]
        cheater = simulate(replace(cfg, cheaters=(1 % cfg.hosts,)), "churn", args.seed)
        results.append(replace(cheater, scenario="churn + cheater"))
        print(table(results, cfg))
        return 1 if any(r.violations for r in results) else 0
    failures = 0
    for seed in range(args.start, args.start + args.seeds):
        cfg = random_config(random.Random(seed))
        for name in SCENARIOS:
            result = simulate(cfg, name, seed)
            if result.violations:
                failures += 1
                print(f"seed {seed} {name} {cfg}: {list(result.violations)}", file=sys.stderr)
    print(f"soak: {args.seeds} seeds x {len(SCENARIOS)} scenarios, {failures} with violations")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
