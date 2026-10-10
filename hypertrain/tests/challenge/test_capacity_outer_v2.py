"""Bounded outer metadata and exactly one actual tiny Torch child; no training."""

from __future__ import annotations

import importlib.util
import json
import os
import struct
import sys
import time
from pathlib import Path

import pytest

from hypertrain.aggregator.capacity_worker import (
    OuterInput,
    OuterRequest,
    collect_result,
)
from hypertrain.data.store import LocalFSStore
from hypertrain.gpu_ops.journal import durable_write
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize

TREE = Path(__file__).parents[2]
spec = importlib.util.spec_from_file_location(
    "outer_capacity_fixture", TREE / "tests/challenge/test_service_capacity_v2.py"
)
assert spec and spec.loader
fixture = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixture
spec.loader.exec_module(fixture)
network = fixture.network
tiny = fixture.tiny


def request_fixture(tiny_run, directory):
    from hypertrain.challenge.trust_v2 import FundedStatus, ReplayEvidence, SettlementStatus
    from hypertrain.protocol.messages_v2 import (
        CommitV2,
        DeltaManifestV2,
        EconomicsPolicyV2,
        RunManifestV2,
    )
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import param_shapes

    store, original = tiny_run
    entries = fixture.roster(4)
    h = "11" * 32
    econ = EconomicsPolicyV2(
        ledger_mode="test",
        G_max_units=100,
        R_collectible_units=100,
        S_min_units=1000,
        beta_ppm=1000000,
        gammaV_units=0,
        s_lower_ppm=1000000,
        q_floor=1000000,
        genesis_allocation_hash=h,
        reward_authority=fixture.service.COORD.ss58,
        round_reward_units=100,
        max_total_issuance=100000,
        shadow_bootstrap_rounds=12,
    )
    body = original.body()
    body["network"]["economics_policy_hash"] = econ.digest()
    manifest = RunManifestV2.model_validate(body)
    staged = LocalFSStore(directory / "objects")
    staged.put(canonicalize(econ.body()))
    staged.put(store.objects.get(manifest.network.aggregation_policy_hash))
    shapes = param_shapes(TrainConfig.from_manifest_v2(manifest).model)
    works = []
    objects = {econ.digest(), manifest.network.aggregation_policy_hash}
    for entry in entries:
        parts = [b"ht-dense-int8-v1", struct.pack("<I", len(shapes))]
        for name, shape in sorted(shapes.items()):
            elements = 1
            for dim in shape:
                elements *= dim
            tensor = (
                struct.pack("<I", 256)
                + struct.pack("<f", 1.0) * ((elements + 255) // 256)
                + bytes(elements)
            )
            parts.extend(
                [
                    struct.pack("<Q", len(name)),
                    name.encode(),
                    struct.pack("<I", len(shape)),
                    b"".join(struct.pack("<Q", dim) for dim in shape),
                    struct.pack("<Q", len(tensor)),
                    tensor,
                ]
            )
        payload = b"".join(parts)
        delta_hash = staged.put(payload)
        c = CommitV2(
            w=0,
            hotkey=entry["hotkey"],
            leaf_scheme="ht-leaf-v1",
            n_leaves=3,
            leaves_root=h,
            metrics_root=h,
            final_theta_hash=h,
            ef_in_hash=h,
            ef_out_hash=h,
            delta_hash=delta_hash,
            delta_bytes=len(payload),
            tokens=4,
        )
        d = DeltaManifestV2(
            w=0,
            hotkey=entry["hotkey"],
            delta_hash=delta_hash,
            uri="object",
            size=len(payload),
            format="ht-dense-int8-v1",
            chunks=[{"off": 0, "len": len(payload), "sha256": delta_hash}],
            grant_hash=h,
            master_acceptance_hash=h,
        )
        replay = ReplayEvidence(
            manifest.run_id(),
            0,
            entry["hotkey"],
            h,
            h,
            h,
            delta_hash,
            h,
            h,
            h,
            h,
            "MATCH",
            "anchored-full",
        )
        funding = FundedStatus(
            manifest.run_id(), entry["hotkey"], entry["admission_id"], "test", 10000, 100, h, True
        )
        settlement = SettlementStatus(manifest.run_id(), 0, entry["hotkey"], False, False, h)
        works.append(
            OuterInput(
                roster=entry,
                commit=c,
                delta_manifest=d,
                replay=replay,
                funding=funding,
                settlement=settlement,
                assignment_hash=h,
                clean_finalizations=12,
            )
        )
        objects.update(
            [
                delta_hash,
                staged.put(canonicalize(c.model_dump(mode="json"))),
                staged.put(canonicalize(d.model_dump(mode="json"))),
            ]
        )
    return OuterRequest(
        manifest=manifest,
        w=0,
        original_roster=entries,
        inputs=works,
        prev_state=None,
        predecessor_tape_hash="0" * 64,
        reference_reward_units=100,
        objects=sorted(objects),
    )


@pytest.mark.parametrize("attack", ["oversize", "duplicate", "roster", "predecessor"])
def test_request_metadata_rejects_without_tensor_allocation(tiny, tmp_path, monkeypatch, attack):
    request = request_fixture(tiny, tmp_path)
    raw = request.body()
    match attack:
        case "oversize":
            data = b"x" * ((1 << 20) + 1)
        case "duplicate":
            raw["inputs"].append(raw["inputs"][0])
            data = canonicalize(raw)
        case "roster":
            raw["original_roster"][0]["admission_id"] = "99" * 32
            data = canonicalize(raw)
        case "predecessor":
            raw["w"] = 1
            data = canonicalize(raw)
    monkeypatch.setattr(
        "hypertrain.trainer.model.init_params", lambda *a: pytest.fail("parent tensor allocation")
    )
    with pytest.raises(ValueError):
        OuterRequest.parse(data)


def test_actual_bounded_outer_child_matches_original_signed_replay(tiny, tmp_path, monkeypatch):
    from hypertrain.aggregator.core import OuterState
    from hypertrain.aggregator.tape_v2 import TapeV2, make_tape, replay_tape
    from hypertrain.miner import island_launch as runtime
    from hypertrain.protocol.envelope_v2 import tape_signing_message
    from hypertrain.protocol.messages_v2 import AggregationPolicyV2

    directory = tmp_path / "outer"
    directory.mkdir()
    request = request_fixture(tiny, directory)
    durable_write(directory / "request.json", canonicalize(request.body()))
    charges = []

    def charge():
        charges.append("outer")
        return True

    # Actual1GiB memory property, only0.5CPU for this tiny authorized qualification child.
    monkeypatch.setattr(runtime, "_CAPACITY_CPU_PERCENT", 50)
    capacity = runtime.CapacityAttempt("77" * 32, "88" * 32, tmp_path / "lock", charge)
    runtime.run_capacity_argv(
        [sys.executable, "-m", "hypertrain.aggregator.capacity_worker", str(directory)],
        directory / "runtime",
        int(time.time()) + 60,
        capacity,
        env=dict(
            os.environ, OMP_NUM_THREADS="1", MKL_NUM_THREADS="1", CUBLAS_WORKSPACE_CONFIG=":4096:8"
        ),
    )
    monkeypatch.setattr(
        "hypertrain.trainer.model.init_params", lambda *a: pytest.fail("parent init")
    )
    monkeypatch.setattr(
        "hypertrain.trainer.compress.decompress", lambda *a, **k: pytest.fail("parent decode")
    )
    with monkeypatch.context() as parent:
        parent.setattr(OuterState, "from_bytes", lambda *a: pytest.fail("parent state load"))
        body, objects = collect_result(directory, request)
    monkeypatch.undo()
    store = LocalFSStore(directory / "objects")
    policy = AggregationPolicyV2.model_validate_json(
        store.get(request.manifest.network.aggregation_policy_hash)
    )
    economics = store.get(request.manifest.network.economics_policy_hash)
    kwargs = dict(
        w=0,
        prev_state=body.prev_state,
        predecessor_tape_hash=request.predecessor_tape_hash,
        inputs=[x.work() for x in request.inputs],
        reference_reward_units=100,
    )
    expected = make_tape(
        store, request.manifest, policy, economics, fixture.service.COORD, **kwargs
    )
    signed = TapeV2(
        body=body,
        signer=fixture.service.COORD.ss58,
        sig=fixture.service.COORD.sign(
            tape_signing_message(body.body(), request.manifest.run_id())
        ).hex(),
    )
    assert signed.body == expected.body
    assert TapeV2.from_bytes(signed.to_bytes()) == signed
    replay_tape(
        store,
        signed,
        request.manifest,
        policy,
        economics,
        signer=fixture.service.COORD.ss58,
        **kwargs,
    )
    assert charges == ["outer"] and sha256_hex(objects[body.out_state]) == body.out_state
    observed = json.loads((directory / "runtime/capacity-observed.json").read_text())
    assert observed["memory.max"] == str(1 << 30) and observed["cpu.max"] == "50000 100000"
    assert (
        "CAPACITY_OUTER_REPLAY_COMPLETE" in (directory / "runtime/capacity-stdout.log").read_text()
    )
    # Parent result hash mismatch rejects before any accepted publication.
    store._path(body.out_state).write_bytes(b"bad")
    with pytest.raises(ValueError, match="hash differs"):
        collect_result(directory, request)


def metadata_result_fixture(tiny_run, directory):
    """Independent reviewer-shaped opaque states; no tensor parser/child/arithmetic."""
    from hypertrain.aggregator.capacity_worker import OuterResult
    from hypertrain.aggregator.tape_v2 import TapeBodyV2, _input
    from hypertrain.aggregator.weighted_v2 import Candidate, allocate_weights
    from hypertrain.protocol.messages_v2 import AggregationPolicyV2, PolicyHashes

    request = request_fixture(tiny_run, directory)
    store = LocalFSStore(directory / "objects")
    previous = store.put(b"opaque prior metadata-control")
    output = store.put(b"opaque output metadata-control")
    records = [
        _input(x.work(), request.manifest, 0)
        for x in sorted(request.inputs, key=lambda x: x.roster.hotkey)
    ]
    policy = AggregationPolicyV2.model_validate_json(
        store.get(request.manifest.network.aggregation_policy_hash)
    )
    allocation = allocate_weights(
        [Candidate(x.roster, x.commit_hash, x.delta_manifest_hash) for x in records], policy
    )
    h = "11" * 32
    body = TapeBodyV2(
        v="ht-tape-v2",
        arithmetic="flat-cclip-cap-v1",
        run_id=request.manifest.run_id(),
        w=0,
        policy_hash=policy.digest(),
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(request.manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        predecessor_tape_hash=request.predecessor_tape_hash,
        prev_state=previous,
        prev_hashes={"theta_hash": h, "outer_state_hash": h, "center_hash": h},
        inputs=records,
        input_root=sha256_hex(canonicalize([x.body() for x in records])),
        excluded=[],
        allocation=allocation,
        preclipped=[],
        copy_suspicion=[],
        out_state=output,
        out_hashes={"theta_hash": h, "outer_state_hash": h, "center_hash": h},
    )
    result = OuterResult(
        request_hash=sha256_hex(canonicalize(request.body())),
        tape_body=body,
        objects=[previous, output],
        replayed=True,
    )
    durable_write(directory / "result.json", canonicalize(result.body()))
    return request, body


@pytest.mark.parametrize(
    "attack",
    [
        "honest",
        "objects",
        "shard",
        "result",
        "object",
        "attempt",
        "ancestor",
        "swap",
        "parent-swap",
    ],
)
def test_collect_rejects_symlink_ancestors_without_tensor_loads(
    tiny, tmp_path, monkeypatch, attack
):
    from hypertrain.aggregator import capacity_worker as worker
    from hypertrain.aggregator.core import OuterState

    parent = tmp_path / "parent"
    directory = parent / "attempt"
    directory.mkdir(parents=True)
    request, body = metadata_result_fixture(tiny, directory)
    events = []

    def fail(*args, **kwargs):
        events.append("tensor")
        pytest.fail("collector loaded tensors")

    monkeypatch.setattr("hypertrain.trainer.model.init_params", fail)
    monkeypatch.setattr("hypertrain.trainer.compress.decompress", fail)
    monkeypatch.setattr(OuterState, "from_bytes", fail)
    if attack == "honest":
        collected, objects = collect_result(directory, request)
        assert collected == body and sha256_hex(objects[body.out_state]) == body.out_state
    else:
        target = {
            "objects": directory / "objects",
            "shard": directory / "objects" / body.out_state[:2],
            "result": directory / "result.json",
            "object": directory / "objects" / body.out_state[:2] / body.out_state,
            "attempt": directory,
            "ancestor": parent,
            "swap": directory / "objects",
            "parent-swap": parent,
        }[attack]
        outside = tmp_path / "outside"
        if attack == "parent-swap":
            original_open = os.open

            def swap_parent(path, flags, *args, **kwargs):
                if path == parent.name and flags & os.O_DIRECTORY:
                    target.rename(outside)
                    target.symlink_to(outside, target_is_directory=True)
                return original_open(path, flags, *args, **kwargs)

            monkeypatch.setattr(os, "open", swap_parent)
        elif attack == "swap":
            original_read = worker._read_at

            def swapped(root, relative, limit):
                data = original_read(root, relative, limit)
                if relative == Path("result.json"):
                    target.rename(outside)
                    target.symlink_to(outside, target_is_directory=True)
                return data

            monkeypatch.setattr(worker, "_read_at", swapped)
        else:
            target.rename(outside)
            target.symlink_to(outside, target_is_directory=outside.is_dir())
        with pytest.raises(ValueError, match="confinement"):
            collect_result(directory, request)
    assert events == []


def test_descriptor_relative_write_and_source_reads_reject_swapped_links(tmp_path):
    from hypertrain.aggregator import capacity_worker as worker

    root = tmp_path / "attempt"
    root.mkdir()
    with worker._directory_fd(root) as fd:
        worker._write_at(fd, Path("objects/aa/state"), b"honest")
        assert worker._read_at(fd, Path("objects/aa/state"), 16) == b"honest"
        outside = tmp_path / "outside"
        (root / "objects").rename(outside)
        (root / "objects").symlink_to(outside, target_is_directory=True)
        with pytest.raises(ValueError, match="confinement"):
            worker._write_at(fd, Path("objects/aa/new"), b"bad")
        assert not (outside / "aa/new").exists()
        with pytest.raises(ValueError, match="confinement"):
            worker._read_at(fd, Path("objects/aa/state"), 16)
    source = tmp_path / "request.json"
    source.symlink_to(outside / "aa/state")
    with pytest.raises(ValueError, match="confinement"):
        worker.bounded_bytes(source, 16)


def test_unsigned_round_lookup_rejects_before_outer_work(tiny, monkeypatch):
    from hypertrain.protocol.envelope import MalformedEnvelope

    store, manifest = tiny
    profile, receipt = fixture.signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), profile, receipt)
    opening = fixture.opening(manifest, 4)
    estimate = store._service_boundary_v2(manifest.run_id(), roster=opening.roster)
    assert estimate is not None
    original_record = store._record_v2

    def unsigned_record(run, kind, id):
        if kind == "round":
            return {"body": {"roster": opening.body()["roster"]}}
        if kind == "round-resource":
            return estimate
        return original_record(run, kind, id)

    def forbidden(*args, **kwargs):
        pytest.fail("unsigned round launched outer work")

    monkeypatch.setattr(store, "_record_v2", unsigned_record)
    monkeypatch.setattr(store, "_capacity_outer_v2", forbidden)
    monkeypatch.setattr("hypertrain.miner.island_launch.run_capacity_argv", forbidden)
    before = store._db.total_changes
    with pytest.raises(MalformedEnvelope, match="invalid versioned envelope"):
        store.aggregate_v2(manifest.run_id(), 0)
    assert store._db.total_changes == before


