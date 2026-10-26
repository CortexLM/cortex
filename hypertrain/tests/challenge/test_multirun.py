"""Several runs (v1 and v2) hosted by one challenge state directory, each isolated."""

from __future__ import annotations

from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge.store import ChallengeError
from hypertrain.data.store import ObjectNotFound
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import QUICKNET_GENESIS

from .conftest import WORKER, Master, Net, bearer, make_client
from .test_service_network_v2 import (  # noqa: F401  (pytest fixture import)
    OWNER,
    admin,
    canonicalize,
    network,
    seal,
)


class OtherNet(Net):
    """Second v1 run: same coordinator/params, distinct manifest so a distinct run_id."""

    def __init__(self, client: TestClient, master: Master) -> None:
        super().__init__(client, master)
        self.manifest = example_manifest().model_copy(update={"init_state_hash": "ab" * 32})
        self.run_id = self.manifest.run_id()

    def publish_state(self, w: int):  # type: ignore[no-untyped-def]
        hosted = self.c.app.state.store.for_run(self.run_id)  # type: ignore[attr-defined]
        sha = hosted.objects.put(f"theta-start-{w}-other".encode())
        return self.c.put(
            f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/state",
            json={"theta_start_sha256": sha},
            headers=bearer("admin-secret"),
        )


def round0(net: Net, *, start: bool = True) -> dict:
    if start:
        net.start()
    body = net.body(0)
    net.push(body["d_assign"])
    for m in net.miners:
        assert net.accept(m, 0).status_code == 200
    net.push(body["d_assign"] + 4)
    for m in net.miners:
        r = net.commit(m, 0)
        assert r.status_code == 200 and r.json()["status"] == "COMMITTED", r.text
        assert net.delta(m, 0).status_code == 200
    return body


def test_two_v1_runs_finalize_independently_and_reopen(
    tmp_path: Path, secrets_dir: Path, master: Master, clock: list[float]
) -> None:
    state = tmp_path / "state"
    with make_client(state, secrets_dir, master, clock) as c:
        a, b = Net(c, master), OtherNet(c, master)
        assert a.run_id != b.run_id
        a.start()
        b.start()  # was 409 "a run already exists in this challenge state"
        assert b.create().status_code == 409  # duplicate still refused
        assert {r["run_id"] for r in c.get("/v1/runs").json()["runs"]} == {a.run_id, b.run_id}
        body = round0(a, start=False)
        assert round0(b, start=False) == b.body(0)
        a.push(body["d_audit"])
        while (lease := c.post("/v1/worker/lease", headers=bearer(WORKER))).status_code == 200:
            job = lease.json()
            net = a if job["run_id"] == a.run_id else b
            assert net.verdict(job, "MATCH").status_code == 200
        a.push(body["d_upload"])
        included = sorted(m.ss58 for m in a.miners)
        for net in (a, b):
            assert net.aggregate(0).status_code == 200
        a.push(body["d_final"])
        # Both runs finalize their own round 0: one shared ledger would refuse the second.
        for net in (a, b):
            r = net.finalize(0, included)
            assert r.status_code == 200, r.text
            assert net.round(0)["state"] == "FINAL"
        # Cross-run: run A's coordinator-signed Finalize is refused on run B's route.
        cross = c.post(
            f"/v1/aggregator/runs/{b.run_id}/rounds/0/finalize",
            json=a.signed(
                a.coord,
                "Finalize",
                {
                    "w": 0,
                    "final_theta_hash_w1": "11" * 32,
                    "included": included,
                    "entitlements_root": "22" * 32,
                },
            ),
            headers=bearer("admin-secret"),
        )
        assert cross.status_code == 400, cross.text
        assert cross.json()["detail"] == "envelope run_id does not match this run"
        # Cross-run: run A's signed miner Commit is refused by run B's intake.
        cross = c.post(
            f"/v1/runs/{b.run_id}/commit",
            json=a.signed(a.miners[0], "Commit", a.commit_body(a.miners[0], 1)),
        )
        assert cross.status_code == 400, cross.text
        assert cross.json()["detail"] == "envelope run_id does not match this run"
        store = c.app.state.store  # type: ignore[attr-defined]
        child = store.for_run(b.run_id)
        assert child is not store and child.state_dir == state / "runs" / b.run_id
        with pytest.raises(ChallengeError) as error:
            child._run(child._db, a.run_id)  # sub-store cannot read the other run's rounds
        assert error.value.status == 404
        with pytest.raises(ChallengeError):
            store._run(store._db, b.run_id)
        only_b = child.objects.put(b"object of run b only")
        with pytest.raises(ObjectNotFound):
            store.objects.get(only_b)
        # Ledgers are separate: each run vested exactly its own round 0.
        assert len(store.ledger.state().entitlements) == len(included)
        assert len(child.ledger.state().entitlements) == len(included)
        final_at = store.ledger.state().entitlements[0].final_round
        epoch_at = QUICKNET_GENESIS + (final_at + 10) * 300 + 1
        paid = a.weights(final_at + 10, epoch_at).json()
        assert paid["full_share_mass"] == 2_000_000 and paid["metadata"]["runs"] == 2
        assert set(paid["weights"]) == set(included)
        assert a.weights(final_at + 10, epoch_at).json() == paid  # first answer is final
        c.app.state.store.close_v2_notifications()  # type: ignore[attr-defined]
    with make_client(state, secrets_dir, master, clock) as c:
        a, b = Net(c, master), OtherNet(c, master)
        assert {r["run_id"] for r in c.get("/v1/runs").json()["runs"]} == {a.run_id, b.run_id}
        assert a.round(0)["state"] == b.round(0)["state"] == "FINAL"
        assert a.weights(final_at + 10, epoch_at).json() == paid
        assert b.finalize(0, included).status_code == 409  # replay set survives restart


