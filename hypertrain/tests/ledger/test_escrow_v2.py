"""Origin accounting negatives with real signatures and persistent SQLite transactions."""

from __future__ import annotations

import importlib
import json
import sqlite3
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path

import numpy as np
import pytest
from pydantic import ValidationError

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import pack_state, unpack_state
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    EscrowV2,
    FinalityEvidence,
    RewardWork,
    Settlement,
    ShadowReservationEvidence,
    authority_message,
    digest,
    genesis_allocation_hash,
    genesis_origin_id,
    reward_origin_id,
    shadow_origin_id,
)
from hypertrain.protocol.envelope_v2 import EnvelopeV2, UnauthorizedSigner, seal
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, ReplayEnv, ReplayVerdict, f32hex
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    ArtifactLimits,
    AuditChallengeV2,
    CommitV2,
    EconomicsPolicyV2,
    EscrowLock,
    EscrowOperation,
    EscrowRelease,
    EscrowTransfer,
    IslandJobV1,
    OriginAllocation,
    RewardFinalize,
    RunManifestV2,
    ShadowReplayReceiptV1,
    ShadowReservationRequestV1,
    ShadowReservationV1,
    ShadowRewardFinalizeV1,
    TestGenesis,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params, param_shapes

torch = importlib.import_module("torch")

COORD = Keypair(bytes(range(32)))
AUDITOR = Keypair(bytes(31) + b"\x01")
HOT = Keypair(b"\x07" * 32)
COLD = Keypair(b"\x08" * 32)
OTHER = Keypair(b"\x09" * 32)
H = "11" * 32
VERIFIED_SHADOW_STATES: dict[str, str] = {}


@dataclass(frozen=True, slots=True)
class Setup:
    manifest: RunManifestV2
    policy: EconomicsPolicyV2
    admission_policy: AdmissionPolicyV2
    rows: np.ndarray
    tree: MerkleTree


