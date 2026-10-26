"""Elastic membership: hosts join, renew, leave or lapse under leases with fencing tokens.

A member is one host slot of a miner; an 8-GPU host is one unit of membership. A lease is live
while `now < expires_ms` on the coordinator's clock, and a heartbeat renews it. Every grant
takes its token from one counter that only grows, so a rejoin or a replacement host fences every
older holder: the old token is refused from then on, even if its host is still running.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum

from hypertrain.elastic.clock import Clock


class LeaseError(Exception):
    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code


class Status(StrEnum):
    ACTIVE = "active"
    LEFT = "left"
    LAPSED = "lapsed"
    REVOKED = "revoked"


@dataclass(frozen=True, slots=True)
class Lease:
    member: str
    token: int
    expires_ms: int


@dataclass(slots=True)
class Member:
    """`owner` is the miner credited for the member's work; `signer` is the SS58 key that signs
    its leave notice (a coldkey-granted server key on the host)."""

    id: str
    owner: str
    signer: str
    lease: Lease
    status: Status
    since_ms: int


class Membership:
    def __init__(self, clock: Clock, ttl_ms: int) -> None:
        if ttl_ms <= 0:
            raise ValueError("ttl_ms must be > 0")
        self.clock = clock
        self.ttl_ms = ttl_ms
        self._members: dict[str, Member] = {}
        self._tokens = 0

    def _live(self, member: Member) -> bool:
        return member.status is Status.ACTIVE and self.clock.now_ms() < member.lease.expires_ms

    def _grant(self, member: str) -> Lease:
        self._tokens += 1
        return Lease(member, self._tokens, self.clock.now_ms() + self.ttl_ms)

    def join(self, member: str, owner: str, signer: str) -> tuple[Lease, Lease | None]:
        """Grant a fresh lease. Joining an id that is still active (its lease not yet swept by
        expire()) replaces that holder, returned second; its token is fenced from now on. A
        revoked id never rejoins."""
        old = self._members.get(member)
        if old is not None and old.owner != owner:
            raise LeaseError("OWNER_MISMATCH", member)
        if old is not None and old.status is Status.REVOKED:
            raise LeaseError("REVOKED", member)
        replaced = old.lease if old is not None and old.status is Status.ACTIVE else None
        lease = self._grant(member)
        self._members[member] = Member(
            member, owner, signer, lease, Status.ACTIVE, self.clock.now_ms()
        )
        return lease, replaced

    def check(self, member: str, token: int) -> Member:
        """The member, if `token` is its current lease and the lease is live."""
        m = self._members.get(member)
        if m is None:
            raise LeaseError("UNKNOWN_MEMBER", member)
        if token != m.lease.token:
            raise LeaseError("STALE_TOKEN", f"{member}: {token} != {m.lease.token}")
        if not self._live(m):
            raise LeaseError("LEASE_EXPIRED" if m.status is Status.ACTIVE else m.status.upper())
        return m

    def renew(self, member: str, token: int) -> Lease:
        m = self.check(member, token)
        m.lease = Lease(member, token, self.clock.now_ms() + self.ttl_ms)
        return m.lease

    def leave(self, member: str, token: int) -> Member:
        m = self.check(member, token)
        m.status = Status.LEFT
        return m

    def revoke(self, member: str) -> Member:
        m = self._members[member]
        m.status = Status.REVOKED
        return m

    def expire(self) -> list[Member]:
        """Lapse every active member whose lease has run out, in id order."""
        lapsed = [
            m
            for _, m in sorted(self._members.items())
            if m.status is Status.ACTIVE and not self._live(m)
        ]
        for m in lapsed:
            m.status = Status.LAPSED
        return lapsed

    def active(self) -> list[Member]:
        return [m for _, m in sorted(self._members.items()) if self._live(m)]

    def get(self, member: str) -> Member | None:
        return self._members.get(member)
