"""Model architecture registry (ADR 0006): arch name -> implementation module.

Stdlib only, and it never imports the modules it names, so torch-free protocol validation reads
the same table that ``trainer.model`` dispatches on. It is read-only and covered by
``implementation_hash`` (challenge/store.py): there is no runtime registration.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from types import MappingProxyType

REDUCTION_ORDERS = ("rank", "unit")


@dataclass(frozen=True)
class Arch:
    """``module`` exposes param_shapes / stage_of / init_params / forward and, for disputes,
    traced_forward. ``decoder`` is built into ``hypertrain.trainer.model``. ``max_ranks`` bounds
    the GPUs of one island (None: no bound). ``reduction_orders`` are the REDUCTION_ORDERS its
    leaves are defined under; "unit" needs evidence that a unit's gradient does not depend on
    the rank that computed it (tests/layout/test_unit_order_canonical.py)."""

    module: str
    max_ranks: int | None = None
    reduction_orders: tuple[str, ...] = ("rank",)


ARCH_REGISTRY: Mapping[str, Arch] = MappingProxyType(
    {
        "decoder": Arch("hypertrain.trainer.model", reduction_orders=REDUCTION_ORDERS),
        # The router bias moves in place from the local expert load at every backward pass and
        # no load is all-reduced, so a second rank or unit would train a different bias.
        "kimi-k3-pro-tiny": Arch("hypertrain.models.kimi_tiny", max_ranks=1),
        "od-encoder": Arch("hypertrain.models.opendecision"),
    }
)


def check_reduction_order(arch: str, order: str) -> None:
    """The reduction-order guard of unit-order training and hardware admission."""
    allowed = ARCH_REGISTRY[arch].reduction_orders if arch in ARCH_REGISTRY else ()
    if order not in allowed:
        raise ValueError(
            f"arch {arch!r} does not allow reduction_order {order!r}: allowed {list(allowed)}"
        )