def test_signed_four_identity_profile_public_gate_denies_before_outer_work(
    tiny, tmp_path, monkeypatch
):
    """Routing evidence only: no eligible MATCH/compute/CAS claim and no child."""
    import sqlite3

    from hypertrain.aggregator.core import OuterState
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.protocol import envelope_v2

    store, manifest = tiny
    profile, receipt = fixture.signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), profile, receipt)
    opening = fixture.opening(manifest, 4)
    raw = canonicalize(
        envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", manifest.run_id(), opening, 100)
    )
    custody = tmp_path / "public-boundary"
    custody.mkdir()
    durable_write(custody / "profile.json", profile)
    durable_write(custody / "receipt.json", receipt)
    durable_write(custody / "opening.json", raw)
    events = []

    def forbidden(*args, **kwargs):
        events.append("work")
        pytest.fail("public denied profile launched/materialized work")

    monkeypatch.setattr("hypertrain.trainer.model.init_params", forbidden)
    monkeypatch.setattr("hypertrain.trainer.compress.decompress", forbidden)
    monkeypatch.setattr(OuterState, "from_bytes", forbidden)
    monkeypatch.setattr("hypertrain.miner.island_launch.run_capacity_argv", forbidden)
    monkeypatch.setattr(store, "_capacity_outer_v2", forbidden)
    before = store._db.total_changes
    with pytest.raises(ChallengeError, match="SERVICE_RUNTIME_NOT_ENFORCED") as denied:
        store.open_round_v2(manifest.run_id(), raw)
    assert store._db.total_changes == before and events == []
    estimate = store._service_boundary_v2(manifest.run_id(), roster=opening.roster)
    assert estimate is not None
    # Exercise aggregate routing against the original roster without inventing an SQL round.
    original_record = store._record_v2

    def roster_record(run, kind, id):
        if kind == "round":
            return envelope_v2.load_json(raw)
        if kind == "round-resource":
            return estimate
        return original_record(run, kind, id)

    monkeypatch.setattr(store, "_record_v2", roster_record)
    monkeypatch.setattr(store, "verified_inputs_v2", forbidden)
    monkeypatch.setattr(store, "_services", forbidden)
    with pytest.raises(ChallengeError, match="SERVICE_RUNTIME_NOT_ENFORCED"):
        store.aggregate_v2(manifest.run_id(), 0)
    assert store._db.total_changes == before and events == []
    assert not store._db.execute(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind IN ('round','applied','service-charge')",
        (manifest.run_id(),),
    ).fetchone()
    with sqlite3.connect(custody / "database.sqlite3") as backup:
        store._db.backup(backup)
    durable_write(
        custody / "outcome.json",
        canonicalize(
            {
                "claim": "signed profile public routing denial only; no genuine eligible round",
                "original_identities": 4,
                "child_launches": 0,
                "tensor_loads": 0,
                "error": str(denied.value),
                "eligible_CAS": "unsupported",
                "stale_CAS": "unverified",
                "aggregate_roster": "read-only metadata oracle; no fabricated SQL round",
            }
        ),
    )
    durable_write(
        custody / "SHA256.json",
        canonicalize(
            {p.name: sha256_hex(p.read_bytes()) for p in sorted(custody.iterdir()) if p.is_file()}
        ),
    )


@pytest.mark.parametrize("fault", ["policy", "signature"])
def test_aggregate_round_authority_rejects_before_runtime_denial(tiny, monkeypatch, fault):
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.envelope import SignatureError

    store, manifest = tiny
    profile, receipt = fixture.signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), profile, receipt)
    opening = fixture.opening(manifest, 4)
    estimate = store._service_boundary_v2(manifest.run_id(), roster=opening.roster)
    assert estimate is not None
    if fault == "policy":
        opening = opening.model_copy(
            update={
                "policy_hashes": opening.policy_hashes.model_copy(
                    update={"aggregation_policy_hash": "99" * 32}
                )
            }
        )
    raw = envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", manifest.run_id(), opening, 100)
    if fault == "signature":
        raw["sig"] = "00" * 64
    original_record = store._record_v2

    def round_record(run, kind, identity):
        if kind == "round":
            return raw
        if kind == "round-resource":
            return estimate
        return original_record(run, kind, identity)

    def forbidden(*args, **kwargs):
        pytest.fail("invalid round reached work intake")

    monkeypatch.setattr(store, "_record_v2", round_record)
    monkeypatch.setattr(store, "verified_inputs_v2", forbidden)
    monkeypatch.setattr(store, "_capacity_outer_v2", forbidden)
    before = store._db.total_changes
    expected = (
        pytest.raises(ChallengeError, match="SERVICE_INPUT_ROUND_AUTHORITY")
        if fault == "policy"
        else pytest.raises(SignatureError, match="signature")
    )
    with expected:
        store.aggregate_v2(manifest.run_id(), 0)
    assert store._db.total_changes == before


@pytest.fixture
def genesis_tiny(tiny, tmp_path):
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages_v2 import RunManifestV2

    source, original = tiny
    body = original.body()
    body["training"]["beacon"]["genesis_time"] = int(time.time())
    manifest = RunManifestV2.model_validate(body)
    objects = LocalFSStore(tmp_path / "genesis-objects")
    for name in (*fixture.PolicyHashes.model_fields, "relay_registry_hash"):
        objects.put(source.objects.get(getattr(manifest.network, name)))
    store = ChallengeStore(
        tmp_path / "genesis-state",
        source.params,
        fixture.service.COORD,
        fixture.service.OWNER.ss58,
        verify_beacon=source.verify_beacon,
        objects=objects,
    )
    store.clock = time.time
    store.push_beacon(fixture.service.fixture_beacon(1))
    store.create_run_v2(
        canonicalize(
            envelope_v2.seal(
                fixture.service.OWNER, "RunManifestV2", manifest.run_id(), manifest, 100
            )
        )
    )
    profile, receipt = fixture.signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), profile, receipt)
    yield store, manifest
    store.close_v2_notifications()
    store._db.close()


def genesis_opening(manifest, *, final=40):
    from hypertrain.protocol import envelope_v2

    body = fixture.opening(manifest, 4).model_copy(update={"d_final": final})
    return canonicalize(
        envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", manifest.run_id(), body, 100)
    )


@pytest.mark.parametrize("attack", ["run", "profile", "source", "cancel", "expired"])
def test_genesis_authority_rejects_before_kernel(genesis_tiny, monkeypatch, attack):
    import threading

    from hypertrain.challenge.store import ChallengeError

    store, manifest = genesis_tiny
    raw = genesis_opening(manifest)
    event = threading.Event()
    if attack == "cancel":
        event.set()
    if attack == "expired":
        monkeypatch.setattr(
            "hypertrain.challenge.store.time.time",
            lambda: manifest.training.beacon.genesis_time + 200,
        )
    if attack in ("profile", "source"):
        record = store._record_v2(manifest.run_id(), "service-admission", "run")
        record["body"]["run_id" if attack == "profile" else "implementation_hash"] = "99" * 32
        with store._tx():
            store._put_record_v2(manifest.run_id(), "service-admission", "run", record)
    monkeypatch.setattr(
        "hypertrain.miner.island_launch.run_capacity_argv",
        lambda *a, **k: pytest.fail("kernel launched"),
    )
    with pytest.raises((ChallengeError, ValueError)):
        store._prepare_capacity_genesis_v2(
            "99" * 32 if attack == "run" else manifest.run_id(), raw, cancel=event
        )
    assert not store._db.execute(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='service-charge'", (manifest.run_id(),)
    ).fetchone()


