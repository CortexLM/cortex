"""Exact independent history/CAS probes; tiny actual inner carry, no activation grid."""

from __future__ import annotations

import importlib.util
import sqlite3
import sys
import threading
from functools import partial
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest

import hypertrain.trainer  # noqa: F401
from hypertrain.aggregator.core import OuterState, TapeError
from hypertrain.aggregator.rollback_v2 import (
    RepairAuthority,
    RepairSource,
    _qualification_body,
    _replay_history,
    make_repair_tape,
    replay_repair_tape,
)
from hypertrain.aggregator.tape_v2 import make_tape
from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.challenge.store import ChallengeStore, assignment_hash
from hypertrain.challenge.trust_v2 import FundedStatus, ReplayEvidence, SettlementStatus
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Chunk
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    AuditJobV2,
    CommitV2,
    DeltaManifestV2,
    StartStateV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import IslandAssignment, emulate, train_island
from hypertrain.trainer.model import init_params

sys.path.insert(0, str(Path(__file__).parents[1] / "aggregator"))
spec = importlib.util.spec_from_file_location(
    "rollback_tiny", Path(__file__).parents[1] / "aggregator/test_rollback_v2.py"
)
assert spec is not None and spec.loader is not None
tiny = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = tiny
spec.loader.exec_module(tiny)


class MemoryStore:
    def __init__(self):
        self.objects = {}

    def put(self, raw):
        key = sha256_hex(raw)
        self.objects[key] = raw
        return key

    def get(self, key):
        return self.objects[key]


def test_exact_cas_probe_binds_original_envelope_even_same_digest_and_receipt():
    # Given: same SQLite snapshot contract as independent reviewer, all original data unchanged.
    store = ChallengeStore.__new__(ChallengeStore)
    store._lock = threading.RLock()
    store._db = sqlite3.connect(":memory:", isolation_level=None)
    store._db.executescript("""
    CREATE TABLE records_v2(run_id,kind,id,data);
    CREATE TABLE admissions_v2(hotkey,clean_count);
    CREATE TABLE disputes_v2(id,turn);
    CREATE TABLE accepted_v2(key,digest,envelope,accepted_beacon,receipt);
    CREATE TABLE escrow_units(origin,owner,bucket,ref,units);
    INSERT INTO accepted_v2 VALUES('source','unchanged',X'010203',1,'{}');
    """)
    before = store._rollback_snapshot_v2("run")
    # When: mutate original signed bytes, retaining normalized digest/receipt/state.
    store._db.execute("BEGIN")
    store._db.execute("UPDATE accepted_v2 SET envelope=X'010204'")
    after = store._rollback_snapshot_v2("run")
    store._db.execute("ROLLBACK")
    # Then: CAS detects exact historical byte change; rollback restores snapshot.
    assert after != before
    assert store._rollback_snapshot_v2("run") == before


