"""Trusted backend dispatch boundaries; synthetic CUDA evidence is not GPU proof."""

from __future__ import annotations

import importlib.util
import json
import os
import sys
from dataclasses import replace
from pathlib import Path

import pytest

from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.gpu_ops.journal import fsha
from hypertrain.gpu_ops.launcher import Reject
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import (
    EconomicsPolicyV2,
    IslandJobV1,
    RunManifestV2,
)


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    loaded = importlib.util.module_from_spec(spec)
    sys.modules[name] = loaded
    spec.loader.exec_module(loaded)
    return loaded


TREE = Path(__file__).parents[2]
service = module("backend_service_fixture", TREE / "tests/challenge/test_service_network_v2.py")
gpu = module("backend_gpu_fixture", TREE / "tests/gpu_ops/test_network_gpu_qualification.py")
network = service.network


def test_genuine_cuda_index_missing_fails_closed(tmp_path):
    with pytest.raises(FileNotFoundError):
        service.genuine_index(tmp_path / "absent-index.json")


@pytest.mark.parametrize("fault", ["owner", "digest", "clock"])
def test_genuine_cuda_index_signed_refusal(tmp_path, fault):
    """CPU-only refusal of untrusted index, not a CUDA qualification."""
    from pydantic import ValidationError

    from hypertrain.protocol.keys import Keypair

    run = "ab" * 32
    record = {
        "schema": "ht-genuine-cuda-context/1",
        "run_id": run,
        "beacon": 2,
        "files": {},
        "configs": ["h0", "h1"],
        "results": ["r0", "r1"],
        "snapshot": {},
        "cases": {},
        "lifecycle": "/absent",
        "d2_reservation": "d2",
    }
    if fault == "clock":
        record["beacon"] = True
    signer = Keypair(b"\x21" * 32) if fault == "owner" else service.OWNER
    commit = "00" * 32 if fault == "digest" else sha256_hex(canonicalize(record))
    receipt = envelope_v2.seal(
        signer, "Receipt", run, Receipt(w=0, commit_hash=commit, received_round=1), 10
    )
    path = tmp_path / "index.json"
    path.write_bytes(canonicalize({"record": record, "receipt": receipt}))
    with pytest.raises((ValueError, ValidationError)):
        service.genuine_index(path)


def test_genuine_cuda_index_changed_file_refused(tmp_path):
    run = "ab" * 32
    member = tmp_path / "manifest.json"
    member.write_bytes(b"changed")
    record = {
        "schema": "ht-genuine-cuda-context/1",
        "run_id": run,
        "beacon": 2,
        "files": {"manifest": {"path": "manifest.json", "sha256": "00" * 32}},
        "configs": ["h0", "h1"],
        "results": ["r0", "r1"],
        "snapshot": {},
        "cases": {},
        "lifecycle": "/absent",
        "d2_reservation": "d2",
    }
    signed = envelope_v2.seal(
        service.OWNER,
        "Receipt",
        run,
        Receipt(w=0, commit_hash=sha256_hex(canonicalize(record)), received_round=1),
        10,
    )
    path = tmp_path / "index.json"
    path.write_bytes(canonicalize({"record": record, "receipt": signed}))
    with pytest.raises(ValueError, match="file changed"):
        service.genuine_index(path)