def test_actual_bounded_genesis_preparation_and_duplicate_debit(
    genesis_tiny, tmp_path, monkeypatch
):
    import sqlite3

    from hypertrain.aggregator.capacity_worker import GenesisRequest, collect_genesis
    from hypertrain.aggregator.core import OuterState
    from hypertrain.auditor.replay import AnchorCache
    from hypertrain.challenge.admission_store import AdmissionError
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages_v2 import RoundOpenV2

    store, manifest = genesis_tiny
    events = []

    def forbidden(*a, **k):
        events.append("tensor")
        pytest.fail("parent tensor initialization/decode")

    monkeypatch.setattr("hypertrain.trainer.model.init_params", forbidden)
    monkeypatch.setattr(AnchorCache, "genesis", forbidden)
    monkeypatch.setattr("hypertrain.trainer.compress.decompress", forbidden)
    monkeypatch.setattr(OuterState, "from_bytes", forbidden)
    before = store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0]
    raw = genesis_opening(manifest)
    result = store._prepare_capacity_genesis_v2(manifest.run_id(), raw)
    assert events == [] and len(result["result"]["starts"]) == 4
    assert store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == before
    assert not store._db.execute(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round'", (manifest.run_id(),)
    ).fetchone()
    charge = store._db.execute(
        "SELECT data FROM records_v2 WHERE run_id=? AND kind='service-charge'", (manifest.run_id(),)
    ).fetchall()
    assert len(charge) == 1
    units = json.loads(charge[0][0])["units"]
    assert not store._service_charge_v2(
        manifest.run_id(), -1, "genesis", 1, kind="outer", units=units
    )
    with pytest.raises(ChallengeError, match="ATTEMPT_RECORDED"):
        store._prepare_capacity_genesis_v2(manifest.run_id(), raw)
    assert (
        store._db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='service-charge'",
            (manifest.run_id(),),
        ).fetchall()
        == charge
    )
    directory = store.state_dir / "capacity-genesis" / manifest.run_id() / "1"
    observed = json.loads((directory / "runtime/capacity-observed.json").read_bytes())
    assert observed["memory.max"] == str(1 << 30) and observed["cpu.max"] == "50000 100000"
    assert (directory / "runtime/capacity-cleaned.json").exists()
    assert "CAPACITY_GENESIS_COMPLETE" in (directory / "runtime/capacity-stdout.log").read_text()
    request = GenesisRequest.model_validate_json((directory / "request.json").read_bytes())
    prepared, _ = collect_genesis(directory, request)
    assert prepared.body() == result["result"]
    opening = RoundOpenV2.model_validate(request.opening["body"]).model_copy(
        update={
            "theta_hash": prepared.starts[0].theta_hash,
            "outer_state_hash": prepared.outer_hashes["outer_state_hash"],
            "center_hash": prepared.outer_hashes["center_hash"],
            "start_state_index_hash": sha256_hex(canonicalize([s.body() for s in prepared.starts])),
        }
    )
    signed = canonicalize(
        envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", manifest.run_id(), opening, 100)
    )
    records = store._db.execute("SELECT * FROM records_v2 ORDER BY kind,id").fetchall()
    paths = sorted(p for p in directory.rglob("*") if p.is_file())
    custody = {str(p): sha256_hex(p.read_bytes()) for p in paths}
    monkeypatch.setattr("hypertrain.miner.island_launch.run_capacity_argv", forbidden)
    monkeypatch.setattr(store, "_capacity_outer_v2", forbidden)

    def assert_no_work():
        assert events == []
        assert store._db.execute("SELECT * FROM records_v2 ORDER BY kind,id").fetchall() == records
        assert store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == before
        assert {str(p): sha256_hex(p.read_bytes()) for p in paths} == custody
        assert not (store.state_dir / "anchors-v2").exists()
        assert not (store.state_dir / "jobs-v2").exists()

    # Real native custody exists; stale beacon and unregistered roster are distinct denials.
    beacon = store.latest_beacon()["round"]
    assert beacon == 1
    store.clock = lambda: (
        manifest.training.beacon.genesis_time + (beacon + 2) * manifest.training.beacon.period
    )
    with pytest.raises(ChallengeError, match="^beacon infrastructure paused$") as stale:
        store.open_round_v2(manifest.run_id(), signed)
    assert stale.value.status == 503
    assert_no_work()
    store.clock = lambda: (
        manifest.training.beacon.genesis_time + (beacon - 1) * manifest.training.beacon.period
    )
    with pytest.raises(AdmissionError, match="^UNKNOWN_HOTKEY$") as unregistered:
        store.open_round_v2(manifest.run_id(), signed)
    assert unregistered.value.code == "UNKNOWN_HOTKEY"
    assert_no_work()
    with sqlite3.connect(tmp_path / "genesis-database.sqlite3") as backup:
        store._db.backup(backup)
    paths = sorted(p for p in directory.rglob("*") if p.is_file())
    durable_write(
        tmp_path / "GENESIS-SHA256.json",
        canonicalize(
            {str(p.relative_to(store.state_dir)): sha256_hex(p.read_bytes()) for p in paths}
        ),
    )


@pytest.fixture
def metadata_genesis(tiny, tmp_path):
    """Deterministic small state oracle; authenticated metadata, no kernel qualification."""
    from hypertrain.aggregator.capacity_worker import GenesisRequest, GenesisResult, collect_genesis
    from hypertrain.aggregator.core import OuterState
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state, tensor_root
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages_v2 import StartStateV2
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    store, manifest = tiny
    profile, receipt = fixture.signed_profile(store, manifest)
    signed = json.loads(genesis_opening(manifest))
    request = GenesisRequest(
        manifest=manifest,
        profile=json.loads(profile),
        manifest_envelope=envelope_v2.seal(
            fixture.service.OWNER, "RunManifestV2", manifest.run_id(), manifest, 100
        ),
        receipt=json.loads(receipt),
        opening=signed,
        accepted=1,
    )
    roster = request.validate_authority()
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    anchor = AnchorCache().genesis(manifest, roster[0].hotkey, theta)
    directory = tmp_path / "metadata-genesis"
    objects = LocalFSStore(directory / "objects")
    state = objects.put(pack_state(theta, anchor.state))
    ef = objects.put(pack_state(anchor.ef))
    starts = [
        StartStateV2(
            run_id=manifest.run_id(),
            w=0,
            hotkey=entry.hotkey,
            theta_hash=state_hash(theta),
            state_object_sha256=state,
            opt_state_hash=optimizer_hash(anchor.state),
            ef_object_sha256=ef,
            ef_hash=state_hash(anchor.ef),
            parent_anchor_hash=anchor.anchor_hash,
            global_step0=0,
            anchor_verdict_hash=anchor.proof_hash,
        )
        for entry in roster
    ]
    outer = OuterState.init({name: value.numpy() for name, value in theta.items()})
    result = GenesisResult(
        request_hash=request.digest(),
        starts=starts,
        state_root=tensor_root(theta, anchor.state),
        outer_state=objects.put(outer.to_bytes()),
        outer_hashes=outer.hashes(),
    )
    durable_write(directory / "request.json", canonicalize(request.body()))
    durable_write(directory / "result.json", canonicalize(result.body()))
    restored, verified = collect_genesis(directory, request)
    assert restored == result and all(sha256_hex(raw) == key for key, raw in verified.items())
    assert not (directory / "runtime").exists()
    return request, result, directory


@pytest.mark.parametrize("fault", ["request", "object", "roster"])
def test_metadata_genesis_original_collector_rejects_tampering(metadata_genesis, fault):
    from hypertrain.aggregator.capacity_worker import collect_genesis

    request, result, directory = metadata_genesis
    if fault == "request":
        request = request.model_copy(update={"accepted": 2})
    elif fault == "object":
        key = result.starts[0].state_object_sha256
        (directory / "objects" / key[:2] / key).write_bytes(b"tampered metadata state")
    else:
        changed = result.model_copy(update={"starts": list(reversed(result.starts))})
        (directory / "result.json").write_bytes(canonicalize(changed.body()))
    with pytest.raises(ValueError, match="capacity genesis.*differs"):
        collect_genesis(directory, request)


