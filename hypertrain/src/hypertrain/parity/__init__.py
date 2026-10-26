"""Parity gate harness: pre-registration, paired-seed TOST, power check, item bootstrap, benchmarks.

Plan todo 13 / parity draft section 3. Equivalence = the 90% CI of the paired mean difference lies
inside (-delta, +delta) (two one-sided tests at alpha 0.05). A failed test is reported as
"equivalence not demonstrated at delta=...", never as "within noise".
"""

from hypertrain.parity.stats import (
    MARGINS,
    PowerCheck,
    TostResult,
    item_bootstrap_ci90,
    power_check,
    t_ppf95,
    tost_paired,
)

__all__ = [
    "MARGINS",
    "PowerCheck",
    "TostResult",
    "item_bootstrap_ci90",
    "power_check",
    "t_ppf95",
    "tost_paired",
]
