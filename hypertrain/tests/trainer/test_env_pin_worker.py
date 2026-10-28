"""ADR 0011 part 3: a worker or an in-process entry point refuses a process that does not honor
the manifest's environment pin, and names the variable. Every case runs in a fresh interpreter
because ATen, MKL and oneDNN read their variables once."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from env_pin_fixtures import (
    CPU_AVX2,
    CPU_AVX512,
    child,
    clean_env,
    honest_observation,
    needs_avx2,
    needs_avx512,
)
from hypertrain.protocol.env_pins import EnvPin, class_pin, pinned_environ
from hypertrain.trainer.determinism import EnvPinMismatch, apply_reference_env, require_gpu_class

REFERENCE_ENV = {
    "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
    "CUDA_DISABLE_PTX_JIT": "1",
    "cpu_threads": 1,
    "pinned": CPU_AVX2,
}
APPLY = """
import json, sys
import hypertrain.trainer
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.trainer.determinism import apply_reference_env, env_observation

env = json.loads(sys.argv[1])
apply_reference_env(env)
print(json.dumps(env_observation(EnvPin.from_body(env["pinned"]), None)))
"""
OBSERVE = """
import json
import hypertrain.trainer
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.trainer.determinism import env_observation

pin = EnvPin.from_body({"cpu": "avx2", "gpu": "none", **json.loads(__import__("sys").argv[1])})
print(json.dumps(env_observation(pin, None)))
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


def honored(pin: dict[str, object] = CPU_AVX2) -> dict[str, str]:
    return pinned_environ(clean_env(), EnvPin.from_body(pin))


def test_the_worker_refuses_a_manifest_without_the_pin() -> None:
    env = {k: v for k, v in REFERENCE_ENV.items() if k != "pinned"}
    with pytest.raises(RuntimeError, match=r"reference_spec\.env\.pinned is required"):
        apply_reference_env(env)


@needs_avx2
def test_worker_accepts_the_pinned_environment_and_observes_it() -> None:
    done = child(APPLY, honored(), json.dumps(REFERENCE_ENV))
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout) == honest_observation(CPU_AVX2)


@needs_avx512
def test_worker_accepts_the_avx512_class_and_observes_it() -> None:
    reference = {**REFERENCE_ENV, "pinned": CPU_AVX512}
    done = child(APPLY, honored(CPU_AVX512), json.dumps(reference))
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout) == honest_observation(CPU_AVX512)


@needs_avx2
def test_a_process_held_to_avx2_refuses_the_avx512_class() -> None:
    reference = {**REFERENCE_ENV, "pinned": CPU_AVX512}
    done = child(
        APPLY, honored(CPU_AVX512) | {"ATEN_CPU_CAPABILITY": "avx2"}, json.dumps(reference)
    )
    assert done.returncode == 1
    assert "ATen CPU capability: manifest 'AVX512', live 'AVX2'" in done.stderr


def test_observation_reports_the_capability_torch_resolved() -> None:
    # The observation is what the process runs, not what the pin would have wanted.
    env = clean_env() | {"MKL_CBWR": "COMPATIBLE", "ATEN_CPU_CAPABILITY": "default"}
    done = child(OBSERVE, env, json.dumps({k: CPU_AVX2[k] for k in ("present", "absent")}))
    assert done.returncode == 0, done.stderr
    seen = json.loads(done.stdout)
    assert seen["cpu_capability"] == "DEFAULT"
    assert seen["env"]["MKL_CBWR"] == "COMPATIBLE" and seen["env"]["HT_FP8"] is None


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
            {"ONEDNN_MAX_CPU_ISA": "ALL"},
            "ONEDNN_MAX_CPU_ISA: manifest 'AVX2', live 'ALL'",
            id="onednn",
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


@pytest.mark.parametrize(
    ("gpu", "device", "message"),
    [
        pytest.param("none", None, None, id="cpu-run-no-device"),
        pytest.param("sm_103", (10, 3), None, id="b300"),
        pytest.param("sm_120", (12, 0), None, id="rtx5090"),
        pytest.param("sm_103", (12, 0), "manifest 'sm_103', live 'sm_120'", id="other-device"),
        pytest.param("sm_103", None, "manifest 'sm_103', live 'none'", id="no-device"),
        pytest.param("none", (10, 3), "manifest 'none', live 'sm_103'", id="device-on-a-cpu-run"),
    ],
)
def test_the_device_must_be_the_gpu_class(
    gpu: str, device: tuple[int, int] | None, message: str | None
) -> None:
    pin = EnvPin.from_body(class_pin("avx2", gpu))
    if message is None:
        require_gpu_class(pin, device)
    else:
        with pytest.raises(EnvPinMismatch, match=f"GPU class: {message}"):
            require_gpu_class(pin, device)


@needs_avx2
@pytest.mark.parametrize("diverging", [False, True], ids=["honored", "diverging"])
def test_every_train_entry_point_refuses_a_diverging_process(diverging: bool) -> None:
    env = honored() | ({"ATEN_CPU_CAPABILITY": "default"} if diverging else {})
    fixtures = str(Path(__file__).parent)
    done = child(ENTRY_POINTS, env, fixtures, json.dumps(CPU_AVX2))
    assert done.returncode == 0, done.stderr
    outcome = json.loads(done.stdout)
    assert set(outcome) == {"train_round", "train_island", "train_unit_order"}
    for name, result in outcome.items():
        if diverging:
            assert result.startswith("EnvPinMismatch: environment pin mismatch: "), (name, result)
        else:
            assert result == "ran", (name, result)
