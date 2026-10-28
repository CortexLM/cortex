"""Real tiny carried work; two repaired rounds, signed independent negative controls."""

import os
from dataclasses import dataclass, replace
from functools import partial
from pathlib import Path
from subprocess import CalledProcessError, CompletedProcess
from typing import Any

import numpy as np
import pytest
import torch
from test_weighted_v2 import KEY, H, economics, policy, roster

from env_pin_fixtures import CPU_AVX2, HOSTILE, ambient
from hypertrain.aggregator import rollback_v2
from hypertrain.aggregator.core import OuterState, TapeError, TapeIntegrityError, load_state
from hypertrain.aggregator.rollback_v2 import (
    RepairAuthority,
    RepairResult,
    RepairSource,
    RepairTape,
    make_repair_tape,
    repair_message,
    replay_repair_tape,
)
from hypertrain.aggregator.tape_v2 import Exclusion, VerifiedInput, make_tape
from hypertrain.auditor.replay import AnchorCache, VerifiedAnchor, pack_state
from hypertrain.challenge.store import assignment_hash
from hypertrain.challenge.trust_v2 import FundedStatus, ReplayEvidence, SettlementStatus
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Chunk, f32hex
from hypertrain.protocol.messages_v2 import (
    CommitV2,
    DeltaManifestV2,
    DisputePolicyV2,
    RunManifestV2,
)
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import Comm, IslandAssignment, emulate, train_island
from hypertrain.trainer.loop import Assignment, train_round
from hypertrain.trainer.model import init_params, param_shapes


def tiny_manifest() -> RunManifestV2:
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
        compute_dtype="fp32",
    )
    b["inner"].update(
        H=2,
        J=1,
        micro_batch=1,
        grad_accum=1,
        state_policy="carry",
        rewarmup_steps=0,
    )
    b["inner"]["lr_schedule"].update(warmup=0, stable=100, peak_lr=f32hex(0.003))
    b["reference_spec"]["layout"].update(pp=1, n_gpus=1, dp_size=1, ep_size=1, zero1=False)
    cfg = TrainConfig.from_manifest(b)
    b["model"]["param_count"] = sum(int(np.prod(s)) for s in param_shapes(cfg.model).values())
    return RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": b,
            "network": {
                "admission_policy_hash": H,
                "economics_policy_hash": economics().digest(),
                "aggregation_policy_hash": policy().digest(),
                "dispute_policy_hash": H,
                "audit_policy_hash": H,
                "relay_registry_hash": H,
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )


def sample(i: int) -> np.ndarray:
    return (np.arange(5, dtype=np.uint32) + i) % 16


@dataclass(frozen=True)
class Scenario:
    store: LocalFSStore
    manifest: RunManifestV2
    previous: str
    sources: tuple[tuple[RepairSource, ...], ...]
    original_second_sources: tuple[RepairSource, ...]
    authorities: tuple[RepairAuthority, ...]
    excluded: tuple[Exclusion, ...]
    first: RepairResult
    second: RepairResult


