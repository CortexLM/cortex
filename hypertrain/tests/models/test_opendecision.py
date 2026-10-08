"""L3: hypertrain.models.opendecision adapter (design B2, C row L3)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest
import torch
from od_fixtures import (
    FULL_COUNT,
    STAGE_A_COUNT,
    assignment,
    cfg_of,
    fresh_carry,
    od_body,
    record_sample,
    text_sample,
)
from opendecision.masking import mask_positions, n_mask_for
from opendecision.model import OpenDecisionModel
from opendecision.records import RecordShape, qmax_for, unpack_records
from opendecision.train import decision_loss, mlm_loss

import hypertrain.models.opendecision as od
from hypertrain.auditor.bisect import OPS, TAIL_OPS
from hypertrain.auditor.replay import pack_state, unpack_state
from hypertrain.datasets.shards16 import write_shards16
from hypertrain.models.opendecision.__main__ import main
from hypertrain.protocol.messages import RunManifest, f32val
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import replay, train_round
from hypertrain.trainer.model import forward as dispatched_forward

HEAD_PREFIXES = ("head.", "scorer.", "type_emb.", "unknown", "log_temp.")


def _round(cfg: TrainConfig, get: object) -> object:
    theta = od.init_params(cfg.model)
    return train_round(cfg, theta, assignment(cfg), get, carry=fresh_carry(theta))  # type: ignore[arg-type]


def test_train_round_twice_identical_leaves_root() -> None:
    cfg = cfg_of()
    a, b = _round(cfg, text_sample(cfg)), _round(cfg, text_sample(cfg))
    assert a.leaves_root == b.leaves_root  # type: ignore[attr-defined]
    assert a.delta_hash == b.delta_hash  # type: ignore[attr-defined]
    assert len(a.leaves) == cfg.inner.H // cfg.inner.J + 1  # type: ignore[attr-defined]


def test_replay_match_and_flipped_token_mismatch_at_right_leaf() -> None:
    cfg = cfg_of()
    get = text_sample(cfg)
    theta = od.init_params(cfg.model)
    asg = assignment(cfg)
    res = train_round(cfg, theta, asg, get, carry=fresh_carry(theta))
    ok = replay(cfg, theta, asg, get, res.leaf_digests, res.delta_hash, carry=fresh_carry(theta))
    assert (ok.result, ok.first_bad_leaf, ok.delta_match) == ("MATCH", None, True)

    t_bad = 2
    victim = asg.batch_ids(cfg, t_bad)[0]

    def flipped(i: int) -> np.ndarray:
        s = get(i).copy()
        if i == victim:
            s[5] = 3 + (int(s[5]) - 2) % (cfg.model.vocab - 3)
        return s

    bad = replay(
        cfg, theta, asg, flipped, res.leaf_digests, res.delta_hash, carry=fresh_carry(theta)
    )
    assert bad.result == "MISMATCH"
    assert bad.first_bad_leaf == t_bad // cfg.inner.J


def _loss_and_grads(
    cfg: TrainConfig, theta: dict[str, torch.Tensor], tokens: torch.Tensor, traced: bool
) -> tuple[torch.Tensor, list[torch.Tensor]]:
    names = sorted(theta)
    p = {n: theta[n].clone().requires_grad_(True) for n in names}
    if traced:
        ce, _ = od.traced_forward(cfg, p, tokens, lambda layer, op, x: x)
    else:
        ce, _ = od.forward(cfg.model, p, tokens)
    gs = torch.autograd.grad(ce, [p[n] for n in names], allow_unused=True)
    return ce.detach(), [
        torch.zeros_like(p[n]) if g is None else g for n, g in zip(names, gs, strict=True)
    ]


@pytest.mark.parametrize(
    ("objective", "profile"),
    [
        ("mlm", "od-fp32-ref-v1"),
        ("mlm", "od-bf16-det-eager-v1"),
        ("decision", "od-fp32-ref-v1"),
        ("distill", "od-bf16-det-eager-v1"),
    ],
)
def test_forward_equals_traced_forward_bitwise(objective: str, profile: str) -> None:
    cfg = cfg_of(objective, profile)
    get = text_sample(cfg) if objective == "mlm" else record_sample(cfg)
    tokens = torch.from_numpy(np.stack([get(i) for i in range(2)]).astype(np.int64))
    theta = od.init_params(cfg.model)
    la, ga = _loss_and_grads(cfg, theta, tokens, traced=False)
    lb, gb = _loss_and_grads(cfg, theta, tokens, traced=True)
    assert torch.isfinite(la) and la.dtype == torch.float32
    assert torch.equal(la, lb)
    assert all(torch.equal(x, y) for x, y in zip(ga, gb, strict=True))
    assert any(bool(g.abs().sum() > 0) for g in ga)


@pytest.mark.parametrize("objective", ["mlm", "decision"])
def test_adapter_matches_opendecision_module(objective: str) -> None:
    """Oracle: the adapter's functional encoder equals L1's nn.Module path on the same weights."""
    cfg = cfg_of(objective)
    m, spec = cfg.model, cfg.model.od
    assert spec is not None
    get = text_sample(cfg) if objective == "mlm" else record_sample(cfg)
    tokens = torch.from_numpy(np.stack([get(i) for i in range(2)]).astype(np.int64))
    theta = od.init_params(m)
    model = OpenDecisionModel(od.od_config(m))
    missing, unexpected = model.load_state_dict(theta, strict=False)
    assert not unexpected and all(k.startswith(HEAD_PREFIXES) for k in missing)
    with torch.no_grad():
        mine = od.forward(m, theta, tokens)[0]
        if objective == "mlm":
            ids = tokens[:, : m.seq_len]
            n = n_mask_for(m.seq_len, f32val(spec.mask_ratio))
            pos = torch.from_numpy(mask_positions(ids.numpy(), spec.mask_seed, n))
            ref = mlm_loss(model, ids, pos)
        else:
            assert spec.record is not None
            shape = RecordShape(**spec.record.model_dump())
            b = unpack_records(tokens, shape, qmax_for(m.vocab))
            logits, ext = model(
                b["ids"], b["mask"], b["opt_ids"], b["opt_mask"], b["instr_ids"], b["qtype"]
            )
            ref = decision_loss(logits, ext, b["y"], b["qtype"], spec.decision_rule)
    torch.testing.assert_close(mine, ref, rtol=1e-5, atol=1e-6)


def test_traced_forward_op_names_match_bisect() -> None:
    cfg = cfg_of()
    tokens = torch.from_numpy(np.stack([text_sample(cfg)(0)]).astype(np.int64))
    seen: list[tuple[int, str]] = []

    def hook(layer: int, op: str, x: torch.Tensor) -> torch.Tensor:
        seen.append((layer, op))
        return x

    od.traced_forward(cfg, od.init_params(cfg.model), tokens, hook)
    L = cfg.model.n_layers
    want = [(-1, "input"), (0, "embed")]
    want += [(i, o) for i in range(L) for o in OPS]
    want += [(L, o) for o in TAIL_OPS[:3]]
    assert seen == want


def test_det_gather_equals_torch_gather_value_and_grad() -> None:
    g = torch.Generator().manual_seed(3)
    B, N, d, n_mask = 3, 10, 5, 4
    h = torch.randn(B, N, d, generator=g, requires_grad=True)
    pos = torch.sort(torch.stack([torch.randperm(N, generator=g)[:n_mask] for _ in range(B)]))[0]
    up = torch.randn(B * n_mask, d, generator=g)
    mine = od._det_gather(h, pos)
    ref = torch.gather(h, 1, pos[..., None].expand(-1, -1, d)).reshape(-1, d)
    assert torch.equal(mine, ref)
    (gm,) = torch.autograd.grad((mine * up).sum(), h)
    (gr,) = torch.autograd.grad((ref * up).sum(), h)
    assert torch.equal(gm, gr)


def test_forward_moe_none_accepted_and_non_none_rejected() -> None:
    cfg = cfg_of()
    tokens = torch.from_numpy(np.stack([text_sample(cfg)(0)]).astype(np.int64))
    theta = od.init_params(cfg.model)
    ce, aux = dispatched_forward(cfg.model, theta, tokens, moe=None)
    assert torch.isfinite(ce) and float(aux) == 0.0
    with pytest.raises(ValueError, match="moe"):
        od.forward(cfg.model, theta, tokens, moe=object())
    with pytest.raises(ValueError, match="moe"):
        dispatched_forward(cfg.model, theta, tokens, moe=lambda *a: a)  # type: ignore[arg-type,return-value]


def test_stage_a_param_set_excludes_head() -> None:
    a, full = cfg_of("mlm").model, cfg_of("decision").model
    sa, sf = od.param_shapes(a), od.param_shapes(full)
    assert all(n.startswith(("tok.", "encoder.")) for n in sa)
    assert not any(n.startswith(HEAD_PREFIXES) for n in sa)
    assert sum(int(np.prod(s)) for s in sa.values()) == STAGE_A_COUNT
    assert sum(int(np.prod(s)) for s in sf.values()) == FULL_COUNT
    with torch.device("meta"):
        sd = OpenDecisionModel(od.od_config(full)).state_dict()
    assert sf == {k: tuple(v.shape) for k, v in sd.items()}
    assert sorted(od.init_params(a)) == sorted(sa)
    assert od.stage_of("tok.weight", a, 2) == 0
    assert od.stage_of("encoder.blocks.1.qkv.weight", a, 2) == 1
    assert od.stage_of("scorer.3.bias", full, 2) == 1


def test_decision_record_path_tiny_shape_trains() -> None:
    cfg = cfg_of("decision")
    get = record_sample(cfg)
    tokens = torch.from_numpy(np.stack([get(i) for i in range(2)]).astype(np.int64))
    theta = od.init_params(cfg.model)
    _, grads = _loss_and_grads(cfg, theta, tokens, traced=False)
    named = dict(zip(sorted(theta), grads, strict=True))
    assert float(named["scorer.3.weight"].abs().sum()) > 0
    assert float(named["head.0.cross.q.weight"].abs().sum()) > 0
    a, b = _round(cfg, get), _round(cfg, get)
    assert a.leaves_root == b.leaves_root  # type: ignore[attr-defined]


def test_import_sets_cublas_before_torch() -> None:
    code = (
        "import os, sys\n"
        "seen = {}\n"
        "class F:\n"
        "    def find_spec(self, name, path=None, target=None):\n"
        "        if name == 'torch' and 'torch' not in seen:\n"
        "            seen['torch'] = os.environ.get('CUBLAS_WORKSPACE_CONFIG')\n"
        "        return None\n"
        "sys.meta_path.insert(0, F())\n"
        "import hypertrain.models.opendecision\n"
        "import torch\n"
        "print(seen['torch'], torch.are_deterministic_algorithms_enabled())\n"
    )
    env = {k: v for k, v in os.environ.items() if k != "CUBLAS_WORKSPACE_CONFIG"}
    out = subprocess.run(
        [sys.executable, "-c", code], env=env, capture_output=True, text=True, check=True
    ).stdout.split()
    assert out == [":4096:8", "True"]


def test_extend_params_and_warmstart_cli(tmp_path: Path) -> None:
    a, full = cfg_of("mlm").model, cfg_of("decision").model
    theta_a = od.init_params(a)
    ext = od.extend_params(theta_a, full)
    assert sorted(ext) == sorted(od.param_shapes(full))
    ref = od.init_params(full)
    assert all(torch.equal(ext[n], ref[n]) for n in ext)
    with pytest.raises(ValueError):
        od.extend_params({**theta_a, "bogus": torch.zeros(1)}, full)

    blob = tmp_path / "a.state"
    blob.write_bytes(pack_state({n: x + 1 for n, x in theta_a.items()}))
    man = tmp_path / "run.json"
    man.write_text(json.dumps(od_body("decision")))
    args = ["warmstart", "--state", str(blob), "--manifest", str(man)]
    assert main([*args, "--out", str(tmp_path / "o")]) == 0
    (out,) = (tmp_path / "o").iterdir()
    theta, _ = unpack_state(out.read_bytes())
    assert torch.equal(theta["tok.weight"], theta_a["tok.weight"] + 1)
    assert torch.equal(theta["scorer.1.weight"], ref["scorer.1.weight"])


def test_check_manifest_param_count() -> None:
    od.check_manifest(RunManifest.model_validate(od_body()))
    od.check_manifest(RunManifest.model_validate(od_body("decision")))
    b = od_body()
    b["model"]["param_count"] = FULL_COUNT
    with pytest.raises(ValueError, match="param_count"):
        od.check_manifest(RunManifest.model_validate(b))


def test_eval_holdout_and_eval_cli(tmp_path: Path) -> None:
    cfg = cfg_of()
    get = text_sample(cfg)
    theta = od.init_params(cfg.model)
    v = od.eval_holdout(cfg.model, theta, get, [0, 1, 2], batch=2)
    tokens = torch.from_numpy(np.stack([get(i) for i in range(3)]).astype(np.int64))
    with torch.no_grad():
        l01 = float(od.forward(cfg.model, theta, tokens[:2])[0])
        l2 = float(od.forward(cfg.model, theta, tokens[2:])[0])
    assert v == pytest.approx((2 * l01 + l2) / 3, rel=0, abs=1e-12)

    hold = tmp_path / "hold"
    write_shards16((get(i) for i in range(8)), hold, cfg.model.seq_len, unit=4, samples_per_shard=8)
    state, man, metrics = tmp_path / "s", tmp_path / "run.json", tmp_path / "public" / "m.jsonl"
    state.write_bytes(pack_state(theta))
    man.write_text(json.dumps(od_body()))
    args = ["eval", "--manifest", str(man), "--state", str(state), "--holdout", str(hold)]
    assert main([*args, "--w", "4", "--metrics", str(metrics)]) == 0
    line = json.loads(metrics.read_text())
    run_id = RunManifest.model_validate(od_body()).run_id()
    want = od.eval_holdout(cfg.model, theta, get, range(8))
    assert line == {"run_id": run_id, "w": 4, "value": want}
