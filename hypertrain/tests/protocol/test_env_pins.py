"""ADR 0011 part 3: the opt-in ``reference_spec.env.pinned`` and its closed, append-only table.

Torch-free on purpose: the manifest model, the table and the environment builder are used by the
launcher and the preflight, which must not import torch.
"""

from __future__ import annotations

import copy
import re
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from env_pin_fixtures import CPU_BF16, CPU_FP32
from hypertrain.protocol.env_pins import (
    ABSENT_ONLY,
    PRESENT_VALUES,
    EnvPin,
    divergence,
    pinned_environ,
)
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import EnvPins, RunManifest

UNPINNED_ID = "ad8beb30084a2e53f817c45f1cbfdf1491162575f10f2c3d190645081b2ba2ff"
CPU_FP32_ID = "14cc59c6012370b027ce1ec07128ca6734a068e27d07a777563d309465465b70"
SRC = Path(__file__).resolve().parents[2] / "src/hypertrain"

# The table as first decided. A later table may only grow: a signed manifest naming any of
# these keeps its meaning forever.
V1_PRESENT = {
    "ATEN_CPU_CAPABILITY": {"avx2"},
    "HT_ATTN_BACKEND": {"math", "sdpa-flash"},
    "MKL_CBWR": {"AVX2"},
    "OMP_DYNAMIC": {"FALSE"},
    "ONEDNN_MAX_CPU_ISA": {"AVX2"},
}
V1_ABSENT_ONLY = {"HT_COMPILE", "HT_FP8", "HT_MOE_GROUPED", "OMP_THREAD_LIMIT"}
# HT_* variables the trainer reads that no pin covers, with the reason.
UNPINNED_ON_PURPOSE = {"HT_ACT_CKPT": "bitwise equal, tests/trainer/test_act_ckpt.py"}


def manifest_body(pinned: dict[str, Any] | None) -> dict[str, Any]:
    body = example_manifest().body()
    if pinned is not None:
        body["reference_spec"]["env"]["pinned"] = copy.deepcopy(pinned)
    return body


def test_a_valid_pin_validates_and_enters_the_run_id() -> None:
    assert EnvPins.model_validate(CPU_FP32).present == CPU_FP32["present"]
    assert EnvPins.model_validate(CPU_BF16).present == CPU_BF16["present"]
    manifest = RunManifest.model_validate(manifest_body(CPU_FP32))
    assert manifest.body()["reference_spec"]["env"]["pinned"] == CPU_FP32
    assert manifest.reference_spec.env.pin() == EnvPin.from_body(CPU_FP32)
    assert manifest.run_id() == CPU_FP32_ID != UNPINNED_ID
    unpinned = RunManifest.model_validate(manifest_body(None))
    assert unpinned.run_id() == UNPINNED_ID
    assert unpinned.reference_spec.env.pin() is None


def _edit_present(name: str, value: Any) -> Callable[[dict[str, Any]], None]:
    return lambda pin: pin["present"].__setitem__(name, value)


def _reverse_absent(pin: dict[str, Any]) -> None:
    pin["absent"].reverse()


def _duplicate_absent(pin: dict[str, Any]) -> None:
    pin["absent"].insert(0, pin["absent"][0])


def _absent_also_present(pin: dict[str, Any]) -> None:
    pin["absent"] = sorted([*pin["absent"], "HT_ATTN_BACKEND"])


def _empty(pin: dict[str, Any]) -> None:
    pin["present"], pin["absent"] = {}, []


REFUSALS = [
    pytest.param(
        _edit_present("LD_PRELOAD", "/x"), "LD_PRELOAD='/x' is not an allowed pin", id="unknown"
    ),
    pytest.param(
        _edit_present("ATEN_CPU_CAPABILITY", "avx512"),
        "ATEN_CPU_CAPABILITY='avx512' is not an allowed pin",
        id="value-outside-table",
    ),
    pytest.param(
        _edit_present("MKL_CBWR", "AUTO"), "MKL_CBWR='AUTO' is not an allowed pin", id="mkl-auto"
    ),
    pytest.param(
        _edit_present("HT_FP8", "1"), "HT_FP8='1' is not an allowed pin", id="absent-only-present"
    ),
    pytest.param(
        lambda pin: pin["absent"].append("PATH"), "cannot name 'PATH' as absent", id="absent-path"
    ),
    pytest.param(_reverse_absent, "'absent' must be sorted and unique", id="absent-reversed"),
    pytest.param(_duplicate_absent, "'absent' must be sorted and unique", id="absent-duplicated"),
    pytest.param(
        _absent_also_present, "names 'HT_ATTN_BACKEND' both present and absent", id="both"
    ),
    pytest.param(_empty, "must name at least one variable", id="empty"),
    pytest.param(lambda pin: pin.pop("absent"), "Field required", id="absent-missing"),
    pytest.param(
        lambda pin: pin.__setitem__("note", "x"), "Extra inputs are not permitted", id="extra-key"
    ),
    pytest.param(
        _edit_present("OMP_DYNAMIC", 0), "Input should be a valid string", id="non-string-value"
    ),
]


@pytest.mark.parametrize(("edit", "message"), REFUSALS)
def test_refusals_are_one_edit_twins_of_a_valid_pin(
    edit: Callable[[dict[str, Any]], None], message: str
) -> None:
    # The unedited twin validates, so each refusal below is caused by its edit alone.
    RunManifest.model_validate(manifest_body(CPU_FP32))
    pin = copy.deepcopy(CPU_FP32)
    edit(pin)
    with pytest.raises(ValidationError, match=re.escape(message)):
        RunManifest.model_validate(manifest_body(pin))


def test_pinned_environ_and_divergence() -> None:
    pin = EnvPin.from_body(CPU_FP32)
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
    }
    assert pinned_environ(out, pin) == out
    assert divergence(out, pin) == {}
    assert divergence(base, pin) == {
        "ATEN_CPU_CAPABILITY": ("avx2", None),
        "HT_ATTN_BACKEND": ("math", None),
        "MKL_CBWR": ("AVX2", "COMPATIBLE"),
        "OMP_DYNAMIC": ("FALSE", None),
        "HT_FP8": (None, "0"),
        "OMP_THREAD_LIMIT": (None, "2"),
    }
    copied = pinned_environ(base, None)
    assert copied == base and copied is not base


def test_table_is_append_only() -> None:
    for name, values in V1_PRESENT.items():
        assert values <= PRESENT_VALUES[name], name
    assert V1_ABSENT_ONLY <= ABSENT_ONLY


def test_every_ht_read_is_pinned_or_waived() -> None:
    read = re.compile(r'(?:environ(?:\.get)?|getenv)[(\["]"(HT_[A-Z_0-9]+)"')
    names = {
        m.group(1)
        for folder in ("trainer", "models")
        for path in (SRC / folder).rglob("*.py")
        for m in read.finditer(path.read_text())
    }
    assert names, "the scan found no HT_* read: the pattern is stale"
    known = set(PRESENT_VALUES) | ABSENT_ONLY | set(UNPINNED_ON_PURPOSE)
    assert names <= known, sorted(names - known)


def test_presets_cover_the_switches() -> None:
    for preset in (CPU_FP32, CPU_BF16):
        assert ABSENT_ONLY <= set(preset["absent"])
        assert {"HT_ATTN_BACKEND", "OMP_DYNAMIC"} <= set(preset["present"])
