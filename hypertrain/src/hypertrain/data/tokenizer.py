"""Tokenizers pinned by sha256. Byte-level (CPU experiments) and HF tokenizer.json (production)."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol


class TokenizerPinError(ValueError):
    """tokenizer.json bytes do not match the pinned sha256."""


class Tokenizer(Protocol):
    name: str
    sha256: str
    vocab_size: int
    eos_id: int

    def encode(self, text: str) -> list[int]: ...


@dataclass(frozen=True)
class HFPin:
    repo: str
    revision: str
    filename: str
    sha256: str
    license: str

    @property
    def url(self) -> str:
        return f"https://huggingface.co/{self.repo}/resolve/{self.revision}/{self.filename}"


# GPT-2 BPE, MIT license. Revision + sha256 checked 2026-10-07 via the HF API.
GPT2_PIN = HFPin(
    repo="openai-community/gpt2",
    revision="607a30d783dfa663caf39e06633721c8d4cfcd7e",
    filename="tokenizer.json",
    sha256="8414cab924d8b9b33013f0d221c5862f365ee9be39c5c2bfae8a5a9e970478a6",
    license="MIT",
)

_BYTE_SPEC = b"ht-byte-tokenizer-v1|vocab=259|bytes=0..255|bos=256|eos=257|pad=258"


class ByteTokenizer:
    """UTF-8 bytes 0..255 plus BOS=256, EOS=257, PAD=258. Pinned by sha256 of its spec string."""

    name = "ht-byte-259"
    sha256 = hashlib.sha256(_BYTE_SPEC).hexdigest()
    vocab_size = 259
    bos_id = 256
    eos_id = 257
    pad_id = 258

    def encode(self, text: str) -> list[int]:
        return list(text.encode("utf-8"))

    def decode(self, ids: list[int]) -> str:
        return bytes(i for i in ids if i < 256).decode("utf-8", errors="replace")


class HFTokenizer:
    """Loads a tokenizer.json only if its bytes hash to the pinned sha256."""

    def __init__(self, path: Path, sha256: str, name: str) -> None:
        raw = Path(path).read_bytes()
        got = hashlib.sha256(raw).hexdigest()
        if got != sha256:
            raise TokenizerPinError(f"{path}: sha256 {got} != pinned {sha256}")
        from tokenizers import Tokenizer as _T  # local import: optional for byte-level runs

        self._tok = _T.from_str(raw.decode("utf-8"))
        self.name = name
        self.sha256 = sha256
        self.vocab_size = self._tok.get_vocab_size(with_added_tokens=True)
        eos = self._tok.token_to_id("<|endoftext|>")
        if eos is None:
            raise TokenizerPinError(f"{path}: no <|endoftext|> token")
        self.eos_id = eos

    def encode(self, text: str) -> list[int]:
        return list(self._tok.encode(text, add_special_tokens=False).ids)