@pytest.fixture(scope="module")
def ordinary_history():
    from hypertrain.aggregator.tape_v2 import VerifiedInput

    store = MemoryStore()
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.protocol.messages_v2 import RunManifestV2

    manifest_body = tiny.tiny_manifest().body()
    rows = np.array([tiny.sample(i) for i in range(32)], dtype="<u4")
    tree = MerkleTree([row.tobytes() for row in rows])
    manifest_body["training"]["dataset"].update(
        n_samples=32, merkle_root=tree.root.hex(), sample_format="u32[seq_len+1] token ids"
    )
    manifest = RunManifestV2.model_validate(manifest_body)
    store.put(canonicalize(tiny.policy().body()))
    store.put(canonicalize(tiny.economics().body()))
    cfg = TrainConfig.from_manifest_v2(manifest)
    theta = init_params(cfg.model)
    initial = store.put(OuterState.init({n: x.numpy() for n, x in theta.items()}).to_bytes())
    jobs, works = [], []
    for i in range(5):
        r = tiny.roster(i)
        anchor = AnchorCache().genesis(manifest, r.hotkey, theta)
        ids = (2 * i, 2 * i + 1)
        result = emulate(
            1,
            partial(
                train_island,
                cfg,
                manifest.training.reference_spec.layout,
                theta_start=theta,
                a=IslandAssignment(manifest.run_id(), 0, ids, 0, 1),
                get_sample=tiny.sample,
                carry=anchor.state,
                ef_in=anchor.ef,
            ),
        )[0]
        commit = CommitV2(
            w=0,
            hotkey=r.hotkey,
            leaf_scheme="ht-leaf-v1",
            n_leaves=3,
            leaves_root=result.leaves_root,
            metrics_root=tiny.H,
            final_theta_hash=result.final_theta_hash,
            ef_in_hash=result.ef_in_hash,
            ef_out_hash=result.ef_out_hash,
            delta_hash=store.put(result.delta_payload),
            delta_bytes=len(result.delta_payload),
            tokens=8,
        )
        delta = DeltaManifestV2(
            w=0,
            hotkey=r.hotkey,
            delta_hash=commit.delta_hash,
            uri="object",
            size=commit.delta_bytes,
            format="ht-dense-int8-v1",
            chunks=[Chunk(off=0, len=commit.delta_bytes, sha256=commit.delta_hash)],
            grant_hash=tiny.H,
            master_acceptance_hash=tiny.H,
        )
        start = StartStateV2(
            run_id=manifest.run_id(),
            w=0,
            hotkey=r.hotkey,
            theta_hash=state_hash(theta),
            state_object_sha256=store.put(pack_state(theta, anchor.state)),
            opt_state_hash=optimizer_hash(anchor.state),
            ef_object_sha256=store.put(pack_state(anchor.ef)),
            ef_hash=state_hash(anchor.ef),
            parent_anchor_hash=anchor.anchor_hash,
            global_step0=0,
            anchor_verdict_hash=anchor.proof_hash,
        )
        challenge = AuditChallengeV2(
            w=0,
            target=r.hotkey,
            beacon_round=2,
            beacon_sig_sha256=tiny.H,
            mode="full",
            segments=[],
            reasons=["final"],
            serve_deadline=100,
            anchor_hash=start.digest(),
            audit_mode="anchored-full",
        )
        job = AuditJobV2(
            run_id=manifest.run_id(),
            job_id=f"{i + 10:064x}",
            auditor_id=tiny.KEY.ss58,
            attempt=1,
            lease_nonce=tiny.H,
            lease_expires=40,
            absolute_deadline=60,
            reservation_id=f"{i + 1:064x}",
            replay_step_budget=2,
            anchor_age=0,
            manifest=manifest,
            challenge_envelope=seal(
                tiny.KEY, "AuditChallengeV2", manifest.run_id(), challenge, 100
            ),
            commit_envelope=seal(
                Keypair(bytes([i + 1]) * 32), "CommitV2", manifest.run_id(), commit, 100
            ),
            sample_ids=list(ids),
            start_state=start,
            preimages=[leaf.preimage for leaf in result.leaves],
            ef_in={
                "sha256": start.ef_object_sha256,
                "size": len(store.get(start.ef_object_sha256)),
            },
            v0={"sha256": store.put(pack_state(theta)), "size": len(pack_state(theta))},
            created_beacon=2,
        )
        assignment = assignment_hash(manifest.run_id(), 0, i, ids)
        work = VerifiedInput(
            r,
            commit,
            delta,
            ReplayEvidence(
                manifest.run_id(),
                0,
                r.hotkey,
                assignment,
                start.digest(),
                result.leaves_root,
                commit.delta_hash,
                commit.final_theta_hash,
                commit.ef_in_hash,
                commit.ef_out_hash,
                job.digest(),
                "MATCH",
                "anchored-full",
            ),
            FundedStatus(manifest.run_id(), r.hotkey, tiny.H, "test", 1000, 100, tiny.H, True),
            SettlementStatus(manifest.run_id(), 0, r.hotkey, False, False, tiny.H),
            assignment,
            12,
        )
        works.append(work)
        jobs.append(job.model_dump(mode="json"))
        store.put(canonicalize(commit.model_dump(mode="json")))
        store.put(canonicalize(delta.model_dump(mode="json")))
    tape = make_tape(
        store,
        manifest,
        tiny.policy(),
        canonicalize(tiny.economics().body()),
        tiny.KEY,
        w=0,
        prev_state=initial,
        predecessor_tape_hash="0" * 64,
        inputs=works,
        reference_reward_units=100,
    )
    inputs = [
        {
            "roster": x.roster.body(),
            "commit": x.commit.model_dump(mode="json"),
            "delta_manifest": x.delta_manifest.model_dump(mode="json"),
            "replay": x.replay.__dict__ if hasattr(x.replay, "__dict__") else {},
            "funding": {},
            "settlement": {},
            "assignment_hash": x.assignment_hash,
            "clean_finalizations": 12,
        }
        for x in works
    ]
    from dataclasses import asdict

    for record, work in zip(inputs, works, strict=True):
        record.update(
            replay=asdict(work.replay),
            funding=asdict(work.funding),
            settlement=asdict(work.settlement),
        )
    opening = seal(
        tiny.KEY,
        "RoundOpenV2",
        manifest.run_id(),
        {
            "w": 0,
            "roster": [x.roster.body() for x in works],
            "theta_hash": state_hash(theta),
            "prev_final_hash": tiny.H,
            "outer_state_hash": tiny.H,
            "center_hash": tiny.H,
            "roster_hash": sha256_hex(canonicalize([x.roster.body() for x in works])),
            "honeypot_commit": tiny.H,
            "d_open": 1,
            "d_assign": 2,
            "d_commit": 3,
            "d_audit": 4,
            "d_upload": 5,
            "d_final": 6,
            "contract_version": 2,
            "registry_epoch": 0,
            "policy_hashes": {
                name: getattr(manifest.network, name)
                for name in (
                    "admission_policy_hash",
                    "economics_policy_hash",
                    "aggregation_policy_hash",
                    "dispute_policy_hash",
                    "audit_policy_hash",
                )
            },
            "start_state_index_hash": tiny.H,
            "audit_mode": "anchored-full",
        },
        100,
    )
    entry = {
        "tape_hash": store.put(tape.to_bytes()),
        "inputs": inputs,
        "audit_jobs": jobs,
        "round_open": opening,
        "reference_reward_units": 100,
    }
    return store, manifest, entry