@pytest.mark.parametrize(
    "attack",
    [
        "honest-draft",
        "source",
        "run",
        "roster",
        "receipt",
        "state",
        "reservation",
        "charge",
        "job-honest",
        "job-state",
        "job-oversize",
        "job-source",
        "job-roster",
        "job-shape",
        "audit-honest",
        "audit-size",
        "audit-shape",
        "audit-path",
        "audit-descriptor",
        "audit-lease",
        "referee-honest",
        "referee-size",
        "referee-shape",
        "referee-path",
        "referee-signed",
        "rollback-honest",
        "rollback-size",
        "rollback-shape",
        "rollback-path",
        "rollback-signed",
        "preview-honest",
        "preview-size",
        "preview-shape",
        "preview-path",
        "preview-signed",
        "schedule-honest",
        "schedule-signed",
        "schedule-size",
        "schedule-path",
        "lease-honest",
        "lease-signed",
        "lease-size",
        "lease-path",
        "complete-honest",
        "complete-signed",
        "complete-size",
        "complete-path",
        "input-honest",
        "input-size",
        "input-path",
        "input-fifo",
        "input-signature",
        "input-lease",
        "input-root",
        "input-funding",
        "input-origins",
        "aggregate-honest",
        "aggregate-size",
        "aggregate-path",
        "aggregate-fifo",
        "aggregate-signed",
        "final-honest",
        "final-signed",
        "protected-stale-runtime",
    ],
)
def test_prepared_profile_opening_metadata_without_tensor_calls(
    tiny, tmp_path, monkeypatch, attack, metadata_genesis
):
    """Signed hermetic metadata/state oracle; opening never gains kernel approval."""
    from hypertrain.aggregator.core import OuterState
    from hypertrain.auditor.replay import AnchorCache
    from hypertrain.challenge.store import ChallengeError, ChallengeStore
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages_v2 import RoundOpenV2

    old_request, old_result, original = metadata_genesis
    source, _ = tiny
    manifest = old_request.manifest
    objects = LocalFSStore(tmp_path / "draft-objects")
    for name in (*fixture.PolicyHashes.model_fields, "relay_registry_hash"):
        objects.put(source.objects.get(getattr(manifest.network, name)))
    store = ChallengeStore(
        tmp_path / "draft-state",
        source.params,
        fixture.service.COORD,
        fixture.service.OWNER.ss58,
        verify_beacon=source.verify_beacon,
        objects=objects,
    )
    store.clock = lambda: manifest.training.beacon.genesis_time
    store.push_beacon(fixture.service.fixture_beacon(1))
    store.create_run_v2(canonicalize(old_request.manifest_envelope))
    try:
        profile, receipt = fixture.signed_profile(store, manifest)
        store.bootstrap_service_capacity_v2(manifest.run_id(), profile, receipt)
        starts = old_result.starts
        opening = RoundOpenV2.model_validate(old_request.opening["body"]).model_copy(
            update={
                "theta_hash": starts[0].theta_hash,
                "outer_state_hash": old_result.outer_hashes["outer_state_hash"],
                "center_hash": old_result.outer_hashes["center_hash"],
                "start_state_index_hash": sha256_hex(canonicalize([s.body() for s in starts])),
            }
        )
        signed = canonicalize(
            envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", manifest.run_id(), opening, 100)
        )
        request = old_request.model_copy(
            update={
                "profile": json.loads(profile),
                "receipt": json.loads(receipt),
                "opening": json.loads(signed),
            }
        )
        result = old_result.model_copy(update={"request_hash": request.digest()})
        directory = store.state_dir / "capacity-genesis" / manifest.run_id() / "1"
        staged = LocalFSStore(directory / "objects")
        for key in {result.outer_state, starts[0].state_object_sha256, starts[0].ef_object_sha256}:
            payload = (original / "objects" / key[:2] / key).read_bytes()
            assert sha256_hex(payload) == key
            staged.put(payload)
            objects.put(payload)
        estimate = store.require_roster_v2(manifest.run_id(), roster=opening.roster)
        reservation = {
            **estimate,
            "roster": opening.body()["roster"],
            "deadline": opening.d_final,
            "request_hash": request.digest(),
        }
        prepared = {
            "profile_hash": sha256_hex(profile),
            "request_hash": request.digest(),
            "result": result.body(),
        }
        if attack == "source":
            request = old_request.model_copy(
                update={"profile": {**old_request.profile, "implementation_hash": "99" * 32}}
            )
        if attack == "run":
            prepared["result"]["starts"][0]["run_id"] = "99" * 32
        if attack == "reservation":
            reservation["outer_work_units"] += 1
        durable_write(directory / "request.json", canonicalize(request.body()))
        durable_write(directory / "result.json", canonicalize(result.body()))
        with store._tx():
            store._put_record_v2(manifest.run_id(), "genesis-resource", "run", reservation)
            store._put_record_v2(manifest.run_id(), "genesis-prepared", "run", prepared)
        # Metadata reservation oracle; real debit method, no runtime permission.
        assert store._service_charge_v2(
            manifest.run_id(), -1, "genesis", 1, kind="outer", units=estimate["outer_work_units"]
        )
        if attack == "receipt":
            record = store._record_v2(manifest.run_id(), "service-admission", "run")
            record["receipt"]["sig"] = "00" * 64
            with store._tx():
                store._put_record_v2(manifest.run_id(), "service-admission", "run", record)
        if attack == "charge":
            identity = sha256_hex(canonicalize({"w": -1, "operation": "genesis", "attempt": 1}))
            record = store._record_v2(manifest.run_id(), "service-charge", identity)
            record["units"] += 1
            with store._tx():
                store._put_record_v2(manifest.run_id(), "service-charge", identity, record)
        if attack in ("roster", "state"):
            body = opening.body()
            if attack == "roster":
                body["roster"][0]["admission_id"] = "99" * 32
                body["roster_hash"] = sha256_hex(canonicalize(body["roster"]))
            else:
                body["theta_hash"] = "99" * 32
            signed = canonicalize(
                envelope_v2.seal(
                    fixture.service.COORD,
                    "RoundOpenV2",
                    manifest.run_id(),
                    RoundOpenV2.model_validate(body),
                    100,
                )
            )
        events = []

        def forbidden(*a, **k):
            events.append("tensor")
            pytest.fail("opening allocated tensors")

        monkeypatch.setattr("hypertrain.trainer.model.init_params", forbidden)
        monkeypatch.setattr(AnchorCache, "genesis", forbidden)
        monkeypatch.setattr("hypertrain.trainer.compress.decompress", forbidden)
        monkeypatch.setattr(OuterState, "from_bytes", forbidden)
        monkeypatch.setattr("hypertrain.auditor.replay.pack_state", forbidden)
        monkeypatch.setattr("hypertrain.auditor.replay.unpack_state", forbidden)
        if attack == "protected-stale-runtime":
            # Explicit invalid metadata, never a claimed successful kernel observation.
            runtime = directory / "runtime"
            runtime.mkdir()
            for name in ("attempt", "observed", "result", "cleaned", "exec"):
                durable_write(
                    runtime / f"capacity-{name}.json",
                    canonicalize({"deadline": False} if name == "attempt" else {}),
                )
            before = store._db.total_changes
            with pytest.raises(ChallengeError, match="SERVICE_PROTECTED_OPEN_RUNTIME"):
                store.open_round_v2(manifest.run_id(), signed)
            assert store._db.total_changes == before
            assert not (store.state_dir / "anchors-v2").exists()
        elif attack.startswith(("input-", "aggregate-", "final-")):
            integrity_preflight_control(
                store, manifest, opening, starts[0], old_result, tiny, tmp_path, attack, monkeypatch
            )
        elif attack.startswith(("schedule-", "lease-", "complete-")):
            routing_preflight_control(store, manifest, opening, starts[0], attack, monkeypatch)
        elif attack.startswith(("referee-", "rollback-", "preview-")):
            dispute_preflight_control(
                store, manifest, opening, starts[0], old_result, original, attack, monkeypatch
            )
        elif attack.startswith("audit-"):
            audit_preflight_control(
                store, manifest, opening, starts[0], signed, attack, monkeypatch
            )
        elif attack.startswith("job-"):
            # Read-only round/identity oracle: tests job boundary, not eligible publication.
            original_record = store._record_v2
            hotkey = starts[0].hotkey
            start_body = starts[0].body()
            sample_width = 2 if manifest.training.dataset.sample_format.startswith("u16") else 4
            dataset_bytes = bytes(
                (manifest.training.model.seq_len + 1)
                * sample_width
                * manifest.training.dataset.n_samples
            )
            dataset = {
                "samples_hash": objects.put(dataset_bytes),
                "proofs_hash": objects.put(
                    canonicalize([[]] * manifest.training.dataset.n_samples)
                ),
            }
            if attack == "job-state":
                start_body["state_object_sha256"] = "99" * 32
            if attack == "job-oversize":
                target = objects._path(starts[0].state_object_sha256)
                target.write_bytes(b"x" * 65537)
            if attack == "job-source":
                (directory / "request.json").unlink()
                changed = old_request.model_copy(
                    update={"profile": {**old_request.profile, "implementation_hash": "99" * 32}}
                )
                durable_write(directory / "request.json", canonicalize(changed.body()))
            if attack == "job-roster":
                hotkey = fixture.roster(5)[4]["hotkey"]
            if attack == "job-shape":
                raw = objects.get(starts[0].state_object_sha256)
                count = struct.unpack_from("<Q", raw)[0]
                header = json.loads(raw[8 : 8 + count])
                name = next(n for n in header if n.startswith("theta/"))
                header[name]["shape"] = [999999]
                encoded = canonicalize(header)
                malformed = struct.pack("<Q", len(encoded)) + encoded + raw[8 + count :]
                digest = objects.put(malformed)
                staged.put(malformed)
                start_body["state_object_sha256"] = digest
                changed_starts = [
                    s.model_copy(update={"state_object_sha256": digest}) for s in starts
                ]
                changed_result = result.model_copy(update={"starts": changed_starts})
                prepared["result"] = changed_result.body()
                with store._tx():
                    store._put_record_v2(manifest.run_id(), "genesis-prepared", "run", prepared)
                (directory / "result.json").unlink()
                durable_write(directory / "result.json", canonicalize(changed_result.body()))
                opening = opening.model_copy(
                    update={
                        "start_state_index_hash": sha256_hex(
                            canonicalize([s.body() for s in changed_starts])
                        )
                    }
                )
                signed = canonicalize(
                    envelope_v2.seal(
                        fixture.service.COORD, "RoundOpenV2", manifest.run_id(), opening, 100
                    )
                )

            def read_oracle(run, kind, identity):
                if kind == "round":
                    return json.loads(signed)
                if kind == "round-resource":
                    return estimate
                if kind == "start":
                    return start_body
                if kind == "assignment":
                    return {"samples": list(range(manifest.training.batch_samples()))}
                if kind == "dataset":
                    return dataset
                return original_record(run, kind, identity)

            monkeypatch.setattr(store, "_record_v2", read_oracle)
            monkeypatch.setattr(store, "_owner_v2", lambda *a: fixture.service.OWNER.ss58)
            with pytest.raises((ChallengeError, ValueError)) as rejected:
                store.island_job_v2(manifest.run_id(), 0, hotkey)
            if attack == "job-honest":
                assert "SERVICE_RUNTIME_NOT_ENFORCED" in str(rejected.value)
            else:
                assert "SERVICE_RUNTIME_NOT_ENFORCED" not in str(rejected.value)
            assert not (store.state_dir / "jobs-v2").exists()
            assert not store._db.execute(
                "SELECT 1 FROM records_v2 WHERE kind='island-job'"
            ).fetchone()
        elif attack == "honest-draft":
            with pytest.raises(ChallengeError, match="SERVICE_RUNTIME_NOT_ENFORCED"):
                store.open_round_v2(manifest.run_id(), signed)
        else:
            with pytest.raises((ChallengeError, ValueError)) as rejected:
                store.open_round_v2(manifest.run_id(), signed)
            assert "SERVICE_RUNTIME_NOT_ENFORCED" not in str(rejected.value)
        assert events == []
        assert not store._db.execute(
            "SELECT 1 FROM records_v2 WHERE run_id=? "
            "AND kind IN ('round','round-resource','start')",
            (manifest.run_id(),),
        ).fetchone()
        # Source records unchanged; transactional freshness changes roll back on denial.
        assert store._record_v2(manifest.run_id(), "genesis-resource", "run") == reservation
    finally:
        store.close_v2_notifications()
        store._db.close()