@pytest.fixture
def authority(tmp_path, monkeypatch, request):
    """Real validators over synthetic rescued files, isolated from live CPU run."""
    name = request.node.originalname
    genuine = (
        name
        in {
            "test_cuda_dispatch_at_admission_audit_and_geometry",
            "test_referee_uses_frozen_backend_before_evidence",
            "test_accepted_role_factory_called_before_kernel",
            "test_real_adapter_audit_canonical_publication",
        }
        and getattr(request.node, "callspec", None) is not None
        and request.node.callspec.params.get("mode") != "cpu"
    )
    genuine = genuine or name == "test_real_adapter_audit_canonical_publication"
    if genuine:
        path = os.environ.get("HT_GENUINE_CUDA_INDEX")
        if not path:
            raise ValueError("genuine CUDA frozen index required")
        with_context = service.genuine_network(Path(path), tmp_path)
        current, configs, results, raw = next(with_context)
        try:
            yield current.store, current.manifest, configs, results, raw
        finally:
            with_context.close()
        return
    network = request.getfixturevalue("network")
    configs_seen = []
    check = gpu.driver.check_inputs

    def capture(cfg):
        configs_seen.append(cfg)
        return check(cfg)

    monkeypatch.setattr(gpu.driver, "check_inputs", capture)
    seed_dir = tmp_path / "seed"
    seed_dir.mkdir()
    gpu.test_exact_profile_accepts_wire_defaults_without_training(seed_dir)
    original = configs_seen[0]
    job = IslandJobV1.model_validate_json(original.seed_job.read_bytes())
    body = job.manifest.body()
    body["network"] = network.manifest.network.model_dump(mode="json")
    body["training"]["beacon"] = network.manifest.training.beacon.model_dump(mode="json")
    body["training"]["coord_pubkey"] = service.COORD.ss58
    body["training"]["auditors"] = [key.ss58 for key in service.AUDITORS]
    manifest = RunManifestV2.model_validate(body)
    job = job.model_copy(update={"manifest": manifest, "run_id": manifest.run_id()})
    original.seed_job.write_text(job.model_dump_json())
    results = gpu.results(tmp_path / "rescued")
    configs = []
    for index, path in enumerate(results):
        for round_index in range(2):
            for rank in range(2):
                summary = path.parent / f"round-{round_index}/published/rank-{rank}/summary.json"
                summary.write_text(json.dumps({"backend": "cuda"}))
        receipts = seed_dir / f"receipt-{index}.json"
        data = json.loads(original.lifecycle_receipts.read_bytes())
        data.update(instance_id=index + 1, machine_id=index + 10, role=f"h{index}")
        receipts.write_text(json.dumps(data))
        cfg = original.model_copy(
            update={
                "instance_id": index + 1,
                "machine_id": index + 10,
                "role": f"h{index}",
                "lifecycle_receipts": receipts,
                "lifecycle_receipts_sha256": fsha(receipts),
            }
        )
        config = seed_dir / f"config-{index}.json"
        config.write_text(cfg.model_dump_json())
        configs.append(config)
        result = json.loads(path.read_bytes())
        result.update(
            config_sha256=fsha(config),
            profile_sha256=cfg.profile_sha256,
            sources=cfg.sources,
        )
        result["environment"].update(
            image_digest=manifest.training.reference_spec.image_digest,
            torch_path="/venv/main/lib/torch/__init__.py",
        )
        path.write_text(json.dumps(result))
    objects = LocalFSStore(tmp_path / "objects")
    for field in (
        "economics_policy_hash",
        "admission_policy_hash",
        "dispute_policy_hash",
        "aggregation_policy_hash",
        "relay_registry_hash",
    ):
        objects.put(network.store.objects.get(getattr(network.manifest.network, field)))
    store = ChallengeStore(
        tmp_path / "authority-store",
        network.store.params,
        service.COORD,
        service.OWNER.ss58,
        network.store.verify_beacon,
        objects,
    )
    # Existing verified fixture beacon, copied as authority input rather than a timer.
    for row in network.store._db.execute("SELECT * FROM beacon"):
        store._db.execute("INSERT INTO beacon VALUES(?,?,?,?)", tuple(row))
    evidence = {
        "run_id": manifest.run_id(),
        "backend": "cuda",
        "reference_hash": sha256_hex(
            canonicalize(manifest.training.reference_spec.model_dump(mode="json"))
        ),
        "layout_hash": manifest.training.reference_spec.layout.model_dump_json(),
        "configs": [fsha(p) for p in configs],
        "results": [fsha(p) for p in results],
        "summaries": [
            fsha(path.parent / f"round-{round_index}/published/rank-{rank}/summary.json")
            for path in results
            for round_index in range(2)
            for rank in range(2)
        ],
    }
    receipt = Receipt(w=0, commit_hash=sha256_hex(canonicalize(evidence)), received_round=1)
    raw = canonicalize(envelope_v2.seal(service.OWNER, "Receipt", manifest.run_id(), receipt, 100))
    store._cpu_network = network
    yield store, manifest, tuple(configs), tuple(results), raw
    store.close_v2_notifications()
    store._db.close()


def register(store, manifest):
    signed = envelope_v2.seal(service.OWNER, "RunManifestV2", manifest.run_id(), manifest, 100)
    store._db.execute(
        "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
        (manifest.run_id(), manifest.model_dump_json(), json.dumps(signed)),
    )


