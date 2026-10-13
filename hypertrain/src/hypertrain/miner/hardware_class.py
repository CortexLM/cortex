"""Hardware-class admission for heterogeneous islands (plan 6.4).

A class is (normalized GPU name, compute capability, torch version, CUDA runtime, determinism
settings digest). A miner joins only if every GPU in its island is in the run's allowed set; an
island mixing classes is admitted only when the run's reduction_order is "unit" (unit order makes
the result independent of which class computed which unit; rank order does not).
"""

from __future__ import annotations

import hashlib
from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass
from typing import Any

from hypertrain.miner.core import Gpu
from hypertrain.protocol.jcs import canonicalize

REDUCTION_ORDERS = ("rank", "unit")


@dataclass(frozen=True, order=True)
class HardwareClass:
    gpu: str
    capability: str
    torch: str
    cuda: str
    determinism: str


@dataclass(frozen=True)
class Admission:
    accepted: bool
    reason: str


def normalize_gpu(name: str) -> str:
    return " ".join(name.lower().replace("nvidia", "").split())


def determinism_digest(settings: Mapping[str, Any]) -> str:
    """sha256 of the JCS bytes of setup_determinism()'s settings (minus process-local keys)."""
    body = {k: v for k, v in settings.items() if k != "torch_preloaded"}
    return hashlib.sha256(canonicalize(body)).hexdigest()


def classes_for(
    gpus: Sequence[Gpu],
    capability: str,
    torch_version: str,
    cuda: str,
    settings: Mapping[str, Any],
) -> tuple[HardwareClass, ...]:
    d = determinism_digest(settings)
    return tuple(
        HardwareClass(normalize_gpu(g.name), capability, torch_version, cuda, d) for g in gpus
    )


def admit(
    run_classes: Collection[HardwareClass],
    declared: Sequence[HardwareClass],
    reduction_order: str = "rank",
) -> Admission:
    if reduction_order not in REDUCTION_ORDERS:
        return Admission(False, f"unknown reduction_order {reduction_order!r}")
    if not declared:
        return Admission(False, "no hardware declared")
    outside = sorted(set(declared) - set(run_classes))
    if outside:
        return Admission(False, f"hardware class not allowed by run: {outside[0]}")
    if len(set(declared)) > 1 and reduction_order != "unit":
        return Admission(False, "mixed hardware classes need reduction_order 'unit'")
    return Admission(True, "ok")
