from __future__ import annotations

import math
from dataclasses import replace

import pytest
import torch
from aud_fixtures import (
    AUDITOR,
    ENV,
    MINER,
    SERVE_DEADLINE,
    TOKEN,
    FakeChallenge,
    carry_round,
    get_sample,
    job_json,
    make_challenge,
    make_commit,
    serve_for,
    world,
)
from fastapi.testclient import TestClient

from hypertrain.auditor.bisect import Executor, Fault, ceil_log, level_span, run_dispute
from hypertrain.auditor.honeypot import (
    Honeypot,
    commit,
    epoch_rates,
    honeypot_round,
    verify_reveal,
)
from hypertrain.auditor.replay import (
    AuditInputs,
    NotReady,
    ServedState,
    audit_full,
    audit_segments,
    classify_mismatch,
    committed_norms,
    forfeit_for,
    make_verdict,
    select_segments,
)
from hypertrain.auditor.worker import Auditor, HttpApi
from hypertrain.protocol.envelope import verify_envelope
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.trainer.loop import LeafRecord, RoundResult, train_round
from hypertrain.trainer.optim import init_state


def inputs(w, res: RoundResult, ch=None, commit_=None) -> AuditInputs:  # type: ignore[no-untyped-def]
    return AuditInputs(
        cfg=w.cfg,
        run_id=w.run_id,
        challenge=ch or make_challenge(),
        commit=commit_ or make_commit(w, res),
        assignment=w.a,
        leaves=[x.digest.hex() for x in res.leaves],
        preimages=[x.preimage for x in res.leaves],
        theta_start=w.theta,
    )


def with_leaf(res: RoundResult, i: int, **update: str) -> RoundResult:
    leaves = list(res.leaves)
    pre = leaves[i].preimage.model_copy(update=update)
    leaves[i] = LeafRecord(pre, bytes.fromhex(pre.digest()))
    root = MerkleTree([x.digest for x in leaves]).root.hex()
    return replace(res, leaves=leaves, leaves_root=root)


@pytest.fixture(scope="module")
def full() -> tuple:  # type: ignore[type-arg]
    w = world("reset")
    return w, train_round(w.cfg, w.theta, w.a, get_sample(w))