def test_v2_second_run_is_isolated_and_reopens(network):  # noqa: F811
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.protocol.messages_v2 import RunManifestV2

    body = network.manifest.body()
    body["training"]["beacon"]["genesis_time"] -= 3
    other = RunManifestV2.model_validate(body)
    response = network.client.post(
        "/v2/admin/runs",
        content=canonicalize(seal(OWNER, "RunManifestV2", other.run_id(), other, 10000)),
        headers=admin(),
    )
    assert response.status_code == 201, response.text
    first = network.join()
    assert first.status_code == 200, first.text
    other_url = "/v2/runs/" + other.run_id()
    assert network.client.get(other_url).status_code == 200
    # Run A's dual-signed join cannot be replayed into run B.
    request = network.client.post(other_url + "/join", content=first.request.content)
    assert request.status_code == 409, request.text
    assert request.json()["detail"] == "DUAL_SIGNATURE_RUN_OR_EXPIRY"
    hotkey = network.store._db.execute("SELECT hotkey FROM admissions_v2").fetchone()[0]
    status = network.client.get(other_url + "/admission/" + hotkey)
    assert status.status_code == 409 and status.json()["detail"] == "UNKNOWN_HOTKEY"
    assert network.client.get(network.url + "/admission/" + hotkey).status_code == 200
    child = network.store.for_run(other.run_id())
    assert child._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 0
    with pytest.raises(ChallengeError):
        child._run_v2(network.manifest.run_id())
    # A duplicate create is still refused and does not disturb the hosted run.
    again = network.client.post(
        "/v2/admin/runs",
        content=canonicalize(seal(OWNER, "RunManifestV2", other.run_id(), other, 10000)),
        headers=admin(),
    )
    assert again.status_code == 409
    store = network.store
    reopened = ChallengeStore(
        store.state_dir,
        store.params,
        store.coord,
        store.owner_hotkey,
        store.verify_beacon,
        store.objects,
    )
    try:
        assert {r["run_id"] for r in reopened.runs()} == {
            network.manifest.run_id(),
            other.run_id(),
        }
        assert reopened.for_run(other.run_id())._run_v2(other.run_id()) == other
        assert reopened._run_v2(network.manifest.run_id()) == network.manifest
    finally:
        reopened.close()
