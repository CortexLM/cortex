"""Permissionless Network v2 admission restricted to hotkeys registered on subnet netuid 100."""

from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path
from types import SimpleNamespace

import httpx
import pytest

from hypertrain.ledger.ledger import Ledger
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import QUICKNET_GENESIS

from .conftest import INTERNAL, PARAMS, SLUG, Master, bearer, make_client, miner

_spec = importlib.util.spec_from_file_location(
    "subnet_gate_service_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert _spec is not None and _spec.loader is not None
service = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = service
_spec.loader.exec_module(service)
HOT, COLD = service.HOT, service.COLD
H = "ab" * 32


class SubnetMaster:
    """Fake Cortex master: GET /v1/metagraph/latest?netuid=N for one subnet."""

    def __init__(self, netuid: int = 100) -> None:
        self.netuid, self.registered, self.queries = netuid, set[str](), list[str]()

    def handler(self, request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/v1/metagraph/latest"
        self.queries.append(request.url.params["netuid"])
        hotkeys = {k: i for i, k in enumerate(sorted(self.registered))}
        return httpx.Response(200, json={"netuid": self.netuid, "hotkeys": hotkeys})


@pytest.fixture
def subnet_master() -> SubnetMaster:
    return SubnetMaster()


@pytest.fixture
def gated(tmp_path, monkeypatch, subnet_master):
    monkeypatch.setattr(service, "subnet", lambda: httpx.MockTransport(subnet_master.handler))
    yield from service.network.__wrapped__(tmp_path, SimpleNamespace(param=None))


def refresh(n) -> None:
    """Drop the 30 s metagraph cache so a (de)registration is observed now."""
    n.client.app.state.metagraph._cache = None


def lock(n, index: int, identity: str) -> None:
    from hypertrain.protocol.messages_v2 import EscrowLock

    op = EscrowLock(
        operation_id=f"{index + 1:064x}",
        owner=COLD[index].ss58,
        units=1000,
        origin_ids=[n.origin_ids[index]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = n.client.post(n.url + "/escrow/lock", json=n.signed(COLD[index], "EscrowLock", op))
    assert response.status_code == 200, response.text


def test_registered_hotkey_reaches_active_without_admin(gated, subnet_master, tmp_path):
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.gpu_ops.work_screen import screen_work
    from hypertrain.protocol.messages_v2 import JoinChallenge

    # Given: only the subnet registration; no operator token used after setup.
    n = gated
    subnet_master.registered.add(HOT[0].ss58)
    response = n.join(0)
    assert response.status_code == 200, response.text
    identity = response.json()["admission_id"]
    lock(n, 0, identity)
    run_id = n.manifest.run_id()
    # When: the miner alone repeats challenge -> own work -> upload -> signed proof.
    for trial in range(12):
        n.push(n.now + 1)
        response = n.client.get(n.url + "/join/" + identity + "/challenge")
        assert response.status_code == 200, response.text
        challenge = JoinChallenge.model_validate(response.json()["body"])
        epoch = n.store._db.execute(
            "SELECT trial_epoch FROM admissions_v2 WHERE admission_id=?", (identity,)
        ).fetchone()[0]
        samples = trial_samples(
            n.manifest, identity, challenge.nonce, n.store._beacon_v2(challenge.seed_beacon)
        )
        job, staged = n.store.stage_trial_v2(run_id, challenge, samples, epoch)
        work = tmp_path / ("miner-" + challenge.nonce)
        work.mkdir()
        for relative in job.object_paths.values():
            shutil.copyfile(staged / relative, work / relative)
        screen, proof = screen_work(job, work, challenge, now_beacon=n.now, backend="cpu")
        for name, ref in zip(
            ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json"),
            proof.artifact_refs,
            strict=True,
        ):
            signature = HOT[0].sign(f"hypertrain/object/2|{run_id}|{ref.sha256}".encode()).hex()
            response = n.client.put(
                n.url + "/objects/" + ref.sha256,
                content=(work / "published/rank-0" / name).read_bytes(),
                headers={"x-hotkey": HOT[0].ss58, "x-object-signature": signature},
            )
            assert response.status_code == 200, response.text
        response = n.client.post(
            n.url + "/join/proof",
            json={
                "proof": n.signed(HOT[0], "WorkProof", proof),
                "screen": n.signed(HOT[0], "WorkScreenV2", screen),
            },
        )
        assert response.status_code == 200, response.text
        record = response.json()
        assert record["clean_count"] == trial + 1 and not record["pending_dispute"]
    # Then: graduated by the service itself through the same finality checks.
    assert record["state"] == "ACTIVE"
    reasons = [
        r[0]
        for r in n.store._db.execute(
            "SELECT reason FROM admission_transitions WHERE admission_id=? ORDER BY seq",
            (identity,),
        )
    ]
    assert reasons == ["FULL_REFERENCE_MATCH", "GRADUATED_FULL_REPLAY"]
    finals = n.store._db.execute(
        "SELECT COUNT(*) FROM admission_reservations WHERE reservation LIKE 'trial-final|%'"
    ).fetchone()[0]
    assert finals == 12
    assert set(subnet_master.queries) == {"100"}


def _routes(n, hot, cold):
    """Every miner-signed v2 route with a body signed by `hot` (or naming it)."""
    run_id = n.manifest.run_id()
    work = {"admission_id": H, "challenge_hash": H, "leaves_root": H, "delta_hash": H}
    proof = n.signed(hot, "WorkProof", {**work, "artifact_refs": [{"sha256": H, "size": 1}]})
    accept = {
        "w": 0,
        "hotkey": hot.ss58,
        "assignment_hash": H,
        "image_digest": "sha256:" + H,
        "driver_version": "cpu",
        "n_gpus": 1,
        "work_screen_hash": H,
    }
    commit = {
        "w": 0,
        "hotkey": hot.ss58,
        "leaf_scheme": "ht-leaf-v1",
        "n_leaves": 1,
        **{k: H for k in ("leaves_root", "metrics_root", "final_theta_hash")},
        **{k: H for k in ("ef_in_hash", "ef_out_hash", "delta_hash")},
        "delta_bytes": 1,
        "tokens": 1,
    }
    delta = {
        "w": 0,
        "hotkey": hot.ss58,
        "delta_hash": H,
        "uri": "objects/" + H,
        "size": 1,
        "format": "ht-dense-int8-v1",
        "chunks": [{"off": 0, "len": 1, "sha256": H}],
        "grant_hash": H,
        "master_acceptance_hash": H,
    }
    chunks = {"size": 1, "chunks": [{"index": 0, "off": 0, "len": 1, "chunk_sha256": H}]}
    bisect = {
        "dispute_id": H,
        "seq": 0,
        "level": "step",
        "ctx": [],
        "interval": [0, 2],
        "N": 2,
        "hashes": [H, H, H],
        "party": hot.ss58,
        "previous_transcript_hash": H,
    }
    serve = {
        "hotkey": hot.ss58,
        "challenge_hash": H,
        "t": 0,
        "uri": "objects/" + H,
        "tensor_root": H,
        "merkle_proof_leaf_in_leaves_root": [],
    }
    ack = {
        "run_id": run_id,
        "cursor": 1,
        "event_hash": H,
        "party": hot.ss58,
        "received_beacon": 1,
        "sig": "00" * 64,
    }
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.messages_v2 import HardwareHint

    join = sign_join(
        hot,
        cold,
        run_id=run_id,
        request_id=H,
        expires_beacon=10000,
        policy_hash=n.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="advisory"),
    ).body()
    rotate = {"hotkey": HOT[0].ss58, "new_hotkey": hot.ss58, "operation_id": H}
    watch = hot.sign(f"hypertrain/watch/2|{run_id}|{hot.ss58}".encode()).hex()
    obj = hot.sign(f"hypertrain/object/2|{run_id}|{H}".encode()).hex()
    c, u = n.client, n.url
    dispute = {"hotkey": hot.ss58, "verdict_hash": H, "action": "accept", "bond_lock": None}
    return {
        "join": lambda: c.post(u + "/join", content=canonicalize(join)),
        "join/proof": lambda: c.post(u + "/join/proof", json={"proof": proof, "screen": proof}),
        "rotate": lambda: c.post(
            u + f"/admission/{HOT[0].ss58}/rotate",
            json=n.signed(COLD[0], "RotateRequest", rotate),
        ),
        "recover": lambda: c.post(
            u + "/admission/recover",
            json=n.signed(hot, "Receipt", {"w": 0, "commit_hash": H, "received_round": 1}),
        ),
        "objects": lambda: c.put(
            u + "/objects/" + H,
            content=b"x",
            headers={"x-hotkey": hot.ss58, "x-object-signature": obj},
        ),
        "accept": lambda: c.post(u + "/accept", json=n.signed(hot, "AcceptV2", accept)),
        "commit": lambda: c.post(u + "/commit", json=n.signed(hot, "CommitV2", commit)),
        "delta": lambda: c.post(u + "/delta", json=n.signed(hot, "DeltaManifestV2", delta)),
        "leaves": lambda: c.post(u + "/leaves", json=proof),
        "upload-grant": lambda: c.post(
            u + "/upload-grant",
            json=relay_envelope.seal(hot, "UploadChunkManifest", run_id, chunks, 10000),
        ),
        "dispute": lambda: c.post(u + "/dispute", json=n.signed(hot, "DisputeV2", dispute)),
        "bisect": lambda: c.post(u + "/bisect", json=n.signed(hot, "BisectV2", bisect)),
        "state-serve": lambda: c.post(u + "/state-serve", json=n.signed(hot, "StateServe", serve)),
        "disputes": lambda: c.get(
            u + "/disputes", params={"party": hot.ss58}, headers={"x-dispute-signature": watch}
        ),
        "disputes/ack": lambda: c.post(u + "/disputes/ack", json=ack),
    }


def test_unregistered_hotkey_forbidden_on_every_miner_route(gated, subnet_master):
    # Given: HOT[0] registered and joined; HOT[1] signs validly but is not on the subnet.
    n = gated
    subnet_master.registered.add(HOT[0].ss58)
    assert n.join(0).status_code == 200
    # When / Then: refused before any state change on every miner-signed route.
    before = n.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0]
    for name, call in _routes(n, HOT[1], COLD[1]).items():
        response = call()
        assert response.status_code == 403, (name, response.status_code, response.text)
        assert "not registered on the subnet" in response.json()["detail"], name
    assert n.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == before


def test_wrong_netuid_is_unavailable(tmp_path, monkeypatch):
    # Given: the master answers for another subnet.
    other = SubnetMaster(netuid=99)
    other.registered.add(HOT[0].ss58)
    monkeypatch.setattr(service, "subnet", lambda: httpx.MockTransport(other.handler))
    generator = service.network.__wrapped__(tmp_path, SimpleNamespace(param=None))
    n = next(generator)
    try:
        # When / Then: fail closed with 503, nothing admitted.
        response = n.join(0)
        assert response.status_code == 503, response.text
        assert "netuid 100" in response.json()["detail"]
        assert other.queries == ["100"]
        assert n.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 0
    finally:
        generator.close()


def test_deregistered_after_join_refused(gated, subnet_master):
    # Given: admitted while registered.
    n = gated
    subnet_master.registered.add(HOT[0].ss58)
    identity = n.join(0).json()["admission_id"]
    n.push(2)
    # When: the hotkey leaves the subnet.
    subnet_master.registered.clear()
    refresh(n)
    # Then: no new trial and no further submissions.
    assert n.client.get(n.url + "/join/" + identity + "/challenge").status_code == 403
    routes = _routes(n, HOT[0], COLD[0])
    for name in ("join/proof", "objects", "accept", "commit", "delta", "leaves", "upload-grant"):
        assert routes[name]().status_code == 403, name
    assert (
        n.store._db.execute(
            "SELECT challenge FROM admissions_v2 WHERE admission_id=?", (identity,)
        ).fetchone()[0]
        is None
    )


def test_weights_exclude_deregistered_hotkeys(tmp_path, secrets_dir, clock):
    # Given: two vested entitlements; one hotkey later deregistered.
    master = Master()
    kept, gone = miner(1).ss58, miner(2).ss58
    state = tmp_path / "state"
    ledger = Ledger(state / "ledger", PARAMS)
    at = QUICKNET_GENESIS + 10
    for k in (kept, gone):
        ledger.commit(0, k, at)
    for k in (kept, gone):
        ledger.verdict(0, k, "UNSAMPLED", 1000, 0, at)
    ledger.finalize(0, at)
    del ledger
    master.registered.add(kept)
    vested_at = QUICKNET_GENESIS + 11 * PARAMS.epoch_seconds
    headers = {**bearer(INTERNAL), "x-platform-challenge-slug": SLUG}
    with make_client(state, secrets_dir, master, clock) as c:
        # When
        first = c.get(f"/internal/v1/get_weights?epoch=11&epoch_at={vested_at}", headers=headers)
        # Then: only the registered hotkey is paid; the rest burns, and the answer is final.
        assert first.status_code == 200, first.text
        body = first.json()
        assert set(body["weights"]) == {kept}
        meta = body["metadata"]
        assert meta["units_unregistered"] == 500_000
        assert meta["units_paid"] == 500_000 and meta["units_burned_this_epoch"] == 500_000
        master.registered.add(gone)
        c.app.state.metagraph._cache = None
        again = c.get(f"/internal/v1/get_weights?epoch=11&epoch_at={vested_at}", headers=headers)
        assert again.content == first.content


def test_weights_fail_closed_on_wrong_netuid(tmp_path, secrets_dir, clock):
    master = SubnetMaster(netuid=7)
    with make_client(tmp_path / "state", secrets_dir, master, clock) as c:  # type: ignore[arg-type]
        r = c.get(
            "/internal/v1/get_weights?epoch=1",
            headers={**bearer(INTERNAL), "x-platform-challenge-slug": SLUG},
        )
        assert r.status_code == 503
