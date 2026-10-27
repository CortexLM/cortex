"""`probe-v2` without --job: a joined miner fetches its own open admission trial over HTTP."""

from __future__ import annotations

import gzip
import importlib.util
import json
import sys
from pathlib import Path

import httpx
import pytest

from hypertrain.miner import cli
from hypertrain.protocol import envelope_v2

_spec = importlib.util.spec_from_file_location(
    "probe_trial_service_fixture",
    Path(__file__).parents[1] / "challenge/test_service_network_v2.py",
)
assert _spec is not None and _spec.loader is not None
service = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = service
_spec.loader.exec_module(service)
network = service.network
HOT = service.HOT[0]


def signed_job(n, key, identity):
    message = f"hypertrain/trial-job/2|{n.manifest.run_id()}|{identity}".encode()
    return n.client.get(
        n.url + f"/join/{identity}/job",
        headers={"x-hotkey": key.ss58, "x-trial-signature": key.sign(message).hex()},
    )


def tamper(body, fault):
    """Apply one named fault to the /job response the miner receives."""
    match fault:
        case "nonce":
            body["nonce"] = "cd" * 32
        case "challenge":  # consistent with itself, but not the signed challenge
            body["nonce"] = body["challenge"]["nonce"] = "cd" * 32
        case "job.json" | "ef_in":
            body["job"]["object_paths"]["samples"] = fault


@pytest.fixture
def miner(network, tmp_path, monkeypatch):
    """HOT[0] joined, its trial seed ready; miner.toml whose HTTP goes to the in-process app."""
    n = network
    response = n.join()
    assert response.status_code == 200, response.text
    identity = response.json()["admission_id"]
    n.push(n.now + 1)
    key = tmp_path / "hot.key"
    key.write_bytes(bytes([80]) * 32)
    key.chmod(0o600)
    work = tmp_path / "work"
    config = tmp_path / "miner.toml"
    config.write_text(
        "\n".join(
            f"{k} = {json.dumps(str(v))}"
            for k, v in {
                "api": "https://coordinator.test",
                "keyfile": key,
                "workdir": work,
                "state_source": "cpu",
                "image_digest": n.manifest.training.reference_spec.image_digest,
                "run_id": n.manifest.run_id(),
                "owner_hotkey": service.OWNER.ss58,
                "device": "cpu",
            }.items()
        )
    )
    requests = []
    fault = {}

    def handler(request):
        requests.append(request)
        path = request.url.path
        for suffix, body in fault.get("serve", {}).items():
            if path.endswith(suffix):
                return httpx.Response(200, json=body)
        response = n.client.request(
            request.method, path, headers=request.headers, content=request.content
        )
        name = fault.get("name")
        if name and path.endswith("/job") and response.status_code == 200:
            body = response.json()
            tamper(body, name)
            return httpx.Response(200, json=body)
        if name == "signature" and path.endswith("/challenge"):
            return httpx.Response(200, json={**response.json(), "sig": "00" * 64})
        if "/objects/" in path and request.method == "GET":
            if name == "corrupt":
                return httpx.Response(200, content=b"corrupt object")
            if name == "gzip":
                encoded = gzip.compress(response.content)
                return httpx.Response(200, content=encoded, headers={"Content-Encoding": "gzip"})
        return httpx.Response(response.status_code, content=response.content)

    client_type = httpx.Client
    monkeypatch.setattr(
        cli.httpx,
        "Client",
        lambda **kwargs: client_type(transport=httpx.MockTransport(handler), **kwargs),
    )
    return n, identity, config, work / n.manifest.run_id() / HOT.ss58, requests, fault


def test_probe_v2_without_job_fetches_own_trial_and_completes(miner, capsys):
    n, identity, config, folder, requests, _ = miner
    assert cli.main(["probe-v2", "--config", str(config)]) == 0
    signed = json.loads(capsys.readouterr().out)
    paths = [r.url.path for r in requests]
    assert n.url + f"/join/{identity}/job" in paths
    assert not any("/admin/" in p for p in paths)
    assert not any("authorization" in r.headers for r in requests)
    # The challenge request is a POST whose body is the owner's signed envelope: no signature
    # travels in a header (the master proxy forwards none).
    asked = next(r for r in requests if r.url.path.endswith("/challenge"))
    assert asked.method == "POST" and "x-trial-signature" not in asked.headers
    envelope = envelope_v2.parse_envelope(asked.content)
    assert (envelope.type, envelope.signer, envelope.body) == (
        "ChallengeRequest",
        HOT.ss58,
        {"admission_id": identity},
    )
    assert envelope_v2.verify_envelope(asked.content)
    assert list(folder.iterdir()) == []
    # The service accepts the probe's proof (it replays the reference itself) ...
    response = n.client.post(n.url + "/join/proof", json=signed)
    assert response.status_code == 200, response.text
    assert response.json()["clean_count"] == 1
    # ... and the finalized trial is no longer served.
    response = signed_job(n, HOT, identity)
    assert response.status_code == 409, response.text
    assert response.json() == {"detail": "REFERENCE_TRIAL_CLOSED"}


@pytest.mark.parametrize(
    "name,error",
    [
        ("corrupt", "sha256 mismatch"),
        ("gzip", "non-identity content encoding"),
        ("nonce", "binding differs"),
        ("challenge", "signed challenge differs"),
        ("signature", "signed challenge differs"),
        ("job.json", "unsafe trial object path"),
        ("ef_in", "unsafe trial object path"),
    ],
)
def test_probe_v2_refuses_unverified_trial_without_probing(miner, caplog, name, error):
    _, _, config, folder, requests, fault = miner
    fault["name"] = name
    assert cli.main(["probe-v2", "--config", str(config)]) == 2
    assert error in caplog.text
    assert not any(r.method == "PUT" for r in requests)
    assert list(folder.iterdir()) == []


def test_probe_v2_refuses_another_admissions_genuine_trial(miner, caplog):
    n, identity, config, folder, requests, fault = miner
    other = n.join(1).json()["admission_id"]
    n.push(n.now + 1)
    challenge = n.challenge(other)
    assert challenge.status_code == 200, challenge.text
    job = signed_job(n, service.HOT[1], other)
    assert job.status_code == 200, job.text
    fault["serve"] = {
        f"/join/{identity}/challenge": challenge.json(),
        f"/join/{identity}/job": job.json(),
    }
    assert cli.main(["probe-v2", "--config", str(config)]) == 2
    assert "binding differs" in caplog.text
    assert not any(r.method == "PUT" for r in requests)
    assert list(folder.iterdir()) == []


@pytest.mark.parametrize("given", ["--job", "--challenge"])
def test_probe_v2_refuses_job_or_challenge_alone(miner, caplog, tmp_path, given):
    _, _, config, _, requests, _ = miner
    stray = tmp_path / "stray.json"
    stray.write_text("{}")
    assert cli.main(["probe-v2", "--config", str(config), given, str(stray)]) == 2
    assert "takes --job and --challenge, or neither" in caplog.text
    assert not any(r.url.path.endswith(("/challenge", "/job")) for r in requests)
