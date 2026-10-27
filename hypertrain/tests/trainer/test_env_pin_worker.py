"""ADR 0011 part 3: a worker or an in-process entry point refuses a process that does not honor
the manifest's environment pin, and names the variable. Every case runs in a fresh interpreter
because ATen, MKL and oneDNN read their variables once."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from env_pin_fixtures import CPU_FP32, child, clean_env, needs_avx2
from hypertrain.protocol.env_pins import EnvPin, pinned_environ
from hypertrain.trainer.determinism import env_pin_evidence

REFERENCE_ENV = {
    "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
    "CUDA_DISABLE_PTX_JIT": "1",
    "cpu_threads": 1,
    "pinned": CPU_FP32,
}
APPLY = """
import json, sys
import hypertrain.trainer
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.trainer.determinism import apply_reference_env, env_pin_evidence

env = json.loads(sys.argv[1])
apply_reference_env(env)
print(json.dumps(env_pin_evidence(EnvPin.from_body(env["pinned"]))))
"""
EVIDENCE = """
import json
import hypertrain.trainer
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.trainer.determinism import env_pin_evidence

pin = EnvPin.from_body({"present": {"MKL_CBWR": "AVX2"}, "absent": []})
print(json.dumps(env_pin_evidence(pin)))
"""
# ATen settles its capability at the first op; a variable changed afterwards is not honored.
LATE_VARIABLE = """
import json, os, sys
import hypertrain.trainer
import torch
from hypertrain.trainer.determinism import apply_reference_env

torch.ones(8, 8) @ torch.ones(8, 8)
os.environ["ATEN_CPU_CAPABILITY"] = "avx2"
apply_reference_env(json.loads(sys.argv[1]))
"""
ENTRY_POINTS = """
import dataclasses, json, sys
import hypertrain.trainer
sys.path.insert(0, sys.argv[1])
from trainer_fixtures import assignment, make_cfg, sample
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.protocol.messages import Layout
from hypertrain.trainer.island import emulate, train_island
from hypertrain.trainer.loop import train_round
from hypertrain.trainer.model import init_params
from hypertrain.trainer.unit_order import train_unit_order

cfg = dataclasses.replace(make_cfg(n_stages=1), env_pin=EnvPin.from_body(json.loads(sys.argv[2])))
a, get, theta = assignment(cfg), sample(cfg), init_params(cfg.model)
layout = Layout(pp=1, n_gpus=1, dp_size=1, ep_size=1, zero1=False)
calls = {
    "train_round": lambda: train_round(cfg, theta, a, get),
    "train_island": lambda: emulate(1, lambda c: train_island(cfg, layout, c, theta, a, get)),
    "train_unit_order": lambda: emulate(1, lambda c: train_unit_order(cfg, c, theta, a, get, 1)),
}
out = {}
for name, call in calls.items():
    try:
        call()
        out[name] = "ran"
    except Exception as error:
        out[name] = f"{type(error).__name__}: {error}"
print(json.dumps(out))
"""


def honored() -> dict[str, str]:
    return pinned_environ(clean_env(), EnvPin.from_body(CPU_FP32))


def test_an_unpinned_manifest_adds_nothing_to_the_summary() -> None:
    assert env_pin_evidence(None) == {}


@needs_avx2
def test_worker_accepts_the_pinned_environment_and_records_evidence() -> None:
    done = child(APPLY, honored(), json.dumps(REFERENCE_ENV))
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout) == {"env_pin": {**CPU_FP32, "cpu_capability": "AVX2"}}


def test_evidence_reports_the_capability_torch_resolved() -> None:
    # The pin does not name ATEN_CPU_CAPABILITY, so the process is not refused: the evidence
    # must still say what ATen actually runs, not what a pinned manifest would have wanted.
    done = child(EVIDENCE, clean_env() | {"MKL_CBWR": "AVX2", "ATEN_CPU_CAPABILITY": "default"})
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout)["env_pin"]["cpu_capability"] == "DEFAULT"


@needs_avx2
@pytest.mark.parametrize(
    ("change", "message"),
    [
        pytest.param({"MKL_CBWR": None}, "MKL_CBWR: manifest 'AVX2', live None", id="missing"),
        pytest.param(
            {"MKL_CBWR": "COMPATIBLE"},
            "MKL_CBWR: manifest 'AVX2', live 'COMPATIBLE'",
            id="other-value",
        ),
        pytest.param(
            {"HT_ATTN_BACKEND": None}, "HT_ATTN_BACKEND: manifest 'math', live None", id="backend"
        ),
        pytest.param({"HT_FP8": "1"}, "HT_FP8: manifest absent, live '1'", id="forbidden-fp8"),
        pytest.param(
            {"OMP_THREAD_LIMIT": "2"},
            "OMP_THREAD_LIMIT: manifest absent, live '2'",
            id="forbidden-thread-limit",
        ),
    ],
)
def test_worker_names_the_variable_that_differs(
    change: dict[str, str | None], message: str
) -> None:
    env = honored()
    for name, value in change.items():
        if value is None:
            del env[name]
        else:
            env[name] = value
    done = child(APPLY, env, json.dumps(REFERENCE_ENV))
    assert done.returncode == 1
    assert f"EnvPinMismatch: environment pin mismatch: {message}\n" in done.stderr


def test_capability_is_read_from_torch_not_from_the_variable() -> None:
    env = honored() | {"ATEN_CPU_CAPABILITY": "default"}
    done = child(LATE_VARIABLE, env, json.dumps(REFERENCE_ENV))
    assert done.returncode == 1
    assert (
        "EnvPinMismatch: environment pin mismatch: ATen CPU capability: "
        "manifest 'AVX2', live 'DEFAULT'\n"
    ) in done.stderr


@needs_avx2
@pytest.mark.parametrize("diverging", [False, True], ids=["honored", "diverging"])
def test_every_train_entry_point_refuses_a_diverging_process(diverging: bool) -> None:
    env = honored() | ({"ATEN_CPU_CAPABILITY": "default"} if diverging else {})
    fixtures = str(Path(__file__).parent)
    done = child(ENTRY_POINTS, env, fixtures, json.dumps(CPU_FP32))
    assert done.returncode == 0, done.stderr
    outcome = json.loads(done.stdout)
    assert set(outcome) == {"train_round", "train_island", "train_unit_order"}
    for name, result in outcome.items():
        if diverging:
            assert result.startswith("EnvPinMismatch: environment pin mismatch: "), (name, result)
        else:
            assert result == "ran", (name, result)
