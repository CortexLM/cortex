"""Honeypot miners (ultrabrain section 4, auditor honesty items 3-4).

The operator runs honeypot islands; their identities are committed per epoch as
sha256(JCS(sorted [{hotkey, mode}]) || salt) inside RoundOpen.honeypot_commit and revealed
(list + salt) after the epoch. Adversarial honeypots measure the catch rate, honest ones the
false-positive rate.
"""

from __future__ import annotations

import hashlib
import hmac
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Literal

import torch

import hypertrain.trainer  # noqa: F401
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.jcs import canonicalize
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, Params, RoundResult, SampleFn, train_round

Mode = Literal["honest", "fabricate", "last_step", "noise"]
FAULT_RESULTS = {"MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"}


@dataclass(frozen=True)
class Honeypot:
    hotkey: str
    mode: Mode

    @property
    def adversarial(self) -> bool:
        return self.mode != "honest"


def _preimage(pots: Sequence[Honeypot]) -> bytes:
    rows = sorted(({"hotkey": p.hotkey, "mode": p.mode} for p in pots), key=lambda r: r["hotkey"])
    if len({r["hotkey"] for r in rows}) != len(rows):
        raise ValueError("duplicate honeypot hotkey")
    return canonicalize(rows, allow_float=False)


def commit(pots: Sequence[Honeypot], salt: bytes) -> str:
    if len(salt) < 16:
        raise ValueError("salt must be >= 16 bytes")
    return hashlib.sha256(_preimage(pots) + salt).hexdigest()


def verify_reveal(commitment: str, pots: Sequence[Honeypot], salt: bytes) -> bool:
    try:
        return hmac.compare_digest(commit(pots, salt), commitment)
    except ValueError:
        return False


def honeypot_round(
    pot: Honeypot,
    cfg: TrainConfig,
    theta_start: Params,
    a: Assignment,
    get_sample: SampleFn,
    v0: Params | None = None,
) -> RoundResult:
    """Train the round as the honeypot's mode dictates. Adversarial modes mirror real cheats:
    ``fabricate`` commits leaves of a run from a shifted start (fake weights), ``last_step``
    perturbs the final step, ``noise`` adds 1e-6 to every weight at the middle step."""
    at = cfg.inner.H if pot.mode == "last_step" else max(1, cfg.inner.H // 2)

    def hook(t: int, theta: Params) -> None:
        if t != at:
            return
        with torch.no_grad():
            if pot.mode == "noise":
                for x in theta.values():
                    x.add_(1e-6)
            elif pot.mode == "last_step":
                theta[sorted(theta)[0]].view(-1)[0] += 1e-3

    if pot.mode != "fabricate":
        return train_round(cfg, theta_start, a, get_sample, v0=v0, after_step=hook)
    res = train_round(cfg, {n: x * 1.01 for n, x in theta_start.items()}, a, get_sample, v0=v0)
    res.leaves[0] = train_round(cfg, theta_start, a, get_sample, v0=v0).leaves[0]
    res.leaves_root = MerkleTree(res.leaf_digests).root.hex()
    return res


@dataclass(frozen=True)
class EpochRates:
    epoch: int
    adversarial_audited: int
    caught: int
    honest_audited: int
    false_positives: int

    @property
    def catch_rate(self) -> float | None:
        return self.caught / self.adversarial_audited if self.adversarial_audited else None

    @property
    def false_positive_rate(self) -> float | None:
        return self.false_positives / self.honest_audited if self.honest_audited else None

    def publish(self) -> dict[str, object]:
        return {
            "epoch": self.epoch,
            "adversarial_audited": self.adversarial_audited,
            "caught": self.caught,
            "catch_rate": self.catch_rate,
            "honest_audited": self.honest_audited,
            "false_positives": self.false_positives,
            "false_positive_rate": self.false_positive_rate,
        }


def epoch_rates(
    epoch: int,
    commitment: str,
    pots: Sequence[Honeypot],
    salt: bytes,
    verdicts: Mapping[str, Sequence[str]],
) -> EpochRates:
    """``verdicts[hotkey]`` = final results (after disputes) of every audited honeypot round."""
    if not verify_reveal(commitment, pots, salt):
        raise ValueError("honeypot reveal does not match the epoch commitment")
    adv = caught = hon = fp = 0
    for p in pots:
        for r in verdicts.get(p.hotkey, ()):
            bad = r in FAULT_RESULTS
            if p.adversarial:
                adv, caught = adv + 1, caught + bad
            else:
                hon, fp = hon + 1, fp + bad
    return EpochRates(epoch, adv, caught, hon, fp)
