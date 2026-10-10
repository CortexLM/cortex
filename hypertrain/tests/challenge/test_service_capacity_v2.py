"""First increment: real signed resource intake, metadata only, no miner sweep."""

from __future__ import annotations

import ast
import importlib.util
import resource
import struct
import sys
from pathlib import Path

import pytest

from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Receipt, f32hex
from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2, RunManifestV2

TREE = Path(__file__).parents[2]
spec = importlib.util.spec_from_file_location(
    "capacity_service_fixture", TREE / "tests/challenge/test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
service = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = service
spec.loader.exec_module(service)
network = service.network


@pytest.fixture
def tiny(network, tmp_path):
    """Create another real owner-signed run with exact resource-profile dimensions."""
    from hypertrain.protocol.messages_v2 import AggregationPolicyV2, AuditPolicyV2

    store = network.store
    body = network.manifest.body()
    m = body["training"]["model"]
    m.update(
        n_layers=1,
        d_model=8,
        n_heads=2,
        n_kv_heads=2,
        d_ff=8,
        vocab=16,
        seq_len=2,
        param_count=728,
        n_experts=1,
        top_k_experts=1,
        compute_dtype="fp32",
    )
    inner = body["training"]["inner"]
    inner.update(H=2, J=1, micro_batch=1, grad_accum=1, opt="adamw", state_policy="carry")
    inner["lr_schedule"].update(warmup=0)
    body["training"]["dataset"]["n_samples"] = 256
    body["training"]["outer"]["opt"] = "nesterov"
    body["training"]["reference_spec"]["layout"].update(
        pp=1, n_gpus=1, dp_size=1, ep_size=1, zero1=False
    )
    old = AggregationPolicyV2.model_validate_json(
        store.objects.get(network.manifest.network.aggregation_policy_hash)
    )
    policy = old.model_copy(update={"cclip_iters": 1})
    store.objects.put(canonicalize(policy.body()))
    body["network"]["aggregation_policy_hash"] = policy.digest()
    audit = AuditPolicyV2.model_validate_json(
        store.objects.get(network.manifest.network.audit_policy_hash)
    ).model_copy(update={"max_steps_per_attempt": 4})
    store.objects.put(canonicalize(audit.body()))
    body["network"]["audit_policy_hash"] = audit.digest()
    manifest = RunManifestV2.model_validate(body)
    objects = LocalFSStore(tmp_path / "capacity-objects")
    for name in (*PolicyHashes.model_fields, "relay_registry_hash"):
        objects.put(network.store.objects.get(getattr(manifest.network, name)))
    store = ChallengeStore(
        tmp_path / "capacity-state",
        network.store.params,
        service.COORD,
        service.OWNER.ss58,
        verify_beacon=network.store.verify_beacon,
        objects=objects,
    )
    store.push_beacon(service.fixture_beacon(1))
    raw = canonicalize(
        envelope_v2.seal(service.OWNER, "RunManifestV2", manifest.run_id(), manifest, 100)
    )
    store.create_run_v2(raw)
    yield store, manifest
    store.close_v2_notifications()
    store._db.close()


def signed_profile(store, manifest, signer=None):
    profile = store._service_profile_v2(manifest.run_id())
    body = canonicalize(profile.model_dump(mode="json"))
    receipt = Receipt(w=0, commit_hash=sha256_hex(body), received_round=1)
    raw = canonicalize(
        envelope_v2.seal(signer or service.OWNER, "Receipt", manifest.run_id(), receipt, 100)
    )
    return body, raw


def roster(n):
    return [
        {
            "hotkey": Keypair(bytes([i + 1]) * 32).ss58,
            "slot": i,
            "q_i": f32hex(1),
            "admission_id": sha256_hex(str(i).encode()),
            "coldkey_group": f"owner{i}",
            "state": "ACTIVE",
            "eligible_weight": 1 << 22,
        }
        for i in range(n)
    ]


def opening(manifest, n=32):
    entries = roster(n)
    return RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash="0" * 64,
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(entries)),
        honeypot_commit="0" * 64,
        d_open=2,
        d_assign=3,
        d_commit=20,
        d_audit=21,
        d_upload=22,
        d_final=100,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash="0" * 64,
        audit_mode="anchored-full",
        roster=entries,
    )


