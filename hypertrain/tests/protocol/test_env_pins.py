"""ADR 0011 part 3: the required ``reference_spec.env.pinned``, its CPU and GPU classes and the
closed, append-only tables they come from.

Torch-free on purpose: the manifest model, the tables and the environment builder are used by the
launcher and the preflight, which must not import torch.
"""

from __future__ import annotations

import copy
import hashlib
import json
import re
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from env_pin_fixtures import CPU_AVX2, CPU_AVX512, honest_observation
from hypertrain.protocol.env_pins import (
    CPU_CLASSES,
    GPU_CLASSES,
    TRAINER_SWITCHES,
    EnvPin,
    class_pin,
    divergence,
    observation_divergence,
    pinned_environ,
)
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import EnvPins, RunManifest

SRC = Path(__file__).resolve().parents[2] / "src/hypertrain"

# The tables as first decided: sha256 of the manifest body of every (cpu, gpu, attention). A later
# table may only grow. A signed manifest naming one of these keeps its meaning forever.
FIRST_TABLE = {
    ("avx2", "none", "math"): "efd340341f534ddf93f457ca96f51796619049839c567740fdf384660495ab39",
    ("avx2", "sm_103", "math"): "c40e15de22bd60ed2042d2f6727a08cd1171d23ae37794720c6a431263e76b25",
    ("avx2", "sm_103", "sdpa-flash"): (
        "b501970963416870e22ca73e4b02fbc94adc08dfd8c42f52055edd21ebeda9cc"
    ),
    ("avx2", "sm_120", "math"): "37409ffb0205a08651be2714a64e8e0b23121b51afbe52963e66035f67a8fc25",
    ("avx2", "sm_120", "sdpa-flash"): (
        "0becbef098311c466e126cc9c0c8f1eac1c549ea89e2edd7757616520e1e41a1"
    ),
    ("avx512", "none", "math"): "18f8a1fd7a692c446350adf7bb41b62903f4aebf59b5267f6edade62f3947bfa",
    ("avx512", "sm_103", "math"): (
        "c444327c8f888757e5fbf8d9ab10d51893a3e212aa47ef32b5236ab6dfda193b"
    ),
    ("avx512", "sm_103", "sdpa-flash"): (
        "41fb66abdbbc27929d41926f97f0ec7de6c162712c007d897432e337c6f80763"
    ),
    ("avx512", "sm_120", "math"): (
        "e4093b5cb27aae641206e38966443bfc7dc6a7922c1cf6fdd728b0b6a038810a"
    ),
    ("avx512", "sm_120", "sdpa-flash"): (
        "c2b77c1302ed82dbcf6a2052485767a7c78ea0c4b8adb39650379dc4c61fd081"
    ),
}
# HT_* variables the trainer reads that no class covers, with the reason.
UNPINNED_ON_PURPOSE = {"HT_ACT_CKPT": "bitwise equal, tests/trainer/test_act_ckpt.py"}


def manifest_body(pinned: dict[str, Any] | None, sm_count: int | None = None) -> dict[str, Any]:
    body = example_manifest().body()
    env = body["reference_spec"]["env"]
    if pinned is None:
        del env["pinned"]
    else:
        env["pinned"] = copy.deepcopy(pinned)
    if sm_count is not None:
        body["reference_spec"]["sm_count"] = sm_count
    return body


def test_a_manifest_must_carry_the_pin() -> None:
    RunManifest.model_validate(manifest_body(CPU_AVX2))  # the unedited twin validates
    with pytest.raises(ValidationError, match=r"reference_spec\.env\.pinned\s+Field required"):
        RunManifest.model_validate(manifest_body(None))
    body = manifest_body(CPU_AVX2)
    body["reference_spec"]["env"]["pinned"] = None
    with pytest.raises(ValidationError, match=r"reference_spec\.env\.pinned"):
        RunManifest.model_validate(body)