def setup(mode: str = "test") -> Setup:
    admission = AdmissionPolicyV2(
        q_base="0000803e",
        E=4,
        clean_finalizations=12,
        work_screen_epoch_rounds=1000,
        max_pending=32,
        max_trial_replays=2,
        max_trials_per_beacon=2,
        join_per_beacon=2,
        join_burst=4,
        ip_prefix_per_beacon=32,
        ip_prefix_burst=64,
        challenge_rounds=100,
        artifact_limits=ArtifactLimits(
            max_object_bytes=1_000_000, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
    )
    alloc_hash = genesis_allocation_hash([(COLD.ss58, 20000, 1), (OTHER.ss58, 10000, 1)])
    policy = EconomicsPolicyV2.model_validate(
        dict(
            ledger_mode=mode,
            G_max_units=100,
            R_collectible_units=100,
            S_min_units=1000,
            beta_ppm=1_000_000,
            gammaV_units=0,
            s_lower_ppm=1_000_000,
            q_floor=1_000_000,
            genesis_allocation_hash=alloc_hash if mode == "test" else None,
            reward_authority=COORD.ss58,
            round_reward_units=1_000_000,
            max_total_issuance=20_000_000,
            shadow_bootstrap_rounds=12,
        )
    )
    b = example_manifest().body()
    b["model"].update(
        n_layers=1,
        d_model=8,
        n_heads=2,
        n_kv_heads=2,
        d_ff=12,
        vocab=16,
        seq_len=4,
        n_experts=1,
        top_k_experts=1,
    )
    b["inner"].update(H=1, J=1, micro_batch=1, grad_accum=1, state_policy="reset")
    b["inner"]["lr_schedule"].update(warmup=0, stable=100, peak_lr=f32hex(0.001))
    b["model"]["param_count"] = sum(
        int(np.prod(s)) for s in param_shapes(TrainConfig.from_manifest(b).model).values()
    )
    rows = (np.arange(16 * 5).reshape(16, 5) % 16).astype("<u4")
    tree = MerkleTree([r.tobytes() for r in rows])
    b["dataset"].update(merkle_root=tree.root.hex(), n_samples=16, depth=4)
    b["reference_spec"]["driver_allowlist"] = ["cpu-test"]
    cfg = TrainConfig.from_manifest(b)
    b["init_state_hash"] = state_hash(init_params(cfg.model))
    manifest = RunManifestV2.model_validate(
        dict(
            manifest_version=2,
            training=b,
            network=dict(
                admission_policy_hash=digest(admission),
                economics_policy_hash=digest(policy),
                aggregation_policy_hash=H,
                dispute_policy_hash=H,
                audit_policy_hash=H,
                relay_registry_hash=H,
                audit_mode="anchored-full",
                full_anchor_version=1,
                capabilities=["island-replay", "all-level-disputes", "transport-receipts"],
            ),
        )
    )
    return Setup(manifest, policy, admission, rows, tree)


def stage(s: Setup, directory: Path, samples: tuple[int, ...], w: int) -> IslandJobV1:
    cfg = TrainConfig.from_manifest_v2(s.manifest)
    theta = init_params(cfg.model)
    inputs = dict(
        start_state=pack_state(theta),
        ef_in=pack_state({k: torch.zeros_like(v) for k, v in theta.items()}),
        v0=pack_state({}),
        samples=b"".join(s.rows[i].tobytes() for i in samples),
        sample_proofs=json.dumps([[h.hex() for h in s.tree.proof(i)] for i in samples]).encode(),
    )
    directory.mkdir(parents=True, exist_ok=True)
    for name, data in inputs.items():
        (directory / name).write_bytes(data)
    return IslandJobV1(
        job_version=1,
        run_id=s.manifest.run_id(),
        w=w,
        manifest=s.manifest,
        sample_ids=list(samples),
        global_step0=0,
        start_state_sha256=sha256_hex(inputs["start_state"]),
        ef_in_sha256=sha256_hex(inputs["ef_in"]),
        v0_sha256=sha256_hex(inputs["v0"]),
        object_paths={k: k for k in inputs},
        deadline=int(time.time()) + 120,
    )


def ledger(
    s: Setup,
    database: Path | sqlite3.Connection,
    settlement: Callable[[str], Settlement] | None = None,
    replay: Callable[[FinalityEvidence], str] | None = None,
) -> EscrowV2:
    def no_unverified_finality(evidence: FinalityEvidence) -> str:
        raise EscrowError("NO_REPLAY_ACCEPTANCE")

    return EscrowV2(
        database,
        s.manifest,
        s.policy,
        auditors=frozenset({AUDITOR.ss58}),
        replay_finality=replay or no_unverified_finality,
        settlement=settlement
        or (
            lambda _: Settlement(
                finality_hash=H,
                closed_dispute_root=H,
                release_beacon=10,
                unresolved=False,
                outcome="MATCH",
            )
        ),
        owner_of=lambda h: COLD.ss58 if h == HOT.ss58 else OTHER.ss58,
        assignment_of=lambda w, h: (0,),
        shadow_assignment_of=lambda binding: (0,),
        shadow_settlement=settlement,
    )


def fund(e: EscrowV2) -> tuple[str, str]:
    alloc = [(COLD.ss58, 20000, 1), (OTHER.ss58, 10000, 1)]
    ah = genesis_allocation_hash(alloc)
    origins = [
        OriginAllocation(
            origin_id=genesis_origin_id(e.run_id, ah, i), owner=h, units=u, mature_at=m
        )
        for i, (h, u, m) in enumerate(alloc)
    ]
    g = TestGenesis(
        run_id=e.run_id,
        allocation_hash=ah,
        total_units=30000,
        origins=origins,
        authority_sig="0" * 128,
    )
    g = TestGenesis.model_validate(
        {**g.body(), "authority_sig": COORD.sign(authority_message(g)).hex()}
    )
    e.test_genesis(g)
    e.mature(H, tuple(o.origin_id for o in origins), 1)
    return origins[0].origin_id, origins[1].origin_id


def lock(origin: str, operation: str = "22" * 32, units: int = 1000) -> EscrowLock:
    return EscrowLock(
        operation_id=operation,
        owner=COLD.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=H,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )


def test_conservation_when_transfer_lock_release_pay_restart(tmp_path: Path) -> None:
    # Given: independently funded origins, not v1 entitlement bytes.
    s = setup()
    e = ledger(s, tmp_path / "ledger.db")
    origin, _ = fund(e)
    e.transfer(
        EscrowTransfer(
            operation_id="33" * 32,
            owner=COLD.ss58,
            recipient=OTHER.ss58,
            units=500,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=None,
        ),
        signer=COLD.ss58,
    )
    receipt = e.lock(lock(origin), signer=COLD.ss58)
    e.release(
        EscrowRelease(
            operation_id="44" * 32,
            owner=COLD.ss58,
            units=1000,
            origin_ids=[origin],
            admission_id=H,
            dispute_id=None,
            lock_id="22" * 32,
            finality_hash=H,
            closed_dispute_root=H,
        ),
        signer=COLD.ss58,
        now_beacon=10,
    )
    e.pay(
        EscrowOperation(
            operation_id="55" * 32,
            owner=COLD.ss58,
            units=500,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=None,
        ),
        signer=COLD.ss58,
    )
    # When: reopen exact pinned mode/run.
    restored = ledger(s, tmp_path / "ledger.db")
    # Then: buckets and receipt retain exact conserved amounts.
    assert restored.balances().conserved()
    assert restored.balances().paid == 500
    assert restored.balances(COLD.ss58).available == 19000
    assert restored.lock(lock(origin), signer=COLD.ss58) == receipt


@pytest.mark.parametrize("fault", ["unknown_origin", "owner", "double_spend", "operation_conflict"])
def test_lock_rejects_when_funding_invalid(tmp_path: Path, fault: str) -> None:
    # Given
    e = ledger(setup(), tmp_path / "ledger.db")
    origin, _ = fund(e)
    e.lock(lock(origin), signer=COLD.ss58)
    op = lock(origin, "33" * 32, 19001)
    signer = COLD.ss58
    if fault == "unknown_origin":
        op = lock("ff" * 32, "33" * 32)
    if fault == "owner":
        signer = OTHER.ss58
    if fault == "operation_conflict":
        op = lock(origin, units=999)
    before = e.balances()
    # When / Then
    with pytest.raises(EscrowError):
        e.lock(op, signer=signer)
    assert e.balances() == before


def test_legacy_entitlement_cannot_fund_empty_ledger(tmp_path: Path) -> None:
    # Given: v1 has no admitted v2 origins or bridge.
    e = ledger(setup(), tmp_path / "ledger.db")
    # When / Then: even a plausible v1 entitlement hash creates no units.
    with pytest.raises(EscrowError, match="UNFUNDED"):
        e.lock(lock(H), signer=COLD.ss58)
    assert e.balances().issued == 0


def test_legacy_source_payout_leaves_v2_zero_after_import_attempt(tmp_path: Path) -> None:
    from hypertrain.ledger import Ledger, Params

    # Given: actual v1 entitlement, independently payable on its own ledger.
    source = Ledger(tmp_path / "v1", Params("hypertrain", 0, 100, 1, 1))
    source.commit(0, COLD.ss58, 10)
    source.verdict(0, COLD.ss58, "MATCH", 4, 1_000_000, 20)
    source.finalize(0, 30)
    e = ledger(setup(), tmp_path / "v2.db")
    # When: attempted copied funding rejected; original source subsequently pays.
    with pytest.raises(EscrowError):
        e.lock(lock(H), signer=COLD.ss58)
    source.get_weights(1, 100, 100)
    # Then: no value exists in both ledgers.
    assert source.state().paid > 0 and e.balances().issued == 0


def test_immature_origin_cannot_lock(tmp_path: Path) -> None:
    # Given
    s = setup()
    e = ledger(s, tmp_path / "ledger.db")
    ah = s.policy.genesis_allocation_hash
    assert ah is not None
    origins = [
        OriginAllocation(
            origin_id=genesis_origin_id(e.run_id, ah, i), owner=h, units=u, mature_at=m
        )
        for i, (h, u, m) in enumerate([(COLD.ss58, 20000, 1), (OTHER.ss58, 10000, 1)])
    ]
    g = TestGenesis(
        run_id=e.run_id,
        allocation_hash=ah,
        total_units=30000,
        origins=origins,
        authority_sig="0" * 128,
    )
    g = TestGenesis.model_validate(
        {**g.body(), "authority_sig": COORD.sign(authority_message(g)).hex()}
    )
    e.test_genesis(g)
    # When / Then
    with pytest.raises(EscrowError):
        e.lock(lock(origins[0].origin_id), signer=COLD.ss58)
    assert e.balances().reward_pending == 30000


@pytest.mark.parametrize("fault", ["authority", "mode", "origin_domain", "allocation"])
def test_genesis_rejects_when_authority_or_mode_wrong(tmp_path: Path, fault: str) -> None:
    # Given
    s = setup("production" if fault == "mode" else "test")
    e = ledger(s, tmp_path / "ledger.db")
    ah = genesis_allocation_hash([(COLD.ss58, 20000, 1), (OTHER.ss58, 10000, 1)])
    origins = [
        OriginAllocation(
            origin_id=genesis_origin_id(e.run_id, ah, i), owner=h, units=u, mature_at=m
        )
        for i, (h, u, m) in enumerate([(COLD.ss58, 20000, 1), (OTHER.ss58, 10000, 1)])
    ]
    if fault == "origin_domain":
        origins[0] = OriginAllocation(origin_id=H, owner=COLD.ss58, units=20000, mature_at=1)
    g = TestGenesis(
        run_id=e.run_id,
        allocation_hash="ff" * 32 if fault == "allocation" else ah,
        total_units=30000,
        origins=origins,
        authority_sig="0" * 128,
    )
    signer = OTHER if fault == "authority" else COORD
    g = TestGenesis.model_validate(
        {**g.body(), "authority_sig": signer.sign(authority_message(g)).hex()}
    )
    # When / Then
    with pytest.raises(EscrowError):
        e.test_genesis(g)
    assert e.balances().issued == 0


@pytest.mark.parametrize("unresolved,beacon", [(True, 10), (False, 9)])
def test_release_rejects_when_disputed_or_unvested(
    tmp_path: Path, unresolved: bool, beacon: int
) -> None:
    # Given
    status = Settlement(
        finality_hash=H,
        closed_dispute_root=H,
        release_beacon=10,
        unresolved=unresolved,
        outcome="MATCH",
    )
    e = ledger(setup(), tmp_path / "ledger.db", settlement=lambda _: status)
    origin, _ = fund(e)
    e.lock(lock(origin), signer=COLD.ss58)
    op = EscrowRelease(
        operation_id="33" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=H,
        dispute_id=None,
        lock_id="22" * 32,
        finality_hash=H,
        closed_dispute_root=H,
    )
    # When / Then
    with pytest.raises(EscrowError):
        e.release(op, signer=COLD.ss58, now_beacon=beacon)
    assert e.balances().admission_locked == 1000


def test_contest_cost_and_burn_when_proven_loss(tmp_path: Path) -> None:
    # Given
    status = Settlement(
        finality_hash=H, closed_dispute_root=H, release_beacon=10, unresolved=False, outcome="FRAUD"
    )
    e = ledger(setup(), tmp_path / "ledger.db", settlement=lambda _: status)
    origin, _ = fund(e)
    op = EscrowLock(
        operation_id="22" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=H,
        kind="LOCK_CONTEST",
    )
    e.lock(op, signer=COLD.ss58)
    slash = EscrowRelease(
        operation_id="33" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=H,
        lock_id=op.operation_id,
        finality_hash=H,
        closed_dispute_root=H,
    )
    # When
    receipt = e.slash(
        slash,
        authority=COORD.ss58,
        now_beacon=10,
        referee=OTHER.ss58,
        referee_cost=100,
        max_referee_cost=100,
    )
    # Then
    assert e.balances().burned == 900 and e.balances().dispute_locked == 0
    assert e.balances(OTHER.ss58).available == 10100 and e.balances().conserved()
    assert (
        e.slash(
            slash,
            authority=COORD.ss58,
            now_beacon=10,
            referee=OTHER.ss58,
            referee_cost=100,
            max_referee_cost=100,
        )
        == receipt
    )


def test_auditor_infrastructure_never_slashes_miner(tmp_path: Path) -> None:
    # Given
    status = Settlement(
        finality_hash=H,
        closed_dispute_root=H,
        release_beacon=10,
        unresolved=False,
        outcome="INFRASTRUCTURE",
    )
    e = ledger(setup(), tmp_path / "ledger.db", settlement=lambda _: status)
    origin, _ = fund(e)
    e.lock(lock(origin), signer=COLD.ss58)
    op = EscrowRelease(
        operation_id="33" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=H,
        dispute_id=None,
        lock_id="22" * 32,
        finality_hash=H,
        closed_dispute_root=H,
    )
    # When / Then
    with pytest.raises(EscrowError):
        e.slash(op, authority=COORD.ss58, now_beacon=10)
    assert e.balances().burned == 0


def test_second_genesis_rejects_conflicting_allocation(tmp_path: Path) -> None:
    # Given
    s = setup()
    e = ledger(s, tmp_path / "ledger.db")
    fund(e)
    ah = s.policy.genesis_allocation_hash
    assert ah is not None
    origins = [
        OriginAllocation(
            origin_id=genesis_origin_id(e.run_id, ah, i), owner=h, units=u, mature_at=m
        )
        for i, (h, u, m) in enumerate([(COLD.ss58, 20001, 1), (OTHER.ss58, 9999, 1)])
    ]
    g = TestGenesis(
        run_id=e.run_id,
        allocation_hash=ah,
        total_units=30000,
        origins=origins,
        authority_sig="0" * 128,
    )
    g = TestGenesis.model_validate(
        {**g.body(), "authority_sig": COORD.sign(authority_message(g)).hex()}
    )
    # When / Then
    with pytest.raises(EscrowError):
        e.test_genesis(g)
    assert e.balances().issued == 30000


def test_origin_state_corruption_rejects_when_reopened(tmp_path: Path) -> None:
    # Given
    s = setup()
    path = tmp_path / "ledger.db"
    e = ledger(s, path)
    fund(e)
    e.db.execute("UPDATE escrow_units SET owner=? WHERE owner=?", (HOT.ss58, COLD.ss58))
    # When / Then: total conservation alone would miss ownership corruption.
    with pytest.raises(EscrowError, match="PERSISTED_STATE_MISMATCH"):
        ledger(s, path)


def test_concurrent_locks_when_same_funded_origin(tmp_path: Path) -> None:
    # Given: two genuine connections, a barrier instead of timing luck.
    s = setup()
    path = tmp_path / "ledger.db"
    e = ledger(s, path)
    origin, _ = fund(e)
    engines = [ledger(s, path), ledger(s, path)]
    barrier = threading.Barrier(2)
    outcomes = []

    def take(index: int) -> None:
        barrier.wait(timeout=10)
        try:
            engines[index].lock(lock(origin, f"{index + 2:02x}" * 32, 15000), signer=COLD.ss58)
            outcomes.append("locked")
        except EscrowError:
            outcomes.append("unfunded")

    # When
    threads = [threading.Thread(target=take, args=(i,)) for i in range(2)]
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=10)
        assert not t.is_alive()
    # Then
    assert sorted(outcomes) == ["locked", "unfunded"]
    assert e.balances().admission_locked == 15000 and e.balances().conserved()