def test_authenticated_ordinary_history_reconstructs_honest_w1_carry(ordinary_history):
    store, manifest, entry = ordinary_history
    anchors = emulate(
        1,
        lambda comm: _replay_history(
            store, manifest, [entry], tiny.KEY.ss58, comm, tiny.sample, "cpu"
        ),
    )[0]
    assert len(anchors) == 5
    assert all(a.w == 0 and a.state.step == 2 for a in anchors)


def test_fresh_exact_layout_history_worker_authenticates_actual_w0_carry(
    ordinary_history, tmp_path
):
    import json

    from hypertrain.aggregator.rollback_v2 import replay_history_context
    from hypertrain.data.store import LocalFSStore
    from hypertrain.protocol.hashing import MerkleTree

    memory, manifest, entry = ordinary_history
    store = LocalFSStore(tmp_path)
    for raw in memory.objects.values():
        store.put(raw)
    rows = np.array([tiny.sample(i) for i in range(32)], dtype="<u4")
    tree = MerkleTree([row.tobytes() for row in rows])
    body = {
        "manifest": manifest.body(),
        "history": [entry],
        "sources": [],
        "backend": "cpu",
        "timeout_seconds": 60,
        "qualification": {"execution_backend": {"backend": "cpu", "authority_hash": None}},
        "samples_hash": store.put(rows.tobytes()),
        "proofs_hash": store.put(
            json.dumps([[p.hex() for p in tree.proof(i)] for i in range(32)]).encode()
        ),
    }
    context = {
        "body": body,
        "signer": tiny.KEY.ss58,
        "sig": tiny.KEY.sign(
            b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(body)).encode()
        ).hex(),
    }
    anchors = replay_history_context(store, manifest, context, tiny.KEY.ss58)
    assert len(anchors) == 5 and all(a.state.step == 2 for a in anchors)


