"""Model architecture registry (ADR 0006): arch name -> implementation module.

Stdlib only, and it never imports the modules it names, so torch-free protocol validation reads
the same table that ``trainer.model`` dispatches on. It is read-only and covered by
``implementation_hash`` (challenge/store.py): there is no runtime registration.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from types import MappingProxyType


@dataclass(frozen=True)
class Arch:
    """``module`` exposes param_shapes / stage_of / init_params / forward and, for disputes,
    traced_forward. ``decoder`` is built into ``hypertrain.trainer.model``."""

    module: str


ARCH_REGISTRY: Mapping[str, Arch] = MappingProxyType(
    {
        "decoder": Arch("hypertrain.trainer.model"),
        "kimi-k3-pro-tiny": Arch("hypertrain.models.kimi_tiny"),
        "od-encoder": Arch("hypertrain.models.opendecision"),
    }
)
