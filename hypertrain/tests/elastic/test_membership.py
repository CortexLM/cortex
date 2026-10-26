"""Leases: a member is live only while its newest token is renewed in time."""

import pytest

from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.membership import LeaseError, Membership, Status


def test_lease_lapses_without_a_heartbeat() -> None:
    clock = FakeClock()
    members = Membership(clock, ttl_ms=1_000)
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
    members = Membership(clock, ttl_ms=1_000)
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
    members = Membership(FakeClock(), ttl_ms=1_000)
    members.join("a", "owner-a", "host-1")
    with pytest.raises(LeaseError) as e:
        members.join("a", "owner-b", "host-2")
    assert e.value.code == "OWNER_MISMATCH"
    members.revoke("a")
    with pytest.raises(LeaseError) as e:
        members.join("a", "owner-a", "host-1")
    assert e.value.code == "REVOKED"