def test_owner_admission_accepts_real32_metadata_when_profile_exact(tiny, monkeypatch):
    # Given: fail-fast tensor allocator proves metadata intake never trains/allocates state.
    store, manifest = tiny

    def fail(*args, **kwargs):
        pytest.fail("unexpected tensor state allocation")

    monkeypatch.setattr("hypertrain.trainer.model.init_params", fail)
    body, raw = signed_profile(store, manifest)
    # When
    digest = store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    estimate = store.require_roster_v2(manifest.run_id(), roster=roster(32))
    # Then
    assert digest == sha256_hex(body)
    assert estimate["count"] == 32
    assert estimate["audit_steps"] == 256 and estimate["repair_steps"] == 512
    assert estimate["tape_bytes_bound"] < 1 << 20
    assert estimate["memory_estimate_bytes"] < 1 << 30
    print(
        f"CAPACITY_EXISTING_FIXTURE_MAX_RSS_KIB={resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}"
    )
    assert store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw) == digest


@pytest.mark.parametrize("signer", [service.COORD, service.HOT[0]])
def test_wrong_role_rejects_when_receipt_not_owner(tiny, signer):
    store, manifest = tiny
    body, raw = signed_profile(store, manifest, signer)
    with pytest.raises(ChallengeError):
        store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    assert not store._db.execute(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='service-admission'", (manifest.run_id(),)
    ).fetchone()


@pytest.mark.parametrize(
    "fault", ["bool", "version_bool", "float", "extra", "limit", "backend", "run"]
)
def test_invalid_internal_profile_rejects_when_signed_by_owner(tiny, fault):
    store, manifest = tiny
    body, _ = signed_profile(store, manifest)
    data = envelope_v2.load_json(body)
    match fault:
        case "bool":
            data["max_complete_roster"] = True
        case "version_bool":
            data["version"] = True
        case "float":
            body = body.replace(b'"max_complete_roster":32', b'"max_complete_roster":32.0')
        case "extra":
            data["qualified"] = True
        case "limit":
            data["max_complete_roster"] = 128
        case "backend":
            data["backend_binding_hash"] = "22" * 32
        case "run":
            data["run_id"] = "22" * 32
    if fault != "float":
        body = canonicalize(data)
    receipt = Receipt(w=0, commit_hash=sha256_hex(body), received_round=1)
    raw = canonicalize(envelope_v2.seal(service.OWNER, "Receipt", manifest.run_id(), receipt, 100))
    with pytest.raises((ChallengeError, ValueError)):
        store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)


@pytest.mark.parametrize("n,admit", [(17, False), (33, True)])
def test_bad_roster_rejects_before_stateallocation_when_admission_missing_or_count_excess(
    tiny, monkeypatch, n, admit
):
    store, manifest = tiny
    if admit:
        body, raw = signed_profile(store, manifest)
        store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    monkeypatch.setattr(
        "hypertrain.trainer.model.init_params", lambda *a, **k: pytest.fail("allocation")
    )
    raw = canonicalize(
        envelope_v2.seal(service.COORD, "RoundOpenV2", manifest.run_id(), opening(manifest, n), 100)
    )
    before = store._db.total_changes
    with pytest.raises(ChallengeError, match="ROSTER_LIMIT"):
        store.open_round_v2(manifest.run_id(), raw)
    assert store._db.total_changes == before


def test_legacy16_guard_preserved_when_no_resource_admission(network):
    assert network.store.require_roster_v2(network.manifest.run_id(), roster=roster(16)) is None


def test_profile_tamper_rejects_when_original_journal_body_changed(tiny):
    store, manifest = tiny
    body, raw = signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    with store._tx():
        record = store._record_v2(manifest.run_id(), "service-admission", "run")
        record["body"]["max_complete_roster"] = 128
        store._put_record_v2(manifest.run_id(), "service-admission", "run", record)
    with pytest.raises(ChallengeError, match="AUTHORITY"):
        store.require_roster_v2(manifest.run_id(), roster=roster(32))


def test_metadata_bytes_reject_when_full_descriptor_bound_exceeds_tape_budget(tiny):
    store, manifest = tiny
    body, raw = signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    entries = roster(32)
    for entry in entries:
        entry["coldkey_group"] = "x" * 40000
    with pytest.raises(ChallengeError, match="RESOURCE_ESTIMATE"):
        store.require_roster_v2(manifest.run_id(), roster=entries)


