"""Admin trial export and the copyable probe-v2 input folder."""

from __future__ import annotations

import importlib.util
import json
import os
import sys
import time
from pathlib import Path

import httpx
import pytest

from hypertrain.challenge.app import OWNER_REQUEST_MAX_ROUNDS
from hypertrain.data.trial_assignment import trial_samples
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import IslandJobV1, JoinChallenge

_spec = importlib.util.spec_from_file_location(
    "trial_export_service_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert _spec is not None and _spec.loader is not None
service = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = service
_spec.loader.exec_module(service)
network = service.network


@pytest.fixture
def trial(network):
    response = network.join()
    assert response.status_code == 200, response.text
    identity = response.json()["admission_id"]
    network.push(network.now + 1)
    response = network.challenge(identity)
    assert response.status_code == 200, response.text
    return network, identity, response.json()


def export(n, identity):
    return n.client.post(n.url + f"/admin/trial/{identity}/export", headers=service.admin())


def job(n, identity, key=None, *, expires=None, run_id=None):
    """Miner-facing job read: a POST whose body is the owner's (or `key`'s) signed JobRequest."""
    request = n.owner_request("JobRequest", identity, key, expires=expires)
    return n.client.post(
        n.url + f"/join/{identity}/job"
        if run_id is None
        else f"/v2/runs/{run_id}/join/{identity}/job",
        json=request,
    )


def forgeries(n, identity, message):
    """Bodies that are not the owner's unexpired `message` for `identity`, whatever else they
    carry. The master proxy forwards no signature header: the proof is only in the body."""
    owner, rival = service.HOT[0], service.HOT[1]
    other = "JobRequest" if message == "ChallengeRequest" else "ChallengeRequest"
    now = n.store._now(n.store._db)
    mine = n.owner_request(message, identity)
    return {
        "no body": b"",
        "a body that is not json": b"not json",
        "a rival signing as itself": n.owner_request(message, identity, rival),
        "the owner's request for another admission": n.signed(
            owner, message, {"admission_id": "ee" * 32}, now + 5
        ),
        "the owner's request under another run": service.seal(
            owner, message, "ff" * 32, {"admission_id": identity}, now + 5
        ),
        "the owner's request for the other route": n.owner_request(other, identity),
        "another message of the owner": n.signed(
            owner, "Receipt", {"w": 0, "commit_hash": "11" * 32, "received_round": 1}, now + 5
        ),
        "an expired request of the owner": n.owner_request(message, identity, expires=now - 1),
        "a request with the owner's signature cut": {**mine, "sig": "00" * 64},
        "a request with a rival as signer": {**mine, "signer": rival.ss58},
    }


def send(n, url, body):
    if isinstance(body, bytes):
        return n.client.post(url, content=body)
    return n.client.post(url, json=body)


def assert_refused(response, name=""):
    assert (response.status_code, response.json()) == (
        403,
        {"detail": "hotkey is not current accepted identity"},
    ), name


def test_challenge_route_refuses_everyone_but_the_owner(network):
    # Given: HOT[0] joined and its seed round arrived; HOT[1] is a registered rival. Opening a
    # trial takes one of the run's trial places, so only the owner may ask for it.
    identity = network.join().json()["admission_id"]
    network.push(network.now + 1)
    url = network.url + f"/join/{identity}/challenge"
    # When / Then: every forgery is refused alike, and nothing was issued
    for name, body in forgeries(network, identity, "ChallengeRequest").items():
        assert_refused(send(network, url, body), name)
    # the earlier contract, a GET with signature headers, is gone
    assert network.client.get(url).status_code == 405
    db = network.store._db
    assert db.execute("SELECT challenge FROM admissions_v2").fetchone()[0] is None
    assert db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] == 0
    # The owner's signed request issues the trial, and asking again returns the same envelope.
    first = network.challenge(identity)
    assert first.status_code == 200 and first.json()["type"] == "JoinChallenge"
    assert network.challenge(identity).json() == first.json()


def test_job_route_refuses_everyone_but_the_owner(trial):
    n, identity, _ = trial
    url = n.url + f"/join/{identity}/job"
    for name, body in forgeries(n, identity, "JobRequest").items():
        assert_refused(send(n, url, body), name)
    # the earlier contract, a GET with signature headers, is gone
    assert n.client.get(url).status_code == 405
    assert not (n.store.state_dir / "trials-v2").exists()
    assert job(n, identity).status_code == 200


