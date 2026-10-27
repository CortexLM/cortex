"""Leases: a member is live only while its newest token is renewed in time."""

import re

import pytest

from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.membership import LeaseError, Membership, Status
from hypertrain.elastic.tokens import MemoryTokenLog


def test_lease_lapses_without_a_heartbeat() -> None:
    clock = FakeClock()
    members = Membership(clock, ttl_ms=1_000, tokens=MemoryTokenLog())
    lease, _ = members.join("a", "owner-a", "signer-a")
    clock.advance(999)
    assert members.renew("a", lease.token).expires_ms == 1_999
    clock.advance(1_000)
    with pytest.raises(LeaseError) as e:
        members.check("a", lease.token)
    assert e.value.code == "LEASE_EXPIRED"
    assert [m.id for m in members.expire()] == ["a"]
    assert members.get("a").status is Status.LAPSED
    assert members.active() == []


def test_rejoin_fences_every_older_token() -> None:
    clock = FakeClock()
    members = Membership(clock, ttl_ms=1_000, tokens=MemoryTokenLog())
    first, _ = members.join("a", "owner-a", "host-1")
    second, replaced = members.join("a", "owner-a", "host-2")
    other, _ = members.join("b", "owner-b", "host-3")
    assert replaced == first
    assert first.token < second.token < other.token
    with pytest.raises(LeaseError) as e:
        members.renew("a", first.token)
    assert e.value.code == "STALE_TOKEN"
    assert members.check("a", second.token).signer == "host-2"


def test_a_member_id_has_one_owner_and_a_revoked_one_never_returns() -> None:
    members = Membership(FakeClock(), ttl_ms=1_000, tokens=MemoryTokenLog())
    members.join("a", "owner-a", "host-1")
    with pytest.raises(LeaseError) as e:
        members.join("a", "owner-b", "host-2")
    assert e.value.code == "OWNER_MISMATCH"
    members.revoke("a")
    with pytest.raises(LeaseError) as e:
        members.join("a", "owner-a", "host-1")
    assert e.value.code == "REVOKED"


def test_no_lease_is_granted_when_its_token_cannot_be_saved() -> None:
    class FullDisk(MemoryTokenLog):
        broken = True

        def save(self, token: int) -> None:
            if self.broken:
                raise OSError("disk full")
            super().save(token)

    log = FullDisk()
    members = Membership(FakeClock(), ttl_ms=1_000, tokens=log)
    with pytest.raises(OSError):
        members.join("a", "owner-a", "host-1")
    assert members.get("a") is None and members.active() == []
    log.broken = False
    lease, _ = members.join("a", "owner-a", "host-1")
    assert lease.token == 2 and log.load() == 2  # token 1 may have become durable: never reused


def test_every_start_draws_its_own_epoch_and_every_lease_carries_it() -> None:
    log = MemoryTokenLog()  # one log, two starts: the mark continues, the epoch does not
    first = Membership(FakeClock(), ttl_ms=1_000, tokens=log)
    lease, _ = first.join("a", "owner-a", "host-1")
    second = Membership(FakeClock(), ttl_ms=1_000, tokens=log)
    assert first.epoch != second.epoch and re.fullmatch(r"[0-9a-f]{64}", first.epoch)
    assert (lease.epoch, first.renew("a", lease.token).epoch) == (first.epoch, first.epoch)
    assert second.join("a", "owner-a", "host-1")[0].epoch == second.epoch
