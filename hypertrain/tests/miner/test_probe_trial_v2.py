"""`probe-v2` without --job: a joined miner fetches its own open admission trial over HTTP."""

from __future__ import annotations

import copy
import gzip
import importlib.util
import json
import sys
from pathlib import Path

import httpx
import pytest

from hypertrain.miner import cli
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.messages_v2 import AdmissionPolicyV2, JoinChallenge, WorkScreenV2

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
    return n.client.post(
        n.url + f"/join/{identity}/job", json=n.owner_request("JobRequest", identity, key)
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
            missed = fault.setdefault("missed", {})
            if missed.get(path, 0) < fault.get("miss", 0):
                missed[path] = missed.get(path, 0) + 1
                return httpx.Response(404, json={"detail": "object not found"})
            # The faults reach the trial inputs, not the admission policy read before them.
            is_policy = path.endswith("/" + n.manifest.network.admission_policy_hash)
            if name == "corrupt" and not is_policy:
                return httpx.Response(200, content=b"corrupt object")
            if name == "gzip" and not is_policy:
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
    # Both owner requests are POSTs whose body is the owner's signed envelope: no signature
    # travels in a header (the master proxy forwards none).
    for suffix, message in (("/challenge", "ChallengeRequest"), ("/job", "JobRequest")):
        asked = next(r for r in requests if r.url.path.endswith(suffix))
        assert asked.method == "POST", suffix
        assert not {"x-hotkey", "x-trial-signature"} & set(asked.headers), suffix
        envelope = envelope_v2.parse_envelope(asked.content)
        assert (envelope.type, envelope.signer, envelope.body) == (
            message,
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


@pytest.fixture
def wide_miner(monkeypatch, request):
    """`miner` over an admission policy whose trial window is 200 rounds (ADR 0017)."""
    monkeypatch.setitem(service.ADMISSION_POLICY_EXTRA, "challenge_rounds", 200)
    return request.getfixturevalue("miner")


def test_probe_v2_proves_a_trial_150_rounds_after_its_seed_under_a_200_round_window(
    wide_miner, capsys
):
    # Given: a trial asked on its seed round, and 150 rounds gone when the miner probes it,
    # which a 100-round window would have refused (see `lapse`)
    n, identity, config, _, _, _ = wide_miner
    asked = n.challenge(identity)
    assert asked.status_code == 200, asked.text
    seed = asked.json()["body"]["seed_beacon"]
    assert asked.json()["body"]["deadline_beacon"] == asked.json()["exp_drand"] == seed + 200
    n.push(seed + 150)
    # When
    assert cli.main(["probe-v2", "--config", str(config)]) == 0
    signed = json.loads(capsys.readouterr().out)
    # Then: the miner signed its pair to the policy's deadline, and the service accepts it
    assert {signed[k]["exp_drand"] for k in ("proof", "screen")} == {seed + 200}
    response = n.client.post(n.url + "/join/proof", json=signed)
    assert response.status_code == 200, response.text
    assert response.json()["admission_id"] == identity and response.json()["clean_count"] == 1


@pytest.mark.parametrize(
    "name,error",
    [
        ("corrupt", "trial input start_state sha256 mismatch"),
        ("gzip", "trial input object has a non-identity content encoding"),
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


@pytest.mark.parametrize(
    "case,error",
    [
        ("window", "trial challenge refused: join challenge window differs from the policy's 100"),
        ("policy", "trial challenge refused: join challenge names another admission policy"),
        ("swapped", "admission policy sha256 mismatch"),
    ],
    ids=["window", "policy", "swapped"],
)
def test_probe_v2_refuses_a_signed_trial_that_is_not_its_policys(miner, caplog, case, error):
    # Given: the coordinator's own signature over a trial that the run's 100-round policy did not
    # issue: a window stretched to 300 rounds, another policy named, or both together with that
    # 300-round policy served in place of the run's; the schema takes all three
    n, identity, config, folder, requests, fault = miner
    asked = n.challenge(identity)
    assert asked.status_code == 200, asked.text
    job = signed_job(n, HOT, identity)
    assert job.status_code == 200, job.text
    body = dict(asked.json()["body"])
    digest = n.manifest.network.admission_policy_hash
    run_policy = n.client.get(n.url + "/objects/" + digest)
    assert run_policy.status_code == 200, run_policy.text
    wide = AdmissionPolicyV2.model_validate({**run_policy.json(), "challenge_rounds": 300})
    edit = {
        "window": {"deadline_beacon": body["seed_beacon"] + 300},
        "policy": {"policy_hash": "cd" * 32},
        "swapped": {"deadline_beacon": body["seed_beacon"] + 300, "policy_hash": wide.digest()},
    }[case]
    challenge = JoinChallenge.model_validate({**body, **edit})
    envelope = seal(
        service.COORD, "JoinChallenge", n.manifest.run_id(), challenge, challenge.deadline_beacon
    )
    fault["serve"] = {
        f"/join/{identity}/challenge": envelope,
        f"/join/{identity}/job": {**job.json(), "challenge": challenge.body()},
    }
    if case == "swapped":
        fault["serve"]["/objects/" + digest] = wide.body()
    # When / Then: refused before any input is fetched, probed or uploaded
    assert cli.main(["probe-v2", "--config", str(config)]) == 2
    assert error in caplog.text
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


def s100_driver(monkeypatch):
    """scripts/ops/s100_test/cpu_miners.py, loaded in its CPU mode."""
    monkeypatch.delenv("HT_MINER_DEVICE", raising=False)
    spec = importlib.util.spec_from_file_location(
        "cpu_miners_probe_contract",
        Path(__file__).parents[2] / "scripts/ops/s100_test/cpu_miners.py",
    )
    assert spec is not None and spec.loader is not None
    driver = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(driver)
    return driver


def lapse(n, identity):
    """The trial window of `identity` passes unproven: its challenge is refused for good."""
    assert n.challenge(identity).status_code == 200
    n.push(n.now + 150)
    refused = n.challenge(identity)
    assert (refused.status_code, refused.json()) == (409, {"detail": "CHALLENGE_EXPIRED"})


def test_probe_v2_resumes_the_same_identity_once_the_operator_reseeds_it(
    miner, monkeypatch, capsys
):
    import hypertrain.miner.core as core

    # Given: RELAY-03, an identity whose trial window lapsed while its driver was down
    n, identity, config, folder, requests, _ = miner
    lapse(n, identity)
    reseeds: list[dict] = []

    def operator(seconds: float) -> None:
        """The wait of the miner is the operator's turn: reseed once, then the round passes."""
        if not reseeds:
            done = n.client.post(n.url + f"/admin/join/{identity}/reseed", headers=service.admin())
            assert done.status_code == 200, done.text
            reseeds.append(done.json())
            n.push(n.now + 1)

    monkeypatch.setattr(core, "RESUME_POLL_SECONDS", 0.0)
    monkeypatch.setattr(core.time, "sleep", operator)
    # When
    assert cli.main(["probe-v2", "--config", str(config)]) == 0
    signed = json.loads(capsys.readouterr().out)
    # Then: the same identity proves its trial: accepted, one more clean trial, no second identity
    response = n.client.post(n.url + "/join/proof", json=signed)
    assert response.status_code == 200, response.text
    assert response.json()["admission_id"] == identity and response.json()["clean_count"] == 1
    assert n.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 1
    assert not any(r.method == "POST" and r.url.path.endswith("/join") for r in requests)
    # And: the journal says which challenge the reseed replaced
    lines = [json.loads(x) for x in (folder / "journal.jsonl").read_text().splitlines()]
    [resume] = [x for x in lines if x["kind"] == "resume"]
    assert resume["admission_id"] == identity and resume["trial"] == 1
    assert resume["old_challenge"] and resume["new_challenge"] != resume["old_challenge"]
    assert reseeds and reseeds[0]["admission_id"] == identity


def test_probe_v2_without_an_operator_ends_with_status_3_and_the_step_to_take(
    miner, monkeypatch, caplog
):
    import hypertrain.miner.core as core

    n, identity, config, folder, requests, _ = miner
    lapse(n, identity)
    monkeypatch.setattr(core, "RESUME_POLLS", 2)
    monkeypatch.setattr(core.time, "sleep", lambda _: None)
    # When: nobody reseeds
    assert cli.main(["probe-v2", "--config", str(config)]) == 3
    # Then: three asks, a last line that names the operator step, nothing probed or uploaded,
    # and the line is not one the s100 driver would take for a transient
    asks = [r for r in requests if r.url.path.endswith(f"/join/{identity}/challenge")]
    assert len(asks) == 3
    last = caplog.records[-1].getMessage()
    assert last.startswith("ChallengeExpired: CHALLENGE_EXPIRED: admission " + identity)
    assert f"/admin/join/{identity}/reseed" in last
    assert not s100_driver(monkeypatch).transient("", f"ERROR {last}")
    assert not any(r.method == "PUT" for r in requests)
    assert list(folder.iterdir()) == []


def test_probe_v2_completes_when_each_trial_input_misses_once(miner, monkeypatch, capsys):
    import hypertrain.miner.core as core

    # Given: RELAY-02, a read miss on a trial input that is intact on the server
    n, _, config, folder, _, fault = miner
    monkeypatch.setattr(core, "RETRY_DELAYS", (0,))
    fault["miss"] = 1
    # When
    assert cli.main(["probe-v2", "--config", str(config)]) == 0
    signed = json.loads(capsys.readouterr().out)
    # Then: every miss was retried once and journaled, and the retried inputs were verified
    # like any other: the service accepts the proof it replays.
    retries = [json.loads(x) for x in (folder / "journal.jsonl").read_text().splitlines()]
    assert len(retries) == len(fault["missed"]) >= 1
    assert {(r["kind"], r["status"], r["attempt"]) for r in retries} == {("retry", 404, 1)}
    response = n.client.post(n.url + "/join/proof", json=signed)
    assert response.status_code == 200, response.text
    assert response.json()["clean_count"] == 1


def test_probe_v2_ends_on_the_line_the_s100_driver_retries_when_an_input_stays_missing(
    miner, monkeypatch, caplog
):
    import hypertrain.miner.core as core

    # Given: a trial input that never arrives
    _, _, config, folder, requests, fault = miner
    monkeypatch.setattr(core, "RETRY_DELAYS", (0, 0))
    fault["miss"] = 99
    # When
    assert cli.main(["probe-v2", "--config", str(config)]) == 2
    # Then: it fails after its own retries, with nothing probed, uploaded or left in the folder
    last = caplog.records[-1]
    assert last.getMessage() == "ChallengeHTTPError: 404: trial input object unavailable"
    retries = [json.loads(x) for x in (folder / "journal.jsonl").read_text().splitlines()]
    assert [r["attempt"] for r in retries] == [1, 2]
    assert not any(r.method == "PUT" for r in requests)
    assert {p.name for p in folder.iterdir()} <= {"journal.jsonl", "journal.lock"}
    # And: the driver takes that final stderr line, as the CLI wrote it, for a transient
    assert s100_driver(monkeypatch).transient("", f"{last.levelname} {last.getMessage()}")


# ADR 0011 part 3: the route refuses a work screen that declares another environment than the
# manifest's pin before the reference run is paid, convicts nothing, and accepts the genuine pair
# afterwards.
EDITS = {
    "mkl": (lambda env: env["env"].__setitem__("MKL_CBWR", "COMPATIBLE"), "MKL_CBWR"),
    "isa": (lambda env: env.__setitem__("cpu_capability", "AVX512"), "cpu_capability"),
    "sm": (lambda env: env.__setitem__("gpu", "sm_103"), "gpu"),
    "attention": (
        lambda env: env["env"].__setitem__("HT_ATTN_BACKEND", "sdpa-flash"),
        "HT_ATTN_BACKEND",
    ),
    "fp8": (lambda env: env["env"].__setitem__("HT_FP8", "1"), "HT_FP8"),
}


@pytest.mark.parametrize("fault", sorted(EDITS))
def test_the_route_refuses_a_diverging_screen_before_the_reference_is_paid(
    miner, capsys, fault: str
) -> None:
    n, _, config, _, _, _ = miner
    assert cli.main(["probe-v2", "--config", str(config)]) == 0
    signed = json.loads(capsys.readouterr().out)
    body = copy.deepcopy(signed["screen"]["body"])
    edit, names = EDITS[fault]
    edit(body["env"])
    screen = WorkScreenV2.model_validate(body)
    resigned = seal(HOT, "WorkScreenV2", n.manifest.run_id(), screen, signed["screen"]["exp_drand"])
    refused = n.client.post(
        n.url + "/join/proof", json={"proof": signed["proof"], "screen": resigned}
    )
    assert refused.status_code == 409
    assert refused.json() == {"detail": f"ENV_PIN_DIVERGES: {names}"}
    # The reference run (which commits a reference) was not started, nothing was convicted ...
    row = n.store._db.execute(
        "SELECT reference_commitment,state,strikes,pending_dispute FROM admissions_v2 "
        "WHERE hotkey=?",
        (HOT.ss58,),
    ).fetchone()
    assert tuple(row) == (None, "APPLIED", 0, 0)
    # ... and the genuine pair is accepted afterwards.
    accepted = n.client.post(n.url + "/join/proof", json=signed)
    assert accepted.status_code == 200, accepted.text
    assert accepted.json()["clean_count"] == 1
