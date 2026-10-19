"""ht-parts-v1 server transport: indexed refs on the wire, whole hashes in jobs/admission."""

from __future__ import annotations

import contextlib
import hashlib
import importlib
import sqlite3
import sys
from pathlib import Path

import pytest

from hypertrain.challenge.admission import _logical, _put_capped, _verify_ref
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.data.store import LocalFSStore, get_large, parse_index, put_large
from hypertrain.protocol.messages_v2 import (
    MAX_OBJECT_BYTES,
    PART_BYTES,
    ArtifactRef,
    IslandJobV1,
    PartIndexV1,
    StartStateV2,
)

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ledger"))
fixtures = importlib.import_module("test_escrow_v2")
BIG = bytes(range(256)) * ((PART_BYTES + 4096) // 256)  # deterministic, > 1 MiB


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def test_logical_ref_binds_stored_length_and_whole_hash(tmp_path: Path) -> None:
    objects = LocalFSStore(tmp_path)
    small = b"plain"
    plain = ArtifactRef(sha256=objects.put(small), size=len(small))
    assert _logical(objects, plain, "X") == (_sha(small), len(small), False)
    key = put_large(BIG, objects.put)
    stored = len(objects.get(key))
    assert key != _sha(BIG) and stored < PART_BYTES
    indexed = ArtifactRef(sha256=key, size=stored)
    assert _logical(objects, indexed, "X") == (_sha(BIG), len(BIG), True)
    for bad in (
        ArtifactRef(sha256=key, size=len(BIG)),  # logical size is not the stored ref size
        ArtifactRef(sha256=_sha(BIG), size=len(BIG)),  # whole sha was never stored
        ArtifactRef(sha256=plain.sha256, size=len(small) + 1),
    ):
        with pytest.raises(AdmissionError, match="X"):
            _logical(objects, bad, "X")


def test_served_root_decodes_indexed_state(tmp_path: Path) -> None:
    import torch

    from hypertrain.auditor.replay import pack_state, tensor_root
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.trainer.optim import OptState

    theta = {"w": torch.arange(300_000, dtype=torch.float32)}
    st = OptState(m={"w": torch.ones(300_000)}, v={"w": torch.zeros(300_000)}, step=3)
    store = ChallengeStore.__new__(ChallengeStore)
    store.objects = LocalFSStore(tmp_path)
    blob = put_large(pack_state(theta, st), store.objects.put)
    assert parse_index(store.objects.get(blob)) is not None
    assert store._served_root(blob) == tensor_root(theta, st)


def test_island_job_maps_indexed_refs_to_whole_hashes(tmp_path: Path, monkeypatch) -> None:
    import torch

    from hypertrain.auditor.replay import pack_state, unpack_state
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.protocol.jcs import canonicalize

    manifest = fixtures.setup().manifest
    run_id, hot = manifest.run_id(), fixtures.HOT.ss58
    objects = LocalFSStore(tmp_path / "objects")
    theta = {"w": torch.arange(300_000, dtype=torch.float32)}
    ef = {"w": torch.ones(300_000)}
    start = StartStateV2(
        run_id=run_id,
        w=0,
        hotkey=hot,
        theta_hash="11" * 32,
        state_object_sha256=put_large(pack_state(theta), objects.put),
        opt_state_hash="22" * 32,
        ef_object_sha256=put_large(pack_state(ef), objects.put),
        ef_hash="33" * 32,
        parent_anchor_hash="44" * 32,
        global_step0=0,
        anchor_verdict_hash="55" * 32,
    )
    training = manifest.training
    width = (training.model.seq_len + 1) * 4
    samples = bytes(range(256)) * (width * training.dataset.n_samples // 256 + 1)
    samples = samples[: width * training.dataset.n_samples]
    proofs = canonicalize([[i] for i in range(training.dataset.n_samples)])
    dataset = {"samples_hash": objects.put(samples), "proofs_hash": objects.put(proofs)}
    records = {
        "start": start.body(),
        "round": {"body": {"d_commit": 10}},
        "dataset": dataset,
    }
    store = ChallengeStore.__new__(ChallengeStore)
    store.objects, store.state_dir = objects, tmp_path / "state"
    store._db = sqlite3.connect(":memory:")  # empty records_v2: first emission of the job
    store._db.execute("CREATE TABLE records_v2(run_id, kind, id, data)")
    put: dict[str, object] = {}
    monkeypatch.setattr(store, "_tx", contextlib.nullcontext, raising=False)
    monkeypatch.setattr(store, "_run_v2", lambda *_: manifest, raising=False)
    monkeypatch.setattr(store, "_service_boundary_v2", lambda *_a, **_k: None, raising=False)
    monkeypatch.setattr(store, "_owner_v2", lambda *_: hot, raising=False)
    monkeypatch.setattr(store, "_assignment_v2", lambda *_: (0,), raising=False)
    monkeypatch.setattr(store, "_record_v2", lambda _r, kind, _i: records[kind], raising=False)
    monkeypatch.setattr(
        store, "_put_record_v2", lambda *a: put.__setitem__(a[1], a[3]), raising=False
    )
    staged = store.island_job_v2(run_id, 0, hot)
    job = IslandJobV1.model_validate(staged["job"])
    directory = store.state_dir / "jobs-v2" / "0" / hot
    for name, field in (("start_state", "start_state_sha256"), ("ef_in", "ef_in_sha256")):
        whole = (directory / name).read_bytes()
        ref = staged["objects"][name]
        assert getattr(job, field) == _sha(whole) != ref and len(whole) > PART_BYTES
        assert parse_index(objects.get(ref)).sha256 == _sha(whole)
        assert get_large(ref, objects.get) == whole
    assert unpack_state((directory / "start_state").read_bytes())[0]["w"].equal(theta["w"])
    assert staged["objects"]["v0"] == job.v0_sha256 == _sha(pack_state({}))
    assert put["island-job"] == job.body()


def _huge_index(size: int) -> tuple[bytes, list[str]]:
    """Metadata-only index of `size` logical bytes; no part bytes exist or are allocated."""
    count = -(-size // PART_BYTES)
    parts = [f"{i:064x}" for i in range(count)]
    return PartIndexV1(sha256="ab" * 32, size=size, parts=parts).encode(), parts


def test_proof_cap_binds_stored_ref_not_logical_size() -> None:
    raw, parts = _huge_index(MAX_OBJECT_BYTES + 3 * PART_BYTES)  # logical > 2 GiB cap
    key, fetched = _sha(raw), []

    class Objects:
        def get(self, sha: str) -> bytes:
            if sha == key:
                return raw
            fetched.append(sha)
            return b"not-the-part"  # first part fails its content hash

    ref = ArtifactRef(sha256=key, size=len(raw))
    assert ref.size < PART_BYTES < MAX_OBJECT_BYTES
    # Past the cap boundary into streamed part verification; stops at the first bad part.
    with pytest.raises(AdmissionError, match="PROOF_OBJECT_HASH"):
        _verify_ref(Objects(), ref, MAX_OBJECT_BYTES)
    assert fetched == parts[:1]
    # Every part is a stored object too: a cap below PART_BYTES rejects before any part read.
    with pytest.raises(AdmissionError, match="PROOF_OBJECT_LIMIT"):
        _verify_ref(Objects(), ref, PART_BYTES - 1)
    with pytest.raises(AdmissionError, match="PROOF_OBJECT_LIMIT"):
        _verify_ref(Objects(), ref, len(raw) - 1)
    assert fetched == parts[:1]


def test_proof_ref_verifies_genuine_index_and_plain(tmp_path: Path) -> None:
    objects = LocalFSStore(tmp_path)
    key = put_large(BIG, objects.put)
    ref = ArtifactRef(sha256=key, size=len(objects.get(key)))
    assert _verify_ref(objects, ref, MAX_OBJECT_BYTES) == (_sha(BIG), len(BIG))
    small = ArtifactRef(sha256=objects.put(b"leaf"), size=4)
    assert _verify_ref(objects, small, 4) == (_sha(b"leaf"), 4)
    with pytest.raises(AdmissionError, match="PROOF_OBJECT_LIMIT"):
        _verify_ref(objects, small, 3)


def test_whole_sha_maps_stored_ref_for_job_bindings(tmp_path: Path) -> None:
    from hypertrain.challenge.store import ChallengeStore

    store = ChallengeStore.__new__(ChallengeStore)
    store.objects = LocalFSStore(tmp_path)
    key = put_large(BIG, store.objects.put)
    plain = store.objects.put(b"v0")
    assert store._whole_sha_v2(key) == _sha(BIG) != key
    assert store._whole_sha_v2(plain) == plain


def test_shadow_anchor_binds_job_whole_bytes_and_start_refs(tmp_path: Path) -> None:
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
    from hypertrain.challenge.store import ChallengeError, ChallengeStore
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    manifest = fixtures.setup().manifest
    hot = fixtures.HOT.ss58
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    genesis = AnchorCache().genesis(manifest, hot, theta)
    start, ef = pack_state(theta, genesis.state), pack_state(genesis.ef)
    (tmp_path / "start_state").write_bytes(start)
    (tmp_path / "ef_in").write_bytes(ef)
    job = IslandJobV1(
        job_version=1,
        run_id=manifest.run_id(),
        w=0,
        manifest=manifest,
        sample_ids=list(range(manifest.training.batch_samples())),
        global_step0=0,
        start_state_sha256=_sha(start),
        ef_in_sha256=_sha(ef),
        v0_sha256=_sha(pack_state({})),
        object_paths={"start_state": "start_state", "ef_in": "ef_in"},
        deadline=1,
    )
    store = ChallengeStore.__new__(ChallengeStore)
    expected = StartStateV2(
        run_id=job.run_id,
        w=0,
        hotkey=hot,
        theta_hash=state_hash(theta),
        state_object_sha256=put_large(start, _sha),
        opt_state_hash=optimizer_hash(genesis.state),
        ef_object_sha256=put_large(ef, _sha),
        ef_hash=state_hash(genesis.ef),
        parent_anchor_hash=genesis.anchor_hash,
        global_step0=0,
        anchor_verdict_hash=genesis.proof_hash,
    ).digest()
    assert store._shadow_anchor_v2(job, hot, tmp_path) == expected
    bad = job.model_copy(update={"ef_in_sha256": "00" * 32})
    with pytest.raises(ChallengeError, match="start bytes differ"):
        store._shadow_anchor_v2(bad, hot, tmp_path)


def test_publication_cap_binds_stored_objects_not_file_size(tmp_path: Path) -> None:
    from hypertrain.challenge.store import ChallengeError, ChallengeStore

    objects = LocalFSStore(tmp_path / "objects")
    assert len(BIG) > PART_BYTES  # logical file above the per-object cap below
    key = _put_capped(objects, BIG, PART_BYTES)
    assert get_large(key, objects.get) == BIG
    with pytest.raises(AdmissionError, match="SHADOW_PUBLICATION_OBJECT_LIMIT"):
        _put_capped(objects, BIG, PART_BYTES - 1)
    with pytest.raises(AdmissionError, match="SHADOW_PUBLICATION_OBJECT_LIMIT"):
        _put_capped(objects, b"abc", 2)
    store = ChallengeStore.__new__(ChallengeStore)
    store.objects = objects
    published = tmp_path / "state.safetensors"
    published.write_bytes(BIG)
    store._addressed_file_v2(published)  # whole file addressed through its index ref
    published.write_bytes(BIG[:-1] + b"x")
    with pytest.raises(ChallengeError, match="addressed publication changed"):
        store._addressed_file_v2(published)


def test_service_audit_staging_refs_match_whole_files(tmp_path: Path) -> None:
    from hypertrain.data.store import put_file

    objects = LocalFSStore(tmp_path / "objects")
    key = put_large(BIG, objects.put)
    anchor = tmp_path / "state"
    anchor.write_bytes(BIG)
    # Anchor/miner files compare by streamed ref; every stored object reads within PART_BYTES.
    assert put_file(anchor, _sha) == key == put_large(BIG, _sha)
    assert all(
        len(objects.get(k)) <= PART_BYTES for k in [key, *parse_index(objects.get(key)).parts]
    )
    anchor.write_bytes(BIG[:-1] + b"x")
    assert put_file(anchor, _sha) != key


def test_service_audit_header_bound_admits_od_large_header() -> None:
    import json

    from hypertrain.protocol.envelope_v2 import load_json

    # od-large state header size class: many tensor entries, ~104600 bytes total.
    entry = {"dtype": "F32", "shape": [1024, 1024], "data_offsets": [0, 4194304]}
    names = [
        f"{p}/blocks.{i}.attn.w{j}"
        for p in ("theta", "m", "v")
        for i in range(24)
        for j in range(12)
    ]
    header = json.dumps({n: entry for n in names}, separators=(",", ":")).encode()
    assert 65536 < len(header) <= PART_BYTES
    assert len(load_json(header, max_bytes=PART_BYTES)) == len(names)
    with pytest.raises(ValueError):
        load_json(header, max_bytes=65536)