def saved_geometry(manifest, hotkey, directory, *, published=False):
    """Complete commitment fixture, not a trained trajectory or CUDA measurement."""
    import torch

    from hypertrain.auditor.replay import AnchorCache, pack_state, tensor_root
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.trainer.compress import compress, state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.island import IslandAssignment
    from hypertrain.trainer.loop import _make_leaf
    from hypertrain.trainer.model import init_params

    cfg = TrainConfig.from_manifest_v2(manifest)
    theta = init_params(cfg.model)
    anchor = AnchorCache().genesis(manifest, hotkey, theta)
    count = manifest.training.batch_samples()
    samples = [
        b"".join(
            ((i + j) % cfg.model.vocab).to_bytes(4, "little") for j in range(cfg.model.seq_len + 1)
        )
        for i in range(count)
    ]
    sample_tree = MerkleTree(samples)
    inputs = {
        "start_state": pack_state(theta, anchor.state),
        "ef_in": pack_state(anchor.ef),
        "v0": pack_state({}),
        "samples": b"".join(samples),
        "sample_proofs": canonicalize(
            [[p.hex() for p in sample_tree.proof(i)] for i in range(count)]
        ),
    }
    directory.mkdir(parents=True, exist_ok=True)
    for name, raw in inputs.items():
        (directory / name).write_bytes(raw)
    job = IslandJobV1(
        job_version=1,
        run_id=manifest.run_id(),
        w=0,
        manifest=manifest,
        sample_ids=list(range(count)),
        global_step0=0,
        start_state_sha256=sha256_hex(inputs["start_state"]),
        ef_in_sha256=sha256_hex(inputs["ef_in"]),
        v0_sha256=sha256_hex(inputs["v0"]),
        object_paths={name: name for name in inputs},
        deadline=1,
    )
    if published:
        publication = directory / "published"
        publication.mkdir()
        for name, raw in inputs.items():
            (publication / name).write_bytes(raw)
        assignment = IslandAssignment(
            job.run_id,
            job.w,
            tuple(job.sample_ids),
            0,
            manifest.training.reference_spec.layout.n_gpus,
        )
        leaves, checkpoints = [], {}
        for step in range(0, cfg.inner.H + 1, cfg.inner.J):
            state = replace(anchor.state, step=step)
            leaves.append(_make_leaf(cfg, assignment, step, theta, state, 0.0, 0.0).preimage)
            checkpoints[step] = pack_state(theta, state)
        delta, ef = compress(
            cfg.compress, {k: torch.zeros_like(x) for k, x in theta.items()}, anchor.ef
        )
        final = replace(anchor.state, step=cfg.inner.H)
        commitments = {
            "leaves_root": MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex(),
            "final_theta_hash": state_hash(theta),
            "state_root": tensor_root(theta, final),
            "ef_out_hash": state_hash(ef),
            "delta_hash": sha256_hex(delta),
            "leaves": [p.digest() for p in leaves],
        }
        for rank in range(manifest.training.reference_spec.layout.n_gpus):
            target = publication / f"rank-{rank}"
            (target / "checkpoints").mkdir(parents=True)
            for step, raw in checkpoints.items():
                (target / "checkpoints" / f"{step}.safetensors").write_bytes(raw)
            values = {
                "state.safetensors": pack_state(theta, final),
                "ef.safetensors": pack_state(ef),
                "delta.bin": delta,
                "leaves.json": canonicalize([p.model_dump(mode="json") for p in leaves]),
                "trace.json": canonicalize(
                    [
                        {
                            "rank": rank,
                            "step": 1,
                            "microbatch": 0,
                            "layer": 0,
                            "op": "fixture.identity",
                            "shape": [1],
                            "dtype": "torch.float32",
                            "sha256": sha256_hex(b"backend-dispatch-metadata"),
                        }
                    ]
                ),
            }
            for name, raw in values.items():
                (target / name).write_bytes(raw)
            (target / "summary.json").write_bytes(
                canonicalize(
                    {
                        "job_hash": job.digest(),
                        "backend": "cpu",
                        "optimizer_step": cfg.inner.H,
                        "commitments": commitments,
                        "work": {
                            "rank": rank,
                            "fingerprint": sha256_hex(canonicalize([job.digest(), rank])),
                            "elapsed_ns": 0,
                            "allocated_bytes": 0,
                            "reserved_bytes": 0,
                            "total_bytes": 1,
                        },
                    }
                )
            )
        validate_artifacts(job, publication)
    return job


def test_reviewed_cuda_overrides_test_economics_and_freezes(authority):
    store, manifest, configs, results, raw = authority
    store._bootstrap_backend_v2(manifest, configs, results, raw)
    register(store, manifest)
    escrow, admission, _ = store._services(manifest.run_id())
    assert admission.backend == "cuda" and escrow.policy.ledger_mode == "test"
    before = escrow.balances()
    store._bootstrap_backend_v2(manifest, configs, results, raw)
    assert escrow.balances() == before
    store._services_v2.clear()
    assert store._services(manifest.run_id())[1].backend == "cuda"


def test_existing_cpu_contract_cannot_promote_after_services(network, authority):
    assert network.store._services(network.manifest.run_id())[1].backend == "cpu"
    store, manifest, configs, results, raw = authority
    register(store, manifest)
    assert store._services(manifest.run_id())[1].backend == "cpu"
    store._services_v2.clear()
    with pytest.raises(ChallengeError, match="precede"):
        store._bootstrap_backend_v2(manifest, configs, results, raw)


@pytest.mark.parametrize("fault", ["owner", "runtime", "artifact", "config", "cpu_summary"])
def test_invalid_reviewed_authority_rejects_without_install(authority, fault):
    store, manifest, configs, results, raw = authority
    if fault == "owner":
        env = envelope_v2.parse_envelope(raw)
        raw = canonicalize(
            envelope_v2.seal(service.COORD, "Receipt", manifest.run_id(), env.body, 100)
        )
    elif fault == "runtime":
        data = json.loads(results[0].read_bytes())
        data["environment"]["drivers"] = ["wrong", "wrong"]
        results[0].write_text(json.dumps(data))
    elif fault == "artifact":
        (results[0].parent / "round-1/published/rank-0/state.safetensors").write_bytes(b"changed")
    elif fault == "cpu_summary":
        (results[0].parent / "round-0/published/rank-0/summary.json").write_text(
            json.dumps({"backend": "cpu"})
        )
    else:
        configs[0].write_bytes(b"{}")
    if fault in ("runtime", "cpu_summary", "config"):
        # Even owner-reviewed bytes must pass runtime/config validation.
        evidence = {
            "run_id": manifest.run_id(),
            "backend": "cuda",
            "reference_hash": sha256_hex(
                canonicalize(manifest.training.reference_spec.model_dump(mode="json"))
            ),
            "layout_hash": manifest.training.reference_spec.layout.model_dump_json(),
            "configs": [fsha(p) for p in configs],
            "results": [fsha(p) for p in results],
            "summaries": [
                fsha(path.parent / f"round-{w}/published/rank-{rank}/summary.json")
                for path in results
                for w in range(2)
                for rank in range(2)
            ],
        }
        receipt = Receipt(w=0, commit_hash=sha256_hex(canonicalize(evidence)), received_round=1)
        raw = canonicalize(
            envelope_v2.seal(service.OWNER, "Receipt", manifest.run_id(), receipt, 100)
        )
    with pytest.raises((ChallengeError, ValueError, Reject)):
        store._bootstrap_backend_v2(manifest, configs, results, raw)
    assert (
        store._db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind='qualification'").fetchone()[
            0
        ]
        == 0
    )


