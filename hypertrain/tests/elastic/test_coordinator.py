"""Coordinator: fenced holders, compare-and-set heads, idempotent retries, credit and penalties."""

import hashlib
from collections.abc import Callable

import pytest

from hypertrain.elastic.checkpoint import CheckpointError, CheckpointManifest, WorkUnit
from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.coordinator import Coordinator, CoordinatorError, Exit, Holder
from hypertrain.elastic.membership import Lease, LeaseError
from hypertrain.elastic.notice import LeaveNotice, NoticeError, SignedLeave, sign_leave
from hypertrain.elastic.store import MemoryCheckpointStore
from hypertrain.protocol.keys import Keypair

RUN = "run-1"
POLICY = hashlib.sha256(b"policy").hexdigest()
TTL = 1_000
PROGRESS = 4_000
KEYS = {name: Keypair(bytes([i + 1]) * 32) for i, name in enumerate("abcdz")}


def setup(
    shards: int = 1, audit: Callable[[WorkUnit, CheckpointManifest], bool] | None = None
) -> tuple[FakeClock, Coordinator, list[WorkUnit]]:
    clock = FakeClock()
    store = MemoryCheckpointStore()
    coord = Coordinator(
        run_id=RUN,
        clock=clock,
        store=store,
        ttl_ms=TTL,
        progress_ms=PROGRESS,
        seed=b"s",
        admit=lambda _member, _owner, signer: signer != KEYS["z"].ss58,  # z holds no grant
        audit=audit,
    )
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


def test_a_round_opens_only_once_every_shard_is_complete() -> None:
    _, coord, (unit,) = setup()
    with pytest.raises(CoordinatorError) as e:
        coord.open_round([WorkUnit(RUN, POLICY, 1, 0, unit.base, 6)])
    assert e.value.code == "ROUND_OPEN"