def audit_preflight_control(store, manifest, opening, start, signed, attack, monkeypatch):
    """Signed lease/descriptor metadata oracle, no eligible MATCH or execution claim."""
    from hypertrain.auditor.replay import AnchorCache
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages import LeafPreimage, Receipt, f32hex
    from hypertrain.protocol.messages_v2 import AuditChallengeV2, AuditJobV2, CommitV2, IslandJobV1

    hot = next(
        key
        for key in (fixture.Keypair(bytes([i + 1]) * 32) for i in range(4))
        if key.ss58 == start.hotkey
    )
    auditor = fixture.service.AUDITORS[0]
    run = manifest.run_id()
    h = "11" * 32
    state = store.objects.get(start.state_object_sha256)
    if attack == "audit-shape":
        size = struct.unpack_from("<Q", state)[0]
        header = json.loads(state[8 : 8 + size])
        header[next(k for k in header if k.startswith("theta/"))]["shape"] = [999999]
        encoded = canonicalize(header)
        state = struct.pack("<Q", len(encoded)) + encoded + state[8 + size :]
        start = start.model_copy(update={"state_object_sha256": store.objects.put(state)})
    if attack == "audit-size":
        state = b"x" * 65537
        start = start.model_copy(update={"state_object_sha256": store.objects.put(state)})
    ef = store.objects.get(start.ef_object_sha256)
    v0 = struct.pack("<Q", 8) + b"{}      "
    v0_hash = store.objects.put(v0)
    sample_ids = list(range(manifest.training.batch_samples()))
    width = (manifest.training.model.seq_len + 1) * (
        2 if manifest.training.dataset.sample_format.startswith("u16") else 4
    )
    dataset_raw = bytes(width * manifest.training.dataset.n_samples)
    proofs = [[] for _ in range(manifest.training.dataset.n_samples)]
    dataset = {
        "samples_hash": store.objects.put(dataset_raw),
        "proofs_hash": store.objects.put(canonicalize(proofs)),
    }
    leaves = [
        LeafPreimage(
            run_id=run,
            w=0,
            t=t,
            stages=[{"theta": h, "m": h, "v": h}],
            batch_ids_sha256=h,
            rng_ctr=0,
            loss_f32=f32hex(0),
            norm_f32=f32hex(0),
        )
        for t in range(3)
    ]
    commit = CommitV2(
        w=0,
        hotkey=start.hotkey,
        leaf_scheme="ht-leaf-v1",
        n_leaves=3,
        leaves_root=h,
        metrics_root=h,
        final_theta_hash=h,
        ef_in_hash=start.ef_hash,
        ef_out_hash=h,
        delta_hash=h,
        delta_bytes=1,
        tokens=len(sample_ids) * manifest.training.model.seq_len,
    )
    challenge = AuditChallengeV2(
        w=0,
        target=start.hotkey,
        beacon_round=1,
        beacon_sig_sha256=h,
        mode="full",
        segments=[],
        reasons=["final"],
        serve_deadline=40,
        anchor_hash=start.digest(),
        audit_mode="anchored-full",
    )
    job_id, nonce = "aa" * 32, "bb" * 32
    job = AuditJobV2(
        run_id=run,
        job_id=job_id,
        auditor_id=auditor.ss58,
        attempt=1,
        lease_nonce=nonce,
        lease_expires=40,
        absolute_deadline=40,
        reservation_id=h,
        replay_step_budget=4,
        anchor_age=0,
        manifest=manifest,
        challenge_envelope=envelope_v2.seal(
            fixture.service.COORD, "AuditChallengeV2", run, challenge, 100
        ),
        commit_envelope=envelope_v2.seal(hot, "CommitV2", run, commit, 100),
        sample_ids=sample_ids,
        start_state=start,
        preimages=leaves,
        ef_in={"sha256": start.ef_object_sha256, "size": len(ef)},
        v0={"sha256": v0_hash, "size": len(v0)},
        created_beacon=1,
    )
    deadline = manifest.training.beacon.genesis_time + 39 * 3
    inputs = {
        "start_state": state,
        "ef_in": ef,
        "v0": v0,
        "samples": dataset_raw[: len(sample_ids) * width],
        "sample_proofs": canonicalize(proofs[: len(sample_ids)]),
    }
    geometry = IslandJobV1(
        job_version=1,
        run_id=run,
        w=0,
        manifest=manifest,
        sample_ids=sample_ids,
        global_step0=0,
        start_state_sha256=start.state_object_sha256,
        ef_in_sha256=start.ef_object_sha256,
        v0_sha256=v0_hash,
        object_paths={k: k for k in inputs},
        deadline=deadline,
    )
    miner = store.state_dir / "jobs-v2/0" / start.hotkey
    miner.mkdir(parents=True)
    for name, data in inputs.items():
        durable_write(miner / name, data)
    if attack == "audit-path":
        (miner / "ef_in").unlink()
        outside = store.state_dir / "outside-ef"
        durable_write(outside, ef)
        (miner / "ef_in").symlink_to(outside)
    anchor_dir = store.state_dir / "anchors-v2" / start.hotkey / start.parent_anchor_hash
    anchor_dir.mkdir(parents=True)
    anchor = {
        "hotkey": start.hotkey,
        "w": -1,
        "anchor_hash": start.parent_anchor_hash,
        "proof_hash": start.anchor_verdict_hash,
        "ef_hash": start.ef_hash,
        "backend": "genesis",
        "path": str(anchor_dir),
    }
    durable_write(
        anchor_dir / "metadata",
        canonicalize(
            {
                "run_id": run,
                "hotkey": start.hotkey,
                "w": -1,
                "layout_hash": AnchorCache.layout_hash(manifest),
                "anchor_hash": start.parent_anchor_hash,
                "proof_hash": start.anchor_verdict_hash,
                "backend": "genesis",
                "state_sha256": start.state_object_sha256,
                "ef_sha256": start.ef_object_sha256,
            }
        ),
    )
    durable_write(anchor_dir / "state", state)
    durable_write(anchor_dir / "ef", ef)
    target = store.state_dir / "audit-geometry-v2" / job_id / nonce
    descriptor = {
        "job": geometry.body(),
        "audit_job_hash": job.digest(),
        "lease_nonce": nonce,
        "expires": 40,
        "absolute": 40,
        "directory": str(target),
    }
    if attack == "audit-descriptor":
        descriptor["lease_nonce"] = "99" * 32
    digest = store.objects.put(canonicalize(descriptor))
    accepted = {
        "descriptor_hash": digest,
        "receipt": envelope_v2.seal(
            fixture.service.COORD,
            "Receipt",
            run,
            Receipt(w=0, commit_hash=digest, received_round=1),
            100,
        ),
    }
    estimate = store.require_roster_v2(run, roster=opening.roster)
    old_record = store._record_v2

    def read_oracle(r, kind, identity):
        if kind == "round":
            return json.loads(signed)
        if kind == "round-resource":
            return estimate
        if kind == "audit-job":
            return job.body()
        if kind == "start":
            return start.body()
        if kind == "island-job":
            return geometry.body()
        if kind == "audit-geometry":
            return accepted
        if kind == "anchor":
            return anchor
        if kind == "dataset":
            return dataset
        return old_record(r, kind, identity)

    monkeypatch.setattr(store, "_record_v2", read_oracle)
    with store._tx():
        store._db.execute(
            "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,challenge,created,absolute,"
            "state,attempts,auditor,nonce,expires,reservation,step_budget) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                job_id,
                run,
                0,
                start.hotkey,
                "{}",
                1,
                40,
                "running",
                1,
                auditor.ss58,
                nonce,
                39 if attack == "audit-lease" else 40,
                h,
                4,
            ),
        )
    events = []

    def stop(*a, **k):
        events.append("stage-launch")
        pytest.fail("audit staged/launched")

    monkeypatch.setattr(store, "_restore_anchor_v2", stop)
    monkeypatch.setattr("hypertrain.auditor.worker.execute_island_audit", stop)
    monkeypatch.setattr("hypertrain.gpu_ops.journal.durable_write", stop)
    store._role_launch = stop
    receipt = envelope_v2.seal(
        auditor, "Receipt", run, Receipt(w=0, commit_hash=nonce, received_round=1), 100
    )
    expected = {
        "audit-honest": "SERVICE_RUNTIME_NOT_ENFORCED",
        "audit-size": "capacity bytes exceed limit",
        "audit-shape": "SERVICE_AUDIT_STATE_SHAPE",
        "audit-path": "confinement",
        "audit-descriptor": "SERVICE_AUDIT_DESCRIPTOR_BINDING",
        "audit-lease": "SERVICE_AUDIT_LEASE_BINDING",
    }[attack]
    with pytest.raises((ChallengeError, ValueError), match=expected):
        store.execute_audit_v2(run, job_id, canonicalize(receipt))
    assert events == [] and not store._lease_guards_v2
    assert not (store.state_dir / "audit-v2").exists() and not target.exists()


