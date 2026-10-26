"""deploy/challenge-profile-row.example.toml: a full profile an operator can copy as is."""

import tomllib
from pathlib import Path

EXAMPLE = Path(__file__).resolve().parent.parent / "deploy/challenge-profile-row.example.toml"


def test_activation_example_shares_sum_to_10000_and_hypertrain_positive() -> None:
    profile = tomllib.loads(EXAMPLE.read_text())
    shares = {c["id"]: c["emission_share_bps"] for c in profile["challenges"]}
    assert sum(shares.values()) == 10000
    assert shares["hypertrain"] > 0
    assert profile["version"] >= 3  # a third challenge needs version >= 3