@pytest.fixture(scope="module")
def actual_reward(
    tmp_path_factory: pytest.TempPathFactory,
) -> tuple[Setup, FinalityEvidence, RewardFinalize]:
    from hypertrain.gpu_ops.work_screen import commitments
    from hypertrain.miner.island_launch import launch_island

    s = setup("production")
    # Two legitimate one-round awards exactly fill this fixture's pinned shared cap.
    policy = EconomicsPolicyV2.model_validate({**s.policy.body(), "max_total_issuance": 2_000_000})
    body = s.manifest.body()
    body["network"]["economics_policy_hash"] = digest(policy)
    s = replace(s, policy=policy, manifest=RunManifestV2.model_validate(body))
    directory = tmp_path_factory.mktemp("real-reward")
    job = stage(s, directory, (0,), 0)
    a = launch_island(job, directory, backend="cpu")
    root, delta = commitments(a)
    theta, _ = unpack_state(a.state.read_bytes())
    c = CommitV2(
        w=0,
        hotkey=HOT.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=2,
        leaves_root=root,
        metrics_root=H,
        final_theta_hash=state_hash(theta),
        ef_in_hash=H,
        ef_out_hash=H,
        delta_hash=delta,
        delta_bytes=a.delta.stat().st_size,
        tokens=4,
    )
    challenge = AuditChallengeV2(
        w=0,
        target=HOT.ss58,
        beacon_round=1,
        beacon_sig_sha256=H,
        mode="full",
        segments=[],
        reasons=["probation"],
        serve_deadline=200,
        anchor_hash=H,
        audit_mode="anchored-full",
    )
    # Independent actual same-layout full replay in another process group.
    refdir = tmp_path_factory.mktemp("real-reference")
    refjob = stage(s, refdir, (0,), 0)
    reference = launch_island(refjob, refdir, backend="cpu")
    assert (
        commitments(reference) == (root, delta)
        and reference.state.read_bytes() == a.state.read_bytes()
    )
    v = ReplayVerdict(
        challenge_hash=digest(challenge),
        first_bad_leaf=None,
        result="MATCH",
        recomputed_leaves_root=commitments(reference)[0],
        replay_env=ReplayEnv(
            image_digest=s.manifest.training.reference_spec.image_digest,
            driver="cpu-test",
            gpu_uuid_sha256=H,
            sm_count=1,
        ),
    )
    final = Finalize(
        w=0, final_theta_hash_w1=state_hash(theta), included=[HOT.ss58], entitlements_root=H
    )
    final_env = EnvelopeV2.model_validate(seal(COORD, "Finalize", s.manifest.run_id(), final, 200))
    evidence = FinalityEvidence(
        final_env,
        (
            RewardWork(
                EnvelopeV2.model_validate(seal(HOT, "CommitV2", s.manifest.run_id(), c, 200)),
                EnvelopeV2.model_validate(
                    seal(AUDITOR, "ReplayVerdict", s.manifest.run_id(), v, 200)
                ),
                EnvelopeV2.model_validate(
                    seal(COORD, "AuditChallengeV2", s.manifest.run_id(), challenge, 200)
                ),
                COLD.ss58,
                (0,),
            ),
        ),
        sha256_hex(reference.state.read_bytes()),
        False,
        10,
        20,
    )
    VERIFIED_SHADOW_STATES[evidence.tape_hash] = state_hash(
        unpack_state(reference.state.read_bytes())[0]
    )
    origins = [
        OriginAllocation(
            origin_id=reward_origin_id(s.manifest.run_id(), 0, HOT.ss58, digest(final)),
            owner=COLD.ss58,
            units=1_000_000,
            mature_at=20,
        )
    ]
    reward = RewardFinalize(
        run_id=s.manifest.run_id(),
        w=0,
        finalize_hash=digest(final),
        tape_hash=evidence.tape_hash,
        verdict_root=sha256_hex(canonicalize([digest(v)])),
        allocation_hash=sha256_hex(canonicalize([o.body() for o in origins])),
        budget_units=1_000_000,
        origin_ids=[o.origin_id for o in origins],
        mature_at=20,
        authority_sig="0" * 128,
    )
    reward = RewardFinalize.model_validate(
        {**reward.body(), "authority_sig": COORD.sign(authority_message(reward)).hex()}
    )
    return s, evidence, reward


