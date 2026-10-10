from __future__ import annotations

import importlib.util
import threading
import time
from pathlib import Path

import numpy as np
import pytest
from pydantic import ValidationError

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.honeypot import fresh_honeypots
from hypertrain.auditor.replay import (
    AnchorCache,
    AuditInputError,
    audit_island,
    optimizer_hash,
    pack_state,
    replay_island_windows,
)
from hypertrain.auditor.worker import LeaseGuard
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    AuditJobV2,
    CommitV2,
    RunManifestV2,
    StartStateV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import emulate, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params


def fixture(objective: str | None = None, state_policy: str = "carry"):
    path = Path(__file__).parents[1] / (
        "layout/test_od_island_v2.py" if objective else "miner/test_island_launch_v2.py"
    )
    spec = importlib.util.spec_from_file_location("replay_l1_fixture", path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    w = mod.od_manifest(objective, 2, True) if objective else mod.tiny_manifest(2)
    b = w.body()
    b["training"]["inner"]["state_policy"] = state_policy
    w = RunManifestV2.model_validate(b)
    cfg = TrainConfig.from_manifest_v2(w)
    get = mod.samples(cfg) if objective else lambda i: (np.arange(5, dtype=np.uint32) + i) % 16
    theta = init_params(cfg.model)
    key = Keypair(bytes([44]) * 32)
    cache = AnchorCache()
    anchor = cache.genesis(w, key.ss58, theta)
    a = Assignment(w.run_id(), 0, tuple(range(w.training.batch_samples())))
    result = emulate(
        2,
        lambda c: train_island(
            cfg,
            w.training.reference_spec.layout,
            c,
            theta,
            a,
            get,
            carry=anchor.state if state_policy == "carry" else None,
            ef_in=anchor.ef,
            v0=anchor.state.v if state_policy == "derived" else None,
        ),
    )[0]
    blobs = (
        pack_state(theta, anchor.state),
        pack_state(anchor.ef),
        pack_state(anchor.state.v if state_policy == "derived" else {}),
    )
    start = StartStateV2(
        run_id=w.run_id(),
        w=0,
        hotkey=key.ss58,
        theta_hash=state_hash(theta),
        state_object_sha256=sha256_hex(blobs[0]),
        opt_state_hash=optimizer_hash(anchor.state),
        ef_object_sha256=sha256_hex(blobs[1]),
        ef_hash=state_hash(anchor.ef),
        parent_anchor_hash=anchor.anchor_hash,
        global_step0=0,
        anchor_verdict_hash=anchor.proof_hash,
    )
    commit = CommitV2(
        w=0,
        hotkey=key.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=cfg.inner.n_leaves,
        leaves_root=result.leaves_root,
        metrics_root="11" * 32,
        final_theta_hash=result.final_theta_hash,
        ef_in_hash=result.ef_in_hash,
        ef_out_hash=result.ef_out_hash,
        delta_hash=result.delta_hash,
        delta_bytes=len(result.delta_payload),
        tokens=len(a.sample_ids) * cfg.model.seq_len,
    )
    challenge = AuditChallengeV2(
        w=0,
        target=key.ss58,
        beacon_round=1,
        beacon_sig_sha256="22" * 32,
        mode="full",
        segments=[],
        reasons=["final"],
        serve_deadline=100,
        anchor_hash=start.digest(),
        audit_mode="anchored-full",
    )
    auditor = Keypair(bytes([45]) * 32)
    job = AuditJobV2(
        run_id=w.run_id(),
        job_id="33" * 32,
        auditor_id=auditor.ss58,
        attempt=1,
        lease_nonce="44" * 32,
        lease_expires=50,
        absolute_deadline=100,
        reservation_id="55" * 32,
        replay_step_budget=2 * cfg.inner.H,
        anchor_age=0,
        manifest=w,
        challenge_envelope=seal(auditor, "AuditChallengeV2", w.run_id(), challenge, 100),
        commit_envelope=seal(key, "CommitV2", w.run_id(), commit, 100),
        sample_ids=list(a.sample_ids),
        start_state=start,
        preimages=[x.preimage for x in result.leaves],
        ef_in=dict(sha256=sha256_hex(blobs[1]), size=len(blobs[1])),
        v0=dict(sha256=sha256_hex(blobs[2]), size=len(blobs[2])),
        created_beacon=1,
    )
    return job, cache, blobs, get, result, key


@pytest.mark.parametrize("objective", [None, "mlm", "decision", "distill"])
def test_full_carry_and_window_equality(objective: str | None) -> None:
    job, cache, blobs, get, expected, _ = fixture(objective)
    outputs = emulate(2, lambda c: audit_island(job, c, get, *blobs, cache, now_round=2))
    assert all(out.result == "MATCH" for out, _ in outputs)
    assert outputs[0][1].delta_payload == expected.delta_payload
    cfg = TrainConfig.from_manifest_v2(job.manifest)
    prior = cache.prior(job.manifest, job.start_state)
    a = Assignment(job.run_id, 0, tuple(job.sample_ids))
    traced_steps = [set(), set()]

    def hook(ctx, x):
        if ctx.op == "attn":
            traced_steps[ctx.rank].add(ctx.step)
        return x

    windows = emulate(
        2,
        lambda c: replay_island_windows(
            cfg,
            job.manifest,
            c,
            init_params(cfg.model),
            prior.state,
            a,
            0,
            cfg.inner.H // cfg.inner.J,
            get,
            hook=hook,
        ),
    )
    assert [p.digest() for p in windows[0][0]] == [p.digest() for p in job.preimages[1:]]
    assert traced_steps == [set(range(1, cfg.inner.H + 1))] * 2


def test_fake_prefix_final_and_ef_corruption() -> None:
    job, cache, blobs, get, expected, key = fixture()
    cfg = TrainConfig.from_manifest_v2(job.manifest)
    prior = cache.prior(job.manifest, job.start_state)
    bad_state = prior.state.clone()
    bad_state.m[sorted(bad_state.m)[0]].add_(0.001)
    bad = pack_state(init_params(cfg.model), bad_state)
    start = job.start_state.model_copy(update={"state_object_sha256": sha256_hex(bad)})
    corrupt = job.model_copy(update={"start_state": start})
    with pytest.raises(AuditInputError, match="fabricated carried prefix"):
        emulate(2, lambda c: audit_island(corrupt, c, get, bad, *blobs[1:], cache, now_round=2))
    with pytest.raises(AuditInputError, match="EF object"):
        emulate(
            2, lambda c: audit_island(job, c, get, blobs[0], b"bad", blobs[2], cache, now_round=2)
        )
    body = dict(job.commit_envelope["body"])
    body["delta_hash"] = "66" * 32
    bad_commit = seal(key, "CommitV2", job.run_id, body, 100)
    mutated = job.model_copy(update={"commit_envelope": bad_commit})
    assert (
        emulate(2, lambda c: audit_island(mutated, c, get, *blobs, cache, now_round=2))[0][0].result
        == "MISMATCH"
    )


def test_old_anchor_work_budget_and_expired_lease_refuse() -> None:
    job, cache, blobs, get, _, _ = fixture()
    for changes in ({"anchor_age": 2}, {"replay_step_budget": 5}):
        with pytest.raises(ValidationError):
            AuditJobV2.model_validate({**job.model_dump(mode="json"), **changes})
    cache.entries.clear()
    with pytest.raises(AuditInputError, match="ANCHOR_BUDGET_EXCEEDED"):
        emulate(2, lambda c: audit_island(job, c, get, *blobs, cache, now_round=2))
    with LeaseGuard(50, 100, time.time() + 120) as guard:
        guard.advance(50)
        with pytest.raises(AuditInputError, match="lease expired"):
            guard.check()
    with pytest.raises(ValueError, match="live authenticated"):
        job.validate_embedded(50)


def test_honeypot_keys_not_reused_after_reveal() -> None:
    first = fresh_honeypots([], ["honest", "last_step"])
    second = fresh_honeypots(first, ["honest", "last_step"])
    assert not {p.hotkey for p in first} & {p.hotkey for p in second}


def test_live_beacon_expiry_cancels_before_collectives() -> None:
    job, cache, blobs, get, _, _ = fixture()
    with pytest.raises(ValueError, match="live authenticated"):
        emulate(
            2,
            lambda c: audit_island(job, c, get, *blobs, cache, now_round=2, beacon_now=lambda: 50),
        )


def test_chain_rejects_expiry_before_reserved_predecessor_execution() -> None:
    from hypertrain.auditor.replay import audit_island_chain

    # Given: a verified beacon already expired the current reservation.
    job, cache, blobs, get, _, _ = fixture()
    # When/Then: do not spend predecessor work under an expired current lease.
    with pytest.raises(ValueError, match="live authenticated"):
        emulate(
            2,
            lambda c: audit_island_chain(
                job,
                job,
                c,
                get,
                blobs,
                blobs,
                cache,
                now_round=2,
                beacon_now=lambda: 50,
            ),
        )


def test_expiry_during_terminal_compression_cannot_publish_anchor(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from hypertrain.trainer import island

    # Given: live replay; compression is the last noncollective work boundary.
    job, cache, blobs, get, _, _ = fixture()
    expired = threading.Event()
    original = island.compress

    def compress(*args, **kwargs):
        result = original(*args, **kwargs)
        expired.set()
        return result

    monkeypatch.setattr(island, "compress", compress)
    # When/Then: terminal lease expiry cannot insert a MATCH anchor.
    with pytest.raises(ValueError, match="live authenticated"):
        emulate(
            2,
            lambda c: audit_island(
                job,
                c,
                get,
                *blobs,
                cache,
                now_round=2,
                beacon_now=lambda: 50 if expired.is_set() else 2,
            ),
        )
    assert all(key[2] == -1 for key in cache.entries)


def test_missing_predecessor_reconstructs_once_under_two_h_budget() -> None:
    from hypertrain.auditor.replay import audit_island_chain

    job, cache, blobs, get, result, key = fixture()
    emulate(2, lambda c: audit_island(job, c, get, *blobs, cache, now_round=2))
    parent = cache.entries[(job.run_id, key.ss58, 0, cache.layout_hash(job.manifest))]
    cfg = TrainConfig.from_manifest_v2(job.manifest)
    theta = {k: x + 0.0001 for k, x in result.final_theta.items()}
    next_blobs = (pack_state(theta, parent.state), pack_state(parent.ef), pack_state({}))
    start = StartStateV2(
        run_id=job.run_id,
        w=1,
        hotkey=key.ss58,
        theta_hash=state_hash(theta),
        state_object_sha256=sha256_hex(next_blobs[0]),
        opt_state_hash=optimizer_hash(parent.state),
        ef_object_sha256=sha256_hex(next_blobs[1]),
        ef_hash=state_hash(parent.ef),
        parent_anchor_hash=parent.anchor_hash,
        global_step0=2,
        anchor_verdict_hash=parent.proof_hash,
    )
    a = Assignment(job.run_id, 1, tuple(job.sample_ids), 2)
    trained = emulate(
        2,
        lambda c: train_island(
            cfg,
            job.manifest.training.reference_spec.layout,
            c,
            theta,
            a,
            get,
            carry=parent.state,
            ef_in=parent.ef,
        ),
    )[0]
    body = dict(job.commit_envelope["body"])
    body.update(
        w=1,
        leaves_root=trained.leaves_root,
        final_theta_hash=trained.final_theta_hash,
        delta_hash=trained.delta_hash,
        delta_bytes=len(trained.delta_payload),
        ef_in_hash=trained.ef_in_hash,
        ef_out_hash=trained.ef_out_hash,
    )
    ch = dict(job.challenge_envelope["body"])
    ch.update(w=1, anchor_hash=start.digest())
    current = AuditJobV2.model_validate(
        {
            **job.model_dump(mode="json"),
            "job_id": "77" * 32,
            "start_state": start.body(),
            "anchor_age": 1,
            "ef_in": {"sha256": sha256_hex(next_blobs[1]), "size": len(next_blobs[1])},
            "preimages": [
                x.preimage.body()
                if hasattr(x.preimage, "body")
                else x.preimage.model_dump(mode="json")
                for x in trained.leaves
            ],
            "commit_envelope": seal(key, "CommitV2", job.run_id, body, 100),
            "challenge_envelope": seal(
                Keypair(bytes([45]) * 32), "AuditChallengeV2", job.run_id, ch, 100
            ),
        }
    )
    del cache.entries[(job.run_id, key.ss58, 0, cache.layout_hash(job.manifest))]
    out = emulate(
        2, lambda c: audit_island_chain(current, job, c, get, next_blobs, blobs, cache, now_round=2)
    )
    assert out[0][0].result == "MATCH"
    short = current.model_copy(update={"replay_step_budget": cfg.inner.H})
    with pytest.raises(AuditInputError, match="reservation mismatch"):
        emulate(
            2,
            lambda c: audit_island_chain(short, job, c, get, next_blobs, blobs, cache, now_round=2),
        )


def test_verified_anchor_restart_corruption_and_od_warm_start(tmp_path: Path) -> None:
    from hypertrain.auditor.replay import tensor_root
    from hypertrain.models.opendecision import extend_params

    job, cache, blobs, get, _, key = fixture("mlm")
    emulate(2, lambda c: audit_island(job, c, get, *blobs, cache, now_round=2))
    source = cache.entries[(job.run_id, key.ss58, 0, cache.layout_hash(job.manifest))]
    path = cache.persist(tmp_path, source)
    fresh = AnchorCache()
    restored = fresh.restore(
        path,
        job.manifest,
        source.anchor_hash,
        source.proof_hash,
        tensor_root(source.theta, source.state),
        state_hash(source.ef),
        expected_hotkey=source.hotkey,
        expected_round=source.w,
        expected_backend=source.backend,
    )
    decision, _, _, _, _, _ = fixture("decision")
    body = decision.manifest.body()
    body["training"]["model"]["od"]["warm_start"] = True
    target = RunManifestV2.model_validate(body)
    theta = extend_params(restored.theta, TrainConfig.from_manifest_v2(target).model)
    warm = fresh.warm_start(target, key.ss58, theta, restored)
    assert warm.state.step == 0
    with pytest.raises(AuditInputError, match="checkpoint tensor"):
        fresh.warm_start(target, key.ss58, {n: x * 1.01 for n, x in theta.items()}, restored)
    (path / "ef").write_bytes(b"corruption")
    with pytest.raises(AuditInputError, match="authentication mismatch"):
        fresh.restore(
            path,
            job.manifest,
            source.anchor_hash,
            source.proof_hash,
            tensor_root(source.theta, source.state),
            state_hash(source.ef),
            expected_hotkey=source.hotkey,
            expected_round=source.w,
            expected_backend=source.backend,
        )


def test_real_cpu_match_restore_rejects_metadata_only_provenance_mutation(tmp_path: Path) -> None:
    import json

    from hypertrain.auditor.replay import tensor_root

    job, cache, blobs, get, _, keypair = fixture()
    results = emulate(2, lambda c: audit_island(job, c, get, *blobs, cache, now_round=2))
    assert all(out.result == "MATCH" for out, _ in results)
    key = (job.run_id, keypair.ss58, 0, cache.layout_hash(job.manifest))
    source = cache.entries[key]
    assert source.backend == "cpu"
    path = cache.persist(tmp_path, source)
    metadata = (path / "metadata").read_bytes()
    original = json.loads(metadata)
    state_root, ef_hash = tensor_root(source.theta, source.state), state_hash(source.ef)
    fresh = AnchorCache()

    def restore():
        return fresh.restore(
            path,
            job.manifest,
            source.anchor_hash,
            source.proof_hash,
            state_root,
            ef_hash,
            expected_hotkey=source.hotkey,
            expected_round=source.w,
            expected_backend="cpu",
        )

    for field, value in (("backend", "cuda"), ("hotkey", Keypair(bytes([46]) * 32).ss58), ("w", 1)):
        assert restore().backend == "cpu"
        assert key in fresh.entries
        (path / "metadata").write_text(json.dumps({**original, field: value}))
        with pytest.raises(AuditInputError, match="authentication mismatch"):
            restore()
        assert fresh.entries == {}  # failed reload invalidates prior insertion
        (path / "metadata").write_bytes(metadata)
    restored = restore()
    assert (restored.hotkey, restored.w, restored.backend) == (source.hotkey, 0, "cpu")
    assert tensor_root(restored.theta, restored.state) == state_root
    assert state_hash(restored.ef) == ef_hash


@pytest.mark.parametrize("state_policy", ["carry", "reset", "derived"])
def test_real_parent_auditor_runtime(tmp_path: Path, state_policy: str) -> None:
    import json

    from hypertrain.auditor.worker import execute_island_audit
    from hypertrain.protocol.hashing import MerkleTree

    job, cache, blobs, get, _, key = fixture(state_policy=state_policy)
    rows = [get(i).astype("<u4").tobytes() for i in job.sample_ids]
    tree = MerkleTree(rows)
    body = job.manifest.body()
    body["training"]["dataset"].update(merkle_root=tree.root.hex(), n_samples=len(rows), depth=2)
    wrapper = RunManifestV2.model_validate(body)
    # Changing dataset pin changes wrapper ID; train the honest fixture under the actual new ID.
    cfg = TrainConfig.from_manifest_v2(wrapper)
    theta = init_params(cfg.model)
    cache = AnchorCache()
    prior = cache.genesis(wrapper, key.ss58, theta)
    a = Assignment(wrapper.run_id(), 0, tuple(job.sample_ids))
    res = emulate(
        2,
        lambda c: train_island(
            cfg,
            wrapper.training.reference_spec.layout,
            c,
            theta,
            a,
            get,
            carry=prior.state if state_policy == "carry" else None,
            ef_in=prior.ef,
            v0=prior.state.v if state_policy == "derived" else None,
        ),
    )[0]
    start = job.start_state.model_copy(
        update={
            "run_id": wrapper.run_id(),
            "parent_anchor_hash": prior.anchor_hash,
            "anchor_verdict_hash": prior.proof_hash,
        }
    )
    ch = dict(job.challenge_envelope["body"])
    ch["anchor_hash"] = start.digest()
    commit = dict(job.commit_envelope["body"])
    commit.update(
        leaves_root=res.leaves_root,
        delta_hash=res.delta_hash,
        final_theta_hash=res.final_theta_hash,
        ef_out_hash=res.ef_out_hash,
    )
    job = AuditJobV2.model_validate(
        {
            **job.model_dump(mode="json"),
            "run_id": wrapper.run_id(),
            "manifest": wrapper.body(),
            "start_state": start.body(),
            "preimages": [x.preimage.model_dump(mode="json") for x in res.leaves],
            "commit_envelope": seal(key, "CommitV2", wrapper.run_id(), commit, 100),
            "challenge_envelope": seal(
                Keypair(bytes([45]) * 32), "AuditChallengeV2", wrapper.run_id(), ch, 100
            ),
        }
    )
    for name, blob in zip(("start_state", "ef_in", "v0"), blobs, strict=True):
        (tmp_path / name).write_bytes(blob)
    (tmp_path / "samples").write_bytes(b"".join(rows))
    (tmp_path / "sample_proofs").write_text(
        json.dumps([[p.hex() for p in tree.proof(i)] for i in range(len(rows))])
    )
    with LeaseGuard(50, 100, time.time() + 120) as guard:
        out, artifacts = execute_island_audit(
            job,
            tmp_path,
            cache,
            guard,
            now_round=2,
            deadline_unix=int(time.time()) + 120,
            backend="cpu",
        )
    assert out.result == "MATCH"
    assert artifacts.delta.read_bytes() == res.delta_payload