@pytest.mark.parametrize("n", [16, 17, 32])
def test_profile_open_denies_before_allocation_or_publication_when_runtime_unenforced(
    tiny, monkeypatch, n
):
    store, manifest = tiny
    body, raw = signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    seen = {"init_params": 0, "AnchorCache": 0, "put": 0}

    def stop(name):
        def forbidden(*args, **kwargs):
            seen[name] += 1
            pytest.fail(f"runtime denial reached {name}")

        return forbidden

    monkeypatch.setattr("hypertrain.trainer.model.init_params", stop("init_params"))
    monkeypatch.setattr("hypertrain.auditor.replay.AnchorCache", stop("AnchorCache"))
    monkeypatch.setattr(store.objects, "put", stop("put"))
    raw = canonicalize(
        envelope_v2.seal(service.COORD, "RoundOpenV2", manifest.run_id(), opening(manifest, n), 100)
    )
    before = store._db.total_changes
    with pytest.raises(ChallengeError, match="SERVICE_RUNTIME_NOT_ENFORCED"):
        store.open_round_v2(manifest.run_id(), raw)
    assert seen == {"init_params": 0, "AnchorCache": 0, "put": 0}
    assert store._db.total_changes == before
    assert not store._db.execute(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind IN "
        "('round-resource','round','round-meta','start','finality','service-charge')",
        (manifest.run_id(),),
    ).fetchone()


def test_existing_reservation_survives_open_denial_before_pool_or_allocation(tiny, monkeypatch):
    store, manifest = tiny
    body, raw = signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    with store._tx():
        store._put_record_v2(
            manifest.run_id(),
            "round-resource",
            "prior",
            {
                "count": 1,
                "audit_steps": 8,
                "repair_steps": 16,
            },
        )
    monkeypatch.setattr(
        "hypertrain.trainer.model.init_params", lambda *a, **k: pytest.fail("allocation")
    )
    raw = canonicalize(
        envelope_v2.seal(service.COORD, "RoundOpenV2", manifest.run_id(), opening(manifest), 100)
    )
    before = store._db.total_changes
    with pytest.raises(ChallengeError, match="SERVICE_RUNTIME_NOT_ENFORCED"):
        store.open_round_v2(manifest.run_id(), raw)
    assert store._db.total_changes == before
    assert store._record_v2(manifest.run_id(), "round-resource", "prior")["count"] == 1
    assert not store._db.execute(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round-resource' AND id='0'",
        (manifest.run_id(),),
    ).fetchone()


def test_legacy_open_without_profile_reaches_existing_allocator_seam(tiny, monkeypatch):
    store, manifest = tiny
    seen = []

    def stop(*args):
        seen.append("init_params")
        raise RuntimeError("legacy allocation sentinel")

    store.clock = lambda: manifest.training.beacon.genesis_time
    monkeypatch.setattr("hypertrain.trainer.model.init_params", stop)
    raw = canonicalize(
        envelope_v2.seal(
            service.COORD, "RoundOpenV2", manifest.run_id(), opening(manifest, 16), 100
        )
    )
    with pytest.raises(RuntimeError, match="legacy allocation sentinel"):
        store.open_round_v2(manifest.run_id(), raw)
    assert seen == ["init_params"]


def test_open_denial_never_refunds_existing_committed_debit(tiny):
    store, manifest = reserved(tiny)
    run_id = manifest.run_id()
    assert store._service_charge_v2(run_id, 0, "prior", 1, kind="audit", units=512)
    before = store._db.total_changes
    raw = canonicalize(
        envelope_v2.seal(service.COORD, "RoundOpenV2", run_id, opening(manifest), 100)
    )
    with pytest.raises(ChallengeError, match="SERVICE_RUNTIME_NOT_ENFORCED"):
        store.open_round_v2(run_id, raw)
    assert store._db.total_changes == before
    assert not store._service_charge_v2(run_id, 0, "prior", 1, kind="audit", units=512)
    with pytest.raises(ChallengeError, match="BUDGET_EXHAUSTED"):
        store._service_charge_v2(run_id, 0, "prior", 2, kind="audit", units=1)


