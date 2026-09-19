"""Immutable SCALE v1 types. Field order mirrors the frozen Rust contract."""

from dataclasses import dataclass
from enum import IntEnum

from .scale import ProtocolError, Reader, byte_vec, fixed, uint, vector

LIVE_SHARES = ((b"bounty", 2000), (b"proof", 8000))
PROPORTIONAL_SHARES = ((b"bounty", 3000), (b"proof", 7000))
BOUNTY_FULL_SHARE_REPORTS = 10


class NoScoreReason(IntEnum):
    NOT_ATTEMPTED = 0
    TIMEOUT = 1
    INVALID_RESPONSE = 2
    ATTESTATION_NOT_VERIFIED = 3
    MINER_ERROR = 4
    RATE_LIMITED = 5
    CHALLENGE_INTERNAL = 6
    POLICY_SKIP = 7


@dataclass(frozen=True)
class Score:
    value: int

    def encode(self) -> bytes:
        return b"\x00" + uint(self.value, 8)


@dataclass(frozen=True)
class NoScore:
    reason: NoScoreReason = NoScoreReason.CHALLENGE_INTERNAL

    def encode(self) -> bytes:
        try:
            code = NoScoreReason(self.reason)
        except ValueError as error:
            raise ProtocolError("unknown NoScore reason") from error
        return b"\x01" + bytes([code])


@dataclass(frozen=True)
class Leaf:
    challenge_id: bytes
    miner_hotkey: bytes
    epoch: int
    score: Score | NoScore
    challenge_sig: bytes

    @property
    def sort_key(self) -> bytes:
        return byte_vec(self.challenge_id) + fixed(self.miner_hotkey, 32)

    def payload(self) -> bytes:
        if not 1 <= len(self.challenge_id) <= 64:
            raise ProtocolError("challenge id must contain 1..64 bytes")
        try:
            self.challenge_id.decode("utf-8")
        except UnicodeDecodeError:
            raise ProtocolError("challenge id must be UTF-8") from None
        return self.sort_key + uint(self.epoch, 8) + self.score.encode()

    def encode(self) -> bytes:
        return self.payload() + fixed(self.challenge_sig, 64)

    @classmethod
    def read(cls, reader: Reader) -> "Leaf":
        challenge = reader.byte_vec()
        hotkey = reader.take(32)
        epoch = reader.uint(8)
        discriminant = reader.uint(1)
        if discriminant == 0:
            score: Score | NoScore = Score(reader.uint(8))
        elif discriminant == 1:
            try:
                score = NoScore(NoScoreReason(reader.uint(1)))
            except ValueError as error:
                raise ProtocolError("unknown NoScore reason") from error
        else:
            raise ProtocolError("unknown score discriminant")
        return cls(challenge, hotkey, epoch, score, reader.take(64))


@dataclass(frozen=True)
class MetagraphRow:
    hotkey: bytes
    uid: int
    stake: int = 0

    def encode(self) -> bytes:
        return fixed(self.hotkey, 32) + uint(self.uid, 2) + uint(self.stake, 8)


@dataclass(frozen=True)
class ParticipantPolicy:
    """SCALE discriminants: all, minimum stake, allowlist, denylist."""

    kind: int = 0
    min_stake: int = 0
    hotkeys: tuple[bytes, ...] = ()

    def encode(self) -> bytes:
        if self.kind == 0:
            return b"\x00"
        if self.kind == 1:
            return b"\x01" + uint(self.min_stake, 8)
        if self.kind not in (2, 3):
            raise ProtocolError("unknown participant policy")
        if tuple(sorted(set(self.hotkeys))) != self.hotkeys:
            raise ProtocolError("participant hotkeys must be sorted and unique")
        return bytes([self.kind]) + vector(self.hotkeys, lambda key: fixed(key, 32))

    def expected(self, rows: tuple[MetagraphRow, ...]) -> set[bytes]:
        self.encode()
        keys = {row.hotkey for row in rows}
        if self.kind == 1:
            return {row.hotkey for row in rows if row.stake >= self.min_stake}
        if self.kind == 2:
            return keys.intersection(self.hotkeys)
        if self.kind == 3:
            return keys.difference(self.hotkeys)
        return keys


