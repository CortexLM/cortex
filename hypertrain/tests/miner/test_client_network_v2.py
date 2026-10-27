"""Explicit v2 client authority and manifest-N launcher integration."""

from __future__ import annotations

import json
from pathlib import Path

import httpx
import pytest

from hypertrain.miner.core import MinerConfig, MinerError, NetworkMiner
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RunManifestV2


def test_client_requires_owner_authority_when_manifest_signed_by_other(tmp_path: Path):
    # Given: valid signature, wrong pinned owner.
    import importlib.util
    import sys

    spec = importlib.util.spec_from_file_location(
        "client_fixture", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
    )
    fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = fixture
    spec.loader.exec_module(fixture)
    manifest = fixture.setup().manifest
    key = Keypair(b"a" * 32)
    keyfile = tmp_path / "miner.key"
    keyfile.write_bytes(b"a" * 32)
    keyfile.chmod(0o600)
    cfg = MinerConfig(
        "https://service.test",
        keyfile,
        tmp_path,
        "unused",
        manifest.training.reference_spec.image_digest,
        run_id=manifest.run_id(),
        owner_hotkey=fixture.COORD.ss58,
    )
    transport = httpx.MockTransport(
        lambda _: httpx.Response(
            200,
            json={
                "manifest_envelope": seal(key, "RunManifestV2", manifest.run_id(), manifest, 1000)
            },
        )
    )
    # When / Then
    with httpx.Client(transport=transport) as client, pytest.raises(MinerError, match="authority"):
        NetworkMiner(cfg, client)


def test_two_rank_manifest_launches_real_runtime(tmp_path: Path):
    # Given: real staged signed dataset, strict two-rank wrapper.
    import importlib.util
    import sys

    spec = importlib.util.spec_from_file_location(
        "rank_fixture", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
    )
    fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = fixture
    spec.loader.exec_module(fixture)
    s = fixture.setup()
    body = s.manifest.body()
    body["training"]["reference_spec"]["layout"].update(n_gpus=2, dp_size=2, ep_size=1, zero1=True)
    wrapper = RunManifestV2.model_validate(body)
    s = fixture.Setup(wrapper, s.policy, s.admission_policy, s.rows, s.tree)
    job = fixture.stage(s, tmp_path / "attempt", (0, 1), 0)
    (tmp_path / "attempt/job.json").write_text(job.model_dump_json())
    keyfile = tmp_path / "key"
    keyfile.write_bytes(b"b" * 32)
    keyfile.chmod(0o600)
    cfg = MinerConfig(
        "https://service.test",
        keyfile,
        tmp_path / "work",
        "unused",
        wrapper.training.reference_spec.image_digest,
        run_id=wrapper.run_id(),
        owner_hotkey=fixture.COORD.ss58,
    )
    transport = httpx.MockTransport(
        lambda _: httpx.Response(
            200,
            json={
                "manifest_envelope": seal(
                    fixture.COORD, "RunManifestV2", wrapper.run_id(), wrapper, 1000
                )
            },
        )
    )
    with httpx.Client(transport=transport) as client:
        network = NetworkMiner(cfg, client)
        # When
        result = network.launch(tmp_path / "attempt/job.json")
    # Then: two real rank artifacts, one identical island commitment.
    summary0 = json.loads((tmp_path / "attempt/published/rank-0/summary.json").read_bytes())
    summary1 = json.loads((tmp_path / "attempt/published/rank-1/summary.json").read_bytes())
    assert summary0["commitments"] == summary1["commitments"]
    assert Path(result["state"]).exists()