def dispute_preflight_control(
    store, manifest, opening, start, genesis, original, attack, monkeypatch
):
    """Given signed metadata-only dispute/tapes; When preflight; Then exact rejection, heavy0."""
    from hypertrain.aggregator.tape_v2 import TapeBodyV2, TapeInput, TapeV2
    from hypertrain.auditor.island_bisect import RefereeEvidence
    from hypertrain.challenge.disputes_v2 import Contest, Turn, WatchEvent, signed_message
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.envelope_v2 import tape_signing_message
    from hypertrain.protocol.messages import Receipt, ReplayEnv, ReplayVerdict, Rollback
    from hypertrain.protocol.messages_v2 import IslandJobV1, ResolutionV2

    run, h, dispute_id = manifest.run_id(), "11" * 32, "bc" * 32
    mode, fault = attack.split("-")
    hot, auditor, referee = start.hotkey, fixture.service.AUDITORS[0], fixture.service.REFEREE
    verdict = ReplayVerdict(
        challenge_hash=h,
        result="MISMATCH",
        first_bad_leaf=0,
        recomputed_leaves_root=h,
        replay_env=ReplayEnv(
            image_digest=manifest.training.reference_spec.image_digest,
            driver=manifest.training.reference_spec.driver_allowlist[0],
            gpu_uuid_sha256=h,
            sm_count=max(1, manifest.training.reference_spec.sm_count),
        ),
    )
    claim = envelope_v2.seal(auditor, "ReplayVerdict", run, verdict, 100)
    contest = Contest(
        verdict_hash=body_digest(claim["body"]),
        challenge_hash=h,
        w=0,
        miner=hot,
        auditor=auditor.ss58,
        referee=referee.ss58,
        step_span=2,
        layer_span=1,
        op_span=1,
    )
    turn = Turn(
        run_id=run,
        dispute_id=dispute_id,
        contest=contest,
        seq=0,
        level="step",
        ctx=[],
        interval=(0, 2),
        expected_party=hot,
        transcript_hash=h,
        turn_deadline=20,
        absolute_deadline=40,
        lock_id=h,
    )
    evidence = RefereeEvidence(
        dispute_id=dispute_id,
        transcript_hash=h,
        reason="FRAUD",
        loser=hot,
        predecessor_hash=h,
        inputs_hash=h,
        output_hash=h,
        op_spec="metadata-only-oracle",
    )
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=h,
        reason="FRAUD",
        loser=hot,
        evidence_hash=evidence.digest(),
    )
    if mode != "referee":
        turn = turn.model_copy(update={"resolution": resolution, "resolved_beacon": 1})
    event = WatchEvent(
        run_id=run,
        cursor=1,
        party=hot,
        kind="turn",
        turn=turn,
        checkpoint=None,
        issued_beacon=1,
        signer=fixture.service.COORD.ss58,
        sig="",
    )
    event = event.model_copy(
        update={"sig": fixture.service.COORD.sign(signed_message(event)).hex()}
    )
    with store._tx():
        store._db.execute(
            "INSERT INTO disputes_v2 VALUES(?,?,?,?,?)",
            (dispute_id, contest.verdict_hash, hot, turn.model_dump_json(), "{}"),
        )
        store._db.execute(
            "INSERT INTO dispute_events_v2(cursor,party,event) VALUES(?,?,?)",
            (1, hot, event.model_dump_json()),
        )
        store._db.execute(
            "INSERT INTO dispute_evidence_v2 VALUES(?,?)",
            (evidence.digest(), evidence.model_dump_json()),
        )
    original_record = store._record_v2
    state = store.objects.get(start.state_object_sha256)
    outer = (original / "objects" / genesis.outer_state[:2] / genesis.outer_state).read_bytes()
    if fault == "shape":
        raw = state if mode == "referee" else outer
        size = struct.unpack_from("<Q", raw)[0]
        header = json.loads(raw[8 : 8 + size])
        header[next(k for k in header if k.startswith("theta/"))]["shape"] = [999999]
        encoded = canonicalize(header)
        raw = struct.pack("<Q", len(encoded)) + encoded + raw[8 + size :]
        if mode == "referee":
            state = raw
        else:
            outer = raw
    if fault == "size":
        if mode == "referee":
            state = b"x" * 65537
        else:
            outer = b"x" * 65537
    state_hash = store.objects.put(state)
    outer_hash = store.objects.put(outer)
    ef = store.objects.get(start.ef_object_sha256)
    v0 = struct.pack("<Q", 8) + b"{}      "
    v0_hash = store.objects.put(v0)
    sample_ids = list(range(manifest.training.batch_samples()))
    width = (manifest.training.model.seq_len + 1) * (
        2 if manifest.training.dataset.sample_format.startswith("u16") else 4
    )
    samples = bytes(width * manifest.training.dataset.n_samples)
    proofs = canonicalize([[]] * manifest.training.dataset.n_samples)
    dataset = {"samples_hash": store.objects.put(samples), "proofs_hash": store.objects.put(proofs)}
    owned = store.state_dir / "audit-geometry-v2" / ("aa" * 32) / ("bb" * 32)
    owned.mkdir(parents=True)
    (owned / "published/rank-0").mkdir(parents=True)
    durable_write(owned / "published/rank-0/trace.json", canonicalize([]))
    inputs = {
        "start_state": state,
        "ef_in": ef,
        "v0": v0,
        "samples": samples[: width * len(sample_ids)],
        "sample_proofs": canonicalize([[]] * len(sample_ids)),
    }
    for name, data in inputs.items():
        durable_write(owned / name, data)
    if fault == "path" and mode == "referee":
        (owned / "ef_in").unlink()
        (owned / "ef_in").symlink_to(store.objects._path(start.ef_object_sha256))
    if fault == "path" and mode != "referee":
        path = store.objects._path(outer_hash)
        path.unlink()
        outside = store.state_dir / "outside-outer"
        durable_write(outside, outer)
        path.symlink_to(outside)
    job = IslandJobV1(
        job_version=1,
        run_id=run,
        w=0,
        manifest=manifest,
        sample_ids=sample_ids,
        global_step0=0,
        start_state_sha256=state_hash,
        ef_in_sha256=start.ef_object_sha256,
        v0_sha256=v0_hash,
        object_paths={name: name for name in inputs},
        deadline=manifest.training.beacon.genesis_time + 39 * 3,
    )
    geometry = {"job": job.body(), "directory": str(owned / "published")}
    descriptor = {
        "job": job.body(),
        "historical_geometry_hash": body_digest(geometry),
        "dispute_id": dispute_id,
        "transcript_hash": h,
        "seq": 0,
        "referee": referee.ss58,
        "absolute_deadline": 40,
        "backend": store._backend_v2(manifest),
    }
    descriptor_hash = store.objects.put(canonicalize(descriptor))
    current = {
        "descriptor_hash": descriptor_hash,
        "receipt": envelope_v2.seal(
            fixture.service.COORD,
            "Receipt",
            run,
            Receipt(w=0, commit_hash=descriptor_hash, received_round=1),
            100,
        ),
    }
    if fault == "signed" and mode == "referee":
        current["receipt"]["sig"] = "00" * 64
    signed_resolution = envelope_v2.seal(referee, "ResolutionV2", run, resolution, 100)
    estimate = store.require_roster_v2(run, roster=opening.roster)
    applied, tapes = {}, {}
    predecessor = "00" * 32
    for w in (0, 1):
        entries = [
            TapeInput(
                roster=r,
                commit_hash=h,
                delta_manifest_hash=h,
                assignment_hash=h,
                anchor_hash=h,
                verdict_hash=h,
                lock_receipt_hash=h,
                settlement_hash=h,
            )
            for r in opening.roster
        ]
        allocation = {
            "policy_hash": manifest.network.aggregation_policy_hash,
            "entries": [
                {
                    "hotkey": r.hotkey,
                    "admission_id": r.admission_id,
                    "probation": False,
                    "weight_units": 4194304,
                    "commit_hash": h,
                    "delta_manifest_hash": h,
                }
                for r in sorted(opening.roster, key=lambda r: r.hotkey.encode())
            ],
        }
        body = TapeBodyV2(
            v="ht-tape-v2",
            arithmetic="flat-cclip-cap-v1",
            run_id=run,
            w=w,
            policy_hash=manifest.network.aggregation_policy_hash,
            policy_hashes=opening.policy_hashes,
            predecessor_tape_hash=predecessor,
            prev_state=outer_hash,
            prev_hashes=genesis.outer_hashes,
            inputs=entries,
            input_root=h,
            excluded=[],
            allocation=allocation,
            preclipped=[],
            copy_suspicion=[],
            out_state=outer_hash,
            out_hashes=genesis.outer_hashes,
        )
        tape = TapeV2(
            body=body,
            signer=fixture.service.COORD.ss58,
            sig=fixture.service.COORD.sign(tape_signing_message(body.body(), run)).hex(),
        )
        if fault == "signed" and mode != "referee" and w == 0:
            tape = tape.model_copy(update={"sig": "00" * 64})
        digest = store.objects.put(tape.to_bytes())
        applied[str(w)] = {
            "tape_hash": digest,
            "prev_state": outer_hash,
            "predecessor_tape_hash": predecessor,
            "out_state": outer_hash,
            "theta_hash": genesis.outer_hashes["theta_hash"],
        }
        tapes[str(w)] = tape
        predecessor = digest

    def read_oracle(r, kind, identity):
        if kind == "round":
            body = opening.model_copy(update={"w": int(identity)})
            return envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", run, body, 100)
        if kind == "round-resource":
            return estimate
        if kind == "verdict":
            return claim
        if kind == "trace-geometry":
            return geometry
        if kind == "start":
            return start.model_copy(update={"state_object_sha256": state_hash}).body()
        if kind == "anchor":
            return {
                "hotkey": hot,
                "w": -1,
                "anchor_hash": start.parent_anchor_hash,
                "proof_hash": start.anchor_verdict_hash,
                "backend": "genesis",
            }
        if kind == "referee-job":
            return current
        if kind == "referee-artifacts":
            return {"descriptor_hash": descriptor_hash}
        if kind == "resolution":
            return signed_resolution
        if kind == "applied":
            return applied[identity]
        if kind == "dataset":
            return dataset
        return original_record(r, kind, identity)

    monkeypatch.setattr(store, "_record_v2", read_oracle)
    events = []

    def forbidden(*a, **k):
        events.append("heavy")
        pytest.fail("profile dispute restored/loaded/executed/published")

    monkeypatch.setattr(store, "_restore_anchor_v2", forbidden)
    monkeypatch.setattr(store, "_rollback_exclusions_v2", forbidden)
    monkeypatch.setattr(store, "_put_record_v2", forbidden)
    monkeypatch.setattr("hypertrain.auditor.island_bisect.IslandParty.published", forbidden)
    monkeypatch.setattr("hypertrain.miner.island_launch.launch_island", forbidden)
    monkeypatch.setattr("hypertrain.aggregator.rollback_v2.execute_repair_context", forbidden)
    store._role_launch = forbidden
    rollback = Rollback(
        w=0,
        excluded=[hot],
        old_theta_hash_w2=genesis.outer_hashes["theta_hash"],
        new_theta_hash_w2=h,
        new_outer_state_hash=h,
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=[resolution.digest()],
    )
    signed = envelope_v2.seal(fixture.service.COORD, "Rollback", run, rollback, 100)
    expected = (
        "SERVICE_RUNTIME_NOT_ENFORCED"
        if fault == "honest"
        else "capacity bytes exceed limit"
        if fault == "size"
        else "SERVICE_REFEREE_STATE_SHAPE"
        if fault == "shape" and mode == "referee"
        else "SERVICE_ROLLBACK_STATE_SHAPE"
        if fault == "shape"
        else "confinement"
        if fault == "path"
        else "signature"
        if mode == "referee"
        else "SERVICE_ROLLBACK_TAPE_AUTHORITY"
    )
    with pytest.raises((ChallengeError, ValueError), match=expected):
        if mode == "referee":
            store.referee_v2(run, dispute_id)
        elif mode == "preview":
            store.rollback_preview_v2(run, canonicalize(signed))
        else:
            store.rollback_v2(run, canonicalize(signed))
    assert events == [] and not store._lease_guards_v2
    assert not (store.state_dir / "referee-v2").exists()
    assert not store._db.execute(
        "SELECT 1 FROM records_v2 WHERE kind IN "
        "('repair-reservation','rollback','repair-preview','referee-job','referee-artifacts')"
    ).fetchone()