def test_a_valid_pin_validates_and_enters_the_run_id() -> None:
    manifest = RunManifest.model_validate(manifest_body(CPU_AVX2))
    assert manifest.body()["reference_spec"]["env"]["pinned"] == CPU_AVX2
    assert manifest.reference_spec.env.pin() == EnvPin.from_body(CPU_AVX2)
    assert EnvPins.model_validate(CPU_AVX512).present["ATEN_CPU_CAPABILITY"] == "avx512"
    other = RunManifest.model_validate(manifest_body(CPU_AVX512))
    assert other.run_id() != manifest.run_id()
    assert example_manifest().run_id() == manifest.run_id()


@pytest.mark.parametrize("cpu", sorted(CPU_CLASSES))
@pytest.mark.parametrize("gpu", sorted(GPU_CLASSES))
def test_every_cpu_class_pairs_with_every_gpu_class(cpu: str, gpu: str) -> None:
    sm_counts = sorted(GPU_CLASSES[gpu].sm_counts)
    for attention in sorted(GPU_CLASSES[gpu].attention):
        pin = class_pin(cpu, gpu, attention)
        manifest = RunManifest.model_validate(
            manifest_body(pin, sm_counts[0] if sm_counts else None)
        )
        assert manifest.reference_spec.env.pin().gpu == gpu


def _edit_present(name: str, value: Any) -> Callable[[dict[str, Any]], None]:
    return lambda pin: pin["present"].__setitem__(name, value)


def _drop(key: str) -> Callable[[dict[str, Any]], None]:
    return lambda pin: pin.pop(key)


def _set(key: str, value: Any) -> Callable[[dict[str, Any]], None]:
    return lambda pin: pin.__setitem__(key, value)


def _reverse_absent(pin: dict[str, Any]) -> None:
    pin["absent"].reverse()


def _duplicate_absent(pin: dict[str, Any]) -> None:
    pin["absent"].insert(0, pin["absent"][0])


def _absent_also_present(pin: dict[str, Any]) -> None:
    pin["absent"] = sorted([*pin["absent"], "HT_ATTN_BACKEND"])


def _empty(pin: dict[str, Any]) -> None:
    pin["present"], pin["absent"] = {}, []


def _drop_present(name: str) -> Callable[[dict[str, Any]], None]:
    return lambda pin: pin["present"].pop(name)


def _drop_absent(name: str) -> Callable[[dict[str, Any]], None]:
    return lambda pin: pin["absent"].remove(name)


REFUSALS = [
    pytest.param(_drop("gpu"), "Field required", id="cpu-only-pin"),
    pytest.param(_drop("cpu"), "Field required", id="gpu-only-pin"),
    pytest.param(_set("gpu", None), "Input should be a valid string", id="gpu-null"),
    pytest.param(_set("cpu", "avx3"), "cpu class 'avx3' is not in the table", id="cpu-unknown"),
    pytest.param(_set("gpu", "sm_90"), "gpu class 'sm_90' is not in the table", id="gpu-unknown"),
    pytest.param(
        _set("cpu", "avx512"), "present differs from the classes avx512/none", id="isa-vs-names"
    ),
    pytest.param(_edit_present("LD_PRELOAD", "/x"), "differs from the classes", id="unknown-name"),
    pytest.param(
        _edit_present("ATEN_CPU_CAPABILITY", "default"),
        "differs from the classes avx2/none: ATEN_CPU_CAPABILITY",
        id="aten-outside-class",
    ),
    pytest.param(
        _edit_present("MKL_CBWR", "AUTO"),
        "differs from the classes avx2/none: MKL_CBWR",
        id="mkl-auto",
    ),
    pytest.param(
        _edit_present("ONEDNN_MAX_CPU_ISA", "ALL"),
        "differs from the classes avx2/none: ONEDNN_MAX_CPU_ISA",
        id="onednn-all",
    ),
    pytest.param(
        _edit_present("HT_FP8", "1"), "differs from the classes avx2/none: HT_FP8", id="fp8-on"
    ),
    pytest.param(
        _edit_present("HT_ATTN_BACKEND", "sdpa-flash"),
        "HT_ATTN_BACKEND='sdpa-flash' is not measured on gpu class 'none'",
        id="flash-without-a-gpu",
    ),
    pytest.param(
        _drop_present("MKL_CBWR"), "differs from the classes avx2/none: MKL_CBWR", id="mkl-missing"
    ),
    pytest.param(
        _drop_present("HT_ATTN_BACKEND"), "must pin HT_ATTN_BACKEND", id="attention-missing"
    ),
    pytest.param(
        _drop_absent("HT_FP8"), "absent differs from the classes avx2/none", id="fp8-not-pinned-off"
    ),
    pytest.param(
        lambda pin: pin["absent"].append("PATH"),
        "absent differs from the classes avx2/none",
        id="absent-path",
    ),
    pytest.param(_reverse_absent, "absent differs from the classes", id="absent-reversed"),
    pytest.param(_duplicate_absent, "absent differs from the classes", id="absent-duplicated"),
    pytest.param(_absent_also_present, "absent differs from the classes avx2/none", id="both"),
    pytest.param(_empty, "must pin HT_ATTN_BACKEND", id="empty"),
    pytest.param(_drop("absent"), "Field required", id="absent-missing"),
    pytest.param(_set("note", "x"), "Extra inputs are not permitted", id="extra-key"),
    pytest.param(
        _edit_present("OMP_DYNAMIC", 0), "Input should be a valid string", id="non-string-value"
    ),
]