def owner_post(n, route, identity, request):
    return n.client.post(n.url + f"/join/{identity}/{route}", json=request)


@pytest.mark.parametrize(
    "route,message", [("challenge", "ChallengeRequest"), ("job", "JobRequest")]
)
def test_a_captured_owner_request_is_good_only_until_its_expiry(trial, route, message):
    # Given: the owner's request, as a third party may capture it on the wire
    n, identity, _ = trial
    now = n.store._now(n.store._db)
    captured = n.owner_request(message, identity, expires=now + 5)
    assert owner_post(n, route, identity, captured).status_code == 200
    # When: its expiry has passed
    n.push(n.now + 6)
    # Then: replaying it asks for nothing
    assert_refused(owner_post(n, route, identity, captured))


@pytest.mark.parametrize(
    "route,message", [("challenge", "ChallengeRequest"), ("job", "JobRequest")]
)
def test_an_owner_request_may_expire_only_so_far_ahead(trial, route, message):
    # The envelope's expiry is the owner's choice, so the service bounds it: a capture cannot be
    # signed to replay for long.
    n, identity, _ = trial
    now = n.store._now(n.store._db)
    farthest = n.owner_request(message, identity, expires=now + OWNER_REQUEST_MAX_ROUNDS)
    assert owner_post(n, route, identity, farthest).status_code == 200
    beyond = n.owner_request(message, identity, expires=now + OWNER_REQUEST_MAX_ROUNDS + 1)
    assert_refused(owner_post(n, route, identity, beyond))


