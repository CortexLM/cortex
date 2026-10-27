"""Coordinator: fenced holders, compare-and-set heads, idempotent retries, credit and penalties."""

import gc
import hashlib
from collections.abc import Callable
from pathlib import Path

import pytest

from hypertrain.elastic.checkpoint import CheckpointError, CheckpointManifest, WorkUnit
from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.coordinator import Coordinator, CoordinatorError, Exit, Holder
from hypertrain.elastic.membership import Lease, LeaseError
from hypertrain.elastic.notice import (
    LeaveNotice,
    NoticeError,
    ShardHead,
    SignedLeave,
    sign_leave,
)
from hypertrain.elastic.store import MemoryCheckpointStore
from hypertrain.elastic.tokens import LocalTokenLog, MemoryTokenLog, TokenLog, TokenLogError
from hypertrain.protocol.keys import Keypair

RUN = "run-1"
POLICY = hashlib.sha256(b"policy").hexdigest()
TTL = 1_000
PROGRESS = 4_000
KEYS = {name: Keypair(bytes([i + 1]) * 32) for i, name in enumerate("abcdz")}


def trusting(_unit: WorkUnit, _manifest: CheckpointManifest) -> bool:
    """The audit of tests about something else: it accepts what check_link accepted."""
    return True


def coordinator(
    clock: FakeClock,
    store: MemoryCheckpointStore,
    tokens: TokenLog,
    audit: Callable[[WorkUnit, CheckpointManifest], bool] = trusting,
    capacity: int = 1,
    ttl_ms: int = TTL,
) -> Coordinator:
    return Coordinator(
        run_id=RUN,
        clock=clock,
        store=store,
        ttl_ms=ttl_ms,
        progress_ms=PROGRESS,
        seed=b"s",
        admit=lambda _member, _owner, signer: signer != KEYS["z"].ss58,  # z holds no grant
        tokens=tokens,
        audit=audit,
        capacity=capacity,
    )


def setup(
    shards: int = 1,
    audit: Callable[[WorkUnit, CheckpointManifest], bool] = trusting,
    capacity: int = 1,
    ttl_ms: int = TTL,
) -> tuple[FakeClock, Coordinator, list[WorkUnit]]:
    clock = FakeClock()
    store = MemoryCheckpointStore()
    coord = coordinator(clock, store, MemoryTokenLog(), audit, capacity, ttl_ms)
    base = store.put(b"base")
    units = [WorkUnit(RUN, POLICY, 0, shard, base, 6) for shard in range(shards)]
    coord.open_round(units)
    return clock, coord, units


def join(coord: Coordinator, name: str) -> Lease:
    return coord.join(name, f"owner-{name}", KEYS[name].ss58).lease


def link(
    coord: Coordinator,
    unit: WorkUnit,
    parent: str | None,
    step: int,
    lease: Lease,
    state: bytes = b"",
) -> str:
    data = state or f"{unit.shard}:{step}".encode()
    return coord.checkpoints.save(unit, parent, step, data, lease.member, lease.token)


def notice(lease: Lease, signer: Keypair) -> LeaveNotice:
    return LeaveNotice(
        run_id=RUN,
        member=lease.member,
        signer=signer.ss58,
        lease=lease.token,
        reason="interruption",
        provider_time="2026-10-26T12:00:00Z",
        heads=[],
    )


def test_only_the_live_holder_extends_a_shard() -> None:
    _, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    assert coord.holder(0) is not None and coord.holder(0).member == "a"
    with pytest.raises(CoordinatorError) as e:
        coord.commit("b", b.token, 0, link(coord, unit, None, 3, b))
    assert e.value.code == "NOT_HOLDER"
    assert coord.commit("a", a.token, 0, link(coord, unit, None, 3, a)).step == 3


def test_only_an_admitted_key_joins_or_replaces_a_member() -> None:
    _, coord, _ = setup()
    a = join(coord, "a")
    for member, signer, code in (
        ("a", KEYS["z"].ss58, "NOT_ADMITTED"),
        ("a", "not-an-ss58-key", "BAD_SIGNER"),
        ("a b", KEYS["c"].ss58, "BAD_MEMBER"),
    ):
        with pytest.raises(LeaseError) as e:
            coord.join(member, "owner-a", signer)
        assert e.value.code == code
    assert coord.holder(0) == Holder("a", a.token)
    assert coord.heartbeat("a", a.token).lease.token == a.token
    assert coord.settle().departures == ()


