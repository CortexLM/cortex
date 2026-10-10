from __future__ import annotations

import importlib.util
from pathlib import Path

import numpy as np
import pytest
import torch
from opendecision.records import RecordShape, pack_record, qmax_for
from pydantic import ValidationError

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.honeypot import Honeypot, honeypot_island
from hypertrain.auditor.replay import pack_state
from hypertrain.models.opendecision import check_manifest, extend_params
from hypertrain.protocol.messages import RunManifest
from hypertrain.protocol.messages_v2 import RunManifestV2
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import TraceContext, emulate, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params


def od_manifest(objective: str, n: int, zero1: bool, dtype: str = "fp32") -> RunManifestV2:
    spec = importlib.util.spec_from_file_location(
        "od_l1_base", Path(__file__).parents[1] / "protocol/test_od_fields.py"
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    b = mod.od_body()
    rec = dict(state_len=8, n_questions=1, n_options=2, opt_len=2, instr_len=2)
    b["model"].update(
        seq_len=8 if objective == "mlm" else 18,
        param_count=116288 if objective == "mlm" else 187140,
        compute_dtype=dtype,
    )
    b["model"]["od"].update(objective=objective, record=None if objective == "mlm" else rec)
    b["inner"].update(H=2, J=1, micro_batch=1, grad_accum=1, state_policy="reset", rewarmup_steps=0)
    b["reference_spec"]["profile"] = "od-fp32-ref-v1" if dtype == "fp32" else "od-bf16-det-eager-v1"
    b["reference_spec"]["layout"].update(pp=1, n_gpus=n, dp_size=n, ep_size=1, zero1=zero1)
    return RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": b,
            "network": {
                **{
                    k: "11" * 32
                    for k in (
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


def samples(cfg: TrainConfig):
    def get(i: int):
        if cfg.model.od.objective == "mlm":
            return 3 + (np.arange(cfg.model.seq_len + 1, dtype=np.uint32) + i) % 200
        shape = RecordShape(**cfg.model.od.record.model_dump())
        return pack_record(
            {
                "state": [3 + i, 4, 5],
                "instr": [[6]],
                "opts": [[[7], [8]]],
                "qtype": [i % 2],
                "y": [i % 2],
                "teacher": [[0.7, 0.2, 0.1]],
            },
            shape,
            qmax_for(cfg.model.vocab),
        ).astype(np.uint32)

    return get


@pytest.mark.parametrize("objective", ["mlm", "decision", "distill"])
@pytest.mark.parametrize("n", [1, 2, 4])
@pytest.mark.parametrize("zero1", [False, True])
def test_od_objectives_layout_reference_and_hooks(objective: str, n: int, zero1: bool) -> None:
    from reference import reference_round

    w = od_manifest(objective, n, zero1)
    check_manifest(w)
    cfg = TrainConfig.from_manifest_v2(w)
    lay = w.training.reference_spec.layout
    theta = init_params(cfg.model)
    a = Assignment(w.run_id(), 0, tuple(range(w.training.batch_samples())))
    get = samples(cfg)
    plain = emulate(n, lambda c: train_island(cfg, lay, c, theta, a, get))
    seen = [[] for _ in range(n)]

    def hook(ctx: TraceContext, x: torch.Tensor) -> torch.Tensor:
        raw = x.detach().cpu().contiguous().reshape(-1).view(torch.uint8).numpy().tobytes()
        seen[ctx.rank].append((ctx, raw))
        return x

    traced = emulate(n, lambda c: train_island(cfg, lay, c, theta, a, get, hook=hook))
    for x in traced:
        assert x.leaf_digests == plain[0].leaf_digests
        assert x.delta_payload == plain[0].delta_payload
        assert pack_state(x.final_theta, x.final_state) == pack_state(
            plain[0].final_theta, plain[0].final_state
        )
    assert plain[0].leaves_root == reference_round(cfg, lay, theta, a, get)["leaves_root"]
    assert all(
        any(ctx.op == "head" and ctx.layer == cfg.model.n_layers for ctx, _ in ops) for ops in seen
    )
    repeated = [[] for _ in range(n)]

    def capture(ctx: TraceContext, x: torch.Tensor) -> torch.Tensor:
        raw = x.detach().cpu().contiguous().reshape(-1).view(torch.uint8).numpy().tobytes()
        repeated[ctx.rank].append((ctx, raw))
        return x

    emulate(n, lambda c: train_island(cfg, lay, c, theta, a, get, hook=capture))
    assert seen == repeated
    if n > 1 or zero1:
        with pytest.raises(ValidationError):
            RunManifest.model_validate(w.training.body())
    if objective != "mlm":
        mlm = TrainConfig.from_manifest_v2(od_manifest("mlm", 1, False))
        extended = extend_params(init_params(mlm.model), cfg.model)
        assert all(
            torch.equal(extended[k], init_params(mlm.model)[k]) for k in init_params(mlm.model)
        )
    pots = emulate(
        n,
        lambda c: honeypot_island(Honeypot(w.training.coord_pubkey, "honest"), w, c, theta, a, get),
    )
    assert pots[0].leaf_digests == plain[0].leaf_digests


@pytest.mark.parametrize("objective", ["mlm", "decision", "distill"])
@pytest.mark.parametrize("zero1", [False, True])
def test_od_bf16_same_layout_cpu(objective: str, zero1: bool) -> None:
    w = od_manifest(objective, 2, zero1, "bf16")
    cfg = TrainConfig.from_manifest_v2(w)
    theta = init_params(cfg.model)
    a = Assignment(w.run_id(), 0, tuple(range(w.training.batch_samples())))
    result = emulate(
        2, lambda c: train_island(cfg, w.training.reference_spec.layout, c, theta, a, samples(cfg))
    )
    assert result[0].leaf_digests == result[1].leaf_digests
    assert result[0].delta_payload == result[1].delta_payload