def verified_shadow(evidence: FinalityEvidence) -> str:
    # A transcript binds independently executed full replay state bytes, not a success sample.
    work = evidence.works[0]
    c = CommitV2.model_validate(work.commit.body)
    final = Finalize.model_validate(evidence.finalize.body)
    actual = VERIFIED_SHADOW_STATES[evidence.tape_hash]
    assert c.final_theta_hash == final.final_theta_hash_w1 == actual
    return actual


def reserve_actual_shadow(
    e: EscrowV2, evidence: FinalityEvidence, admission_id: str = H
) -> ShadowReservationV1:
    """Reuse independently executed module work; reserve before reward signing."""
    final = Finalize.model_validate(evidence.finalize.body)
    commit = CommitV2.model_validate(evidence.works[0].commit.body)
    request = ShadowReservationRequestV1(
        run_id=e.run_id,
        admission_id=admission_id,
        hotkey=commit.hotkey,
        coldkey=evidence.works[0].owner,
        trial_epoch=final.w,
        commit_hash=digest(commit),
        finalize_hash=digest(final),
        custody_hash=evidence.tape_hash,
        graph_hash=H,
        economic_admission_hash=H,
        nonce=H,
        assignment_hash=H,
        finalized_beacon=evidence.finalized_beacon,
        mature_at=evidence.finalized_beacon + e.manifest.training.verify.E_vest_rounds,
    )
    return e.reserve_shadow(request, original_trial_custody(evidence))