def test_kimi_manifest_joins_and_runs_v2_through_the_cli(monkeypatch, tmp_path: Path):
    """D18: join-v2 then run-v2 --job for kimi-k3-pro-tiny through the signed v2 manifest; the
    torchrun worker (a fresh interpreter) must equal the in-process reference loop bit for bit."""
    import importlib.util
    import math
    import sys

    import numpy as np

    import hypertrain.models.kimi_tiny as kt
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.miner import cli
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.protocol.messages import f32hex
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.loop import Assignment, train_round
    from hypertrain.trainer.model import init_params, param_shapes

    spec = importlib.util.spec_from_file_location(
        "kimi_fixture", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
    )
    fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = fixture
    spec.loader.exec_module(fixture)
    # Given: an owner-signed v2 manifest with the kimi MICRO geometry and a real dataset.
    s = fixture.setup()
    body = s.manifest.body()
    training = body["training"]
    training["model"].update(
        arch=kt.ARCH,
        n_layers=3,
        d_model=32,
        n_heads=4,
        n_kv_heads=4,
        d_ff=16,
        vocab=64,
        seq_len=16,
        n_experts=8,
        top_k_experts=2,
        rope_theta=50000,
        init_seed=7,
        capacity_factor=f32hex(1.0),
        aux_loss_coef=f32hex(1e-4),
        param_count=sum(math.prod(x) for x in kt.param_shapes(kt.MICRO).values()),
    )
    training["inner"].update(H=2, J=1, micro_batch=2)
    rows = ((np.arange(16 * 17) * 7919 + 3).reshape(16, 17) % 64).astype("<u4")
    tree = MerkleTree([r.tobytes() for r in rows])
    training["dataset"].update(merkle_root=tree.root.hex(), n_samples=16, depth=4)
    cfg = TrainConfig.from_manifest_v2(RunManifestV2.model_validate(body))
    training["init_state_hash"] = state_hash(init_params(cfg.model))
    manifest = RunManifestV2.model_validate(body)
    run = manifest.run_id()
    s = fixture.Setup(manifest, s.policy, s.admission_policy, rows, tree)
    job = fixture.stage(s, tmp_path / "attempt", (0, 1, 2, 3), 0)
    (tmp_path / "attempt/job.json").write_text(job.model_dump_json())
    for name, seed in (("hot.key", b"b"), ("cold.key", b"c")):
        (tmp_path / name).write_bytes(seed * 32)
        (tmp_path / name).chmod(0o600)
    config = tmp_path / "miner.toml"
    config.write_text(
        f'api = "https://service.test"\nkeyfile = "{tmp_path / "hot.key"}"\n'
        f'workdir = "{tmp_path / "work"}"\nstate_source = "unused"\n'
        f'image_digest = "{manifest.training.reference_spec.image_digest}"\n'
        f'run_id = "{run}"\nowner_hotkey = "{fixture.COORD.ss58}"\ndevice = "cpu"\n'
    )
    envelope = seal(fixture.COORD, "RunManifestV2", run, manifest, 1000)
    joins: list[dict] = []

    def master(request: httpx.Request) -> httpx.Response:
        if (request.method, request.url.path) == ("GET", f"/v2/runs/{run}"):
            return httpx.Response(200, json={"manifest_envelope": envelope})
        if (request.method, request.url.path) == ("POST", f"/v2/runs/{run}/join"):
            joins.append(json.loads(request.content))
            return httpx.Response(200, json={"state": "APPLIED"})
        return httpx.Response(404, json={"detail": "unexpected route"})

    client_type = httpx.Client
    monkeypatch.setattr(
        cli.httpx,
        "Client",
        lambda **kwargs: client_type(transport=httpx.MockTransport(master), **kwargs),
    )
    args = ["--config", str(config)]
    # When
    joined = ["join-v2", *args, "--coldkey", str(tmp_path / "cold.key")]
    assert cli.main([*joined, "--request-id", "ab" * 32, "--expiry", "100"]) == 0
    assert cli.main(["run-v2", *args, "--job", str(tmp_path / "attempt/job.json")]) == 0
    # Then: the join binds this run and policy; the worker trained kimi exactly as the reference.
    assert [(j["run_id"], j["policy_hash"]) for j in joins] == [
        (run, manifest.network.admission_policy_hash)
    ]
    published = tmp_path / "attempt/published/rank-0"
    commitments = json.loads((published / "summary.json").read_bytes())["commitments"]
    theta0 = init_params(cfg.model)
    ref = train_round(
        cfg,
        theta0,
        Assignment(run, 0, (0, 1, 2, 3), 0),
        lambda i: rows[i].astype(np.uint32),
    )
    assert (
        commitments["leaves_root"],
        commitments["final_theta_hash"],
        commitments["delta_hash"],
    ) == (ref.leaves_root, ref.final_theta_hash, ref.delta_hash)
    assert (published / "delta.bin").read_bytes() == ref.delta_payload
    theta, _ = unpack_state((published / "state.safetensors").read_bytes())
    assert sorted(theta) == sorted(param_shapes(cfg.model)) == sorted(kt.param_shapes(cfg.model))
    bias = "layers.001.router_bias"
    assert not np.array_equal(theta[bias].numpy(), theta0[bias].numpy())