def reserved(tiny):
    store, manifest = tiny
    body, raw = signed_profile(store, manifest)
    store.bootstrap_service_capacity_v2(manifest.run_id(), body, raw)
    entries = roster(32)
    estimate = store.require_roster_v2(manifest.run_id(), roster=entries)
    with store._tx():
        store._put_record_v2(
            manifest.run_id(),
            "round",
            "0",
            envelope_v2.seal(
                service.COORD, "RoundOpenV2", manifest.run_id(), opening(manifest), 100
            ),
        )
        store._put_record_v2(
            manifest.run_id(),
            "round-resource",
            "0",
            {
                **estimate,
                "deadline": 100,
                "max_attempts": 2,
                "max_running_jobs": 2,
            },
        )
    return store, manifest


@pytest.mark.parametrize("method", ["_assignment_v2", "verified_inputs_v2", "aggregate_v2"])
def test_original_roster_guard_rejects_before_load_when_reservation_changed(
    tiny, monkeypatch, method
):
    store, manifest = reserved(tiny)
    with store._tx():
        record = store._record_v2(manifest.run_id(), "round", "0")
        record["body"]["roster"].pop()
        store._put_record_v2(manifest.run_id(), "round", "0", record)
    monkeypatch.setattr(
        "hypertrain.trainer.model.init_params", lambda *a, **k: pytest.fail("state load")
    )
    with pytest.raises(ChallengeError, match="RESOURCE_BINDING"):
        if method == "_assignment_v2":
            store._assignment_v2(manifest.run_id(), 0, service.HOT[0].ss58)
        else:
            getattr(store, method)(manifest.run_id(), 0)


def test_heavy32_execution_remains_blocked_when_only_accounting_admitted(tiny, monkeypatch):
    store, manifest = reserved(tiny)
    events = {"allocation": 0, "state_load": 0, "decode": 0, "worker_launch": 0}

    def count(name):
        def forbidden(*args, **kwargs):
            events[name] += 1
            pytest.fail(name)

        return forbidden

    for symbol, name in (
        ("hypertrain.trainer.model.init_params", "allocation"),
        ("hypertrain.aggregator.core.load_state", "state_load"),
        ("hypertrain.trainer.compress.decompress", "decode"),
        ("hypertrain.auditor.worker.execute_island_audit", "worker_launch"),
    ):
        monkeypatch.setattr(symbol, count(name))
    with pytest.raises(ChallengeError, match="RUNTIME_NOT_ENFORCED"):
        store.aggregate_v2(manifest.run_id(), 0)
    assert events == {"allocation": 0, "state_load": 0, "decode": 0, "worker_launch": 0}


@pytest.mark.parametrize("kind,limit", [("object", 65536), ("tape", 1 << 20), ("context", 1 << 20)])
def test_actual_byte_limits_reject_when_one_byte_over_profile(tiny, kind, limit):
    store, manifest = reserved(tiny)
    store._service_bytes_v2(manifest.run_id(), 0, b"x" * limit, kind=kind)
    with pytest.raises(ChallengeError, match="BYTES"):
        store._service_bytes_v2(manifest.run_id(), 0, b"x" * (limit + 1), kind=kind)


@pytest.mark.parametrize("kind,limit", [("audit", 512), ("outer", 50_000_000), ("repair", 1024)])
def test_charges_survive_failure_restart_without_duplicate_debit(tiny, kind, limit):
    store, manifest = reserved(tiny)
    run_id = manifest.run_id()
    assert store._service_charge_v2(run_id, 0, "job", 1, kind=kind, units=limit)
    # A failed execution does not undo the committed debit; next service process restores SQLite.
    restored = ChallengeStore(
        store.state_dir,
        store.params,
        service.COORD,
        service.OWNER.ss58,
        verify_beacon=store.verify_beacon,
        objects=store.objects,
    )
    try:
        assert not restored._service_charge_v2(run_id, 0, "job", 1, kind=kind, units=limit)
        with pytest.raises(ChallengeError, match="BUDGET_EXHAUSTED"):
            restored._service_charge_v2(run_id, 0, "job", 2, kind=kind, units=1)
        with pytest.raises(ChallengeError, match="CONFLICT"):
            restored._service_charge_v2(run_id, 0, "job", 1, kind=kind, units=1)
        assert (
            restored._db.execute(
                "SELECT COUNT(*) FROM records_v2 WHERE run_id=? AND kind='service-charge'",
                (run_id,),
            ).fetchone()[0]
            == 1
        )
    finally:
        restored.close_v2_notifications()
        restored._db.close()


