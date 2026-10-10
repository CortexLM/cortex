"""Network-v2 boundary and custody regression checks; no training, clocks or sleeps."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest
from pydantic import JsonValue, ValidationError
from test_od_fields import od_body

from hypertrain.aggregator.core import (
    replay_tape,
    tape_bytes,
    tape_message,
    verify_tape,
)
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol import envelope as envelope_v1
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.envelope import (
    ExpiredError,
    MalformedEnvelope,
    ReplayError,
    RunMismatchError,
    SignatureError,
    UnauthorizedSigner,
)
from hypertrain.protocol.envelope import signing_message as v1_signing_message
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import LeafPreimage, Receipt, RunManifest
from hypertrain.protocol.messages_v2 import (
    AcceptV2,
    AggregationPolicyV2,
    ArtifactRef,
    AuditChallengeV2,
    AuditJobV2,
    BisectV2,
    CommitV2,
    DisputeV2,
    EconomicsPolicyV2,
    EscrowLock,
    EscrowRelease,
    EscrowTransfer,
    IslandJobV1,
    JoinRequest,
    RosterEntryV2,
    RunManifestV2,
    StartStateV2,
    WeightAllocationV2,
    WorkProof,
)
from hypertrain.protocol.relay_messages import (
    AcceptedUploadAck,
    ChunkCustodyAck,
    CustodyRelease,
    RelayReceipt,
    RetentionExtension,
    RetentionExtensionAck,
    RetrievalRequest,
    RetrievalResponse,
    UploadChunkManifest,
    UploadGrant,
    extend_retention,
    has_full_custody,
    validate_release,
)
from hypertrain.protocol.schema_v2 import render, write

RUN = "ab" * 32
H = "11" * 32
MINER = Keypair(bytes([7]) * 32)
COLD = Keypair(bytes([8]) * 32)


def wrapper(training: dict[str, JsonValue] | None = None) -> RunManifestV2:
    body = training or example_manifest().body()
    if training is None:
        body["model"]["param_count"] = 4_329_216
        body["reference_spec"]["driver_allowlist"] = ["580"]
    return RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": body,
            "network": {
                **{
                    name: H
                    for name in (
                        "admission_policy_hash",
                        "economics_policy_hash",
                        "aggregation_policy_hash",
                        "dispute_policy_hash",
                        "audit_policy_hash",
                        "relay_registry_hash",
                    )
                },
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )


def commit(tokens: int = 7680) -> CommitV2:
    return CommitV2(
        w=3,
        hotkey=MINER.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=7,
        leaves_root=H,
        metrics_root=H,
        final_theta_hash=H,
        ef_in_hash=H,
        ef_out_hash=H,
        delta_hash=H,
        delta_bytes=1024,
        tokens=tokens,
    )


@pytest.mark.parametrize("n", [1, 2, 4, 8])
@pytest.mark.parametrize("objective", ["mlm", "decision", "distill"])
@pytest.mark.parametrize("zero1", [False, True])
def test_od_general_n_when_v2(n: int, objective: str, zero1: bool) -> None:
    # Given
    body = od_body()
    body["model"]["param_count"] = 116_288
    body["inner"].update(H=1, J=1, micro_batch=1, grad_accum=1)
    body["reference_spec"]["layout"].update(n_gpus=n, dp_size=n, ep_size=1, zero1=zero1)
    if objective != "mlm":
        rec = {"state_len": 8, "n_questions": 1, "n_options": 2, "opt_len": 2, "instr_len": 2}
        body["model"]["od"].update(objective=objective, record=rec)
        body["model"].update(seq_len=18, param_count=187_140)
    # When
    model = wrapper(body)
    # Then
    assert model.training.batch_samples() == n
    assert model.run_id() != model.training.training_hash()
    if n != 1 or zero1:
        with pytest.raises(ValidationError):
            RunManifest.model_validate(body)


def test_assign_unit_when_only_island_batch_is_divisible() -> None:
    # Given
    body = od_body()
    body["model"]["param_count"] = 116_288
    body["inner"].update(H=1, J=1, micro_batch=1, grad_accum=1)
    body["reference_spec"]["layout"].update(n_gpus=8, dp_size=8)
    body["dataset"].update(assign_unit=8, unit_sha256_root=H, n_samples=4096)
    # When
    model = wrapper(body)
    # Then
    assert model.training.batch_samples() == 8
    with pytest.raises(ValidationError):
        RunManifest.model_validate(body)


@pytest.mark.parametrize("field,value", [("param_count", 1), ("d_ff", 128), ("n_kv_heads", 2)])
def test_od_invalid_geometry_when_v2(field: str, value: int) -> None:
    body = od_body()  # Given
    body["model"][field] = value
    with pytest.raises(ValidationError):  # When / Then
        wrapper(body)


@pytest.mark.parametrize("n,ep,pp", [(4, 2, 1), (4, 1, 2), (3, 1, 1)])
def test_invalid_layout_when_v2(n: int, ep: int, pp: int) -> None:
    body = od_body()  # Given
    body["reference_spec"]["layout"].update(n_gpus=n, dp_size=4, ep_size=ep, pp=pp)
    with pytest.raises(ValidationError):  # When / Then
        wrapper(body)


def test_wrapper_when_training_run_id_requested() -> None:
    model = wrapper()  # Given
    with pytest.raises(ValueError):  # When / Then
        model.training.run_id()
    assert model.run_id() == sha256_hex(canonicalize(model.body(), allow_float=False))


def test_commit_when_token_entitlement_or_leaf_count_differs() -> None:
    manifest = wrapper()  # Given
    data = commit().model_dump(mode="json")
    inner = manifest.training.inner
    samples = manifest.training.batch_samples()
    data.update(tokens=samples * manifest.training.model.seq_len, n_leaves=inner.H // inner.J + 1)
    # When / Then
    CommitV2.model_validate(data).validate_assignment(manifest, samples)
    data["tokens"] += 1
    with pytest.raises(ValueError):
        CommitV2.model_validate(data).validate_assignment(manifest, samples)


def test_accept_when_claimed_rank_count_differs() -> None:
    manifest = wrapper()  # Given
    ref = manifest.training.reference_spec
    data = dict(
        w=1,
        hotkey=MINER.ss58,
        assignment_hash=H,
        image_digest=ref.image_digest,
        driver_version=ref.driver_allowlist[0],
        n_gpus=ref.layout.n_gpus,
        work_screen_hash=H,
    )
    # When / Then
    AcceptV2.model_validate(data).validate_manifest(manifest)
    with pytest.raises(ValueError):
        wrong = AcceptV2.model_validate({**data, "n_gpus": ref.layout.n_gpus + 1})
        wrong.validate_manifest(manifest)


@pytest.mark.parametrize(
    "change",
    [
        {"w": True},
        {"tokens": "1"},
        {"tokens": -1},
        {"tokens": 1.0},
        {"delta_bytes": 2**31 + 1},
        {"extra": 1},
        {"tokens": 2**53},
    ],
)
def test_strict_body_when_invalid_number(change: dict[str, JsonValue]) -> None:
    data = commit().model_dump(mode="json")  # Given
    data.update(change)
    with pytest.raises(MalformedEnvelope):  # When / Then
        envelope_v2.seal(MINER, "CommitV2", RUN, data, 100)


def test_exact_preimage_when_sealed() -> None:
    body = commit().model_dump(mode="json")  # Given
    expected = b"hypertrain/2|CommitV2|" + sha256_hex(canonicalize(body)).encode()
    # When / Then
    assert envelope_v2.signing_message("CommitV2", body, 100, RUN) == (
        expected + b"|100|" + RUN.encode()
    )
    assert v1_signing_message("Commit", body, 100, RUN) == (
        b"hypertrain/1|Commit|" + sha256_hex(canonicalize(body)).encode() + b"|100|" + RUN.encode()
    )


def test_replay_when_same_body_or_conflicting_body_after_restart() -> None:
    intake = envelope_v2.Intake(RUN)  # Given
    first = envelope_v2.seal(MINER, "CommitV2", RUN, commit(), 100)
    intake.accept(first, 50)
    restarted = envelope_v2.Intake(RUN, reservations=dict(intake.reservations))
    # When
    duplicate = restarted.accept(envelope_v2.seal(MINER, "CommitV2", RUN, commit(), 101), 50)
    # Then
    assert duplicate == commit() and len(restarted.reservations) == 1
    with pytest.raises(ReplayError):
        restarted.accept(envelope_v2.seal(MINER, "CommitV2", RUN, commit(1), 100), 50)


def test_workproof_when_consecutive_accepted_commit_subjects_share_admission() -> None:
    intake = envelope_v2.Intake(RUN)
    commits = [commit().model_copy(update={"w": w}) for w in (0, 1)]
    proofs = []
    for model in commits:
        accepted = intake.accept(envelope_v2.seal(MINER, "CommitV2", RUN, model, 100), 50)
        proof = WorkProof(
            admission_id=H,
            challenge_hash=envelope_v1.body_digest(accepted.model_dump(mode="json")),
            leaves_root=model.leaves_root,
            delta_hash=model.delta_hash,
            artifact_refs=[ArtifactRef(sha256=H, size=1)],
        )
        assert intake.accept(envelope_v2.seal(MINER, "WorkProof", RUN, proof, 100), 50) == proof
        assert envelope_v2.replay_key("WorkProof", RUN, MINER.ss58, proof) == (
            RUN,
            "WorkProof",
            MINER.ss58,
            H,
            proof.challenge_hash,
        )
        proofs.append(proof)
    restarted = envelope_v2.Intake(RUN, reservations=dict(intake.reservations))
    for proof in proofs:
        assert restarted.accept(envelope_v2.seal(MINER, "WorkProof", RUN, proof, 101), 50) == proof
    assert len(restarted.reservations) == 4
    # Proof payload is not nonce material; same subject retains conflict detection.
    for change in (
        {"leaves_root": "22" * 32},
        {"delta_hash": "22" * 32},
        {"artifact_refs": [ArtifactRef(sha256="22" * 32, size=2)]},
    ):
        with pytest.raises(ReplayError):
            restarted.accept(
                envelope_v2.seal(MINER, "WorkProof", RUN, proofs[1].model_copy(update=change), 100),
                50,
            )
    with pytest.raises(RunMismatchError):
        envelope_v2.Intake("22" * 32).accept(
            envelope_v2.seal(MINER, "WorkProof", RUN, proofs[0], 100), 50
        )


def test_workproof_when_distinct_admission_trial_challenges_share_identity() -> None:
    intake = envelope_v2.Intake(RUN)
    proof = WorkProof(
        admission_id=H,
        challenge_hash=H,
        leaves_root=H,
        delta_hash=H,
        artifact_refs=[ArtifactRef(sha256=H, size=1)],
    )
    for subject in (H, "22" * 32):
        current = proof.model_copy(update={"challenge_hash": subject})
        assert intake.accept(envelope_v2.seal(MINER, "WorkProof", RUN, current, 100), 50) == current
    assert len(intake.reservations) == 2


def test_workproof_existing_service_binding_rejects_cross_subject_reuse(tmp_path, request) -> None:
    # Boundary fixture seeds signed accepted commits, not training/admission qualification.
    spec = importlib.util.spec_from_file_location(
        "nonce_service_fixture", Path(__file__).parents[1] / "challenge/test_service_network_v2.py"
    )
    assert spec is not None and spec.loader is not None
    fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = fixture
    spec.loader.exec_module(fixture)
    network_fixture = fixture.network.__wrapped__(tmp_path, request)
    network = next(network_fixture)
    try:
        run = network.manifest.run_id()
        hot = fixture.HOT[0]
        admission_id = network.join().json()["admission_id"]
        proofs, commits = [], []
        for w in (0, 1):
            leaf = LeafPreimage(
                run_id=run,
                w=w,
                t=0,
                stages=[dict(theta=H, m=H, v=H)],
                batch_ids_sha256=H,
                rng_ctr=0,
                loss_f32="00000000",
                norm_f32="00000000",
            )
            raw = canonicalize([leaf.model_dump(mode="json")])
            object_hash = network.store.objects.put(raw)
            from hypertrain.protocol.hashing import MerkleTree

            model = commit().model_copy(
                update={
                    "w": w,
                    "hotkey": hot.ss58,
                    "n_leaves": 1,
                    "leaves_root": MerkleTree([bytes.fromhex(leaf.digest())]).root.hex(),
                }
            )
            with network.store._tx():
                network.store._put_record_v2(
                    run, "commit", f"{w}:{hot.ss58}", network.signed(hot, "CommitV2", model)
                )
            proof = WorkProof(
                admission_id=admission_id,
                challenge_hash=envelope_v1.body_digest(model.model_dump(mode="json")),
                leaves_root=model.leaves_root,
                delta_hash=model.delta_hash,
                artifact_refs=[ArtifactRef(sha256=object_hash, size=len(raw))],
            )
            signed = network.signed(hot, "WorkProof", proof)
            for _ in range(2):
                response = network.client.post(network.url + "/leaves", json=signed)
                assert response.status_code == 200, response.text
            proofs.append(proof)
            commits.append(model)
        conflict = proofs[1].model_copy(update={"delta_hash": "22" * 32})
        response = network.client.post(
            network.url + "/leaves", json=network.signed(hot, "WorkProof", conflict)
        )
        assert response.status_code == 409, response.text
        # New nonce alone is insufficient: subject must name an accepted commit.
        wrong = proofs[1].model_copy(update={"challenge_hash": "33" * 32})
        response = network.client.post(
            network.url + "/leaves", json=network.signed(hot, "WorkProof", wrong)
        )
        assert response.status_code == 422, response.text
        # Old signed proof cannot be rebound to a different run.
        response = network.client.post(
            network.url + "/leaves", json=envelope_v2.seal(hot, "WorkProof", RUN, proofs[0], 10000)
        )
        assert response.status_code == 409, response.text
        # New accepted round with old signed leaf bytes fails existing round/Merkle binding.
        third = commits[1].model_copy(update={"w": 2})
        with network.store._tx():
            network.store._put_record_v2(
                run, "commit", f"2:{hot.ss58}", network.signed(hot, "CommitV2", third)
            )
        rebound = proofs[1].model_copy(
            update={"challenge_hash": envelope_v1.body_digest(third.model_dump(mode="json"))}
        )
        response = network.client.post(
            network.url + "/leaves", json=network.signed(hot, "WorkProof", rebound)
        )
        assert response.status_code == 422, response.text
        assert (
            network.store._db.execute(
                "SELECT COUNT(*) FROM records_v2 WHERE kind='leaves'"
            ).fetchone()[0]
            == 2
        )
    finally:
        network_fixture.close()


def test_signed_boundary_when_run_expiry_role_or_domain_changed() -> None:
    env = envelope_v2.seal(MINER, "CommitV2", RUN, commit(), 100)  # Given
    # When / Then
    assert envelope_v2.verify_envelope(env)
    with pytest.raises(RunMismatchError):
        envelope_v2.Intake(H).accept(env, 50)
    with pytest.raises(ExpiredError):
        envelope_v2.Intake(RUN).accept(env, 101)
    with pytest.raises(UnauthorizedSigner):
        intake = envelope_v2.Intake(RUN, allowed_signers={"CommitV2": lambda s: s == COLD.ss58})
        intake.accept(env, 50)
    altered = {**env, "sig": "0" * 128}
    with pytest.raises(SignatureError):
        envelope_v2.Intake(RUN).accept(altered, 50)
    assert not relay_envelope.verify_envelope(env)
    assert not envelope_v2.verify_envelope({**env, "v": "ht/1"})


@pytest.mark.parametrize(
    "raw",
    [
        '{"a":1,"a":2}',
        '{"body":{"w":1,"w":2}}',
        '{"w":1.0}',
        '{"w":NaN}',
        '{"w":Infinity}',
        "[" * 33 + "0" + "]" * 33,
        '{"value":"' + "x" * 4097 + '"}',
        '{"value":' + str(2**53) + "}",
    ],
)
def test_parser_when_duplicates_floats_or_bounds_exceeded(raw: str) -> None:
    with pytest.raises(MalformedEnvelope):  # Given / When / Then
        envelope_v2.load_json(raw)


def test_parser_when_valid_json_and_unknown_envelope_fields() -> None:
    env = envelope_v2.seal(MINER, "CommitV2", RUN, commit(), 100)  # Given
    raw = json.dumps(env).encode()
    # When / Then
    assert envelope_v2.Intake(RUN).accept(raw, 50) == commit()
    with pytest.raises(MalformedEnvelope):
        envelope_v2.parse_envelope({**env, "unexpected": 1})
    with pytest.raises(MalformedEnvelope):
        envelope_v2.load_json(raw, max_bytes=8)


def test_join_when_both_keys_sign_and_identity_changes() -> None:
    request = JoinRequest(  # Given
        run_id=RUN,
        request_id=H,
        hotkey=MINER.ss58,
        coldkey=COLD.ss58,
        expires_beacon=100,
        policy_hash=H,
        hardware_hint={"device_name": "5090", "device_count": 8, "driver": "580"},
        hot_sig="0" * 128,
        cold_sig="0" * 128,
    )
    msg = envelope_v2.join_signing_message(request)
    data = request.body()
    data.update(hot_sig=MINER.sign(msg).hex(), cold_sig=COLD.sign(msg).hex())
    signed = JoinRequest.model_validate(data)
    # When / Then
    assert envelope_v2.verify_join(signed, RUN, 100)
    assert not envelope_v2.verify_join(signed, H, 100)
    assert not envelope_v2.verify_join(signed, RUN, 101)
    changed = JoinRequest.model_validate({**data, "coldkey": MINER.ss58})
    assert not envelope_v2.verify_join(changed, RUN, 50)


def relay_fixture() -> tuple[
    UploadGrant, UploadChunkManifest, AcceptedUploadAck, list[ChunkCustodyAck]
]:
    manifest = UploadChunkManifest(
        size=(4 << 20) + 8,
        chunks=[
            {"index": 0, "off": 0, "len": 4 << 20, "chunk_sha256": H},
            {"index": 1, "off": 4 << 20, "len": 8, "chunk_sha256": "22" * 32},
        ],
    )
    grant = UploadGrant(
        w=1,
        hotkey=MINER.ss58,
        relay_id="eu",
        assignment_epoch=1,
        delta_hash=H,
        size=manifest.size,
        chunk_manifest_hash=manifest.chunk_manifest_hash(),
        retain_until=500,
        nonce=H,
        exp_drand=100,
    )
    ack = AcceptedUploadAck(
        grant_hash=grant.digest(),
        w=1,
        hotkey=MINER.ss58,
        relay_id="eu",
        assignment_epoch=1,
        delta_hash=H,
        size=grant.size,
        chunk_manifest_hash=grant.chunk_manifest_hash,
        key_id="key1",
        accepted_beacon=10,
        service_deadline=90,
    )
    chunks = [
        ChunkCustodyAck(
            **c.body(),
            upload_ack_hash=ack.digest(),
            grant_hash=grant.digest(),
            durable_chunk_key=f"chunk/{c.index}",
            key_id="key1",
            received_beacon=20,
            retain_until=500,
        )
        for c in manifest.chunks
    ]
    return grant, manifest, ack, chunks


def test_full_custody_when_opportunity_prefix_or_complete_cover() -> None:
    grant, manifest, ack, chunks = relay_fixture()  # Given
    # When / Then
    assert not has_full_custody(grant, manifest, ack, [])
    assert not has_full_custody(grant, manifest, ack, chunks[:1])
    assert has_full_custody(grant, manifest, ack, chunks)
    assert not has_full_custody(grant, manifest, ack, [chunks[0], chunks[0]])
    changed = chunks[1].body()
    changed["key_id"] = "other"
    assert not has_full_custody(
        grant, manifest, ack, [chunks[0], ChunkCustodyAck.model_validate(changed)]
    )
    changed["key_id"] = "key1"
    changed["received_beacon"] = 91
    assert not has_full_custody(
        grant, manifest, ack, [chunks[0], ChunkCustodyAck.model_validate(changed)]
    )


def test_full_custody_when_complete_receipt() -> None:
    grant, manifest, ack, _ = relay_fixture()  # Given
    receipt = RelayReceipt(
        w=1,
        hotkey=MINER.ss58,
        delta_hash=H,
        size=grant.size,
        grant_hash=grant.digest(),
        chunk_manifest_hash=grant.chunk_manifest_hash,
        durable_object_key="objects/test",
        key_id="key1",
        received_drand=30,
        retain_until=500,
    )
    # When / Then
    assert has_full_custody(grant, manifest, ack, [], receipt)
    env = relay_envelope.seal(MINER, "RelayReceipt", RUN, receipt, 100)
    assert relay_envelope.verify_envelope(env)
    assert relay_envelope.Intake(RUN).accept(env, 50) == receipt
    assert not envelope_v2.verify_envelope(env)


def retrieval(requested: int = 101, deadline: int = 111) -> RetrievalRequest:
    return RetrievalRequest(
        request_id=H,
        nonce=H,
        relay_id="eu",
        key_id="key1",
        assignment_hash=H,
        grant_hash=H,
        custody_ack_hashes=[H],
        receipt_hash=None,
        retention_hash=H,
        object_or_chunk_hash=sha256_hex(b"hello"),
        size=5,
        requested_beacon=requested,
        deadline_beacon=deadline,
    )


def test_retrieval_when_after_upload_and_near_retention_expiry() -> None:
    request = retrieval()  # Given: upload cutoff 100, request 101
    # When / Then
    request.validate_horizon(500)
    retrieval(495, 500).validate_horizon(500)
    with pytest.raises(ValueError):
        request.validate_horizon(110)
    with pytest.raises(ValueError):
        request.validate_horizon(500, released=True)
    with pytest.raises(ValidationError):
        retrieval(500, 500)


def test_retrieval_response_when_bytes_are_independently_hashed() -> None:
    request = retrieval()  # Given
    response = RetrievalResponse(
        request_hash=request.digest(),
        nonce=H,
        key_id="key1",
        status="SERVED",
        object_hash=sha256_hex(b"hello"),
        size=5,
        served_beacon=102,
        body_sha256=sha256_hex(b"hello"),
        error_code=None,
    )
    # When / Then
    response.validate_request(request, b"hello")
    with pytest.raises(ValueError):
        response.validate_request(request, b"wrong")


def test_retention_when_extension_ack_before_old_horizon() -> None:
    extension = RetentionExtension(  # Given
        grant_hash=H,
        custody_hashes=[H],
        seq=1,
        previous_retention_hash=H,
        dispute_ids=[H],
        issued_beacon=490,
        retain_until=600,
    )
    ack = RetentionExtensionAck(
        extension_hash=extension.digest(),
        key_id="key1",
        accepted_beacon=499,
    )
    args = dict(
        grant_hash=H,
        custody_hashes=[H],
        key_id="key1",
        previous_hash=H,
        previous_seq=0,
        previous_horizon=500,
    )
    # When / Then
    assert extend_retention(extension, ack, **args) == 600
    late = RetentionExtensionAck(
        extension_hash=extension.digest(),
        key_id="key1",
        accepted_beacon=500,
    )
    with pytest.raises(ValueError):
        extend_retention(extension, late, **args)


def test_release_when_disputes_closed_and_all_horizons_expired() -> None:
    release = CustodyRelease(  # Given
        grant_hash=H,
        custody_hashes=[H],
        retention_hash=H,
        finality_hash=H,
        vesting_beacon=500,
        closed_dispute_root=H,
        release_beacon=520,
    )
    args = dict(
        grant_hash=H,
        custody_hashes=[H],
        retention_hash=H,
        retain_until=600,
        finality_beacon=490,
        epoch_rounds=10,
        retrieval_deadlines=[610],
    )
    # When / Then
    assert validate_release(release, disputes_closed_beacon=510, **args) == 610
    with pytest.raises(ValueError):
        validate_release(release, disputes_closed_beacon=None, **args)


@pytest.mark.parametrize("level,ctx", [("step", []), ("layer", [1]), ("op", [1, 2])])
def test_bisection_when_context_matches_level(level: str, ctx: list[int]) -> None:
    data = dict(
        dispute_id=H,
        seq=0,
        level=level,
        ctx=ctx,
        interval=[0, 8],
        N=2,
        hashes=[H] * 3,
        party=MINER.ss58,
        previous_transcript_hash=H,
    )  # Given
    # When / Then
    assert BisectV2.model_validate(data).ctx == ctx
    with pytest.raises(ValidationError):
        BisectV2.model_validate({**data, "ctx": [1, 2, 3]})
    with pytest.raises(ValidationError):
        BisectV2.model_validate({**data, "N": True})


def test_dispute_when_contest_is_unfunded() -> None:
    with pytest.raises(ValidationError):  # Given / When / Then
        DisputeV2(hotkey=MINER.ss58, verdict_hash=H, action="contest", bond_lock=None)


def test_weight_caps_when_probation_cohort_exceeds_quarter() -> None:
    entries = [
        dict(
            hotkey=Keypair(bytes([i]) * 32).ss58,
            admission_id=H,
            probation=False,
            weight_units=1 << 22,
            commit_hash=H,
            delta_manifest_hash=H,
        )
        for i in range(4)
    ]  # Given
    entries.sort(key=lambda e: e["hotkey"])
    # When / Then
    assert WeightAllocationV2.model_validate({"policy_hash": H, "entries": entries})
    entries[0]["probation"] = True
    entries[1]["probation"] = True
    with pytest.raises(ValidationError):
        WeightAllocationV2.model_validate({"policy_hash": H, "entries": entries})


def test_economics_when_test_genesis_used_in_production() -> None:
    data = dict(
        ledger_mode="production",
        G_max_units=100,
        R_collectible_units=100,
        S_min_units=1000,
        beta_ppm=1_000_000,
        gammaV_units=0,
        s_lower_ppm=1_000_000,
        q_floor=1_000_000,
        genesis_allocation_hash=H,
        reward_authority=MINER.ss58,
        round_reward_units=100,
        max_total_issuance=1000,
        shadow_bootstrap_rounds=12,
    )  # Given
    with pytest.raises(ValidationError):  # When / Then
        EconomicsPolicyV2.model_validate(data)


def test_schema_when_exported_to_temporary_directory(tmp_path: Path) -> None:
    files = render()  # Given
    paths = write(tmp_path / "v2")  # When
    # Then
    assert {p.name: p.read_bytes() for p in paths} == files
    assert b'"additionalProperties": false' in files["RunManifestV2.json"]


def test_local_job_when_path_escapes_attempt_directory() -> None:
    manifest = wrapper()  # Given
    data = dict(
        job_version=1,
        run_id=manifest.run_id(),
        w=1,
        manifest=manifest,
        sample_ids=list(range(manifest.training.batch_samples())),
        global_step0=0,
        start_state_sha256=H,
        ef_in_sha256=H,
        v0_sha256=H,
        object_paths={"state": "../state"},
        deadline=100,
    )
    with pytest.raises(ValidationError):  # When / Then
        IslandJobV1.model_validate(data)
    data["object_paths"] = {"state": "objects/state"}
    assert IslandJobV1.model_validate(data).manifest == manifest


def test_receipts_when_distinct_commits_share_round() -> None:
    # Given
    intake = envelope_v2.Intake(RUN)
    first = Receipt(w=3, commit_hash=H, received_round=50)
    intake.accept(envelope_v2.seal(COLD, "Receipt", RUN, first, 100), 50)
    second = Receipt(w=3, commit_hash="22" * 32, received_round=50)
    # When
    accepted = intake.accept(envelope_v2.seal(COLD, "Receipt", RUN, second, 100), 50)
    # Then
    assert accepted == second and len(intake.reservations) == 2
    restarted = envelope_v2.Intake(RUN, reservations=dict(intake.reservations))
    conflict = Receipt(w=3, commit_hash=H, received_round=51)
    with pytest.raises(ReplayError):
        restarted.accept(envelope_v2.seal(COLD, "Receipt", RUN, conflict, 100), 51)


def test_rotation_when_persisted_coldkey_authorizes_distinct_hotkeys() -> None:
    # Given: service callback represents the persisted old coldkey, not the old hotkey.
    new = Keypair(bytes([9]) * 32)
    body = dict(hotkey=MINER.ss58, new_hotkey=new.ss58, operation_id=H)
    intake = envelope_v2.Intake(
        RUN, allowed_signers={"RotateRequest": lambda signer: signer == COLD.ss58}
    )
    # When
    accepted = intake.accept(envelope_v2.seal(COLD, "RotateRequest", RUN, body, 100), 50)
    # Then
    assert accepted.model_dump(mode="json") == body
    with pytest.raises(UnauthorizedSigner):
        intake.accept(envelope_v2.seal(MINER, "RotateRequest", RUN, body, 100), 50)
    with pytest.raises(ReplayError):
        intake.accept(
            envelope_v2.seal(COLD, "RotateRequest", RUN, {**body, "new_hotkey": MINER.ss58}, 100),
            50,
        )


def audit_job_body(*, signer: Keypair = MINER, expiry: int = 100) -> dict[str, JsonValue]:
    manifest = wrapper()
    run = manifest.run_id()
    start = StartStateV2(
        run_id=run,
        w=3,
        hotkey=MINER.ss58,
        theta_hash=H,
        state_object_sha256=H,
        opt_state_hash=H,
        ef_object_sha256=H,
        ef_hash=H,
        parent_anchor_hash=H,
        global_step0=0,
        anchor_verdict_hash=H,
    )
    challenge = AuditChallengeV2(
        w=3,
        target=MINER.ss58,
        beacon_round=10,
        beacon_sig_sha256=H,
        mode="full",
        segments=[],
        reasons=["final"],
        serve_deadline=100,
        anchor_hash=start.digest(),
        audit_mode="anchored-full",
    )
    preimages = [
        LeafPreimage(
            run_id=run,
            w=3,
            t=t,
            stages=[dict(theta=H, m=H, v=H)],
            batch_ids_sha256=H,
            rng_ctr=0,
            loss_f32="00000000",
            norm_f32="00000000",
        ).model_dump(mode="json")
        for t in range(0, 31, 5)
    ]
    data: dict[str, JsonValue] = dict(
        run_id=run,
        job_id=H,
        auditor_id=COLD.ss58,
        attempt=1,
        lease_nonce=H,
        lease_expires=50,
        absolute_deadline=100,
        reservation_id=H,
        replay_step_budget=30,
        anchor_age=0,
        manifest=manifest.body(),
        challenge_envelope=envelope_v2.seal(COLD, "AuditChallengeV2", run, challenge, expiry),
        commit_envelope=envelope_v2.seal(
            signer,
            "CommitV2",
            run,
            commit(manifest.training.batch_samples() * manifest.training.model.seq_len),
            expiry,
        ),
        sample_ids=list(range(manifest.training.batch_samples())),
        start_state=start.body(),
        preimages=preimages,
        ef_in=dict(sha256=H, size=1),
        v0=dict(sha256=H, size=1),
        created_beacon=10,
    )
    return data


def test_audit_job_when_commit_signed_by_another_identity() -> None:
    data = audit_job_body(signer=COLD)  # Given
    with pytest.raises(ValidationError):  # When / Then
        AuditJobV2.model_validate(data)


@pytest.mark.parametrize("expired_field", ["commit_envelope", "challenge_envelope"])
def test_audit_job_when_embedded_record_expired_before_creation(expired_field: str) -> None:
    # Given: no authenticated original acceptance evidence exists in this wire contract.
    data = audit_job_body()
    expired = audit_job_body(expiry=1)
    data[expired_field] = expired[expired_field]
    with pytest.raises(ValidationError):  # When / Then
        AuditJobV2.model_validate(data)


def test_audit_job_when_expiry_is_live_at_authenticated_creation() -> None:
    data = audit_job_body(expiry=10)  # Given
    accepted = AuditJobV2.model_validate(data)  # When
    assert accepted.created_beacon == 10  # Then: inclusive expiry, not replay-time renewal.


def test_audit_intake_when_created_beacon_cannot_waive_live_expiry() -> None:
    # Given: signed job created while records were live; no original acceptance receipt supplied.
    data = audit_job_body(expiry=10)
    raw = envelope_v2.seal(COLD, "AuditJobV2", data["run_id"], data, 100)
    with pytest.raises(ValueError):  # When / Then
        envelope_v2.Intake(data["run_id"]).accept(raw, 11)


def test_audit_intake_when_all_embedded_signatures_are_live() -> None:
    data = audit_job_body()  # Given
    raw = envelope_v2.seal(COLD, "AuditJobV2", data["run_id"], data, 100)
    accepted = envelope_v2.Intake(data["run_id"]).accept(raw, 20)  # When
    assert isinstance(accepted, AuditJobV2)  # Then


@pytest.mark.parametrize("value", ["0000807f", "0000c07f"])
@pytest.mark.parametrize(
    "section,field",
    [
        ("model", "capacity_factor"),
        ("model", "aux_loss_coef"),
        ("model", "init_std"),
        ("inner", "eps"),
        ("inner", "wd"),
        ("inner", "grad_clip"),
        ("inner", "muon_momentum"),
        ("outer", "lr"),
        ("outer", "momentum"),
        ("outer", "topk_frac"),
        ("outer", "ef_beta"),
        ("outer", "preclip_norm"),
        ("outer", "cclip_tau"),
        ("verify", "influence_cap"),
    ],
)
def test_training_when_computational_f32_is_nonfinite(section: str, field: str, value: str) -> None:
    body = wrapper().body()  # Given
    body["training"]["inner"]["opt"] = "muon"
    body["training"][section][field] = value
    with pytest.raises(ValidationError):  # When / Then
        RunManifestV2.model_validate(body)


@pytest.mark.parametrize(
    "path",
    [
        ["inner", "betas", 0],
        ["inner", "betas", 1],
        ["inner", "lr_schedule", "peak_lr"],
        ["operator_budget", "honeypot", "rate"],
    ],
)
def test_training_when_nested_f32_is_nonfinite(path: list[str | int]) -> None:
    body = wrapper().body()  # Given
    body["training"][path[0]][path[1]][path[2]] = "0000807f"
    with pytest.raises(ValidationError):  # When / Then
        RunManifestV2.model_validate(body)


@pytest.mark.parametrize("model", [EscrowLock, EscrowRelease, EscrowTransfer])
def test_escrow_when_origin_reference_repeated(
    model: type[EscrowLock] | type[EscrowRelease] | type[EscrowTransfer],
) -> None:
    # Given
    data = dict(
        operation_id=H, owner=COLD.ss58, units=1, origin_ids=[H, H], admission_id=H, dispute_id=None
    )
    if model is EscrowLock:
        data.update(kind="LOCK_ADMISSION")
    elif model is EscrowRelease:
        data.update(lock_id=H, finality_hash=H, closed_dispute_root=H)
    else:
        data.update(recipient=MINER.ss58)
    with pytest.raises(ValidationError):  # When / Then
        model.model_validate(data)


def test_roster_when_probability_is_nonfinite() -> None:
    data = dict(
        hotkey=MINER.ss58,
        slot=0,
        q_i="0000807f",
        admission_id=H,
        coldkey_group="group",
        state="ACTIVE",
        eligible_weight=0,
    )  # Given
    with pytest.raises(ValidationError):  # When / Then
        RosterEntryV2.model_validate(data)


@pytest.mark.parametrize("field", ["preclip_norm", "cclip_tau"])
def test_aggregation_when_computational_f32_is_nonfinite(field: str) -> None:
    data = dict(
        arithmetic="flat-cclip-cap-v1",
        order="utf8",
        center="prev_outer_update",
        trust_mode="uniform-verified",
        weight_quantum=1 << 24,
        miner_cap_units=1 << 22,
        probation_cap_units=1 << 22,
        owner_group_caps={},
        preclip_norm="0000803f",
        cclip_tau="0000803f",
        cclip_iters=1,
    )  # Given
    data[field] = "0000807f"
    with pytest.raises(ValidationError):  # When / Then
        AggregationPolicyV2.model_validate(data)


@pytest.mark.parametrize("field", ["mask_ratio", "rps_weight", "distill_temp"])
def test_od_when_objective_f32_is_nonfinite(field: str) -> None:
    body = od_body()  # Given
    body["model"]["param_count"] = 116_288
    body["model"]["od"][field] = "0000c07f"
    with pytest.raises(ValidationError):  # When / Then
        wrapper(body)


@pytest.mark.parametrize("field", ["loss_f32", "norm_f32"])
def test_audit_job_when_metric_f32_is_nonfinite(field: str) -> None:
    data = audit_job_body()  # Given
    data["preimages"][0][field] = "0000807f"
    with pytest.raises(ValidationError):  # When / Then
        AuditJobV2.model_validate(data)


def test_v1_when_opaque_published_baseline_is_replayed(tmp_path: Path) -> None:
    # Given: fixed captured bytes, never freshly sign or regenerate expected values.
    fixture = json.loads((Path(__file__).parent / "fixtures/network_v1_baseline.json").read_bytes())
    for message in fixture["messages"]:
        body_bytes = bytes.fromhex(message["body_hex"])
        envelope_bytes = bytes.fromhex(message["envelope_hex"])
        body, envelope = json.loads(body_bytes), json.loads(envelope_bytes)
        # When / Then: parse the stored signature, compare the entire body/preimage/envelope.
        model = envelope_v1.Intake(message["run_id"]).accept(envelope, 50)
        assert canonicalize(model.model_dump(mode="json"), allow_float=False) == body_bytes
        assert envelope_v1.signing_message(
            message["type"], body, message["exp_drand"], message["run_id"]
        ) == bytes.fromhex(message["preimage_hex"])
        assert canonicalize(envelope, allow_float=False) == envelope_bytes
        assert envelope_v1.verify_envelope(envelope)
        assert not envelope_v1.verify_envelope({**envelope, "sig": "0" * 128})
    assert canonicalize(example_manifest().body(), allow_float=False) == bytes.fromhex(
        fixture["messages"][0]["body_hex"]
    )
    leaf_bytes = bytes.fromhex(fixture["leaf"]["body_hex"])
    leaf = LeafPreimage.model_validate_json(leaf_bytes)
    assert canonicalize(leaf.model_dump(mode="json"), allow_float=False) == leaf_bytes
    assert leaf.digest() == fixture["leaf"]["digest"]
    tape_fixture = fixture["tape"]
    tape = json.loads(bytes.fromhex(tape_fixture["signed_hex"]))
    store = LocalFSStore(tmp_path / "objects")
    for key, value in tape_fixture["objects"].items():
        assert store.put(bytes.fromhex(value)) == key
    # When: actual v1 signature verification and arithmetic replay, tiny two-element state.
    result = replay_tape(store, tape, tape["signer"])
    # Then
    assert verify_tape(tape) and not verify_tape({**tape, "sig": "0" * 128})
    assert canonicalize(tape["body"], allow_float=False) == bytes.fromhex(tape_fixture["body_hex"])
    assert tape_message(tape["body"]["run_id"], tape["body"]) == bytes.fromhex(
        tape_fixture["preimage_hex"]
    )
    assert tape_bytes(tape) == bytes.fromhex(tape_fixture["signed_hex"])
    assert sha256_hex(result.to_bytes()) == tape["body"]["out_state"]