def test_run_v2_cli_dispatches_exact_round_argument(monkeypatch, capsys, tmp_path):
    from hypertrain.miner import cli

    # Parser-only contract; actual NetworkMiner.run_round has separate real HTTP/TLS proof.
    calls = []

    class ParserActor:
        def __init__(self, cfg, client):
            pass

        def run_round(self, w):
            calls.append(w)
            return "UPLOADED"

    monkeypatch.setattr(cli, "NetworkMiner", ParserActor)
    monkeypatch.setattr(cli.MinerConfig, "load", lambda _: object())
    assert cli.main(["run-v2", "--round", "7"]) == 0
    assert calls == [7]
    assert json.loads(capsys.readouterr().out) == {"status": "UPLOADED"}


def _parts_miner(tmp_path: Path):
    """NetworkMiner over a mock v2 object route backed by a real LocalFSStore."""
    import importlib.util
    import sys

    from hypertrain.data.store import LocalFSStore
    from hypertrain.protocol.envelope_v2 import Intake, parse_envelope
    from hypertrain.protocol.hashing import sha256_hex

    spec = importlib.util.spec_from_file_location(
        "parts_fixture", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
    )
    fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = fixture
    spec.loader.exec_module(fixture)
    manifest = fixture.setup().manifest
    keyfile = tmp_path / "key"
    keyfile.write_bytes(b"c" * 32)
    keyfile.chmod(0o600)
    cfg = MinerConfig(
        "https://service.test",
        keyfile,
        tmp_path / "work",
        "unused",
        manifest.training.reference_spec.image_digest,
        run_id=manifest.run_id(),
        owner_hotkey=fixture.COORD.ss58,
    )
    store, puts = LocalFSStore(tmp_path / "objects"), []
    envelope = seal(fixture.COORD, "RunManifestV2", manifest.run_id(), manifest, 1000)
    hotkey = Keypair(b"c" * 32)

    def route(request: httpx.Request) -> httpx.Response:
        if "/objects/" not in request.url.path:
            return httpx.Response(200, json={"manifest_envelope": envelope, "now_round": 5})
        sha = request.url.path.rsplit("/", 1)[1]
        if request.method == "POST":
            # The body is the signed request, then the object; no header carries a signature.
            assert not [h for h in request.headers if h.lower().startswith("x-")]
            head = int.from_bytes(request.content[:4], "big")
            signed, data = request.content[4 : 4 + head], request.content[4 + head :]
            upload = Intake(manifest.run_id(), {"ObjectUploadRequest": hotkey.ss58.__eq__}).accept(
                signed, 5
            )
            assert (upload.sha256, upload.size) == (sha, len(data)) == (sha256_hex(data), len(data))
            assert parse_envelope(signed).exp_drand == 5 + 20 and len(data) <= 1 << 20
            puts.append(data)
            return httpx.Response(200, json={"sha256": store.put(data)})
        path = store._path(sha)
        return (
            httpx.Response(200, content=path.read_bytes())
            if path.is_file()
            else httpx.Response(404)
        )

    return NetworkMiner(cfg, httpx.Client(transport=httpx.MockTransport(route))), store, puts


def test_parts_publish_and_fetch_when_artifact_exceeds_one_part(tmp_path: Path):
    import hashlib
    import os
    from types import SimpleNamespace

    from hypertrain.data.store import parse_index
    from hypertrain.gpu_ops.work_screen import artifact_ref

    # Given: a >1 MiB artifact and a small one.
    miner, _, puts = _parts_miner(tmp_path)
    big, small = os.urandom(3 * (1 << 20) + 5), b"small-object"
    (tmp_path / "big").write_bytes(big)
    (tmp_path / "small").write_bytes(small)
    # When: signed per-part publication.
    ref = artifact_ref(tmp_path / "big", miner._put_object)
    plain = artifact_ref(tmp_path / "small", miner._put_object)
    # Then: ref = index object (true SHA + stored length), index last, screen ref identical.
    index = parse_index(puts[4])
    assert index is not None and index.sha256 == hashlib.sha256(big).hexdigest()
    assert (ref.sha256, ref.size) == (hashlib.sha256(puts[4]).hexdigest(), len(puts[4]))
    assert artifact_ref(tmp_path / "big") == ref and len(puts) == 6
    assert (plain.sha256, plain.size) == (hashlib.sha256(small).hexdigest(), len(small))
    # When: the job pins the whole SHA, objects map points at the index ref.
    directory = tmp_path / "job"
    directory.mkdir()
    job = SimpleNamespace(
        start_state_sha256=index.sha256,
        ef_in_sha256="00" * 32,
        v0_sha256="00" * 32,
        object_paths={"start_state": "start_state"},
    )
    miner._fetch_inputs(job, {"start_state": ref.sha256}, directory)
    # Then: reassembled file exists before any launch.
    assert (directory / "start_state").read_bytes() == big


