"""Public gateway JSON contracts, mapped to the signed SCALE leaf payload."""

from typing import Annotated

from pydantic import BaseModel, ConfigDict, Field

from cortex.errors import ServiceError
from cortex.protocol import Leaf, NoScore, NoScoreReason, Score

U64 = Annotated[int, Field(ge=0, le=2**64 - 1)]


class RequestModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class ScoreValue(RequestModel):
    value: U64


class AbsenceValue(RequestModel):
    reason: int = Field(ge=0, le=7)


class ScoreWire(RequestModel):
    score: ScoreValue


class AbsenceWire(RequestModel):
    no_score: AbsenceValue


def _hex(value: str, size: int) -> bytes:
    normalized = value.strip()
    if normalized.startswith(("0x", "0X")):
        normalized = normalized[2:]
    if len(normalized) != size * 2:
        raise ServiceError(400, "invalid raw weight hex length")
    try:
        raw = bytes.fromhex(normalized)
    except ValueError:
        raise ServiceError(400, "invalid raw weight hex") from None
    if len(raw) != size:
        raise ServiceError(400, "invalid raw weight hex length")
    return raw


class RawWeightRequest(RequestModel):
    challenge_id: str = Field(min_length=1, max_length=64)
    miner_hotkey: str = Field(max_length=132)
    epoch: U64
    score_or_absence: ScoreWire | AbsenceWire
    challenge_sig: str = Field(max_length=132)

    def to_leaf(self) -> Leaf:
        value = self.score_or_absence
        score = (
            Score(value.score.value)
            if isinstance(value, ScoreWire)
            else NoScore(NoScoreReason(value.no_score.reason))
        )
        return Leaf(
            self.challenge_id.encode(),
            _hex(self.miner_hotkey, 32),
            self.epoch,
            score,
            _hex(self.challenge_sig, 64),
        )


class SealRequest(RequestModel):
    epoch: U64
    netuid: int | None = Field(default=None, ge=0, le=65535)
    block_b: U64 | None = None


def leaf_request(leaf: Leaf) -> dict:
    """Build the existing raw-weight wire JSON for challenge emitters."""
    score = (
        {"score": {"value": leaf.score.value}}
        if isinstance(leaf.score, Score)
        else {"no_score": {"reason": int(leaf.score.reason)}}
    )
    return dict(
        challenge_id=leaf.challenge_id.decode(),
        miner_hotkey=leaf.miner_hotkey.hex(),
        epoch=leaf.epoch,
        score_or_absence=score,
        challenge_sig=leaf.challenge_sig.hex(),
    )