@pytest.fixture(scope="module")
def scenario(tmp_path_factory: pytest.TempPathFactory) -> Scenario:
    store = LocalFSStore(tmp_path_factory.mktemp("rollback"))
    manifest = tiny_manifest()
    cfg = TrainConfig.from_manifest_v2(manifest)
    lay = manifest.training.reference_spec.layout
    theta = init_params(cfg.model)
    cache = AnchorCache()
    anchors = [cache.genesis(manifest, roster(i).hotkey, theta) for i in range(5)]
    previous = store.put(OuterState.init({n: x.numpy() for n, x in theta.items()}).to_bytes())
    current, predecessor = previous, "0" * 64
    sources, authorities = [], []
    for w in range(2):
        works, round_sources, next_anchors = [], [], []
        start = {n: torch.from_numpy(x.copy()) for n, x in load_state(store, current).theta.items()}
        for i, anchor in enumerate(anchors):
            r = roster(i)
            ids = (w * 20 + i * 2, w * 20 + i * 2 + 1)
            result = emulate(
                1,
                partial(
                    train_island,
                    cfg,
                    lay,
                    theta_start=start,
                    a=IslandAssignment(manifest.run_id(), w, ids, w * 2, 1),
                    get_sample=sample,
                    ef_in=anchor.ef,
                    carry=anchor.state,
                ),
            )[0]
            dh = store.put(result.delta_payload)
            commit = CommitV2(
                w=w,
                hotkey=r.hotkey,
                leaf_scheme="ht-leaf-v1",
                n_leaves=3,
                leaves_root=result.leaves_root,
                metrics_root=H,
                final_theta_hash=result.final_theta_hash,
                ef_in_hash=result.ef_in_hash,
                ef_out_hash=result.ef_out_hash,
                delta_hash=dh,
                delta_bytes=len(result.delta_payload),
                tokens=8,
            )
            delta = DeltaManifestV2(
                w=w,
                hotkey=r.hotkey,
                delta_hash=dh,
                uri="object",
                size=len(result.delta_payload),
                format="ht-dense-int8-v1",
                chunks=[Chunk(off=0, len=len(result.delta_payload), sha256=dh)],
                grant_hash=H,
                master_acceptance_hash=H,
            )
            ah = assignment_hash(manifest.run_id(), w, i, ids)
            work = VerifiedInput(
                r,
                commit,
                delta,
                ReplayEvidence(
                    manifest.run_id(),
                    w,
                    r.hotkey,
                    ah,
                    anchor.anchor_hash,
                    result.leaves_root,
                    dh,
                    result.final_theta_hash,
                    result.ef_in_hash,
                    result.ef_out_hash,
                    H,
                    "MATCH",
                    "anchored-full",
                ),
                FundedStatus(manifest.run_id(), r.hotkey, H, "test", 1000, 100, H, True),
                SettlementStatus(manifest.run_id(), w, r.hotkey, False, False, H),
                ah,
                12,
            )
            miner = Keypair(bytes([i + 1]) * 32)
            ce = canonicalize(seal(miner, "CommitV2", manifest.run_id(), commit, 100))
            de = canonicalize(seal(miner, "DeltaManifestV2", manifest.run_id(), delta, 100))
            for blob in (
                canonicalize(commit.model_dump(mode="json")),
                canonicalize(delta.model_dump(mode="json")),
                ce,
                de,
            ):
                store.put(blob)
            round_sources.append(RepairSource(work, ce, de, sha256_hex(ce + de), ids, anchor))
            works.append(work)
            next_anchors.append(
                VerifiedAnchor(
                    manifest.run_id(),
                    r.hotkey,
                    w,
                    anchor.layout_hash,
                    result.final_state,
                    result.ef_out,
                    H,
                    sha256_hex(pack_state(result.final_theta, result.final_state)),
                    result.final_theta,
                    "cpu",
                )
            )
        tape = make_tape(
            store,
            manifest,
            policy(),
            canonicalize(economics().body()),
            KEY,
            w=w,
            prev_state=current,
            predecessor_tape_hash=predecessor,
            inputs=works,
            reference_reward_units=100,
        )
        predecessor = store.put(tape.to_bytes())
        authorities.append(
            RepairAuthority(
                KEY.ss58,
                predecessor,
                H,
                sha256_hex(canonicalize(manifest.training.reference_spec.model_dump(mode="json"))),
                AnchorCache.layout_hash(manifest),
                "cpu",
            )
        )
        sources.append(tuple(round_sources))
        current, anchors = tape.body.out_state, next_anchors
    excluded = (Exclusion(hotkey=roster(4).hotkey, reason="FRAUD", evidence_hash=H),)

    def repair(comm: Comm) -> RepairResult:
        return make_repair_tape(
            store,
            manifest,
            policy(),
            canonicalize(economics().body()),
            KEY,
            authorities[0],
            comm,
            sample,
            w=0,
            prev_state=previous,
            predecessor_tape_hash="0" * 64,
            sources=sources[0][:4],
            excluded=excluded,
            reference_reward_units=100,
        )

    first = emulate(1, repair)[0]
    repaired_anchors = {a.hotkey: a for a in first.anchors}
    second_sources = tuple(
        replace(s, anchor=repaired_anchors[s.work.roster.hotkey]) for s in sources[1][:4]
    )
    second = emulate(
        1,
        lambda comm: make_repair_tape(
            store,
            manifest,
            policy(),
            canonicalize(economics().body()),
            KEY,
            authorities[1],
            comm,
            sample,
            w=1,
            prev_state=first.tape.body.arithmetic.out_state,
            predecessor_tape_hash=store.put(first.tape.to_bytes()),
            sources=second_sources,
            excluded=excluded,
            reference_reward_units=100,
        ),
    )[0]
    return Scenario(
        store,
        manifest,
        previous,
        (sources[0][:4], second_sources),
        sources[1][:4],
        tuple(authorities),
        excluded,
        first,
        second,
    )