@pytest.mark.parametrize("fault", ["part", "index", "whole", "oversize", "name", "alias"])
def test_parts_fetch_rejects_without_partial_file(tmp_path: Path, fault: str):
    import hashlib
    import os
    from types import SimpleNamespace

    from hypertrain.data.store import parse_index
    from hypertrain.gpu_ops.work_screen import artifact_ref

    # Given: honest publication, then one fault.
    miner, store, puts = _parts_miner(tmp_path)
    big = os.urandom(2 * (1 << 20) + 9)
    (tmp_path / "big").write_bytes(big)
    ref = artifact_ref(tmp_path / "big", miner._put_object).sha256
    whole = hashlib.sha256(big).hexdigest()
    if fault == "part":  # tampered part bytes under its key
        part = store._path(parse_index(puts[-1]).parts[1])
        raw = part.read_bytes()
        part.write_bytes(raw[:-1] + bytes([raw[-1] ^ 1]))
    elif fault == "index":  # objects map names an unknown ref
        ref = "ab" * 32
    elif fault == "whole":  # parts honest, but the job pins another whole SHA
        whole = "cd" * 32
    elif fault == "oversize":  # an oversized body on the wire
        ref = store.put(os.urandom(3 << 20))
    objects = {"../start_state" if fault == "name" else "start_state": ref}
    paths = {"start_state": "start_state"}
    if fault == "alias":  # two names, one confined file: later would overwrite earlier
        objects["ef_in"], paths["ef_in"] = ref, "./start_state"
    directory = tmp_path / "job"
    directory.mkdir()
    job = SimpleNamespace(
        start_state_sha256=whole,
        ef_in_sha256=whole,
        v0_sha256=whole,
        object_paths=paths,
    )
    # When / Then: rejected, and nothing is published into the job directory.
    with pytest.raises(
        (MinerError, httpx.HTTPStatusError), match="corrupt|mismatch|404|part bound|object names"
    ):
        miner._fetch_inputs(job, objects, directory)
    assert os.listdir(directory) == [] and not (tmp_path / "start_state").exists()


def test_object_get_stops_reading_when_stream_exceeds_part_bound(tmp_path: Path):
    # Given: a server streaming an unbounded body in 64 KiB pieces, never prebuffered.
    miner, _, _ = _parts_miner(tmp_path)
    served = []

    class Endless(httpx.SyncByteStream):
        def __iter__(self):
            while len(served) < 1024:  # 64 MiB if fully consumed
                served.append(64 << 10)
                yield bytes(64 << 10)

    route = miner.api.c._transport.handler
    miner.api.c._transport.handler = lambda request: (
        httpx.Response(200, stream=Endless()) if request.method == "GET" else route(request)
    )
    # When / Then: refused at the 1 MiB bound; at most one piece read beyond it.
    with pytest.raises(MinerError, match="part bound"):
        miner._get_object("ab" * 32)
    assert sum(served) <= (1 << 20) + (64 << 10)


def test_publication_stops_when_cancel_set_during_first_put(tmp_path: Path):
    import threading
    import time

    from hypertrain.data.store import parse_index
    from hypertrain.gpu_ops.work_screen import artifact_ref

    # Given: a 4-part file; cancel is set while the first part PUT is in flight.
    miner, _, puts = _parts_miner(tmp_path)
    (tmp_path / "big").write_bytes(bytes(range(256)) * (3 * 4096 + 1))
    cancel, sent = threading.Event(), miner._put_object

    def first(data: bytes) -> str:
        cancel.set()
        return sent(data)

    miner._put_object = first
    # When / Then: no further part and no index; the one part is harmless content-addressed.
    with pytest.raises(MinerError, match="deadline/cancellation"):
        artifact_ref(tmp_path / "big", miner._bounded_put(time.time() + 3600, cancel))
    assert len(puts) == 1 and parse_index(puts[0]) is None
    # And: a passed deadline publishes nothing at all.
    with pytest.raises(MinerError, match="deadline/cancellation"):
        artifact_ref(tmp_path / "big", miner._bounded_put(time.time() - 1, None))
    assert len(puts) == 1


