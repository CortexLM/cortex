"""Elastic membership: hosts join, renew, leave or lapse under leases with fencing tokens.

A member is one host slot of a miner; an 8-GPU host is one unit of membership. A lease is live
while `now < expires_ms` on the coordinator's clock, and a heartbeat renews it. Every grant
takes its token from one counter that only grows, so a rejoin or a replacement host fences every
older holder: the old token is refused from then on, even if its host is still running. The
counter is saved (TokenLog) before a lease is returned, so it also outlives a restart of the
coordinator: no token is issued twice, and a signed leave names one lease for ever. A lease also
carries the epoch of the start that granted it, drawn at random and never kept in the log, so a
log restored from an older copy cannot make an old leave name a new lease.
"""

from __future__ import annotations

import secrets
from dataclasses import dataclass
from enum import StrEnum

from hypertrain.elastic.clock import Clock
from hypertrain.elastic.tokens import TokenLog


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
    epoch: str  # the epoch of the coordinator that granted it: a leave must name it


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
    def __init__(self, clock: Clock, ttl_ms: int, tokens: TokenLog) -> None:
        if ttl_ms <= 0:
            raise ValueError("ttl_ms must be > 0")
        self.clock = clock
        self.ttl_ms = ttl_ms
        self._members: dict[str, Member] = {}
        self._log = tokens
        self._tokens = tokens.load()
        # Drawn at every start and never read back from the log: a log restored from an older
        # copy hands out tokens an earlier start already delivered, but not its epoch.
        self.epoch = secrets.token_hex(32)

    def _live(self, member: Member) -> bool:
        return member.status is Status.ACTIVE and self.clock.now_ms() < member.lease.expires_ms

    def _grant(self, member: str) -> Lease:
        token = self._tokens + 1
        self._tokens = token  # spent even if the save fails: it may have become durable
        self._log.save(token)  # before the lease exists: a token that was lost is never reissued
        return Lease(member, token, self.clock.now_ms() + self.ttl_ms, self.epoch)

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
        m.lease = Lease(member, token, self.clock.now_ms() + self.ttl_ms, self.epoch)
        return m.lease

    def leave(self, member: str, token: int) -> Member:
        m = self.check(member, token)
        m.status = Status.LEFT
        return m

    def lapse(self, member: str) -> Member:
        """End a live lease now, as if it had run out; the member may join again."""
        m = self._members[member]
        m.status = Status.LAPSED
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
