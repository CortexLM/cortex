from __future__ import annotations

import hashlib
import json
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest
import torch
from trainer_fixtures import RUN_ID, H, J, assignment, make_cfg, sample, small_manifest_body

from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import LeafPreimage, f32val
from hypertrain.trainer.compress import (
    _sparse_encode,
    canonical_topk,
    compress,
    decompress,
)
from hypertrain.trainer.config import CompressConfig, InnerConfig, TrainConfig
from hypertrain.trainer.determinism import apply_reference_env
from hypertrain.trainer.loop import Assignment, batch_hash, replay, stage_states, train_round
from hypertrain.trainer.model import init_params, route
from hypertrain.trainer.optim import OptState, derive_v0, init_state
from hypertrain.trainer.rng import philox4x32, rng_ctr

HERE = Path(__file__).resolve().parent
SRC = HERE.parents[1] / "src" / "hypertrain" / "trainer"
WORKER = HERE / "_worker.py"
CHILD_ENV = {k: v for k, v in os.environ.items() if k != "CUBLAS_WORKSPACE_CONFIG"}


def _run_worker(*args: str) -> subprocess.Popen[str]:
    return subprocess.Popen(
        [sys.executable, str(WORKER), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=CHILD_ENV,
    )


def _collect(p: subprocess.Popen[str]) -> dict[str, object]:
    out, err = p.communicate(timeout=300)
    assert p.returncode == 0, err
    result: dict[str, object] = json.loads(out)
    return result


@pytest.mark.parametrize(
    ("arch", "dtype", "codec"),
    [
        ("dense", "fp32", "dense-int8"),
        ("dense", "bf16", "sparseloco"),
        ("moe", "fp32", "sparseloco"),
        ("moe", "bf16", "dense-int8"),
    ],
)
def test_two_processes_identical_roots(arch: str, dtype: str, codec: str) -> None:
    procs = [_run_worker(arch, dtype, codec) for _ in range(2)]
    a, b = (_collect(p) for p in procs)
    print(f"{arch}/{dtype}/{codec} leaves_root={a['leaves_root']} delta_hash={a['delta_hash']}")
    assert a["torch_preloaded"] is False
    assert a["leaves"] == b["leaves"]
    assert a["leaves_root"] == b["leaves_root"]
    assert a["delta_hash"] == b["delta_hash"]
    assert a["ef_out_hash"] == b["ef_out_hash"]
    assert len(a["leaves"]) == H // J + 1  # type: ignore[arg-type]


def test_determinism_flags_applied_before_torch_in_child() -> None:
    code = (
        "import sys, os, json; assert 'torch' not in sys.modules;"
        "import hypertrain.trainer as t; import torch;"
        "print(json.dumps({**t.DETERMINISM, 'env': os.environ['CUBLAS_WORKSPACE_CONFIG']}))"
    )
    out = subprocess.run(
        [sys.executable, "-c", code], env=CHILD_ENV, capture_output=True, text=True, check=True
    )
    d = json.loads(out.stdout)
    assert d["env"] == ":4096:8" and d["CUBLAS_WORKSPACE_CONFIG"] == ":4096:8"
    assert d["torch_preloaded"] is False
    assert d["deterministic_algorithms"] is True and d["deterministic_warn_only"] is False
    assert d["cudnn_benchmark"] is False and d["cudnn_deterministic"] is True
    assert d["cudnn_allow_tf32"] is False and d["matmul_allow_tf32"] is False
    assert d["bf16_reduced_precision_reduction"] is False


def test_late_setup_after_bare_torch_import_is_rejected() -> None:
    code = "import torch\nimport hypertrain.trainer\n"
    out = subprocess.run(
        [sys.executable, "-c", code], env=CHILD_ENV, capture_output=True, text=True
    )
    assert out.returncode != 0
    assert "torch imported before hypertrain determinism setup" in out.stderr


@pytest.mark.parametrize("moe", [False, True])
def test_replay_reproduces_every_leaf(moe: bool) -> None:
    cfg = make_cfg(moe=moe)
    theta0, a, get = init_params(cfg.model), assignment(cfg), sample(cfg)
    res = train_round(cfg, theta0, a, get)
    rep = replay(cfg, theta0, a, get, res.leaf_digests, res.delta_hash)
    assert rep.result == "MATCH" and rep.first_bad_leaf is None and rep.delta_match is True
    assert rep.recomputed_leaves_root == res.leaves_root


def test_noise_at_last_step_flags_final_leaf() -> None:
    cfg = make_cfg()
    theta0, a, get = init_params(cfg.model), assignment(cfg), sample(cfg)
    honest = train_round(cfg, theta0, a, get)

    def inject(t: int, theta: dict[str, torch.Tensor]) -> None:
        if t == H - 1:
            theta["layers.000.wq"][0, 0] += 1e-3

    cheat = train_round(cfg, theta0, a, get, after_step=inject)
    rep = replay(cfg, theta0, a, get, cheat.leaf_digests, cheat.delta_hash)
    print(f"honest_root={honest.leaves_root} cheat_root={cheat.leaves_root} report={rep}")
    assert rep.result == "MISMATCH"
    assert rep.first_bad_leaf == H // J
    assert rep.delta_match is False
    assert cheat.leaf_digests[:-1] == honest.leaf_digests[:-1]


def test_one_batch_id_change_affects_only_from_first_leaf() -> None:
    cfg = make_cfg(moe=True)
    theta0, get = init_params(cfg.model), sample(cfg)
    per_step = cfg.inner.micro_batch * cfg.inner.grad_accum
    flip_step = 3
    base = train_round(cfg, theta0, assignment(cfg), get).leaf_digests
    bad = train_round(
        cfg, theta0, assignment(cfg, flip=(flip_step - 1) * per_step), get
    ).leaf_digests
    first = -(-flip_step // J)
    assert base[:first] == bad[:first]
    assert all(x != y for x, y in zip(base[first:], bad[first:], strict=True))


def test_wrong_round_start_is_leaf_zero() -> None:
    cfg = make_cfg()
    theta0, a, get = init_params(cfg.model), assignment(cfg), sample(cfg)
    res = train_round(cfg, theta0, a, get)
    shifted = {k: v.clone() for k, v in theta0.items()}
    shifted["norm.weight"][0] += 1.0
    assert replay(cfg, shifted, a, get, res.leaf_digests).first_bad_leaf == 0
    assert replay(cfg, theta0, a, get, res.leaf_digests[:-1]).first_bad_leaf == H // J


def test_reset_policy_zero_state_at_round_start() -> None:
    cfg = make_cfg(n_stages=2)
    theta0 = init_params(cfg.model)
    dirty = OptState({k: torch.ones_like(v) for k, v in theta0.items()}, {}, 9)
    with pytest.raises(ValueError):
        init_state(cfg.inner, theta0, carry=dirty)
    st = init_state(cfg.inner, theta0)
    assert st.step == 0 and sorted(st.m) == sorted(theta0) == sorted(st.v)
    assert all(int(torch.count_nonzero(t)) == 0 for t in [*st.m.values(), *st.v.values()])
    res = train_round(cfg, theta0, assignment(cfg), sample(cfg))
    assert res.leaves[0].preimage.stages == stage_states(cfg, theta0, st)
    assert res.final_state.step == H
    assert any(int(torch.count_nonzero(t)) for t in res.final_state.m.values())


def test_carry_and_derived_policies_and_muon() -> None:
    base = make_cfg()
    theta0, a, get = init_params(base.model), assignment(base), sample(base)
    first = train_round(base, theta0, a, get)

    carry_cfg = make_cfg(policy="carry")
    with pytest.raises(ValueError):
        train_round(carry_cfg, theta0, a, get)
    c1 = train_round(carry_cfg, theta0, a, get, carry=first.final_state)
    c2 = train_round(carry_cfg, theta0, a, get, carry=first.final_state)
    assert c1.leaves_root == c2.leaves_root != first.leaves_root

    der_cfg = make_cfg(policy="derived")
    v0 = derive_v0({k: torch.full_like(v, 0.5) for k, v in theta0.items()}, H)
    assert torch.equal(v0["norm.weight"], torch.full_like(theta0["norm.weight"], (0.5 / H) ** 2))
    d = train_round(der_cfg, theta0, a, get, v0=v0)
    assert d.leaves[0].digest != first.leaves[0].digest

    muon_cfg = make_cfg(opt="muon", rewarmup=2)
    m1 = train_round(muon_cfg, theta0, a, get)
    m2 = train_round(muon_cfg, theta0, a, get)
    assert m1.leaves_root == m2.leaves_root != first.leaves_root
    assert "layers.000.wq" not in m1.final_state.v and "emb.weight" in m1.final_state.v


def _reference_topk(x: torch.Tensor, k: int) -> list[int]:
    return sorted(sorted(range(x.numel()), key=lambda i: (-abs(float(x[i])), i))[:k])


def test_canonical_topk_permutation_invariance_on_ties() -> None:
    gen = np.random.default_rng(0)
    base = np.array([3.0, -3.0, 3.0, 1.0, -1.0, 2.0, -2.0, 2.0, 0.0, 1.0], dtype=np.float32)
    for k in range(1, base.size + 1):
        expect = canonical_topk(torch.from_numpy(base), k)
        assert expect.tolist() == _reference_topk(torch.from_numpy(base), k)
        for _ in range(20):
            x = base.copy()
            for mag in (3.0, 2.0, 1.0):
                pos = np.flatnonzero(np.abs(x) == mag)
                x[pos] = x[gen.permutation(pos)]
            got = canonical_topk(torch.from_numpy(x), k)
            assert torch.equal(got, expect)
            perm = gen.permutation(base.size)
            y = torch.from_numpy(base[perm])
            sel = canonical_topk(y, k)
            assert sorted(np.abs(base[perm][sel.numpy()]).tolist()) == sorted(
                np.abs(base[expect.numpy()]).tolist()
            )
            assert sel.tolist() == _reference_topk(y, k)


def test_dense_int8_roundtrip_bound_and_ef() -> None:
    gen = torch.Generator().manual_seed(1)
    delta = {"a": torch.randn(1000, generator=gen), "b": torch.randn(3, 7, generator=gen) * 1e-3}
    ef = {k: torch.randn(v.shape, generator=gen) * 1e-4 for k, v in delta.items()}
    payload, ef_out = compress(CompressConfig(), delta, ef)
    codec, dec = decompress(payload)
    assert codec == "dense-int8"
    for k in delta:
        acc = ef[k] + delta[k]
        assert torch.equal(ef_out[k], acc - dec[k])
        flat = acc.reshape(-1)
        for s in range(0, flat.numel(), 256):
            blk = flat[s : s + 256]
            err = (blk - dec[k].reshape(-1)[s : s + 256]).abs().max()
            assert float(err) <= float(blk.abs().max()) / 127 * 0.5 * (1 + 1e-6)
    assert compress(CompressConfig(), delta, ef)[0] == payload


def test_sparseloco_roundtrip_bound_and_ef() -> None:
    gen = torch.Generator().manual_seed(2)
    delta = {"w": torch.randn(64, 32, generator=gen)}
    ef = {"w": torch.randn(64, 32, generator=gen) * 0.1}
    cfg = CompressConfig("sparseloco", topk_frac=0.1, bits=2, ef_beta=0.9)
    payload, ef_out = compress(cfg, delta, ef)
    _, dec = decompress(payload)
    acc = ef["w"] * 0.9 + delta["w"]
    assert torch.equal(ef_out["w"], acc - dec["w"])
    idx = canonical_topk(acc, int(np.ceil(0.1 * acc.numel())))
    nz = torch.nonzero(dec["w"].reshape(-1)).reshape(-1)
    assert torch.equal(nz, idx)
    sel = acc.reshape(-1)[idx].abs()
    span = float(sel.max() - sel.min())
    err = (acc.reshape(-1)[idx] - dec["w"].reshape(-1)[idx]).abs().max()
    assert float(err) <= span / 4 * (1 + 1e-6) + 1e-7
    assert torch.equal(torch.sign(dec["w"].reshape(-1)[idx]), torch.sign(acc.reshape(-1)[idx]))
    body, deq = _sparse_encode(acc, 0.1)
    assert torch.equal(deq, dec["w"])
    assert len(payload) < acc.numel() * 4 * 0.2


@pytest.mark.parametrize(
    "mutate",
    [
        lambda b: b[:-1],
        lambda b: b + b"\x00",
        lambda b: b"bogus" + b[5:],
        lambda b: b[:40],
    ],
)
def test_malformed_payload_rejected(mutate: object) -> None:
    delta = {"w": torch.randn(300)}
    for cfg in (CompressConfig(), CompressConfig("sparseloco", topk_frac=0.1, bits=2)):
        payload, _ = compress(cfg, delta, {"w": torch.zeros(300)})
        with pytest.raises(ValueError):
            decompress(mutate(payload))  # type: ignore[operator]


def test_router_ties_lowest_index_and_capacity_order() -> None:
    logits = torch.tensor([[1.0, 1.0, 1.0, 0.0], [0.0, 2.0, 2.0, 2.0], [5.0, 5.0, 0.0, 0.0]])
    experts, keep = route(logits, 2, capacity=10)
    assert experts.view(2, 3).t().tolist() == [[0, 1], [1, 2], [0, 1]]
    assert bool(keep.all())
    experts, keep = route(torch.zeros(5, 2), 1, capacity=2)
    assert experts.tolist() == [0] * 5
    assert keep.tolist() == [True, True, False, False, False]


def test_malformed_config_and_manifest_rejected() -> None:
    with pytest.raises(ValueError):
        InnerConfig(lr=1e-3, H=5, J=2, micro_batch=1)
    with pytest.raises(ValueError):
        CompressConfig("sparseloco", topk_frac=0.1, bits=8)
    body = example_manifest().body()
    for path, value in [
        (("model", "router_tiebreak"), "random"),
        (("model", "d_model"), "32"),
        (("inner", "lr_schedule", "peak_lr"), 3e-4),
        (("reference_spec", "env", "cpu_threads"), "1"),
        (("reference_spec", "layout", "pp"), 0),
    ]:
        bad = json.loads(json.dumps(body))
        node = bad
        for k in path[:-1]:
            node = node[k]
        node[path[-1]] = value
        with pytest.raises(ValueError):
            TrainConfig.from_manifest(bad)
    cfg = make_cfg()
    with pytest.raises(ValueError):
        train_round(cfg, init_params(cfg.model), Assignment(RUN_ID, 0, (1, 2)), sample(cfg))


def test_protocol_manifest_parses_to_config() -> None:
    body = example_manifest().body()
    cfg = TrainConfig.from_manifest(body)
    assert cfg.inner.lr == f32val(body["inner"]["lr_schedule"]["peak_lr"])
    assert cfg.inner.beta2 == f32val(body["inner"]["betas"][1])
    assert cfg.model.capacity_factor == f32val(body["model"]["capacity_factor"])
    assert cfg.model.aux_loss_coef == f32val(body["model"]["aux_loss_coef"])
    assert cfg.model.init_std == f32val(body["model"]["init_std"])
    assert cfg.inner.muon_momentum == f32val(body["inner"]["muon_momentum"])
    assert cfg.inner.ns_steps == body["inner"]["ns_steps"]
    assert cfg.cpu_threads == body["reference_spec"]["env"]["cpu_threads"]
    assert cfg.n_stages == body["reference_spec"]["layout"]["pp"]
    assert cfg.compress.codec == "dense-int8" and not cfg.model.is_moe


def test_replay_from_protocol_manifest_and_leaf_preimages() -> None:
    body = small_manifest_body(moe=True, opt="muon", codec="sparseloco")
    cfg = TrainConfig.from_manifest(body)
    assert cfg.model.is_moe and cfg.inner.opt == "muon" and cfg.n_stages == 2
    theta0, a, get = init_params(cfg.model), assignment(cfg), sample(cfg)
    res = train_round(cfg, theta0, a, get)
    for rec in res.leaves:
        pre = LeafPreimage.model_validate(rec.preimage.model_dump(mode="json"))
        assert pre.rng_ctr <= 2**53 - 1
        assert bytes.fromhex(pre.digest()) == rec.digest
    assert MerkleTree([bytes.fromhex(r.preimage.digest()) for r in res.leaves]).root.hex() == (
        res.leaves_root
    )
    rep = replay(cfg, theta0, a, get, res.leaf_digests, res.delta_hash)
    assert rep.result == "MATCH" and rep.first_bad_leaf is None


def test_batch_hash_is_committed_in_leaf() -> None:
    cfg = make_cfg()
    res = train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))
    honest_root = res.leaves_root
    zero = "00" * 32
    mutated = [r.preimage.model_copy(update={"batch_ids_sha256": zero}) for r in res.leaves]
    mutated_root = MerkleTree([bytes.fromhex(p.digest()) for p in mutated]).root.hex()
    assert mutated_root != honest_root
    for r, p in zip(res.leaves, mutated, strict=True):
        if r.preimage.batch_ids_sha256 != zero:
            assert bytes.fromhex(p.digest()) != r.digest