def test_conflicting_reviewed_record_rejects(authority):
    store, manifest, configs, results, raw = authority
    store._bootstrap_backend_v2(manifest, configs, results, raw)
    data = json.loads(results[0].read_bytes())
    data["elapsed_seconds"] = 2
    results[0].write_text(json.dumps(data))
    record = store._record_v2(
        manifest.run_id(),
        "qualification",
        manifest.training.reference_spec.image_digest,
    )
    evidence = envelope_v2.load_json(store.objects.get(record["authority_hash"]))
    evidence["results"] = [fsha(p) for p in results]
    receipt = Receipt(w=0, commit_hash=sha256_hex(canonicalize(evidence)), received_round=1)
    raw = canonicalize(envelope_v2.seal(service.OWNER, "Receipt", manifest.run_id(), receipt, 100))
    with pytest.raises(ChallengeError, match="conflicts"):
        store._bootstrap_backend_v2(manifest, configs, results, raw)
    assert (
        store._record_v2(
            manifest.run_id(),
            "qualification",
            manifest.training.reference_spec.image_digest,
        )
        == record
    )


def test_changed_authority_and_cpu_anchor_reject(authority):
    from hypertrain.auditor.replay import AnchorCache
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    store, manifest, configs, results, raw = authority
    store._bootstrap_backend_v2(manifest, configs, results, raw)
    register(store, manifest)
    assert store._backend_v2(manifest) == "cuda"
    cache = AnchorCache()
    anchor = cache.genesis(
        manifest,
        service.HOT[0].ss58,
        init_params(TrainConfig.from_manifest_v2(manifest).model),
    )
    store._persist_anchor_v2(manifest, anchor, cache)
    assert (
        store._restore_anchor_v2(manifest.run_id(), anchor.hotkey, -1, AnchorCache()).backend
        == "genesis"
    )
    with pytest.raises(ChallengeError, match="anchor backend"):
        store._persist_anchor_v2(manifest, replace(anchor, backend="cpu", w=0), cache)
    store._db.execute(
        "UPDATE records_v2 SET data=json_set(data,'$.backend','cpu') WHERE kind='anchor'"
    )
    with pytest.raises(ChallengeError, match="anchor backend"):
        store._restore_anchor_v2(manifest.run_id(), anchor.hotkey, -1, AnchorCache())
    store._db.execute(
        "UPDATE records_v2 SET data=json_set(data,'$.backend','cpu') WHERE kind='qualification'"
    )
    with pytest.raises(ChallengeError, match="authority differs"):
        store._backend_v2(manifest)


def test_production_requires_qualification(authority):
    store, manifest, *_ = authority
    econ = EconomicsPolicyV2.model_validate_json(
        store.objects.get(manifest.network.economics_policy_hash)
    )
    production = econ.model_copy(
        update={"ledger_mode": "production", "genesis_allocation_hash": None}
    )
    body = manifest.body()
    body["network"]["economics_policy_hash"] = store.objects.put(canonicalize(production.body()))
    manifest = RunManifestV2.model_validate(body)
    with pytest.raises(ChallengeError, match="qualification required"):
        store._backend_v2(manifest)


def test_cuda_dispatch_rejects_missing_accepted_round(authority):
    store, manifest, configs, results, raw = authority
    store._bootstrap_backend_v2(manifest, configs, results, raw)
    register(store, manifest)
    before = store._db.total_changes
    with pytest.raises(ChallengeError, match="missing accepted round: 0"):
        store.require_roster_v2(manifest.run_id(), 0)
    assert store._db.total_changes == before


