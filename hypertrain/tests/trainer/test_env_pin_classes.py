"""ADR 0011 part 3: a CPU class is sufficient for the real TEST model. Under the class pin a
hostile ambient gives the roots of the clean class environment, and the class is the domain it
names. A fresh interpreter per case because ATen, MKL and oneDNN read their variables once.

Mutant guards, not red-before tests: they pass on any tree that has the table. They kill a class
that loses a key the model's roots depend on (MKL_CBWR or ATEN_CPU_CAPABILITY for fp32,
ONEDNN_MAX_CPU_ISA for bf16) and a protocol.md that drifts from the classes it documents."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from env_pin_fixtures import HOSTILE, child, clean_env, needs_avx2, needs_avx512
from hypertrain.protocol.env_pins import CPU_CLASSES, GPU_CLASSES, EnvPin, class_pin, pinned_environ

KIMI_ROOTS = """
import dataclasses, json, sys
import hypertrain.trainer
import hypertrain.models.kimi_tiny as kt
from hypertrain.protocol.env_pins import EnvPin, class_pin

doc = kt.run_manifest(dataclasses.replace(kt.MICRO, compute_dtype=sys.argv[1]))
pin = EnvPin.from_body(class_pin(sys.argv[2], "none"))
(result,) = kt.qualify(doc, "cpu", 1, pin)
print(json.dumps([result[key] for key in ("leaves_root", "delta_hash", "state_root")]))
"""
# A bf16 matmul at K=4096: MICRO's K is too small to show ONEDNN_MAX_CPU_ISA on the avx512 class.
BF16_MATMUL = """
import hashlib
import hypertrain.trainer
import torch

torch.set_num_threads(1)
g = torch.Generator().manual_seed(7)
a = torch.randn(64, 4096, generator=g).bfloat16()
b = torch.randn(4096, 64, generator=g).bfloat16()
print(hashlib.sha256((a @ b).float().numpy().tobytes()).hexdigest())
"""
CLASSES = [
    pytest.param("avx2", marks=needs_avx2, id="avx2"),
    pytest.param("avx512", marks=needs_avx512, id="avx512"),
]


def pin_of(cpu: str) -> EnvPin:
    return EnvPin.from_body(class_pin(cpu, "none"))


def roots(env: dict[str, str], dtype: str, cpu: str) -> list[str]:
    done = child(KIMI_ROOTS, env, dtype, cpu)
    assert done.returncode == 0, done.stderr
    return list(json.loads(done.stdout))


def matmul(env: dict[str, str]) -> str:
    done = child(BF16_MATMUL, env)
    assert done.returncode == 0, done.stderr
    return done.stdout.strip()


@pytest.mark.parametrize("dtype", ["fp32", "bf16"])
@pytest.mark.parametrize("cpu", CLASSES)
def test_kimi_roots_follow_the_cpu_class(cpu: str, dtype: str) -> None:
    reference = roots(pinned_environ(clean_env(), pin_of(cpu)), dtype, cpu)
    hostile = pinned_environ(clean_env() | HOSTILE, pin_of(cpu))
    assert roots(hostile, dtype, cpu) == reference


@needs_avx2
@needs_avx512
@pytest.mark.parametrize("dtype", ["fp32", "bf16"])
def test_the_two_cpu_classes_are_two_domains(dtype: str) -> None:
    avx2 = roots(pinned_environ(clean_env(), pin_of("avx2")), dtype, "avx2")
    avx512 = roots(pinned_environ(clean_env(), pin_of("avx512")), dtype, "avx512")
    assert avx2 != avx512


@pytest.mark.parametrize("cpu", CLASSES)
def test_onednn_isa_is_needed_by_every_cpu_class(cpu: str) -> None:
    # Without ONEDNN_MAX_CPU_ISA in the class a hostile ambient changes a bf16 matmul.
    pin = pin_of(cpu)
    without = EnvPin(
        pin.cpu, pin.gpu, tuple(x for x in pin.present if x[0] != "ONEDNN_MAX_CPU_ISA"), pin.absent
    )
    reference = matmul(pinned_environ(clean_env(), pin))
    assert matmul(pinned_environ(clean_env() | HOSTILE, pin)) == reference
    assert matmul(pinned_environ(clean_env() | HOSTILE, without)) != reference


def test_protocol_doc_carries_the_classes_it_is_tested_with() -> None:
    doc = (Path(__file__).parents[2] / "docs/protocol.md").read_text()
    for name in (*CPU_CLASSES, *GPU_CLASSES):
        assert f"`{name}`" in doc, name
    for cpu in CPU_CLASSES.values():
        for value in cpu.present.values():
            assert f"`{value}`" in doc, value
