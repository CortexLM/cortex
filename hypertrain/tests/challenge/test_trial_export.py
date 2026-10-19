"""Admin trial export and the copyable probe-v2 input folder."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import httpx
import pytest

from hypertrain.data.trial_assignment import trial_samples
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
    response = network.client.get(network.url + f"/join/{identity}/challenge")
    assert response.status_code == 200, response.text
    return network, identity, response.json()


def export(n, identity):
    return n.client.post(n.url + f"/admin/trial/{identity}/export", headers=service.admin())


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
        if fault.get("challenge") and request.url.path.endswith("/challenge"):
            body = response.json()
            if fault["challenge"] == "signature":
                body["sig"] = "00" * 64
            else:
                body["body"]["nonce"] = "cd" * 32
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
    assert len(requests) == 7
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