def test_source_binding_ignores_unrelated_methods_but_rejects_capacity_changes(tmp_path):
    source = TREE / "src/hypertrain"
    paths = (
        "aggregator/core.py",
        "aggregator/tape_v2.py",
        "aggregator/rollback_v2.py",
        "trainer/compress.py",
    )
    original = ChallengeStore._service_implementation_v2(source, paths)
    tree = ast.parse((source / "challenge/store.py").read_text())
    # Inspect AST selection directly, without rewriting production/source fixtures.
    import unittest.mock

    unrelated = ast.FunctionDef(
        name="unrelated",
        args=ast.arguments(posonlyargs=[], args=[], kwonlyargs=[], kw_defaults=[], defaults=[]),
        body=[ast.Pass()],
        decorator_list=[],
    )
    tree.body.append(unrelated)
    with unittest.mock.patch("hypertrain.challenge.store.ast.parse", return_value=tree):
        assert ChallengeStore._service_implementation_v2(source, paths) == original
    node = next(
        n
        for n in ast.walk(tree)
        if isinstance(n, ast.FunctionDef) and n.name == "_service_bytes_v2"
    )
    node.body.append(
        ast.Raise(
            exc=ast.Call(func=ast.Name(id="RuntimeError", ctx=ast.Load()), args=[], keywords=[])
        )
    )
    with unittest.mock.patch("hypertrain.challenge.store.ast.parse", return_value=tree):
        assert ChallengeStore._service_implementation_v2(source, paths) != original


def test_finalization_cannot_reset_charges_and_nested_failure_cannot_erase_debit(tiny):
    store, manifest = reserved(tiny)
    run_id = manifest.run_id()
    with store._tx():
        with pytest.raises(ChallengeError, match="REQUIRES_COMMITTED_TRANSACTION"):
            store._service_charge_v2(run_id, 0, "job", 1, kind="audit", units=1)
    assert store._service_charge_v2(run_id, 0, "job", 1, kind="audit", units=512)
    with store._tx():
        store._put_record_v2(run_id, "finalize", "0", {"w": 0})
    with pytest.raises(ChallengeError, match="BUDGET_EXHAUSTED"):
        store._service_charge_v2(run_id, 0, "next", 1, kind="audit", units=1)


