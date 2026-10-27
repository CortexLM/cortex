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
        response = network.challenge(identity)
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
    response = network.challenge(newcomer)
    assert response.status_code == 200, response.text


def test_unfunded_join_past_its_window_does_not_block_its_coldkey(network):
    # Given: HOT[0] applied with COLD[0] and never funded or asked for its challenge
    first = network.join(0)
    assert first.status_code == 200, first.text
    network.push(103)  # its window (to round 102) is over
    # When: the same coldkey applies again with another registered hotkey
    request = service.sign_join(
        service.HOT[1],
        service.COLD[0],
        run_id=network.manifest.run_id(),
        request_id="ab" * 32,
        expires_beacon=10000,
        policy_hash=network.manifest.network.admission_policy_hash,
        hardware_hint=service.HardwareHint(device_name="cpu", device_count=1, driver="advisory"),
    )
    second = network.client.post(
        network.url + "/join", content=service.canonicalize(request.body())
    )
    # Then: the dead application was retired by the dispatcher's join
    assert second.status_code == 200, second.text
    record = network.client.get(network.url + f"/admission/{service.HOT[0].ss58}").json()["record"]
    assert (record["state"], record["reason"]) == ("SUSPENDED", "APPLIED_EXPIRED")