def original_trial_custody(evidence: FinalityEvidence) -> ShadowReservationEvidence:
    """Use original retained reference bytes, not the acceptance replay callback."""
    item = evidence.works[0]
    return ShadowReservationEvidence(
        finalize=evidence.finalize,
        commit=item.commit,
        owner=item.owner,
        sample_ids=item.sample_ids,
        custody_hash=evidence.tape_hash,
        reference_final_state_hash=VERIFIED_SHADOW_STATES[evidence.tape_hash],
    )


def sign_actual_shadow(
    reward: RewardFinalize, binding: ShadowReservationV1
) -> ShadowRewardFinalizeV1:
    origin = OriginAllocation(
        origin_id=shadow_origin_id(binding),
        owner=binding.coldkey,
        units=reward.budget_units,
        mature_at=binding.mature_at,
    )
    unsigned = ShadowRewardFinalizeV1.model_validate(
        {
            **reward.body(),
            "shadow": True,
            "shadow_ordinal": binding.shadow_ordinal,
            "reservation_hash": digest(binding),
            "mature_at": binding.mature_at,
            "origin_ids": [origin.origin_id],
            "allocation_hash": sha256_hex(canonicalize([origin.body()])),
        }
    )
    return unsigned.model_copy(
        update={"authority_sig": COORD.sign(authority_message(unsigned)).hex()}
    )


def test_production_mints_once_and_matures_once_when_real_verified_shadow(
    tmp_path: Path, actual_reward: tuple[Setup, FinalityEvidence, RewardFinalize]
) -> None:
    # Given: actual two-execution trajectory with pinned production issuance.
    s, evidence, reward = actual_reward
    e = ledger(
        s,
        tmp_path / "ledger.db",
        replay=verified_shadow,
        settlement=lambda _: Settlement(
            finality_hash=reward.finalize_hash,
            closed_dispute_root=H,
            release_beacon=20,
            unresolved=False,
            outcome="MATCH",
        ),
    )
    binding = reserve_actual_shadow(e, evidence)
    shadow_reward = sign_actual_shadow(reward, binding)
    evidence = replace(evidence, shadow=True)
    # When
    receipt = e.reward_finalize(shadow_reward, evidence)
    assert e.reward_finalize(shadow_reward, evidence) == receipt
    # Then: still pending until authorized maturity; no admission/live-model influence.
    assert e.balances().reward_pending == 1_000_000 and e.balances().admission_locked == 0
    with pytest.raises(EscrowError):
        e.mature(H, tuple(shadow_reward.origin_ids), 19)
    first = e.mature(H, tuple(shadow_reward.origin_ids), 20)
    assert e.mature(H, tuple(shadow_reward.origin_ids), 20) == first
    with pytest.raises(EscrowError):
        e.mature("22" * 32, tuple(shadow_reward.origin_ids), 20)
    assert e.balances().available == 1_000_000 and e.balances().conserved()


def test_reward_maturity_rejects_when_dispute_open(tmp_path: Path, actual_reward) -> None:
    # Given
    s, evidence, reward = actual_reward
    e = ledger(
        s,
        tmp_path / "ledger.db",
        replay=verified_shadow,
        settlement=lambda _: Settlement(
            finality_hash=reward.finalize_hash,
            closed_dispute_root=H,
            release_beacon=20,
            unresolved=True,
            outcome="MATCH",
        ),
    )
    binding = reserve_actual_shadow(e, evidence)
    shadow_reward = sign_actual_shadow(reward, binding)
    e.reward_finalize(shadow_reward, replace(evidence, shadow=True))
    # When / Then
    with pytest.raises(EscrowError):
        e.mature(H, tuple(shadow_reward.origin_ids), 20)
    assert e.balances().reward_pending == 1_000_000


def test_reward_callbacks_share_atomic_mutation_snapshot(tmp_path: Path, actual_reward) -> None:
    # Given: authenticated records live in the actual service connection.
    s, evidence, reward = actual_reward
    connection = sqlite3.connect(tmp_path / "ledger.db", isolation_level=None)
    calls = []

    def replay(facts: FinalityEvidence) -> str:
        assert connection.in_transaction
        calls.append("replay")
        return verified_shadow(facts)

    e = ledger(s, connection, replay=replay)

    def owner(hotkey: str) -> str:
        assert connection.in_transaction
        calls.append("owner")
        return COLD.ss58

    def assignment(w: int, hotkey: str) -> tuple[int, ...]:
        assert connection.in_transaction
        calls.append("assignment")
        return (0,)

    e.owner_of, e.assignment_of = owner, assignment
    # When
    e.reward_finalize(reward, evidence)
    # Then: authority checks and journal/buckets share one SQLite snapshot.
    assert calls == ["replay", "assignment", "owner"]
    assert e.balances().reward_pending == 1_000_000 and e.balances().conserved()


def test_production_reward_rejects_when_policy_run_rebound(tmp_path: Path, actual_reward) -> None:
    from dataclasses import replace

    # Given: different pinned total cap, real work rebound to that run would be required.
    s, evidence, reward = actual_reward
    policy = EconomicsPolicyV2.model_validate({**s.policy.body(), "max_total_issuance": 999999})
    body = s.manifest.body()
    body["network"]["economics_policy_hash"] = digest(policy)
    changed = replace(s, policy=policy, manifest=RunManifestV2.model_validate(body))
    e = ledger(changed, tmp_path / "ledger.db", replay=verified_shadow)
    # When / Then: old-run authority cannot cross pinned economic policy.
    with pytest.raises(EscrowError):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 0


