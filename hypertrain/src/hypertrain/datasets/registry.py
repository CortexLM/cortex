"""Reviewed dataset registry (A20): sources, mixes, tokenizer pin. Fails closed on bad ratios."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, cast

from hypertrain.data.tokenizer import HFPin
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize

REGISTRY_PATH = Path(__file__).with_name("registry.json")
OFFSET = 3  # PAD 0, MASK 1, CLS 2 (A5)
FORBIDDEN_SOURCES = frozenset({"microsoft/ms_marco"})  # A9: no licence tag, fail closed

MODERNBERT_PIN = HFPin(
    repo="answerdotai/ModernBERT-base",
    revision="8949b909ec900327062f0ebf497f51aef5e6f0c8",
    filename="tokenizer.json",
    sha256="9fd55248d51d33976b324fc11592e28071da7d41e0e9401dfb7082e30574b7b1",
    license="Apache-2.0",
)


class RegistryError(ValueError):
    pass


@dataclass(frozen=True)
class Source:
    id: str
    repo: str
    revision: str
    config: str | None
    files: tuple[str, ...]
    text_field: str
    licence: str
    role: Literal["pretrain", "distill", "finetune", "eval", "holdout"]
    exclude: tuple[str, ...] = ()
    # {"column": str, "allow": {value: licence}, "deny": [value, ...]}
    filter: dict[str, Any] | None = field(default=None, compare=False, hash=False)


@dataclass(frozen=True)
class Mix:
    id: str
    components: tuple[tuple[str, float], ...]
    tokenizer: str
    seq_len: int | None
    record: dict[str, Any] | None
    holdout: str | None
    decontam_against: tuple[str, ...]
    expected_merkle_root: str | None = None  # registry build record; builder refuses a mismatch


def load_registry(path: Path | None = None) -> tuple[dict[str, Source], dict[str, Mix], str]:
    """(sources, mixes, registry_sha256 = sha256(JCS(file)))."""
    raw = json.loads(Path(path or REGISTRY_PATH).read_text())
    sources = {
        k: Source(
            id=k,
            repo=v["repo"],
            revision=v["revision"],
            config=v.get("config"),
            files=tuple(v["files"]),
            text_field=v["text_field"],
            licence=v["licence"],
            role=v["role"],
            exclude=tuple(v.get("exclude", ())),
            filter=v.get("filter"),
        )
        for k, v in raw["sources"].items()
    }
    builds = raw.get("build_records", {})
    mixes = {
        k: Mix(
            id=k,
            components=tuple((c, float(r)) for c, r in v["components"]),
            tokenizer=v["tokenizer"],
            seq_len=v["seq_len"],
            record=v["record"],
            holdout=v["holdout"],
            decontam_against=tuple(v["decontam_against"]),
            expected_merkle_root=builds.get(k, {}).get("merkle_root"),
        )
        for k, v in raw["mixes"].items()
    }
    for s in sources.values():
        if s.repo in FORBIDDEN_SOURCES:
            raise RegistryError(f"source {s.id}: {s.repo} is excluded (A9)")
        if len(s.revision) != 40:
            raise RegistryError(f"source {s.id}: revision must be a 40-char commit sha")
    for m in mixes.values():
        for cid, _ in m.components:
            if cid not in sources:
                raise RegistryError(f"mix {m.id}: unknown source {cid}")
        for cid in (*m.decontam_against, *([m.holdout] if m.holdout else [])):
            if cid not in sources:
                raise RegistryError(f"mix {m.id}: unknown source {cid}")
        if abs(sum(r for _, r in m.components) - 1.0) > 1e-9:
            raise RegistryError(f"mix {m.id}: ratios sum to {sum(r for _, r in m.components)}")
        if (m.seq_len is None) == (m.record is None):
            raise RegistryError(f"mix {m.id}: exactly one of seq_len / record")
    return sources, mixes, sha256_hex(canonicalize(cast(Any, raw)))


def tokenizer_pin(name: str, path: Path | None = None) -> HFPin:
    raw = json.loads(Path(path or REGISTRY_PATH).read_text())["tokenizers"]
    if name not in raw:
        raise RegistryError(f"unknown tokenizer {name}")
    return HFPin(**raw[name])