def test_a_lapsed_lease_releases_its_shard_and_head_without_a_tick() -> None:
    clock, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)
    clock.advance(TTL // 2)
    coord.heartbeat("b", b.token)
    clock.advance(TTL // 2)  # a's lease runs out now, b's is still live
    grant = coord.heartbeat("b", b.token)
    assert [(x.unit.shard, x.head, x.step) for x in grant.assignments] == [(0, k3, 3)]
    departures = coord.settle().departures
    assert [(d.member, d.kind, d.released) for d in departures] == [("a", Exit.LAPSE, (0,))]
    with pytest.raises(LeaseError):
        coord.commit("a", a.token, 0, link(coord, unit, k3, 6, a))


def test_a_holder_that_stops_progressing_loses_its_shard_unpenalised() -> None:
    clock, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")

    def wait(ms: int) -> None:  # both keep heartbeating, so no lease runs out
        for _ in range(ms // 500):
            clock.advance(500)
            coord.heartbeat("b", b.token)
            if coord.membership.get("a").status == "active":
                coord.heartbeat("a", a.token)

    wait(1_000)
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)  # progress: the deadline restarts here
    wait(PROGRESS - 500)
    assert coord.holder(0) == Holder("a", a.token)
    wait(500)
    assert [(d.member, d.kind, d.released) for d in coord.settle().departures] == [
        ("a", Exit.STALLED, (0,))
    ]
    assert coord.holder(0) == Holder("b", b.token)
    assert coord.heartbeat("b", b.token).assignments[0].head == k3
    with pytest.raises(LeaseError) as e:
        coord.heartbeat("a", a.token)
    assert e.value.code == "LAPSED"
    settlement = coord.settle()
    assert settlement.penalties == {} and settlement.credits == {"owner-a": 3}


def keep_alive(clock: FakeClock, coord: Coordinator, leases: dict[str, Lease], ms: int) -> None:
    """Advance `ms`, heartbeating every live member each 500 ms so no lease runs out."""
    for _ in range(ms // 500):
        clock.advance(500)
        for name, lease in leases.items():
            if coord.membership.get(name).status == "active":
                try:
                    coord.heartbeat(name, lease.token)
                except LeaseError as e:
                    assert e.code == "LAPSED"  # stalled by this very call's sweep


def test_a_host_that_rejoins_in_a_loop_still_loses_a_shard_it_never_advances() -> None:
    clock, coord, _ = setup()
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}
    assert coord.holder(0) == Holder("a", leases["a"].token)
    for _ in range(2 * PROGRESS // 500):  # a crash loop: a new host for a every 500 ms
        keep_alive(clock, coord, leases, 500)
        if coord.membership.get("a").status == "active":
            leases["a"] = join(coord, "a")
    assert coord.holder(0) == Holder("b", leases["b"].token)
    kinds = [d.kind for d in coord.settle().departures if d.member == "a"]
    assert Exit.STALLED in kinds and kinds.count(Exit.REPLACED) > 1


def test_a_member_working_two_shards_in_turn_is_not_stalled() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a")}
    assert coord.holder(0) == coord.holder(1) == Holder("a", leases["a"].token)
    for shard in (0, 1):  # shard 1 waits while shard 0 is trained
        head = None
        for step in range(1, 7):
            keep_alive(clock, coord, leases, PROGRESS // 2)
            head = link(coord, units[shard], head, step, leases["a"])
            coord.commit("a", leases["a"].token, shard, head)
    settlement = coord.settle()
    assert settlement.complete and settlement.departures == ()
    assert settlement.credits == {"owner-a": 12}
    assert settlement.moves == ()  # no other member to hand shard 1 to while shard 0 was trained


def test_a_shard_handed_to_a_busy_member_does_not_extend_its_window() -> None:
    clock, coord, units = setup(shards=3, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}
    mine = [s for s in range(3) if coord.holder(s).member == "a"]
    assert len(mine) == 2  # a holds two shards, b the third
    keep_alive(clock, coord, leases, 500)
    coord.commit("a", leases["a"].token, mine[0], link(coord, units[mine[0]], None, 6, leases["a"]))
    keep_alive(clock, coord, leases, 500)  # a's window runs from that link at t=500
    coord.leave(sign_leave(notice(leases["b"], KEYS["b"]), KEYS["b"]))
    assert {s for s in range(3) if coord.holder(s) == Holder("a", leases["a"].token)} == {
        mine[1],
        next(s for s in range(3) if s not in mine),
    }  # b's shard moves to a at t=1000, which a never advances
    del leases["b"]
    keep_alive(clock, coord, leases, PROGRESS - 500)  # to t = 500 + PROGRESS
    stalled = [d for d in coord.settle().departures if d.kind is Exit.STALLED]
    assert [(d.member, d.at_ms) for d in stalled] == [("a", 500 + PROGRESS)]


def test_late_members_are_all_lapsed_before_their_shards_move() -> None:
    clock, coord, _ = setup(shards=3, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}
    held = {name: tuple(s for s in range(3) if coord.holder(s).member == name) for name in "ab"}
    assert sorted(len(shards) for shards in held.values()) == [1, 2]
    keep_alive(clock, coord, leases, PROGRESS)  # nobody ever commits
    departures = coord.settle().departures
    assert {d.member: (d.kind, d.released) for d in departures} == {
        name: (Exit.STALLED, shards) for name, shards in held.items()
    }


def test_a_departure_moves_only_the_departed_members_shards() -> None:
    clock, coord, _ = setup(shards=3)
    leases = {name: join(coord, name) for name in "abcd"}  # capacity 1: d is a spare
    before = {shard: coord.holder(shard) for shard in range(3)}
    assert "d" not in {h.member for h in before.values() if h is not None}
    victim = before[2]  # the last shard, so a planner that reshuffled would move shard 0
    assert victim is not None
    clock.advance(TTL // 2)
    for name, lease in leases.items():
        if name != victim.member:
            coord.heartbeat(name, lease.token)
    clock.advance(TTL // 2)
    coord.tick()
    after = {shard: coord.holder(shard) for shard in range(3)}
    assert after[0] == before[0] and after[1] == before[1]
    assert after[2] is not None and after[2].member == "d"


def test_a_commit_must_extend_the_accepted_head() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)
    with pytest.raises(CheckpointError) as e:
        coord.commit("a", a.token, 0, link(coord, unit, None, 6, a))
    assert e.value.code == "NOT_HEAD_CHILD"
    assert coord.settle().heads == {0: k3}
    assert coord.commit("a", a.token, 0, link(coord, unit, k3, 6, a)).step == 6
    assert coord.settle().complete and coord.holder(0) is None


def test_a_retried_commit_is_acknowledged_again_without_new_credit() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    k3 = link(coord, unit, None, 3, a)
    first, again = coord.commit("a", a.token, 0, k3), coord.commit("a", a.token, 0, k3)
    assert (first.duplicate, again.duplicate, again.step) == (False, True, 3)
    assert coord.settle().credits == {"owner-a": 3}


def test_a_retried_commit_is_acknowledged_after_its_lease_and_round_ended() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)
    k6 = link(coord, unit, k3, 6, a)
    coord.commit("a", a.token, 0, k6)  # the answer is lost; the round completes
    coord.leave(sign_leave(notice(a, KEYS["a"]), KEYS["a"]))
    coord.open_round([WorkUnit(RUN, POLICY, 1, 0, unit.base, 6)])
    again = coord.commit("a", a.token, 0, k6)
    assert (again.step, again.duplicate) == (6, True)
    assert coord.settle().credits == {}


def test_a_failed_audit_penalises_only_its_producer() -> None:
    forged = hashlib.sha256(b"forged").hexdigest()
    _, coord, (unit,) = setup(audit=lambda _unit, m: m.state != forged)
    a, b = join(coord, "a"), join(coord, "b")
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)
    with pytest.raises(CoordinatorError) as e:
        coord.commit("a", a.token, 0, link(coord, unit, k3, 6, a, state=b"forged"))
    assert e.value.code == "AUDIT_FAILED"
    with pytest.raises(LeaseError) as lease_error:
        coord.heartbeat("a", a.token)
    assert lease_error.value.code == "REVOKED"
    assert coord.heartbeat("b", b.token).assignments[0].head == k3
    coord.commit("b", b.token, 0, link(coord, unit, k3, 6, b))
    settlement = coord.settle()
    assert settlement.penalties == {"owner-a": 1}
    assert settlement.credits == {"owner-a": 3, "owner-b": 3}


def test_a_signed_leave_releases_the_shards_at_once_without_penalty() -> None:
    _, coord, (unit,) = setup()
    a, _ = join(coord, "a"), join(coord, "b")
    coord.commit("a", a.token, 0, link(coord, unit, None, 3, a))
    signed = sign_leave(notice(a, KEYS["a"]), KEYS["a"])
    departure = coord.leave(signed)
    assert (departure.kind, departure.released) == (Exit.NOTICE, (0,))
    assert departure.notice == signed.notice  # heads and provider time kept as evidence
    assert coord.holder(0) is not None and coord.holder(0).member == "b"
    assert coord.leave(signed) == departure  # a retry after a lost answer
    settlement = coord.settle()
    assert settlement.penalties == {}
    assert settlement.credits == {"owner-a": 3}


def test_a_forged_leave_signature_is_refused() -> None:
    _, coord, _ = setup()
    a = join(coord, "a")
    body = notice(a, KEYS["a"])
    forged = SignedLeave(notice=body, signature=KEYS["z"].sign(body.signing_message()).hex())
    with pytest.raises(NoticeError):
        coord.leave(forged)
    assert coord.holder(0) is not None and coord.holder(0).member == "a"


def test_a_leave_signed_by_a_key_the_member_did_not_register_is_refused() -> None:
    _, coord, _ = setup()
    a = join(coord, "a")
    with pytest.raises(NoticeError):
        coord.leave(sign_leave(notice(a, KEYS["z"]), KEYS["z"]))
    assert coord.holder(0) is not None and coord.holder(0).member == "a"


def test_a_replacement_host_resumes_the_shard_from_the_accepted_head() -> None:
    _, coord, (unit,) = setup()
    old = join(coord, "a")
    k3 = link(coord, unit, None, 3, old)
    coord.commit("a", old.token, 0, k3)
    grant = coord.join("a", "owner-a", KEYS["b"].ss58)  # new host, before the old lease ends
    new = grant.lease
    assert [(x.unit.shard, x.head) for x in grant.assignments] == [(0, k3)]
    assert [(d.kind, d.token) for d in coord.settle().departures] == [(Exit.REPLACED, old.token)]
    with pytest.raises(LeaseError) as e:
        coord.commit("a", old.token, 0, link(coord, unit, k3, 6, old))
    assert e.value.code == "STALE_TOKEN"
    assert coord.checkpoints.resume(unit, k3) == (3, b"0:3")
    coord.commit("a", new.token, 0, link(coord, unit, k3, 6, new))
    assert coord.settle().credits == {"owner-a": 6}


def test_a_holder_cannot_commit_a_link_another_member_produced() -> None:
    clock, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    a_link = link(coord, unit, None, 3, a)  # saved, never committed
    clock.advance(TTL // 2)
    coord.heartbeat("b", b.token)
    clock.advance(TTL // 2)
    assert coord.heartbeat("b", b.token).assignments[0].unit == unit  # a lapsed, b holds
    with pytest.raises(CoordinatorError) as e:
        coord.commit("b", b.token, 0, a_link)
    assert e.value.code == "WRONG_PRODUCER"
    assert coord.settle().credits == {}


def test_a_head_whose_state_was_never_uploaded_is_refused() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    ghost = CheckpointManifest(
        run_id=unit.run_id,
        policy=unit.policy,
        round=unit.round,
        shard=unit.shard,
        base=unit.base,
        parent=None,
        step=3,
        state=hashlib.sha256(b"never uploaded").hexdigest(),
        producer="a",
        lease=a.token,
    )
    key = coord.checkpoints.store.put(ghost.encode())
    with pytest.raises(CheckpointError) as e:
        coord.commit("a", a.token, 0, key)
    assert e.value.code == "MISSING_STATE"
    assert coord.settle().heads == {0: None}


def test_a_leave_signed_for_another_run_is_refused() -> None:
    _, coord, _ = setup()
    a = join(coord, "a")  # token 1 here, and token 1 for the same key in any other run
    replayed = notice(a, KEYS["a"]).model_copy(update={"run_id": "run-0"})
    with pytest.raises(NoticeError):
        coord.leave(sign_leave(replayed, KEYS["a"]))
    assert coord.holder(0) == Holder("a", a.token)


def test_open_round_refuses_units_no_manifest_can_bind() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    coord.commit("a", a.token, 0, link(coord, unit, None, 6, a))
    missing = hashlib.sha256(b"not stored").hexdigest()
    for bad in (
        WorkUnit(RUN, POLICY, 1, 0, unit.base, 0),
        WorkUnit(RUN, POLICY.upper(), 1, 0, unit.base, 6),
        WorkUnit(RUN, POLICY, 1, 0, missing, 6),
        WorkUnit(RUN, POLICY, 1, 2**53, unit.base, 6),
        WorkUnit(RUN, POLICY, 2**64, 0, unit.base, 6),
    ):
        with pytest.raises(ValueError):
            coord.open_round([bad])
    assert coord.round == 0 and coord.settle().complete
    with pytest.raises(ValueError):
        Coordinator(
            run_id="cortex/v3 run",
            clock=FakeClock(),
            store=MemoryCheckpointStore(),
            ttl_ms=TTL,
            progress_ms=PROGRESS,
            seed=b"s",
            admit=lambda *_: True,
            tokens=MemoryTokenLog(),
            audit=trusting,
        )


def test_a_departure_after_the_last_commit_is_in_the_closing_settlement() -> None:
    _, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    coord.commit("a", a.token, 0, link(coord, unit, None, 6, a))
    assert coord.settle().complete
    coord.leave(sign_leave(notice(b, KEYS["b"]), KEYS["b"]))  # idle b leaves before round 1
    closed = coord.open_round([WorkUnit(RUN, POLICY, 1, 0, unit.base, 6)])
    assert closed is not None and closed.round == 0
    assert [(d.member, d.kind) for d in closed.departures] == [("b", Exit.NOTICE)]
    assert coord.settle().departures == ()


def test_a_round_opens_only_once_every_shard_is_complete() -> None:
    _, coord, (unit,) = setup()
    with pytest.raises(CoordinatorError) as e:
        coord.open_round([WorkUnit(RUN, POLICY, 1, 0, unit.base, 6)])
    assert e.value.code == "ROUND_OPEN"


@pytest.mark.parametrize("durable", [False, True])
def test_a_signed_leave_cannot_be_replayed_after_a_restart(tmp_path: Path, durable: bool) -> None:
    clock, store = FakeClock(), MemoryCheckpointStore()
    tokens: TokenLog = LocalTokenLog.create(tmp_path / "tokens") if durable else MemoryTokenLog()
    units = [WorkUnit(RUN, POLICY, 0, 0, store.put(b"base"), 6)]
    first = coordinator(clock, store, tokens)
    first.open_round(units)
    old = first.join("a", "owner-a", KEYS["a"].ss58).lease
    signed = sign_leave(notice(old, KEYS["a"]), KEYS["a"])
    first.leave(signed)
    if isinstance(tokens, LocalTokenLog):
        tokens.close()  # the master dies, and the system drops its lock on the log
        tokens = LocalTokenLog(tmp_path / "tokens")
    restarted = coordinator(clock, store, tokens)  # it restarts: only the mark stays
    restarted.open_round(units)
    new = restarted.join("a", "owner-a", KEYS["a"].ss58).lease
    assert new.token > old.token
    for replay in (
        lambda: restarted.leave(signed),
        lambda: restarted.heartbeat("a", old.token),  # a host of the old process is fenced too
    ):
        with pytest.raises(LeaseError) as e:
            replay()
        assert e.value.code == "STALE_TOKEN"
    assert restarted.holder(0) == Holder("a", new.token)


def test_a_coordinator_does_not_start_on_a_token_log_it_cannot_read(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    with pytest.raises(TokenLogError):  # missing: a lost volume is not a new run
        coordinator(FakeClock(), MemoryCheckpointStore(), LocalTokenLog(path))
    path.write_bytes(b"garbage")
    with pytest.raises(TokenLogError):
        coordinator(FakeClock(), MemoryCheckpointStore(), LocalTokenLog(path))


def test_a_second_coordinator_does_not_start_on_a_token_log_that_is_held(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    first = coordinator(FakeClock(), MemoryCheckpointStore(), LocalTokenLog.create(path))
    with pytest.raises(TokenLogError):
        coordinator(FakeClock(), MemoryCheckpointStore(), LocalTokenLog(path))
    assert first.membership.get("a") is None  # the first one is untouched and still grants
    first.join("a", "owner-a", KEYS["a"].ss58)
    assert LocalTokenLog.read(path) == 1


def test_a_retry_is_answered_one_round_later_and_no_longer() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    k6 = link(coord, unit, None, 6, a)
    coord.commit("a", a.token, 0, k6)
    signed = sign_leave(notice(a, KEYS["a"]), KEYS["a"])
    departure = coord.leave(signed)
    b = join(coord, "b")
    next_unit = WorkUnit(RUN, POLICY, 1, 0, unit.base, 6)
    coord.open_round([next_unit])
    assert coord.commit("a", a.token, 0, k6).duplicate  # the round after: still answered
    assert coord.leave(signed) == departure
    coord.commit("b", b.token, 0, link(coord, next_unit, None, 6, b))
    coord.open_round([WorkUnit(RUN, POLICY, 2, 0, unit.base, 6)])
    for retry in (lambda: coord.commit("a", a.token, 0, k6), lambda: coord.leave(signed)):
        with pytest.raises(LeaseError) as e:  # two rounds later: the ledgers have moved on
            retry()
        assert e.value.code == "LEFT"


def test_the_leaves_kept_do_not_grow_with_the_rounds() -> None:
    _, coord, (unit,) = setup()
    for r in range(20):  # one leave notice a round, each carrying its evidence
        this = WorkUnit(RUN, POLICY, r, 0, unit.base, 6)
        if r:
            coord.open_round([this])
        a = join(coord, "a")
        coord.commit("a", a.token, 0, link(coord, this, None, 6, a))
        coord.leave(sign_leave(notice(a, KEYS["a"]), KEYS["a"]))
    gc.collect()
    kept = sum(isinstance(o, LeaveNotice) for o in gc.get_objects())
    assert kept <= 2  # the last two rounds': the current one and the retry window's


def test_a_leave_names_no_more_heads_than_the_round_has_shards() -> None:
    _, coord, _ = setup(shards=2)
    a = join(coord, "a")
    head = ShardHead(shard=0, head=None)
    flood = notice(a, KEYS["a"]).model_copy(update={"heads": [head] * 3})
    with pytest.raises(NoticeError):
        coord.leave(sign_leave(flood, KEYS["a"]))
    assert coord.holder(0) == Holder("a", a.token)  # refused before it released anything
    full = notice(a, KEYS["a"]).model_copy(update={"heads": [head, head]})
    assert coord.leave(sign_leave(full, KEYS["a"])).notice == full


def test_a_retried_leave_is_answered_even_when_the_next_round_is_smaller() -> None:
    _, coord, units = setup(shards=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}
    for shard in (0, 1):  # each member completes the shard it holds
        owner = coord.holder(shard)
        lease = leases[owner.member]
        coord.commit(owner.member, lease.token, shard, link(coord, units[shard], None, 6, lease))
    heads = [ShardHead(shard=0, head=None), ShardHead(shard=1, head=None)]
    signed = sign_leave(
        notice(leases["a"], KEYS["a"]).model_copy(update={"heads": heads}), KEYS["a"]
    )
    departure = coord.leave(signed)  # two heads in a round of two shards
    coord.open_round([WorkUnit(RUN, POLICY, 1, 0, units[0].base, 6)])  # a round of one shard
    assert coord.leave(signed) == departure  # the retry of an accepted leave is not a new notice


def test_a_coordinator_cannot_be_built_without_an_audit() -> None:
    with pytest.raises(TypeError, match="audit"):
        Coordinator(  # type: ignore[call-arg]
            run_id=RUN,
            clock=FakeClock(),
            store=MemoryCheckpointStore(),
            ttl_ms=TTL,
            progress_ms=PROGRESS,
            seed=b"s",
            admit=lambda *_: True,
            tokens=MemoryTokenLog(),
        )


def test_a_withdrawn_link_takes_its_credit_and_the_links_built_on_it() -> None:
    _, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    k2 = link(coord, unit, None, 2, a)
    coord.commit("a", a.token, 0, k2)
    k4 = link(coord, unit, k2, 4, a, state=b"forged")  # accepted: only a later replay catches it
    coord.commit("a", a.token, 0, k4)
    coord.leave(sign_leave(notice(a, KEYS["a"]), KEYS["a"]))
    k6 = link(coord, unit, k4, 6, b)  # b resumed from the forged state in good faith
    coord.commit("b", b.token, 0, k6)
    assert coord.settle().complete
    assert coord.settle().credits == {"owner-a": 4, "owner-b": 2}

    voided = coord.withdraw(k4)

    assert [(v.key, v.member, v.steps) for v in voided] == [(k4, "a", 2), (k6, "b", 2)]
    settlement = coord.settle()
    assert (settlement.heads, settlement.credits) == ({0: k2}, {"owner-a": 2})
    assert not settlement.complete and settlement.penalties == {"owner-a": 1}
    assert [x.kind for x in settlement.departures] == [Exit.NOTICE]  # a had left already
    resumed = coord.heartbeat("b", b.token).assignments  # honest b is not penalised, nor revoked
    assert [(x.head, x.step) for x in resumed] == [(k2, 2)]
    with pytest.raises(LeaseError) as e:
        coord.join("a", "owner-a", KEYS["a"].ss58)
    assert e.value.code == "REVOKED"
    with pytest.raises(CheckpointError) as refused:  # a retry of a voided link is not an ack
        coord.commit("b", b.token, 0, k6)
    assert refused.value.code == "NOT_HEAD_CHILD"
    coord.commit("b", b.token, 0, link(coord, unit, k2, 6, b, state=b"replayed"))
    assert coord.settle().credits == {"owner-a": 2, "owner-b": 4} and coord.settle().complete


def test_withdrawing_a_link_of_a_live_holder_revokes_it_and_frees_its_shard() -> None:
    _, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)
    coord.withdraw(k3)
    with pytest.raises(LeaseError) as e:
        coord.heartbeat("a", a.token)
    assert e.value.code == "REVOKED"
    assert coord.holder(0) == Holder("b", b.token)
    assert [(x.head, x.step) for x in coord.heartbeat("b", b.token).assignments] == [(None, 0)]
    settlement = coord.settle()
    assert [(d.member, d.kind, d.released) for d in settlement.departures] == [
        ("a", Exit.REVOKED, (0,))
    ]
    assert settlement.credits == {} and settlement.penalties == {"owner-a": 1}


def test_withdrawing_gives_the_honest_holder_a_fresh_window() -> None:
    clock, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    k2 = link(coord, unit, None, 2, a, state=b"forged")
    coord.commit("a", a.token, 0, k2)
    coord.leave(sign_leave(notice(a, KEYS["a"]), KEYS["a"]))  # b takes the shard over
    keep_alive(clock, coord, {"b": b}, 500)
    coord.commit("b", b.token, 0, link(coord, unit, k2, 4, b))  # t = 500; its window ends at 4_500
    keep_alive(clock, coord, {"b": b}, PROGRESS - 500)
    coord.withdraw(k2)  # t = 4_000, long after the forged link: b has to restore from the base
    keep_alive(clock, coord, {"b": b}, PROGRESS - 500)  # t = 7_500, inside the fresh window
    assert coord.holder(0) == Holder("b", b.token)
    assert [d.kind for d in coord.settle().departures] == [Exit.NOTICE]


def test_withdraw_lapses_the_expired_before_it_revokes() -> None:
    clock, coord, (unit,) = setup()
    a = join(coord, "a")
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)
    clock.advance(TTL)  # a's lease ran out and nothing has swept it yet
    coord.withdraw(k3)
    assert [(d.member, d.kind) for d in coord.settle().departures] == [("a", Exit.LAPSE)]
    assert coord.settle().penalties == {"owner-a": 1}


def test_only_an_accepted_link_of_the_open_round_can_be_withdrawn() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    k6 = link(coord, unit, None, 6, a)
    coord.commit("a", a.token, 0, k6)
    with pytest.raises(CoordinatorError) as unknown:
        coord.withdraw(hashlib.sha256(b"never accepted").hexdigest())
    assert unknown.value.code == "UNKNOWN_LINK"
    coord.open_round([WorkUnit(RUN, POLICY, 1, 0, unit.base, 6)])
    with pytest.raises(CoordinatorError) as closed:
        coord.withdraw(k6)
    assert closed.value.code == "ROUND_CLOSED"
    assert coord.settle().penalties == {}


def test_a_replacement_after_a_link_restarts_the_window_once() -> None:
    clock, coord, (unit,) = setup()
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}
    keep_alive(clock, coord, leases, 500)
    coord.commit("a", leases["a"].token, 0, link(coord, unit, None, 3, leases["a"]))  # t = 500
    keep_alive(clock, coord, leases, PROGRESS - 1_000)
    leases["a"] = coord.join("a", "owner-a", KEYS["c"].ss58).lease  # a new host at t = 3_500
    keep_alive(clock, coord, leases, PROGRESS - 500)  # t = 7_000, inside the window it opened
    assert coord.holder(0) == Holder("a", leases["a"].token)
    keep_alive(clock, coord, leases, 500)  # t = 7_500: that window is over
    assert [d.kind for d in coord.settle().departures if d.member == "a"] == [
        Exit.REPLACED,
        Exit.STALLED,
    ]


@pytest.mark.parametrize(
    ("member", "signer", "code"), [("b", "b", "STALE_TOKEN"), ("a", "c", "LEFT")]
)
def test_a_leave_replay_is_answered_only_to_the_member_and_key_that_left(
    member: str, signer: str, code: str
) -> None:
    _, coord, _ = setup()
    a, b = join(coord, "a"), join(coord, "b")
    coord.leave(sign_leave(notice(a, KEYS["a"]), KEYS["a"]))
    claim = notice(a, KEYS[signer]).model_copy(update={"member": member})  # a's lease, others' name
    with pytest.raises(LeaseError) as e:
        coord.leave(sign_leave(claim, KEYS[signer]))
    assert e.value.code == code
    assert coord.holder(0) == Holder("b", b.token)


def test_a_round_number_is_opened_once() -> None:
    _, coord, (unit,) = setup()
    a = join(coord, "a")
    coord.commit("a", a.token, 0, link(coord, unit, None, 6, a))
    with pytest.raises(ValueError):
        coord.open_round([WorkUnit(RUN, POLICY, 0, 0, unit.base, 6)])  # round 0 again
    coord.open_round([WorkUnit(RUN, POLICY, 1, 0, unit.base, 6)])
    coord.commit(
        "a", a.token, 0, link(coord, WorkUnit(RUN, POLICY, 1, 0, unit.base, 6), None, 6, a)
    )
    for again in (0, 1):
        with pytest.raises(ValueError):
            coord.open_round([WorkUnit(RUN, POLICY, again, 0, unit.base, 6)])
    assert coord.round == 1


def test_stalled_members_end_in_id_order() -> None:
    clock, coord, _ = setup(shards=12)
    names = [f"m{i:02d}" for i in range(11, -1, -1)]  # joined in the reverse of id order
    leases = {n: coord.join(n, "owner", KEYS["a"].ss58).lease for n in names}
    assert len({coord.holder(s).member for s in range(12)}) == 12  # one shard each
    keep_alive(clock, coord, leases, PROGRESS)  # nobody ever commits
    ended = [d.member for d in coord.settle().departures if d.kind is Exit.STALLED]
    assert ended == sorted(names)  # a set of 12 ids almost never iterates in id order


def test_expired_leases_lapse_in_id_order() -> None:
    clock, coord, _ = setup(shards=3)
    for name in "cab":  # joined in the reverse of id order
        join(coord, name)
    clock.advance(TTL)
    coord.tick()
    assert [(d.member, d.kind) for d in coord.settle().departures] == [
        ("a", Exit.LAPSE),
        ("b", Exit.LAPSE),
        ("c", Exit.LAPSE),
    ]


def test_settle_lapses_the_expired_before_it_reports() -> None:
    clock, coord, _ = setup()
    join(coord, "a")
    clock.advance(TTL)  # no other call: settle() alone has to notice
    assert [(d.member, d.kind) for d in coord.settle().departures] == [("a", Exit.LAPSE)]


def test_a_live_member_cannot_extend_a_shard_nobody_holds() -> None:
    _, coord, units = setup(shards=2)
    a = join(coord, "a")  # capacity 1: it holds one shard and the other waits for a member
    waiting = next(s for s in (0, 1) if coord.holder(s) is None)
    with pytest.raises(CoordinatorError) as e:
        coord.commit("a", a.token, waiting, link(coord, units[waiting], None, 3, a))
    assert e.value.code == "NOT_HOLDER"
    assert coord.settle().credits == {} and coord.holder(waiting) is None


def test_a_shard_its_busy_holder_never_works_goes_to_a_member_that_holds_nothing() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a")}
    assert coord.holder(0) == coord.holder(1) == Holder("a", leases["a"].token)
    leases["b"] = join(coord, "b")  # holds nothing
    head = None
    for step in (1, 2):  # a advances shard 0 every 2 s and never touches shard 1
        keep_alive(clock, coord, leases, 2_000)
        head = link(coord, units[0], head, step, leases["a"])
        coord.commit("a", leases["a"].token, 0, head)
    keep_alive(clock, coord, leases, 500)
    assert coord.holder(0) == Holder("a", leases["a"].token)
    assert coord.holder(1) == Holder("b", leases["b"].token)
    settlement = coord.settle()
    assert [(m.shard, m.from_member, m.to_member, m.at_ms) for m in settlement.moves] == [
        (1, "a", "b", PROGRESS)  # one window after a got it
    ]
    assert settlement.moves[0].from_token == leases["a"].token  # the simulator keys on it
    assert settlement.departures == () and settlement.penalties == {}  # nobody is stalled or fined
    for step in (3, 4):  # b never advances shard 1: its window, opened at the hand-over, runs out
        keep_alive(clock, coord, leases, 2_000 if step == 4 else 1_500)
        head = link(coord, units[0], head, step, leases["a"])
        coord.commit("a", leases["a"].token, 0, head)
    assert [(d.member, d.kind, d.at_ms) for d in coord.settle().departures] == [
        ("b", Exit.STALLED, 2 * PROGRESS)
    ]
    assert coord.holder(1) == Holder("a", leases["a"].token)  # and the shard goes back to a


def test_a_member_that_advances_both_its_shards_keeps_them() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    clock.advance(10_000)  # the round has been open for a while when the hosts arrive
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}
    heads: dict[int, str | None] = {0: None, 1: None}
    for step in (1, 2, 3):
        keep_alive(clock, coord, leases, 2_000)
        for shard in (0, 1):
            heads[shard] = link(coord, units[shard], heads[shard], step, leases["a"])
            coord.commit("a", leases["a"].token, shard, heads[shard])
    holders = {coord.holder(s) for s in (0, 1) if coord.holder(s)}
    assert holders == {Holder("a", leases["a"].token)}
    assert coord.settle().moves == ()


def test_shards_do_not_pass_between_busy_members() -> None:
    clock, coord, units = setup(shards=3, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # one holds two shards, one holds one
    held = {n: [s for s in range(3) if coord.holder(s).member == n] for n in "ab"}
    assert sorted(len(shards) for shards in held.values()) == [1, 2]
    heads: dict[str, str | None] = {"a": None, "b": None}
    for step in (1, 2):  # each works the first of its shards; the second shard of the pair is idle
        keep_alive(clock, coord, leases, 2_000)
        for name in "ab":
            shard = held[name][0]
            heads[name] = link(coord, units[shard], heads[name], step, leases[name])
            coord.commit(name, leases[name].token, shard, heads[name])
    keep_alive(clock, coord, leases, 500)
    assert {n: [s for s in range(3) if coord.holder(s).member == n] for n in "ab"} == held
    assert coord.settle().moves == () and coord.settle().departures == ()


def test_a_shard_is_not_taken_from_a_member_that_holds_nothing_else() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    keep_alive(clock, coord, leases, 500)
    coord.commit("a", leases["a"].token, 0, link(coord, units[0], None, 6, leases["a"]))  # done
    keep_alive(clock, coord, leases, PROGRESS - 500)  # shard 1 has waited a whole window for a
    assert coord.holder(1) == Holder("a", leases["a"].token)
    assert coord.settle().moves == ()
    assert coord.commit("a", leases["a"].token, 1, link(coord, units[1], None, 3, leases["a"]))


def test_a_rejoin_does_not_refresh_the_clock_of_a_shard_nobody_works() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    keep_alive(clock, coord, leases, 2_000)
    coord.commit("a", leases["a"].token, 0, link(coord, units[0], None, 1, leases["a"]))
    keep_alive(clock, coord, leases, 1_500)
    leases["a"] = coord.join("a", "owner-a", KEYS["c"].ss58).lease  # t = 3_500, a new host
    keep_alive(clock, coord, leases, 500)  # t = 4_000: shard 1 has waited a window for a
    assert [(m.shard, m.at_ms) for m in coord.settle().moves] == [(1, PROGRESS)]
    assert coord.holder(0) == Holder("a", leases["a"].token)  # the shard it works stays


def test_a_member_that_holds_nothing_is_handed_one_shard_at_a_time() -> None:
    clock, coord, units = setup(shards=3, capacity=3)
    leases = {"a": join(coord, "a")}
    assert {coord.holder(s) for s in range(3)} == {Holder("a", leases["a"].token)}
    leases["b"] = join(coord, "b")
    head = None
    for step in (1, 2):  # a works shard 0 only: shards 1 and 2 are both a window old
        keep_alive(clock, coord, leases, 2_000)
        head = link(coord, units[0], head, step, leases["a"])
        coord.commit("a", leases["a"].token, 0, head)
    assert [(m.shard, m.to_member) for m in coord.settle().moves] == [(1, "b")]
    assert coord.holder(2) == Holder("a", leases["a"].token)  # b is busy with shard 1 now


def test_a_hand_over_takes_a_queued_shard_never_the_one_its_holder_works() -> None:
    clock, coord, units = setup(shards=3, capacity=3)
    leases = {name: join(coord, name) for name in "abc"}  # a holds all three, b and c nothing
    keep_alive(clock, coord, leases, 2_000)
    coord.commit("a", leases["a"].token, 0, link(coord, units[0], None, 6, leases["a"]))  # done
    keep_alive(clock, coord, leases, 2_000)  # t = 4_000: shards 1 and 2 are a window old
    # a works its shards in ascending order, so it is on shard 1 and shard 2 is queued behind it
    assert [m.shard for m in coord.settle().moves] == [2]
    assert coord.holder(1) == Holder("a", leases["a"].token)


def test_a_host_that_works_its_second_shard_first_loses_the_first() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    head = None
    for step in (1, 2):  # a advances shard 1 and never touches shard 0, which comes first
        keep_alive(clock, coord, leases, 2_000)
        head = link(coord, units[1], head, step, leases["a"])
        coord.commit("a", leases["a"].token, 1, head)
    assert [(m.shard, m.to_member) for m in coord.settle().moves] == [(0, "b")]


def test_a_hand_over_goes_to_the_rendezvous_winner_among_the_idle() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {name: join(coord, name) for name in "abcd"}  # a holds both; b, c and d are idle
    head = None
    for step in (1, 2):  # a works shard 0 only, so shard 1 is the idle one
        keep_alive(clock, coord, leases, 2_000)
        head = link(coord, units[0], head, step, leases["a"])
        coord.commit("a", leases["a"].token, 0, head)
    # the winner for shard 1 of round 0 is neither the first idle member by id or by token, nor
    # the last, and a plan that dropped the round from the seed would pick b
    assert [(m.shard, m.to_member) for m in coord.settle().moves] == [(1, "c")]


def test_a_handed_over_shard_gets_a_fresh_clock() -> None:
    clock, coord, units = setup(shards=3, capacity=2)

    def host(name: str) -> Lease:
        return coord.join(name, f"owner-{name}", KEYS["a"].ss58).lease

    leases = {name: host(name) for name in "cab"}  # c holds shards 0 and 1, a shard 2, b nothing
    assert [coord.holder(shard).member for shard in range(3)] == ["c", "c", "a"]
    heads: dict[int, str | None] = {0: None, 2: None}
    for step in (1, 2):  # c works shard 0 and a works shard 2, so shard 1 waits
        keep_alive(clock, coord, leases, 2_000)
        for name, shard in (("c", 0), ("a", 2)):
            heads[shard] = link(coord, units[shard], heads[shard], step, leases[name])
            coord.commit(name, leases[name].token, shard, heads[shard])
    assert [(m.shard, m.to_member) for m in coord.settle().moves] == [(1, "b")]  # t = 4_000
    clock.advance(500)
    leases["d"] = host("d")  # a second idle member arrives
    coord.leave(sign_leave(notice(leases["a"], KEYS["a"]), KEYS["a"]))  # shard 2 goes to b
    assert coord.holder(2) == Holder("b", leases["b"].token)
    coord.commit("b", leases["b"].token, 2, link(coord, units[2], heads[2], 3, leases["b"]))
    coord.heartbeat("b", leases["b"].token)  # b advanced shard 2; d is idle; shard 1 is 500 ms old
    assert [m.shard for m in coord.settle().moves] == [1]  # so shard 1 is not handed on again


def test_another_member_cannot_have_a_link_it_did_not_produce_acknowledged() -> None:
    _, coord, (unit,) = setup()
    a, b = join(coord, "a"), join(coord, "b")
    k3 = link(coord, unit, None, 3, a)
    coord.commit("a", a.token, 0, k3)
    with pytest.raises(CoordinatorError) as e:
        coord.commit("b", b.token, 0, k3)  # b retries a's key: that is no retry of b's
    assert e.value.code == "NOT_HOLDER"


def test_a_round_reports_only_its_own_hand_overs() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    keep_alive(clock, coord, leases, 2_000)
    first = link(coord, units[0], None, 3, leases["a"])
    coord.commit("a", leases["a"].token, 0, first)  # a works shard 0
    keep_alive(clock, coord, leases, 2_000)  # t = 4_000: shard 1 goes to b
    coord.commit("a", leases["a"].token, 0, link(coord, units[0], first, 6, leases["a"]))
    coord.commit("b", leases["b"].token, 1, link(coord, units[1], None, 6, leases["b"]))
    closed = coord.open_round([WorkUnit(RUN, POLICY, 1, 0, units[0].base, 6)])
    assert closed is not None and [m.shard for m in closed.moves] == [1]
    assert coord.settle().moves == ()  # the next round starts with none


def test_a_host_that_loses_a_shard_to_a_hand_over_gets_a_fresh_window() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    keep_alive(clock, coord, leases, 2_000)
    coord.commit("a", leases["a"].token, 0, link(coord, units[0], None, 3, leases["a"]))
    keep_alive(
        clock, coord, leases, 2_000
    )  # t = 4_000: shard 1 goes to b; a's window ends at 6_000
    assert [m.shard for m in coord.settle().moves] == [1]
    keep_alive(clock, coord, leases, 3_000)  # t = 7_000: no link since t = 2_000, and no stall
    assert coord.holder(0) == Holder("a", leases["a"].token)
    assert coord.settle().departures == ()  # a lost work it did not choose to lose


def test_a_host_never_loses_its_last_shard_to_the_hand_overs_of_one_sweep() -> None:
    # Nobody calls between t = 1_000 and t = 5_500, so both shards are a window old at the sweep.
    clock, coord, units = setup(shards=2, capacity=2, ttl_ms=10 * PROGRESS)
    a = join(coord, "a")
    join(coord, "b"), join(coord, "c")  # a holds both shards; b and c hold nothing
    clock.advance(1_000)
    coord.commit("a", a.token, 1, link(coord, units[1], None, 1, a))  # a advances its second shard
    clock.advance(2_500)
    a = coord.join("a", "owner-a", KEYS["d"].ss58).lease  # t = 3_500: a new host, a fresh window
    clock.advance(2_000)  # t = 5_500
    assert [m.shard for m in coord.settle().moves] == [0]  # the idle shard goes, one for each call
    assert coord.holder(1) == Holder("a", a.token)  # and the one a works stays, though it is old


def test_a_shard_a_stall_frees_goes_to_the_idle_member_not_to_a_hand_over() -> None:
    clock, coord, units = setup(shards=3, capacity=2)
    leases = {"a": join(coord, "a"), "d": join(coord, "d")}  # a holds two shards, d the third
    leases["b"] = join(coord, "b")  # and b nothing
    mine = [s for s in range(3) if coord.holder(s).member == "a"]
    freed = next(s for s in range(3) if s not in mine)
    keep_alive(clock, coord, leases, 2_000)
    coord.commit("a", leases["a"].token, mine[0], link(coord, units[mine[0]], None, 1, leases["a"]))
    keep_alive(clock, coord, leases, 2_000)  # t = 4_000: d never linked and stalls; a's second
    settlement = coord.settle()  # shard is a window old, and b is idle until it takes d's shard
    assert [(d.member, d.kind) for d in settlement.departures] == [("d", Exit.STALLED)]
    assert coord.holder(freed) == Holder("b", leases["b"].token)
    assert settlement.moves == ()


@pytest.mark.parametrize(
    ("linked", "stalled_at"), [(False, 2 * PROGRESS), (True, 2 * PROGRESS + 1_000)]
)
def test_the_window_a_hand_over_gives_is_not_renewed_by_a_replacement(
    linked: bool, stalled_at: int
) -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    keep_alive(clock, coord, leases, 2_000)
    head = link(coord, units[0], None, 3, leases["a"])
    coord.commit("a", leases["a"].token, 0, head)
    keep_alive(clock, coord, leases, 2_000)  # t = 4_000: shard 1 goes to b, a's window restarts
    assert [m.shard for m in coord.settle().moves] == [1]
    if linked:  # a link opens a window the next replacement may renew again
        keep_alive(clock, coord, leases, 500)
        coord.commit("a", leases["a"].token, 0, link(coord, units[0], head, 4, leases["a"]))
    keep_alive(clock, coord, leases, 500 if linked else 1_000)
    leases["a"] = coord.join("a", "owner-a", KEYS["c"].ss58).lease  # t = 5_000
    leases["b"] = coord.join("b", "owner-b", KEYS["d"].ss58).lease  # b never linked: it renews
    keep_alive(clock, coord, leases, 4_000)
    stalled = [(d.member, d.at_ms) for d in coord.settle().departures if d.kind is Exit.STALLED]
    assert stalled == [("a", stalled_at), ("b", 9_000)]  # b's window ran from t = 4_000 + 1_000


def test_a_withdrawn_link_is_no_advance_for_the_hand_over_rule_or_the_clock() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    keep_alive(clock, coord, leases, 500)
    forged = link(coord, units[1], None, 2, leases["a"], state=b"forged")
    coord.commit("a", leases["a"].token, 1, forged)
    coord.leave(sign_leave(notice(leases["a"], KEYS["a"]), KEYS["a"]))  # t = 500: b takes both
    assert coord.holder(0) == coord.holder(1) == Holder("b", leases["b"].token)
    keep_alive(clock, coord, leases, 500)
    coord.commit(
        "b", leases["b"].token, 1, link(coord, units[1], forged, 4, leases["b"])
    )  # t = 1_000
    keep_alive(clock, coord, leases, 2_500)
    coord.withdraw(forged)  # t = 3_500: b advanced nothing that counts, and starts shard 1 again
    leases["c"] = join(coord, "c")  # an idle member
    keep_alive(clock, coord, leases, 1_500)  # t = 5_000: shard 0 is 4_500 ms old, shard 1 1_500
    assert coord.settle().moves == ()  # shard 0 is b's first and b advanced nothing since
    assert (
        coord.holder(0) == coord.holder(1) == Holder("b", leases["b"].token)
    )  # not stalled either


def test_a_shard_released_by_another_host_does_not_carry_that_hosts_link_time() -> None:
    clock, coord, units = setup(shards=2, capacity=2)
    leases = {"a": join(coord, "a"), "b": join(coord, "b")}  # a holds both, b nothing
    keep_alive(clock, coord, leases, 1_000)
    head = link(coord, units[1], None, 1, leases["a"])
    coord.commit("a", leases["a"].token, 1, head)  # a works its second shard
    keep_alive(clock, coord, leases, 3_000)  # t = 4_000: shard 0 goes to b
    assert [(m.shard, m.to_member) for m in coord.settle().moves] == [(0, "b")]
    keep_alive(clock, coord, leases, 500)
    coord.commit(
        "a", leases["a"].token, 1, link(coord, units[1], head, 2, leases["a"])
    )  # t = 4_500
    keep_alive(clock, coord, leases, 500)
    coord.leave(sign_leave(notice(leases["a"], KEYS["a"]), KEYS["a"]))  # t = 5_000: b gets shard 1
    assert coord.holder(1) == Holder("b", leases["b"].token)
    keep_alive(clock, coord, leases, 500)
    leases["c"] = join(coord, "c")  # an idle member
    keep_alive(clock, coord, leases, 1_500)
    leases["b"] = coord.join("b", "owner-b", KEYS["d"].ss58).lease  # t = 7_000: a window for b
    keep_alive(
        clock, coord, leases, 1_000
    )  # t = 8_000: shard 0 is a window old for the second time
    # b advanced nothing since it got shard 0: a's link on shard 1 is not b's
    assert [(m.shard, m.at_ms) for m in coord.settle().moves] == [(0, PROGRESS)]


def test_an_advance_in_the_millisecond_a_shard_was_got_does_not_free_it() -> None:
    clock, coord, units = setup(shards=2, capacity=2, ttl_ms=10 * PROGRESS)
    a = join(coord, "a")
    join(coord, "b")  # a holds both shards, b nothing
    coord.commit(
        "a", a.token, 1, link(coord, units[1], None, 1, a)
    )  # in the millisecond it got both
    clock.advance(3_500)
    coord.join("a", "owner-a", KEYS["c"].ss58)  # a new host: a fresh window
    clock.advance(500)  # t = 4_000: both shards are a window old
    assert [m.shard for m in coord.settle().moves] == [1]  # shard 0 was advanced by nothing later