@pytest.mark.parametrize(("edit", "message"), REFUSALS)
def test_refusals_are_one_edit_twins_of_a_valid_pin(
    edit: Callable[[dict[str, Any]], None], message: str
) -> None:
    # The unedited twin validates, so each refusal below is caused by its edit alone.
    RunManifest.model_validate(manifest_body(CPU_AVX2))
    pin = copy.deepcopy(CPU_AVX2)
    edit(pin)
    with pytest.raises(ValidationError, match=re.escape(message)):
        RunManifest.model_validate(manifest_body(pin))


def test_a_gpu_class_names_the_device_the_manifest_runs_on() -> None:
    pin = class_pin("avx2", "sm_103")
    RunManifest.model_validate(manifest_body(pin, 148))
    with pytest.raises(ValidationError, match="sm_count 170 is not measured on gpu class sm_103"):
        RunManifest.model_validate(manifest_body(pin, 170))
    with pytest.raises(ValidationError, match="sm_count 148 is not measured on gpu class sm_120"):
        RunManifest.model_validate(manifest_body(class_pin("avx2", "sm_120"), 148))


def test_pinned_environ_and_divergence() -> None:
    pin = EnvPin.from_body(CPU_AVX2)
    base = {
        "PATH": "/usr/bin",
        "MKL_CBWR": "COMPATIBLE",
        "HT_FP8": "0",
        "OMP_THREAD_LIMIT": "2",
        "HT_ACT_CKPT": "1",
    }
    snapshot = dict(base)
    out = pinned_environ(base, pin)
    assert base == snapshot
    assert out == {
        "PATH": "/usr/bin",
        "HT_ACT_CKPT": "1",
        "ATEN_CPU_CAPABILITY": "avx2",
        "HT_ATTN_BACKEND": "math",
        "MKL_CBWR": "AVX2",
        "OMP_DYNAMIC": "FALSE",
        "ONEDNN_MAX_CPU_ISA": "AVX2",
    }
    assert pinned_environ(out, pin) == out
    assert divergence(out, pin) == {}
    assert divergence(base, pin) == {
        "ATEN_CPU_CAPABILITY": ("avx2", None),
        "HT_ATTN_BACKEND": ("math", None),
        "MKL_CBWR": ("AVX2", "COMPATIBLE"),
        "OMP_DYNAMIC": ("FALSE", None),
        "ONEDNN_MAX_CPU_ISA": ("AVX2", None),
        "HT_FP8": (None, "0"),
        "OMP_THREAD_LIMIT": (None, "2"),
    }


def _env(name: str, value: str | None) -> Callable[[dict[str, Any]], None]:
    return lambda seen: seen["env"].__setitem__(name, value)