@pytest.mark.parametrize(
    "case", ["valid", "later-shape", "codec", "format", "oversize", "stored-oversize", "legacy"]
)
def test_delta_intake_checks_trusted_geometry_before_durable_acceptance(tiny, monkeypatch, case):
    import torch

    import hypertrain.trainer.compress as codec
    from hypertrain.protocol.messages_v2 import CommitV2, DeltaManifestV2
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import param_shapes

    store, manifest = tiny if case == "legacy" else reserved(tiny)
    run_id, miner = manifest.run_id(), service.HOT[0]
    config = TrainConfig.from_manifest_v2(manifest)
    shapes = param_shapes(config.model)
    parts = [codec.MAGIC["dense-int8"], struct.pack("<I", len(shapes))]
    for index, (name, shape) in enumerate(sorted(shapes.items())):
        elements = 1
        for dim in shape:
            elements *= dim
        body = struct.pack("<I", 256) + struct.pack("<f", 1.0) * ((elements + 255) // 256)
        body += bytes(elements)
        if case == "later-shape" and index == len(shapes) - 1:
            shape = (2**63,) * len(shape)
        parts += [
            struct.pack("<Q", len(name)),
            name.encode(),
            struct.pack("<I", len(shape)),
            b"".join(struct.pack("<Q", dim) for dim in shape),
            struct.pack("<Q", len(body)),
            body,
        ]
    payload = b"legacy opaque bytes" if case == "legacy" else b"".join(parts)
    if case == "codec":
        parts = [codec.MAGIC["sparseloco"], struct.pack("<I", len(shapes))]
        for name, shape in sorted(shapes.items()):
            body = struct.pack("<QffIB", 1, 0.0, 0.0, 0, 0)
            parts += [
                struct.pack("<Q", len(name)),
                name.encode(),
                struct.pack("<I", len(shape)),
                b"".join(struct.pack("<Q", dim) for dim in shape),
                struct.pack("<Q", len(body)),
                body,
            ]
        payload = b"".join(parts)
    if case == "oversize":
        payload = b"x" * 65537
    payload_hash = store.objects.put(payload)
    delta = DeltaManifestV2(
        w=0,
        hotkey=miner.ss58,
        delta_hash=payload_hash,
        uri="file:///delta",
        size=len(payload),
        format="ht-sparse-v1" if case in ("codec", "format") else "ht-dense-int8-v1",
        chunks=[{"off": 0, "len": len(payload), "sha256": payload_hash}],
        grant_hash="01" * 32,
        master_acceptance_hash="02" * 32,
    )
    commit = CommitV2(
        w=0,
        hotkey=miner.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=3,
        leaves_root="03" * 32,
        metrics_root="04" * 32,
        final_theta_hash="05" * 32,
        ef_in_hash="06" * 32,
        ef_out_hash="07" * 32,
        delta_hash=payload_hash,
        delta_bytes=len(payload),
        tokens=4,
    )
    store._services(run_id)  # Real shared admission table; no authentication stub.
    store.clock = lambda: manifest.training.beacon.genesis_time
    with store._tx():
        store._db.execute(
            "INSERT INTO admissions_v2(admission_id,run_id,hotkey,coldkey,state) VALUES(?,?,?,?,?)",
            ("08" * 32, run_id, miner.ss58, service.OWNER.ss58, "ACTIVE"),
        )
        store._put_record_v2(
            run_id,
            "round",
            "0",
            {"body": opening(manifest, 16 if case == "legacy" else 32).model_dump(mode="json")},
        )
        store._put_record_v2(run_id, "assignment", f"0:{miner.ss58}", {"samples": [0, 1]})
        store._put_record_v2(
            run_id,
            "commit",
            f"0:{miner.ss58}",
            envelope_v2.seal(miner, "CommitV2", run_id, commit, 100),
        )
        store._put_record_v2(
            run_id,
            "master-acceptance",
            delta.master_acceptance_hash,
            {"body": {"delta_hash": payload_hash}},
        )
    raw = canonicalize(envelope_v2.seal(miner, "DeltaManifestV2", run_id, delta, 100))
    if case == "stored-oversize":
        # Actual backing object lies about committed size; bounded read must stop at size+1.
        store.objects._path(payload_hash).write_bytes(payload + b"x" * 65536)
    events = {"decode": 0, "allocation": 0, "unbounded_get": 0}
    original_get = store.objects.get
    original_open = Path.open
    reads = []

    class BoundedFile:
        def __enter__(self):
            self.file = original_open(store.objects._path(payload_hash), "rb")
            return self

        def read(self, size):
            reads.append(size)
            return self.file.read(size)

        def __exit__(self, *args):
            self.file.close()

    def open_path(path, *args, **kwargs):
        if path == store.objects._path(payload_hash) and case != "legacy":
            return BoundedFile()
        return original_open(path, *args, **kwargs)

    def get(key):
        if key == payload_hash:
            events["unbounded_get"] += 1
        return original_get(key)

    def forbidden(name):
        def fail(*args, **kwargs):
            events[name] += 1
            pytest.fail(name)

        return fail

    monkeypatch.setattr(store.objects, "get", get)
    monkeypatch.setattr(Path, "open", open_path)
    for name in ("_int8_decode", "_sparse_decode"):
        monkeypatch.setattr(codec, name, forbidden("decode"))
    for name in ("zeros", "empty", "tensor", "from_numpy"):
        monkeypatch.setattr(torch, name, forbidden("allocation"))
    if case in ("valid", "legacy"):
        result = store.training_v2(run_id, "delta", raw)
        assert result["body"]["commit_hash"] == sha256_hex(
            canonicalize(delta.model_dump(mode="json"))
        )
        assert store.training_v2(run_id, "delta", raw) == result
    else:
        error = {
            "later-shape": "SERVICE_DELTA_PAYLOAD",
            "codec": "SERVICE_DELTA_CODEC",
            "format": "SERVICE_DELTA_CODEC",
            "oversize": "SERVICE_OBJECT_BYTES",
            "stored-oversize": "original payload chunks mismatch",
        }[case]
        with pytest.raises(ChallengeError, match=error):
            store.training_v2(run_id, "delta", raw)
        assert not store._db.execute(
            "SELECT 1 FROM accepted_v2 WHERE envelope=?", (raw,)
        ).fetchone()
        assert not store._db.execute(
            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='delta'", (run_id,)
        ).fetchone()
        assert not store.objects._path(
            sha256_hex(canonicalize(delta.model_dump(mode="json")))
        ).exists()
    assert events == {"decode": 0, "allocation": 0, "unbounded_get": int(case == "legacy")}
    assert reads == ([] if case in ("oversize", "legacy") else [delta.size + 1])


def test_changed_codec_source_invalidates_prior_admission_without_requalification(
    tiny, monkeypatch
):
    store, manifest = tiny
    run_id = manifest.run_id()
    codec_path = TREE / "src/hypertrain/trainer/compress.py"
    current = Path.read_bytes

    def prior_bytes(path):
        return current(path) + b"\n# prior codec source\n" if path == codec_path else current(path)

    with monkeypatch.context() as old:
        old.setattr(Path, "read_bytes", prior_bytes)
        body, raw = signed_profile(store, manifest)
        store.bootstrap_service_capacity_v2(run_id, body, raw)
    prior = store._record_v2(run_id, "service-admission", "run")
    with pytest.raises(ChallengeError, match="SERVICE_PROFILE_AUTHORITY"):
        store.require_roster_v2(run_id, roster=roster(32))
    current_body, current_raw = signed_profile(store, manifest)
    with pytest.raises(ChallengeError, match="SERVICE_PROFILE_IMMUTABLE"):
        store.bootstrap_service_capacity_v2(run_id, current_body, current_raw)
    assert store._record_v2(run_id, "service-admission", "run") == prior


def test_real_capacity_launch_failure_keeps_debit_and_prevents_restart_rerun(
    tiny, tmp_path, monkeypatch
):
    import os
    import time

    from hypertrain.miner import island_launch as launcher

    store, manifest = reserved(tiny)
    run_id = manifest.run_id()
    monkeypatch.setattr(launcher, "_CAPACITY_MEMORY", 64 << 20)
    monkeypatch.setattr(launcher, "_CAPACITY_CPU_PERCENT", 50)
    profile, profile_hash = store._service_admission_v2(run_id)
    assert profile.memory_reservation_bytes == 1 << 30
    calls = []

    def charge():
        calls.append("debit")
        return store._service_charge_v2(run_id, 0, "actual-runtime", 1, kind="audit", units=512)

    capacity = launcher.CapacityAttempt("88" * 32, profile_hash, tmp_path / "runtime.lock", charge)
    attempt = tmp_path / "runtime"
    with pytest.raises(launcher.IslandFailure, match="execution failed"):
        launcher.run_capacity_argv(
            [sys.executable, "-c", "raise SystemExit(7)"],
            attempt,
            int(time.time()) + 20,
            capacity,
            env=dict(os.environ),
        )
    assert calls == ["debit"] and (attempt / "capacity-cleaned.json").exists()
    assert (
        "ExecMainStatus=7"
        in envelope_v2.load_json((attempt / "capacity-result.json").read_bytes())["status"]
    )
    restored = ChallengeStore(
        store.state_dir,
        store.params,
        service.COORD,
        service.OWNER.ss58,
        verify_beacon=store.verify_beacon,
        objects=store.objects,
    )
    try:
        capacity2 = launcher.CapacityAttempt(
            "88" * 32,
            profile_hash,
            tmp_path / "runtime.lock",
            lambda: restored._service_charge_v2(
                run_id, 0, "actual-runtime", 1, kind="audit", units=512
            ),
        )
        with pytest.raises(launcher.IslandFailure, match="already charged"):
            launcher.run_capacity_argv(
                [sys.executable, "-c", "raise SystemExit(99)"],
                tmp_path / "new-process-attempt",
                int(time.time()) + 20,
                capacity2,
                env=dict(os.environ),
            )
        assert not (tmp_path / "new-process-attempt/capacity-attempt.json").exists()
        with pytest.raises(ChallengeError, match="BUDGET_EXHAUSTED"):
            restored._service_charge_v2(run_id, 0, "actual-runtime", 2, kind="audit", units=1)
        assert (
            restored._db.execute(
                "SELECT COUNT(*) FROM records_v2 WHERE kind='service-charge'"
            ).fetchone()[0]
            == 1
        )
    finally:
        restored.close_v2_notifications()
        restored._db.close()
