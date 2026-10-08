"""drand quicknet (bls-unchained-g1-rfc9380): parsing, BLS verification, multi-relay pull client."""

from __future__ import annotations

import hashlib
from collections.abc import Mapping, Sequence
from typing import Any

import httpx
from py_ecc.bls.hash_to_curve import hash_to_G1
from py_ecc.bls.point_compression import compress_G1, decompress_G1, decompress_G2
from py_ecc.optimized_bls12_381 import FQ12, G2, curve_order, is_inf, multiply, neg
from py_ecc.optimized_bls12_381.optimized_pairing import final_exponentiate, miller_loop

from hypertrain.beacon.core import (
    BeaconRound,
    BeaconUnavailable,
    BeaconVerificationError,
    PushBeacon,
    round_at_or_after,
)

QUICKNET_CHAIN_HASH = "52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971"
QUICKNET_PUBLIC_KEY = (
    "83cf0f2896adee7eb8b5f01fcad3912212c437e0073e911fb90022d3e760183c"
    "8c4b450b6a0a6c3ac6a5776a2d1064510d1fec758c921cc22b0e17e63aaf4bcb"
    "5ed66304de9cf809bd274ca73bab4af5a6e9c76a4bc09e76eae8991ef5ece45a"
)
QUICKNET_PERIOD = 3
QUICKNET_GENESIS = 1692803367
QUICKNET_SCHEME = "bls-unchained-g1-rfc9380"
DST_G1 = b"BLS_SIG_BLS12381G1_XMD:SHA-256_SSWU_RO_NUL_"
RELAYS = ("https://api.drand.sh", "https://drand.cloudflare.com")
_UA = {"User-Agent": "hypertrain-beacon/0.1"}  # drand.cloudflare.com answers 403 without one


def _hex(value: Any, n_bytes: int, field: str) -> bytes:
    if not isinstance(value, str) or len(value) != 2 * n_bytes:
        raise BeaconVerificationError(f"{field}: expected {2 * n_bytes} hex chars")
    try:
        return bytes.fromhex(value)
    except ValueError:
        raise BeaconVerificationError(f"{field}: not hex") from None


def _pairing_ok(sig: bytes, round: int, pubkey: bytes) -> bool:
    # Unchained G1 scheme: e(sig, g2) == e(H(sha256(be64(round))), pk), checked as one product.
    sig_int = int.from_bytes(sig, "big")
    try:
        s = decompress_G1(sig_int)  # type: ignore[arg-type]
        pk = decompress_G2(
            (int.from_bytes(pubkey[:48], "big"), int.from_bytes(pubkey[48:], "big"))  # type: ignore[arg-type]
        )
    except (ValueError, AssertionError):
        return False
    # decompress only checks on-curve: without canonical + identity + r-torsion checks a relay
    # can add a cofactor-torsion point and pick among many "valid" signatures (grindable seed).
    if compress_G1(s) != sig_int or is_inf(s) or is_inf(pk):
        return False
    if not is_inf(multiply(s, curve_order)) or not is_inf(multiply(pk, curve_order)):
        return False
    msg = hashlib.sha256(round.to_bytes(8, "big")).digest()
    h = hash_to_G1(msg, DST_G1, hashlib.sha256)  # type: ignore[arg-type]
    product = miller_loop(G2, s) * miller_loop(neg(pk), h)
    return bool(final_exponentiate(product) == FQ12.one())


def verify_quicknet(round: int, signature: str, pubkey: str = QUICKNET_PUBLIC_KEY) -> bool:
    """True iff `signature` is the quicknet group's BLS signature on `round`. Never raises."""
    if type(round) is not int or not 1 <= round < 2**64:
        return False
    try:
        sig = bytes.fromhex(signature)
        pk = bytes.fromhex(pubkey)
    except (ValueError, TypeError):
        return False
    if len(sig) != 48 or len(pk) != 96:
        return False
    return _pairing_ok(sig, round, pk)


