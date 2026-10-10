from __future__ import annotations

import tracemalloc
from dataclasses import replace
from itertools import product
from pathlib import Path
from time import perf_counter, process_time

import numpy as np
import pytest
import torch

from hypertrain.aggregator.core import OuterParams, OuterState, TapeError, preclip, sqnorm
from hypertrain.aggregator.tape_v2 import TapeV2, VerifiedInput, make_tape, replay_tape
from hypertrain.aggregator.weighted_v2 import (
    Candidate,
    InsufficientEligibleWeight,
    _completion_capacity,
    aggregate_weighted,
    allocate_weights,
)
from hypertrain.auditor.replay import AuditInputs, audit_full
from hypertrain.challenge.trust_v2 import FundedStatus, ReplayEvidence, SettlementStatus
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.envelope_v2 import tape_signing_message
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import AuditChallenge, Chunk, Commit, f32hex
from hypertrain.protocol.messages_v2 import (
    AggregationPolicyV2,
    CommitV2,
    DeltaManifestV2,
    EconomicsPolicyV2,
    RosterEntryV2,
    RunManifestV2,
)
from hypertrain.trainer.compress import compress
from hypertrain.trainer.config import CompressConfig, InnerConfig, ModelConfig, TrainConfig
from hypertrain.trainer.loop import Assignment, train_round
from hypertrain.trainer.model import init_params

Q = 1 << 24
H = "11" * 32
KEY = Keypair(bytes(range(32)))


def policy(groups: dict[str, int] | None = None) -> AggregationPolicyV2:
    return AggregationPolicyV2(
        arithmetic="flat-cclip-cap-v1",
        order="utf8",
        center="prev_outer_update",
        trust_mode="uniform-verified",
        weight_quantum=Q,
        miner_cap_units=Q // 4,
        probation_cap_units=Q // 4,
        owner_group_caps=groups or {},
        preclip_norm=f32hex(10),
        cclip_tau=f32hex(10),
        cclip_iters=1,
    )


def economics() -> EconomicsPolicyV2:
    return EconomicsPolicyV2(
        ledger_mode="test",
        G_max_units=100,
        R_collectible_units=100,
        S_min_units=1000,
        beta_ppm=1_000_000,
        gammaV_units=0,
        s_lower_ppm=1_000_000,
        q_floor=1_000_000,
        genesis_allocation_hash=H,
        reward_authority=KEY.ss58,
        round_reward_units=100,
        max_total_issuance=100000,
        shadow_bootstrap_rounds=12,
    )


def roster(i: int, probation: bool = False, owner: str | None = None) -> RosterEntryV2:
    return RosterEntryV2(
        hotkey=Keypair(bytes([i + 1]) * 32).ss58,
        slot=i,
        q_i=f32hex(1),
        admission_id=H,
        coldkey_group=owner or f"owner{i}",
        state="PROBATION" if probation else "ACTIVE",
        eligible_weight=Q // 4,
    )


def candidate(i: int, probation: bool = False, owner: str | None = None) -> Candidate:
    return Candidate(roster(i, probation, owner), H, H)


