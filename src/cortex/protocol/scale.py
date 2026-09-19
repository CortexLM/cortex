"""Bounded, canonical SCALE primitives for the frozen v1 wire contract."""

from collections.abc import Callable, Iterable
from typing import TypeVar

T = TypeVar("T")


class ProtocolError(ValueError):
    """Malformed or unverifiable consensus input."""


def uint(value: int, width: int) -> bytes:
    if type(value) is not int or not 0 <= value < 1 << (width * 8):
        raise ProtocolError(f"integer outside u{width * 8}")
    return value.to_bytes(width, "little")


def compact(value: int) -> bytes:
    if type(value) is not int or not 0 <= value < 1 << 536:
        raise ProtocolError("invalid SCALE compact integer")
    if value < 1 << 6:
        return bytes([value << 2])
    if value < 1 << 14:
        return ((value << 2) | 1).to_bytes(2, "little")
    if value < 1 << 30:
        return ((value << 2) | 2).to_bytes(4, "little")
    size = max(4, (value.bit_length() + 7) // 8)
    return bytes([((size - 4) << 2) | 3]) + value.to_bytes(size, "little")


def byte_vec(value: bytes) -> bytes:
    return compact(len(value)) + value


def vector[T](values: Iterable[T], encode: Callable[[T], bytes]) -> bytes:
    items = tuple(values)
    return compact(len(items)) + b"".join(encode(item) for item in items)


def fixed(value: bytes, size: int) -> bytes:
    if not isinstance(value, bytes) or len(value) != size:
        raise ProtocolError(f"expected {size} bytes")
    return value


class Reader:
    """Rejects truncated, noncanonical, oversized and trailing wire bytes."""

    def __init__(self, data: bytes, *, max_bytes: int = 32 * 1024 * 1024):
        if len(data) > max_bytes:
            raise ProtocolError("SCALE input too large")
        self.data = data
        self.position = 0

    def take(self, size: int) -> bytes:
        if size < 0 or self.position + size > len(self.data):
            raise ProtocolError("truncated SCALE input")
        start = self.position
        self.position += size
        return self.data[start : self.position]

    def uint(self, size: int) -> int:
        return int.from_bytes(self.take(size), "little")

    def compact(self) -> int:
        start = self.position
        first = self.uint(1)
        mode = first & 3
        if mode == 0:
            value = first >> 2
        elif mode in (1, 2):
            value = int.from_bytes(bytes([first]) + self.take((1 << mode) - 1), "little") >> 2
        else:
            value = self.uint((first >> 2) + 4)
        if self.data[start : self.position] != compact(value):
            raise ProtocolError("noncanonical SCALE compact integer")
        return value

    def vector(self, decode: Callable[[], T], *, limit: int = 262144) -> tuple[T, ...]:
        size = self.compact()
        if size > limit:
            raise ProtocolError("SCALE vector too large")
        return tuple(decode() for _ in range(size))

    def byte_vec(self, *, limit: int = 64) -> bytes:
        size = self.compact()
        if size > limit:
            raise ProtocolError("SCALE byte vector too large")
        return self.take(size)

    def finish(self) -> None:
        if self.position != len(self.data):
            raise ProtocolError("trailing SCALE bytes")
