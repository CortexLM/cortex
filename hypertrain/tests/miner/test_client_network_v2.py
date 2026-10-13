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
    from hypertrain.protocol.keys import verify

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
            return httpx.Response(200, json={"manifest_envelope": envelope})
        sha = request.url.path.rsplit("/", 1)[1]
        if request.method == "PUT":
            message = f"hypertrain/object/2|{manifest.run_id()}|{sha}".encode()
            signature = bytes.fromhex(request.headers["X-Object-Signature"])
            assert request.headers["X-Hotkey"] == hotkey.ss58
            assert verify(hotkey.public, message, signature) and len(request.content) <= 1 << 20
            puts.append(request.content)
            return httpx.Response(200, json={"sha256": store.put(request.content)})
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