@pytest.mark.parametrize("mutation", ["theta", "optimizer", "ef", "signature", "missing"])
def test_authenticated_history_rejects_forged_predecessor_carry(ordinary_history, mutation):
    from copy import deepcopy

    from hypertrain.auditor.replay import unpack_state

    store, manifest, entry = ordinary_history
    candidate = deepcopy(entry)
    job = candidate["audit_jobs"][0]
    if mutation == "missing":
        candidate["audit_jobs"].pop()
    elif mutation == "signature":
        job["commit_envelope"]["sig"] = "0" * 128
    else:
        if mutation == "ef":
            values, _ = unpack_state(store.get(job["ef_in"]["sha256"]))
            next(iter(values.values())).view(-1)[0] = 0.25
            raw = pack_state(values)
            job["ef_in"] = {"sha256": store.put(raw), "size": len(raw)}
        else:
            theta, state = unpack_state(store.get(job["start_state"]["state_object_sha256"]))
            target = theta if mutation == "theta" else state.m
            next(iter(target.values())).view(-1)[0] += 0.25
            job["start_state"]["state_object_sha256"] = store.put(pack_state(theta, state))
    with pytest.raises((TapeError, ValueError)):
        emulate(
            1,
            lambda comm: _replay_history(
                store, manifest, [candidate], tiny.KEY.ss58, comm, tiny.sample, "cpu"
            ),
        )


