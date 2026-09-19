"""Bounty intake and scoring service; backend availability gates every report."""

import hashlib
import hmac
import re
import time
from collections.abc import Callable, Sequence

from cortex.protocol.crypto import decode_hotkey, verify_substrate

from .backend import BackendUnavailable, PublicBackend
from .scoring import BountyScore
from .store import BountyStore, StoreError, normalize_text

TERMS_TEXT = (
    "By pairing a Bittensor hotkey to a Cortex Chat account for Bounty Challenge, you accept "
    "that this dedicated mining account, its logs, and its conversations may be used for research, "
    "to fix product and backend bugs, and to remunerate (or penalize) the bound miner hotkey. "
    "Do not pair a private personal account."
)
PAIR_GRANT_MAX_TTL_SECONDS = 300
_ACCOUNT_ID_PATTERN = re.compile(r"[A-Za-z0-9._:-]{1,128}")


def validate_account_id(account: str) -> None:
    if not _ACCOUNT_ID_PATTERN.fullmatch(account):
        raise StoreError(400, "invalid account_id")


def pair_payload(account: str, nonce: str, expiry: int) -> bytes:
    validate_account_id(account)
    if not re.fullmatch(r"[a-fA-F0-9]{16,64}", nonce):
        raise StoreError(400, "invalid nonce")
    if not 0 < expiry <= 2**64 - 1:
        raise StoreError(400, "invalid or expired pairing window")
    return f"cortex-bounty-v1|{account}|{nonce}|{expiry}".encode()


class BountyService:
    def __init__(
        self,
        store: BountyStore,
        backend: PublicBackend,
        *,
        session_secret: bytes,
        admin_tokens: Sequence[str] = (),
        admin_hashes: Sequence[str] = (),
        clock: Callable[[], float] = time.time,
    ):
        if len(session_secret) < 32:
            raise ValueError("Bounty session secret must contain at least 32 bytes")
        self.store, self.backend, self.clock = store, backend, clock
        self._secret = session_secret
        self._admin_hashes = tuple(admin_hashes) + tuple(
            hashlib.sha256(t.encode()).hexdigest() for t in admin_tokens if t
        )

    def require_operator(self, authorization: str | None) -> None:
        if not self._admin_hashes:
            raise StoreError(503, "auth_unconfigured")
        if not authorization or not authorization.startswith("Bearer "):
            raise StoreError(401, "unauthorized")
        token = authorization[7:].strip()
        digest = hashlib.sha256(token.encode()).hexdigest()
        matches = [hmac.compare_digest(digest, item) for item in self._admin_hashes]
        if not token or not any(matches):
            raise StoreError(401, "unauthorized")

    def pair(self, body) -> dict:
        if not body.terms_accepted:
            raise StoreError(403, "terms_required")
        payload = pair_payload(body.account_id, body.nonce, body.exp)
        now = int(self.clock())
        if now >= body.exp:
            raise StoreError(400, "invalid or expired pairing window")
        try:
            hotkey = decode_hotkey(body.hotkey)
            signature = bytes.fromhex(body.signature.removeprefix("0x"))
        except ValueError:
            raise StoreError(400, "invalid hotkey or signature") from None
        if len(signature) != 64:
            raise StoreError(400, "invalid signature")
        if not verify_substrate(hotkey, payload, signature):
            raise StoreError(401, "signature verification failed")
        return self.store.bind_pair(body.account_id, hotkey.hex(), body.nonce, now, self._secret)

    def grant_pair(self, body) -> dict:
        validate_account_id(body.account_id)
        now = int(self.clock())
        if body.expires_at <= now:
            raise StoreError(400, "pair grant must expire in the future")
        if body.expires_at > now + PAIR_GRANT_MAX_TTL_SECONDS:
            raise StoreError(
                400,
                f"pair grant must expire within {PAIR_GRANT_MAX_TTL_SECONDS} seconds",
            )
        try:
            hotkey = decode_hotkey(body.hotkey).hex()
        except ValueError:
            raise StoreError(400, "invalid hotkey") from None
        return self.store.grant_pair(
            body.account_id,
            hotkey,
            expires_at=body.expires_at,
            now=now,
        )

    async def submit(self, body) -> dict:
        # The Rust intake only checked configuration. Actually reading the feed
        # closes the documented 503/no-row contract during upstream outages.
        if not self.backend.configured:
            raise StoreError(503, "scoring unconfigured: set BOUNTY_BACKEND_PUBLIC_URL")
        pairing = self.store.lookup_session(body.session, self._secret)
        if body.hotkey:
            try:
                hotkey = decode_hotkey(body.hotkey).hex()
            except ValueError:
                raise StoreError(400, "invalid hotkey") from None
            if hotkey != pairing["miner_hotkey"]:
                raise StoreError(403, "hotkey_mismatch")
        repro = body.repro_steps or ""
        validate_substance(body.title, body.body, repro)
        try:
            await self.backend.fetch()
        except BackendUnavailable as exc:
            raise StoreError(503, str(exc)) from None
        return self.store.insert_report(pairing, body.title, body.body, repro, int(self.clock()))

    async def score(self, expected: list[str]) -> dict[str, BountyScore]:
        """Produce exact-E outcomes, including ChallengeInternal on feed failure."""
        keys = sorted({decode_hotkey(raw).hex() for raw in expected})
        try:
            snapshot = await self.backend.fetch()
            return snapshot.score(keys)
        except BackendUnavailable:
            return {key: BountyScore(reason="ChallengeInternal") for key in keys}


def validate_substance(title: str, body: str, repro: str) -> None:
    if not title.strip() or not body.strip():
        raise StoreError(400, "title_and_body_required")
    if normalize_text(title) == normalize_text(body):
        raise StoreError(400, "title_and_body_must_differ")
    if len(body.strip()) < 80:
        raise StoreError(400, "body must be at least 80 characters")
    if len(repro.strip()) < 20:
        raise StoreError(400, "repro_steps must be at least 20 characters")
    if len({token for token in normalize_text(body).split() if len(token) >= 3}) < 4:
        raise StoreError(400, "body_lacks_distinct_evidence")
