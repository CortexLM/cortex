"""Signed, digest-pinned OpenRouter judge configuration; credentials stay separate."""

from __future__ import annotations

import hashlib
import time
from pathlib import Path
from typing import Literal

from pydantic import Field, model_validator

from cortex.protocol.crypto import public_key, sign_raw, verify_raw

from .models import AgentLimits, Digest, StrictModel, canonical_bytes, safe_model_id
from .provider import Completion, ModelProvider

OFFER_DOMAIN = b"cortex-inference-offer-v1"


class InferenceOffer(StrictModel):
    schema_version: Literal[1] = 1
    provider: Literal["openrouter"] = "openrouter"
    endpoint: Literal["https://openrouter.ai/api/v1/chat/completions"] = (
        "https://openrouter.ai/api/v1/chat/completions"
    )
    model: str
    limits: AgentLimits
    issuer_public_key: Digest
    status: Literal["open", "closed"]
    valid_from_unix: int = Field(ge=0)
    valid_until_unix: int = Field(gt=0)
    signature: str = Field(pattern=r"^[0-9a-f]{128}$")

    @model_validator(mode="after")
    def validate_configuration(self) -> InferenceOffer:
        safe_model_id(self.model)
        if self.valid_until_unix <= self.valid_from_unix:
            raise ValueError("invalid inference offer validity window")
        return self

    def signing_payload(self) -> bytes:
        return canonical_bytes(self.model_dump(mode="json", exclude={"signature"}))

    def commitment(self) -> str:
        return hashlib.sha256(self.signing_payload()).hexdigest()

    def verify(self, expected_commitment: str, *, now: float | None = None) -> None:
        if self.commitment() != expected_commitment:
            raise ValueError("inference offer commitment mismatch")
        if not verify_raw(
            bytes.fromhex(self.issuer_public_key),
            OFFER_DOMAIN,
            self.signing_payload(),
            bytes.fromhex(self.signature),
        ):
            raise ValueError("inference offer signature invalid")
        moment = time.time() if now is None else now
        if self.status != "open" or not self.valid_from_unix <= moment < self.valid_until_unix:
            raise ValueError("inference offer closed or outside validity window")

    def verify_runtime(self, model: str, limits: AgentLimits, expected_commitment: str) -> None:
        self.verify(expected_commitment)
        if self.model != model or self.limits != limits:
            raise ValueError("inference runtime differs from pinned offer")

    @classmethod
    def load(cls, path: Path, expected_commitment: str) -> InferenceOffer:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 16384:
            raise ValueError("inference offer must be a bounded regular file")
        offer = cls.model_validate_json(path.read_bytes())
        offer.verify(expected_commitment)
        return offer


def sign_offer(offer: InferenceOffer, seed: bytes) -> InferenceOffer:
    if public_key(seed).hex() != offer.issuer_public_key:
        raise ValueError("inference offer issuer key mismatch")
    return offer.model_copy(
        update={"signature": sign_raw(seed, OFFER_DOMAIN, offer.signing_payload()).hex()}
    )


class OfferBoundProvider:
    """Recheck offer validity before every paid request, including long jobs."""

    def __init__(self, provider: ModelProvider, offer: InferenceOffer, commitment: str) -> None:
        offer.verify_runtime(provider.model, offer.limits, commitment)
        self.provider, self.offer, self.commitment = provider, offer, commitment
        self.model = provider.model
        self.api_key_file = getattr(provider, "api_key_file", None)

    async def complete(self, **kwargs) -> Completion:
        self.offer.verify_runtime(self.provider.model, self.offer.limits, self.commitment)
        return await self.provider.complete(**kwargs)