def test_export_matches_prepare_reference_inputs(trial):
    n, identity, envelope = trial
    response = export(n, identity)
    assert response.status_code == 200, response.text
    body = response.json()
    job = IslandJobV1.model_validate(body["job"])
    job.manifest.validate_run_id(job.run_id)
    challenge = JoinChallenge.model_validate(envelope["body"])
    assert job.sample_ids == list(
        trial_samples(
            n.manifest, identity, challenge.nonce, n.store._beacon_v2(challenge.seed_beacon)
        )
    )
    assert body["challenge"] == challenge.body()
    # The operator reads the coordinator-signed envelope from the export, not from the route.
    assert body["challenge_envelope"] == envelope
    assert envelope_v2.verify_envelope(body["challenge_envelope"])
    assert body["nonce"] == challenge.nonce
    row = n.store._db.execute(
        "SELECT trial_epoch FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()
    assert job.w == row["trial_epoch"]
    assert (
        job.deadline
        == n.manifest.training.beacon.genesis_time + (challenge.deadline_beacon - 1) * 3
    )
    directory = n.store.state_dir / "trials-v2" / challenge.nonce
    saved = IslandJobV1.model_validate_json((directory / "job.json").read_bytes())
    saved.manifest.validate_run_id(saved.run_id)
    assert saved == job
    assert (directory / "job.json").read_text() == job.model_dump_json()
    assert set(body["objects"]) == {"start_state", "ef_in", "v0", "samples", "sample_proofs"}
    for name, digest in body["objects"].items():
        raw = (directory / name).read_bytes()
        assert sha256_hex(raw) == digest
        downloaded = n.client.get(n.url + "/objects/" + digest)
        assert downloaded.status_code == 200, downloaded.text
        assert downloaded.content == raw
    for name in ("start_state", "ef_in", "v0"):
        assert body["objects"][name] == getattr(job, name + "_sha256")


def test_export_is_idempotent_without_challenge_lifecycle_calls(trial, monkeypatch):
    n, identity, envelope = trial
    _, admission, _ = n.store._services(n.manifest.run_id())

    def forbidden(*args, **kwargs):
        pytest.fail("export must read the stored challenge, not issue one")

    monkeypatch.setattr(admission, "challenge", forbidden)
    before = [tuple(row) for row in n.store._db.execute("SELECT * FROM admission_trials")]
    first = export(n, identity)
    actual_write = Path.write_bytes

    def write(path, raw):
        if path.name == "job.json":
            pytest.fail("retry must not rewrite immutable job.json")
        return actual_write(path, raw)

    monkeypatch.setattr(Path, "write_bytes", write)
    second = export(n, identity)
    assert first.status_code == second.status_code == 200
    assert first.json() == second.json()
    assert first.json()["challenge"] == envelope["body"]
    assert sorted(p.name for p in (n.store.state_dir / "trials-v2").iterdir()) == [
        envelope["body"]["nonce"]
    ]
    assert [tuple(row) for row in n.store._db.execute("SELECT * FROM admission_trials")] == before


@pytest.mark.parametrize("known", [True, False])
def test_export_before_challenge(network, known):
    identity = network.join().json()["admission_id"] if known else "ab" * 32
    response = export(network, identity)
    assert response.status_code == 409, response.text
    assert response.json() == {"detail": "NO_CHALLENGE"}
    assert not (network.store.state_dir / "trials-v2").exists()


@pytest.mark.parametrize("outcome", ["EXPIRED", None])
def test_export_requires_open_trial(trial, outcome):
    n, identity, _ = trial
    with n.store._tx() as db:
        if outcome is None:
            db.execute("DELETE FROM admission_trial_results")
        else:
            db.execute("UPDATE admission_trial_results SET outcome=?", (outcome,))
    response = export(n, identity)
    assert response.status_code == 409, response.text
    assert response.json() == {"detail": "REFERENCE_TRIAL_CLOSED"}
    assert not (n.store.state_dir / "trials-v2").exists()


def test_export_requires_admin(trial):
    n, identity, _ = trial
    response = n.client.post(n.url + f"/admin/trial/{identity}/export")
    assert response.status_code == 401, response.text
    assert not (n.store.state_dir / "trials-v2").exists()


def test_owner_reads_the_admin_export_without_admin_token(trial):
    n, identity, envelope = trial
    response = job(n, identity)
    assert response.status_code == 200, response.text
    assert response.json() == export(n, identity).json()
    assert response.json()["challenge"] == envelope["body"]


@pytest.mark.parametrize(
    "unknown,status,detail", [("run", 404, "unknown run"), ("admission", 409, "UNKNOWN_ADMISSION")]
)
def test_job_route_unknown_ids_answer_like_challenge(trial, unknown, status, detail):
    n, identity, _ = trial
    run_id = "cd" * 32 if unknown == "run" else n.manifest.run_id()
    identity = "ab" * 32 if unknown == "admission" else identity
    challenge = n.client.post(f"/v2/runs/{run_id}/join/{identity}/challenge", content=b"{}")
    response = job(n, identity, run_id=run_id)
    assert (response.status_code, response.json()) == (status, {"detail": detail})
    assert (challenge.status_code, challenge.json()) == (status, {"detail": detail})


def test_export_refuses_changed_job(trial):
    n, identity, envelope = trial
    directory = n.store.state_dir / "trials-v2" / envelope["body"]["nonce"]
    directory.mkdir(parents=True)
    path = directory / "job.json"
    path.write_bytes(b"different job")
    response = export(n, identity)
    assert response.status_code == 409, response.text
    assert "immutable" in response.json()["detail"]
    assert path.read_bytes() == b"different job"


def test_export_unavailable_seed(trial):
    n, identity, envelope = trial
    with n.store._tx() as db:
        db.execute("DELETE FROM beacon WHERE round=?", (envelope["body"]["seed_beacon"],))
    response = export(n, identity)
    assert response.status_code == 503, response.text
    assert "beacon" in response.json()["detail"]
    assert not (n.store.state_dir / "trials-v2").exists()


@pytest.fixture
def cli(trial, tmp_path, monkeypatch):
    from hypertrain.challenge import trial_export

    n, identity, envelope = trial
    requests = []
    fault = {}

    def handler(request):
        requests.append(request)
        if "/objects/" in request.url.path:
            assert "authorization" not in request.headers
            if fault.get("corrupt") == sum("/objects/" in r.url.path for r in requests):
                return httpx.Response(200, content=b"corrupt object")
        if fault.get("status") and request.method == "POST":
            return httpx.Response(
                fault["status"],
                text="export unavailable " + request.headers["authorization"],
            )
        response = n.client.request(
            request.method, request.url.path, headers=request.headers, content=request.content
        )
        if response.status_code == 200 and request.method == "POST" and "job" in fault:
            body = response.json()
            body["job"]["object_paths"]["samples"] = fault["job"]
            return httpx.Response(200, json=body)
        if response.status_code == 200 and request.method == "POST" and fault.get("challenge"):
            body = response.json()
            envelope = body["challenge_envelope"]
            if fault["challenge"] == "signature":
                envelope["sig"] = "00" * 64
            else:
                envelope["body"]["nonce"] = "cd" * 32
            return httpx.Response(200, json=body)
        return httpx.Response(response.status_code, content=response.content)

    client_type = httpx.Client
    monkeypatch.setattr(
        trial_export.httpx,
        "Client",
        lambda **kwargs: client_type(transport=httpx.MockTransport(handler), **kwargs),
    )
    out = tmp_path / "download" / "trial"
    args = [
        "--api",
        "https://coordinator.test",
        "--run-id",
        n.manifest.run_id(),
        "--admission-id",
        identity,
        "--admin-token-file",
        str(tmp_path / "admin"),
        "--out",
        str(out),
    ]
    return trial_export, args, out, requests, fault


@pytest.mark.parametrize("existing", [False, True])
def test_cli_exports_copyable_folder(cli, trial, capsys, record_property, monkeypatch, existing):
    module, args, out, requests, _ = cli
    n, identity, envelope = trial
    if existing:
        out.mkdir(parents=True)
        monkeypatch.chdir(out)
        args[-1] = "."
        inode = out.stat().st_ino
    assert module.main(args) == 0
    if existing:
        assert out.stat().st_ino == inode
        assert Path.cwd() == out
    output = capsys.readouterr()
    assert output.err == ""
    report = json.loads(output.out)
    record_property("cli_json", output.out.strip())
    job = IslandJobV1.model_validate_json((out / "job.json").read_bytes())
    job.manifest.validate_run_id(job.run_id)
    assert (out / "job.json").read_bytes() == canonicalize(job.body())
    assert json.loads((out / "challenge.json").read_bytes()) == envelope
    expected = {"job.json", "challenge.json", *job.object_paths.values()}
    assert {p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file()} == expected
    assert report == {
        "out": "." if existing else str(out),
        "run_id": n.manifest.run_id(),
        "admission_id": identity,
        "nonce": envelope["body"]["nonce"],
        "deadline": job.deadline,
        "files": {name: sha256_hex((out / name).read_bytes()) for name in expected},
    }
    staged = n.store.state_dir / "trials-v2" / report["nonce"]
    for relative in job.object_paths.values():
        assert (out / relative).read_bytes() == (staged / relative).read_bytes()
    assert len(requests) == 6
    assert not any(r.url.path.endswith("/challenge") for r in requests)
    assert requests[0].method == "POST"
    assert requests[0].url.path == n.url + f"/admin/trial/{identity}/export"
    assert requests[0].headers["authorization"] == service.admin()["Authorization"]


@pytest.mark.parametrize(
    "fault,error",
    [
        ({"corrupt": 5}, "sha256"),
        ({"status": 409}, "409 export unavailable"),
        ({"status": 302}, "302 export unavailable"),
        ({"job": "../escape"}, "path"),
        ({"job": "job.json"}, "path"),
        ({"job": "ef_in"}, "path"),
        ({"challenge": True}, "challenge"),
        ({"challenge": "signature"}, "challenge"),
    ],
)
def test_cli_failure_leaves_no_partial_folder(cli, capsys, fault, error):
    module, args, out, _, faults = cli
    faults.update(fault)
    assert module.main(args) == 1
    output = capsys.readouterr()
    assert output.out == ""
    assert error in output.err.lower()
    assert service.admin()["Authorization"].removeprefix("Bearer ") not in output.err
    assert not out.exists() or list(out.iterdir()) == []
    assert not (out.parent / "escape").exists()
    assert list(out.parent.glob(".trial-export-*")) == []


def test_cli_refuses_resolved_path_escape(cli, tmp_path, capsys):
    module, args, out, _, fault = cli
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "keep").write_bytes(b"existing data")
    actual_resolve = Path.resolve

    def resolve(path, *args, **kwargs):
        if path.name == "samples" and path.parent.name.startswith(".trial-export-"):
            return outside / "escaped"
        return actual_resolve(path, *args, **kwargs)

    fault["job"] = "samples"
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(Path, "resolve", resolve)
        assert module.main(args) == 1
    assert "path" in capsys.readouterr().err
    assert not out.exists()
    assert sorted(p.name for p in outside.iterdir()) == ["keep"]
    assert (outside / "keep").read_bytes() == b"existing data"