def test_rng_ctr_fits_protocol_range() -> None:
    for w in (0, 1, 2**40):
        for t in range(0, 50):
            assert 0 <= rng_ctr(RUN_ID, w, t, -1) <= 2**53 - 1


def test_thread_pin_from_manifest_and_mismatch_refused() -> None:
    body = example_manifest().body()
    code = (
        "import json, sys; from hypertrain.trainer.determinism import apply_reference_env;"
        "env = json.loads(sys.argv[1]); print(apply_reference_env(env)['num_threads'])"
    )
    env2 = dict(body["reference_spec"]["env"], cpu_threads=2)
    out = subprocess.run(
        [sys.executable, "-c", code, json.dumps(env2)],
        env=CHILD_ENV,
        capture_output=True,
        text=True,
        check=True,
    )
    assert out.stdout.strip() == "2"
    cfg = TrainConfig.from_manifest(
        json.loads(json.dumps(body).replace('"cpu_threads": 1', '"cpu_threads": 3'))
    )
    assert cfg.cpu_threads == 3
    with pytest.raises(RuntimeError, match="cpu thread pin mismatch"):
        train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))
    bad_env = dict(body["reference_spec"]["env"], CUBLAS_WORKSPACE_CONFIG=":16:8")
    with pytest.raises(RuntimeError):
        apply_reference_env(bad_env)