@pytest.mark.parametrize("lease_state", ["live", "expired", "cancelled"])
def test_cuda_dispatch_at_admission_audit_and_geometry(authority, monkeypatch, lease_state):
    from hypertrain.auditor import worker
    from hypertrain.challenge import admission as admission_module
    from hypertrain.miner import island_launch
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.messages import LeafPreimage
    from hypertrain.protocol.messages_v2 import (
        AuditChallengeV2,
        AuditJobV2,
        CommitV2,
        HardwareHint,
        PolicyHashes,
        RoundOpenV2,
        StartStateV2,
    )

    store, manifest, configs, results, raw = authority
    if hasattr(store, "_genuine_index"):
        from hypertrain.auditor.replay import AuditInputError

        case = store._genuine_index.cases["audit:" + lease_state]
        if "beacon" in case:
            signed_beacon = store._genuine_files[case["beacon"]].read_bytes()
            number = json.loads(signed_beacon)["round"]
            store.clock = lambda: manifest.training.beacon.genesis_time + (number - 1) * 3
            store.push_beacon(json.loads(signed_beacon))
        joined = store._genuine_files[case["join"]].read_bytes()
        admission = store._services(manifest.run_id())[1]
        accepted = admission.join(joined, now=store._now(store._db), ip_prefix="local")
        lock = store._genuine_files[case["admission_lock"]].read_bytes()
        admission.lock(lock, now=store._now(store._db))
        admission.challenge(accepted.admission_id, now=store._now(store._db))
        calls = []

        def screen(*args, **kwargs):
            calls.append(("admission", kwargs["backend"]))
            raise ChallengeError(409, "admission dispatch sentinel")

        monkeypatch.setattr(admission_module, "screen_work", screen)
        with pytest.raises(ChallengeError, match="admission dispatch sentinel"):
            admission.prepare_reference(accepted.admission_id, now=store._now(store._db))
        geometry = IslandJobV1.model_validate_json(
            store._genuine_files[case["geometry_job"]].read_bytes()
        )
        receipt = store._genuine_files[case["audit_receipt"]].read_bytes()
        lease_id = json.loads(store._genuine_files[case["lease"]].read_bytes())["id"]

        def audit(*args, **kwargs):
            calls.append(("audit", kwargs["backend"]))
            if lease_state == "cancelled":
                args[3].revoke()
            args[3].check()
            return island_launch.launch_island(
                kwargs["publication_job"],
                kwargs["publication_directory"],
                backend=kwargs["backend"],
                trace=True,
                cancel=args[3].cancelled,
            )

        def launch(job, directory, **kwargs):
            calls.append(("geometry", kwargs["backend"]))
            expected_deadline = (
                manifest.training.beacon.genesis_time
                + (json.loads(store._genuine_files[case["lease"]].read_bytes())["expires"] - 1) * 3
            )
            assert job == geometry.model_copy(update={"deadline": expected_deadline})
            assert (
                store._record_v2(manifest.run_id(), "island-job", "0:" + service.HOT[0].ss58)
                == geometry.body()
            )
            raise ChallengeError(409, "dispatch sentinel, no GPU kernels")

        monkeypatch.setattr(worker, "execute_island_audit", audit)
        monkeypatch.setattr(island_launch, "launch_island", launch)
        error = (
            pytest.raises(ChallengeError, match="dispatch sentinel, no GPU kernels")
            if lease_state == "live"
            else pytest.raises((ChallengeError, AuditInputError, ValueError))
        )
        with error:
            store.execute_audit_v2(manifest.run_id(), lease_id, receipt)
        expected = [("admission", "cuda")]
        if lease_state != "expired":
            expected.append(("audit", "cuda"))
        if lease_state == "live":
            expected.append(("geometry", "cuda"))
        assert calls == expected and not store._lease_guards_v2
        return
    store._bootstrap_backend_v2(manifest, configs, results, raw)
    register(store, manifest)
    admission = store._services(manifest.run_id())[1]
    assert admission.backend == "cuda"
    calls = []
    joined = admission.join(
        canonicalize(
            sign_join(
                service.HOT[0],
                service.COLD[0],
                run_id=manifest.run_id(),
                request_id="ab" * 32,
                expires_beacon=100,
                policy_hash=manifest.network.admission_policy_hash,
                hardware_hint=HardwareHint(
                    device_name="advisory", device_count=2, driver="advisory"
                ),
            ).body()
        ),
        now=1,
        ip_prefix="local",
    )
    store._db.execute("INSERT INTO beacon VALUES(?,?,?,?)", (2, "00" * 96, "00" * 32, 0))
    admission.challenge(joined.admission_id, now=2)
    seed = IslandJobV1.model_validate_json(
        gpu.driver.Qualification.model_validate_json(configs[0].read_bytes()).seed_job.read_bytes()
    )
    monkeypatch.setattr(
        admission,
        "stage_reference",
        lambda challenge, samples, epoch: (
            seed.model_copy(update={"w": epoch, "sample_ids": list(samples)}),
            store.state_dir,
        ),
    )

    def screen(*args, **kwargs):
        calls.append(("admission", kwargs["backend"]))
        raise ChallengeError(409, "admission dispatch sentinel")

    monkeypatch.setattr(admission_module, "screen_work", screen)
    with pytest.raises(ChallengeError, match="admission dispatch sentinel"):
        admission.prepare_reference(joined.admission_id, now=2)
    hot, auditor = service.HOT[0], service.AUDITORS[0]
    run = manifest.run_id()
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    anchor = AnchorCache().genesis(manifest, hot.ss58, theta)
    proposed_start = StartStateV2(
        run_id=run,
        w=0,
        hotkey=hot.ss58,
        theta_hash=state_hash(theta),
        state_object_sha256=sha256_hex(pack_state(theta, anchor.state)),
        opt_state_hash=optimizer_hash(anchor.state),
        ef_object_sha256=sha256_hex(pack_state(anchor.ef)),
        ef_hash=state_hash(anchor.ef),
        parent_anchor_hash=anchor.anchor_hash,
        global_step0=0,
        anchor_verdict_hash=anchor.proof_hash,
    )
    status = admission.status(hot.ss58, now=2)
    roster = [
        {
            "hotkey": hot.ss58,
            "slot": 0,
            "q_i": "0000803f",
            "admission_id": status.record.admission_id,
            "coldkey_group": status.record.coldkey,
            "state": status.record.state,
            "eligible_weight": 4194304,
        }
    ]
    opening = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=3,
        d_assign=4,
        d_commit=30,
        d_audit=31,
        d_upload=32,
        d_final=34,
        contract_version=2,
        registry_epoch=0,
        audit_mode="anchored-full",
        roster=roster,
        policy_hashes=PolicyHashes.model_validate(
            {name: getattr(manifest.network, name) for name in PolicyHashes.model_fields}
        ),
        start_state_index_hash=sha256_hex(canonicalize([proposed_start.body()])),
    )
    store.open_round_v2(
        run,
        canonicalize(envelope_v2.seal(service.COORD, "RoundOpenV2", run, opening, 100)),
    )
    h = store.objects.put(b"input")
    start = StartStateV2(
        run_id=run,
        w=0,
        hotkey=hot.ss58,
        theta_hash=h,
        state_object_sha256=h,
        opt_state_hash=h,
        ef_object_sha256=h,
        ef_hash=h,
        parent_anchor_hash=h,
        global_step0=0,
        anchor_verdict_hash=h,
    )
    commit = CommitV2(
        w=0,
        hotkey=hot.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=7,
        leaves_root=h,
        metrics_root=h,
        final_theta_hash=h,
        ef_in_hash=h,
        ef_out_hash=h,
        delta_hash=h,
        delta_bytes=1,
        tokens=manifest.training.batch_samples() * manifest.training.model.seq_len,
    )
    challenge = AuditChallengeV2(
        w=0,
        target=hot.ss58,
        beacon_round=1,
        beacon_sig_sha256=h,
        mode="full",
        segments=[],
        reasons=["final"],
        serve_deadline=100,
        anchor_hash=start.digest(),
        audit_mode="anchored-full",
    )
    job = AuditJobV2(
        run_id=run,
        job_id=h,
        auditor_id=auditor.ss58,
        attempt=1,
        lease_nonce=h,
        lease_expires=50,
        absolute_deadline=100,
        reservation_id=h,
        replay_step_budget=30,
        anchor_age=0,
        manifest=manifest,
        challenge_envelope=envelope_v2.seal(service.COORD, "AuditChallengeV2", run, challenge, 100),
        commit_envelope=envelope_v2.seal(hot, "CommitV2", run, commit, 100),
        sample_ids=list(range(manifest.training.batch_samples())),
        start_state=start,
        preimages=[
            LeafPreimage(
                run_id=run,
                w=0,
                t=t,
                stages=[dict(theta=h, m=h, v=h)],
                batch_ids_sha256=h,
                rng_ctr=0,
                loss_f32="00000000",
                norm_f32="00000000",
            )
            for t in range(0, 31, 5)
        ],
        ef_in=dict(sha256=h, size=5),
        v0=dict(sha256=h, size=5),
        created_beacon=1,
    )
    geometry = saved_geometry(manifest, hot.ss58, store.state_dir / "jobs-v2/0" / hot.ss58)
    with store._tx():
        store._db.execute(
            "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,challenge,created,absolute,"
            "state,attempts,auditor,nonce,expires) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            (h, run, 0, hot.ss58, "{}", 1, 100, "running", 1, auditor.ss58, h, 50),
        )
        store._put_record_v2(run, "audit-job", h, job.body())
        store._put_record_v2(run, "island-job", "0:" + hot.ss58, geometry.body())
        samples = store.objects.put(bytes(60 * 17 * 2))
        proofs = store.objects.put(canonicalize([[]] * 60))
        store._put_record_v2(
            run, "dataset", "inputs", {"samples_hash": samples, "proofs_hash": proofs}
        )
    monkeypatch.setattr(store, "_fresh_v2", lambda *args: 50 if lease_state == "expired" else 1)
    monkeypatch.setattr(store, "_restore_anchor_v2", lambda *args: None)

    def audit(*args, **kwargs):
        calls.append(("audit", kwargs["backend"]))
        assert kwargs["deadline_unix"] == manifest.training.beacon.genesis_time + 49 * 3
        if lease_state == "cancelled":
            args[3].revoke()
        args[3].check()
        return island_launch.launch_island(
            kwargs["publication_job"],
            kwargs["publication_directory"],
            backend=kwargs["backend"],
            trace=True,
            cancel=args[3].cancelled,
        )

    def launch(*args, **kwargs):
        calls.append(("geometry", kwargs["backend"]))
        assert args[0] == geometry.model_copy(
            update={"deadline": manifest.training.beacon.genesis_time + 49 * 3}
        )
        assert store._record_v2(run, "island-job", "0:" + hot.ss58) == geometry.body()
        raise ChallengeError(409, "dispatch sentinel, no GPU kernels")

    monkeypatch.setattr(worker, "execute_island_audit", audit)
    monkeypatch.setattr(island_launch, "launch_island", launch)
    receipt = envelope_v2.seal(
        auditor, "Receipt", run, Receipt(w=0, commit_hash=h, received_round=1), 100
    )
    from hypertrain.auditor.replay import AuditInputError

    expected_error = (
        pytest.raises(ChallengeError, match="dispatch sentinel, no GPU kernels")
        if lease_state == "live"
        else pytest.raises((ChallengeError, AuditInputError, ValueError))
    )
    with expected_error:
        store.execute_audit_v2(run, h, canonicalize(receipt))
    expected = [("admission", "cuda")]
    if lease_state != "expired":
        expected.append(("audit", "cuda"))
    if lease_state == "live":
        expected.append(("geometry", "cuda"))
    assert calls == expected
    assert not store._lease_guards_v2


