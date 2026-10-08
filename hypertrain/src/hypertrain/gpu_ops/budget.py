"""Admission budget gate (plan todo 12 budget rules; pattern research-quality-multiregion task8).

effective_cap = min(USD50, prepaid credit at snapshot-0), shared by todos 12+16.
Phase A cap = USD10 including its USD5 cleanup reserve.
debits_so_far = snapshot-0 credit - current credit (conservative prepaid accounting).
Admit only if worst-case (price x hard deadline + storage + egress, per host) + reserve fits
the remaining Phase A cap, the remaining effective cap, AND the available credit.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from decimal import ROUND_CEILING, Decimal
from typing import Any

GLOBAL_CAP_USD = Decimal("50")
PHASE_A_CAP_USD = Decimal("10")
CLEANUP_RESERVE_USD = Decimal("5")
HOURS_PER_MONTH = Decimal("720")


def dec(value: Any) -> Decimal | None:
    if isinstance(value, bool) or value is None:
        return None
    try:
        d = Decimal(str(value))
    except ArithmeticError:
        return None
    return d if d.is_finite() and d >= 0 else None


def cents_up(x: Decimal) -> Decimal:
    return x.quantize(Decimal("0.01"), rounding=ROUND_CEILING)


def host_worst_case(
    offer: Mapping[str, Any], hours: Decimal, disk_gb: int, egress_gb: int
) -> Decimal:
    dph = dec(offer.get("dph_total"))
    storage = dec(offer.get("storage_cost"))
    up, down = dec(offer.get("inet_up_cost")), dec(offer.get("inet_down_cost"))
    if None in (dph, storage, up, down):
        raise ValueError("offer price fields missing or invalid")
    assert dph is not None and storage is not None and up is not None and down is not None
    storage_usd = storage * Decimal(disk_gb) * hours / HOURS_PER_MONTH
    egress_usd = max(up, down) * Decimal(egress_gb)
    return cents_up(dph * hours + storage_usd + egress_usd)


def evaluate(
    *,
    snapshot0_credit: Decimal,
    current_credit: Decimal,
    offers: Sequence[Mapping[str, Any]],
    hard_deadline_seconds: int,
    disk_gb: int,
    egress_gb: int,
    phase_cap: Decimal = PHASE_A_CAP_USD,
    reserve: Decimal = CLEANUP_RESERVE_USD,
) -> dict[str, Any]:
    hours = Decimal(hard_deadline_seconds) / Decimal(3600)
    effective_cap = min(GLOBAL_CAP_USD, snapshot0_credit)
    debits = max(Decimal(0), snapshot0_credit - current_credit)
    per_host = [host_worst_case(o, hours, disk_gb, egress_gb) for o in offers]
    hardware = sum(per_host, Decimal(0))
    worst = hardware + reserve
    checks = {
        "phase_cap": debits + worst <= phase_cap,
        "effective_cap": debits + worst <= effective_cap,
        "credit_covers": worst <= current_credit,
        "reserve_within_phase_cap": reserve <= phase_cap,
    }
    return {
        "admit": all(checks.values()),
        "checks": checks,
        "effective_cap_usd": str(effective_cap),
        "phase_cap_usd": str(phase_cap),
        "debits_so_far_usd": str(debits),
        "per_host_worst_case_usd": [str(x) for x in per_host],
        "hardware_worst_case_usd": str(hardware),
        "reserve_usd": str(reserve),
        "worst_case_total_usd": str(worst),
        "hard_deadline_hours": str(hours),
        "current_credit_usd": str(current_credit),
        "snapshot0_credit_usd": str(snapshot0_credit),
    }