def test_cli_publication_failure_rolls_back(cli, monkeypatch, capsys):
    module, args, out, _, _ = cli
    out.mkdir(parents=True)
    inode = out.stat().st_ino
    actual_rename = Path.rename
    published = 0

    def rename(path, target):
        nonlocal published
        if path.parent.name.startswith(".trial-export-"):
            published += 1
            if published == 2:
                raise OSError("simulated publication failure")
        return actual_rename(path, target)

    with monkeypatch.context() as patch:
        patch.setattr(Path, "rename", rename)
        assert module.main(args) == 1
    output = capsys.readouterr()
    assert output.out == ""
    assert "simulated publication failure" in output.err
    assert list(out.iterdir()) == []
    assert out.stat().st_ino == inode
    assert list(out.parent.glob(".trial-export-*")) == []
    assert module.main(args) == 0


def test_cli_rejects_malformed_token_before_http(cli, monkeypatch, capsys):
    module, args, out, requests, _ = cli
    token = service.admin()["Authorization"].removeprefix("Bearer ") + "\nsecond-line"
    actual_read = Path.read_text

    def read(path, *a, **kwargs):
        return token if path == Path(args[7]) else actual_read(path, *a, **kwargs)

    def client(**kwargs):
        raise httpx.LocalProtocolError(f"Illegal header value {('Bearer ' + token).encode()!r}")

    monkeypatch.setattr(Path, "read_text", read)
    monkeypatch.setattr(module.httpx, "Client", client)
    assert module.main(args) == 1
    output = capsys.readouterr()
    assert output.out == ""
    safe_error = output.err.strip() == "admin token file must contain one ASCII bearer token"
    assert safe_error, "malformed token must be rejected before HTTP without exposing it"
    assert requests == []
    assert not out.exists()