def test_uniform_exact_dyadic_weights_when_four_active() -> None:
    # Given
    candidates = [candidate(i) for i in range(4)]
    # When
    allocation = allocate_weights(candidates[::-1], policy())
    # Then
    assert [e.weight_units for e in allocation.entries] == [Q // 4] * 4
    assert allocation == allocate_weights(candidates, policy())


@pytest.mark.parametrize("n,probation", [(3, False), (4, True), (10, True)])
def test_infeasible_caps_when_capacity_insufficient(n: int, probation: bool) -> None:
    # Given / When / Then
    with pytest.raises(InsufficientEligibleWeight):
        allocate_weights([candidate(i, probation) for i in range(n)], policy())


def test_intersecting_owner_probation_caps_when_feasible_completion_required() -> None:
    # Given: spending owner A on probation would strand the active capacity.
    candidates = [
        candidate(0, owner="A"),
        candidate(1, True, "A"),
        candidate(2, owner="B"),
        candidate(3, True, "C"),
        candidate(4, owner="D"),
    ]
    p = policy({"A": Q // 4, "B": Q // 4, "C": Q // 4, "D": Q // 4})
    # When
    allocation = allocate_weights(candidates, p)
    # Then: constrained completion gives A's active all its owner capacity.
    assert sum(e.weight_units for e in allocation.entries) == Q
    by_id = {e.hotkey: e.weight_units for e in allocation.entries}
    assert by_id[candidates[0].roster.hotkey] == Q // 4
    assert by_id[candidates[1].roster.hotkey] == 0


def test_probation_bound_and_utf8_remainder_when_multiple_newcomers() -> None:
    # Given
    candidates = [candidate(i, i >= 4) for i in range(7)]
    # When
    allocation = allocate_weights(candidates, policy())
    # Then
    assert sum(e.weight_units for e in allocation.entries if e.probation) == Q // 4
    assert sum(e.weight_units for e in allocation.entries) == Q
    newcomers = [e.weight_units for e in allocation.entries if e.probation]
    assert max(newcomers) - min(newcomers) <= 1


def test_owner_capacity_rejected_when_known_group_owns_entire_roster() -> None:
    # Given / When / Then
    with pytest.raises(InsufficientEligibleWeight):
        allocate_weights(
            [candidate(i, owner="shared") for i in range(8)], policy({"shared": Q // 2})
        )


def test_rescaled_copy_rejected_when_norm_screen_passes_own_assignment_replay() -> None:
    # Given: two honest non-IID trajectories; exact real trainer/replay, two steps.
    cfg = TrainConfig(
        model=ModelConfig(
            n_layers=1,
            d_model=16,
            n_heads=2,
            n_kv_heads=2,
            d_ff=24,
            vocab=32,
            seq_len=4,
            n_experts=0,
        ),
        inner=InnerConfig(lr=0.003, H=2, J=1, micro_batch=1, state_policy="reset"),
        compress=CompressConfig("dense-int8", ef_beta=0),
        n_stages=1,
    )
    theta = init_params(cfg.model)

    def sample(i: int) -> np.ndarray:
        return (np.arange(5, dtype=np.uint32) + i) % 32

    assignments = [Assignment(H, 0, (1, 2)), Assignment(H, 0, (17, 18))]
    results = [train_round(cfg, theta, a, sample) for a in assignments]
    c = Commit(
        w=0,
        hotkey=KEY.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=3,
        leaves_root=results[0].leaves_root,
        metrics_root=H,
        final_theta_hash=results[0].final_theta_hash,
        ef_in_hash=results[0].ef_in_hash,
        ef_out_hash=results[0].ef_out_hash,
        delta_hash=results[0].delta_hash,
        delta_bytes=len(results[0].delta_payload),
        tokens=8,
    )
    challenge = AuditChallenge(
        w=0,
        target=KEY.ss58,
        beacon_round=1,
        beacon_sig_sha256=H,
        mode="full",
        segments=[],
        reasons=["random"],
        serve_deadline=100,
    )
    audits = [
        AuditInputs(
            cfg,
            H,
            challenge,
            c.model_copy(
                update={
                    "leaves_root": res.leaves_root,
                    "final_theta_hash": res.final_theta_hash,
                    "ef_in_hash": res.ef_in_hash,
                    "ef_out_hash": res.ef_out_hash,
                    "delta_hash": res.delta_hash,
                    "delta_bytes": len(res.delta_payload),
                }
            ),
            a,
            [x.digest.hex() for x in res.leaves],
            [x.preimage for x in res.leaves],
            theta,
        )
        for a, res in zip(assignments, results, strict=True)
    ]
    assert all(audit_full(a, sample).result == "MATCH" for a in audits)
    honest = {n: (theta[n] - results[0].final_theta[n]).numpy() for n in theta}
    copied = {n: (theta[n] - results[1].final_theta[n]).numpy() for n in theta}
    scale = np.float32((sqnorm(honest) / sqnorm(copied)) ** 0.5)
    copied = {n: a * scale for n, a in copied.items()}
    _, clipped = preclip(copied, sqnorm(honest) ** 0.5 * 1.01)
    assert not clipped
    td = {n: torch.from_numpy(a) for n, a in copied.items()}
    payload, _ = compress(cfg.compress, td, {n: torch.zeros_like(a) for n, a in td.items()})
    attacked = replace(
        audits[0],
        commit=c.model_copy(
            update={"delta_hash": sha256_hex(payload), "delta_bytes": len(payload)}
        ),
    )
    # When
    verdict = audit_full(attacked, sample)
    # Then: median norm is not provenance; own assignment final compression rejects.
    assert verdict.result == "MISMATCH"


def fixture(store: LocalFSStore, n: int = 4) -> tuple[RunManifestV2, list[VerifiedInput], str]:
    p, econ = policy(), economics()
    training = example_manifest().model_dump(mode="json")
    training["model"]["param_count"] = 4329216
    manifest = RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": training,
            "network": {
                "admission_policy_hash": H,
                "economics_policy_hash": econ.digest(),
                "aggregation_policy_hash": p.digest(),
                "dispute_policy_hash": H,
                "audit_policy_hash": H,
                "relay_registry_hash": H,
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )
    previous = store.put(OuterState.init({"x": np.array([2.0, 3.0], np.float32)}).to_bytes())
    works = []
    for i in range(n):
        r = roster(i)
        delta = {"x": torch.tensor([float(i + 1), float(4 - i)])}
        payload, _ = compress(CompressConfig("dense-int8", ef_beta=0), delta, {"x": torch.zeros(2)})
        delta_hash = store.put(payload)
        c = CommitV2(
            w=0,
            hotkey=r.hotkey,
            leaf_scheme="ht-leaf-v1",
            n_leaves=7,
            leaves_root=H,
            metrics_root=H,
            final_theta_hash=H,
            ef_in_hash=H,
            ef_out_hash=H,
            delta_hash=delta_hash,
            delta_bytes=len(payload),
            tokens=manifest.training.batch_samples() * manifest.training.model.seq_len,
        )
        d = DeltaManifestV2(
            w=0,
            hotkey=r.hotkey,
            delta_hash=delta_hash,
            uri="object",
            size=len(payload),
            format="ht-dense-int8-v1",
            chunks=[Chunk(off=0, len=len(payload), sha256=delta_hash)],
            grant_hash=H,
            master_acceptance_hash=H,
        )
        store.put(canonicalize(c.model_dump(mode="json")))
        store.put(canonicalize(d.model_dump(mode="json")))
        replay = ReplayEvidence(
            manifest.run_id(),
            0,
            r.hotkey,
            H,
            H,
            H,
            delta_hash,
            H,
            H,
            H,
            H,
            "MATCH",
            "anchored-full",
        )
        funding = FundedStatus(manifest.run_id(), r.hotkey, H, "test", 1000, 100, H, True)
        settlement = SettlementStatus(manifest.run_id(), 0, r.hotkey, False, False, H)
        works.append(VerifiedInput(r, c, d, replay, funding, settlement, H, 12))
    return manifest, works, previous


def test_real_payload_tape_when_replayed_independently(tmp_path: Path) -> None:
    # Given
    store = LocalFSStore(tmp_path)
    manifest, works, previous = fixture(store)
    args = dict(
        w=0, prev_state=previous, predecessor_tape_hash=H, inputs=works, reference_reward_units=100
    )
    tape = make_tape(store, manifest, policy(), canonicalize(economics().body()), KEY, **args)
    # When
    output = replay_tape(
        store,
        TapeV2.from_bytes(tape.to_bytes()),
        manifest,
        policy(),
        canonicalize(economics().body()),
        signer=KEY.ss58,
        **args,
    )
    # Then
    assert sha256_hex(output.to_bytes()) == tape.body.out_state
    assert not np.array_equal(output.theta["x"], np.array([2.0, 3.0], np.float32))


@pytest.mark.parametrize(
    "mutation",
    [
        "omitted",
        "duplicate",
        "assignment",
        "copied",
        "unfunded",
        "dispute",
        "shadow",
        "weight",
        "policy",
        "predecessor",
        "signature",
        "missing",
    ],
)
def test_tape_rejects_when_original_inputs_or_gates_altered(tmp_path: Path, mutation: str) -> None:
    # Given
    store = LocalFSStore(tmp_path)
    manifest, works, previous = fixture(store, 5 if mutation == "weight" else 4)
    args = dict(
        w=0, prev_state=previous, predecessor_tape_hash=H, inputs=works, reference_reward_units=100
    )
    tape = make_tape(store, manifest, policy(), canonicalize(economics().body()), KEY, **args)
    p = policy()
    match mutation:
        case "omitted":
            args["inputs"] = works[:-1]
        case "duplicate":
            args["inputs"] = works + [works[0]]
        case "assignment":
            works[0] = replace(works[0], assignment_hash="22" * 32)
        case "copied":
            works[0] = replace(
                works[0], replay=replace(works[0].replay, delta_hash=works[1].commit.delta_hash)
            )
        case "unfunded":
            works[0] = replace(works[0], funding=replace(works[0].funding, locked_units=0))
        case "dispute":
            works[0] = replace(
                works[0], settlement=replace(works[0].settlement, unresolved_dispute=True)
            )
        case "shadow":
            works[0] = replace(works[0], clean_finalizations=0)
        case "weight":
            entries = list(tape.body.allocation.entries)
            entries[0] = entries[0].model_copy(update={"weight_units": entries[0].weight_units - 1})
            entries[1] = entries[1].model_copy(update={"weight_units": entries[1].weight_units + 1})
            allocation = tape.body.allocation.model_validate(
                {"policy_hash": p.digest(), "entries": [e.body() for e in entries]}
            )
            body = tape.body.model_copy(update={"allocation": allocation})
            tape = TapeV2(
                body=body,
                signer=KEY.ss58,
                sig=KEY.sign(tape_signing_message(body.body(), manifest.run_id())).hex(),
            )
        case "policy":
            p = policy({"owner0": 1})
        case "predecessor":
            args["predecessor_tape_hash"] = "22" * 32
        case "signature":
            tape = tape.model_copy(update={"sig": "00" * 64})
        case "missing":
            store._path(works[0].commit.delta_hash).unlink()
    # When / Then
    with pytest.raises((ValueError, TapeError, RuntimeError)):
        replay_tape(
            store, tape, manifest, p, canonicalize(economics().body()), signer=KEY.ss58, **args
        )


def test_nonuniform_cclip_matches_scalar_oracle_when_owner_cap_saturates() -> None:
    # Given: previous update center, not momentum; nonuniform caps and clipping.
    candidates = [candidate(i, owner="small" if i == 0 else None) for i in range(5)]
    p = policy({"small": Q // 16}).model_copy(
        update={"preclip_norm": f32hex(2), "cclip_tau": f32hex(0.5), "cclip_iters": 2}
    )
    allocation = allocate_weights(candidates, p)
    previous = OuterState(
        {"x": np.array([4.0], np.float32)},
        {"x": np.array([7.0], np.float32)},
        {"x": np.array([0.3], np.float32)},
    )
    deltas = {
        c.roster.hotkey: {"x": np.array([i + 1.0], np.float32)} for i, c in enumerate(candidates)
    }
    params = OuterParams(
        "nesterov", f32hex(0.1), f32hex(0.9), p.preclip_norm, p.cclip_tau, p.cclip_iters
    )
    center = np.float32(0.3)
    for _ in range(2):
        acc = np.float32(0)
        for e in allocation.entries:
            d = deltas[e.hotkey]["x"][0]
            if d > 2:
                d = np.float32(d * np.float32(2 / float(d)))
            diff = np.float32(d - center)
            scale = np.float32(min(1.0, 0.5 / abs(float(diff))))
            acc = np.float32(acc + np.float32(e.weight_units / Q) * np.float32(diff * scale))
        center = np.float32(center + acc)
    u = np.float32(np.float32(0.9) * np.float32(7) + center)
    theta = np.float32(np.float32(4) - np.float32(0.1) * np.float32(center + np.float32(0.9) * u))
    # When
    result = aggregate_weighted(previous, deltas, allocation, p, params)
    # Then
    assert result.state.center["x"][0] == center
    assert result.state.u["x"][0] == u
    assert result.state.theta["x"][0] == theta


def test_tape_body_hash_deterministic_when_input_order_changes(tmp_path: Path) -> None:
    # Given
    store = LocalFSStore(tmp_path)
    manifest, works, previous = fixture(store)
    # When
    tapes = [
        make_tape(
            store,
            manifest,
            policy(),
            canonicalize(economics().body()),
            KEY,
            w=0,
            prev_state=previous,
            predecessor_tape_hash=H,
            inputs=order,
            reference_reward_units=100,
        )
        for order in (works, works[::-1])
    ]
    # Then: sr25519 signatures need not be deterministic; signed body digest must be.
    assert tapes[0].body.digest() == tapes[1].body.digest()


def test_completion_matches_exhaustive_cut_oracle_when_two_cohorts_two_owners() -> None:
    # Given: all 3^7 capacity combinations; active source arc cannot bind.
    edges = [(0, 1), (0, 2), (1, 3), (1, 4), (2, 3), (2, 4), (3, 5), (4, 5)]
    cuts = [
        {0} | {i + 1 for i, bit in enumerate(bits) if bit}
        for bits in product((False, True), repeat=4)
    ]
    for values in product(range(3), repeat=7):
        capacities = (8, *values)
        graph = [[0] * 6 for _ in range(6)]
        for (u, v), cap in zip(edges, capacities, strict=True):
            graph[u][v] = cap
        expected = min(
            sum(graph[u][v] for u, v in edges if u in cut and v not in cut) for cut in cuts
        )
        # When
        actual = _completion_capacity(
            [capacities[2], capacities[3]],
            [capacities[4], capacities[5]],
            [capacities[6], capacities[7]],
            capacities[1],
        )
        # Then: independent exhaustive min-cut, not a second augmenting-path algorithm.
        assert actual == expected


def test_four_miner_feasibility_matches_exhaustive_reference_when_caps_intersect() -> None:
    # Given: Q/4 ceiling forces the unique complete vector; enumerate every cohort mask.
    base = [candidate(i) for i in range(4)]
    partitions = [("A", "A", "B", "B"), ("A", "A", "A", "B"), ("A",) * 4]
    for probation in product((False, True), repeat=4):
        for owners in partitions:
            for owner_cap in (Q // 4, Q // 2, Q):
                candidates = [
                    replace(
                        c,
                        roster=c.roster.model_copy(
                            update={
                                "state": "PROBATION" if probation[i] else "ACTIVE",
                                "coldkey_group": owners[i],
                            }
                        ),
                    )
                    for i, c in enumerate(base)
                ]
                expected = sum(probation) <= 1 and all(
                    owners.count(group) * (Q // 4) <= owner_cap for group in set(owners)
                )
                # When / Then:144 exact integer cases, at most four identities.
                if expected:
                    allocation = allocate_weights(
                        candidates, policy({"A": owner_cap, "B": owner_cap})
                    )
                    assert [e.weight_units for e in allocation.entries] == [Q // 4] * 4
                else:
                    with pytest.raises(InsufficientEligibleWeight):
                        allocate_weights(candidates, policy({"A": owner_cap, "B": owner_cap}))


def test_maxmin_matches_exhaustive_integer_reference_when_small_residual_budget() -> None:
    # Given: four large owners can carry at most Q-8; small miners must carry at least8.
    ordered = sorted([candidate(i) for i in range(8)], key=lambda c: c.roster.hotkey.encode())
    fixed = ordered[:4]
    small = ordered[4:]
    for probation in product((False, True), repeat=4):
        for shared_cap in (3, 5, 8):
            eligible = (2, 3, 4, 5)
            candidates = [
                replace(
                    c,
                    roster=c.roster.model_copy(
                        update={"coldkey_group": f"fixed{i}", "eligible_weight": Q // 4}
                    ),
                )
                for i, c in enumerate(fixed)
            ]
            candidates += [
                replace(
                    c,
                    roster=c.roster.model_copy(
                        update={
                            "coldkey_group": "shared" if i < 2 else f"small{i}",
                            "state": "PROBATION" if probation[i] else "ACTIVE",
                            "eligible_weight": eligible[i],
                        }
                    ),
                )
                for i, c in enumerate(small)
            ]
            p = policy(
                {
                    "fixed0": Q // 4,
                    "fixed1": Q // 4,
                    "fixed2": Q // 4,
                    "fixed3": Q // 4 - 8,
                    "shared": shared_cap,
                }
            )
            feasible = [
                x
                for x in product(*(range(cap + 1) for cap in eligible))
                if sum(x) >= 8 and x[0] + x[1] <= shared_cap
            ]
            expected = max(feasible, key=lambda x: (tuple(sorted(x)), x))
            # When
            allocation = allocate_weights(candidates[::-1], p)
            # Then: exhaustive sorted-leximin optimum, UTF-8 remainder preference.
            assert tuple(e.weight_units for e in allocation.entries[4:]) == expected


@pytest.mark.parametrize("n", [128, 1024])
def test_scale_roster_when_unique_owner_caps_saturate_in_many_stages(n: int) -> None:
    # Given: n distinct owners, n/4 saturation breakpoints, two intersecting cohorts.
    from hypertrain.protocol.keys import encode_hotkey

    template = roster(0)
    candidates = [
        Candidate(
            template.model_copy(
                update={
                    "hotkey": encode_hotkey(i.to_bytes(32, "little")),
                    "slot": i,
                    "coldkey_group": f"owner{i}",
                    "state": "PROBATION" if i % 2 else "ACTIVE",
                }
            ),
            H,
            H,
        )
        for i in range(n)
    ]
    p = policy({f"owner{i}": i // 2 + 1 for i in range(n // 2)})
    # When: label measurements; never assert time or scheduling thresholds.
    cpu, wall = process_time(), perf_counter()
    allocation = allocate_weights(candidates, p)
    cpu, wall = process_time() - cpu, perf_counter() - wall
    tracemalloc.start()
    traced = allocate_weights(candidates[::-1], p)
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    print(f"SCALE n={n} cpu_s={cpu:.6f} wall_s={wall:.6f} traced_peak_bytes={peak}")
    # Then: deterministic arithmetic contract, not a timing-luck admission test.
    assert len(allocation.entries) == n
    assert traced == allocation
    assert sum(e.weight_units for e in allocation.entries) == Q
    assert sum(e.weight_units for e in allocation.entries if e.probation) <= Q // 4
    owners = {c.roster.hotkey: c.roster.coldkey_group for c in candidates}
    assert all(
        e.weight_units <= min(Q // 4, p.owner_group_caps.get(owners[e.hotkey], Q))
        for e in allocation.entries
    )


def test_oversized_roster_rejected_before_sort_when_exceeding_wire_ceiling() -> None:
    # Given: count check precedes duplicate-key validation or expensive arithmetic.
    candidates = [candidate(0)] * 1025
    # When / Then
    with pytest.raises(InsufficientEligibleWeight, match="oversized candidate set"):
        allocate_weights(candidates, policy())