OBSERVED_DIVERGENCES = [
    pytest.param(_set("cpu_capability", "AVX512"), ["cpu_capability"], id="isa-resolved"),
    pytest.param(_env("ATEN_CPU_CAPABILITY", "avx512"), ["ATEN_CPU_CAPABILITY"], id="aten-name"),
    pytest.param(_env("MKL_CBWR", "COMPATIBLE"), ["MKL_CBWR"], id="mkl"),
    pytest.param(_env("MKL_CBWR", None), ["MKL_CBWR"], id="mkl-unset"),
    pytest.param(_env("ONEDNN_MAX_CPU_ISA", "ALL"), ["ONEDNN_MAX_CPU_ISA"], id="onednn"),
    pytest.param(_env("OMP_DYNAMIC", "TRUE"), ["OMP_DYNAMIC"], id="omp-dynamic"),
    pytest.param(_env("HT_ATTN_BACKEND", "sdpa-flash"), ["HT_ATTN_BACKEND"], id="attention"),
    pytest.param(_env("HT_FP8", "1"), ["HT_FP8"], id="fp8"),
    pytest.param(_env("OMP_THREAD_LIMIT", "2"), ["OMP_THREAD_LIMIT"], id="thread-limit"),
    pytest.param(_set("gpu", "sm_103"), ["gpu"], id="gpu-sm"),
    pytest.param(_env("LD_PRELOAD", "/x"), ["LD_PRELOAD"], id="name-outside-the-pin"),
    pytest.param(_drop("env"), sorted(CPU_AVX2["present"]), id="no-env"),
    pytest.param(
        lambda seen: (seen["env"].__setitem__("HT_FP8", "1"), seen.__setitem__("gpu", "sm_120")),
        ["HT_FP8", "gpu"],
        id="two",
    ),
]


@pytest.mark.parametrize(("edit", "names"), OBSERVED_DIVERGENCES)
def test_an_observation_that_differs_names_every_name_that_differs(
    edit: Callable[[dict[str, Any]], None], names: list[str]
) -> None:
    pin = EnvPin.from_body(CPU_AVX2)
    seen = honest_observation(pin)
    assert observation_divergence(pin, seen) == []  # the unedited twin conforms
    edit(seen)
    assert observation_divergence(pin, seen) == names


def test_a_gpu_pin_expects_its_device_and_the_cpu_pin_beside_it() -> None:
    pin = EnvPin.from_body(class_pin("avx512", "sm_103", "sdpa-flash"))
    seen = honest_observation(pin)
    assert seen["gpu"] == "sm_103" and seen["cpu_capability"] == "AVX512"
    assert observation_divergence(pin, seen) == []
    assert observation_divergence(pin, {**seen, "gpu": "none"}) == ["gpu"]


def test_the_class_tables_are_append_only() -> None:
    # Rows never change: what a signed manifest named stays what it was. The table may only grow.
    digest = {
        (cpu, gpu, attention): hashlib.sha256(
            json.dumps(class_pin(cpu, gpu, attention), sort_keys=True).encode()
        ).hexdigest()
        for cpu in CPU_CLASSES
        for gpu in GPU_CLASSES
        for attention in sorted(GPU_CLASSES[gpu].attention)
    }
    assert FIRST_TABLE.items() <= digest.items()
    assert GPU_CLASSES["sm_103"].device == (10, 3) and GPU_CLASSES["sm_103"].sm_counts == {148}
    assert GPU_CLASSES["sm_120"].device == (12, 0) and GPU_CLASSES["sm_120"].sm_counts == {170}
    assert GPU_CLASSES["none"].device is None and GPU_CLASSES["none"].attention == {"math"}


def test_every_ht_read_is_pinned_or_waived() -> None:
    read = re.compile(r'(?:environ(?:\.get)?|getenv)[(\["]"(HT_[A-Z_0-9]+)"')
    names = {
        m.group(1)
        for folder in ("trainer", "models")
        for path in (SRC / folder).rglob("*.py")
        for m in read.finditer(path.read_text())
    }
    assert names, "the scan found no HT_* read: the pattern is stale"
    pinned = {HT for HT in (*TRAINER_SWITCHES, "HT_ATTN_BACKEND")}
    assert names <= pinned | set(UNPINNED_ON_PURPOSE), sorted(names - pinned)
