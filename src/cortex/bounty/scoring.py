"""Bounty's integer-only precision, severity and displacement rules."""

from dataclasses import dataclass, field

SCORE_MAX = 1_000_000
MIN_PRECISION_BPS = 6_000
MAX_TRIAGE_NOISE_BPS = 5_000
SEVERITY_BPS = {"trivial": 625, "minor": 2_500, "major": 5_000, "critical": 10_000}


@dataclass
class Holdout:
    valid_by_severity: dict[str, int] = field(default_factory=dict)
    valid_unpriced: int = 0
    malicious: int = 0
    duplicate: int = 0
    already_fixed: int = 0

    def record(self, verdict: str, severity: str | None = None) -> None:
        if verdict == "valid":
            if severity is None:
                self.valid_unpriced += 1
            elif severity not in SEVERITY_BPS:
                raise ValueError("unknown severity")
            else:
                self.valid_by_severity[severity] = self.valid_by_severity.get(severity, 0) + 1
        elif verdict == "invalid_malicious":
            self.malicious += 1
        elif verdict == "duplicate":
            self.duplicate += 1
        elif verdict == "already_fixed_not_prod":
            self.already_fixed += 1
        else:
            raise ValueError("unknown adjudication")

    @property
    def valid(self) -> int:
        return sum(self.valid_by_severity.values())

    @property
    def decided(self) -> int:
        return self.valid + self.malicious

    @property
    def precision_bps(self) -> int | None:
        return self.valid * 10_000 // self.decided if self.decided else None

    @property
    def impact_bps(self) -> int | None:
        total = sum(
            SEVERITY_BPS[severity] * count for severity, count in self.valid_by_severity.items()
        )
        return total // self.valid if self.valid else None

    @property
    def noise_bps(self) -> int | None:
        noise = self.duplicate + self.already_fixed
        total = self.decided + self.valid_unpriced + noise
        return noise * 10_000 // total if total else None

    @property
    def net_credit(self) -> int:
        # Preserve the contract's per-report integer truncation (trivial = 6).
        return (
            sum((100 * SEVERITY_BPS[s] // 10_000) * n for s, n in self.valid_by_severity.items())
            - 100 * self.malicious
        )


@dataclass(frozen=True)
class ChampionVerdict:
    eligible: bool
    lattice: int
    failed: tuple[str, ...]
    challenger_precision_bps: int | None
    challenger_impact_bps: int | None
    challenger_noise_bps: int | None


def judge_challenger(champion: Holdout, challenger: Holdout) -> ChampionVerdict:
    failed = []
    precision, impact, noise = challenger.precision_bps, challenger.impact_bps, challenger.noise_bps
    if challenger.decided < 3:
        failed.append("thin_holdout")
    if challenger.net_credit < 0:
        failed.append("penalty")
    if challenger.valid_unpriced:
        failed.append("severity_evidence_missing")
    if precision is not None and precision < MIN_PRECISION_BPS:
        failed.append("precision_floor")
    if noise is not None and noise > MAX_TRIAGE_NOISE_BPS:
        failed.append("triage_noise")
    if precision is None or (
        champion.precision_bps is not None and precision <= champion.precision_bps
    ):
        failed.append("no_precision_win")
    lattice = (
        SCORE_MAX * precision * impact // 100_000_000
        if not failed and precision is not None and impact is not None
        else 0
    )
    return ChampionVerdict(not failed, lattice, tuple(failed), precision, impact, noise)


@dataclass(frozen=True)
class BountyScore:
    """Exactly one outcome per expected hotkey; absence always has zero value."""

    value: int = 0
    reason: str | None = None