def test_sample_validation() -> None:
    cfg = make_cfg()

    def bad(i: int) -> np.ndarray:  # type: ignore[type-arg]
        return np.full(cfg.model.seq_len + 1, cfg.model.vocab, dtype=np.uint32)

    with pytest.raises(ValueError):
        train_round(cfg, init_params(cfg.model), assignment(cfg), bad)


def test_philox_known_answer_vectors() -> None:
    zero = philox4x32(np.zeros((1, 4), dtype=np.uint32), (0, 0))
    assert zero[0].tolist() == [0x6627E8D5, 0xE169C58D, 0xBC57AC4C, 0x9B00DBD8]
    ones = philox4x32(np.full((1, 4), 0xFFFFFFFF, dtype=np.uint32), (0xFFFFFFFF, 0xFFFFFFFF))
    assert ones[0].tolist() == [0x408F276D, 0x41C83B0E, 0xA20BC7C6, 0x6D5451FD]


def test_forbidden_constructs_absent() -> None:
    forbidden = re.compile(r"\b(topk|index_add_?|scatter_add_?|compile|allclose|isclose)\(")
    for path in SRC.glob("*.py"):
        hits = forbidden.findall(path.read_text())
        assert not hits, f"{hits} in {path.name}"


def test_leaf_batch_ids_sha256_matches_assigned_ids() -> None:
    cfg = make_cfg()
    a = assignment(cfg)
    res = train_round(cfg, init_params(cfg.model), a, sample(cfg))
    per = cfg.inner.micro_batch * cfg.inner.grad_accum
    assert res.leaves[0].preimage.batch_ids_sha256 == hashlib.sha256(b"").hexdigest()
    for leaf in res.leaves[1:]:
        ids = a.sample_ids[(leaf.t - J) * per : leaf.t * per]
        assert len(ids) == J * per
        expect = hashlib.sha256(b"".join(struct.pack("<Q", i) for i in ids)).hexdigest()
        assert leaf.preimage.batch_ids_sha256 == expect == batch_hash(ids)