def accept(
    s: Scenario,
    tape: RepairTape,
    *,
    w: int,
    sources: tuple[RepairSource, ...],
    authority: RepairAuthority,
    previous: str,
    predecessor: str,
) -> RepairResult:
    return emulate(
        1,
        lambda comm: replay_repair_tape(
            s.store,
            tape,
            s.manifest,
            policy(),
            canonicalize(economics().body()),
            authority,
            comm,
            sample,
            w=w,
            prev_state=previous,
            predecessor_tape_hash=predecessor,
            sources=sources,
            excluded=s.excluded,
            reference_reward_units=100,
        ),
    )[0]


def test_two_round_repair_retains_state_and_recomputes_changed_theta(scenario: Scenario) -> None:
    # Given: two original tapes, exclusion; fresh reader has no repair cache.
    s = scenario
    # When: independently rerun both rounds, retain the first replay's actual m/v/EF.
    first = accept(
        s,
        RepairTape.from_bytes(s.first.tape.to_bytes()),
        w=0,
        sources=s.sources[0],
        authority=s.authorities[0],
        previous=s.previous,
        predecessor="0" * 64,
    )
    anchors = {a.hotkey: a for a in first.anchors}
    sources = tuple(replace(x, anchor=anchors[x.work.roster.hotkey]) for x in s.sources[1])
    second = accept(
        s,
        RepairTape.from_bytes(s.second.tape.to_bytes()),
        w=1,
        sources=sources,
        authority=s.authorities[1],
        previous=first.tape.body.arithmetic.out_state,
        predecessor=sha256_hex(first.tape.to_bytes()),
    )
    # Then: w+1 original deltas are not substituted; actual carry reaches step4.
    assert all(a.state.step == 4 for a in second.anchors)
    assert second.state.to_bytes() == s.second.state.to_bytes()
    assert any(
        e.delta_hash != x.work.commit.delta_hash
        for e in second.tape.body.recomputed
        for x in sources
        if e.hotkey == x.work.roster.hotkey
    )
    assert all(
        e.start_optimizer_hash == a.final_optimizer_hash
        for e in second.tape.body.recomputed
        for a in first.tape.body.recomputed
        if e.hotkey == a.hotkey
    )


def test_repaired_inner_work_matches_independent_single_rank_trainer(scenario: Scenario) -> None:
    # Given: exact N1 layout; separate train_round implementation, real retained state.
    s = scenario
    cfg = TrainConfig.from_manifest_v2(s.manifest)
    theta = {n: torch.from_numpy(x.copy()) for n, x in s.first.state.theta.items()}
    expected = {e.hotkey: e for e in s.second.tape.body.recomputed}
    # When: independently run original w+1 assignments at the repaired global theta.
    for source in s.sources[1]:
        result = train_round(
            cfg,
            theta,
            Assignment(s.manifest.run_id(), 1, source.sample_ids, 2),
            sample,
            ef_in=source.anchor.ef,
            carry=source.anchor.state,
        )
        # Then: no stale delta, reset optimizer, lost EF or changed trainer can pass.
        e = expected[source.work.roster.hotkey]
        assert result.delta_hash == e.delta_hash
        assert result.leaves_root == e.leaves_root
        assert pack_state(result.final_theta, result.final_state) == s.store.get(
            e.final_state_object
        )
        assert pack_state(result.ef_out) == s.store.get(e.final_ef_object)


