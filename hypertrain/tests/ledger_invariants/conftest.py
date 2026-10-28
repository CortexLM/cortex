"""Hypothesis profile and session fixtures of the ledger-invariants lot.

Nothing here reaches a test outside this directory: the profile is loaded only while a module of
this directory is imported, because `@given` reads the default settings at decoration time.
"""

from __future__ import annotations

from collections.abc import Iterator
from types import ModuleType
from typing import Any

import pytest
from hypothesis import HealthCheck, settings
from hypothesis.configuration import set_hypothesis_home_dir

from .support import ESCROW_FIXTURE, LOT, Real, load_fixture_module

settings.register_profile(
    "li",
    derandomize=True,
    database=None,
    deadline=None,
    max_examples=40,
    suppress_health_check=[HealthCheck.too_slow, HealthCheck.function_scoped_fixture],
)


@pytest.hookimpl(wrapper=True)
def pytest_make_collect_report(collector: pytest.Collector) -> Any:
    if not (isinstance(collector, pytest.Module) and LOT in collector.path.parents):
        return (yield)
    previous = settings.get_current_profile_name()
    settings.load_profile("li")
    try:
        return (yield)
    finally:
        settings.load_profile(previous)


@pytest.fixture(scope="module", autouse=True)
def _hypothesis_home(tmp_path_factory: pytest.TempPathFactory) -> Iterator[None]:
    """Keep hypothesis' constants cache out of the working tree."""
    set_hypothesis_home_dir(tmp_path_factory.mktemp("hypothesis"))
    yield
    set_hypothesis_home_dir(None)


@pytest.fixture(scope="session")
def li_escrow() -> ModuleType:
    """tests/ledger/test_escrow_v2.py, loaded once: setup, ledger, fund, lock, constants."""
    return load_fixture_module(*ESCROW_FIXTURE)


@pytest.fixture(scope="session")
def real_reward(li_escrow: ModuleType, tmp_path_factory: pytest.TempPathFactory) -> Real:
    """(Setup, FinalityEvidence, RewardFinalize) of a real w=0 round: two CPU executions of the
    tiny model, auditor MATCH, coordinator-signed Finalize and reward. About 8 s per process."""
    reward: Real = li_escrow.actual_reward.__wrapped__(tmp_path_factory)
    return reward
