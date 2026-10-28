"""The environment pin is a required part of a signed run manifest, over HTTP (ADR 0011, part 3).

A body without ``reference_spec.env.pinned`` is signed by the real owner key here, so a refusal is
the manifest model's and not the envelope's. A stored run whose manifest the model now refuses is
retired (409) without disturbing the other runs of the same state directory.
"""

from __future__ import annotations

import json
from typing import Any

import httpx
import pytest
from fastapi.testclient import TestClient

from hypertrain.protocol import envelope, envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair

from .conftest import ADMIN, OWNER, WORKER, Master, Net, bearer
from .test_multirun import OtherNet, round0
from .test_service_network_v2 import OWNER as OWNER_V2
from .test_service_network_v2 import Network, admin
from .test_service_network_v2 import network as network

Body = dict[str, Any]

# What the manifest model says for each way of not naming a whole pin, and where.
REFUSED = {
    "absent": ("pinned", "Field required"),
    "null": ("pinned", "Input should be a valid dictionary"),
    "cpu_only": ("pinned.gpu", "Field required"),
    "gpu_only": ("pinned.cpu", "Field required"),
}


def damage_pin(env: Body, variant: str) -> None:
    match variant:
        case "absent":
            del env["pinned"]
        case "null":
            env["pinned"] = None
        case "cpu_only":
            del env["pinned"]["gpu"]
        case "gpu_only":
            del env["pinned"]["cpu"]
        case _:
            raise AssertionError(variant)


def sign_v1(body: Body, owner: Keypair = OWNER, exp: int = 2**40) -> Body:
    """The owner's ht/1 envelope over ``body``, signed without ``seal``'s own model check."""
    run_id = sha256_hex(canonicalize(body, allow_float=False))
    sig = owner.sign(envelope.signing_message("RunManifest", body, exp, run_id))
    return {
        "v": envelope.VERSION,
        "type": "RunManifest",
        "run_id": run_id,
        "body": body,
        "signer": owner.ss58,
        "exp_drand": exp,
        "sig": sig.hex(),
    }


def sign_v2(body: Body, owner: Keypair = OWNER_V2, exp: int = 10000) -> Body:
    """The owner's ht/2 envelope over ``body``, signed without ``seal``'s own model check."""
    run_id = sha256_hex(canonicalize(body, allow_float=False))
    sig = owner.sign(envelope_v2.signing_message("RunManifestV2", body, exp, run_id))
    return {
        "v": envelope_v2.VERSION,
        "type": "RunManifestV2",
        "run_id": run_id,
        "body": body,
        "signer": owner.ss58,
        "exp_drand": exp,
        "sig": sig.hex(),
    }


def post_v1(client: TestClient, body: Body) -> httpx.Response:
    return client.post("/v1/admin/runs", json=sign_v1(body), headers=bearer(ADMIN))


def post_v2(client: TestClient, body: Body) -> httpx.Response:
    return client.post("/v2/admin/runs", content=canonicalize(sign_v2(body)), headers=admin())


def second_v1(net: Net) -> Net:
    """A distinct run on the same coordinator and ledger parameters."""
    other = Net(net.c, net.master)
    other.manifest = net.manifest.model_copy(update={"init_state_hash": "ab" * 32})
    other.run_id = other.manifest.run_id()
    return other


def second_v2(network: Network) -> Body:
    body = network.manifest.body()
    body["training"]["beacon"]["genesis_time"] -= 3
    return body


@pytest.mark.parametrize("variant", REFUSED)
def test_v1_manifest_without_a_complete_pin_is_unprocessable(
    client: TestClient, master: Master, variant: str
):
    # Given a manifest the owner signed, whose env pin is absent, null or half a pin
    body = Net(client, master).manifest.body()
    damage_pin(body["reference_spec"]["env"], variant)
    # When
    response = post_v1(client, body)
    # Then the model refuses it where it is wrong, and no run is stored
    where, message = REFUSED[variant]
    assert response.status_code == 422, response.text
    detail = response.json()["detail"]
    assert detail.startswith(f"reference_spec.env.{where}: "), detail
    assert message in detail, detail
    assert client.get("/v1/runs").json() == {"runs": []}


def test_v1_manifest_with_a_cpu_class_and_gpu_none_is_accepted(client: TestClient, master: Master):
    # Given the same signing path, with "gpu": "none" spelled out beside cpu "avx2"
    body = Net(client, master).manifest.body()
    pinned = body["reference_spec"]["env"]["pinned"]
    assert (pinned["cpu"], pinned["gpu"]) == ("avx2", "none")
    # When
    response = post_v1(client, body)
    # Then
    assert response.status_code == 201, response.text
    assert client.get("/v1/runs").json() == {
        "runs": [{"run_id": response.json()["run_id"], "status": "created"}]
    }


