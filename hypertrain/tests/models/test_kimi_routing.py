"""Tie-breaking of the kimi router at PROFILE width (32 experts, top-4), ADR 0006.

A CPU unstable sort keeps index order at 8 experts or fewer but not at 32, so the 4-expert tie row
of test_kimi_tiny.py cannot tell a stable ``route`` from ``torch.sort(stable=False)``.
Replay needs every host to pick the same experts, hence ties go to the lowest expert index.
"""

from __future__ import annotations

import pytest
import torch

import hypertrain.models.kimi_tiny as kt

# The rows below name expert ids up to 31 and the expected picks assume top-4: both are constants
# of the test, not read from PROFILE, so a profile change fails
# test_profile_has_the_routing_shape_these_rows_assume and does not abort collection.
E, K = 32, 4


def _row(base: float, **at: float) -> torch.Tensor:
    """Scores of one token: ``base`` everywhere, ``e<i>=v`` overrides expert i."""
    row = torch.full((E,), base)
    for name, value in at.items():
        row[int(name[1:])] = value
    return row


def _bias(*experts: int, value: float = 0.25) -> torch.Tensor:
    bias = torch.zeros(E)
    bias[list(experts)] = value
    return bias


CASES = {
    "all-equal": ([_row(0.5)] * 3, _bias(), [[0, 1, 2, 3]] * 3),
    "partial-ties": (
        [
            # one clear winner, then six tied experts: the lowest three
            _row(0.5, e28=0.9, e2=0.7, e9=0.7, e16=0.7, e17=0.7, e24=0.7, e31=0.7),
            # two clear winners, then a tie among everything else
            _row(0.3, e20=0.95, e5=0.85),
            # a tie at the top
            _row(0.1, e7=0.9, e8=0.9),
        ],
        _bias(),
        [[28, 2, 9, 16], [20, 5, 0, 1], [7, 8, 0, 1]],
    ),
    # 0.25 + 0.25 == 0.5 exactly in float32: the bias creates the tie
    "tie-made-by-the-bias": (
        [_row(0.25, e20=0.5, e25=0.5, e30=0.5, e31=0.5)] * 2,
        _bias(3, 10),
        [[3, 10, 20, 25]] * 2,
    ),
    "bias-evens-out-every-expert": (
        [torch.tensor([0.5, 0.25] * (E // 2))] * 2,
        torch.tensor([0.0, 0.25] * (E // 2)),
        [[0, 1, 2, 3]] * 2,
    ),
}


def test_profile_has_the_routing_shape_these_rows_assume() -> None:
    assert (kt.PROFILE.n_experts, kt.PROFILE.top_k_experts) == (E, K)


@pytest.mark.parametrize("case", CASES)
def test_ties_go_to_the_lowest_expert_ids_in_rank_major_order_when_profile_width(case: str) -> None:
    rows, bias, want = CASES[case]
    experts = kt.topk_experts(torch.stack(rows), bias, K)
    assert experts.view(K, len(rows)).t().tolist() == want
