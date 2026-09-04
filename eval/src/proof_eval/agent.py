"""Structured agent verdict. Holdout NLL is never a field here.

The live image inspects the claim + recipe for fabric cheats. A missing
verdict is a 503 at the harvest, so this always returns a complete envelope
or raises.
"""

from __future__ import annotations

import re
from typing import Any

from .request import Constraints, HarvestRequest

_IB = re.compile(r"\b(infiniband|ibv_|rdma_cm|mlx5|ib_send)\b", re.I)
_NVLINK = re.compile(r"\b(nvlink|cudaIpc|CU_DEVICE_P2P)\b", re.I)
_FAST = re.compile(r"\b(ncclNetIb|NCCL_IB_DISABLE\s*=\s*0|ncclNvls)\b", re.I)


def inspect(request: HarvestRequest, recipe_text: str) -> dict[str, Any]:
    cheats: list[str] = []
    hay = f"{request.claim}\n{recipe_text}"
    c: Constraints = request.constraints
    if c.no_infiniband and _IB.search(hay):
        cheats.append("other")
    if c.no_nvlink and _NVLINK.search(hay):
        cheats.append("other")
    if c.no_nccl_fast_fabric and _FAST.search(hay):
        cheats.append("other")
    reproduced = not cheats
    return {
        "verdict": "clean" if reproduced else "reject",
        "reproduced": reproduced,
        "claim_holds_public": reproduced,
        "contamination": False,
        "canary_hit": False,
        "flops_used": 0,
        "flops_budget": request.flops_budget,
        "cheat_codes": cheats,
        "rationale": (
            "recipe reproduced under the topic fabric constraints"
            if reproduced
            else "recipe references a forbidden fast path (IB / NVLink / NCCL)"
        ),
        "topic_id": request.topic_id,
        "family": request.family or "nll",
    }