@pytest.mark.parametrize("mode", ["cuda", "cpu", "cpu_anchor", "missing", "mutated"])
def test_referee_uses_frozen_backend_before_evidence(authority, monkeypatch, mode):
    from hypertrain.auditor.replay import AnchorCache
    from hypertrain.challenge.disputes_v2 import Contest, Turn
    from hypertrain.miner import island_launch
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    if mode == "cpu":
        import os

        network = authority[0]._cpu_network

        from hypertrain.protocol.messages_v2 import DisputeV2, EscrowLock

        snapshot = Path(os.environ["HT_NETWORK_VERIFIED_SNAPSHOT"])
        assert (snapshot / "challenge.db").is_file()
        rounds = module(
            "cpu_referee_verified_round",
            TREE / "tests/e2e/test_service_network_v2_e2e.py",
        )
        calls = []

        def check_referee(current):
            store, manifest = current.store, current.manifest
            run = manifest.run_id()
            assert store._backend_v2(manifest) == "cpu"
            assert store._record_v2(run, "execution-backend", "run")["authority_hash"] is None
            opening = store._record_v2(run, "round", "0")
            assert envelope_v2.verify_envelope(opening)
            assert opening["signer"] == service.COORD.ss58
            assert store.require_roster_v2(run, 0) is None
            hot = service.HOT[0]
            row = store._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='contest' "
                "AND json_extract(data,'$.miner')=?",
                (run, hot.ss58),
            ).fetchone()
            contest = Contest.model_validate_json(row[0])
            h = sha256_hex(canonicalize([run, contest.verdict_hash, hot.ss58]))
            origin = store._db.execute(
                "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' "
                "AND units>=100",
                (service.COLD[0].ss58,),
            ).fetchone()[0]
            lock = EscrowLock(
                operation_id="cd" * 32,
                owner=service.COLD[0].ss58,
                units=100,
                origin_ids=[origin],
                admission_id=None,
                dispute_id=h,
                kind="LOCK_CONTEST",
            )
            response = current.client.post(
                current.url + "/escrow/lock",
                json=current.signed(service.COLD[0], "EscrowLock", lock),
            )
            assert response.status_code == 200, response.text
            response = current.client.post(
                current.url + "/dispute",
                json=current.signed(
                    hot,
                    "DisputeV2",
                    DisputeV2(
                        hotkey=hot.ss58,
                        verdict_hash=contest.verdict_hash,
                        action="contest",
                        bond_lock=lock.operation_id,
                    ),
                ),
            )
            assert response.status_code == 200, response.text
            disputes = store._services(run)[2]
            turn = disputes.get(h)
            geometry = store._record_v2(run, "trace-geometry", contest.verdict_hash)
            before = store._db.execute("SELECT COUNT(*) FROM dispute_evidence_v2").fetchone()[0]

            def launch(job, directory, **kwargs):
                calls.append(kwargs["backend"])
                assert kwargs["backend"] == "cpu" and kwargs["trace"] is True
                record = store._record_v2(run, "referee-job", h + ":" + turn.transcript_hash)
                assert envelope_v2.verify_envelope(record["receipt"])
                assert record["receipt"]["signer"] == service.COORD.ss58
                descriptor = envelope_v2.load_json(store.objects.get(record["descriptor_hash"]))
                assert descriptor["job"] == job.body()
                assert descriptor["backend"] == "cpu"
                assert descriptor["historical_geometry_hash"] == sha256_hex(canonicalize(geometry))
                assert directory == store.state_dir / "referee-v2" / h / turn.transcript_hash
                raise ChallengeError(409, "referee dispatch sentinel")

            monkeypatch.setattr(island_launch, "launch_island", launch)
            with pytest.raises(ChallengeError, match="referee dispatch sentinel"):
                store.referee_v2(run, h)
            assert calls == ["cpu"]
            assert disputes.get(h) == turn
            assert (
                store._db.execute("SELECT COUNT(*) FROM dispute_evidence_v2").fetchone()[0]
                == before
            )
            assert store._record_v2(run, "trace-geometry", contest.verdict_hash) == geometry
            assert not store._lease_guards_v2

        monkeypatch.setattr(rounds, "_funded_dispute_watch", check_referee)
        rounds.test_four_actual_funded_identities_graduate_shadow_only(network, monkeypatch)
        assert calls == ["cpu"]
        return

    store, manifest, configs, results, raw = authority
    if hasattr(store, "_genuine_index"):
        case = store._genuine_index.cases["referee"]
        h = json.loads(store._genuine_files[case["turn"]].read_bytes())["dispute_id"]
        disputes = store._services(manifest.run_id())[2]
        turn = disputes.get(h)
        geometry = store._record_v2(manifest.run_id(), "trace-geometry", turn.contest.verdict_hash)
        before = store._db.execute("SELECT COUNT(*) FROM dispute_evidence_v2").fetchone()[0]
        if mode == "cpu_anchor":
            store._db.execute(
                "UPDATE records_v2 SET data=json_set(data,'$.backend','cpu') WHERE kind='anchor'"
            )
        elif mode == "missing":
            store._db.execute("DELETE FROM records_v2 WHERE kind='qualification'")
        elif mode == "mutated":
            record = store._record_v2(
                manifest.run_id(),
                "qualification",
                manifest.training.reference_spec.image_digest,
            )
            env = envelope_v2.parse_envelope(record["receipt"])
            record["receipt"] = envelope_v2.seal(
                service.COORD, "Receipt", manifest.run_id(), env.body, env.exp_drand
            )
            store._put_record_v2(
                manifest.run_id(),
                "qualification",
                manifest.training.reference_spec.image_digest,
                record,
            )
        calls = []

        def launch(job, directory, **kwargs):
            calls.append(kwargs["backend"])
            assert kwargs["backend"] == "cuda" and kwargs["trace"] is True
            raise ChallengeError(409, "referee dispatch sentinel")

        monkeypatch.setattr(island_launch, "launch_island", launch)
        error = (
            pytest.raises(ChallengeError, match="referee dispatch sentinel")
            if mode == "cuda"
            else pytest.raises(ChallengeError)
        )
        with error:
            store.referee_v2(manifest.run_id(), h)
        assert calls == (["cuda"] if mode == "cuda" else [])
        assert disputes.get(h) == turn
        assert (
            store._record_v2(manifest.run_id(), "trace-geometry", turn.contest.verdict_hash)
            == geometry
        )
        assert store._db.execute("SELECT COUNT(*) FROM dispute_evidence_v2").fetchone()[0] == before
        assert not store._lease_guards_v2
        return
    if mode != "cpu":
        store._bootstrap_backend_v2(manifest, configs, results, raw)
    register(store, manifest)
    _, _, disputes = store._services(manifest.run_id())
    cache = AnchorCache()
    hot = service.HOT[0].ss58
    anchor = cache.genesis(manifest, hot, init_params(TrainConfig.from_manifest_v2(manifest).model))
    store._persist_anchor_v2(manifest, anchor, cache)
    h = "bc" * 32
    contest = Contest(
        verdict_hash=h,
        challenge_hash=h,
        w=0,
        miner=hot,
        auditor=service.AUDITORS[0].ss58,
        referee=service.REFEREE.ss58,
        step_span=30,
        layer_span=1,
        op_span=1,
    )
    turn = Turn(
        run_id=manifest.run_id(),
        dispute_id=h,
        contest=contest,
        seq=0,
        level="step",
        ctx=[],
        interval=(0, 30),
        expected_party=hot,
        transcript_hash=h,
        turn_deadline=50,
        absolute_deadline=100,
        lock_id=h,
    )
    original = store.state_dir / "claim"
    job = saved_geometry(manifest, hot, original, published=True)
    with store._tx():
        store._db.execute(
            "INSERT INTO disputes_v2 VALUES(?,?,?,?,?)",
            (h, h, hot, turn.model_dump_json(), "{}"),
        )
        store._put_record_v2(
            manifest.run_id(),
            "trace-geometry",
            h,
            {"job": job.body(), "directory": str(original / "published")},
        )
        store._put_record_v2(
            manifest.run_id(), "verdict", "0:" + hot, {"body": {"result": "MATCH"}}
        )
    if mode == "cpu_anchor":
        store._db.execute(
            "UPDATE records_v2 SET data=json_set(data,'$.backend','cpu') WHERE kind='anchor'"
        )
    elif mode == "missing":
        store._db.execute("DELETE FROM records_v2 WHERE kind='qualification'")
    elif mode == "mutated":
        record = store._record_v2(
            manifest.run_id(),
            "qualification",
            manifest.training.reference_spec.image_digest,
        )
        env = envelope_v2.parse_envelope(record["receipt"])
        record["receipt"] = envelope_v2.seal(
            service.COORD, "Receipt", manifest.run_id(), env.body, env.exp_drand
        )
        store._put_record_v2(
            manifest.run_id(),
            "qualification",
            manifest.training.reference_spec.image_digest,
            record,
        )
    monkeypatch.setattr(store, "_snapshot_current_v2", lambda _: None)
    calls = []

    def launch(*args, **kwargs):
        calls.append(kwargs["backend"])
        raise ChallengeError(409, "referee dispatch sentinel")

    monkeypatch.setattr(island_launch, "launch_island", launch)
    expected_error = (
        pytest.raises(ChallengeError, match="referee dispatch sentinel")
        if mode in ("cuda", "cpu")
        else pytest.raises(ChallengeError)
    )
    with expected_error:
        store.referee_v2(manifest.run_id(), h)
    assert calls == ([mode] if mode in ("cuda", "cpu") else [])
    assert disputes.get(h) == turn
    assert store._db.execute("SELECT COUNT(*) FROM dispute_evidence_v2").fetchone()[0] == 0
