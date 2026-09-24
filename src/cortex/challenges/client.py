"""Read container weights once per completed epoch and turn them into leaf scores."""

from __future__ import annotations

import asyncio
import json
import math
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path

import httpx

from cortex.errors import ServiceError
from cortex.http import read_private_file
from cortex.protocol.crypto import decode_hotkey
from cortex.protocol.models import FULL_SHARE_SCORE, NoScore, NoScoreReason, Score

from .registry import RegistryEntry

MAX_RESPONSE_BYTES = 8 * 1024 * 1024
MAX_WEIGHTS = 65536
ATTEMPTS = 3


@dataclass(frozen=True)
class ChallengeWeights:
    weights: dict[bytes, Fraction]
    full_share_mass: Fraction | None = None


def parse_weights(body: bytes, *, slug: str, epoch: int) -> ChallengeWeights:
    def finite(value: object) -> Fraction:
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError("weight must be a number")
        if not math.isfinite(value) or value < 0:
            raise ValueError("weight must be finite and non-negative")
        return Fraction(value)

    document = json.loads(body, parse_constant=lambda _: math.nan)
    if (
        not isinstance(document, dict)
        or document.get("challenge_slug") != slug
        or document.get("epoch") != epoch
        or not isinstance(document.get("weights"), dict)
        or len(document["weights"]) > MAX_WEIGHTS
    ):
        raise ValueError("weights response does not match the requested challenge epoch")
    weights: dict[bytes, Fraction] = {}
    for key, value in document["weights"].items():
        hotkey = decode_hotkey(key)
        if hotkey in weights:
            raise ValueError("duplicate hotkey in weights response")
        weights[hotkey] = finite(value)
    mass = document.get("full_share_mass")
    return ChallengeWeights(weights, None if mass is None else finite(mass))


def leaf_scores(
    answer: ChallengeWeights, expected: set[bytes], *, algorithm_version: int
) -> dict[bytes, Score | NoScore]:
    """Exact-E scores. Hotkeys outside E are ignored and never change the denominator."""
    kept = {key: value for key, value in answer.weights.items() if key in expected and value > 0}
    if algorithm_version == 3:
        denominator = max(sum(kept.values(), Fraction(0)), answer.full_share_mass or Fraction(0))
        raw = {
            key: math.floor(FULL_SHARE_SCORE * value / denominator) for key, value in kept.items()
        }
    else:
        # Algorithms 1 and 2 sign raw integer counts; the protocol applies the Bounty cap.
        if any(value.denominator != 1 for value in kept.values()):
            raise ValueError("algorithm 1 and 2 weights must be integers")
        raw = {key: int(value) for key, value in kept.items()}
    return {
        key: Score(raw[key]) if raw.get(key, 0) > 0 else NoScore(NoScoreReason.NOT_ATTEMPTED)
        for key in expected
    }


class ChallengeClient:
    """The master's only call into a container; internal tokens never leave this process."""

    def __init__(self, http: httpx.AsyncClient, secrets_dir: Path, *, retry_seconds: float = 5):
        self.http, self.secrets_dir, self.retry_seconds = http, secrets_dir, retry_seconds

    async def weights(self, entry: RegistryEntry, epoch: int) -> ChallengeWeights:
        token = read_private_file(self.secrets_dir / entry.id / "internal.token")
        for attempt in range(ATTEMPTS):
            try:
                async with self.http.stream(
                    "GET",
                    f"{entry.url}/internal/v1/get_weights",
                    params={"epoch": str(epoch)},
                    headers={
                        "authorization": f"Bearer {token}",
                        "x-platform-challenge-slug": entry.id,
                    },
                    timeout=60,
                ) as response:
                    if response.status_code != 200:
                        raise ServiceError(503, f"challenge answered HTTP {response.status_code}")
                    body = bytearray()
                    async for chunk in response.aiter_bytes():
                        body.extend(chunk)
                        if len(body) > MAX_RESPONSE_BYTES:
                            raise ServiceError(503, "challenge weights response too large")
                return parse_weights(bytes(body), slug=entry.id, epoch=epoch)
            except httpx.TransportError:
                # A container restarting for an update must not burn a whole epoch.
                if attempt + 1 == ATTEMPTS:
                    raise
                await asyncio.sleep(self.retry_seconds * (attempt + 1))
        raise AssertionError("unreachable")