def routing_preflight_control(store, manifest, opening, start, attack, monkeypatch):
    """Given current signed metadata; When routing; Then target rejection and no write."""
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.protocol.messages import LeafPreimage, Receipt, ReplayEnv, ReplayVerdict, f32hex
    from hypertrain.protocol.messages_v2 import AuditChallengeV2, AuditJobV2, CommitV2, IslandJobV1

    run, h = manifest.run_id(), "11" * 32
    mode, fault = attack.split("-")
    auditor = fixture.service.AUDITORS[0]
    if mode == "schedule":
        store.push_beacon(fixture.service.fixture_beacon(3))
        store.clock = lambda: manifest.training.beacon.genesis_time + 6
    keys = {
        fixture.Keypair(bytes([i + 1]) * 32).ss58: fixture.Keypair(bytes([i + 1]) * 32)
        for i in range(4)
    }
    leaves = [
        LeafPreimage(
            run_id=run,
            w=0,
            t=t,
            stages=[{"theta": h, "m": h, "v": h}],
            batch_ids_sha256=h,
            rng_ctr=0,
            loss_f32=f32hex(0),
            norm_f32=f32hex(0),
        )
        for t in range(3)
    ]
    leaves_raw = canonicalize([p.model_dump(mode="json") for p in leaves])
    if mode == "lease" and fault == "size":
        leaves_raw = b"x" * 65537
    leaves_hash = store.objects.put(leaves_raw)
    root = MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex()
    commits = {}
    for entry in opening.roster:
        c = CommitV2(
            w=0,
            hotkey=entry.hotkey,
            leaf_scheme="ht-leaf-v1",
            n_leaves=3,
            leaves_root=root,
            metrics_root=h,
            final_theta_hash=h,
            ef_in_hash=start.ef_hash,
            ef_out_hash=h,
            delta_hash=h,
            delta_bytes=1,
            tokens=manifest.training.batch_samples() * manifest.training.model.seq_len,
        )
        signed_commit = envelope_v2.seal(keys[entry.hotkey], "CommitV2", run, c, 100)
        if fault == "signed" and mode == "schedule" and entry.hotkey == start.hotkey:
            signed_commit["sig"] = "00" * 64
        commits[entry.hotkey] = signed_commit
        if mode == "schedule":
            # Real metadata intake creates original receipt. No ACTIVE/PASS/MATCH fabrication.
            original_record = store._record_v2
            estimate = store.require_roster_v2(run, roster=opening.roster)

            def commit_records(
                r, kind, identity, estimate=estimate, entry=entry, original_record=original_record
            ):
                if kind == "round":
                    return envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", run, opening, 100)
                if kind == "round-resource":
                    return estimate
                if kind == "assignment":
                    return {"samples": [0, 1]}
                if kind == "accept":
                    return {"metadata_oracle": True}
                if kind == "start":
                    return start.model_copy(update={"hotkey": entry.hotkey}).body()
                return original_record(r, kind, identity)

            with monkeypatch.context() as setup:
                setup.setattr(store, "_record_v2", commit_records)
                setup.setattr(store, "_owner_v2", lambda *a: fixture.service.OWNER.ss58)
                if not (fault == "signed" and entry.hotkey == start.hotkey):
                    store.training_v2(run, "commit", canonicalize(signed_commit))
    challenge = AuditChallengeV2(
        w=0,
        target=start.hotkey,
        beacon_round=1,
        beacon_sig_sha256=h,
        mode="full",
        segments=[],
        reasons=["final"],
        serve_deadline=40,
        anchor_hash=start.digest(),
        audit_mode="anchored-full",
    )
    signed_challenge = envelope_v2.seal(
        fixture.service.COORD, "AuditChallengeV2", run, challenge, 100
    )
    if mode == "lease" and fault == "signed":
        signed_challenge["sig"] = "00" * 64
    job_id, nonce = "aa" * 32, "bb" * 32
    job_commit = commits[start.hotkey]
    if mode == "schedule" and fault == "signed":
        job_commit = envelope_v2.seal(keys[start.hotkey], "CommitV2", run, job_commit["body"], 100)
    v0 = struct.pack("<Q", 8) + b"{}      "
    v0_hash = store.objects.put(v0)
    ef = store.objects.get(start.ef_object_sha256)
    job = AuditJobV2(
        run_id=run,
        job_id=job_id,
        auditor_id=auditor.ss58,
        attempt=1,
        lease_nonce=nonce,
        lease_expires=40,
        absolute_deadline=40,
        reservation_id=h,
        replay_step_budget=4,
        anchor_age=0,
        manifest=manifest,
        challenge_envelope=envelope_v2.seal(
            fixture.service.COORD, "AuditChallengeV2", run, challenge, 100
        ),
        commit_envelope=job_commit,
        sample_ids=[0, 1],
        start_state=start,
        preimages=leaves,
        ef_in={"sha256": start.ef_object_sha256, "size": len(ef)},
        v0={"sha256": v0_hash, "size": len(v0)},
        created_beacon=1,
    )
    geometry = IslandJobV1(
        job_version=1,
        run_id=run,
        w=0,
        manifest=manifest,
        sample_ids=[0, 1],
        global_step0=0,
        start_state_sha256=start.state_object_sha256,
        ef_in_sha256=start.ef_object_sha256,
        v0_sha256=v0_hash,
        object_paths={
            name: name for name in ("start_state", "ef_in", "v0", "samples", "sample_proofs")
        },
        deadline=manifest.training.beacon.genesis_time + 39 * 3,
    )
    directory = store.state_dir / "audit-geometry-v2" / job_id / nonce
    (directory / "published/rank-0").mkdir(parents=True)
    trace = canonicalize(
        [
            {
                "rank": 0,
                "step": 0,
                "microbatch": 0,
                "layer": 0,
                "op": "metadata-oracle",
                "shape": [1],
                "dtype": "torch.float32",
                "sha256": h,
            }
        ]
    )
    trace_path = directory / "published/rank-0/trace.json"
    durable_write(trace_path, trace)
    descriptor = {
        "job": geometry.body(),
        "audit_job_hash": job.digest(),
        "lease_nonce": nonce,
        "expires": 40,
        "absolute": 40,
        "directory": str(directory),
    }
    descriptor_raw = (
        b"x" * ((1 << 20) + 1)
        if mode == "complete" and fault == "size"
        else canonicalize(descriptor)
    )
    descriptor_hash = store.objects.put(descriptor_raw)
    accepted = {
        "descriptor_hash": descriptor_hash,
        "receipt": envelope_v2.seal(
            fixture.service.COORD,
            "Receipt",
            run,
            Receipt(w=0, commit_hash=descriptor_hash, received_round=1),
            100,
        ),
    }
    if mode == "complete" and fault == "signed":
        accepted["receipt"]["sig"] = "00" * 64
    verdict = ReplayVerdict(
        challenge_hash=body_digest(job.challenge_envelope["body"]),
        result="MISMATCH",
        first_bad_leaf=0,
        recomputed_leaves_root=h,
        replay_env=ReplayEnv(
            image_digest=manifest.training.reference_spec.image_digest,
            driver=manifest.training.reference_spec.driver_allowlist[0],
            gpu_uuid_sha256=h,
            sm_count=max(1, manifest.training.reference_spec.sm_count),
        ),
    )
    if fault == "path":
        path = (
            store.objects._path(manifest.network.audit_policy_hash)
            if mode == "schedule"
            else store.objects._path(leaves_hash)
            if mode == "lease"
            else trace_path
        )
        data = path.read_bytes()
        path.unlink()
        outside = store.state_dir / "outside-routing"
        durable_write(outside, data)
        path.symlink_to(outside)
    if mode == "schedule" and fault == "size":
        store.objects._path(manifest.network.audit_policy_hash).write_bytes(b"x" * 65537)
    estimate = store.require_roster_v2(run, roster=opening.roster)
    old_record = store._record_v2

    def read_oracle(r, kind, identity):
        if kind == "round":
            return envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", run, opening, 100)
        if kind == "round-resource":
            return estimate
        if kind == "commit":
            return commits[identity.split(":", 1)[1]]
        if kind == "start":
            return start.model_copy(update={"hotkey": identity.split(":", 1)[1]}).body()
        if kind == "leaves":
            return {"object": leaves_hash}
        if kind == "audit-job":
            return job.body()
        if kind == "executed-verdict":
            return verdict.model_dump(mode="json")
        if kind == "island-job":
            return geometry.body()
        if kind == "audit-geometry":
            return accepted
        return old_record(r, kind, identity)

    monkeypatch.setattr(store, "_record_v2", read_oracle)
    if mode != "schedule":
        from types import SimpleNamespace

        # Read-only metadata lease oracle. No fabricated SQL lease or eligible MATCH.
        connection = store._db
        lease_row = dict(
            id=job_id,
            run_id=run,
            w=0,
            hotkey=start.hotkey,
            challenge=json.dumps(signed_challenge),
            created=1,
            absolute=40,
            state="queued" if mode == "lease" else "running",
            attempts=0 if mode == "lease" else 1,
            auditor=auditor.ss58,
            nonce=nonce,
            expires=40,
            reservation=h,
            step_budget=4,
        )

        class LeaseReadOracle:
            def __getattr__(self, name):
                return getattr(connection, name)

            def execute(self, sql, parameters=()):
                if sql.startswith("SELECT * FROM audit_leases_v2 WHERE"):
                    assert parameters == ((run,) if mode == "lease" else (job_id, run))
                    return SimpleNamespace(fetchone=lambda: lease_row)
                return connection.execute(sql, parameters)

        monkeypatch.setattr(store, "_db", LeaseReadOracle())
    events = []

    def forbidden(*a, **k):
        events.append("heavy-write")
        pytest.fail("routing crossed denied boundary")

    monkeypatch.setattr(store, "_put_record_v2", forbidden)
    monkeypatch.setattr(store, "_authenticated_v2", forbidden)
    monkeypatch.setattr(store, "_fresh_v2", forbidden)
    monkeypatch.setattr(store, "_expire_leases_v2", forbidden)
    monkeypatch.setattr("hypertrain.auditor.island_bisect.IslandParty.published", forbidden)
    before = store._db.total_changes
    expected = (
        "SERVICE_RUNTIME_NOT_ENFORCED"
        if fault == "honest"
        else "capacity bytes exceed limit"
        if fault == "size"
        else "confinement"
        if fault == "path"
        else "SERVICE_COMPLETE_DESCRIPTOR_SIGNATURE"
        if mode == "complete"
        else "signature"
    )
    with pytest.raises((ChallengeError, ValueError), match=expected):
        if mode == "schedule":
            store.schedule_audits_v2(run, 0)
        elif mode == "lease":
            store.lease_v2(
                run,
                canonicalize(
                    envelope_v2.seal(
                        auditor,
                        "Receipt",
                        run,
                        Receipt(w=0, commit_hash=nonce, received_round=1),
                        100,
                    )
                ),
            )
        else:
            store.complete_audit_v2(
                run,
                job_id,
                canonicalize(envelope_v2.seal(auditor, "ReplayVerdict", run, verdict, 100)),
            )
    assert events == [] and store._db.total_changes == before