def test_honest_non_genesis_repair_uses_actual_reconstructed_optimizer_ef(ordinary_history):

    import torch

    from hypertrain.aggregator.core import load_state
    from hypertrain.aggregator.tape_v2 import Exclusion, VerifiedInput

    store, manifest, entry = ordinary_history
    anchors = emulate(
        1,
        lambda comm: _replay_history(
            store, manifest, [entry], tiny.KEY.ss58, comm, tiny.sample, "cpu"
        ),
    )[0]
    ordinary = __import__("hypertrain.aggregator.tape_v2", fromlist=["TapeV2"]).TapeV2.from_bytes(
        store.get(entry["tape_hash"])
    )
    previous = ordinary.body.out_state
    theta = {n: torch.from_numpy(x.copy()) for n, x in load_state(store, previous).theta.items()}
    cfg = TrainConfig.from_manifest_v2(manifest)
    sources = []
    for i, anchor in enumerate(sorted(anchors, key=lambda a: a.hotkey)):
        roster = next(tiny.roster(j) for j in range(5) if tiny.roster(j).hotkey == anchor.hotkey)
        ids = (20 + 2 * i, 21 + 2 * i)
        result = emulate(
            1,
            partial(
                train_island,
                cfg,
                manifest.training.reference_spec.layout,
                theta_start=theta,
                a=IslandAssignment(manifest.run_id(), 1, ids, 2, 1),
                get_sample=tiny.sample,
                carry=anchor.state,
                ef_in=anchor.ef,
            ),
        )[0]
        commit = CommitV2(
            w=1,
            hotkey=anchor.hotkey,
            leaf_scheme="ht-leaf-v1",
            n_leaves=3,
            leaves_root=result.leaves_root,
            metrics_root=tiny.H,
            final_theta_hash=result.final_theta_hash,
            ef_in_hash=result.ef_in_hash,
            ef_out_hash=result.ef_out_hash,
            delta_hash=store.put(result.delta_payload),
            delta_bytes=len(result.delta_payload),
            tokens=8,
        )
        delta = DeltaManifestV2(
            w=1,
            hotkey=anchor.hotkey,
            delta_hash=commit.delta_hash,
            uri="object",
            size=commit.delta_bytes,
            format="ht-dense-int8-v1",
            chunks=[Chunk(off=0, len=commit.delta_bytes, sha256=commit.delta_hash)],
            grant_hash=tiny.H,
            master_acceptance_hash=tiny.H,
        )
        assignment = assignment_hash(manifest.run_id(), 1, roster.slot, ids)
        work = VerifiedInput(
            roster,
            commit,
            delta,
            ReplayEvidence(
                manifest.run_id(),
                1,
                anchor.hotkey,
                assignment,
                anchor.anchor_hash,
                result.leaves_root,
                commit.delta_hash,
                commit.final_theta_hash,
                commit.ef_in_hash,
                commit.ef_out_hash,
                tiny.H,
                "MATCH",
                "anchored-full",
            ),
            FundedStatus(manifest.run_id(), anchor.hotkey, tiny.H, "test", 1000, 100, tiny.H, True),
            SettlementStatus(manifest.run_id(), 1, anchor.hotkey, False, False, tiny.H),
            assignment,
            12,
        )
        key = Keypair(bytes([roster.slot + 1]) * 32)
        ce = canonicalize(seal(key, "CommitV2", manifest.run_id(), commit, 100))
        de = canonicalize(seal(key, "DeltaManifestV2", manifest.run_id(), delta, 100))
        for raw in (
            canonicalize(commit.model_dump(mode="json")),
            canonicalize(delta.model_dump(mode="json")),
            ce,
            de,
        ):
            store.put(raw)
        sources.append(RepairSource(work, ce, de, sha256_hex(ce + de), ids, anchor))
    tape = make_tape(
        store,
        manifest,
        tiny.policy(),
        canonicalize(tiny.economics().body()),
        tiny.KEY,
        w=1,
        prev_state=previous,
        predecessor_tape_hash=entry["tape_hash"],
        inputs=[s.work for s in sources],
        reference_reward_units=100,
    )
    authority = RepairAuthority(
        tiny.KEY.ss58,
        store.put(tape.to_bytes()),
        tiny.H,
        sha256_hex(canonicalize(manifest.training.reference_spec.model_dump(mode="json"))),
        AnchorCache.layout_hash(manifest),
        "cpu",
    )
    excluded = [
        Exclusion(hotkey=sources[4].work.roster.hotkey, reason="FRAUD", evidence_hash=tiny.H)
    ]
    repaired = emulate(
        1,
        lambda comm: make_repair_tape(
            store,
            manifest,
            tiny.policy(),
            canonicalize(tiny.economics().body()),
            tiny.KEY,
            authority,
            comm,
            tiny.sample,
            w=1,
            prev_state=previous,
            predecessor_tape_hash=entry["tape_hash"],
            sources=sources[:4],
            excluded=excluded,
            reference_reward_units=100,
        ),
    )[0]
    fresh = emulate(
        1,
        lambda comm: _replay_history(
            store, manifest, [entry], tiny.KEY.ss58, comm, tiny.sample, "cpu"
        ),
    )[0]
    by_key = {a.hotkey: a for a in fresh}
    from dataclasses import replace

    honest = [replace(s, anchor=by_key[s.work.roster.hotkey]) for s in sources[:4]]
    checked = emulate(
        1,
        lambda comm: replay_repair_tape(
            store,
            repaired.tape,
            manifest,
            tiny.policy(),
            canonicalize(tiny.economics().body()),
            authority,
            comm,
            tiny.sample,
            w=1,
            prev_state=previous,
            predecessor_tape_hash=entry["tape_hash"],
            sources=honest,
            excluded=excluded,
            reference_reward_units=100,
        ),
    )[0]
    assert checked.state.to_bytes() == repaired.state.to_bytes()
    assert all(a.state.step == 4 for a in checked.anchors)
    # Exact checkpoint dispatch consumes independently authenticated ordinary history,
    # never bypasses provenance because candidate execution can reproduce forged carry.
    from dataclasses import asdict

    from hypertrain.aggregator.checkpoint import CheckpointError, _network_replay

    items = []
    for source in honest:
        a = source.anchor
        store.put(source.commit_envelope)
        store.put(source.delta_envelope)
        items.append(
            {
                "work": {
                    "roster": source.work.roster.body(),
                    "commit": source.work.commit.model_dump(mode="json"),
                    "delta_manifest": source.work.delta_manifest.model_dump(mode="json"),
                    "replay": asdict(source.work.replay),
                    "funding": asdict(source.work.funding),
                    "settlement": asdict(source.work.settlement),
                    "assignment_hash": source.work.assignment_hash,
                    "clean_finalizations": 12,
                },
                "anchor": {
                    "hotkey": a.hotkey,
                    "anchor_hash": a.anchor_hash,
                    "proof_hash": a.proof_hash,
                    "state_object": store.put(pack_state(a.theta, a.state)),
                    "ef_object": store.put(pack_state(a.ef)),
                },
            }
        )
    body = {"history": [entry], "source_tape_hash": authority.source_tape_hash, "sources": items}
    context = {"body": body}
    candidate = SimpleNamespace(body=repaired.tape.body.arithmetic, repair=repaired.tape)
    item = {
        "repair_context": context,
        "rollback": {"body": {"w": 1}},
        "prev_state": previous,
        "predecessor_tape_hash": entry["tape_hash"],
        "round_open": {"body": {"theta_hash": state_hash(theta)}},
    }
    patch = pytest.MonkeyPatch()
    try:
        patch.setattr(
            "hypertrain.aggregator.rollback_v2.replay_history_context",
            lambda *args: emulate(
                1,
                lambda comm: _replay_history(
                    store, manifest, [entry], tiny.KEY.ss58, comm, tiny.sample, "cpu"
                ),
            )[0],
        )
        called = []
        patch.setattr(
            "hypertrain.aggregator.rollback_v2.execute_repair_context",
            lambda *args, **kwargs: (
                called.append(True)
                or emulate(
                    1,
                    lambda comm: replay_repair_tape(
                        store,
                        repaired.tape,
                        manifest,
                        tiny.policy(),
                        canonicalize(tiny.economics().body()),
                        authority,
                        comm,
                        tiny.sample,
                        w=1,
                        prev_state=previous,
                        predecessor_tape_hash=entry["tape_hash"],
                        sources=honest,
                        excluded=excluded,
                        reference_reward_units=100,
                    ),
                )[0]
            ),
        )
        output, result_anchors = _network_replay(
            store,
            item,
            candidate,
            manifest,
            tiny.policy(),
            canonicalize(tiny.economics().body()),
            tiny.KEY.ss58,
            None,
        )
        assert output.to_bytes() == checked.state.to_bytes() and len(result_anchors) == 4
        items[0]["anchor"]["anchor_hash"] = "ff" * 32
        calls = len(called)
        with pytest.raises(CheckpointError, match="carried anchor"):
            _network_replay(
                store,
                item,
                candidate,
                manifest,
                tiny.policy(),
                canonicalize(tiny.economics().body()),
                tiny.KEY.ss58,
                None,
            )
        assert len(called) == calls
    finally:
        patch.undo()