@pytest.mark.parametrize(
    "fault",
    ["authority", "budget", "origin", "allocation", "audit", "signature", "cross_run", "maturity"],
)
def test_production_reward_rejects_when_invalid(
    tmp_path: Path, actual_reward: tuple[Setup, FinalityEvidence, RewardFinalize], fault: str
) -> None:
    from dataclasses import replace

    # Given
    s, evidence, reward = actual_reward
    e = ledger(s, tmp_path / "ledger.db", replay=verified_shadow)
    data = reward.body()
    if fault == "budget":
        data["budget_units"] = 1_000_001
    if fault == "origin":
        data["origin_ids"] = [H]
    if fault == "allocation":
        data["allocation_hash"] = H
    if fault == "cross_run":
        data["run_id"] = H
    if fault == "maturity":
        data["mature_at"] = 19
    if fault in ("audit", "signature"):
        item = evidence.works[0]
        v = ReplayVerdict.model_validate(item.verdict.body)
        if fault == "audit":
            v = ReplayVerdict.model_validate({**v.model_dump(mode="json"), "result": "MISMATCH"})
        env = EnvelopeV2.model_validate(
            seal(
                OTHER if fault == "signature" else AUDITOR,
                "ReplayVerdict",
                s.manifest.run_id(),
                v,
                200,
            )
        )
        evidence = replace(evidence, works=(replace(item, verdict=env),))
    reward = RewardFinalize.model_validate(data)
    signer = OTHER if fault == "authority" else COORD
    reward = RewardFinalize.model_validate(
        {**reward.body(), "authority_sig": signer.sign(authority_message(reward)).hex()}
    )
    # When / Then
    with pytest.raises((EscrowError, ValueError)):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 0


def shadow_ledger(s: Setup, path: Path, final_hash: str) -> EscrowV2:
    return ledger(
        s,
        path,
        replay=verified_shadow,
        settlement=lambda _: Settlement(
            finality_hash=final_hash,
            closed_dispute_root=H,
            release_beacon=20,
            unresolved=False,
            outcome="MATCH",
        ),
    )


def test_shadow_and_live_reward_domains_do_not_collide(tmp_path: Path, actual_reward) -> None:
    # Given: the same genuine independently replayed w=0 work in separate signed domains.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    binding = reserve_actual_shadow(e, evidence)
    shadow = sign_actual_shadow(live, binding)
    original_live = live.model_dump_json()
    live_signature_message = authority_message(live)
    # When
    shadow_receipt = e.reward_finalize(shadow, replace(evidence, shadow=True))
    live_receipt = e.reward_finalize(live, evidence)
    # Then: two conserved issuances, distinct origins, unchanged live wire/signature domain.
    assert shadow_receipt.operation_id != live_receipt.operation_id
    assert set(shadow.origin_ids).isdisjoint(live.origin_ids)
    assert e.balances().issued == 2_000_000 and e.balances().conserved()
    assert live.model_dump_json() == original_live
    assert authority_message(live) == live_signature_message
    assert live_signature_message == (
        b"hypertrain/origin/2|RewardFinalize|"
        + e.run_id.encode()
        + b"|"
        + sha256_hex(canonicalize(live.model_dump(mode="json", exclude={"authority_sig"}))).encode()
    )
    assert set(RewardFinalize.model_fields) == {
        "run_id",
        "w",
        "finalize_hash",
        "tape_hash",
        "verdict_root",
        "allocation_hash",
        "budget_units",
        "origin_ids",
        "mature_at",
        "authority_sig",
    }
    assert e.db.execute("SELECT w FROM escrow_finalized").fetchone()[0] == 0
    assert e.db.execute("SELECT ordinal FROM escrow_shadow_finalized").fetchone()[0] == 0


def test_shadow_and_live_share_pinned_issuance_cap(tmp_path: Path, actual_reward) -> None:
    # Given: two original awards fill the genuine fixture cap, not an inserted balance.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    shadow = sign_actual_shadow(live, reserve_actual_shadow(e, evidence))
    e.reward_finalize(shadow, replace(evidence, shadow=True))
    e.reward_finalize(live, evidence)
    # When / Then: another pending award cannot overreserve shared issuance.
    with pytest.raises(EscrowError, match="TOTAL_ISSUANCE_BUDGET"):
        reserve_actual_shadow(e, evidence, "22" * 32)
    assert e.balances().issued == s.policy.max_total_issuance
    assert e.db.execute("SELECT COUNT(*) FROM admission_reservations").fetchone()[0] == 1


def test_live_origin_mapping_keeps_original_commit_body_hotkey(
    tmp_path: Path, actual_reward
) -> None:
    # Given: genuine HOT-signed work; original intake rejects a different envelope signer.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    item = evidence.works[0]
    commit = EnvelopeV2.model_validate(
        seal(
            OTHER,
            "CommitV2",
            e.run_id,
            item.commit.body,
            200,
        )
    )
    facts = replace(evidence, works=(replace(item, commit=commit),))
    before = e.balances()
    events = e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    with pytest.raises(UnauthorizedSigner, match="hotkey differs from signer"):
        e.reward_finalize(live, facts)
    assert e.balances() == before
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == events
    # When
    receipt = e.reward_finalize(live, evidence)
    # Then: the valid origin maps HOT work to its authorized COLD economic owner.
    row = e.db.execute("SELECT origin,hotkey FROM escrow_origins").fetchone()
    assert (row["origin"], row["hotkey"]) == (live.origin_ids[0], HOT.ss58)
    owners = e.db.execute(
        "SELECT DISTINCT owner FROM escrow_units WHERE origin=?", (live.origin_ids[0],)
    ).fetchall()
    assert [owner["owner"] for owner in owners] == [COLD.ss58]
    assert receipt.event_hash
    assert e.balances().issued == 1_000_000 and e.balances().conserved()