@pytest.mark.parametrize(
    "mutation",
    [
        "predecessor",
        "delta",
        "anchor",
        "layout",
        "reference",
        "qualification",
        "source_authority",
        "source_tape",
        "acceptance",
        "assignment",
        "old_carry",
        "original_carry",
        "anchor_layout",
        "anchor_backend",
        "source_delta_authority",
        "signer",
    ],
)
def test_independent_replay_rejects_resigned_false_provenance(
    scenario: Scenario,
    mutation: str,
) -> None:
    # Given: mutate candidate, re-sign with real coordinator; expected authorities unchanged.
    s = scenario
    body = s.second.tape.body.model_dump(mode="json")
    sources = s.sources[1]
    other = "22" * 32
    match mutation:
        case "predecessor":
            body["arithmetic"]["predecessor_tape_hash"] = other
        case "delta":
            body["recomputed"][0]["delta_hash"] = s.sources[1][0].work.commit.delta_hash
        case "anchor":
            body["recomputed"][0]["source_anchor_hash"] = other
        case "layout" | "reference" | "qualification" | "source_tape":
            field = {
                "layout": "layout_hash",
                "reference": "reference_hash",
                "qualification": "qualification_hash",
                "source_tape": "source_tape_hash",
            }[mutation]
            body[field] = other
        case "acceptance":
            body["recomputed"][0]["acceptance_hash"] = other
        case "assignment":
            body["recomputed"][0]["sample_ids"] = [999, 1000]
        case "source_authority":
            fake = canonicalize(
                seal(KEY, "CommitV2", s.manifest.run_id(), sources[0].work.commit, 100)
            )
            sources = (replace(sources[0], commit_envelope=fake), *sources[1:])
        case "old_carry":
            original = s.sources[0][0].anchor
            sources = (replace(sources[0], anchor=original), *sources[1:])
        case "original_carry":
            sources = s.original_second_sources
        case "anchor_layout":
            anchor = replace(sources[0].anchor, layout_hash=other)
            sources = (replace(sources[0], anchor=anchor), *sources[1:])
        case "anchor_backend":
            anchor = replace(sources[0].anchor, backend="cuda")
            sources = (replace(sources[0], anchor=anchor), *sources[1:])
        case "source_delta_authority":
            fake = canonicalize(
                seal(
                    KEY,
                    "DeltaManifestV2",
                    s.manifest.run_id(),
                    sources[0].work.delta_manifest,
                    100,
                )
            )
            sources = (replace(sources[0], delta_envelope=fake), *sources[1:])
        case "signer":
            pass
    parsed = s.second.tape.body.model_validate(body)
    key = Keypair(bytes([99]) * 32) if mutation == "signer" else KEY
    tape = RepairTape(body=parsed, signer=key.ss58, sig=key.sign(repair_message(parsed)).hex())
    # When / Then
    with pytest.raises(TapeError):
        accept(
            s,
            tape,
            w=1,
            sources=sources,
            authority=s.authorities[1],
            previous=s.first.tape.body.arithmetic.out_state,
            predecessor=sha256_hex(s.first.tape.to_bytes()),
        )


@pytest.mark.parametrize(
    "part,value",
    [
        ("ef", float("nan")),
        ("m", float("nan")),
        ("v", float("inf")),
        ("theta", float("nan")),
    ],
)
@pytest.mark.parametrize("replay", [False, True])
def test_nonfinite_authenticated_anchor_rejects_before_publication(
    scenario: Scenario,
    monkeypatch: pytest.MonkeyPatch,
    part: str,
    value: float,
    replay: bool,
) -> None:
    # Given: same authenticated metadata; poisoned actual decoded carry tensors.
    s = scenario
    anchor = s.sources[1][0].anchor
    original = getattr(anchor, part) if part in ("ef", "theta") else getattr(anchor.state, part)
    values = {n: x.clone() for n, x in original.items()}
    next(iter(values.values())).view(-1)[0] = value
    bad = (
        replace(anchor, **{part: values})
        if part in ("ef", "theta")
        else replace(anchor, state=replace(anchor.state, **{part: values}))
    )
    sources = (replace(s.sources[1][0], anchor=bad), *s.sources[1][1:])

    def no_publication(raw: bytes) -> str:
        pytest.fail("invalid carry published an object")

    monkeypatch.setattr(s.store, "put", no_publication)
    # When / Then: authentic signature never authorizes nonfinite state.
    with pytest.raises(TapeIntegrityError, match="nonfinite repair"):
        if replay:
            accept(
                s,
                s.second.tape,
                w=1,
                sources=sources,
                authority=s.authorities[1],
                previous=s.first.tape.body.arithmetic.out_state,
                predecessor=sha256_hex(s.first.tape.to_bytes()),
            )
        else:
            emulate(
                1,
                lambda comm: make_repair_tape(
                    s.store,
                    s.manifest,
                    policy(),
                    canonicalize(economics().body()),
                    KEY,
                    s.authorities[1],
                    comm,
                    sample,
                    w=1,
                    prev_state=s.first.tape.body.arithmetic.out_state,
                    predecessor_tape_hash=sha256_hex(s.first.tape.to_bytes()),
                    sources=sources,
                    excluded=s.excluded,
                    reference_reward_units=100,
                ),
            )


