"""A round's work is never minted for an admission convicted of training fraud (ADR 0011 Part 8)."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

from hypertrain.ledger.escrow_v2 import EscrowError

_spec = importlib.util.spec_from_file_location(
    "escrow_v2_fixtures", Path(__file__).with_name("test_escrow_v2.py")
)
assert _spec is not None and _spec.loader is not None
base = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = base
_spec.loader.exec_module(base)
ledger, verified_shadow, actual_reward = base.ledger, base.verified_shadow, base.actual_reward
__all__ = ["actual_reward"]


@pytest.mark.parametrize("convicted", [True, False], ids=["convicted", "not-convicted"])
def test_a_convicted_admissions_work_is_refused_and_an_honest_one_is_minted(
    actual_reward, tmp_path, convicted
):
    s, evidence, reward = actual_reward
    e = ledger(s, tmp_path / "ledger.db", replay=verified_shadow)
    e.banned = lambda hotkey: convicted
    if convicted:
        with pytest.raises(EscrowError, match="BANNED_WORK"):
            e.reward_finalize(reward, evidence)
        assert e.balances().issued == 0
    else:
        e.reward_finalize(reward, evidence)
        assert e.balances().issued == reward.budget_units