def retire_v1(net: Net) -> None:
    """Leave the stored run as a manifest written before the pin was required."""
    legacy = net.manifest.body()
    del legacy["reference_spec"]["env"]["pinned"]
    store = net.c.app.state.store  # type: ignore[attr-defined]
    store._db.execute(
        "UPDATE runs SET manifest=?, envelope=? WHERE run_id=?",
        (json.dumps(legacy), json.dumps(sign_v1(legacy)), net.run_id),
    )


def test_v1_stored_run_without_a_pin_is_retired_and_the_other_run_keeps_working(
    client: TestClient, master: Master
):
    # Given a first run stored before the pin was required, and a second run created after
    old = Net(client, master)
    old.start()
    retire_v1(old)
    new = second_v1(old)
    assert new.create().status_code == 201
    # When both are read
    retired = client.get(f"/v1/runs/{old.run_id}")
    working = client.get(f"/v1/runs/{new.run_id}")
    # Then the old one is retired by name (not a 500) and the new one is served
    assert retired.status_code == 409, retired.text
    assert retired.json()["detail"] == (
        "RUN_MANIFEST_RETIRED: the stored manifest is refused at reference_spec.env.pinned"
    )
    assert working.status_code == 200, working.text
    assert working.json()["status"] == "created"


def test_v1_retired_run_with_a_pending_audit_draw_does_not_stop_the_beacon(
    client: TestClient, master: Master
):
    # Given a retired run whose round 0 awaits its audit draw, and a second run created after
    old = Net(client, master)
    old.start()
    d_audit = old.body(0)["d_audit"]
    retire_v1(old)
    new = second_v1(old)
    assert new.create().status_code == 201
    # When the beacon reaches the round's audit draw
    response = old.push(d_audit)
    # Then the push is accepted: the clock of the other run goes on
    assert response.status_code == 200, response.text
    assert client.get("/v1/beacon/latest").json()["round"] == d_audit
    assert client.get(f"/v1/runs/{new.run_id}").status_code == 200


def test_v1_retired_run_with_a_queued_audit_job_does_not_starve_the_other_run(
    client: TestClient, master: Master
):
    # Given two runs with a committed round 0 whose audit jobs are queued, the first then retired
    old, new = Net(client, master), OtherNet(client, master)
    old.start()
    new.start()
    body = round0(old, start=False)
    round0(new, start=False)
    old.push(body["d_audit"])
    retire_v1(old)
    # When an auditor asks for work
    lease = client.post("/v1/worker/lease", headers=bearer(WORKER))
    # Then it is given the job of the run that still works
    assert lease.status_code == 200, lease.text
    assert lease.json()["run_id"] == new.run_id


@pytest.mark.parametrize("variant", REFUSED)
def test_v2_manifest_without_a_complete_pin_is_unprocessable(network: Network, variant: str):
    # Given a RunManifestV2 the owner signed, whose env pin is absent, null or half a pin
    body = second_v2(network)
    damage_pin(body["training"]["reference_spec"]["env"], variant)
    # When
    response = post_v2(network.client, body)
    # Then the model refuses it where it is wrong, and only the fixture's run is stored
    where, message = REFUSED[variant]
    assert response.status_code == 422, response.text
    detail = response.json()["detail"]
    assert detail.startswith(f"training.reference_spec.env.{where}: "), detail
    assert message in detail, detail
    assert [r["run_id"] for r in network.store.runs()] == [network.manifest.run_id()]


def test_v2_manifest_with_a_cpu_class_and_gpu_none_is_accepted(network: Network):
    # Given the same signing path, with "gpu": "none" spelled out beside cpu "avx2"
    body = second_v2(network)
    pinned = body["training"]["reference_spec"]["env"]["pinned"]
    assert (pinned["cpu"], pinned["gpu"]) == ("avx2", "none")
    # When
    response = post_v2(network.client, body)
    # Then
    assert response.status_code == 201, response.text
    assert network.client.get("/v2/runs/" + response.json()["run_id"]).status_code == 200


def test_v2_stored_run_without_a_pin_is_retired_and_the_other_run_keeps_working(
    network: Network,
):
    # Given the first run stored before the pin was required, and a second run created after
    legacy = network.manifest.body()
    del legacy["training"]["reference_spec"]["env"]["pinned"]
    network.store._db.execute(
        "UPDATE runs SET manifest=?, envelope=? WHERE run_id=?",
        (json.dumps(legacy), json.dumps(sign_v2(legacy)), network.manifest.run_id()),
    )
    created = post_v2(network.client, second_v2(network))
    assert created.status_code == 201, created.text
    # When both are read
    retired = network.client.get(network.url)
    working = network.client.get("/v2/runs/" + created.json()["run_id"])
    # Then the old one is retired by name (not a 500) and the new one is served
    assert retired.status_code == 409, retired.text
    assert retired.json()["detail"] == (
        "RUN_MANIFEST_RETIRED: the stored manifest is refused at training.reference_spec.env.pinned"
    )
    assert working.status_code == 200, working.text
