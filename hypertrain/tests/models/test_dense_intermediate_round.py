"""One real-size round of the D68 intermediate TEST model, serialised and replayed (ADR 0015).

It trains two steps of the 100,676,352-parameter row (token batch cut to 1 x 32, which changes no
parameter or artifact size), serialises what an island publishes and replays the round. It takes
about 30 s and 6 GB on one pinned thread, below the od-base round of test_model_registry (54 s,
8.8 GB) that already runs in a shard. The sizes it pins are the inputs of the transfer budget
in ADR 0015: if the serialisation changes, the table there is stale.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import numpy.typing as npt
import torch

from hypertrain.auditor.replay import pack_state
from hypertrain.protocol.env_pins import EnvPin, class_pin
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, replay, train_round
from hypertrain.trainer.model import init_params
from hypertrain.trainer.rng import rng_ctr, uniform_f64

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
sys.path.insert(0, str(SCRIPTS))
from run_mfu import PROFILES, manifest_body  # noqa: E402  (isort: skip)

# Every manifest pins its environment (DET-01); a CPU class with no GPU, as CI runs these.
CPU_PIN = EnvPin.from_body(class_pin("avx2", "none"))

P = 100_676_352
STATE_BYTES = 1_208_136_936  # theta, m, v and the step: 12 P plus the safetensors header
THETA_BYTES = 402_712_448  # theta alone, and so the error-feedback file: 4 P plus the header
DELTA_BYTES = 102_253_376  # dense-int8: 1.01565 P


def test_a_real_size_round_serialises_to_the_budget_sizes_and_replays_match() -> None:
    row = json.loads(PROFILES.read_text())["profiles"]["mfu-dense-0.1b"]
    row["model"].update(seq_len=32, compute_dtype="fp32")  # CPU: no bf16 matmul worth waiting for
    row["inner"].update(H=2, J=2, micro_batch=1)
    cfg = TrainConfig.from_manifest(manifest_body(row, CPU_PIN, torch.device("cpu")))
    run_id = "ab" * 32
    samples = Assignment(run_id=run_id, w=1, sample_ids=(0, 1))

    def sample(i: int) -> npt.NDArray[np.uint32]:
        u = uniform_f64(cfg.model.seq_len + 1, rng_ctr("round-data", 0, i, 0))
        return np.floor(u * cfg.model.vocab).astype(np.uint32)

    theta = init_params(cfg.model)
    assert sum(t.numel() for t in theta.values()) == P

    result = train_round(cfg, theta, samples, sample)

    assert len(pack_state(result.final_theta, result.final_state)) == STATE_BYTES
    assert len(pack_state(result.final_theta)) == THETA_BYTES
    assert len(result.delta_payload) == DELTA_BYTES
    assert len(result.leaves) == 2  # J = H: the start and the end, so two checkpoints
    report = replay(
        cfg, theta, samples, sample, [leaf.digest for leaf in result.leaves], result.delta_hash
    )
    assert report.result == "MATCH"
