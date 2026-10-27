"""Audit lease liveness: dating, head of line, single flight, cooldown, fail-fast, second window."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    D_AUDIT,
    lease,
    lease_rounds,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)

A = AUDITOR_A


def test_a_re_lease_after_the_first_window_validates(network):
    seed_round(network)
    schedule(network)
    late = D_AUDIT + 1  # an auditor that polls one beacon round after the audit round
    network.push(late)
    response = lease(network, A, 1)
    assert response.status_code == 200, response.text
    body = response.json()["body"]
    assert (body["created_beacon"], body["lease_expires"]) == (late, late + lease_rounds(network))