def test_finite_input_terminal_overflow_rejects_before_publication(
    scenario: Scenario,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Given: finite full carry, overflow in actual H-step model forward/optimizer.
    s = scenario
    anchor = s.sources[0][0].anchor
    values = {n: torch.full_like(x, 3e38) for n, x in anchor.state.m.items()}
    assert all(bool(torch.isfinite(x).all()) for x in values.values())
    bad = replace(anchor, state=replace(anchor.state, m=values))
    sources = (replace(s.sources[0][0], anchor=bad), *s.sources[0][1:])

    def no_publication(raw: bytes) -> str:
        pytest.fail("terminal overflow published an object")

    monkeypatch.setattr(s.store, "put", no_publication)
    # When / Then
    with pytest.raises((TapeError, ValueError)):
        emulate(
            1,
            lambda comm: make_repair_tape(
                s.store,
                s.manifest,
                policy(),
                canonicalize(economics().body()),
                KEY,
                s.authorities[0],
                comm,
                sample,
                w=0,
                prev_state=s.previous,
                predecessor_tape_hash="0" * 64,
                sources=sources,
                excluded=s.excluded,
                reference_reward_units=100,
            ),
        )


def pinned_manifest() -> RunManifestV2:
    body = tiny_manifest().body()
    body["training"]["reference_spec"]["env"]["pinned"] = CPU_AVX2
    return RunManifestV2.model_validate(body)


PINNED_WORKER_ENV = {
    "ATEN_CPU_CAPABILITY": "avx2",
    "HT_ATTN_BACKEND": "math",
    "MKL_CBWR": "AVX2",
    "OMP_DYNAMIC": "FALSE",
    "ONEDNN_MAX_CPU_ISA": "AVX2",
    "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
    "OMP_NUM_THREADS": "1",
}


def test_worker_env_follows_the_manifest_pins(monkeypatch: pytest.MonkeyPatch) -> None:
    ambient(monkeypatch, **HOSTILE)
    ambient_env = dict(os.environ)
    env = rollback_v2._worker_env(pinned_manifest())
    assert {k: env.get(k) for k in [*HOSTILE, "CUBLAS_WORKSPACE_CONFIG", "OMP_NUM_THREADS"]} == {
        **PINNED_WORKER_ENV,
        "HT_FP8": None,
        "HT_MOE_GROUPED": None,
        "OMP_THREAD_LIMIT": None,
    }
    assert os.environ == ambient_env


def record_environment_then_fail(seen: dict[str, str]) -> Any:
    """Stand-in for subprocess.run in rollback_v2: the worker never starts."""

    def run(argv: list[str], *, env: dict[str, str], check: bool = False, **kwargs: Any) -> Any:
        seen.update(env)
        if check:
            raise CalledProcessError(1, argv, b"", b"stopped after reading the environment")
        return CompletedProcess(argv, 1, b"", b"stopped after reading the environment")

    return run


def signed_context(key: Keypair, body: dict[str, Any]) -> dict[str, Any]:
    message = b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(body)).encode()
    return {"signer": key.ss58, "body": body, "sig": key.sign(message).hex()}


