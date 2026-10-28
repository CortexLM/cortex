"""A confirmed audit mismatch is enforced unless the deployment says otherwise (D81).

ADR 0011 Part 8, decision 18. The switch is a keyword of the store and a field of the app's
Config, not a manifest field and not an environment variable. The sanction itself, end to end
through the beacon sweeper, is tested in tests/e2e/test_mismatch_sanction_v2_e2e.py.
"""

from __future__ import annotations

import dataclasses
from pathlib import Path

import pytest

from hypertrain.challenge.app import create_app
from hypertrain.challenge.store import ChallengeStore

from .conftest import make_config, verify_fixture


def store_of(tmp_path: Path, **changes: bool) -> ChallengeStore:
    config = dataclasses.replace(
        make_config(tmp_path / "state", tmp_path / "no-secrets"), **changes
    )
    store: ChallengeStore = create_app(config, verify_beacon=verify_fixture).state.store
    return store


def test_the_app_enforces_a_confirmed_mismatch_by_default(tmp_path: Path) -> None:
    store = store_of(tmp_path)
    try:
        assert store.enforce_audit_mismatch is True
    finally:
        store.close()


def test_the_app_hands_the_record_only_mode_it_is_given_to_its_store(tmp_path: Path) -> None:
    store = store_of(tmp_path, enforce_audit_mismatch=False)
    try:
        assert store.enforce_audit_mismatch is False
    finally:
        store.close()


@pytest.mark.parametrize("enforce", [True, False])
def test_a_child_store_takes_the_switch_of_its_parent(tmp_path: Path, enforce: bool) -> None:
    parent = store_of(tmp_path, enforce_audit_mismatch=enforce)
    child = parent._open_child(tmp_path / "child")
    try:
        assert child.enforce_audit_mismatch is enforce
    finally:
        child.close()
        parent.close()