def test_live_issuance_preserves_pending_shadow_headroom(tmp_path: Path, actual_reward) -> None:
    # Given: original retained work earned U; the next trusted-intake request reserves the last U.
    # Distinct admission IDs exercise the internal ledger seam, not a fabricated public history.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    first = reserve_actual_shadow(e, evidence)
    e.reward_finalize(sign_actual_shadow(live, first), replace(evidence, shadow=True))
    pending = reserve_actual_shadow(e, evidence, "22" * 32)
    assert pending.shadow_ordinal == 1
    assert e.balances().issued == 1_000_000
    # When / Then: original live w=0 validates, but cannot spend the reserved issuance headroom.
    with pytest.raises(EscrowError, match="TOTAL_ISSUANCE_BUDGET"):
        e.reward_finalize(live, evidence)
    assert e.balances().issued == 1_000_000 and e.balances().conserved()
    assert e.db.execute("SELECT COUNT(*) FROM escrow_finalized").fetchone()[0] == 0
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 1
    assert reserve_actual_shadow(e, evidence, "22" * 32) == pending


@pytest.mark.parametrize(
    "fault",
    [
        "missing_reservation",
        "signature",
        "cross_type_signature",
        "ordinal",
        "domain",
        "binding",
        "allocation",
        "budget",
        "early_vesting",
        "owner",
        "audit",
        "assignment",
    ],
)
def test_shadow_reward_rejects_unbound_authority(tmp_path: Path, actual_reward, fault: str) -> None:
    # Given: valid retained work, one reserved ordinal, independently signed shadow body.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    binding = reserve_actual_shadow(e, evidence)
    reward = sign_actual_shadow(live, binding)
    facts = replace(evidence, shadow=True)
    if fault == "missing_reservation":
        e = shadow_ledger(s, tmp_path / "other.db", live.finalize_hash)
    body = reward.body()
    if fault == "ordinal":
        body["shadow_ordinal"] = 1
    if fault == "binding":
        body["reservation_hash"] = "22" * 32
    if fault == "allocation":
        body["origin_ids"] = [H]
    if fault == "budget":
        body["budget_units"] = 1_000_001
    if fault == "early_vesting":
        body["mature_at"] = 19
    reward = ShadowRewardFinalizeV1.model_validate(body)
    reward = reward.model_copy(
        update={
            "authority_sig": (OTHER if fault == "signature" else COORD)
            .sign(authority_message(reward))
            .hex()
        }
    )
    if fault == "cross_type_signature":
        reward = reward.model_copy(update={"authority_sig": live.authority_sig})
    if fault == "domain":
        facts = evidence
    if fault == "owner":
        facts = replace(facts, works=(replace(facts.works[0], owner=OTHER.ss58),))
    if fault == "audit":
        item = facts.works[0]
        verdict = ReplayVerdict.model_validate(item.verdict.body)
        envelope = EnvelopeV2.model_validate(
            seal(
                HOT,
                "ReplayVerdict",
                e.run_id,
                verdict,
                200,
            )
        )
        facts = replace(facts, works=(replace(item, verdict=envelope),))
    if fault == "assignment":
        e.shadow_assignment_of = lambda binding: (1,)
    # When / Then
    with pytest.raises((EscrowError, ValueError)):
        e.reward_finalize(reward, facts)
    assert e.balances().issued == 0 and e.balances().conserved()
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 0


def test_shadow_reservation_survives_crash_and_conflicting_request(
    tmp_path: Path, actual_reward
) -> None:
    # Given: durable reservation returned before signing; process then disappears.
    s, evidence, live = actual_reward
    path = tmp_path / "ledger.db"
    e = shadow_ledger(s, path, live.finalize_hash)
    binding = reserve_actual_shadow(e, evidence)
    e.db.close()
    # When
    e = shadow_ledger(s, path, live.finalize_hash)
    same = reserve_actual_shadow(e, evidence)
    # Then: recover original binding without reassignment or issuance.
    assert same == binding and same.shadow_ordinal == 0
    with pytest.raises(EscrowError, match="SHADOW_RESERVATION_PENDING"):
        reserve_actual_shadow(e, evidence, "22" * 32)
    request = ShadowReservationRequestV1.model_validate(
        {k: v for k, v in binding.body().items() if k != "shadow_ordinal"}
    )
    with pytest.raises(EscrowError, match="SHADOW_RESERVATION_CONFLICT"):
        e.reserve_shadow(
            request.model_copy(update={"custody_hash": "33" * 32}),
            replace(original_trial_custody(evidence), custody_hash="33" * 32),
        )
    assert e.balances().issued == 0


@pytest.mark.parametrize("accepted", [False, True])
def test_shadow_reservation_recovers_before_mutable_authority(
    tmp_path: Path, actual_reward, accepted: bool
) -> None:
    # Given: original signed bodies and custody already own an immutable reservation.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    binding = reserve_actual_shadow(e, evidence)
    if accepted:
        e.reward_finalize(sign_actual_shadow(live, binding), replace(evidence, shadow=True))

    def unavailable_owner(hotkey: str) -> str:
        raise EscrowError("OWNER_CHANGED_AFTER_RESERVATION")

    def unavailable_replay(facts: FinalityEvidence) -> str:
        raise EscrowError("ACCEPTANCE_REPLAY_UNAVAILABLE")

    e.owner_of, e.replay_finality = unavailable_owner, unavailable_replay
    # When
    recovered = reserve_actual_shadow(e, evidence)
    # Then: immutable authenticated replay ignores new-issuance predicates, not signatures.
    assert recovered == binding
    forged = replace(
        original_trial_custody(evidence),
        commit=EnvelopeV2.model_validate(
            seal(OTHER, "CommitV2", e.run_id, evidence.works[0].commit.body, 200)
        ),
    )
    request = ShadowReservationRequestV1.model_validate(
        {k: v for k, v in binding.body().items() if k != "shadow_ordinal"}
    )
    with pytest.raises(ValueError):
        e.reserve_shadow(request, forged)