def parse_round(payload: Mapping[str, Any], pubkey: str = QUICKNET_PUBLIC_KEY) -> BeaconRound:
    """Validate one drand JSON round {round, signature, randomness}; raise on any defect."""
    rnd = payload.get("round")
    if type(rnd) is not int or rnd < 1:
        raise BeaconVerificationError("round: expected positive int")
    sig = _hex(payload.get("signature"), 48, "signature")
    rand = _hex(payload.get("randomness"), 32, "randomness")
    if hashlib.sha256(sig).digest() != rand:
        raise BeaconVerificationError(f"round {rnd}: randomness != sha256(signature)")
    if not verify_quicknet(rnd, sig.hex(), pubkey):
        raise BeaconVerificationError(f"round {rnd}: BLS signature invalid")
    return BeaconRound(rnd, sig.hex(), rand.hex(), bls_verified=True)


def quicknet_push_beacon() -> PushBeacon:
    return PushBeacon(parse_round, QUICKNET_GENESIS, QUICKNET_PERIOD)


class DrandQuicknet:
    """Pull client for out-of-band components: every relay must agree, every round is verified."""

    def __init__(
        self,
        relays: Sequence[str] = RELAYS,
        client: httpx.Client | None = None,
        timeout: float = 10.0,
        min_relays: int = 2,
    ) -> None:
        if len(relays) < min_relays:
            raise ValueError(f"need >= {min_relays} relays")
        self.relays = tuple(r.rstrip("/") for r in relays)
        self.min_relays = min_relays
        self._client = client or httpx.Client(timeout=timeout, headers=_UA)

    def round_at_or_after(self, t: int) -> int:
        return round_at_or_after(t, QUICKNET_GENESIS, QUICKNET_PERIOD)

    def _fetch(self, path: str) -> list[dict[str, Any]]:
        got: list[dict[str, Any]] = []
        errors: list[str] = []
        for relay in self.relays:
            try:
                r = self._client.get(f"{relay}/{QUICKNET_CHAIN_HASH}/{path}")
                r.raise_for_status()
                body = r.json()
                if not isinstance(body, dict):
                    raise ValueError("non-object body")
                got.append(body)
            except (httpx.HTTPError, ValueError) as e:
                errors.append(f"{relay}: {type(e).__name__}: {e}")
        if len(got) < self.min_relays:
            raise BeaconUnavailable(f"{len(got)}/{len(self.relays)} relays answered: {errors}")
        return got

    def info_matches(self) -> bool:
        infos = self._fetch("info")
        return all(
            i.get("public_key") == QUICKNET_PUBLIC_KEY
            and i.get("hash") == QUICKNET_CHAIN_HASH
            and i.get("period") == QUICKNET_PERIOD
            and i.get("genesis_time") == QUICKNET_GENESIS
            and i.get("schemeID") == QUICKNET_SCHEME
            for i in infos
        )

    def _agree(self, bodies: list[dict[str, Any]]) -> BeaconRound:
        rounds = [parse_round(b) for b in bodies]
        if any(r != rounds[0] for r in rounds):
            raise BeaconVerificationError(f"relays disagree: {[r.round for r in rounds]}")
        return rounds[0]

    def get(self, round: int) -> BeaconRound:
        if type(round) is not int or round < 1:
            raise ValueError("round must be a positive int")
        br = self._agree(self._fetch(f"public/{round}"))
        if br.round != round:
            raise BeaconVerificationError(f"asked round {round}, relays returned {br.round}")
        return br

    def latest(self) -> BeaconRound:
        bodies = self._fetch("public/latest")
        # Relays may straddle a round boundary; re-fetch the smallest reported round from all.
        lowest = min(r if type(r := b.get("round")) is int else 0 for b in bodies)
        if all(b.get("round") == lowest for b in bodies):
            return self._agree(bodies)
        return self.get(lowest)
