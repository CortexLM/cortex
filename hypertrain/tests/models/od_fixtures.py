from __future__ import annotations

import copy
import importlib.util
from pathlib import Path
from typing import Any

import numpy as np
import numpy.typing as npt
import torch
from opendecision.records import RecordShape, pack_record, qmax_for

from hypertrain.protocol.messages import f32hex
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, SampleFn
from hypertrain.trainer.optim import OptState
from hypertrain.trainer.rng import rng_ctr, uniform_f64

RUN_ID = "cd" * 32
SEQ = 32
H, J, MB = 3, 1, 2
REC = {"state_len": 16, "n_questions": 2, "n_options": 3, "opt_len": 4, "instr_len": 4}
STAGE_A_COUNT, FULL_COUNT = 116_288, 187_140


def _l2_od_body() -> dict[str, Any]:
    p = Path(__file__).parents[1] / "protocol" / "test_od_fields.py"
    spec = importlib.util.spec_from_file_location("_od_fields_l3", p)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.od_body()  # type: ignore[no-any-return]


def od_body(objective: str = "mlm", profile: str = "od-fp32-ref-v1") -> dict[str, Any]:
    b = copy.deepcopy(_l2_od_body())
    b["reference_spec"]["profile"] = profile
    b["model"]["compute_dtype"] = "fp32" if profile == "od-fp32-ref-v1" else "bf16"
    b["inner"] |= {"H": H, "J": J, "micro_batch": MB, "grad_accum": 1}
    b["inner"]["lr_schedule"] |= {"peak_lr": f32hex(1e-3), "warmup": 1}
    if objective == "mlm":
        b["model"] |= {"seq_len": SEQ, "param_count": STAGE_A_COUNT}
    else:
        shape = RecordShape(**REC)
        b["model"] |= {"seq_len": shape.length - 1, "param_count": FULL_COUNT}
        b["model"]["od"] |= {"objective": objective, "record": dict(REC)}
    return b


def cfg_of(objective: str = "mlm", profile: str = "od-fp32-ref-v1") -> TrainConfig:
    return TrainConfig.from_manifest(od_body(objective, profile))


def text_sample(cfg: TrainConfig) -> SampleFn:
    def get(i: int) -> npt.NDArray[np.uint32]:
        u = uniform_f64(cfg.model.seq_len + 1, rng_ctr("od-data", 0, i, 0))
        return (3 + np.floor(u * (cfg.model.vocab - 3))).astype(np.uint32)

    return get


def record_sample(cfg: TrainConfig) -> SampleFn:
    shape, qmax = RecordShape(**REC), qmax_for(cfg.model.vocab)

    def get(i: int) -> npt.NDArray[np.uint32]:
        r = np.random.default_rng(i)

        def ids(n: int) -> list[int]:
            return [int(x) for x in r.integers(3, cfg.model.vocab, n)]

        kq = [3, 2]
        teacher = []
        for k in kq:
            p = r.random(k + 1)
            teacher.append(list(p / p.sum()))
        ex = {
            "state": ids(12),
            "instr": [ids(3), ids(2)],
            "opts": [[ids(4) for _ in range(k)] for k in kq],
            "qtype": [i % 2, 0],
            "y": [i % 3, None],
            "teacher": teacher,
        }
        return pack_record(ex, shape, qmax).astype(np.uint32)

    return get


def assignment(cfg: TrainConfig) -> Assignment:
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum
    return Assignment(run_id=RUN_ID, w=1, sample_ids=tuple(100 + 7 * i for i in range(n)))


def fresh_carry(theta: dict[str, torch.Tensor]) -> OptState:
    return OptState(
        {n: torch.zeros_like(x) for n, x in theta.items()},
        {n: torch.zeros_like(x) for n, x in theta.items()},
        0,
    )
