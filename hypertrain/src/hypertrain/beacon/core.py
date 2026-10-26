"""Beacon interface, push+verify ingestion, and a deterministic fixture beacon for tests."""

from __future__ import annotations

import hashlib
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any, Protocol


class BeaconError(Exception):
    """Base class for beacon failures."""


class BeaconUnavailable(BeaconError):  # noqa: N818 - reads as a state, like KeyError
    """The requested round is not (yet) known to this beacon."""


class BeaconVerificationError(BeaconError):
    """A round failed signature, randomness, or cross-relay consistency checks."""


@dataclass(frozen=True)
class BeaconRound:
    round: int
    signature: str  # lowercase hex
    randomness: str  # lowercase hex, sha256(signature bytes) for drand unchained schemes
    bls_verified: bool  # True only when a BLS pairing check passed against the group key


def round_at_or_after(t: int, genesis: int, period: int) -> int:
    """First round whose emission time (genesis + (r-1)*period) is >= t. Integer math only."""
    if t <= genesis:
        return 1
    return -(-(t - genesis) // period) + 1


class Beacon(Protocol):
    def round_at_or_after(self, t: int) -> int: ...

    def get(self, round: int) -> BeaconRound: ...


class PushBeacon:
    """Container-side PUSH+VERIFY: the operator relay posts rounds, every one is verified here.

    No egress needed. `verify` is the scheme check (drand quicknet BLS by default).
    """

    def __init__(
        self,
        verify: Callable[[Mapping[str, Any]], BeaconRound],
        genesis: int,
        period: int,
    ) -> None:
        self._verify = verify
        self._genesis = genesis
        self._period = period
        self._rounds: dict[int, BeaconRound] = {}

    def ingest(self, payload: Mapping[str, Any]) -> BeaconRound:
        """Verify and store one pushed round; raises BeaconVerificationError on any mismatch."""
        br = self._verify(payload)
        known = self._rounds.get(br.round)
        if known is not None and known != br:
            raise BeaconVerificationError(f"conflicting payload for round {br.round}")
        self._rounds[br.round] = br
        return br

    def round_at_or_after(self, t: int) -> int:
        return round_at_or_after(t, self._genesis, self._period)

    def get(self, round: int) -> BeaconRound:
        try:
            return self._rounds[round]
        except KeyError:
            raise BeaconUnavailable(f"round {round} not pushed yet") from None


class FixtureBeacon:
    """Deterministic test beacon: randomness = sha256(seed | round); rounds advance explicitly."""

    def __init__(self, seed: bytes = b"hypertrain-fixture", current: int = 1, period: int = 3):
        if current < 1:
            raise ValueError("current round must be >= 1")
        self._seed = seed
        self.current = current
        self._period = period

    def advance(self, n: int = 1) -> int:
        if n < 0:
            raise ValueError("rounds never go backwards")
        self.current += n
        return self.current

    def round_at_or_after(self, t: int) -> int:
        return round_at_or_after(t, 0, self._period)

    def get(self, round: int) -> BeaconRound:
        if round < 1 or round > self.current:
            # A future round is unpredictable by contract; refuse instead of precomputing it.
            raise BeaconUnavailable(f"round {round} not emitted (current {self.current})")
        sig = hashlib.sha256(self._seed + b"|sig|" + str(round).encode()).digest()
        return BeaconRound(round, sig.hex(), hashlib.sha256(sig).hexdigest(), bls_verified=False)
