"""Closed-form audit and vesting tables used by docs/mechanism.md (pure math, no randomness).

Run: uv run --frozen python scripts/gen_mechanism_tables.py \
> experiments/results/mechanism_tables.json
"""

import json
import math
from fractions import Fraction


def cumulative(q: Fraction, n: int) -> float:
    return float(1 - (1 - q) ** n)


def segments(u: int, k: int, c: int) -> float:
    """P(at least one of c cheated segments is drawn) with k random draws, final not involved."""
    return 1 - math.comb(u - c, k) / math.comb(u, k)


def main() -> None:
    qs = {"q005": Fraction(1, 20), "q010": Fraction(1, 10), "q025": Fraction(1, 4)}
    out: dict[str, object] = {
        "cumulative_detection": {
            q: {str(n): cumulative(v, n) for n in (1, 5, 10, 20)} for q, v in qs.items()
        },
        "vesting_rounds_E": {q: math.ceil(1 / v) for q, v in qs.items()},
        "segments_U30_k3": {
            "no_final_involved": {str(c): segments(30, 3, c) for c in (1, 3, 10, 29)},
            "last_step_only_without_final_rule": 3 / 30,
            "last_step_only_with_final_rule": 1.0,
        },
        "reward_example": {
            "round_budget_units": 10**6,
            "three_equal_miners_entitlement": 10**6 // 3,
            "burn_with_one_faulty_of_three": 10**6 - 2 * (10**6 // 3),
            "f_min": 0.8,
            "f_max": 1.0,
        },
        "deterrence": {
            "rule": "q*(S+R) >= G with S=E*R, G=g*R, g<=1 gives E >= g/q - 1; E=ceil(1/q)",
            "g": 1.0,
        },
    }
    print(json.dumps(out, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