def test_shadow_reservation_needs_no_acceptance_replay(tmp_path: Path, actual_reward) -> None:
    # Given: only original independently executed trial custody exists before reservation.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)

    def unavailable(facts: FinalityEvidence) -> str:
        raise EscrowError("NO_ACCEPTED_AUDITOR_RECEIPT_YET")

    e.replay_finality = unavailable
    # When
    binding = reserve_actual_shadow(e, evidence)
    # Then: ordinal can be signed now; acceptance must still invoke real independent replay.
    assert binding.shadow_ordinal == 0 and not e.db.in_transaction
    with pytest.raises(EscrowError, match="NO_ACCEPTED_AUDITOR_RECEIPT_YET"):
        e.reward_finalize(sign_actual_shadow(live, binding), replace(evidence, shadow=True))
    assert e.balances().issued == 0


def test_shadow_reservation_requires_durable_outer_commit(tmp_path: Path, actual_reward) -> None:
    # Given
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    # When / Then: never return an ordinal whose surrounding transaction can still roll back.
    with e.tx():
        with pytest.raises(EscrowError, match="SHADOW_RESERVATION_REQUIRES_COMMIT"):
            reserve_actual_shadow(e, evidence)
    assert e.db.execute("SELECT COUNT(*) FROM admission_reservations").fetchone()[0] == 0


def test_shadow_acceptance_rolls_back_with_outer_transaction(tmp_path: Path, actual_reward) -> None:
    # Given: signed reward after durable reservation; acceptance shares caller transaction.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    binding = reserve_actual_shadow(e, evidence)
    reward = sign_actual_shadow(live, binding)
    # When: caller's evidence/settlement acceptance fails after ledger insertion.
    with pytest.raises(EscrowError, match="CALLER_ROLLBACK"):
        with e.tx():
            e.reward_finalize(reward, replace(evidence, shadow=True))
            raise EscrowError("CALLER_ROLLBACK")
    # Then: original reservation survives, no funds/event/finality accepted.
    assert e.balances().issued == 0
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 0
    assert e.db.execute("SELECT COUNT(*) FROM escrow_shadow_finalized").fetchone()[0] == 0
    assert reserve_actual_shadow(e, evidence) == binding
    receipt = e.reward_finalize(reward, replace(evidence, shadow=True))
    assert receipt.event_hash and e.balances().issued == 1_000_000


def test_shadow_accepted_reply_recovers_after_restart(tmp_path: Path, actual_reward) -> None:
    # Given: acceptance committed, original HTTP reply lost.
    s, evidence, live = actual_reward
    path = tmp_path / "ledger.db"
    e = shadow_ledger(s, path, live.finalize_hash)
    binding = reserve_actual_shadow(e, evidence)
    reward = sign_actual_shadow(live, binding)
    receipt = e.reward_finalize(reward, replace(evidence, shadow=True))
    e.db.close()
    e = shadow_ledger(s, path, live.finalize_hash)
    # When
    replay = e.reward_finalize(reward, replace(evidence, shadow=True))
    # Then
    assert replay == receipt and e.balances().issued == 1_000_000
    changed = reward.model_copy(update={"tape_hash": H})
    changed = changed.model_copy(
        update={"authority_sig": COORD.sign(authority_message(changed)).hex()}
    )
    with pytest.raises(EscrowError, match="OPERATION_CONFLICT"):
        e.reward_finalize(changed, replace(evidence, shadow=True))
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 1


@pytest.mark.parametrize("fault", ["too_early", "unresolved", "fraud", "finality", "release"])
def test_shadow_maturity_uses_shadow_finality_and_disputes(
    tmp_path: Path, actual_reward, fault: str
) -> None:
    # Given: shadow issuance has no live finalized w entry.
    s, evidence, live = actual_reward
    e = shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    reward = sign_actual_shadow(live, reserve_actual_shadow(e, evidence))
    e.reward_finalize(reward, replace(evidence, shadow=True))
    e.shadow_settlement = lambda _: Settlement(
        finality_hash=H if fault == "finality" else live.finalize_hash,
        closed_dispute_root=H,
        release_beacon=21 if fault == "release" else 20,
        unresolved=fault == "unresolved",
        outcome="FRAUD" if fault == "fraud" else "MATCH",
    )
    # When / Then
    with pytest.raises(EscrowError):
        e.mature(H, tuple(reward.origin_ids), 19 if fault == "too_early" else 20)
    assert e.balances().available == 0 and e.balances().reward_pending == 1_000_000


@pytest.mark.parametrize("number", [True, -1, 12, 1.0, "0"])
def test_shadow_wire_rejects_non_strict_or_out_of_range_ordinal(actual_reward, number) -> None:
    # Given: original live body with explicit signed shadow domain fields.
    _, _, live = actual_reward
    body = {**live.body(), "shadow": True, "shadow_ordinal": number, "reservation_hash": H}
    # When / Then
    with pytest.raises(ValidationError):
        ShadowRewardFinalizeV1.model_validate(body)


@pytest.mark.parametrize("domain", [1, "true", False, None])
def test_shadow_wire_requires_actual_boolean_domain(actual_reward, domain) -> None:
    # Given
    _, _, live = actual_reward
    # When / Then
    with pytest.raises(ValidationError):
        ShadowRewardFinalizeV1.model_validate(
            {
                **live.body(),
                "shadow": domain,
                "shadow_ordinal": 0,
                "reservation_hash": H,
            }
        )


def test_shadow_replay_wire_rejects_unbound_fields(actual_reward) -> None:
    # Given: complete strict custody shape, not a claimed successful replay.
    s, _, _ = actual_reward
    body = {name: H for name in ShadowReplayReceiptV1.model_fields}
    body.update(
        run_id=s.manifest.run_id(),
        hotkey=HOT.ss58,
        coldkey=COLD.ss58,
        trial_epoch=0,
        shadow_ordinal=0,
        completed_beacon=10,
    )
    accepted = ShadowReplayReceiptV1.model_validate(body)
    # When / Then
    with pytest.raises(ValidationError):
        ShadowReplayReceiptV1.model_validate({**accepted.body(), "completed_beacon": True})
    with pytest.raises(ValidationError):
        ShadowReplayReceiptV1.model_validate({**accepted.body(), "success": True})
