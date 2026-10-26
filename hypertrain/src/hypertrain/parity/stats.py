"""Paired-seed TOST, seed-variance power check and hierarchical item bootstrap (numpy only)."""

from __future__ import annotations

import math
import statistics
from collections.abc import Sequence
from dataclasses import asdict, dataclass
from typing import Any

import numpy as np

# Pre-registered equivalence margins (plan todo 13; parity draft section 3).
# loss_rel is relative ((L_arm - L_ctrl) / L_ctrl); the others are absolute accuracy points.
MARGINS: dict[str, float] = {"loss_rel": 0.005, "mmlu": 1.0, "bench_mean": 1.0, "gsm8k": 2.0}
ALPHA = 0.05

# One-sided Student t 0.95 quantiles; df between rows uses the next lower row (conservative).
_T95 = {
    1: 6.314,
    2: 2.920,
    3: 2.353,
    4: 2.132,
    5: 2.015,
    6: 1.943,
    7: 1.895,
    8: 1.860,
    9: 1.833,
    10: 1.812,
    11: 1.796,
    12: 1.782,
    13: 1.771,
    14: 1.761,
    15: 1.753,
    16: 1.746,
    17: 1.740,
    18: 1.734,
    19: 1.729,
    20: 1.725,
    25: 1.708,
    30: 1.697,
    40: 1.684,
    60: 1.671,
    120: 1.658,
}


def t_ppf95(df: int) -> float:
    if df < 1:
        raise ValueError("df must be >= 1")
    return _T95[max(k for k in _T95 if k <= df)]


@dataclass(frozen=True)
class TostResult:
    metric: str
    delta: float
    n: int
    mean: float
    sd: float
    ci90: tuple[float, float]
    equivalent: bool
    non_inferior: bool
    verdict: str

    def as_dict(self) -> dict[str, Any]:
        return asdict(self)


def tost_paired(diffs: Sequence[float], delta: float, metric: str = "loss_rel") -> TostResult:
    """Two one-sided tests at alpha 0.05 on paired differences (arm - control).

    Equivalent iff the 90% t-CI of the mean difference lies strictly inside (-delta, +delta);
    non-inferior iff its upper bound is below +delta (higher difference = worse arm).
    """
    xs = [float(x) for x in diffs]
    if delta <= 0:
        raise ValueError("delta must be > 0")
    n = len(xs)
    if n < 2 or not all(math.isfinite(x) for x in xs):
        m = statistics.fmean(xs) if xs else math.nan
        return TostResult(
            metric,
            delta,
            n,
            m,
            math.nan,
            (-math.inf, math.inf),
            False,
            False,
            f"equivalence not demonstrated at delta={delta:g} (need >= 2 finite paired seeds)",
        )
    m, sd = statistics.fmean(xs), statistics.stdev(xs)
    hw = t_ppf95(n - 1) * sd / math.sqrt(n)
    lo, hi = m - hw, m + hw
    eq = -delta < lo and hi < delta
    ci = f"90% CI [{lo:+.4g}, {hi:+.4g}], mean {m:+.4g}"
    verdict = (
        f"equivalent at delta={delta:g} ({ci})"
        if eq
        else f"equivalence not demonstrated at delta={delta:g} ({ci})"
    )
    return TostResult(metric, delta, n, m, sd, (lo, hi), eq, hi < delta, verdict)


@dataclass(frozen=True)
class PowerCheck:
    n: int
    sd_diff: float
    delta: float
    ci90_half_width: float
    power_at_zero: float
    n_required: int | None
    warning: str | None


def _power(sd: float, n: int, delta: float) -> tuple[float, float]:
    """Approximate P(TOST passes | true diff = 0): P(|mean| < delta - hw), hw at the sample sd."""
    se = sd / math.sqrt(n)
    hw = t_ppf95(n - 1) * se
    if hw >= delta:
        return hw, 0.0
    if se == 0:
        return hw, 1.0
    z = (delta - hw) / se
    return hw, max(0.0, math.erf(z / math.sqrt(2)))


def power_check(sd_diff: float, n: int, delta: float, target: float = 0.8) -> PowerCheck:
    """Seed-variance power check; warns when n paired seeds cannot reach ``target`` power."""
    if n < 2 or sd_diff < 0 or delta <= 0:
        raise ValueError("need n >= 2, sd_diff >= 0, delta > 0")
    hw, pw = _power(sd_diff, n, delta)
    need = next((k for k in range(2, 201) if _power(sd_diff, k, delta)[1] >= target), None)
    warn = None
    if pw < target:
        warn = (
            f"underpowered: n={n} paired seeds, sd of differences {sd_diff:.4g} -> 90% CI "
            f"half-width {hw:.4g} vs delta {delta:g}; power at true difference 0 ~{pw:.2f} "
            f"(< {target}); need n >= {need if need else '>200'}"
        )
    return PowerCheck(n, sd_diff, delta, hw, pw, need, warn)


def item_bootstrap_ci90(
    arm: npt_like, ctrl: npt_like, n_boot: int = 2000, seed: int = 0
) -> tuple[float, tuple[float, float]]:
    """Hierarchical paired bootstrap of the accuracy difference in points (arm - control).

    ``arm``/``ctrl`` are 0/1 correctness arrays [n_seeds, n_items] on the same items and seeds.
    Each replicate resamples seeds and items with replacement (seed variance + item variance).
    """
    a, c = np.asarray(arm, dtype=np.float64), np.asarray(ctrl, dtype=np.float64)
    if a.shape != c.shape or a.ndim != 2 or a.size == 0:
        raise ValueError("arm and ctrl must be equal-shape [n_seeds, n_items] arrays")
    d = (a - c) * 100.0
    rng = np.random.default_rng(seed)
    s, n = d.shape
    si = rng.integers(0, s, size=(n_boot, s))
    ii = rng.integers(0, n, size=(n_boot, n))
    reps = np.array([d[si[b]][:, ii[b]].mean() for b in range(n_boot)])
    lo, hi = np.quantile(reps, [0.05, 0.95])
    return float(d.mean()), (float(lo), float(hi))


npt_like = Any