def test_object_get_rejects_compressed_response_before_decoding(tmp_path: Path):
    import gzip

    # Given: a hostile server ignoring Accept-Encoding: identity, gzip bomb body.
    miner, _, _ = _parts_miner(tmp_path)
    body, served, seen = gzip.compress(bytes(4 << 20)), [], []

    class Bomb(httpx.SyncByteStream):
        def __iter__(self):
            served.append(len(body))
            yield body

    def gzip_route(request: httpx.Request) -> httpx.Response:
        seen.append(request.headers["Accept-Encoding"])
        return httpx.Response(200, headers={"Content-Encoding": "gzip"}, stream=Bomb())

    miner.api.c._transport.handler = gzip_route
    # When / Then: refused from headers alone; nothing read or decoded.
    with pytest.raises(MinerError, match="content encoding"):
        miner._get_object("ab" * 32)
    assert seen == ["identity"] and served == []


def test_object_put_sends_content_length_and_retries_5xx_and_transport(tmp_path: Path, monkeypatch):
    import hypertrain.miner.core as core

    # Given: a proxy that drops the first upload, answers 503 to the second, then stores.
    monkeypatch.setattr(core, "RETRY_DELAYS", (0, 0, 0))
    miner, store, puts = _parts_miner(tmp_path)
    miner._round()  # the round is read once and reused: the failures below hit the upload itself
    route, seen = miner.api.c._transport.handler, []

    def flaky(request: httpx.Request) -> httpx.Response:
        seen.append(
            (
                request.headers.get("Content-Length"),
                request.headers.get("Transfer-Encoding"),
                len(request.content),
            )
        )
        if len(seen) == 1:
            raise httpx.ConnectError("reset", request=request)
        return httpx.Response(503) if len(seen) == 2 else route(request)

    miner.api.c._transport.handler = flaky
    # When: one object upload.
    digest = miner._put_object(b"payload")
    # Then: stored after two retries, every attempt a sized (non-chunked) body.
    assert store.get(digest) == b"payload" and len(puts) == 1
    assert len(seen) == 3
    assert all(length == str(size) and encoding is None for length, encoding, size in seen)


def test_the_round_is_read_once_for_many_uploads(tmp_path: Path):
    # Given: a miner whose master counts the reads of its current round.
    miner, _, puts = _parts_miner(tmp_path)
    route, reads = miner.api.c._transport.handler, []

    def counting(request: httpx.Request) -> httpx.Response:
        if request.method == "GET" and "/objects/" not in request.url.path:
            reads.append(request.url.path)
        return route(request)

    miner.api.c._transport.handler = counting
    # When: five uploads within the time a read is reused.
    for number in range(5):
        miner._put_object(b"part %d" % number)
    # Then: one read signed all of them.
    assert len(puts) == 5 and len(reads) == 1


def test_each_upload_attempt_is_signed_with_the_round_it_is_sent_in(tmp_path: Path, monkeypatch):
    import hypertrain.miner.core as core
    from hypertrain.protocol.envelope_v2 import parse_envelope

    # Given: a master whose round moves between the attempts (a read is never reused here),
    # and that answers 503 to the first two uploads.
    monkeypatch.setattr(core, "RETRY_DELAYS", (0, 0))
    monkeypatch.setattr(core, "ROUND_TTL", -1)
    miner, _, _ = _parts_miner(tmp_path)
    route, rounds, expiries = miner.api.c._transport.handler, iter([5, 9, 12]), []

    def moving(request: httpx.Request) -> httpx.Response:
        if request.method == "GET" and "/objects/" not in request.url.path:
            return httpx.Response(200, json={**route(request).json(), "now_round": next(rounds)})
        head = int.from_bytes(request.content[:4], "big")
        expiries.append(parse_envelope(request.content[4 : 4 + head]).exp_drand)
        digest = request.url.path.rsplit("/", 1)[1]
        return (
            httpx.Response(503)
            if len(expiries) < 3
            else httpx.Response(200, json={"sha256": digest})
        )

    miner.api.c._transport.handler = moving
    # When / Then: a retry after a long backoff carries a request good for its own round.
    miner._put_object(b"payload")
    assert expiries == [5 + 20, 9 + 20, 12 + 20]


