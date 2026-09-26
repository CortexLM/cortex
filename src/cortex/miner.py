"""Miner clients: Bittensor hotkey identity, signed topics, exact artifact bytes."""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import time
from pathlib import Path

import httpx

from cortex.errors import ServiceError
from cortex.http import read_private_file
from cortex.proof.artifacts import verify_artifact
from cortex.proof.models import Submission, SubmissionLookup, Topic
from cortex.proof.service import SUBMIT_DOMAIN, TOPIC_DOMAIN
from cortex.protocol.crypto import verify_raw
from cortex.wallet import HotkeySigner


def pair_payload(account: str, nonce: str, expiry: int) -> bytes:
    """Exact Substrate-context preimage verified by the CortexLM/bounty challenge."""
    if not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", account):
        raise ValueError("invalid account_id")
    if not re.fullmatch(r"[a-fA-F0-9]{16,64}", nonce) or not 0 < expiry <= 2**64 - 1:
        raise ValueError("invalid pairing nonce or expiry")
    return f"cortex-bounty-v1|{account}|{nonce}|{expiry}".encode()


def load_seed(path: Path) -> bytes:
    value = read_private_file(path, 128)
    try:
        seed = bytes.fromhex(value)
    except ValueError:
        raise ServiceError(503, "key file must contain a 32-byte hexadecimal seed") from None
    if len(seed) != 32:
        raise ServiceError(503, "key file must contain a 32-byte hexadecimal seed")
    return seed


def write_submission_receipt(path: Path, envelope: SubmissionLookup) -> None:
    descriptor: int | None = None
    parent_descriptor: int | None = None
    created = False
    try:
        descriptor = os.open(
            path,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o600,
        )
        created = True
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w") as stream:
            descriptor = None
            stream.write(envelope.model_dump_json())
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        parent_descriptor = os.open(
            path.parent or Path("."), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
        )
        os.fsync(parent_descriptor)
    except BaseException:
        if descriptor is not None:
            os.close(descriptor)
        if created:
            try:
                path.unlink()
            except OSError:
                pass
        raise
    finally:
        if parent_descriptor is not None:
            os.close(parent_descriptor)


class MinerClient:
    def __init__(
        self,
        *,
        base_url: str,
        client: httpx.AsyncClient,
        signer: HotkeySigner | None = None,
        seed: bytes | None = None,
        proof_public_key: bytes | None = None,
        submit_timeout_seconds: float = 7200,
    ):
        if (signer is None) == (seed is None):
            raise ValueError("provide a wallet signer or a development seed")
        if seed is not None:
            signer = HotkeySigner.from_dev_seed(seed)
        if signer is None:
            raise ValueError("miner hotkey signer required")
        if proof_public_key is not None and len(proof_public_key) != 32:
            raise ValueError("proof public key must be 32 bytes")
        if submit_timeout_seconds < 0:
            raise ValueError("submit timeout must be nonnegative")
        self.base_url = base_url.rstrip("/")
        self._signer = signer
        self.proof_public_key = proof_public_key
        self.http = client
        self.submit_timeout = None if submit_timeout_seconds == 0 else submit_timeout_seconds

    @property
    def hotkey(self) -> str:
        return self._signer.ss58_address

    async def topic(self, topic_id: str) -> Topic:
        if self.proof_public_key is None:
            raise ServiceError(400, "Proof owner public key required")
        response = await self.http.get(
            self.base_url + "/challenge/proof/v1/proof/topics/" + topic_id,
            timeout=60,
            follow_redirects=False,
        )
        response.raise_for_status()
        topic = Topic.model_validate(response.json())
        try:
            valid = verify_raw(
                self.proof_public_key,
                TOPIC_DOMAIN,
                topic.signing_payload(),
                bytes.fromhex(topic.signature),
            )
        except ValueError:
            valid = False
        if not valid or topic.id != topic_id:
            raise ServiceError(401, "topic signature or identity mismatch")
        if topic.status != "open":
            raise ServiceError(400, "topic is not open")
        return topic

    async def submit_proof(
        self,
        *,
        topic_id: str,
        artifact: bytes,
        claim: str,
        manifest: dict | None = None,
        declared_flops: int = 0,
        env: dict[str, str] | None = None,
        nonce: str | None = None,
        receipt_path: Path | None = None,
    ) -> dict:
        topic = await self.topic(topic_id)
        commitment = hashlib.sha256(artifact).hexdigest()
        verify_artifact(artifact, commitment)
        unsigned = Submission.model_validate(
            {
                "miner_hotkey": self._signer.public_key.hex(),
                "hotkey_signature": "00" * 64,
                "topic_id": topic.id,
                "artifact_digest": commitment,
                "claim": claim,
                "declared_flops": declared_flops,
                "manifest": manifest or {},
                "env": env or {},
                "submit_nonce": nonce or secrets.token_hex(32),
            }
        )
        signed = unsigned.model_copy(
            update={
                "hotkey_signature": self._signer.sign(
                    SUBMIT_DOMAIN, unsigned.signing_payload()
                ).hex()
            }
        )
        envelope = SubmissionLookup.model_validate(
            signed.model_dump(exclude={"artifact_uri", "env"})
        )
        if receipt_path is not None:
            write_submission_receipt(receipt_path, envelope)
        body = signed.model_dump()
        body["env"] = env or {}
        response = await self.http.post(
            self.base_url + "/challenge/proof/v1/submissions",
            data={"json": json.dumps(body)},
            files={"artifact": ("artifact.tar", artifact, "application/x-tar")},
            timeout=httpx.Timeout(self.submit_timeout, connect=30),
            follow_redirects=False,
        )
        response.raise_for_status()
        return response.json()

    async def lookup_proof(self, receipt_path: Path) -> dict:
        try:
            envelope = SubmissionLookup.model_validate_json(
                read_private_file(receipt_path, 128 * 1024)
            )
        except ValueError:
            raise ServiceError(400, "invalid Proof submission receipt") from None
        if envelope.miner_hotkey != self._signer.public_key.hex() or not verify_raw(
            self._signer.public_key,
            SUBMIT_DOMAIN,
            envelope.signing_payload(),
            bytes.fromhex(envelope.hotkey_signature),
        ):
            raise ServiceError(401, "Proof receipt does not belong to this hotkey")
        response = await self.http.post(
            self.base_url + "/challenge/proof/v1/submissions/lookup",
            json=envelope.model_dump(),
            timeout=60,
            follow_redirects=False,
        )
        response.raise_for_status()
        return response.json()

    async def pair_bounty(self, *, account_id: str, accept_terms: bool) -> dict:
        if not accept_terms:
            raise ValueError("pairing requires explicit acceptance of Bounty research terms")
        nonce, expiry = secrets.token_hex(16), int(time.time()) + 300
        response = await self.http.post(
            self.base_url + "/challenge/bounty/v1/pair",
            json={
                "account_id": account_id,
                "hotkey": self.hotkey,
                "nonce": nonce,
                "exp": expiry,
                "signature": self._signer.sign_substrate(
                    pair_payload(account_id, nonce, expiry)
                ).hex(),
                "terms_accepted": True,
            },
            timeout=60,
            follow_redirects=False,
        )
        response.raise_for_status()
        return response.json()

    async def report_bounty(self, *, session: str, title: str, body: str, repro_steps: str) -> dict:
        response = await self.http.post(
            self.base_url + "/challenge/bounty/v1/reports",
            json={
                "session": session,
                "hotkey": self.hotkey,
                "title": title,
                "body": body,
                "repro_steps": repro_steps,
            },
            timeout=60,
            follow_redirects=False,
        )
        response.raise_for_status()
        return response.json()