def test_cli_refuses_nonempty_output(cli, capsys):
    module, args, out, requests, _ = cli
    out.mkdir(parents=True)
    existing = out / "keep"
    existing.write_bytes(b"existing data")
    assert module.main(args) == 2
    output = capsys.readouterr()
    assert "empty" in output.err.lower()
    assert existing.read_bytes() == b"existing data"
    assert requests == []


def staged_inputs(n, identity):
    """The arguments `stage_trial_v2` takes for the open trial of `identity`."""
    store = n.store
    row = store._db.execute(
        "SELECT challenge,trial_epoch FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()
    challenge = JoinChallenge.model_validate_json(row["challenge"])
    beacon = store._beacon_v2(challenge.seed_beacon)
    samples = trial_samples(n.manifest, identity, challenge.nonce, beacon)
    return n.manifest.run_id(), challenge, samples, row["trial_epoch"]


def test_staging_survives_a_write_that_died_halfway(trial, monkeypatch):
    # Given: the disk fills while the first input is written
    n, identity, _ = trial
    arguments = staged_inputs(n, identity)
    real = Path.write_bytes

    def dies_halfway(self, data):
        real(self, data[: len(data) // 2])
        raise OSError("disk full")

    monkeypatch.setattr(Path, "write_bytes", dies_halfway)
    with pytest.raises(OSError, match="disk full"):
        n.store.stage_trial_v2(*arguments)
    monkeypatch.setattr(Path, "write_bytes", real)
    # When / Then: the half-written file is not left under the name of an input, so the next
    # staging works instead of refusing the torn bytes as "immutable trial inputs changed"
    job, directory = n.store.stage_trial_v2(*arguments)
    assert [p.name for p in directory.iterdir()].count("start_state") == 1
    assert sha256_hex((directory / "start_state").read_bytes()) == job.start_state_sha256


def test_staging_again_does_not_rewrite_an_input_a_launch_may_be_reading(trial):
    # Given: the inputs were staged once and a launch reads them
    n, identity, _ = trial
    arguments = staged_inputs(n, identity)
    _, directory = n.store.stage_trial_v2(*arguments)
    names = ("start_state", "ef_in", "v0", "samples", "sample_proofs")
    seen = {name: os.stat(directory / name) for name in names}
    time.sleep(0.05)
    # When: the same trial is staged again (every export and every reference stages it)
    n.store.stage_trial_v2(*arguments)
    # Then: not one input was opened for writing again
    for name in names:
        now = os.stat(directory / name)
        assert (now.st_ino, now.st_mtime_ns) == (seen[name].st_ino, seen[name].st_mtime_ns), name