def assert_pinned_worker_environment(seen: dict[str, str]) -> None:
    assert {k: seen.get(k) for k in PINNED_WORKER_ENV} == PINNED_WORKER_ENV
    assert not {"HT_FP8", "HT_MOE_GROUPED", "OMP_THREAD_LIMIT"} & set(seen)


def test_history_worker_is_started_under_the_manifest_pins(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ambient(monkeypatch, **HOSTILE)
    manifest, key, seen = pinned_manifest(), Keypair(bytes([7]) * 32), {}
    context = signed_context(key, {"manifest": manifest.body(), "timeout_seconds": 5})
    monkeypatch.setattr(rollback_v2.subprocess, "run", record_environment_then_fail(seen))
    with pytest.raises(TapeIntegrityError, match="history replay failed: stopped after reading"):
        rollback_v2.replay_history_context(LocalFSStore(tmp_path), manifest, context, key.ss58)
    assert_pinned_worker_environment(seen)


def repair_context(store: LocalFSStore, key: Keypair) -> tuple[RunManifestV2, dict[str, Any]]:
    """A pinned manifest and the signed round-0 repair context of `key`, with no source."""
    referees = DisputePolicyV2(
        referees=[key.ss58],
        max_open=32,
        max_transcript_entries=256,
        max_entry_bytes=65536,
        fanout=2,
        max_referee_reassignments=1,
        max_referee_cost_units=1,
    )
    document = pinned_manifest().body()
    document["network"]["dispute_policy_hash"] = store.put(referees.model_dump_json().encode())
    manifest = RunManifestV2.model_validate(document)
    qualification = {"execution_backend": {"backend": "cpu", "authority_hash": None}}
    context = signed_context(
        key,
        {
            "run_id": manifest.run_id(),
            "backend": "cpu",
            "qualification": qualification,
            "qualification_hash": sha256_hex(canonicalize(qualification)),
            "manifest": manifest.body(),
            "w": 0,
            "timeout_seconds": 5,
            "exclusion_proofs": [],
            "excluded": [],
            "sources": [],
        },
    )
    return manifest, context


def test_repair_worker_is_started_under_the_manifest_pins(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ambient(monkeypatch, **HOSTILE)
    store, key, seen = LocalFSStore(tmp_path), Keypair(bytes([7]) * 32), {}
    manifest, context = repair_context(store, key)
    monkeypatch.setattr(rollback_v2.subprocess, "run", record_environment_then_fail(seen))
    with pytest.raises(TapeIntegrityError, match="repair worker failed: stopped after reading"):
        rollback_v2.execute_repair_context(store, manifest, context, key.ss58)
    assert_pinned_worker_environment(seen)


@pytest.mark.parametrize("agrees", [True, False], ids=["same-body", "other-body"])
def test_a_fresh_worker_that_disagrees_with_the_signed_tape_is_a_fault_of_the_master(
    scenario: Scenario, monkeypatch: pytest.MonkeyPatch, agrees: bool
) -> None:
    # Given: a repair tape the coordinator signed, and a fresh worker that recomputes the
    # scenario's second repair body
    store, key = scenario.store, Keypair(bytes([7]) * 32)
    manifest, context = repair_context(store, key)
    recomputed = scenario.second.tape.body

    def worker(argv: list[str], **kwargs: Any) -> Any:
        (Path(argv[-2]) / "body.json").write_bytes(recomputed.model_dump_json().encode())
        return CompletedProcess(argv, 0, b"", b"")

    monkeypatch.setattr(rollback_v2.subprocess, "run", worker)
    body = recomputed if agrees else recomputed.model_copy(update={"source_tape_hash": "00" * 32})
    tape = RepairTape(body=body, signer=key.ss58, sig=key.sign(repair_message(body)).hex())
    # When / Then: the same bytes are accepted; other bytes under a valid signature are the
    # master's own pipeline diverging, never a conflict a caller could fix (the app answers 500)
    if agrees:
        assert rollback_v2.execute_repair_context(store, manifest, context, key.ss58, tape=tape)
    else:
        with pytest.raises(TapeIntegrityError, match="independent repair context replay differs"):
            rollback_v2.execute_repair_context(store, manifest, context, key.ss58, tape=tape)