def test_checkpoint_exact_probe_refuses_first_non_genesis_without_history(monkeypatch):
    from hypertrain.aggregator.checkpoint import CheckpointError, _network_replay

    # Given: candidate carries first non-genesis repair; no independent predecessor evidence.
    called = []
    monkeypatch.setattr(
        "hypertrain.aggregator.rollback_v2.execute_repair_context",
        lambda *a, **k: called.append(True),
    )
    tape = SimpleNamespace(body=SimpleNamespace(w=1))
    with pytest.raises(CheckpointError, match="authenticated predecessor history"):
        _network_replay(
            None, {"repair_context": {"body": {}}}, tape, None, None, b"", tiny.KEY.ss58, None
        )
    assert called == []


@pytest.mark.parametrize("mutation", ["cpu", "backend", "hash", "reference", "layout"])
def test_fifth_backend_consumer_binds_exact_reviewed_record_without_gpu_claim(mutation):
    manifest = tiny.tiny_manifest()
    reference = sha256_hex(canonicalize(manifest.training.reference_spec.model_dump(mode="json")))
    reviewed = {
        "run_id": manifest.run_id(),
        "backend": "cuda",
        "reference_hash": reference,
        "layout_hash": manifest.training.reference_spec.layout.model_dump_json(),
    }
    body = {
        "backend": "cuda",
        "reference_hash": reference,
        "qualification": {
            "execution_backend": {
                "backend": "cuda",
                "authority_hash": sha256_hex(canonicalize(reviewed)),
            },
            "reviewed_qualification": reviewed,
        },
    }
    _qualification_body(manifest, body)
    if mutation == "cpu":
        body["backend"] = "cpu"
        body["qualification"]["execution_backend"]["backend"] = "cpu"
    elif mutation == "backend":
        body["qualification"]["execution_backend"]["backend"] = "cpu"
    elif mutation == "hash":
        body["qualification"]["execution_backend"]["authority_hash"] = "ff" * 32
    else:
        reviewed[mutation + "_hash"] = "ff" * 32
    with pytest.raises(TapeError):
        _qualification_body(manifest, body)
    _qualification_body(
        manifest,
        {
            "backend": "cpu",
            "qualification": {"execution_backend": {"backend": "cpu", "authority_hash": None}},
        },
    )
