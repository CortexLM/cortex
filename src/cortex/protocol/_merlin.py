"""Public Merlin transcript (STROBE-128 / Keccak-f1600).

Port of merlin 3.0's transcript.rs/strobe.rs (MIT), for the fixed Schnorrkel
verification transcript. Secret scalars never enter this Python code; point
and scalar arithmetic is performed by libsodium. Keep the independent Rust
signature vectors when changing this implementation.

MIT License — Copyright (c) 2018 Henry de Valence.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""

_MASK = (1 << 64) - 1
_ROTATION = (
    (0, 36, 3, 41, 18),
    (1, 44, 10, 45, 2),
    (62, 6, 43, 15, 61),
    (28, 55, 25, 21, 56),
    (27, 20, 39, 8, 14),
)
_ROUNDS = (
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808A,
    0x8000000080008000,
    0x000000000000808B,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008A,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000A,
    0x000000008000808B,
    0x800000000000008B,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800A,
    0x800000008000000A,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
)


def _rotate(value: int, bits: int) -> int:
    return ((value << bits) | (value >> (64 - bits))) & _MASK


def _keccak(state: bytearray) -> None:
    words = [int.from_bytes(state[i : i + 8], "little") for i in range(0, 200, 8)]
    for constant in _ROUNDS:
        columns = [
            words[x] ^ words[x + 5] ^ words[x + 10] ^ words[x + 15] ^ words[x + 20]
            for x in range(5)
        ]
        for x in range(5):
            delta = columns[(x - 1) % 5] ^ _rotate(columns[(x + 1) % 5], 1)
            for y in range(5):
                words[x + 5 * y] ^= delta
        permuted = [0] * 25
        for x in range(5):
            for y in range(5):
                permuted[y + 5 * ((2 * x + 3 * y) % 5)] = _rotate(words[x + 5 * y], _ROTATION[x][y])
        for x in range(5):
            for y in range(5):
                words[x + 5 * y] = permuted[x + 5 * y] ^ (
                    (~permuted[(x + 1) % 5 + 5 * y]) & permuted[(x + 2) % 5 + 5 * y]
                )
        words[0] ^= constant
    state[:] = b"".join(word.to_bytes(8, "little") for word in words)


class _Strobe:
    def __init__(self):
        self.state = bytearray(200)
        self.state[:18] = bytes([1, 168, 1, 0, 1, 96]) + b"STROBEv1.0.2"
        _keccak(self.state)
        self.position = 0
        self.begin = 0
        self.absorb(18, b"Merlin v1.0")

    def _run(self) -> None:
        self.state[self.position] ^= self.begin
        self.state[self.position + 1] ^= 4
        self.state[167] ^= 128
        _keccak(self.state)
        self.position = self.begin = 0

    def _absorb(self, data: bytes) -> None:
        for byte in data:
            self.state[self.position] ^= byte
            self.position += 1
            if self.position == 166:
                self._run()

    def _start(self, flags: int) -> None:
        previous = self.begin
        self.begin = self.position + 1
        self._absorb(bytes([previous, flags]))
        if flags & 36 and self.position:
            self._run()

    def absorb(self, flags: int, data: bytes) -> None:
        self._start(flags)
        self._absorb(data)

    def squeeze(self, size: int) -> bytes:
        self._start(7)
        output = bytearray()
        for _ in range(size):
            output.append(self.state[self.position])
            self.state[self.position] = 0
            self.position += 1
            if self.position == 166:
                self._run()
        return bytes(output)


class Transcript:
    def __init__(self, label: bytes):
        self.strobe = _Strobe()
        self.append(b"dom-sep", label)

    def append(self, label: bytes, data: bytes) -> None:
        self.strobe.absorb(18, label + len(data).to_bytes(4, "little"))
        self.strobe.absorb(2, data)

    def challenge(self, label: bytes, size: int) -> bytes:
        self.strobe.absorb(18, label + size.to_bytes(4, "little"))
        return self.strobe.squeeze(size)
