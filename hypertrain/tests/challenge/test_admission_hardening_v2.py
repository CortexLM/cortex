"""Store-level regressions of the admission hardening lot (ADR 0010), through the HTTP routes."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "admission_hardening_service_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert _spec is not None and _spec.loader is not None
service = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = service
_spec.loader.exec_module(service)
network = service.network


def open_trials(n) -> int:
    return n.store._db.execute(
        "SELECT COUNT(*) FROM admission_trial_results WHERE outcome='OPEN'"
    ).fetchone()[0]


def test_beacon_push_frees_the_places_of_unproven_challenges(network):
    # Given: two identities open a trial each (window 2..102) and never prove it
    ids = [network.join(i).json()["admission_id"] for i in (0, 1)]
    network.push(2)
    for identity in ids:
        response = network.client.get(network.url + f"/join/{identity}/challenge")
        assert response.status_code == 200, response.text
    assert open_trials(network) == 2
    # When: the beacon passes the deadline and nobody asks for anything
    network.push(103)
    # Then: the places are free without any request, and nobody was suspended
    assert open_trials(network) == 0
    states = network.store._db.execute("SELECT state FROM admissions_v2").fetchall()
    assert [row[0] for row in states] == ["APPLIED", "APPLIED"]
    # and a newcomer gets its trial instead of TRIAL_CAPACITY
    newcomer = network.join(2).json()["admission_id"]
    network.push(104)
    response = network.client.get(network.url + f"/join/{newcomer}/challenge")
    assert response.status_code == 200, response.text