def integrity_preflight_control(
    store, manifest, opening, start, genesis, tiny, tmp_path, attack, monkeypatch
):
    """Signed read-only integrity oracle; no SQL eligibility, funding success or CAS proof."""
    from types import SimpleNamespace

    from hypertrain.aggregator.tape_v2 import TapeBodyV2, TapeInput, TapeV2
    from hypertrain.challenge.admission import AdmissionStatus
    from hypertrain.challenge.admission_store import AdmissionRecord
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.challenge.trust_v2 import FundedStatus, ReplayEvidence
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.envelope_v2 import tape_signing_message
    from hypertrain.protocol.messages import (
        Finalize,
        LeafPreimage,
        Receipt,
        ReplayEnv,
        ReplayVerdict,
        f32hex,
    )
    from hypertrain.protocol.messages_v2 import (
        AuditChallengeV2,
        AuditJobV2,
        CommitV2,
        DeltaManifestV2,
        IslandJobV1,
    )

    mode, fault = attack.split("-")
    run, h = manifest.run_id(), "11" * 32
    connection = store._db
    original_record = store._record_v2
    reference = request_fixture(tiny, tmp_path / "codec-input")
    codec_store = LocalFSStore(tmp_path / "codec-input/objects")
    payload = codec_store.get(reference.inputs[0].commit.delta_hash)
    if mode == "input" and fault == "size":
        payload = b"x" * 65537
    delta_hash = store.objects.put(payload)
    auditor = fixture.service.AUDITORS[0]
    keys = {
        fixture.Keypair(bytes([i + 1]) * 32).ss58: fixture.Keypair(bytes([i + 1]) * 32)
        for i in range(4)
    }
    intake, leases, jobs, records, origins = {}, {}, {}, {}, []
    ef = store.objects.get(start.ef_object_sha256)
    v0 = struct.pack("<Q", 8) + b"{}      "
    v0_hash = store.objects.put(v0)
    leaves = [
        LeafPreimage(
            run_id=run,
            w=0,
            t=t,
            stages=[{"theta": h, "m": h, "v": h}],
            batch_ids_sha256=h,
            rng_ctr=0,
            loss_f32=f32hex(0),
            norm_f32=f32hex(0),
        )
        for t in range(3)
    ]
    descriptor_records = {}
    statuses = {}
    for index, entry in enumerate(opening.roster):
        hot = entry.hotkey
        original_start = start.model_copy(update={"hotkey": hot})
        identity = "0:" + hot
        commit = CommitV2(
            w=0,
            hotkey=hot,
            leaf_scheme="ht-leaf-v1",
            n_leaves=3,
            leaves_root=h,
            metrics_root=h,
            final_theta_hash=h,
            ef_in_hash=start.ef_hash,
            ef_out_hash=h,
            delta_hash=delta_hash,
            delta_bytes=len(payload),
            tokens=4,
        )
        delta = DeltaManifestV2(
            w=0,
            hotkey=hot,
            delta_hash=delta_hash,
            uri="object",
            size=len(payload),
            format="ht-dense-int8-v1",
            chunks=[{"off": 0, "len": len(payload), "sha256": delta_hash}],
            grant_hash=h,
            master_acceptance_hash=h,
        )
        commit_env = envelope_v2.seal(keys[hot], "CommitV2", run, commit, 100)
        delta_env = envelope_v2.seal(keys[hot], "DeltaManifestV2", run, delta, 100)
        records[("commit", identity)] = commit_env
        records[("delta", identity)] = delta_env
        for kind, typed, wire in (
            ("CommitV2", commit, commit_env),
            ("DeltaManifestV2", delta, delta_env),
        ):
            key = json.dumps(
                list(envelope_v2.replay_key(kind, run, hot, typed)),
                sort_keys=True,
                separators=(",", ":"),
            )
            digest = body_digest(wire["body"])
            intake[key] = {
                "digest": digest,
                "envelope": canonicalize(wire),
                "accepted_beacon": 1,
                "receipt": json.dumps(
                    envelope_v2.seal(
                        fixture.service.COORD,
                        "Receipt",
                        run,
                        Receipt(w=0, commit_hash=digest, received_round=1),
                        100,
                    )
                ),
            }
        job_id = f"{index + 1:064x}"
        nonce = f"{index + 8:064x}"
        challenge = AuditChallengeV2(
            w=0,
            target=hot,
            beacon_round=1,
            beacon_sig_sha256=h,
            mode="full",
            segments=[],
            reasons=["final"],
            serve_deadline=40,
            anchor_hash=original_start.digest(),
            audit_mode="anchored-full",
        )
        job = AuditJobV2(
            run_id=run,
            job_id=job_id,
            auditor_id=auditor.ss58,
            attempt=1,
            lease_nonce=nonce,
            lease_expires=40,
            absolute_deadline=40,
            reservation_id=h,
            replay_step_budget=4,
            anchor_age=0,
            manifest=manifest,
            challenge_envelope=envelope_v2.seal(
                fixture.service.COORD, "AuditChallengeV2", run, challenge, 100
            ),
            commit_envelope=commit_env,
            sample_ids=[0, 1],
            start_state=original_start,
            preimages=leaves,
            ef_in={"sha256": start.ef_object_sha256, "size": len(ef)},
            v0={"sha256": v0_hash, "size": len(v0)},
            created_beacon=1,
        )
        jobs[job_id] = job
        verdict = ReplayVerdict(
            challenge_hash=body_digest(job.challenge_envelope["body"]),
            result="MATCH",
            first_bad_leaf=None,
            recomputed_leaves_root="99" * 32 if fault == "root" and index == 0 else h,
            replay_env=ReplayEnv(
                image_digest=manifest.training.reference_spec.image_digest,
                driver=manifest.training.reference_spec.driver_allowlist[0],
                gpu_uuid_sha256=h,
                sm_count=max(1, manifest.training.reference_spec.sm_count),
            ),
        )
        verdict_env = envelope_v2.seal(auditor, "ReplayVerdict", run, verdict, 100)
        if fault == "signature" and index == 0:
            verdict_env["sig"] = "00" * 64
        records[("verdict", identity)] = verdict_env
        records[("executed-verdict", job_id)] = verdict.model_dump(mode="json")
        replay = ReplayEvidence(
            run,
            0,
            hot,
            h,
            original_start.digest(),
            h,
            delta_hash,
            h,
            start.ef_hash,
            h,
            body_digest(verdict_env["body"]),
            "MATCH",
            "anchored-full",
        )
        from dataclasses import asdict

        records[("replay", identity)] = asdict(replay)
        records[("assignment", identity)] = {"assignment_hash": h}
        leases[hot] = {
            "id": job_id,
            "state": "completed",
            "result": "MATCH",
            "auditor": auditor.ss58,
            "nonce": nonce,
            "expires": 39 if fault == "lease" and index == 0 else 40,
            "absolute": 40,
        }
        geometry = IslandJobV1(
            job_version=1,
            run_id=run,
            w=0,
            manifest=manifest,
            sample_ids=[0, 1],
            global_step0=0,
            start_state_sha256=start.state_object_sha256,
            ef_in_sha256=start.ef_object_sha256,
            v0_sha256=v0_hash,
            object_paths={k: k for k in ("start_state", "ef_in", "v0", "samples", "sample_proofs")},
            deadline=manifest.training.beacon.genesis_time + 39 * 3,
        )
        records[("island-job", identity)] = geometry.body()
        descriptor = {
            "job": geometry.body(),
            "audit_job_hash": job.digest(),
            "lease_nonce": nonce,
            "expires": 40,
            "absolute": 40,
            "directory": str(store.state_dir / "audit-geometry-v2" / job_id / nonce),
        }
        digest = store.objects.put(canonicalize(descriptor))
        descriptor_records[f"{job_id}:{nonce}"] = {
            "descriptor_hash": digest,
            "receipt": envelope_v2.seal(
                fixture.service.COORD,
                "Receipt",
                run,
                Receipt(w=0, commit_hash=digest, received_round=1),
                100,
            ),
        }
        owner = fixture.service.OWNER.ss58
        admission_record = AdmissionRecord(
            entry.admission_id,
            hot,
            owner,
            entry.state,
            12,
            (1, 2, 3),
            0,
            False,
            "metadata-oracle",
            manifest.network.admission_policy_hash,
            40,
            None,
        )
        funding = FundedStatus(run, hot, entry.admission_id, "test", 10000, 0, h, True)
        statuses[hot] = AdmissionStatus(
            admission_record,
            funding,
            1000000,
            1000000,
            not (fault == "funding" and index == 0),
            False,
        )
    from hypertrain.protocol.messages_v2 import EscrowLock

    origins = {}
    for entry in opening.roster:
        lock = EscrowLock(
            operation_id=h,
            owner=fixture.service.OWNER.ss58,
            units=10000,
            origin_ids=[h],
            admission_id=entry.admission_id,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        origins[entry.admission_id] = (
            []
            if fault == "origins"
            else [
                {"origin": h, "units": 10000, "issued_origin": h, "request": lock.model_dump_json()}
            ]
        )
    # No ledger SQL mutations/auto-graduation; oracle facts test guards only, not funded success.
    escrow, admission, disputes = store._services(run)
    monkeypatch.setattr(admission, "status", lambda hotkey, **k: statuses[hotkey])
    monkeypatch.setattr(escrow, "locked", lambda *a: (10000, h))
    monkeypatch.setattr(store, "_owner_v2", lambda *a: fixture.service.OWNER.ss58)
    outer = genesis.outer_state
    allocation = {
        "policy_hash": manifest.network.aggregation_policy_hash,
        "entries": [
            {
                "hotkey": r.hotkey,
                "admission_id": r.admission_id,
                "probation": False,
                "weight_units": 4194304,
                "commit_hash": h,
                "delta_manifest_hash": h,
            }
            for r in sorted(opening.roster, key=lambda r: r.hotkey.encode())
        ],
    }
    tape_body = TapeBodyV2(
        v="ht-tape-v2",
        arithmetic="flat-cclip-cap-v1",
        run_id=run,
        w=0,
        policy_hash=manifest.network.aggregation_policy_hash,
        policy_hashes=opening.policy_hashes,
        predecessor_tape_hash="00" * 32,
        prev_state=outer,
        prev_hashes=genesis.outer_hashes,
        inputs=[
            TapeInput(
                roster=r,
                commit_hash=h,
                delta_manifest_hash=h,
                assignment_hash=h,
                anchor_hash=h,
                verdict_hash=h,
                lock_receipt_hash=h,
                settlement_hash=h,
            )
            for r in opening.roster
        ],
        input_root=h,
        excluded=[],
        allocation=allocation,
        preclipped=[],
        copy_suspicion=[],
        out_state=outer,
        out_hashes=genesis.outer_hashes,
    )
    tape = TapeV2(
        body=tape_body,
        signer=fixture.service.COORD.ss58,
        sig=fixture.service.COORD.sign(tape_signing_message(tape_body.body(), run)).hex(),
    )
    if fault == "signed" and mode == "aggregate":
        tape = tape.model_copy(update={"sig": "00" * 64})
    tape_raw = (
        b"x" * ((1 << 20) + 1) if fault == "size" and mode == "aggregate" else tape.to_bytes()
    )
    tape_hash = store.objects.put(tape_raw)
    applied = {
        "tape_hash": tape_hash,
        "prev_state": outer,
        "predecessor_tape_hash": "00" * 32,
        "out_state": outer,
        "theta_hash": genesis.outer_hashes["theta_hash"],
    }
    target = store.objects._path(delta_hash if mode == "input" else tape_hash)
    if fault in ("path", "fifo"):
        target.unlink()
        if fault == "fifo":
            os.mkfifo(target)
        else:
            outside = store.state_dir / "outside-integrity"
            durable_write(outside, payload if mode == "input" else tape_raw)
            target.symlink_to(outside)
    estimate = store.require_roster_v2(run, roster=opening.roster)

    def read_oracle(r, kind, identity):
        if kind == "round":
            return envelope_v2.seal(fixture.service.COORD, "RoundOpenV2", run, opening, 100)
        if kind == "round-resource":
            return estimate
        if kind == "audit-job":
            return jobs[identity].body()
        if kind == "audit-geometry":
            return descriptor_records[identity]
        if kind == "applied":
            return applied
        if (kind, identity) in records:
            return records[(kind, identity)]
        return original_record(r, kind, identity)

    monkeypatch.setattr(store, "_record_v2", read_oracle)

    class IntegrityReadOracle:
        def __getattr__(self, name):
            return getattr(connection, name)

        def execute(self, sql, parameters=()):
            if sql == "SELECT * FROM accepted_v2 WHERE key=?":
                return SimpleNamespace(fetchone=lambda: intake.get(parameters[0]))
            if sql.startswith("SELECT * FROM audit_leases_v2 WHERE run_id=? AND w=? AND hotkey=?"):
                return SimpleNamespace(fetchone=lambda: leases[parameters[2]])
            if sql.startswith("SELECT u.origin,u.units,o.origin AS issued_origin"):
                return SimpleNamespace(fetchall=lambda: origins[parameters[1]])
            if sql.startswith("SELECT data FROM records_v2 WHERE run_id=? AND kind='applied'"):
                return SimpleNamespace(fetchone=lambda: [json.dumps(applied)])
            return connection.execute(sql, parameters)

    monkeypatch.setattr(store, "_db", IntegrityReadOracle())
    events = []

    def forbidden(*a, **k):
        events.append("heavy-sql")
        pytest.fail("integrity crossed denied boundary")

    for name in ("_put_record_v2", "_authenticated_v2", "_fresh_v2", "_capacity_outer_v2"):
        monkeypatch.setattr(store, name, forbidden)
    monkeypatch.setattr(escrow, "reward_finalize", forbidden)
    before = connection.total_changes
    final = Finalize(
        w=0,
        final_theta_hash_w1=genesis.outer_hashes["theta_hash"],
        included=sorted(keys),
        entitlements_root=h,
    )
    final_raw = envelope_v2.seal(fixture.service.COORD, "Finalize", run, final, 100)
    if mode == "final" and fault == "signed":
        final_raw["sig"] = "00" * 64
    expected = (
        "SERVICE_RUNTIME_NOT_ENFORCED"
        if fault == "honest"
        else "capacity bytes exceed limit"
        if fault == "size"
        else "capacity object is not a regular file"
        if fault == "fifo"
        else "confinement"
        if fault == "path"
        else "signature"
        if fault in ("signature", "signed") and mode != "aggregate"
        else "SERVICE_AGGREGATE_TAPE_AUTHORITY"
        if fault == "signed"
        else "SERVICE_INPUT_MATCH_AUTHORITY"
        if fault in ("lease", "root")
        else "SERVICE_INPUT_FUNDING_AUTHORITY"
        if fault == "funding"
        else "SERVICE_INPUT_FUNDING_ORIGINS"
    )
    with pytest.raises((ChallengeError, ValueError), match=expected):
        if mode == "input":
            store.verified_inputs_v2(run, 0)
        elif mode == "aggregate":
            store.aggregate_v2(run, 0)
        else:
            store.finalize_v2(run, 0, canonicalize(final_raw))
    assert events == [] and connection.total_changes == before
