"""Frozen peer-root and dissent signatures (BUNDLE_SPEC §§10–12)."""

from dataclasses import dataclass
from enum import IntEnum

from .crypto import public_key, sign_raw, verify_raw
from .scale import ProtocolError, Reader, fixed, uint

ROOT_DOMAIN = b"base-root-v1"
DISSENT_DOMAIN = b"base-dissent-v1"


class DissentReason(IntEnum):
    VECTOR_MISMATCH = 0
    LEAF_SIGNATURE_INVALID = 1
    LEAF_CHALLENGE_KEY_UNKNOWN = 2
    INCOMPLETE_PARTICIPANT_SET = 3
    MERKLE_ROOT_MISMATCH = 4
    EMISSION_SHARE_MISMATCH = 5
    METAGRAPH_ROOT_MISMATCH = 6
    BLOCK_HASH_MISMATCH = 7
    PROTOCOL_VERSION_UNSUPPORTED = 8
    PEER_ROOT_CONFLICT = 9
    PEER_SAMPLE_INSUFFICIENT = 10
    SHARE_MASS_BELOW_THRESHOLD = 11
    BUNDLE_SIGNATURE_INVALID = 12
    AGGREGATION_OVERFLOW = 13
    EMPTY_SCORE_VECTOR_NO_SUBMIT = 14
    UID_MAP_MISMATCH = 15
    MEASUREMENTS_DIGEST_MISMATCH = 16
    DUPLICATE_LEAF = 17
    QUARANTINE_EXHAUSTED = 18


@dataclass(frozen=True)
class RootStatement:
    epoch: int
    merkle_root: bytes
    hotkey: bytes
    signature: bytes

    def payload(self) -> bytes:
        return uint(self.epoch, 8) + fixed(self.merkle_root, 32)

    def verify(self) -> None:
        if not verify_raw(
            fixed(self.hotkey, 32), ROOT_DOMAIN, self.payload(), fixed(self.signature, 64)
        ):
            raise ProtocolError("invalid peer root signature")

    def encode(self) -> bytes:
        return self.payload() + fixed(self.hotkey, 32) + fixed(self.signature, 64)

    def to_json(self) -> dict:
        return dict(
            epoch=self.epoch,
            merkle_root=self.merkle_root.hex(),
            hotkey=self.hotkey.hex(),
            signature=self.signature.hex(),
        )

    @classmethod
    def from_json(cls, value: dict) -> "RootStatement":
        try:
            statement = cls(
                value["epoch"],
                bytes.fromhex(value["merkle_root"]),
                bytes.fromhex(value["hotkey"]),
                bytes.fromhex(value["signature"]),
            )
            statement.verify()
            return statement
        except (KeyError, TypeError, ValueError):
            raise ProtocolError("invalid signed root response") from None

    @classmethod
    def decode(cls, data: bytes) -> "RootStatement":
        reader = Reader(data)
        result = cls(reader.uint(8), reader.take(32), reader.take(32), reader.take(64))
        reader.finish()
        result.verify()
        return result

    @classmethod
    def sign(cls, seed: bytes, epoch: int, root: bytes) -> "RootStatement":
        payload = uint(epoch, 8) + fixed(root, 32)
        return cls(epoch, root, public_key(seed), sign_raw(seed, ROOT_DOMAIN, payload))


@dataclass(frozen=True)
class Dissent:
    epoch: int
    bundle_root: bytes
    expected_vector_hash: bytes
    actual_vector_hash: bytes
    reason: int
    validator_hotkey: bytes
    signature: bytes
    protocol_version: int = 1

    def payload(self) -> bytes:
        return (
            uint(self.protocol_version, 2)
            + uint(self.epoch, 8)
            + fixed(self.bundle_root, 32)
            + fixed(self.expected_vector_hash, 32)
            + fixed(self.actual_vector_hash, 32)
            + uint(self.reason, 1)
        )

    def encode(self) -> bytes:
        return self.payload() + fixed(self.validator_hotkey, 32) + fixed(self.signature, 64)

    def verify(self) -> None:
        if self.protocol_version != 1 or not verify_raw(
            self.validator_hotkey, DISSENT_DOMAIN, self.payload(), self.signature
        ):
            raise ProtocolError("invalid dissent signature or version")

    @classmethod
    def decode(cls, data: bytes) -> "Dissent":
        reader = Reader(data)
        version = reader.uint(2)
        result = cls(
            reader.uint(8),
            reader.take(32),
            reader.take(32),
            reader.take(32),
            reader.uint(1),
            reader.take(32),
            reader.take(64),
            version,
        )
        reader.finish()
        result.verify()  # Unknown reasons remain verifiable/persistable as raw wire evidence.
        return result

    @classmethod
    def sign(
        cls,
        seed: bytes,
        epoch: int,
        root: bytes,
        expected: bytes,
        actual: bytes,
        reason: DissentReason,
    ) -> "Dissent":
        unsigned = cls(epoch, root, expected, actual, int(reason), public_key(seed), bytes(64))
        return cls(
            epoch,
            root,
            expected,
            actual,
            int(reason),
            public_key(seed),
            sign_raw(seed, DISSENT_DOMAIN, unsigned.payload()),
        )