@dataclass(frozen=True)
class ChallengeEntry:
    id: bytes
    public_key: bytes
    emission_share_bps: int
    policy: ParticipantPolicy = ParticipantPolicy()

    def encode(self) -> bytes:
        if not 1 <= len(self.id) <= 64:
            raise ProtocolError("challenge id must contain 1..64 bytes")
        return (
            byte_vec(self.id)
            + fixed(self.public_key, 32)
            + uint(self.emission_share_bps, 2)
            + self.policy.encode()
        )


@dataclass(frozen=True)
class TrustRoot:
    """Local trust, constructed only from independently verified owner documents."""

    challenges: tuple[ChallengeEntry, ...]
    measurements_digest: bytes
    gateway_hotkey: bytes
    challenges_version: int = 1
    measurements_version: int = 1
    introduced_epoch: int = 0

    @property
    def shares(self) -> tuple[tuple[bytes, int], ...]:
        return tuple((entry.id, entry.emission_share_bps) for entry in self.challenges)

    @property
    def algorithm_version(self) -> int:
        self.validate()
        return 2 if self.shares == PROPORTIONAL_SHARES else 1

    def challenges_body(self) -> bytes:
        self.validate()
        return vector(self.challenges, ChallengeEntry.encode)

    def validate(self) -> None:
        if self.shares not in (LIVE_SHARES, PROPORTIONAL_SHARES):
            raise ProtocolError("shares must be bounty/proof=2000/8000 or 3000/7000")
        if self.shares == PROPORTIONAL_SHARES and self.challenges_version < 2:
            raise ProtocolError("proportional shares require challenges version >= 2")
        fixed(self.measurements_digest, 32)
        fixed(self.gateway_hotkey, 32)
        uint(self.challenges_version, 4)
        uint(self.measurements_version, 4)
        uint(self.introduced_epoch, 8)
        for challenge in self.challenges:
            challenge.encode()


def encode_final_vector(values: tuple[tuple[int, int], ...]) -> bytes:
    return vector(values, lambda pair: uint(pair[0], 2) + uint(pair[1], 2))


@dataclass(frozen=True)
class BundleBody:
    protocol_version: int
    epoch: int
    netuid: int
    block_b: int
    block_hash: bytes
    metagraph_root: bytes
    algorithm_version: int
    emission_shares: tuple[tuple[bytes, int], ...]
    measurements_digest: bytes
    uid_map: tuple[tuple[bytes, int], ...]
    leaves: tuple[Leaf, ...]
    merkle_root: bytes
    final_vector: tuple[tuple[int, int], ...]
    gateway_hotkey: bytes

    def encode(self) -> bytes:
        return b"".join(
            (
                uint(self.protocol_version, 2),
                uint(self.epoch, 8),
                uint(self.netuid, 2),
                uint(self.block_b, 8),
                fixed(self.block_hash, 32),
                fixed(self.metagraph_root, 32),
                uint(self.algorithm_version, 2),
                vector(self.emission_shares, lambda pair: byte_vec(pair[0]) + uint(pair[1], 2)),
                fixed(self.measurements_digest, 32),
                vector(self.uid_map, lambda pair: fixed(pair[0], 32) + uint(pair[1], 2)),
                vector(self.leaves, Leaf.encode),
                fixed(self.merkle_root, 32),
                encode_final_vector(self.final_vector),
                fixed(self.gateway_hotkey, 32),
            )
        )

    @classmethod
    def read(cls, reader: Reader) -> "BundleBody":
        return cls(
            reader.uint(2),
            reader.uint(8),
            reader.uint(2),
            reader.uint(8),
            reader.take(32),
            reader.take(32),
            reader.uint(2),
            reader.vector(lambda: (reader.byte_vec(), reader.uint(2)), limit=64),
            reader.take(32),
            reader.vector(lambda: (reader.take(32), reader.uint(2)), limit=65536),
            reader.vector(lambda: Leaf.read(reader)),
            reader.take(32),
            reader.vector(lambda: (reader.uint(2), reader.uint(2)), limit=65536),
            reader.take(32),
        )


@dataclass(frozen=True)
class Bundle:
    body: BundleBody
    gateway_sig: bytes

    def encode(self) -> bytes:
        return self.body.encode() + fixed(self.gateway_sig, 64)

    @classmethod
    def decode(cls, data: bytes) -> "Bundle":
        reader = Reader(data)
        result = cls(BundleBody.read(reader), reader.take(64))
        reader.finish()
        if result.encode() != data:
            raise ProtocolError("noncanonical bundle encoding")
        return result