def test_object_put_does_not_retry_4xx_and_gives_up_after_bounded_5xx(tmp_path: Path, monkeypatch):
    import hypertrain.miner.core as core

    monkeypatch.setattr(core, "RETRY_DELAYS", (0, 0))
    miner, _, _ = _parts_miner(tmp_path)
    miner._round()  # the round is read once and reused: the answers below are for the upload
    calls = []

    def answer(status: int):
        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(status)
            return httpx.Response(status)

        return handler

    # 403: refused at once.
    miner.api.c._transport.handler = answer(403)
    with pytest.raises(httpx.HTTPStatusError):
        miner._put_object(b"x")
    assert calls == [403]
    # 500 forever: 1 attempt + len(RETRY_DELAYS) retries, then the error propagates.
    calls.clear()
    miner.api.c._transport.handler = answer(500)
    with pytest.raises(httpx.HTTPStatusError):
        miner._put_object(b"x")
    assert calls == [500] * 3


def test_object_get_and_api_get_retry_5xx_but_signed_post_is_never_replayed(
    tmp_path: Path, monkeypatch
):
    import hypertrain.miner.core as core
    from hypertrain.miner.core import ChallengeHTTPError

    monkeypatch.setattr(core, "RETRY_DELAYS", (0, 0, 0))
    miner, store, _ = _parts_miner(tmp_path)
    sha = store.put(b"part")
    route, hits = miner.api.c._transport.handler, []

    def flaky(request: httpx.Request) -> httpx.Response:
        hits.append(request.method)
        if len(hits) % 2 == 1:
            return httpx.Response(502, json={"detail": "bad gateway"})
        if request.method == "POST":
            return httpx.Response(200, json={"ok": True})
        return route(request)

    miner.api.c._transport.handler = flaky
    assert miner._get_object(sha) == b"part" and hits == ["GET", "GET"]
    hits.clear()
    assert "manifest_envelope" in miner.api.call("GET", "/v2/runs/x") and len(hits) == 2
    hits.clear()
    with pytest.raises(ChallengeHTTPError, match="502"):
        miner.api.call("POST", "/v2/runs/x/commit", json={})
    assert hits == ["POST"]


def test_fetch_inputs_reuses_own_carry_and_downloads_only_theta(tmp_path: Path):
    import hashlib
    from types import SimpleNamespace

    import torch

    from hypertrain.auditor.replay import pack_state
    from hypertrain.data.store import put_large
    from hypertrain.trainer.optim import OptState

    # Given: the miner's own previous publication (theta_old + m/v/step, ef) and a new global
    # theta published by the master as a theta-only object.
    miner, store, _ = _parts_miner(tmp_path)
    n = 300_000  # > 1 MiB per tensor: indexed objects
    carry = OptState({"w": torch.full((n,), 0.5)}, {"w": torch.full((n,), 0.25)}, 7)
    previous = tmp_path / "prev"
    previous.mkdir()
    (previous / "state.safetensors").write_bytes(pack_state({"w": torch.zeros(n)}, carry))
    ef = pack_state({"w": torch.ones(n)})
    (previous / "ef.safetensors").write_bytes(ef)
    theta = {"w": torch.arange(n, dtype=torch.float32)}
    start = pack_state(theta, carry)
    theta_ref = put_large(pack_state(theta), store.put)
    full = {"start_state": put_large(start, store.put), "ef_in": put_large(ef, store.put)}
    job = SimpleNamespace(
        start_state_sha256=hashlib.sha256(start).hexdigest(),
        ef_in_sha256=hashlib.sha256(ef).hexdigest(),
        v0_sha256="00" * 32,
        object_paths={"start_state": "start_state", "ef_in": "ef_in"},
    )
    gets: list[str] = []
    fetch = miner._get_object
    miner._get_object = lambda digest: gets.append(digest) or fetch(digest)
    directory = tmp_path / "job"
    directory.mkdir()
    # When: inputs are fetched with the local carry available.
    miner._fetch_inputs(job, full, directory, theta_ref, previous)
    # Then: byte-identical inputs; only the theta object was downloaded.
    assert (directory / "start_state").read_bytes() == start
    assert (directory / "ef_in").read_bytes() == ef
    assert gets[0] == theta_ref and not set(gets) & set(full.values())
    assert sorted(p.name for p in directory.iterdir()) == ["ef_in", "start_state"]


