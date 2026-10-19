"""Does a matured contract-2 reward origin ever reach GET /internal/v1/get_weights?"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location(
    "v2_weights_bridge_e2e", Path(__file__).parents[1] / "e2e/test_service_network_v2_e2e.py"
)
assert spec is not None and spec.loader is not None
e2e = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = e2e
spec.loader.exec_module(e2e)
network = e2e.network


@pytest.mark.slow
@pytest.mark.xfail(
    strict=True,
    reason="v2 rewards not bridged to get_weights: store.py:2000 reads only the v1 Ledger",
)
def test_v2_matured_reward_reaches_get_weights(network, monkeypatch, tmp_path):
    # Given: the shortest existing flow to a live round 0 that finalize_v2 settles.
    e2e._graduate_four(network)
    e2e._live_checkpoint(network)
    store = network.store
    run_id = network.manifest.run_id()
    assert store._record_v2(run_id, "replayed-finality", "0")["shadow"] is False
    origin, hotkey, mature_at = store._db.execute(
        "SELECT origin,hotkey,mature_at FROM escrow_origins WHERE reward_round=0 AND issued>0 "
        "ORDER BY hotkey LIMIT 1"
    ).fetchone()
    # When: beacon rounds pass mature_at (push_beacon matures it), then Cortex asks for weights.
    network.push(max(network.now, mature_at) + 1)
    matured = store._db.execute(
        "SELECT SUM(units) FROM escrow_units WHERE origin=? AND bucket='available'", (origin,)
    ).fetchone()[0]
    assert matured and matured > 0
    epoch_at = int(store.clock())
    epoch = (epoch_at - store.params.genesis_unix) // store.params.epoch_seconds
    token = tmp_path / "internal"
    token.write_text("integration-internal")
    route = next(
        r for r in network.client.app.routes if getattr(r, "path", "") == "/internal/v1/get_weights"
    )
    code = route.endpoint.__code__
    config = route.endpoint.__closure__[code.co_freevars.index("config")].cell_contents
    monkeypatch.setattr(config, "internal_token_file", token)
    response = network.client.get(
        f"/internal/v1/get_weights?epoch={epoch}&epoch_at={epoch_at}",
        headers={
            "Authorization": "Bearer integration-internal",
            "x-platform-challenge-slug": config.slug,
        },
    )
    # Then: the matured v2 reward is paid as weight to its hotkey.
    assert response.status_code == 200, response.text
    assert response.json()["weights"].get(hotkey, 0) > 0, response.text
