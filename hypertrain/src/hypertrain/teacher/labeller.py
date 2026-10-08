"""Letter-logprob teacher: one question -> K_q+1 probabilities (unknown last, A11)."""

from __future__ import annotations

import hashlib
import json
import math
import os
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from hypertrain.teacher.client import TeacherClient, estimate_tokens
from hypertrain.teacher.config import TeacherConfig, TeacherError

UNKNOWN_TEXT = "cannot be determined from the text"
LETTERS = "ABCDEFGHIJ"


@dataclass(frozen=True)
class Question:
    state: str
    instruction: str
    options: tuple[str, ...]
    qtype: int  # opendecision QTYPES: choice 0, score 1, noul 2
    gold: int | None


def build_prompt(q: Question, order: tuple[int, ...]) -> str:
    """order = item ids shown top to bottom; id len(q.options) is the unknown option."""
    texts = [*q.options, UNKNOWN_TEXT]
    lines = [f"{LETTERS[i]}. {texts[o]}" for i, o in enumerate(order)]
    return (
        "Read the text and answer the question using only the text.\n\n"
        f"Text:\n{q.state}\n\nQuestion: {q.instruction}\n\nOptions:\n"
        + "\n".join(lines)
        + "\n\nAnswer with a single letter only."
    )


def rotations(q: Question, cap: int) -> list[tuple[int, ...]]:
    n = len(q.options) + 1
    if n > len(LETTERS):
        raise TeacherError(f"{n} options exceed {len(LETTERS)} letters")
    items = tuple(range(n))
    rs = range(n) if cap <= 0 or cap >= n else sorted({round(i * n / cap) for i in range(cap)})
    return [items[r:] + items[:r] for r in rs]


def cache_key(cfg: TeacherConfig, prompt: str, order: tuple[int, ...]) -> str:
    blob = json.dumps([cfg.base_url, cfg.model, prompt, list(order)], ensure_ascii=False)
    return hashlib.sha256(blob.encode()).hexdigest()


def letter_probs(alts: list[tuple[str, float]] | None, n: int) -> list[float] | None:
    """Renormalised probabilities over the first n letters; None if no letter is present."""
    if not alts:
        return None
    mass = [0.0] * n
    for tok, lp in alts:
        t = tok.strip().upper()
        if len(t) == 1 and t in LETTERS[:n]:
            mass[LETTERS.index(t)] += math.exp(lp)
    s = sum(mass)
    return [m / s for m in mass] if s > 0 else None


class LabelCache:
    """One JSON file per key; os.replace makes each write atomic."""

    def __init__(self, root: Path) -> None:
        self.root = Path(root)

    def _path(self, key: str) -> Path:
        return self.root / key[:2] / f"{key}.json"

    def get(self, key: str) -> dict[str, Any] | None:
        p = self._path(key)
        if not p.exists():
            return None
        d: dict[str, Any] = json.loads(p.read_text())
        return d

    def put(self, key: str, value: dict[str, Any]) -> None:
        p = self._path(key)
        p.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=p.parent, suffix=".tmp")
        try:
            with os.fdopen(fd, "w") as f:
                json.dump(value, f)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, p)
        except BaseException:
            Path(tmp).unlink(missing_ok=True)
            raise


@dataclass
class LabelOutcome:
    probs: list[float] | None  # K_q+1, unknown last; None = FAILED
    status: str  # ok | failed | missing
    calls: int = 0
    cached: int = 0
    cost: float = 0.0


def _aggregate(per_order: dict[tuple[int, ...], list[float]], n: int, cap_all: bool) -> list[float]:
    if cap_all:
        from opendecision.teacher import cyclic_average

        opts = [str(i) for i in range(n)]
        table = {tuple(str(o) for o in k): v for k, v in per_order.items()}
        out = cyclic_average(lambda _s, _q, order: table[tuple(order)], None, {}, opts)
        return [float(x) for x in out]
    acc = np.zeros(n)
    for order, p in per_order.items():
        for pos, o in enumerate(order):
            acc[o] += p[pos]
    return [float(x) for x in acc / len(per_order)]


def label_question(
    q: Question,
    cfg: TeacherConfig,
    cache: LabelCache,
    client: TeacherClient | None,
) -> LabelOutcome:
    """client=None -> cache only (status 'missing' if any rotation was never labelled)."""
    n = len(q.options) + 1
    orders = rotations(q, cfg.rotations)
    out = LabelOutcome(None, "ok")
    per: dict[tuple[int, ...], list[float]] = {}
    for order in orders:
        prompt = build_prompt(q, order)
        key = cache_key(cfg, prompt, order)
        rec = cache.get(key)
        if rec is not None:
            out.cached += 1
        elif client is None:
            return LabelOutcome(None, "missing", out.calls, out.cached, out.cost)
        else:
            comp = client.complete(prompt)  # BudgetExceeded propagates
            probs = letter_probs(comp.top_logprobs, n)
            rec = {"probs": probs, "cost": comp.cost}
            cache.put(key, rec)
            out.calls += 1
            out.cost += comp.cost
        if rec["probs"] is None:
            out.status = "failed"  # never fabricate a label
        else:
            per[order] = rec["probs"]
    if out.status == "failed":
        return out
    out.probs = _aggregate(per, n, cap_all=len(per) == n)
    return out


def estimate_question(q: Question, cfg: TeacherConfig, cache: LabelCache) -> tuple[int, float]:
    """(uncached calls, estimated USD) without network."""
    calls, usd = 0, 0.0
    for order in rotations(q, cfg.rotations):
        prompt = build_prompt(q, order)
        if cache.get(cache_key(cfg, prompt, order)) is None:
            calls += 1
            usd += estimate_tokens(prompt) * cfg.price_prompt + cfg.price_completion
    return calls, usd