def test_honest_full_replay_match(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    out = audit_full(inputs(w, res), get_sample(w))
    assert out.result == "MATCH" and out.first_bad_leaf is None
    assert out.recomputed_leaves_root == res.leaves_root


def test_fabricated_leaf_mismatch_at_first_bad_leaf(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    fake = with_leaf(res, 2, loss_f32="3f800000")
    out = audit_full(inputs(w, fake), get_sample(w))
    assert (out.result, out.first_bad_leaf) == ("MISMATCH", 2)
    assert out.recomputed_leaves_root == res.leaves_root


def test_fake_weights_mismatch_at_leaf_1(full) -> None:  # type: ignore[no-untyped-def]
    w, _ = full
    res = honeypot_round(Honeypot(MINER.ss58, "fabricate"), w.cfg, w.theta, w.a, get_sample(w))
    out = audit_full(inputs(w, res), get_sample(w))
    assert (out.result, out.first_bad_leaf) == ("MISMATCH", 1)


def test_metrics_lie_is_mismatch_not_trusted(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    lie = with_leaf(res, 3, norm_f32="00000000")
    assert audit_full(inputs(w, lie), get_sample(w)).first_bad_leaf == 3


def test_bad_proof_root_and_preimage(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    c = make_commit(w, res).model_copy(update={"leaves_root": "e" * 64})
    assert audit_full(inputs(w, res, commit_=c), get_sample(w)).result == "BAD_PROOF"
    x = inputs(w, res)
    pres = list(x.preimages)
    pres[1] = pres[1].model_copy(update={"loss_f32": "00000001"})
    out = audit_full(replace(x, preimages=pres), get_sample(w))
    assert (out.result, out.first_bad_leaf) == ("BAD_PROOF", 1)


def test_assignment_violation_without_replay(full) -> None:  # type: ignore[no-untyped-def]
    w, _ = full
    per = w.cfg.inner.micro_batch * w.cfg.inner.grad_accum
    ids = list(w.a.sample_ids)
    ids[3 * per] = 999_999  # step 4 -> leaf 2
    shifted = train_round(w.cfg, w.theta, replace(w.a, sample_ids=tuple(ids)), get_sample(w))
    out = audit_full(inputs(w, shifted), get_sample(w))
    assert (out.result, out.first_bad_leaf) == ("ASSIGNMENT_VIOLATION", 2)
    assert out.recomputed_leaves_root == "0" * 64


def test_delta_hash_mismatch(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    c = make_commit(w, res).model_copy(update={"delta_hash": "d" * 64})
    out = audit_full(inputs(w, res, commit_=c), get_sample(w))
    assert out.result == "MISMATCH" and out.first_bad_leaf is None


@pytest.mark.parametrize(
    ("n", "fault"),
    [
        (2, Fault(5, 1, "attn")),
        (4, Fault(2, 0, "embed")),
        (3, Fault(6, 2, "update")),
        (2, Fault(1, 2, "head")),
    ],
)
def test_bisection_converges_to_injected_op(n: int, fault: Fault) -> None:
    w = world("reset")
    g = get_sample(w)
    honest = Executor(AUDITOR.ss58, w.cfg, w.theta, w.a, g)
    cheat = Executor(MINER.ss58, w.cfg, w.theta, w.a, g, fault=fault)
    ref = Executor("referee", w.cfg, w.theta, w.a, g)
    r = run_dispute("ab" * 32, w.cfg, honest, cheat, ref, n, (0, w.cfg.inner.H))
    assert (r.step, r.layer, r.op) == (fault.step, fault.layer, fault.op)
    assert r.resolution.loser == MINER.ss58
    layer_span = level_span(w.cfg, "layer", (fault.step,))
    op_span = level_span(w.cfg, "op", (fault.step, fault.layer))
    assert 1 <= r.rounds["step"] <= ceil_log(w.cfg.inner.H, n)
    assert 1 <= r.rounds["layer"] <= ceil_log(layer_span, n)
    assert 1 <= r.rounds["op"] <= ceil_log(op_span, n)
    assert all(len(b.hashes) == b.N + 1 for b in r.transcript)


def test_bisection_names_lying_auditor() -> None:
    w = world("reset")
    g = get_sample(w)
    bad_auditor = Executor(AUDITOR.ss58, w.cfg, w.theta, w.a, g, fault=Fault(3, 1, "mlp"))
    miner = Executor(MINER.ss58, w.cfg, w.theta, w.a, g)
    ref = Executor("referee", w.cfg, w.theta, w.a, g)
    r = run_dispute("cd" * 32, w.cfg, bad_auditor, miner, ref, 2, (0, w.cfg.inner.H))
    assert r.resolution.loser == AUDITOR.ss58 and (r.step, r.layer, r.op) == (3, 1, "mlp")


def test_transient_when_miner_rerun_reproduces_auditor(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    glitched = with_leaf(res, 3, loss_f32="40000000")
    ch = make_challenge()
    v = make_verdict(ch, audit_full(inputs(w, glitched), get_sample(w)), ENV)
    rerun = train_round(w.cfg, w.theta, w.a, get_sample(w)).leaves_root
    assert classify_mismatch(v, glitched.leaves_root, rerun, 0, 1) == "TRANSIENT"
    assert classify_mismatch(v, glitched.leaves_root, rerun, 1, 1) == "FAULT"
    assert classify_mismatch(v, glitched.leaves_root, glitched.leaves_root, 0, 1) == "CONTEST"
    assert classify_mismatch(v, glitched.leaves_root, "1" * 64, 0, 1) == "FAULT"
    f = forfeit_for(3, MINER.ss58, "TRANSIENT", [v.challenge_hash], 100, 900)
    assert (f.round_reward_burned, f.escrow_burned_units, f.blacklist) == (100, 0, False)
    g = forfeit_for(3, MINER.ss58, "MISMATCH", [v.challenge_hash], 100, 900)
    assert (g.escrow_burned_units, g.blacklist) == (900, True)


def test_honeypot_commit_reveal_and_rates(full) -> None:  # type: ignore[no-untyped-def]
    w, honest = full
    keys = [f"5{c}" + "x" * 46 for c in "ABCD"]
    pots = [
        Honeypot(keys[0], "honest"),
        Honeypot(keys[1], "fabricate"),
        Honeypot(keys[2], "last_step"),
        Honeypot(keys[3], "noise"),
    ]
    salt = b"s" * 32
    c = commit(pots, salt)
    assert verify_reveal(c, list(reversed(pots)), salt)
    assert not verify_reveal(c, [replace(pots[0], mode="noise"), *pots[1:]], salt)
    assert not verify_reveal(c, pots, b"t" * 32)
    assert not verify_reveal(c, pots[1:], salt)
    results: dict[str, list[str]] = {}
    for p in pots:
        res = honeypot_round(p, w.cfg, w.theta, w.a, get_sample(w))
        results[p.hotkey] = [audit_full(inputs(w, res), get_sample(w)).result]
    rates = epoch_rates(7, c, pots, salt, results).publish()
    assert rates["caught"] == 3 and rates["catch_rate"] == 1.0
    assert rates["false_positives"] == 0 and rates["false_positive_rate"] == 0.0
    with pytest.raises(ValueError):
        epoch_rates(7, c, pots[:2], salt, results)


def binom_interval_99(n: int, p: float) -> tuple[int, int]:
    cdf, lo, hi = 0.0, None, n
    for k in range(n + 1):
        cdf += math.comb(n, k) * p**k * (1 - p) ** (n - k)
        if lo is None and cdf > 0.005:
            lo = k
        if cdf >= 0.995:
            hi = k
            break
    return lo or 0, hi


@pytest.fixture(scope="module")
def carry30() -> tuple:  # type: ignore[type-arg]
    w = world("carry", H=30, J=1)

    def last(t: int, theta: dict[str, torch.Tensor]) -> None:
        theta["head.weight"].view(-1)[0] += 1e-3

    return w, carry_round(w), carry_round(w, after_last_step=last)


def _segment_trial(w, cr, beacon: str, final_always: bool, q_top: int) -> str:  # type: ignore[no-untyped-def]
    res = cr.result
    norms = committed_norms([x.preimage for x in res.leaves])
    segs = select_segments(
        w.run_id,
        3,
        make_challenge(beacon=beacon).beacon_sig_sha256,
        MINER.ss58,
        norms,
        3,
        q_top,
        final_always,
    )
    ch = make_challenge("segments", segs, beacon=beacon)
    serves = {a: ServedState(*serve_for(w, cr, ch, a)) for a, _ in segs}
    return audit_segments(inputs(w, res, ch), get_sample(w), serves, 100, segs).result


def test_carry_windows_equal_trainer_carry_round(carry30) -> None:  # type: ignore[no-untyped-def]
    w, honest, _ = carry30
    st0 = init_state(replace(w.cfg.inner, state_policy="reset"), w.theta)
    ref = train_round(w.cfg, w.theta, w.a, get_sample(w), carry=st0)
    assert ref.leaves_root == honest.result.leaves_root
    assert ref.delta_hash == honest.result.delta_hash


def test_segments_honest_match(carry30) -> None:  # type: ignore[no-untyped-def]
    w, honest, _ = carry30
    assert _segment_trial(w, honest, "honest", True, 1) == "MATCH"


def test_segments_final_always_catches_last_step_30_of_30(carry30) -> None:  # type: ignore[no-untyped-def]
    w, _, cheat = carry30
    caught = sum(_segment_trial(w, cheat, f"b{i}", True, 0) == "MISMATCH" for i in range(30))
    assert caught == 30


def test_segments_ablation_without_final_inside_binomial(carry30) -> None:  # type: ignore[no-untyped-def]
    w, _, cheat = carry30
    trials, k, u = 200, 3, 30
    caught = sum(_segment_trial(w, cheat, f"a{i}", False, 0) == "MISMATCH" for i in range(trials))
    lo, hi = binom_interval_99(trials, k / u)
    print(f"ablation caught {caught}/{trials}, 99% interval [{lo}, {hi}]")
    assert lo <= caught <= hi and caught < trials


def test_segments_withheld_and_not_ready(carry30) -> None:  # type: ignore[no-untyped-def]
    w, honest, _ = carry30
    segs = [(5, 6), (29, 30)]
    ch = make_challenge("segments", segs)
    x = inputs(w, honest.result, ch)
    with pytest.raises(NotReady):
        audit_segments(x, get_sample(w), {}, SERVE_DEADLINE)
    out = audit_segments(x, get_sample(w), {}, SERVE_DEADLINE + 1)
    assert (out.result, out.first_bad_leaf) == ("WITHHELD", 5)


def test_segments_served_state_mismatch_is_bad_proof(carry30) -> None:  # type: ignore[no-untyped-def]
    w, honest, _ = carry30
    segs = [(5, 6)]
    ch = make_challenge("segments", segs)
    serve, _ = serve_for(w, honest, ch, 5)
    _, other_blob = serve_for(w, honest, ch, 6)
    out = audit_segments(
        inputs(w, honest.result, ch), get_sample(w), {5: ServedState(serve, other_blob)}, 1
    )
    assert out.result == "BAD_PROOF"
    out = audit_segments(
        inputs(w, honest.result, ch), get_sample(w), {5: ServedState(serve, b"junk")}, 1
    )
    assert out.result == "BAD_PROOF"


def _auditor(fc: FakeChallenge, w) -> tuple[Auditor, TestClient]:  # type: ignore[no-untyped-def]
    client = TestClient(fc.app())
    return Auditor(HttpApi(client, TOKEN), AUDITOR, ENV, get_sample(w)), client


def test_worker_happy_match_closes_job(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    fc = FakeChallenge(w.m)
    fc.add_job("j1", job_json(w, fc, res, make_challenge()))
    aud, client = _auditor(fc, w)
    assert aud.run_once() == "MATCH"
    assert fc.state["j1"] == "closed" and not fc.forfeits
    env = fc.verdicts["j1"]
    assert verify_envelope(env) and env["signer"] == AUDITOR.ss58
    assert env["body"]["recomputed_leaves_root"] == res.leaves_root
    assert aud.run_once() is None
    with pytest.raises(RuntimeError, match="409"):
        HttpApi(client, TOKEN).heartbeat("j1", "lease-j1")
    with pytest.raises(RuntimeError, match="401"):
        HttpApi(client, "wrong").lease()


def test_worker_withheld_records_forfeit(carry30) -> None:  # type: ignore[no-untyped-def]
    w, honest, _ = carry30
    fc = FakeChallenge(w.m, now_round=SERVE_DEADLINE)
    norms = committed_norms([x.preimage for x in honest.result.leaves])
    segs = select_segments(
        w.run_id,
        3,
        make_challenge().beacon_sig_sha256,
        MINER.ss58,
        norms,
        w.m.verify.k_segments,
        w.m.verify.Q_top,
    )
    fc.add_job("j2", job_json(w, fc, honest.result, make_challenge("segments", segs)))
    aud, _ = _auditor(fc, w)
    assert aud.run_once() == "FAILED"
    assert fc.failures[-1]["retry"] is True and fc.state["j2"] == "queued"
    fc.now_round = SERVE_DEADLINE + 1
    assert aud.run_once() == "WITHHELD"
    assert fc.forfeits[0]["cause"] == "WITHHELD" and fc.forfeits[0]["blacklist"] is True


def test_worker_segments_served_match(carry30) -> None:  # type: ignore[no-untyped-def]
    w, honest, _ = carry30
    fc = FakeChallenge(w.m)
    norms = committed_norms([x.preimage for x in honest.result.leaves])
    segs = select_segments(
        w.run_id,
        3,
        make_challenge().beacon_sig_sha256,
        MINER.ss58,
        norms,
        w.m.verify.k_segments,
        w.m.verify.Q_top,
    )
    ch = make_challenge("segments", segs)
    fc.add_job("j3", job_json(w, fc, honest.result, ch))
    for a, _ in segs:
        serve, blob = serve_for(w, honest, ch, a)
        fc.serves.setdefault("j3", []).append(
            {"serve": serve.model_dump(mode="json"), "blob_sha256": fc.put(blob)}
        )
    aud, _ = _auditor(fc, w)
    assert aud.run_once() == "MATCH" and fc.state["j3"] == "closed"


def test_worker_rejects_coordinator_segment_draw_and_malformed_job(carry30, full) -> None:  # type: ignore[no-untyped-def]
    w, honest, _ = carry30
    fc = FakeChallenge(w.m)
    fc.add_job("j4", job_json(w, fc, honest.result, make_challenge("segments", [(0, 1)])))
    bad = job_json(w, fc, honest.result, make_challenge("segments", [(0, 1)]))
    bad["assignment"]["sample_ids"][0] = True
    fc.add_job("j5", bad)
    aud, _ = _auditor(fc, w)
    assert aud.run_once() == "FAILED" and fc.state["j4"] == "failed"
    assert aud.run_once() == "FAILED" and fc.state["j5"] == "failed"
    assert all(f["retry"] is False for f in fc.failures)


def test_verdict_from_non_auditor_rejected(full) -> None:  # type: ignore[no-untyped-def]
    w, res = full
    fc = FakeChallenge(w.m)
    fc.add_job("j6", job_json(w, fc, res, make_challenge()))
    client = TestClient(fc.app())
    aud = Auditor(HttpApi(client, TOKEN), MINER, ENV, get_sample(w))
    assert aud.run_once() == "FAILED"
    assert "422" in fc.failures[-1]["reason"] and "j6" not in fc.verdicts
