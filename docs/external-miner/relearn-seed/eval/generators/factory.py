"""Synthetic factory: champion generates, frozen GLM-5.3 judges.

Train and eval generators are disjoint (different seeds). Items that
collide with the official-bench blocklist are dropped.
"""

from __future__ import annotations

import hashlib

from decontam.benches import is_contaminated


def generate(kind: str, seed: str, n: int) -> list[str]:
    if kind not in {"train", "eval"}:
        raise ValueError("kind must be train or eval")
    out: list[str] = []
    i = 0
    while len(out) < n:
        h = hashlib.sha256(f"{kind}:{seed}:{i}".encode()).hexdigest()
        prompt = f"relearn-{kind}-{h[:16]}"
        if not is_contaminated(prompt):
            out.append(prompt)
        i += 1
    return out


def disjoint(seed: str, n: int) -> tuple[list[str], list[str]]:
    train = generate("train", seed + "-train", n)
    eval_ = generate("eval", seed + "-eval", n)
    if set(train) & set(eval_):
        raise RuntimeError("train/eval generators collided")
    return train, eval_