def test_fetch_inputs_falls_back_to_full_download_when_local_carry_differs(tmp_path: Path):
    import hashlib
    from types import SimpleNamespace

    import torch

    from hypertrain.auditor.replay import pack_state
    from hypertrain.data.store import put_large
    from hypertrain.trainer.optim import OptState

    # Given: a stale local publication whose m/v and ef differ from the master's pins.
    miner, store, _ = _parts_miner(tmp_path)
    n = 300_000
    previous = tmp_path / "prev"
    previous.mkdir()
    stale = OptState({"w": torch.zeros(n)}, {"w": torch.zeros(n)}, 1)
    (previous / "state.safetensors").write_bytes(pack_state({"w": torch.zeros(n)}, stale))
    (previous / "ef.safetensors").write_bytes(pack_state({"w": torch.zeros(n)}))
    theta = {"w": torch.arange(n, dtype=torch.float32)}
    start = pack_state(theta, OptState({"w": torch.ones(n)}, {"w": torch.ones(n)}, 2))
    ef = pack_state({"w": torch.ones(n)})
    full = {"start_state": put_large(start, store.put), "ef_in": put_large(ef, store.put)}
    job = SimpleNamespace(
        start_state_sha256=hashlib.sha256(start).hexdigest(),
        ef_in_sha256=hashlib.sha256(ef).hexdigest(),
        v0_sha256="00" * 32,
        object_paths={"start_state": "start_state", "ef_in": "ef_in"},
    )
    directory = tmp_path / "job"
    directory.mkdir()
    # When / Then: the pinned full objects are downloaded instead; no temp file remains.
    miner._fetch_inputs(job, full, directory, put_large(pack_state(theta), store.put), previous)
    assert (directory / "start_state").read_bytes() == start
    assert (directory / "ef_in").read_bytes() == ef
    assert sorted(p.name for p in directory.iterdir()) == ["ef_in", "start_state"]


@pytest.mark.parametrize("bad", ["missing", "corrupt", "wrong_shape"])
def test_fetch_inputs_falls_back_to_full_download_on_bad_theta_ref(tmp_path: Path, bad: str):
    import hashlib
    from types import SimpleNamespace

    import torch

    from hypertrain.auditor.replay import pack_state
    from hypertrain.data.store import put_large
    from hypertrain.trainer.optim import OptState

    # Given: a matching local carry, but the theta ref is unfetchable, corrupt or mis-shaped.
    miner, store, _ = _parts_miner(tmp_path)
    n = 300_000
    carry = OptState({"w": torch.full((n,), 0.5)}, {"w": torch.full((n,), 0.25)}, 7)
    previous = tmp_path / "prev"
    previous.mkdir()
    (previous / "state.safetensors").write_bytes(pack_state({"w": torch.zeros(n)}, carry))
    theta = {"w": torch.arange(n, dtype=torch.float32)}
    start = pack_state(theta, carry)
    theta_ref = {
        "missing": "ab" * 32,
        "corrupt": store.put(b"not a safetensors file"),
        "wrong_shape": put_large(pack_state({"w": torch.ones(n + 1)}), store.put),
    }[bad]
    full = {"start_state": put_large(start, store.put)}
    job = SimpleNamespace(
        start_state_sha256=hashlib.sha256(start).hexdigest(),
        ef_in_sha256="00" * 32,
        v0_sha256="00" * 32,
        object_paths={"start_state": "start_state"},
    )
    gets: list[str] = []
    fetch = miner._get_object
    miner._get_object = lambda digest: gets.append(digest) or fetch(digest)
    directory = tmp_path / "job"
    directory.mkdir()
    # When: inputs are fetched.
    miner._fetch_inputs(job, full, directory, theta_ref, previous)
    # Then: no MinerError; the pinned full object was downloaded; no temp file remains.
    assert (directory / "start_state").read_bytes() == start
    assert full["start_state"] in gets
    assert sorted(p.name for p in directory.iterdir()) == ["start_state"]
